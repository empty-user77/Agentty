//! Monitoring → In-app browsers: every browser open in this window — whose it is (the workspace
//! and terminal, or the plugin), whether it is on screen, running out of sight or unloaded, its
//! pages and the memory they use, a small picture of the page — with a way to go to its terminal
//! or close it at once.
//!
//! The pictures are a cache: taken while the page is open for a browser that has none or whose
//! picture is over five minutes old, 240 points wide, into a private folder of the data folder.
//! A new one replaces the one before (so the page does not flicker with pictures loading again
//! and again); a browser that went away takes its picture with it; an unloaded browser keeps its
//! last one.

use super::browser::BrowserPanel;
use super::{Page, Workbench};
use crate::i18n::{t, tf};
use crate::theme::{hex, Chrome};
use crate::ui::{action_button, icon, IconSize, TypeScale};
use crate::usage_view::{card, kpi};
use crate::webview::WebView;
use gpui::{div, prelude::*, px, ClickEvent, Context, ObjectFit, SharedString};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

const REFRESH: Duration = Duration::from_secs(2);
/// How long a picture is used before a new one is taken.
const PREVIEW_FOR: Duration = Duration::from_secs(5 * 60);
const PREVIEW_WIDTH: f64 = 240.;

/// The latest picture of each row (`t<pane>` for a terminal's browser, `p<plugin>` for a plugin's
/// pages) and when it was taken, shared with the snapshot callbacks.
pub(super) type Previews = Rc<RefCell<HashMap<String, (PathBuf, Instant)>>>;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum State {
    Shown,
    Background,
    Closed,
    Unloaded,
}

enum Owner {
    Terminal(u64),
    Plugin,
}

struct Row {
    key: String,
    /// The page the picture is taken of (the tab in front).
    view: Option<Rc<RefCell<Option<WebView>>>>,
    owner: Owner,
    /// "Workspace · terminal", or the plugin's name.
    name: String,
    /// What the terminal runs (its title), or the automation.
    detail: String,
    state: State,
    pages: Vec<String>,
    memory: u64,
    seen: Option<Instant>,
}

impl Workbench {
    fn browser_row(&self, pane: u64, browser: &BrowserPanel, state: State, cx: &gpui::App) -> Row {
        let terminal = self.all_panes().into_iter().find(|p| p.read(cx).pane_id == pane);
        let workspace = terminal
            .as_ref()
            .and_then(|p| self.locate(p))
            .and_then(|(w, _)| self.workspaces.get(w))
            .map(|ws| short(&self.workspace_title(ws, cx)))
            .unwrap_or_default();
        // What runs in the terminal (an agent's session title), when it says more than the folder.
        let title = terminal.as_ref().map(|p| p.read(cx).display_title()).unwrap_or_default();
        let detail = if short(&title) == workspace || title.contains('/') { String::new() } else { title };
        let front = browser.tabs.get(browser.active.min(browser.tabs.len().saturating_sub(1)));
        Row {
            key: format!("t{pane}"),
            view: front.map(|tab| tab.webview.clone()),
            owner: Owner::Terminal(pane),
            name: tf(cx, "browsers.terminal", &[("workspace", &workspace), ("n", &pane.to_string())]),
            detail: agentty_bridge::fsutil::one_line(&detail, 60),
            state: if browser.unloaded { State::Unloaded } else { state },
            pages: browser.tabs.iter().map(|tab| tab.label(cx)).collect(),
            memory: super::browser_budget::memory_of_browser(browser),
            seen: (state != State::Shown).then_some(browser.last_shown),
        }
    }

