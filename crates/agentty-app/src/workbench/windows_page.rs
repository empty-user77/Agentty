//! Settings → Workspaces: every Agentty window, each with its own list of workspaces. A window
//! can be brought forward, reopened when it was closed, and deleted (after asking) with the
//! workspaces it holds.

use super::ask::{Ask, AskAction, AskChoice};
use super::persist::{ClosedWindows, LayoutState};
use super::Workbench;
use crate::i18n::{t, tf};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{action_button, relative_time, TypeScale};
use gpui::{div, prelude::*, px, ClickEvent, Context, SharedString, Window};

/// How long what was read of the closed windows' files is shown before they are read again.
const SAVED_FRESH: std::time::Duration = std::time::Duration::from_secs(3);

/// One workspace as the page lists it.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct WindowWorkspace {
    pub title: String,
    pub group: Option<String>,
    /// Its terminals run something (only known for an open window).
    pub running: bool,
}

/// A window and what is in it.
#[derive(Clone, Debug)]
pub(super) struct WindowSummary {
    pub slot: usize,
    /// Open now (else it was closed and can be reopened).
    pub open: bool,
    /// The window this page is shown in.
    pub current: bool,
    pub closed_at_ms: Option<u64>,
    pub workspaces: Vec<WindowWorkspace>,
    /// Terminals still running in it; deleting the window ends them.
    pub running: usize,
    /// Files with unsaved changes in its editor; deleting the window drops the changes.
    pub unsaved: usize,
}

/// Closed windows, as read from their layout files.
pub(super) struct SavedWindows {
    read_at: std::time::Instant,
    windows: Vec<WindowSummary>,
}

/// The workspaces of a layout that is not open: their names (or folder), and their groups'.
fn saved_workspaces(state: &LayoutState) -> Vec<WindowWorkspace> {
    state
        .workspaces
        .iter()
        .map(|ws| {
            let title = ws
                .name
                .clone()
                .or_else(|| {
                    ws.tabs.get(ws.active_tab).or(ws.tabs.first()).and_then(|t| super::first_pane(&t.layout)).map(|p| p.title.clone())
                })
                .filter(|t| !t.trim().is_empty())
                .map(|t| crate::terminal::strip_agent_mark(&t).to_string())
                .unwrap_or_else(|| crate::ui::tilde(&ws.cwd));
            let title = super::card_title(title);
            let group = ws.group.and_then(|g| state.groups.iter().find(|x| x.id == g)).map(|g| g.name.clone());
            WindowWorkspace { title, group, running: false }
        })
        .collect()
}

impl Workbench {
    /// This window's workspaces as the page lists them.
    fn own_summary(&self, current: bool, cx: &gpui::App) -> WindowSummary {
        let workspaces = self
            .workspaces
            .iter()
            .map(|ws| WindowWorkspace {
                title: self.workspace_label(ws, cx),
                group: ws.group.and_then(|g| self.groups.iter().find(|x| x.id == g)).map(|g| g.name.clone()),
                running: ws.tabs.iter().flat_map(|t| t.root.leaves()).any(|p| p.read(cx).is_running()),
            })
            .collect();
        let running = self.all_panes().iter().filter(|p| p.read(cx).is_running()).count();
        let unsaved = self.editor.as_ref().map_or(0, |editor| editor.read(cx).dirty_count());
        WindowSummary { slot: self.slot, open: true, current, closed_at_ms: None, workspaces, running, unsaved }
    }

    /// Every window: the open ones (this one included), then the closed ones that can be reopened.
    fn window_summaries(&mut self, cx: &mut Context<Self>) -> Vec<WindowSummary> {
        let mut windows = vec![self.own_summary(true, cx)];
        for handle in crate::workbenches(cx) {
            if handle.window_id() == self.window_id {
                continue;
            }
            if let Ok(other) = handle.read(cx) {
                windows.push(other.own_summary(false, cx));
            }
        }
        let open: Vec<usize> = windows.iter().map(|w| w.slot).collect();
        let stale = self.saved_windows.as_ref().is_none_or(|s| s.read_at.elapsed() > SAVED_FRESH);
        if stale {
            let closed = ClosedWindows::load();
            let saved = LayoutState::all_window_slots()
                .into_iter()
                .map(|slot| {
                    let state = LayoutState::load(slot);
                    WindowSummary {
                        slot,
                        open: false,
                        current: false,
                        closed_at_ms: closed.windows.iter().find(|w| w.slot == slot).map(|w| w.closed_at_ms),
                        workspaces: saved_workspaces(&state),
                        running: 0,
                        unsaved: 0,
                    }
                })
                .collect();
            self.saved_windows = Some(SavedWindows { read_at: std::time::Instant::now(), windows: saved });
        }
        let saved = self.saved_windows.as_ref().map(|s| s.windows.clone()).unwrap_or_default();
        windows.extend(saved.into_iter().filter(|w| !open.contains(&w.slot)));
        windows.sort_by_key(|w| (!w.open, w.slot));
        windows
    }

