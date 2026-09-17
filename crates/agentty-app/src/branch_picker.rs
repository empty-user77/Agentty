//! Branch picker shared by the pane branch chip and AgentGit: search box, local / remote sections,
//! lazy paging while scrolling, a remote search (`git ls-remote`) for branches never fetched, and
//! "New branch". Every place that switches branches uses this, so they behave the same.

use crate::i18n::{t, tf};
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, now_ms, relative_time, IconSize, TypeScale};
use agentty_bridge::git::{self, Branch};
use gpui::{
    div, prelude::*, px, AnyElement, ClickEvent, Context, EventEmitter, FocusHandle, Focusable, SharedString, Subscription, Window,
};
use std::path::PathBuf;
use std::time::Duration;

const PAGE: usize = 60;
const ROW_HEIGHT: f32 = 28.;
const LIST_HEIGHT: f32 = 300.;

pub enum BranchPickerEvent {
    /// Switch to (or, in merge mode, merge) this branch; `remote` = a remote-tracking name.
    Pick {
        name: String,
        remote: bool,
    },
    Create(String),
    Dismiss,
}

#[derive(Clone)]
enum Row {
    Section(&'static str),
    Branch { name: String, remote: bool, current: bool, time: Option<i64> },
    More,
    Searching,
}

pub struct BranchPicker {
    repo: PathBuf,
    current: Option<String>,
    /// `None` while loading.
    branches: Option<Vec<Branch>>,
    /// Remote branches found by name on the servers (not fetched yet).
    remote_hits: Vec<String>,
    searching: bool,
    search_generation: u64,
    limit: usize,
    query: gpui::Entity<TextInput>,
    new_branch: Option<gpui::Entity<TextInput>>,
    scroll: gpui::UniformListScrollHandle,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<BranchPickerEvent> for BranchPicker {}

impl Focusable for BranchPicker {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl BranchPicker {
    pub fn new(repo: PathBuf, current: Option<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| TextInput::new("", t(cx, "git.search_branches"), window, cx));
        let subscription = cx.subscribe(&query, |this, _, event: &TextInputEvent, cx| match event {
            TextInputEvent::Changed => this.query_changed(cx),
            TextInputEvent::Confirmed => {
                // Enter picks the first match.
                if let Some(Row::Branch { name, remote, .. }) = this.rows(cx).into_iter().find(|r| matches!(r, Row::Branch { .. })) {
                    cx.emit(BranchPickerEvent::Pick { name, remote });
                }
            }
            TextInputEvent::Cancelled => cx.emit(BranchPickerEvent::Dismiss),
            _ => {}
        });
        window.focus(&query.focus_handle(cx));
        let mut picker = Self {
            repo,
            current,
            branches: None,
            remote_hits: Vec::new(),
            searching: false,
            search_generation: 0,
            limit: PAGE,
            query,
            new_branch: None,
            scroll: gpui::UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            _subscriptions: vec![subscription],
        };
        picker.load(cx);
        picker
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        let repo = self.repo.clone();
        let task = cx.background_spawn(async move { git::branches(&repo).unwrap_or_default() });
        cx.spawn(async move |this, cx| {
            let branches = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.current.is_none() {
                    this.current = branches.iter().find(|b| b.current).map(|b| b.name.clone());
                }
                this.branches = Some(branches);
                cx.notify();
            });
        })
        .detach();
    }

    fn query_text(&self, cx: &gpui::App) -> String {
        self.query.read(cx).text().trim().to_lowercase()
    }