    fn browser_rows(&self, cx: &gpui::App) -> Vec<Row> {
        let mut rows = Vec::new();
        let in_plugin_workspace = self.plugin_workspace_shown.is_some();
        if let Some(owner) = self.browser_owner {
            let shown = if in_plugin_workspace { self.stashed_browser.as_ref() } else { self.browser.as_ref() };
            if let Some(browser) = shown {
                let state = if in_plugin_workspace { State::Background } else { State::Shown };
                rows.push(self.browser_row(owner, browser, state, cx));
            }
        }
        let mut kept: Vec<(&u64, &BrowserPanel)> = self.pane_browsers.iter().collect();
        kept.sort_by_key(|(_, b)| std::cmp::Reverse(b.last_shown));
        for (pane, browser) in kept {
            let state = if browser.closed { State::Closed } else { State::Background };
            rows.push(self.browser_row(*pane, browser, state, cx));
        }
        rows.sort_by_key(|row| row.state);
        // Plugins' pages, one row per plugin.
        type PluginPages = (String, Vec<String>, std::collections::HashSet<i32>, bool, Rc<RefCell<Option<WebView>>>);
        let mut plugins: Vec<PluginPages> = Vec::new();
        for page in &self.plugin_browsers {
            let view = page.webview.borrow();
            let url = view.as_ref().and_then(|v| v.current_url()).unwrap_or_default();
            let pid = view.as_ref().and_then(|v| v.web_process_id());
            match plugins.iter_mut().find(|(name, ..)| *name == page.plugin_name) {
                Some((_, pages, pids, shown, _)) => {
                    pages.push(url);
                    pids.extend(pid);
                    *shown |= page.shown;
                }
                None => plugins.push((page.plugin_name.clone(), vec![url], pid.into_iter().collect(), page.shown, page.webview.clone())),
            }
        }
        for (name, pages, pids, shown, view) in plugins {
            rows.push(Row {
                key: format!("p{name}"),
                view: Some(view),
                owner: Owner::Plugin,
                name: tf(cx, "browsers.plugin", &[("name", &name)]),
                detail: String::new(),
                state: if shown { State::Shown } else { State::Background },
                pages,
                memory: super::browser_budget::memory_of_pids(pids),
                seen: None,
            });
        }
        rows
    }