    /// Asks before deleting window `slot` and everything in it.
    fn ask_delete_window(&mut self, window: &WindowSummary, name: &str, cx: &mut Context<Self>) {
        let key = if window.open { "windows.delete_body" } else { "windows.delete_body_closed" };
        let mut body = tf(cx, key, &[("n", &window.workspaces.len().to_string())]);
        if window.running > 0 {
            body.push(' ');
            body.push_str(&tf(cx, "windows.delete_running", &[("n", &window.running.to_string())]));
        }
        if window.unsaved > 0 {
            body.push(' ');
            body.push_str(&tf(cx, "windows.delete_unsaved", &[("n", &window.unsaved.to_string())]));
        }
        self.ask(
            Ask {
                title: tf(cx, "windows.delete_title", &[("name", name)]).into(),
                body: Some(body.into()),
                choices: vec![AskChoice {
                    label: t(cx, "windows.delete").into(),
                    action: AskAction::DeleteWindow(window.slot),
                    primary: false,
                    danger: true,
                }],
                cancel: true,
            },
            cx,
        );
    }

    /// Deletes window `slot`: closes it when it is open (its terminals end with it) and forgets its
    /// workspaces. The main window is never deleted.
    pub(super) fn delete_window(&mut self, slot: usize, window: &mut Window, cx: &mut Context<Self>) {
        if slot == 0 {
            return;
        }
        self.saved_windows = None;
        if slot == self.slot {
            // Nothing of it is written again: the file goes, then the window.
            self.closed = true;
            LayoutState::forget_window(slot);
            window.remove_window();
            cx.defer(crate::set_app_menus);
            return;
        }
        // Forgotten now, so the page never reads it back while the window is still going.
        LayoutState::forget_window(slot);
        let other = crate::workbenches(cx).into_iter().find(|h| h.read(cx).is_ok_and(|wb| wb.slot == slot));
        let this = cx.entity().downgrade();
        cx.defer(move |cx| {
            if let Some(handle) = other {
                let _ = handle.update(cx, |wb, window, _| {
                    wb.closed = true;
                    window.remove_window();
                });
            }
            crate::set_app_menus(cx);
            // The page listed it as open until now.
            let _ = this.update(cx, |_, cx| cx.notify());
        });
        cx.notify();
    }

    /// Brings window `slot` to the front (reopening it when it was closed), on workspace
    /// `workspace` when one was picked.
    fn show_window(&mut self, slot: usize, workspace: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        if slot == self.slot {
            if let Some(index) = workspace.filter(|i| *i < self.workspaces.len()) {
                self.activate_workspace(index, window, cx);
            }
            return;
        }
        self.saved_windows = None;
        cx.defer(move |cx| {
            let open = crate::workbenches(cx).into_iter().find(|h| h.read(cx).is_ok_and(|wb| wb.slot == slot));
            let handle = match open {
                Some(handle) => Some(handle),
                None => crate::reopen_window(slot, cx),
            };
            if let Some(handle) = handle {
                let _ = handle.update(cx, |wb, window, cx| {
                    window.activate_window();
                    if let Some(index) = workspace.filter(|i| *i < wb.workspaces.len()) {
                        wb.activate_workspace(index, window, cx);
                    }
                });
                cx.activate(true);
            }
        });
    }