    fn query_changed(&mut self, cx: &mut Context<Self>) {
        self.limit = PAGE;
        self.remote_hits.clear();
        self.search_generation += 1;
        let generation = self.search_generation;
        let query = self.query_text(cx);
        self.searching = query.chars().count() >= 2;
        cx.notify();
        if !self.searching {
            return;
        }
        let repo = self.repo.clone();
        cx.spawn(async move |this, cx| {
            // Debounced: only the last query of a burst of typing reaches the servers.
            cx.background_executor().timer(Duration::from_millis(450)).await;
            if this.read_with(cx, |this, _| this.search_generation != generation).unwrap_or(true) {
                return;
            }
            let found = cx.background_spawn(async move { git::search_remote_branches(&repo, &query).unwrap_or_default() }).await;
            let _ = this.update(cx, |this, cx| {
                if this.search_generation == generation {
                    this.remote_hits = found;
                    this.searching = false;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn rows(&self, cx: &gpui::App) -> Vec<Row> {
        let Some(branches) = &self.branches else { return Vec::new() };
        let query = self.query_text(cx);
        let matches = |name: &str| query.is_empty() || name.to_lowercase().contains(&query);
        let locals: Vec<&Branch> = branches.iter().filter(|b| !b.remote && matches(&b.name)).collect();
        let local_names: std::collections::HashSet<&str> = branches.iter().filter(|b| !b.remote).map(|b| b.name.as_str()).collect();
        // Remote branches that already have a local branch are the same thing twice.
        let short = |name: &str| name.split_once('/').map(|(_, n)| n.to_string()).unwrap_or_else(|| name.to_string());
        let remotes: Vec<&Branch> =
            branches.iter().filter(|b| b.remote && matches(&b.name) && !local_names.contains(short(&b.name).as_str())).collect();
        let mut rows = Vec::new();
        let mut shown = 0;
        let push = |row: Row, rows: &mut Vec<Row>, shown: &mut usize| {
            if *shown < self.limit {
                rows.push(row);
                *shown += 1;
            }
        };
        if !locals.is_empty() {
            rows.push(Row::Section("git.local_branches"));
            for b in &locals {
                push(
                    Row::Branch {
                        name: b.name.clone(),
                        remote: false,
                        current: self.current.as_ref() == Some(&b.name),
                        time: Some(b.time),
                    },
                    &mut rows,
                    &mut shown,
                );
            }
        }
        let known: std::collections::HashSet<String> = branches.iter().map(|b| b.name.clone()).collect();
        let extra: Vec<&String> =
            self.remote_hits.iter().filter(|name| !known.contains(*name) && !local_names.contains(short(name).as_str())).collect();
        if !remotes.is_empty() || !extra.is_empty() {
            rows.push(Row::Section("git.remote_branches"));
            for b in &remotes {
                push(Row::Branch { name: b.name.clone(), remote: true, current: false, time: Some(b.time) }, &mut rows, &mut shown);
            }
            for name in extra {
                push(Row::Branch { name: name.clone(), remote: true, current: false, time: None }, &mut rows, &mut shown);
            }
        }
        if locals.len() + remotes.len() > self.limit {
            rows.push(Row::More);
        }
        if self.searching {
            rows.push(Row::Searching);
        }
        rows
    }

    fn render_row(&mut self, index: usize, row: Row, cx: &mut Context<Self>) -> AnyElement {
        match row {
            Row::Section(label) => div()
                .id(("branch-section", index))
                .h(px(ROW_HEIGHT))
                .px_2()
                .flex()
                .items_end()
                .pb_1()
                .t_caption()
                .text_color(hex(Chrome::MUTED))
                .child(t(cx, label))
                .into_any_element(),
            Row::More => {
                // Reaching the end of the page loads the next one.
                let this = cx.entity().downgrade();
                cx.defer(move |cx| {
                    let _ = this.update(cx, |this, cx| {
                        this.limit += PAGE;
                        cx.notify();
                    });
                });
                crate::ui::loading_row(t(cx, "git.loading_branches")).h(px(ROW_HEIGHT)).into_any_element()
            }
            Row::Searching => crate::ui::loading_row(t(cx, "git.searching_remote")).h(px(ROW_HEIGHT)).into_any_element(),
            Row::Branch { name, remote, current, time } => {
                let label = name.clone();
                div()
                    .id(("branch-row", index))
                    .h(px(ROW_HEIGHT))
                    .px_2()
                    .rounded_md()
                    .flex()
                    .items_center()
                    .gap_2()
                    .t_body()
                    .cursor_pointer()
                    .text_color(hex(Chrome::FOREGROUND))
                    .hover(|s| s.bg(hex(Chrome::ACCENT)).text_color(hex(Chrome::BRIGHT)))
                    .child(
                        div()
                            .w(px(IconSize::INLINE))
                            .flex_shrink_0()
                            .when(current, |d| d.child(icon("check", IconSize::INLINE, hex(Chrome::BRIGHT)))),
                    )
                    .child(icon(if remote { "globe" } else { "git-branch" }, 12., hex(Chrome::MUTED)))
                    .child(div().flex_1().min_w_0().truncate().child(label))
                    .child(div().flex_shrink_0().t_small().text_color(hex(Chrome::MUTED)).child(
                        time.map(|t| relative_time(now_ms(), t as u64 * 1000)).unwrap_or_else(|| t(cx, "git.not_fetched").to_string()),
                    ))
                    .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        cx.emit(BranchPickerEvent::Pick { name: name.clone(), remote });
                    }))
                    .into_any_element()
            }
        }
    }

    fn open_new_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| TextInput::new(self.query.read(cx).text().to_string(), t(cx, "git.new_branch_name"), window, cx));
        cx.subscribe(&input, |this, input, event: &TextInputEvent, cx| match event {
            TextInputEvent::Confirmed => {
                let name = input.read(cx).text().trim().to_string();
                if !name.is_empty() {
                    cx.emit(BranchPickerEvent::Create(name));
                }
            }
            TextInputEvent::Cancelled => {
                this.new_branch = None;
                cx.notify();
            }
            _ => {}
        })
        .detach();
        window.focus(&input.focus_handle(cx));
        self.new_branch = Some(input);
        cx.notify();
    }
}

fn field(input: gpui::Entity<TextInput>, accent: bool) -> gpui::Div {
    let focus = input.clone();
    div()
        .flex()
        .items_center()
        .gap_2()
        .px_2()
        .py_1()
        .rounded_md()
        .border_1()
        .border_color(hex(if accent { Chrome::ACCENT } else { Chrome::BORDER }))
        .bg(hex(0x1a1a1a))
        .t_body()
        .text_color(hex(Chrome::BRIGHT))
        .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| window.focus(&focus.focus_handle(cx)))
        .child(input)
}

