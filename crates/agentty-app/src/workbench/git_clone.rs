//! "Clone from Git" (the Git button in the tab strip): clones a repository into the folder the
//! terminal in front is in. With the `gh` CLI signed in the field also searches GitHub — the
//! user's own repositories first — and a pick clones it; without it (or for any other host) the
//! field takes an https / ssh / git address. A repository that is there already is not cloned
//! again: the dialog says where it is.

use super::Workbench;
use crate::i18n::{t, tf};
use crate::launch::{LaunchSpec, PaneKind};
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, tilde, TypeScale};
use agentty_bridge::clone::{RemoteRepo, Source};
use gpui::{div, prelude::*, px, AnyElement, AppContext, ClickEvent, Context, Entity, SharedString, Subscription, Task, Window};
use std::path::PathBuf;
use std::time::Duration;

pub(super) struct CloneDialog {
    /// Where the clone goes: the folder of the terminal in front when the dialog opened.
    dir: PathBuf,
    input: Entity<TextInput>,
    _input_events: Subscription,
    /// `None` while finding out whether `gh` is there and signed in.
    gh: Option<bool>,
    mine: Vec<RemoteRepo>,
    results: Vec<RemoteRepo>,
    search: Option<Task<()>>,
    cloning: Option<String>,
    /// The last outcome: (text, is an error, the folder cloned into).
    status: Option<(String, bool, Option<PathBuf>)>,
}

