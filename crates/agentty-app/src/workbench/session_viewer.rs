//! Read-only view of a local session's conversation, opened from the sessions list, with buttons
//! to continue it in a new workspace or as a new tab.

use super::Workbench;
use crate::i18n::{t, tf};
use crate::launch::{home_dir, LaunchSpec};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, now_ms, relative_time, tilde, IconSize, TypeScale};
use agentty_bridge::model::{Role, SessionInfo, Turn};
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, FontWeight, SharedString, Window};
use std::path::PathBuf;

/// Turns rendered at most (the newest ones).
const MAX_TURNS: usize = 200;

pub struct SessionViewer {
    pub session: SessionInfo,
    /// `None` while loading; `Err` when the transcript can't be read.
    pub turns: Option<Result<Vec<Turn>, String>>,
    pub scroll: gpui::ScrollHandle,
}

impl Workbench {
    pub(super) fn open_session_viewer(&mut self, session: SessionInfo, cx: &mut Context<Self>) {
        if self.session_viewer.as_ref().is_some_and(|v| v.session.path == session.path) {
            return;
        }
        let (agent, id) = (session.agent, session.id.clone());
        self.session_viewer = Some(SessionViewer { session, turns: None, scroll: gpui::ScrollHandle::new() });
        self.page = None;
        let load_id = id.clone();
        let task =
            cx.background_spawn(async move { agentty_bridge::load(agent, &load_id).map(|(_, turns)| turns).map_err(|e| format!("{e:#}")) });
        cx.spawn(async move |this, cx| {
            let turns = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(viewer) = this.session_viewer.as_mut().filter(|v| v.session.id == id) {
                    viewer.turns = Some(turns);
                    // Start at the end, where the conversation left off.
                    viewer.scroll.scroll_to_bottom();
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Continues the session as a new tab of the active workspace.
    pub(super) fn resume_session_in_tab(&mut self, session: &SessionInfo, window: &mut Window, cx: &mut Context<Self>) {
        let cwd = session.cwd.as_ref().map(PathBuf::from).filter(|p| p.is_dir()).unwrap_or_else(home_dir);
        let spec = LaunchSpec::resume(session.agent, session.id.clone(), session.title.clone(), cwd);
        self.session_viewer = None;
        self.open_tab(spec, window, cx);
    }

    pub(super) fn render_session_viewer(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let viewer = self.session_viewer.as_ref()?;
        let session = &viewer.session;
        let now = now_ms();
        let (resume, resume_tab) = (session.clone(), session.clone());
        let has_workspace = !self.workspaces.is_empty();
        let button = |id: &'static str, label: String, primary: bool| {
            div()
                .id(id)
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap_1p5()
                .px_2p5()
                .py_1()
                .rounded_md()
                .t_small()
                .cursor_pointer()
                .bg(if primary { hex(Chrome::ACCENT) } else { hex(0x2d2d30) })
                .text_color(hex(Chrome::BRIGHT))
                .hover(|s| s.opacity(0.85))
                .child(label)
        };
        let header = div()
            .flex_shrink_0()
            .px_5()
            .py_3()
            .flex()
            .items_center()
            .gap_3()
            .border_b_1()
            .border_color(hex(Chrome::BORDER))
            .child(crate::brand::avatar(session.agent.id(), 28.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(
                        div()
                            .t_title()
                            .font_weight(FontWeight::SEMIBOLD)
                            .truncate()
                            .text_color(hex(Chrome::BRIGHT))
                            .child(session.title.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .t_small()
                            .text_color(hex(Chrome::MUTED))
                            .child(session.agent.display_name())
                            .child("·")
                            .child(relative_time(now, session.updated_at))
                            .children(
                                session.cwd.as_ref().map(|c| div().truncate().child(format!("· {}", tilde(std::path::Path::new(c))))),
                            ),
                    ),
            )
            .child(button("viewer-resume-workspace", t(cx, "viewer.resume_workspace").into(), true).on_click(cx.listener(
                move |this, _: &ClickEvent, window, cx| {
                    this.session_viewer = None;
                    this.resume_session(&resume, window, cx);
                },
            )))
            .when(has_workspace, |d| {
                d.child(
                    button("viewer-resume-tab", t(cx, "viewer.resume_tab").into(), false)
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.resume_session_in_tab(&resume_tab, window, cx))),
                )
            })
            .child(crate::ui::icon_only(
                "viewer-close",
                "x",
                cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.session_viewer = None;
                    this.focus_active(window, cx);
                    cx.notify();
                }),
            ));

        let body: AnyElement = match &viewer.turns {
            None => crate::ui::loading_row(t(cx, "viewer.loading")).into_any_element(),
            Some(Err(error)) => div().p_5().t_body().text_color(hex(Chrome::ERROR)).child(error.clone()).into_any_element(),
            Some(Ok(turns)) if turns.is_empty() => {
                div().p_5().t_body().text_color(hex(Chrome::MUTED)).child(t(cx, "viewer.empty")).into_any_element()
            }
            Some(Ok(turns)) => {
                let skipped = turns.len().saturating_sub(MAX_TURNS);
                let mut list = div().px_5().py_4().flex().flex_col().gap_3().max_w(px(920.));
                if skipped > 0 {
                    list = list.child(div().t_small().text_color(hex(Chrome::MUTED)).child(tf(
                        cx,
                        "viewer.skipped",
                        &[("n", &skipped.to_string())],
                    )));
                }
                for (index, turn) in turns.iter().skip(skipped).enumerate() {
                    list = list.child(render_turn(index, turn, session.agent, cx));
                }
                let scroll = viewer.scroll.clone();
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(div().id("session-viewer-scroll").size_full().overflow_y_scroll().track_scroll(&scroll).child(list))
                    .child(crate::ui::scrollbar(scroll))
                    .into_any_element()
            }
        };
        Some(div().size_full().flex().flex_col().bg(hex(Chrome::EDITOR)).child(header).child(body).into_any_element())
    }
}

fn render_turn(index: usize, turn: &Turn, agent: agentty_bridge::model::Agent, cx: &mut Context<Workbench>) -> AnyElement {
    let user = turn.role == Role::User;
    let mut text = div().flex().flex_col().gap_0p5().t_body().text_color(hex(if user { Chrome::BRIGHT } else { Chrome::FOREGROUND }));
    for line in turn.text.lines().take(400) {
        // Tool calls read as compact chips.
        let tool = line.starts_with("[tool: ");
        text = text.child(if tool {
            div()
                .flex()
                .items_center()
                .gap_1()
                .t_small()
                .text_color(hex(Chrome::MUTED))
                .child(icon("square-terminal", 12., hex(Chrome::MUTED)))
                .child(div().truncate().child(line.trim_start_matches("[tool: ").trim_end_matches(']').to_string()))
        } else {
            div().child(if line.is_empty() { " ".to_string() } else { line.to_string() })
        });
    }
    let (id, label) = if user {
        ("you", t(cx, "viewer.you").to_string())
    } else {
        (crate::brand::kind_id(agent.into()), agent.display_name().to_string())
    };
    div()
        .id(SharedString::from(format!("turn-{index}")))
        .flex()
        .gap_3()
        .p_3()
        .rounded_lg()
        .when(user, |d| d.bg(hex_alpha(Chrome::ACCENT, 0.12)).border_1().border_color(hex_alpha(Chrome::ACCENT, 0.3)))
        .child(if user {
            div().flex_shrink_0().size(px(22.)).rounded_full().bg(hex(0x3a3a3c)).flex().items_center().justify_center().child(icon(
                "users",
                IconSize::INLINE - 2.,
                hex(Chrome::BRIGHT),
            ))
        } else {
            crate::brand::avatar(id, 22.)
        })
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().t_small().font_weight(FontWeight::SEMIBOLD).text_color(hex(Chrome::MUTED)).child(label))
                .child(text),
        )
        .into_any_element()
}