    /// Repaints the page every couple of seconds while it is open (memory changes on its own).
    fn keep_browsers_page_fresh(&mut self, cx: &mut Context<Self>) {
        if self.browsers_page_refreshing {
            return;
        }
        self.browsers_page_refreshing = true;
        self.take_browser_previews(cx);
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(REFRESH).await;
            let open = this
                .update(cx, |this, cx| {
                    let open = this.page == Some(Page::Browsers);
                    if open {
                        this.take_browser_previews(cx);
                        cx.notify();
                    } else {
                        this.browsers_page_refreshing = false;
                    }
                    open
                })
                .unwrap_or(false);
            if !open {
                break;
            }
        })
        .detach();
    }

    /// A small picture of each browser's page in front that has none, or one over five minutes
    /// old (an unloaded browser has no page: its last picture stays).
    fn take_browser_previews(&mut self, cx: &mut Context<Self>) {
        let dir = previews_dir();
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        }
        for key in self.previews_taking_done.borrow_mut().drain(..) {
            self.previews_taking.remove(&key);
        }
        let stamp = crate::ui::now_ms();
        for row in self.browser_rows(cx) {
            let fresh = self.browser_previews.borrow().get(&row.key).is_some_and(|(_, at)| at.elapsed() < PREVIEW_FOR);
            if fresh || self.previews_taking.contains(&row.key) {
                continue;
            }
            let Some(view) = row.view else { continue };
            let borrowed = view.borrow();
            let Some(page) = borrowed.as_ref() else { continue };
            let path = dir.join(format!("{}-{stamp}.png", file_safe(&row.key)));
            let (previews, key, taken) = (self.browser_previews.clone(), row.key.clone(), path.clone());
            self.previews_taking.insert(row.key.clone());
            let taking = self.previews_taking_done.clone();
            page.snapshot_png_sized(
                path,
                PREVIEW_WIDTH,
                Box::new(move |result| {
                    match result {
                        Ok(_) => {
                            if let Some((before, _)) = previews.borrow_mut().insert(key.clone(), (taken, Instant::now())) {
                                let _ = std::fs::remove_file(before);
                            }
                        }
                        Err(_) => {
                            let _ = std::fs::remove_file(taken);
                        }
                    }
                    taking.borrow_mut().push(key);
                }),
            );
        }
    }

    pub(super) fn render_browsers_page(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        self.keep_browsers_page_fresh(cx);
        let rows = self.browser_rows(cx);
        // A browser that went away takes its picture with it.
        {
            let mut previews = self.browser_previews.borrow_mut();
            let gone: Vec<String> = previews.keys().filter(|key| !rows.iter().any(|row| &row.key == *key)).cloned().collect();
            for key in gone {
                if let Some((path, _)) = previews.remove(&key) {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
        let previews = self.browser_previews.borrow().clone();
        let prefs = crate::settings::settings(cx).browser.clone();
        let total = super::browser_budget::system_memory();
        let used: u64 = self.browser_memory_total();
        let cap = total / 100 * u64::from(prefs.memory_limit.min(100));
        let running = rows.iter().filter(|r| matches!(r.state, State::Shown | State::Background | State::Closed)).count();
        let out_of_sight =
            rows.iter().filter(|r| matches!(r.owner, Owner::Terminal(_)) && matches!(r.state, State::Background | State::Closed)).count();
        let unloaded = rows.iter().filter(|r| r.state == State::Unloaded).count();

        let header = div()
            .flex()
            .items_center()
            .gap_3()
            .child(div().t_heading().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(t(cx, "page.browsers")))
            .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "browsers.hint")));
        let kpis = div()
            .flex()
            .flex_wrap()
            .gap_3()
            .child(kpi(
                t(cx, "browsers.memory"),
                megabytes(used),
                tf(cx, "browsers.memory_sub", &[("limit", &megabytes(cap)), ("percent", &prefs.memory_limit.to_string())]),
                if cap > 0 && used > cap { Chrome::ERROR } else { Chrome::PURPLE },
            ))
            .child(kpi(
                t(cx, "browsers.running"),
                running.to_string(),
                tf(cx, "browsers.running_sub", &[("n", &out_of_sight.to_string()), ("limit", &prefs.background_limit.max(1).to_string())]),
                Chrome::GREEN,
            ))
            .child(kpi(t(cx, "browsers.unloaded"), unloaded.to_string(), t(cx, "browsers.unloaded_sub").to_string(), Chrome::MUTED));

        let columns = div()
            .px_4()
            .py_2()
            .flex()
            .items_center()
            .gap_3()
            .border_b_1()
            .border_color(hex(Chrome::BORDER))
            .t_small()
            .text_color(hex(Chrome::MUTED))
            .child(div().flex_1().min_w_0().child(t(cx, "browsers.owner")))
            .child(div().w(px(110.)).child(t(cx, "browsers.state")))
            .child(div().w(px(84.)).flex().justify_end().child(t(cx, "browsers.memory")))
            .child(div().w(px(72.)).flex().justify_end().child(t(cx, "browsers.seen")))
            .child(div().w(px(196.)));
        let mut table = card().flex().flex_col().overflow_hidden().child(columns);
        if rows.is_empty() {
            table = table.child(crate::ui::hint(t(cx, "browsers.empty")).px_4().py_4());
        }
        for (index, row) in rows.into_iter().enumerate() {
            let (label, color) = match row.state {
                State::Shown => (t(cx, "browsers.shown"), Chrome::SUCCESS),
                State::Background => (t(cx, "browsers.background"), Chrome::BLUE),
                State::Closed => (t(cx, "browsers.closed"), Chrome::WARNING),
                State::Unloaded => (t(cx, "browsers.unloaded_state"), Chrome::MUTED),
            };
            let seen = row.seen.map(|at| ago(at.elapsed(), cx)).unwrap_or_default();
            let actions = match row.owner {
                Owner::Terminal(pane) => div()
                    .flex()
                    .justify_end()
                    .gap_1()
                    .child(action_button(
                        SharedString::from(format!("browser-go-{pane}")),
                        t(cx, "browsers.go"),
                        cx.listener(move |this, _: &ClickEvent, window, cx| this.go_to_terminal_browser(pane, window, cx)),
                    ))
                    .child(action_button(
                        SharedString::from(format!("browser-close-{pane}")),
                        t(cx, "browsers.close"),
                        cx.listener(move |this, _: &ClickEvent, window, cx| this.close_terminal_browser_now(pane, window, cx)),
                    )),
                Owner::Plugin => {
                    div().flex().justify_end().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "browsers.plugin_managed"))
                }
            };
            let pages = row.pages.join(" · ");
            table = table.child(
                div()
                    .id(("browser-row", index))
                    .px_4()
                    .py_2()
                    .flex()
                    .items_center()
                    .gap_3()
                    .border_b_1()
                    .border_color(hex(0x2a2a2a))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .w(px(96.))
                                    .h(px(60.))
                                    .flex_shrink_0()
                                    .rounded_md()
                                    .overflow_hidden()
                                    .border_1()
                                    .border_color(hex(Chrome::BORDER))
                                    .bg(hex(0x1a1a1a))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .map(|d| match previews.get(&row.key) {
                                        Some((path, _)) => d.child(gpui::img(path.clone()).size_full().object_fit(ObjectFit::Cover)),
                                        None => d.child(icon(
                                            if matches!(row.owner, Owner::Plugin) { "puzzle" } else { "globe" },
                                            IconSize::INLINE,
                                            hex(Chrome::MUTED),
                                        )),
                                    }),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(div().t_body().truncate().text_color(hex(Chrome::BRIGHT)).child(row.name))
                                    .when(!row.detail.is_empty(), |d| {
                                        d.child(div().t_small().truncate().text_color(hex(Chrome::MUTED)).child(row.detail))
                                    })
                                    .child(div().t_caption().truncate().text_color(hex(Chrome::MUTED)).child(pages)),
                            ),
                    )
                    .child(div().w(px(110.)).t_small().text_color(hex(color)).child(label))
                    .child(div().w(px(84.)).flex().justify_end().t_small().child(megabytes(row.memory)))
                    .child(div().w(px(72.)).flex().justify_end().t_small().text_color(hex(Chrome::MUTED)).child(seen))
                    .child(div().w(px(196.)).child(actions)),
            );
        }
        div().id("browsers-page").size_full().overflow_y_scroll().p_6().flex().flex_col().gap_4().child(header).child(kpis).child(table)
    }

    /// "Go to terminal": its workspace and terminal in front, with its browser (opened again if the
    /// user had closed it).
    fn go_to_terminal_browser(&mut self, pane: u64, window: &mut gpui::Window, cx: &mut Context<Self>) {
        self.page = None;
        self.select_terminal(pane, window, cx);
        self.show_terminal_browser(pane, cx);
        cx.notify();
    }

    /// "Close now": the terminal's browser closes and its pages' memory is freed.
    fn close_terminal_browser_now(&mut self, pane: u64, window: &mut gpui::Window, cx: &mut Context<Self>) {
        self.drop_terminal_browser(pane, window);
        cx.notify();
    }
}