impl Workbench {
    pub(super) fn open_clone_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dir) = self
            .active_pane()
            .map(|pane| pane.read(cx).display_cwd())
            .or_else(|| self.workspaces.get(self.active_workspace).map(|ws| ws.cwd.clone()))
        else {
            return;
        };
        let input = cx.new(|cx| TextInput::localized("", "clone.placeholder", window, cx));
        let events = cx.subscribe(&input, |this, input, event: &TextInputEvent, cx| match event {
            TextInputEvent::Changed => {
                let query = input.read(cx).text().to_string();
                this.search_repos(query, cx);
            }
            TextInputEvent::Confirmed => {
                let text = input.read(cx).text().to_string();
                this.clone_from_text(&text, cx);
            }
            TextInputEvent::Cancelled => this.close_clone_dialog(cx),
            _ => {}
        });
        window.focus(&gpui::Focusable::focus_handle(&input, cx));
        self.clone_dialog = Some(CloneDialog {
            dir,
            input,
            _input_events: events,
            gh: None,
            mine: Vec::new(),
            results: Vec::new(),
            search: None,
            cloning: None,
            status: None,
        });
        cx.spawn(async move |this, cx| {
            let (ready, mine) = cx
                .background_spawn(async move {
                    let ready = agentty_bridge::clone::gh_ready();
                    (ready, if ready { agentty_bridge::clone::my_repos() } else { Vec::new() })
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(dialog) = this.clone_dialog.as_mut() {
                    dialog.gh = Some(ready);
                    dialog.results = mine.clone();
                    dialog.mine = mine;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn close_clone_dialog(&mut self, cx: &mut Context<Self>) {
        if self.clone_dialog.take().is_some() {
            // The keyboard goes back to the terminal the dialog was opened over.
            self.refocus = true;
            cx.notify();
        }
    }

    /// Searches GitHub a moment after typing stops; an address is not searched for.
    fn search_repos(&mut self, query: String, cx: &mut Context<Self>) {
        let Some(dialog) = self.clone_dialog.as_mut() else { return };
        if dialog.gh != Some(true) {
            return;
        }
        let query = query.trim().to_string();
        let is_address = matches!(agentty_bridge::clone::parse_source(&query), Ok(Source::Url(_)));
        if query.is_empty() || is_address {
            dialog.results = if is_address { Vec::new() } else { dialog.mine.clone() };
            dialog.search = None;
            return cx.notify();
        }
        let mine = dialog.mine.clone();
        dialog.search = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(350)).await;
            let found = cx.background_spawn(async move { agentty_bridge::clone::search_repos(&query, &mine) }).await;
            let _ = this.update(cx, |this, cx| {
                if let Some(dialog) = this.clone_dialog.as_mut() {
                    dialog.results = found;
                    dialog.search = None;
                    cx.notify();
                }
            });
        }));
        cx.notify();
    }

    fn clone_from_text(&mut self, text: &str, cx: &mut Context<Self>) {
        match agentty_bridge::clone::parse_source(text) {
            Ok(source) => self.start_clone(source, cx),
            Err(err) => {
                if let Some(dialog) = self.clone_dialog.as_mut() {
                    dialog.status = Some((format!("{err:#}"), true, None));
                    cx.notify();
                }
            }
        }
    }

    fn start_clone(&mut self, source: Source, cx: &mut Context<Self>) {
        let Some(dialog) = self.clone_dialog.as_mut() else { return };
        if dialog.cloning.is_some() {
            return;
        }
        let dir = dialog.dir.clone();
        // Said before anything runs, so an existing copy is never cloned over or next to.
        if let Some(existing) = agentty_bridge::clone::already_there(&dir, &source) {
            dialog.status = Some((tf(cx, "clone.exists", &[("path", &tilde(&existing))]), true, None));
            return cx.notify();
        }
        dialog.cloning = source.folder_name();
        dialog.status = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { agentty_bridge::clone::clone(&dir, &source) }).await;
            let _ = this.update(cx, |this, cx| {
                if let Some(dialog) = this.clone_dialog.as_mut() {
                    dialog.cloning = None;
                    dialog.status = Some(match result {
                        Ok(path) => (tf(cx, "clone.done", &[("path", &tilde(&path))]), false, Some(path)),
                        Err(err) => (format!("{err:#}"), true, None),
                    });
                }
                this.refresh_files_panel(cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn render_clone_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.clone_dialog.as_ref()?;
        let busy = dialog.cloning.is_some();
        let hint = match dialog.gh {
            None => t(cx, "clone.checking_gh"),
            Some(true) => t(cx, "clone.hint_gh"),
            Some(false) => t(cx, "clone.hint_url"),
        };
        let button = |id: &'static str, label: String, primary: bool| {
            div()
                .id(id)
                .flex_shrink_0()
                .px_3()
                .py_1p5()
                .rounded_md()
                .t_body()
                .cursor_pointer()
                .bg(if primary { hex(Chrome::ACCENT) } else { hex(0x2d2d30) })
                .text_color(hex(Chrome::BRIGHT))
                .hover(|s| s.opacity(0.85))
                .child(label)
        };
        let focus = dialog.input.clone();
        let field = div()
            .flex_1()
            .min_w_0()
            .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
                window.focus(&gpui::Focusable::focus_handle(&focus, cx));
                // The workbench behind tracks focus too and would take it back on the same click.
                window.prevent_default();
                cx.stop_propagation();
            })
            .cursor_text()
            .px_2()
            .py_1p5()
            .rounded_md()
            .border_1()
            .border_color(hex(Chrome::BORDER))
            .bg(hex(0x1a1a1a))
            .t_body()
            .child(dialog.input.clone());
        let mut list = div().id("clone-results").max_h(px(300.)).overflow_y_scroll().flex().flex_col().gap_0p5();
        if dialog.gh == Some(true) {
            if dialog.search.is_some() {
                list = list.child(crate::ui::loading_row(t(cx, "clone.searching")));
            }
            for (index, repo) in dialog.results.iter().enumerate() {
                let full_name = repo.full_name.clone();
                list = list.child(
                    div()
                        .id(SharedString::from(format!("clone-repo-{index}")))
                        .px_2()
                        .py_1p5()
                        .rounded_md()
                        .cursor_pointer()
                        .hover(|s| s.bg(hex(Chrome::HOVER)))
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(icon(if repo.private { "lock" } else { "git-fork" }, 12., hex(Chrome::MUTED)))
                        .child(div().flex_shrink_0().t_body().text_color(hex(Chrome::BRIGHT)).child(repo.full_name.clone()))
                        .child(div().min_w_0().truncate().t_small().text_color(hex(Chrome::MUTED)).child(repo.description.clone()))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.start_clone(Source::GitHub(full_name.clone()), cx);
                        })),
                );
            }
        }
        let status = if let Some(name) = &dialog.cloning {
            Some(crate::ui::loading_row(tf(cx, "clone.cloning", &[("name", name)])).into_any_element())
        } else {
            dialog.status.as_ref().map(|(text, error, cloned)| {
                let cloned = cloned.clone();
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .t_small()
                            .text_color(hex(if *error { Chrome::ERROR } else { Chrome::SUCCESS }))
                            .child(text.clone()),
                    )
                    .when_some(cloned, |d, path| {
                        d.child(button("clone-open", t(cx, "clone.open").to_string(), false).on_click(cx.listener(
                            move |this, _: &ClickEvent, window, cx| {
                                this.clone_dialog = None;
                                this.open_tab(LaunchSpec::new(PaneKind::Shell, path.clone()), window, cx);
                            },
                        )))
                    })
                    .into_any_element()
            })
        };
        Some(
            div()
                .id("clone-dialog-overlay")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(hex_alpha(0x000000, 0.45))
                .occlude()
                .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, _, _, cx| this.close_clone_dialog(cx)))
                .child(
                    div()
                        .id("clone-dialog")
                        .w(px(620.))
                        .max_h(gpui::relative(0.9))
                        .p_5()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .rounded_xl()
                        .bg(hex(Chrome::OVERLAY))
                        .border_1()
                        .border_color(hex(Chrome::OVERLAY_BORDER))
                        .shadow_lg()
                        // Clicks inside the dialog are its own, not a click outside that closes it.
                        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(icon("git-branch", crate::ui::IconSize::BUTTON, hex(Chrome::BRIGHT)))
                                .child(
                                    div()
                                        .t_title()
                                        .font_weight(crate::theme::EMPHASIS)
                                        .text_color(hex(Chrome::BRIGHT))
                                        .child(t(cx, "clone.title")),
                                ),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .flex()
                                .items_center()
                                .gap_1()
                                .t_small()
                                .text_color(hex(Chrome::MUTED))
                                .child(icon("folder", 12., hex(Chrome::MUTED)))
                                // A deep folder is cut at the end rather than run out of the dialog.
                                .child(div().min_w_0().truncate().child(tf(cx, "clone.into", &[("folder", &tilde(&dialog.dir))]))),
                        )
                        .child(div().flex().items_center().gap_2().child(field).child(
                            button("clone-start", t(cx, "clone.start").to_string(), true).when(busy, |d| d.opacity(0.5)).on_click(
                                cx.listener(|this, _: &ClickEvent, _, cx| {
                                    let Some(text) = this.clone_dialog.as_ref().map(|d| d.input.read(cx).text().to_string()) else {
                                        return;
                                    };
                                    this.clone_from_text(&text, cx);
                                }),
                            ),
                        ))
                        .child(div().t_small().text_color(hex(Chrome::MUTED)).child(hint))
                        .children(status)
                        .child(list),
                )
                .into_any_element(),
        )
    }
}
