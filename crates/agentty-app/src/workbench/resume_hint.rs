//! When a terminal pane sits in a folder with earlier Claude Code / Codex sessions, a slim bar
//! offers to continue them right there — no need to remember `claude -r`.

use super::{Pane, Workbench};
use crate::i18n::{t, tf};
use crate::launch::{home_dir, LaunchSpec};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, now_ms, popover, relative_time, IconSize, TypeScale};
use agentty_bridge::model::SessionInfo;
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, SharedString, Window};
use std::path::Path;

const MAX_LISTED: usize = 8;

impl Workbench {
    /// Earlier sessions started in `dir`, newest first.
    pub(super) fn sessions_in(&self, dir: &Path) -> Vec<&SessionInfo> {
        let mut found: Vec<&SessionInfo> = self.sessions.iter().filter(|s| s.cwd.as_deref().is_some_and(|c| Path::new(c) == dir)).collect();
        found.sort_by_key(|s| std::cmp::Reverse(s.updated_at));
        found
    }

    /// Replaces a shell pane with the resumed session, in the same place of the layout.
    pub(super) fn resume_in_pane(&mut self, pane: &Pane, session: SessionInfo, window: &mut Window, cx: &mut Context<Self>) {
        self.resume_menu = None;
        let Some((w, t)) = self.locate(pane) else { return };
        let cwd = session.cwd.as_ref().map(std::path::PathBuf::from).filter(|p| p.is_dir()).unwrap_or_else(home_dir);
        let spec = LaunchSpec::resume(session.agent, session.id.clone(), session.title.clone(), cwd);
        let resumed = self.spawn_pane(spec, cx);
        let tab = &mut self.workspaces[w].tabs[t];
        tab.root = tab.root.map(&mut |p: &Pane| if p == pane { resumed.clone() } else { p.clone() });
        if tab.active == *pane {
            tab.active = resumed.clone();
        }
        // Dropping the old view shuts its shell down.
        self.pane_subscriptions.remove(&pane.entity_id());
        self.persist(cx);
        self.focus_pane(&resumed, window, cx);
        cx.notify();
    }