impl Render for BranchPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.rows(cx);
        let count = rows.len();
        let body: AnyElement = match &self.branches {
            None => crate::ui::loading_row(t(cx, "git.loading_branches")).into_any_element(),
            Some(_) if count == 0 => {
                crate::ui::hint(if self.query_text(cx).is_empty() { t(cx, "git.no_branches") } else { t(cx, "git.no_match") })
                    .into_any_element()
            }
            Some(_) => {
                let height = (count as f32 * ROW_HEIGHT).min(LIST_HEIGHT);
                div()
                    .relative()
                    .h(px(height))
                    .child(
                        gpui::uniform_list(
                            "branch-list",
                            count,
                            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                                let rows = this.rows(cx);
                                range
                                    .filter_map(|i| rows.get(i).cloned().map(|row| (i, row)))
                                    .map(|(i, row)| this.render_row(i, row, cx))
                                    .collect::<Vec<_>>()
                            }),
                        )
                        .track_scroll(self.scroll.clone())
                        .size_full(),
                    )
                    .into_any_element()
            }
        };
        let footer: AnyElement = match &self.new_branch {
            Some(input) => div()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().t_small().text_color(hex(Chrome::MUTED)).child(tf(
                    cx,
                    "git.new_branch_from",
                    &[("branch", self.current.as_deref().unwrap_or("HEAD"))],
                )))
                .child(field(input.clone(), true))
                .into_any_element(),
            None => div()
                .id("branch-picker-new")
                .h(px(ROW_HEIGHT))
                .px_2()
                .rounded_md()
                .flex()
                .items_center()
                .gap_2()
                .t_body()
                .cursor_pointer()
                .text_color(hex(Chrome::FOREGROUND))
                .hover(|s| s.bg(hex(Chrome::ACCENT)).text_color(hex(Chrome::BRIGHT)))
                .child(icon("plus", IconSize::INLINE, hex(Chrome::MUTED)))
                .child(t(cx, "git.new_branch"))
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    cx.stop_propagation();
                    this.open_new_branch(window, cx);
                }))
                .into_any_element(),
        };
        div()
            .id("branch-picker")
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .gap_1()
            .child(div().px_1().pt_1().child(field(self.query.clone(), false).child(icon(
                "search",
                IconSize::INLINE,
                hex_alpha(Chrome::FOREGROUND, 0.5),
            ))))
            .child(div().px_1().child(body))
            .child(div().mx_1().h(px(1.)).bg(hex(Chrome::OVERLAY_BORDER)))
            .child(div().px_1().pb_1().child(footer))
    }
}

pub fn branch_label(name: &str, remote: bool) -> SharedString {
    if remote {
        name.split_once('/').map(|(_, n)| n.to_string()).unwrap_or_else(|| name.to_string()).into()
    } else {
        name.to_string().into()
    }
}