/// Where the pictures are kept (private: they show the pages).
fn previews_dir() -> PathBuf {
    agentty_bridge::fsutil::data_dir().join("cache").join("browser-previews")
}

/// A row key as a file name (a plugin's name may hold anything).
fn file_safe(key: &str) -> String {
    key.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect()
}

/// Pictures left by an earlier run: they go when the app starts.
pub(super) fn clear_previews() {
    let _ = std::fs::remove_dir_all(previews_dir());
}

/// A folder's name, from a title that is a path (a shell workspace is titled by its folder).
fn short(title: &str) -> String {
    title.trim_end_matches('/').rsplit('/').next().unwrap_or(title).to_string()
}

fn ago(elapsed: Duration, cx: &gpui::App) -> String {
    let minutes = elapsed.as_secs() / 60;
    match minutes {
        0 => t(cx, "browsers.just_now").to_string(),
        1..=59 => tf(cx, "browsers.minutes_ago", &[("n", &minutes.to_string())]),
        _ => tf(cx, "browsers.hours_ago", &[("n", &(minutes / 60).to_string())]),
    }
}

fn megabytes(bytes: u64) -> String {
    let mb = bytes as f64 / 1_048_576.0;
    if mb >= 1024.0 {
        format!("{:.1} GB", mb / 1024.0)
    } else {
        format!("{mb:.0} MB")
    }
}