    pub(super) fn render_resume_hint(&self, pane: &Pane, cx: &mut Context<Self>) -> Option<AnyElement> {
        let view = pane.read(cx);
        if view.is_agent() || !view.is_running() || !crate::settings::settings(cx).resume_bar {
            return None;
        }
        let dir = view.display_cwd();
        if dir == home_dir() || self.resume_dismissed.contains(&(view.pane_id, dir.clone())) {
            return None;
        }
        let sessions = self.sessions_in(&dir);
        let latest = (*sessions.first()?).clone();
        let pane_id = view.pane_id;
        let now = now_ms();
        let (resume_pane, dismiss_dir) = (pane.clone(), dir.clone());
        let menu_open = self.resume_menu == Some(pane_id);

        let mut bar = div()
            .relative()
            .flex_shrink_0()
            .h(px(30.))
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .bg(hex_alpha(Chrome::ACCENT, 0.12))
            .border_b_1()
            .border_color(hex_alpha(Chrome::ACCENT, 0.35))
            .t_small()
            .child(icon("history", IconSize::INLINE, hex(Chrome::BLUE)))
            .child(crate::brand::avatar(latest.agent.id(), 16.))
            .child(div().flex_shrink().min_w_0().truncate().text_color(hex(Chrome::FOREGROUND)).child(tf(
                cx,
                "resume.hint",
                &[("n", &sessions.len().to_string()), ("title", &latest.title), ("ago", &relative_time(now, latest.updated_at))],
            )))
            // What it was last about, so similar titles can be told apart.
            .when_some(latest.last_prompt.clone(), |d, prompt| {
                d.child(div().flex_1().min_w_0().truncate().text_color(hex(Chrome::MUTED)).child(format!("“{prompt}”")))
            })
            .when(latest.last_prompt.is_none(), |d| d.child(div().flex_1()))
            .child(
                div()
                    .id(("resume-latest", pane_id as usize))
                    .flex_shrink_0()
                    .px_2()
                    .py_0p5()
                    .rounded_sm()
                    .bg(hex(Chrome::ACCENT))
                    .text_color(hex(Chrome::BRIGHT))
                    .cursor_pointer()
                    .hover(|s| s.bg(hex(0x1a8ae6)))
                    .child(t(cx, "resume.continue"))
                    .on_click(
                        cx.listener(move |this, _: &ClickEvent, window, cx| this.resume_in_pane(&resume_pane, latest.clone(), window, cx)),
                    ),
            );
        if sessions.len() > 1 {
            bar = bar.child(
                div()
                    .id(("resume-more", pane_id as usize))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap_0p5()
                    .px_2()
                    .py_0p5()
                    .rounded_sm()
                    .text_color(hex(Chrome::FOREGROUND))
                    .cursor_pointer()
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .child(t(cx, "resume.choose"))
                    .child(icon("chevron-down", 12., hex(Chrome::MUTED)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        if this.just_dismissed("resume-menu") {
                            return;
                        }
                        this.resume_menu = if this.resume_menu == Some(pane_id) { None } else { Some(pane_id) };
                        cx.notify();
                    })),
            );
        }
        bar = bar.child(crate::ui::icon_only(
            ("resume-dismiss", pane_id as usize),
            "x",
            cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.resume_dismissed.insert((pane_id, dismiss_dir.clone()));
                this.resume_menu = None;
                cx.notify();
            }),
        ));

        if menu_open {
            let mut list = popover().w(px(460.)).on_mouse_down_out(cx.listener(|this, _, _, cx| {
                if this.resume_menu.take().is_some() {
                    this.note_dismissed("resume-menu");
                }
                cx.notify();
            }));
            for (index, session) in sessions.iter().take(MAX_LISTED).enumerate() {
                let (session_pane, chosen) = (pane.clone(), (*session).clone());
                let summary = session_summary(session);
                list = list.child(
                    div()
                        .id(SharedString::from(format!("resume-item-{pane_id}-{index}")))
                        .px_2()
                        .py_1p5()
                        .rounded_md()
                        .cursor_pointer()
                        .flex()
                        .items_start()
                        .gap_2()
                        .hover(|s| s.bg(hex(Chrome::ACCENT)))
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                            this.resume_in_pane(&session_pane, chosen.clone(), window, cx)
                        }))
                        .child(div().pt_0p5().child(crate::brand::avatar(session.agent.id(), 18.)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap_0p5()
                                .child(
                                    div()
                                        .flex()
                                        .gap_2()
                                        .t_body()
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .truncate()
                                                .text_color(hex(Chrome::BRIGHT))
                                                .child(session.title.clone()),
                                        )
                                        .child(
                                            div()
                                                .flex_shrink_0()
                                                .t_small()
                                                .text_color(hex(Chrome::MUTED))
                                                .child(relative_time(now, session.updated_at)),
                                        ),
                                )
                                .children(summary.map(|text| div().t_small().truncate().text_color(hex(Chrome::MUTED)).child(text))),
                        ),
                );
            }
            bar = bar.child(div().absolute().top(px(30.)).right(px(8.)).child(gpui::deferred(list).with_priority(2)));
        }
        Some(bar.into_any_element())
    }
}

/// One line telling a session apart from similarly titled ones: its last prompt, else last reply.
pub fn session_summary(session: &SessionInfo) -> Option<String> {
    match (&session.last_prompt, &session.last_reply) {
        (Some(prompt), Some(reply)) => Some(format!("› {prompt}  ·  {reply}")),
        (Some(prompt), None) => Some(format!("› {prompt}")),
        (None, Some(reply)) => Some(reply.clone()),
        (None, None) => None,
    }
}