    pub(super) fn render_windows_settings(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let windows = self.window_summaries(cx);
        let now = crate::ui::now_ms();
        let mut page = div().flex().flex_col().gap_4().child(div().t_body().text_color(hex(Chrome::MUTED)).child(t(cx, "windows.intro")));
        for (position, summary) in windows.into_iter().enumerate() {
            let slot = summary.slot;
            let name = window_name(slot, position, cx);
            let state = if summary.current {
                t(cx, "windows.this_window").to_string()
            } else if summary.open {
                t(cx, "windows.open").to_string()
            } else {
                match summary.closed_at_ms {
                    Some(at) => tf(cx, "windows.closed_at", &[("when", &relative_time(now, at))]),
                    None => t(cx, "windows.closed").to_string(),
                }
            };
            let counts = if summary.running > 0 {
                tf(cx, "windows.counts_running", &[("n", &summary.workspaces.len().to_string()), ("running", &summary.running.to_string())])
            } else {
                tf(cx, "windows.counts", &[("n", &summary.workspaces.len().to_string())])
            };
            let mut actions = div().flex().items_center().gap_1();
            if !summary.current {
                let label = if summary.open { t(cx, "windows.bring_forward") } else { t(cx, "windows.reopen") };
                actions = actions.child(action_button(
                    SharedString::from(format!("window-show-{slot}")),
                    label,
                    cx.listener(move |this, _: &ClickEvent, window, cx| this.show_window(slot, None, window, cx)),
                ));
            }
            if slot > 0 {
                let (target, target_name) = (summary.clone(), name.clone());
                actions = actions.child(
                    div()
                        .id(SharedString::from(format!("window-delete-{slot}")))
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .cursor_pointer()
                        .t_small()
                        .text_color(hex(Chrome::ERROR))
                        .hover(|s| s.bg(hex_alpha(Chrome::ERROR, 0.15)))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.ask_delete_window(&target, &target_name, cx)))
                        .child(t(cx, "windows.delete")),
                );
            }
            let mut list = div().flex().flex_col().gap_px();
            if summary.workspaces.is_empty() {
                list = list.child(div().px_2().py_1().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "windows.empty")));
            }
            for (index, ws) in summary.workspaces.iter().enumerate() {
                list =
                    list.child(
                        div()
                            .id(SharedString::from(format!("window-{slot}-ws-{index}")))
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .flex()
                            .items_center()
                            .gap_2()
                            .cursor_pointer()
                            .hover(|s| s.bg(hex(Chrome::HOVER)))
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.show_window(slot, Some(index), window, cx)))
                            .child(div().flex_shrink_0().size(px(6.)).rounded_full().bg(if ws.running {
                                hex(Chrome::SUCCESS)
                            } else {
                                hex_alpha(Chrome::MUTED, 0.5)
                            }))
                            .child(div().flex_1().min_w_0().truncate().t_body().text_color(hex(Chrome::FOREGROUND)).child(ws.title.clone()))
                            .children(ws.group.clone().map(|g| {
                                div().flex_shrink_0().max_w(px(160.)).truncate().t_caption().text_color(hex(Chrome::MUTED)).child(g)
                            })),
                    );
            }
            page = page.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .rounded_md()
                    .border_1()
                    .border_color(if summary.current { hex_alpha(Chrome::ACCENT, 0.7) } else { hex(Chrome::BORDER) })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(crate::ui::icon("app-window", crate::ui::IconSize::INLINE, hex(Chrome::MUTED)))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(div().t_body().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(name))
                                    .child(div().t_small().text_color(hex(Chrome::MUTED)).child(format!("{state} · {counts}"))),
                            )
                            .child(actions),
                    )
                    .child(list),
            );
        }
        page
    }
}

/// "Main window", then "Window 2", "Window 3", … in the order the page lists them.
fn window_name(slot: usize, position: usize, cx: &gpui::App) -> String {
    if slot == 0 {
        t(cx, "windows.main").to_string()
    } else {
        tf(cx, "windows.nth", &[("n", &(position + 1).to_string())])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_closed_window_lists_its_workspaces_by_name_folder_and_group() {
        let state: LayoutState = serde_json::from_value(serde_json::json!({
            "groups": [{"id": 7, "name": "Backend"}],
            "workspaces": [
                {"id": 1, "name": "API", "group": 7, "cwd": "/work/api", "tabs": []},
                {"id": 2, "cwd": "/work/web", "tabs": [], "group": 99},
            ],
        }))
        .unwrap();
        let listed = saved_workspaces(&state);
        assert_eq!(listed[0], WindowWorkspace { title: "API".into(), group: Some("Backend".into()), running: false });
        // No name and no tab: its folder. A group that no longer exists is no group.
        assert_eq!(listed[1].title, "web");
        assert_eq!(listed[1].group, None);
    }
}
