//! Read-only view of a local session's conversation, opened from the sessions list, with buttons
//! to continue it in a new workspace or as a new tab.

use super::Workbench;
use crate::i18n::{t, tf};
use crate::launch::{home_dir, LaunchSpec};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, now_ms, relative_time, tilde, IconSize, TypeScale};
use agentty_bridge::model::{Role, SessionInfo, Turn};
use gpui::{div, list, prelude::*, px, AnyElement, App, ClickEvent, Context, ListAlignment, ListState, SharedString, Window};
use std::path::PathBuf;
use std::rc::Rc;

/// Turns rendered at most (the newest ones).
const MAX_TURNS: usize = 200;

pub struct SessionViewer {
    pub session: SessionInfo,
    /// The newest [`MAX_TURNS`] turns; `None` while loading, `Err` when the transcript can't be read.
    pub turns: Option<Result<Rc<Vec<Turn>>, String>>,
    /// Older turns left out.
    skipped: usize,
    /// Virtual list: only the turns on screen are laid out, so long sessions scroll smoothly.
    list: ListState,
}

impl Workbench {
    pub(super) fn open_session_viewer(&mut self, session: SessionInfo, cx: &mut Context<Self>) {
        if self.session_viewer.as_ref().is_some_and(|v| v.session.path == session.path) {
            return;
        }
        let (agent, id) = (session.agent, session.id.clone());
        self.session_viewer =
            Some(SessionViewer { session, turns: None, skipped: 0, list: ListState::new(0, ListAlignment::Bottom, px(600.)) });
        self.page = None;
        let load_id = id.clone();
        let task =
            cx.background_spawn(async move { agentty_bridge::load(agent, &load_id).map(|(_, turns)| turns).map_err(|e| format!("{e:#}")) });
        cx.spawn(async move |this, cx| {
            let turns = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(viewer) = this.session_viewer.as_mut().filter(|v| v.session.id == id) {
                    let turns = turns.map(|mut turns| {
                        viewer.skipped = turns.len().saturating_sub(MAX_TURNS);
                        Rc::new(turns.split_off(viewer.skipped))
                    });
                    // Bottom-aligned: it opens at the end, where the conversation left off.
                    let rows = turns.as_ref().map_or(0, |t| t.len() + usize::from(viewer.skipped > 0));
                    viewer.list = ListState::new(rows, ListAlignment::Bottom, px(600.));
                    viewer.turns = Some(turns);
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
                            .font_weight(crate::theme::EMPHASIS)
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
                let (turns, skipped, agent) = (turns.clone(), viewer.skipped, session.agent);
                let notice = (skipped > 0).then(|| tf(cx, "viewer.skipped", &[("n", &skipped.to_string())]));
                let rows = list(viewer.list.clone(), move |row, _, cx| {
                    // Each row is laid out only while on screen; spacing is padding, not a gap.
                    let item = div().px_5().pt_3().max_w(px(920.));
                    match (&notice, row) {
                        (Some(text), 0) => item.pt_4().t_small().text_color(hex(Chrome::MUTED)).child(text.clone()).into_any_element(),
                        _ => {
                            let index = row - usize::from(notice.is_some());
                            match turns.get(index) {
                                Some(turn) => item.child(render_turn(index, turn, agent, cx)).into_any_element(),
                                None => item.into_any_element(),
                            }
                        }
                    }
                })
                .size_full()
                .pb_4();
                div()
                    .relative()
                    .group(crate::ui::SCROLL_GROUP)
                    .flex_1()
                    .min_h_0()
                    .child(rows)
                    .child(crate::ui::list_scrollbar(viewer.list.clone()))
                    .into_any_element()
            }
        };
        Some(div().size_full().flex().flex_col().bg(hex(Chrome::EDITOR)).child(header).child(body).into_any_element())
    }
}

fn render_turn(index: usize, turn: &Turn, agent: agentty_bridge::model::Agent, cx: &App) -> AnyElement {
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
                .child(div().t_small().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::MUTED)).child(label))
                .child(text),
        )
        .into_any_element()
}
