//! The run log: one panel docked under the terminals, a tab for each run started from the run
//! button, whatever pane it was pressed in. It only shows output — nothing is typed into it — with
//! words or a pattern to filter by, a level filter with counts, wrapping, ⌘-click on addresses,
//! and the run's stop and restart at hand. A run that fails because its port is taken says what
//! holds the port and offers to stop it and run again.
//!
//! The panel exists only for runs: it opens with the first and goes away with the last tab.
//! Hiding it keeps the runs going; the run panel in the status bar brings it back.

use super::Workbench;
use crate::i18n::{t, tf};
use crate::procinfo::ServingProcess;
use crate::run_log::{self, Event, Filter, Level, LogBuffer, LogLine, Matcher, Stream};
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, icon_only_sized, tilde, Tooltip, TypeScale};
use futures::StreamExt;
use gpui::{
    div, list, prelude::*, px, AnyElement, ClickEvent, Context, Entity, HighlightStyle, ListAlignment, ListState, SharedString, StyledText,
    Subscription, Window,
};
use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

pub const DEFAULT_HEIGHT: f32 = 260.;
const MIN_HEIGHT: f32 = 120.;
/// What the terminals above keep at least while the panel is dragged taller.
const MIN_TERMINALS: f32 = 180.;
/// Ctrl-C first; `SIGTERM` this much later, `SIGKILL` after as long again.
const STOP_GRACE: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TabState {
    Running { pid: u32 },
    Stopping { pid: u32 },
    Exited(Option<i32>),
    Failed,
}

impl TabState {
    pub fn is_running(self) -> bool {
        matches!(self, TabState::Running { .. } | TabState::Stopping { .. })
    }
}

/// A tab's lines and which of them the filter lets through. Shared with the list's renderer.
#[derive(Default)]
struct Store {
    buffer: LogBuffer,
    /// Line numbers (counted from the run's first line) the filter shows.
    shown: Vec<usize>,
    /// The first line not looked at by the filter yet.
    scanned: usize,
}

/// A run that ended because a port it wanted was taken.
pub(super) struct Conflict {
    /// The ports its output names (none when it doesn't say).
    ports: Vec<u16>,
    /// What listens on them now, and whether each belongs to this project (it works in it, or it
    /// is one of the log's runs). `None` while looked up.
    holders: Option<Vec<(ServingProcess, bool)>>,
    /// Being stopped, before the run starts again.
    busy: bool,
}

pub(super) struct LogTab {
    pub id: u64,
    pub root: PathBuf,
    pub config: String,
    pub title: String,
    pub command: String,
    pub dir: PathBuf,
    /// Ports the configuration names, for a failure that doesn't say which port was taken.
    named_ports: Vec<u16>,
    pub state: TabState,
    store: Rc<RefCell<Store>>,
    list: ListState,
    /// Which start of the process the tab is on: what an earlier one still reports is dropped.
    run: u64,
    /// Start again once the process ended (a restart).
    restart: bool,
    /// Error lines the user hasn't looked at (the tab was not in front).
    unseen_errors: usize,
    /// Scrolled to the end: new lines keep it there. Scrolling up stops that until it is back.
    follow: Rc<Cell<bool>>,
    conflict: Option<Conflict>,
    /// The number of the first line of the current run: what came before is an earlier run's.
    run_start: usize,
}

#[derive(Default)]
pub(super) struct LogPanel {
    pub tabs: Vec<LogTab>,
    pub active: usize,
    pub open: bool,
    search: Option<(Entity<TextInput>, Subscription)>,
    filter: Filter,
    matcher: Option<Rc<Matcher>>,
    filter_error: Option<String>,
    wrap: bool,
    next_id: u64,
    /// The top edge being dragged: where the pointer started, and the height then.
    pub drag: Option<(f32, f32)>,
}

impl LogPanel {
    pub fn tab(&self, id: u64) -> Option<&LogTab> {
        self.tabs.iter().find(|t| t.id == id)
    }

    fn index_of(&self, id: u64) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    fn matcher(&self) -> Rc<Matcher> {
        self.matcher.clone().unwrap_or_else(|| Rc::new(Matcher::All))
    }

    /// For the debug driver's `probe`.
    pub fn debug_state(&self) -> serde_json::Value {
        serde_json::json!({
            "open": self.open,
            "active": self.tabs.get(self.active).map(|t| t.id),
            "filter": { "query": self.filter.query, "regex": self.filter.regex, "case": self.filter.case_sensitive, "level": self.filter.level.map(|l| format!("{l:?}")) },
            "filterError": self.filter_error,
            "tabs": self.tabs.iter().map(|t| {
                let store = t.store.borrow();
                serde_json::json!({
                    "id": t.id, "title": t.title, "config": t.config, "state": format!("{:?}", t.state),
                    "lines": store.buffer.len(), "shown": store.shown.len(), "errors": store.buffer.errors(), "warnings": store.buffer.warnings(), "follow": t.follow.get(),
                    "conflict": t.conflict.as_ref().map(|c| serde_json::json!({ "ports": c.ports, "busy": c.busy, "holders": c.holders.as_ref().map(|h| h.iter().map(|(p, ours)| format!("{} {:?} {} ours={ours}", p.pid, p.ports, p.command)).collect::<Vec<_>>()) })),
                    "last": store.buffer.lines().skip(store.buffer.len().saturating_sub(3)).map(|l| l.text.clone()).collect::<Vec<_>>(),
                })
            }).collect::<Vec<_>>(),
        })
    }
}

/// The lines of `buffer` from line number `from` on: the numbers of those the filter lets
/// through, and how many errors there are among them (for the tab's badge).
fn scan(buffer: &LogBuffer, from: usize, matcher: &Matcher, filter: &Filter) -> (Vec<usize>, usize) {
    let dropped = buffer.dropped();
    let (mut shown, mut errors) = (Vec::new(), 0);
    for (index, line) in buffer.lines().enumerate().skip(from.saturating_sub(dropped)) {
        if line.stream != Stream::Note {
            errors += usize::from(line.level == Level::Error);
        }
        if passes(line, matcher, filter) {
            shown.push(dropped + index);
        }
    }
    (shown, errors)
}

impl Drop for LogPanel {
    /// The window this panel belongs to closed: its runs end with it, as they do when Agentty quits.
    fn drop(&mut self) {
        for tab in &self.tabs {
            if let TabState::Running { pid } | TabState::Stopping { pid } = tab.state {
                run_log::signal(pid, 1);
            }
        }
    }
}

/// Whether `line` gets through the filter.
fn passes(line: &LogLine, matcher: &Matcher, filter: &Filter) -> bool {
    filter.shows_level(line.level) && matcher.find(&line.text).is_some()
}

impl Workbench {
    pub(super) fn log(&self) -> &LogPanel {
        &self.runs.log
    }

    /// The tab of configuration `config` of `root`, if one was opened.
    pub(super) fn log_tab_of(&self, root: &std::path::Path, config: &str) -> Option<u64> {
        self.log().tabs.iter().find(|t| t.root == root && t.config == config).map(|t| t.id)
    }

    pub(super) fn log_tab_running(&self, id: u64) -> bool {
        self.log().tab(id).is_some_and(|t| t.state.is_running())
    }

    /// Runs `command` in a tab of the log (a new one, or the configuration's own again), shows it.
    pub(super) fn run_in_log(
        &mut self,
        root: PathBuf,
        config: &agentty_bridge::run_configs::RunConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (title, command, dir, named_ports) = (config.title(), config.command.clone(), config.dir.clone(), config.named_ports());
        let config = config.id.clone();
        self.ensure_log_search(window, cx);
        let id = match self.log_tab_of(&root, &config) {
            Some(id) => {
                let index = self.log().index_of(id).expect("found above");
                let tab = &mut self.runs.log.tabs[index];
                if tab.state.is_running() {
                    return self.show_log_tab(id, cx);
                }
                tab.command = command;
                tab.dir = dir;
                tab.title = title;
                tab.named_ports = named_ports;
                id
            }
            None => {
                self.runs.log.next_id += 1;
                let id = self.runs.log.next_id;
                self.runs.log.tabs.push(LogTab {
                    id,
                    root,
                    config,
                    title,
                    command,
                    dir,
                    named_ports,
                    state: TabState::Exited(None),
                    store: Rc::new(RefCell::new(Store::default())),
                    list: following_list(),
                    run: 0,
                    restart: false,
                    unseen_errors: 0,
                    follow: Rc::new(Cell::new(true)),
                    conflict: None,
                    run_start: 0,
                });
                let tab = self.runs.log.tabs.last().expect("just pushed");
                let follow = tab.follow.clone();
                tab.list.set_scroll_handler(move |event, _, _| follow.set(event.visible_range.end >= event.count));
                id
            }
        };
        self.launch_log_tab(id, cx);
        self.show_log_tab(id, cx);
    }

    /// Opens the panel at tab `id`.
    pub(super) fn show_log_tab(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(index) = self.log().index_of(id) else { return };
        let log = &mut self.runs.log;
        log.active = index;
        log.open = true;
        log.tabs[index].unseen_errors = 0;
        cx.notify();
    }

    fn launch_log_tab(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(index) = self.log().index_of(id) else { return };
        let (command, dir) = (self.log().tabs[index].command.clone(), self.log().tabs[index].dir.clone());
        let started = run_log::start(&command, &dir);
        let note = format!("▶ {command}  ·  {}", tilde(&dir));
        let tab = &mut self.runs.log.tabs[index];
        tab.run += 1;
        tab.restart = false;
        tab.conflict = None;
        tab.follow.set(true);
        let run = tab.run;
        {
            let mut store = tab.store.borrow_mut();
            tab.run_start = store.buffer.dropped() + store.buffer.len();
            store.buffer.note(&note, Level::Info);
        }
        match started {
            Ok((pid, mut events)) => {
                tab.state = TabState::Running { pid };
                cx.spawn(async move |this, cx| {
                    while let Some(event) = events.next().await {
                        // Bursts of output are taken together: one filter pass and one repaint.
                        let mut batch = vec![event];
                        while let Ok(event) = events.try_recv() {
                            batch.push(event);
                        }
                        let ended = batch.iter().any(|event| matches!(event, Event::Exited(_)));
                        // Once it ended the channel is let go: a daemon it left behind still
                        // holding its output would otherwise keep the reading threads alive.
                        if this.update(cx, |this, cx| this.log_events(id, run, batch, cx)).is_err() || ended {
                            break;
                        }
                    }
                })
                .detach();
            }
            Err(err) => {
                tab.state = TabState::Failed;
                tab.store.borrow_mut().buffer.note(&format!("✕ {err}"), Level::Error);
            }
        }
        self.append_shown(index);
        cx.notify();
    }

    fn log_events(&mut self, id: u64, run: u64, batch: Vec<Event>, cx: &mut Context<Self>) {
        let Some(index) = self.log().index_of(id) else { return };
        if self.log().tabs[index].run != run {
            return;
        }
        let mut ended = None;
        {
            let tab = &self.runs.log.tabs[index];
            // Whatever code a run ends with after it was asked to stop, it was stopped.
            let stopping = matches!(tab.state, TabState::Stopping { .. });
            let mut store = tab.store.borrow_mut();
            for event in batch {
                match event {
                    Event::Output(stream, bytes) => store.buffer.push(stream, &bytes),
                    Event::Exited(code) => {
                        let code = code.filter(|_| !stopping);
                        store.buffer.flush();
                        let text = match code {
                            Some(code) => tf(cx, "run.log_exited", &[("code", &code.to_string())]),
                            None => t(cx, "run.log_stopped").to_string(),
                        };
                        store.buffer.note(&format!("■ {text}"), if code.is_some_and(|c| c != 0) { Level::Error } else { Level::Info });
                        ended = Some(code);
                    }
                }
            }
        }
        let added_errors = self.append_shown(index);
        let log = &mut self.runs.log;
        let in_front = log.open && log.active == index;
        let tab = &mut log.tabs[index];
        if !in_front {
            tab.unseen_errors += added_errors;
        }
        if let Some(code) = ended {
            tab.state = TabState::Exited(code);
            if tab.restart {
                self.launch_log_tab(id, cx);
            } else if code.is_some_and(|c| c != 0) {
                self.look_for_port_conflict(id, cx);
            }
        }
        cx.notify();
    }

    /// A run that failed because its port was taken says so at its end: the panel then names what
    /// holds the port and offers to stop it and run again.
    fn look_for_port_conflict(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(index) = self.log().index_of(id) else { return };
        let tab = &mut self.runs.log.tabs[index];
        let ports = {
            let store = tab.store.borrow();
            // The end of this run's output only: an earlier run's "address in use" is old news.
            let first = tab.run_start.saturating_sub(store.buffer.dropped()).max(store.buffer.len().saturating_sub(80));
            run_log::port_conflict(store.buffer.lines().skip(first))
        };
        let Some(mut ports) = ports else { return };
        // The output didn't say which port: the ones the configuration names, if any.
        if ports.is_empty() {
            ports = tab.named_ports.clone();
        }
        tab.conflict = Some(Conflict { ports: ports.clone(), holders: None, busy: false });
        let run = tab.run;
        let root = tab.root.clone();
        let ours: Vec<u32> = self
            .log()
            .tabs
            .iter()
            .filter_map(|t| match t.state {
                TabState::Running { pid } | TabState::Stopping { pid } => Some(pid),
                _ => None,
            })
            .collect();
        cx.spawn(async move |this, cx| {
            // Only what listens on a port known to be the one: guessing (whatever serves in the
            // project) would offer to stop an unrelated server. And only what belongs to this
            // project may be stopped from here: the output that named the port is the script's,
            // which could name the user's database.
            let holders = cx
                .background_spawn(async move {
                    crate::procinfo::serving_on(&ports)
                        .into_iter()
                        .map(|process| {
                            let in_project = process.group.is_some_and(|g| ours.contains(&g))
                                || crate::procinfo::cwd_of(process.pid).is_some_and(|dir| dir.starts_with(&root));
                            (process, in_project)
                        })
                        .collect()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let Some(index) = this.log().index_of(id) else { return };
                let tab = &mut this.runs.log.tabs[index];
                if let Some(conflict) = tab.conflict.as_mut().filter(|_| tab.run == run) {
                    conflict.holders = Some(holders);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Stops what holds the port a run wanted, then starts the run again.
    fn free_port_and_restart(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(index) = self.log().index_of(id) else { return };
        let Some(conflict) = self.runs.log.tabs[index].conflict.as_mut() else { return };
        if conflict.busy {
            return;
        }
        conflict.busy = true;
        let holders: Vec<(u32, u16)> =
            conflict.holders.iter().flatten().filter(|(_, ours)| *ours).filter_map(|(p, _)| Some((p.pid, *p.ports.first()?))).collect();
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_spawn(async move {
                for (pid, port) in holders {
                    crate::procinfo::stop_serving(pid, port);
                }
            })
            .await;
            let _ = this.update(cx, |this, cx| this.restart_log_tab(id, cx));
        })
        .detach();
    }

    /// Brings the tab's shown lines up to date with its buffer after lines were added (and old
    /// ones dropped). Returns how many error lines came in.
    fn append_shown(&mut self, index: usize) -> usize {
        let matcher = self.log().matcher();
        let filter = self.log().filter.clone();
        let tab = &self.runs.log.tabs[index];
        let mut store = tab.store.borrow_mut();
        let store = &mut *store;
        let dropped = store.buffer.dropped();
        let gone = store.shown.iter().take_while(|n| **n < dropped).count();
        if gone > 0 {
            store.shown.drain(..gone);
            tab.list.splice(0..gone, 0);
        }
        let before = store.shown.len();
        let (shown, errors) = scan(&store.buffer, store.scanned, &matcher, &filter);
        store.scanned = dropped + store.buffer.len();
        store.shown.extend(shown);
        let added = store.shown.len() - before;
        if added > 0 {
            tab.list.splice(before..before, added);
            if tab.follow.get() {
                tab.list.scroll_to_reveal_item(store.shown.len() - 1);
            }
        }
        errors
    }

    /// Filters a tab's lines again from scratch (the filter changed).
    fn refilter_tab(&mut self, index: usize) {
        let matcher = self.log().matcher();
        let filter = self.log().filter.clone();
        let tab = &self.runs.log.tabs[index];
        let mut store = tab.store.borrow_mut();
        let dropped = store.buffer.dropped();
        let shown: Vec<usize> =
            store.buffer.lines().enumerate().filter(|(_, line)| passes(line, &matcher, &filter)).map(|(i, _)| dropped + i).collect();
        store.shown = shown;
        store.scanned = dropped + store.buffer.len();
        tab.list.reset(store.shown.len());
        if !store.shown.is_empty() {
            tab.list.scroll_to_reveal_item(store.shown.len() - 1);
        }
        tab.follow.set(true);
    }

    fn apply_log_filter(&mut self, cx: &mut Context<Self>) {
        let query = self.log().search.as_ref().map(|(input, _)| input.read(cx).text().to_string()).unwrap_or_default();
        let log = &mut self.runs.log;
        log.filter.query = query;
        match log.filter.matcher() {
            Ok(matcher) => {
                log.matcher = Some(Rc::new(matcher));
                log.filter_error = None;
            }
            // A pattern being typed: the words keep filtering as they were until it is complete,
            // while a level picked meanwhile still applies.
            Err(err) => log.filter_error = Some(err),
        }
        for index in 0..self.log().tabs.len() {
            self.refilter_tab(index);
        }
        cx.notify();
    }

    fn ensure_log_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.log().search.is_some() {
            return;
        }
        let input = cx.new(|cx| TextInput::new("", t(cx, "run.log_search"), window, cx));
        let subscription = cx.subscribe(&input, |this, input, event: &TextInputEvent, cx| match event {
            TextInputEvent::Changed => this.apply_log_filter(cx),
            TextInputEvent::Cancelled if !input.read(cx).text().is_empty() => {
                input.update(cx, |input, cx| input.set_text("", cx));
                this.apply_log_filter(cx);
            }
            _ => {}
        });
        self.runs.log.search = Some((input, subscription));
    }

    /// Stops tab `id`'s process: Ctrl-C, then harder if it doesn't listen.
    pub(super) fn stop_log_tab(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(index) = self.log().index_of(id) else { return };
        let tab = &mut self.runs.log.tabs[index];
        let TabState::Running { pid } = tab.state else { return };
        tab.state = TabState::Stopping { pid };
        let run = tab.run;
        run_log::signal(pid, 0);
        cx.spawn(async move |this, cx| {
            for force in [1, 2] {
                cx.background_executor().timer(STOP_GRACE).await;
                let still =
                    this.update(cx, |this, _| this.log().tab(id).is_some_and(|t| t.run == run && t.state == TabState::Stopping { pid }));
                if !matches!(still, Ok(true)) {
                    break;
                }
                run_log::signal(pid, force);
            }
        })
        .detach();
        cx.notify();
    }

    /// Stops tab `id`'s process if it runs, and starts it again in the same tab once it ended.
    pub(super) fn restart_log_tab(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(index) = self.log().index_of(id) else { return };
        // The configuration's command now: a `package.json` may have changed since.
        let tab = &self.log().tabs[index];
        if let Some(config) = self.run_configs(&tab.root).iter().find(|c| c.id == tab.config).cloned() {
            let tab = &mut self.runs.log.tabs[index];
            tab.command = config.command;
            tab.dir = config.dir;
        }
        let tab = &mut self.runs.log.tabs[index];
        match tab.state {
            TabState::Running { .. } => {
                tab.restart = true;
                self.stop_log_tab(id, cx);
            }
            TabState::Stopping { .. } => tab.restart = true,
            TabState::Exited(_) | TabState::Failed => {
                let note = format!("── {} ──", t(cx, "run.restart"));
                tab.store.borrow_mut().buffer.note(&note, Level::Info);
                self.launch_log_tab(id, cx);
            }
        }
        self.show_log_tab(id, cx);
    }

    /// Closes tab `id`, ending its process.
    fn close_log_tab(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(index) = self.log().index_of(id) else { return };
        let tab = self.runs.log.tabs.remove(index);
        if let TabState::Running { pid } | TabState::Stopping { pid } = tab.state {
            run_log::signal(pid, 1);
            cx.spawn(async move |_, cx| {
                cx.background_executor().timer(Duration::from_secs(3)).await;
                run_log::signal(pid, 2);
            })
            .detach();
        }
        let log = &mut self.runs.log;
        if log.tabs.is_empty() {
            log.open = false;
            log.active = 0;
        } else if log.active >= index && log.active > 0 {
            log.active -= 1;
        }
        cx.notify();
    }

    fn clear_log_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(tab) = self.runs.log.tabs.get(index) {
            tab.store.borrow_mut().buffer.clear();
            self.refilter_tab(index);
            cx.notify();
        }
    }

    /// While the panel's top edge is dragged.
    pub(super) fn drag_log_panel(&mut self, pointer_y: f32, viewport: f32, cx: &mut Context<Self>) {
        let Some((start_y, start_height)) = self.log().drag else { return };
        let height = (start_height + start_y - pointer_y).clamp(MIN_HEIGHT, (viewport - MIN_TERMINALS - 120.).max(MIN_HEIGHT));
        gpui::BorrowAppContext::update_global::<crate::settings::SettingsStore, _>(cx, |store, _| store.settings.run_log_height = height);
        cx.notify();
    }

    /// The debug driver's `log` command: `` (toggle), `filter:<words>`, `regex:<pattern>`,
    /// `level:error|warn|all`, `close:<id>`, `restart:<id>`, `stop:<id>`, `clear`.
    pub(super) fn debug_log(&mut self, argument: &str, cx: &mut Context<Self>) {
        let set_query = |this: &mut Self, text: &str, regex: bool, cx: &mut Context<Self>| {
            this.runs.log.filter.regex = regex;
            if let Some(input) = this.log().search.as_ref().map(|(input, _)| input.clone()) {
                input.update(cx, |input, cx| input.set_text(text.to_string(), cx));
            }
            this.apply_log_filter(cx);
        };
        let id = |rest: &str| rest.parse::<u64>().ok();
        match argument.split_once(':') {
            None if argument == "clear" => self.clear_log_tab(self.log().active, cx),
            None => {
                self.runs.log.open = !self.log().open && !self.log().tabs.is_empty();
                cx.notify();
            }
            Some(("filter", text)) => set_query(self, text, false, cx),
            Some(("regex", text)) => set_query(self, text, true, cx),
            Some(("level", level)) => {
                self.runs.log.filter.level = match level {
                    "error" => Some(Level::Error),
                    "warn" => Some(Level::Warn),
                    _ => None,
                };
                self.apply_log_filter(cx);
            }
            Some(("close", rest)) => {
                if let Some(id) = id(rest) {
                    self.close_log_tab(id, cx);
                }
            }
            Some(("restart", rest)) => {
                if let Some(id) = id(rest) {
                    self.restart_log_tab(id, cx);
                }
            }
            Some(("stop", rest)) => {
                if let Some(id) = id(rest) {
                    self.stop_log_tab(id, cx);
                }
            }
            _ => {}
        }
    }

    // -- rendering ----------------------------------------------------------------------------------

    pub(super) fn render_log_panel(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let log = self.log();
        if !log.open || log.tabs.is_empty() {
            return None;
        }
        let height = crate::settings::settings(cx).run_log_height.max(MIN_HEIGHT);
        let active = log.active.min(log.tabs.len() - 1);
        let tab = &log.tabs[active];

        let splitter = div()
            .id("run-log-resize")
            .group("run-log-resize")
            .h(px(5.))
            .w_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .cursor(gpui::CursorStyle::ResizeUpDown)
            .child(
                div()
                    .h(px(1.))
                    .w_full()
                    .bg(hex(Chrome::BORDER))
                    .group_hover("run-log-resize", |s| s.h(px(5.)).bg(hex(Chrome::ACCENT)))
                    .when(log.drag.is_some(), |d| d.h(px(5.)).bg(hex(Chrome::ACCENT))),
            )
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    let height = crate::settings::settings(cx).run_log_height.max(MIN_HEIGHT);
                    this.runs.log.drag = Some((f32::from(event.position.y), height));
                    cx.stop_propagation();
                    cx.notify();
                }),
            );

        Some(
            div()
                .id("run-log")
                .h(px(height))
                .flex_shrink_0()
                .flex()
                .flex_col()
                .bg(hex(Chrome::EDITOR))
                .child(splitter)
                .child(self.render_log_tabs(active, cx))
                .child(self.render_log_toolbar(tab, cx))
                .children(self.render_log_conflict(tab, window, cx))
                .child(self.render_log_lines(tab, cx))
                .into_any_element(),
        )
    }

    fn render_log_tabs(&self, active: usize, cx: &mut Context<Self>) -> AnyElement {
        let log = self.log();
        let mut tabs = div().id("run-log-tabs").flex().flex_1().min_w_0().h_full().overflow_x_scroll();
        for (index, tab) in log.tabs.iter().enumerate() {
            let selected = index == active;
            let id = tab.id;
            let color = state_color(tab.state);
            let running = tab.state.is_running();
            tabs = tabs.child(
                div()
                    .id(("run-log-tab", tab.id as usize))
                    .h_full()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap_2()
                    .pl_3()
                    .pr_1()
                    .max_w(px(260.))
                    .border_r_1()
                    .border_color(hex(Chrome::BORDER))
                    .cursor_pointer()
                    .bg(hex(if selected { Chrome::EDITOR } else { Chrome::TAB_INACTIVE }))
                    .when(selected, |d| d.border_t_2().border_color(hex(Chrome::ACCENT)))
                    .text_color(hex(if selected { Chrome::BRIGHT } else { Chrome::MUTED }))
                    .hover(|s| s.text_color(hex(Chrome::BRIGHT)))
                    .tooltip(Tooltip::text(format!("{}\n{}", tab.command, tilde(&tab.dir)), None))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.show_log_tab(id, cx)))
                    .child(match tab.state {
                        // Asked to stop and still ending: a spinner.
                        TabState::Stopping { .. } => {
                            crate::ui::dot_spinner(("run-log-tab-stopping", tab.id as usize), 10., hex(color)).into_any_element()
                        }
                        _ => div()
                            .flex_shrink_0()
                            .size(px(8.))
                            .rounded_full()
                            .map(|d| if running { d.bg(hex(color)) } else { d.border_1().border_color(hex(color)) })
                            .into_any_element(),
                    })
                    .child(div().min_w_0().truncate().t_small().child(tab.title.clone()))
                    .when(tab.unseen_errors > 0, |d| {
                        d.child(
                            div()
                                .flex_shrink_0()
                                .px_1()
                                .rounded_sm()
                                .t_caption()
                                .bg(hex_alpha(Chrome::ERROR, 0.25))
                                .text_color(hex(Chrome::ERROR))
                                .child(tab.unseen_errors.min(999).to_string()),
                        )
                    })
                    .child(
                        icon_only_sized(("run-log-tab-close", tab.id as usize), "x", 18., 11., |_, _, _| {})
                            .tooltip(Tooltip::text(t(cx, if running { "run.log_close_running" } else { "run.log_close" }), None))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                cx.stop_propagation();
                                this.close_log_tab(id, cx);
                            })),
                    ),
            );
        }
        let tab = &log.tabs[active];
        let id = tab.id;
        let running = tab.state.is_running();
        let stopping = matches!(tab.state, TabState::Stopping { .. });
        let actions = div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap_0p5()
            .px_1p5()
            .child(if stopping {
                div()
                    .id("run-log-stopping")
                    .size(px(24.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .tooltip(Tooltip::text(t(cx, "run.state.stopping"), None))
                    .child(crate::ui::dot_spinner("run-log-stopping-spin", 12., hex(Chrome::WARNING)))
            } else if running {
                tool_button("run-log-stop", "square", Chrome::ERROR, t(cx, "run.stop").into())
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.stop_log_tab(id, cx)))
            } else {
                tool_button("run-log-start", "play", Chrome::SUCCESS, t(cx, "run.start").into())
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.restart_log_tab(id, cx)))
            })
            .child(
                tool_button("run-log-restart", "rotate-cw", Chrome::BLUE, t(cx, "run.restart").into())
                    .when(stopping, |d| d.opacity(0.4))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.restart_log_tab(id, cx))),
            )
            .child(div().w(px(1.)).h(px(14.)).mx_1().bg(hex(Chrome::BORDER)))
            .child(
                tool_button("run-log-clear", "trash-2", Chrome::MUTED, t(cx, "run.log_clear").into())
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.clear_log_tab(active, cx))),
            )
            .child(tool_button("run-log-hide", "minimize-2", Chrome::MUTED, t(cx, "run.log_hide").into()).on_click(cx.listener(
                |this, _: &ClickEvent, window, cx| {
                    this.runs.log.open = false;
                    this.focus_active(window, cx);
                    cx.notify();
                },
            )));
        div()
            .h(px(32.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .bg(hex(Chrome::TAB_INACTIVE))
            .border_b_1()
            .border_color(hex(Chrome::BORDER))
            .child(tabs)
            .child(actions)
            .into_any_element()
    }

    fn render_log_toolbar(&self, tab: &LogTab, cx: &mut Context<Self>) -> AnyElement {
        let log = self.log();
        let toggle = |key: &'static str, label: &'static str, on: bool, tip: &'static str, cx: &mut Context<Self>| {
            div()
                .id(key)
                .flex_shrink_0()
                .h(px(22.))
                .min_w(px(24.))
                .px_1p5()
                .flex()
                .items_center()
                .justify_center()
                .rounded_md()
                .cursor_pointer()
                .t_caption()
                .font_weight(crate::theme::EMPHASIS)
                .when(on, |d| d.bg(hex_alpha(Chrome::ACCENT, 0.28)).text_color(hex(Chrome::BRIGHT)))
                .when(!on, |d| d.text_color(hex(Chrome::MUTED)).hover(|s| s.bg(hex(Chrome::HOVER)).text_color(hex(Chrome::BRIGHT))))
                .tooltip(Tooltip::text(t(cx, tip), None))
                .child(label)
        };
        let store = tab.store.borrow();
        let level = log.filter.level;
        // ALL · WARN n · ERROR n, one segmented control.
        let segment =
            |key: &'static str, label: &'static str, value: Option<Level>, count: Option<usize>, color: u32, cx: &mut Context<Self>| {
                let on = level == value;
                div()
                    .id(key)
                    .h(px(20.))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .t_caption()
                    .font_weight(crate::theme::EMPHASIS)
                    .when(on, |d| d.bg(hex(Chrome::SELECTED)).text_color(hex(Chrome::BRIGHT)))
                    .when(!on, |d| d.text_color(hex(Chrome::MUTED)).hover(|s| s.text_color(hex(Chrome::BRIGHT))))
                    .tooltip(Tooltip::text(t(cx, "run.log_level"), None))
                    .child(label)
                    .children(count.filter(|n| *n > 0).map(|n| {
                        div().px_1().rounded_sm().bg(hex_alpha(color, 0.22)).text_color(hex(color)).child(if n > 999 {
                            "999+".to_string()
                        } else {
                            n.to_string()
                        })
                    }))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.runs.log.filter.level = value;
                        this.apply_log_filter(cx);
                    }))
            };
        let levels = div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .p_0p5()
            .rounded_md()
            .bg(hex(Chrome::TAB_INACTIVE))
            .border_1()
            .border_color(hex(Chrome::BORDER))
            .child(segment("run-log-all", "ALL", None, None, Chrome::MUTED, cx))
            .child(segment("run-log-warn", "WARN", Some(Level::Warn), Some(store.buffer.warnings()), Chrome::WARNING, cx))
            .child(segment("run-log-error", "ERROR", Some(Level::Error), Some(store.buffer.errors()), Chrome::ERROR, cx));
        let filtering = !log.filter.query.is_empty() || log.filter.level.is_some();
        let count = if filtering {
            tf(cx, "run.log_count_filtered", &[("shown", &store.shown.len().to_string()), ("total", &store.buffer.len().to_string())])
        } else {
            tf(cx, "run.log_count", &[("total", &store.buffer.len().to_string())])
        };
        let (list, follow) = (tab.list.clone(), tab.follow.clone());
        let following = tab.follow.get();
        let last = store.shown.len();
        div()
            .h(px(34.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_1p5()
            .px_2()
            .border_b_1()
            .border_color(hex(Chrome::BORDER))
            .bg(hex(Chrome::PANEL))
            .child(
                div()
                    .w(px(300.))
                    .flex_shrink()
                    .min_w(px(140.))
                    .h(px(24.))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .rounded_md()
                    .bg(hex(Chrome::EDITOR))
                    .border_1()
                    .border_color(hex(if log.filter_error.is_some() { Chrome::ERROR } else { Chrome::BORDER }))
                    .child(icon("search", 12., hex(Chrome::MUTED)))
                    .child(div().flex_1().min_w_0().t_small().children(log.search.as_ref().map(|(input, _)| input.clone())))
                    .child(toggle("run-log-case", "Aa", log.filter.case_sensitive, "run.log_case", cx).on_click(cx.listener(
                        |this, _: &ClickEvent, _, cx| {
                            this.runs.log.filter.case_sensitive = !this.log().filter.case_sensitive;
                            this.apply_log_filter(cx);
                        },
                    )))
                    .child(toggle("run-log-regex", ".*", log.filter.regex, "run.log_regex", cx).on_click(cx.listener(
                        |this, _: &ClickEvent, _, cx| {
                            this.runs.log.filter.regex = !this.log().filter.regex;
                            this.apply_log_filter(cx);
                        },
                    ))),
            )
            .child(levels)
            .child(toggle("run-log-wrap", "↵", log.wrap, "run.log_wrap", cx).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.runs.log.wrap = !this.log().wrap;
                cx.notify();
            })))
            .child(div().flex_1().min_w_0().truncate().t_caption().text_color(hex(Chrome::ERROR)).children(log.filter_error.clone()))
            .child(div().flex_shrink_0().t_caption().text_color(hex(Chrome::MUTED)).child(count))
            .child(
                div()
                    .id("run-log-end")
                    .flex_shrink_0()
                    .size(px(24.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .cursor_pointer()
                    // Lit while new lines come in out of sight: the click takes the view back to them.
                    .when(!following, |d| d.bg(hex_alpha(Chrome::ACCENT, 0.3)))
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .tooltip(Tooltip::text(t(cx, "run.log_end"), None))
                    .child(icon("arrow-down", 12., hex(if following { Chrome::MUTED } else { Chrome::BRIGHT })))
                    .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                        follow.set(true);
                        if last > 0 {
                            list.scroll_to_reveal_item(last - 1);
                        }
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    /// The port a failed run wanted is taken: what holds it, and a way to free it and run again.
    fn render_log_conflict(&self, tab: &LogTab, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let conflict = tab.conflict.as_ref()?;
        let id = tab.id;
        let ports = conflict.ports.iter().map(|p| format!(":{p}")).collect::<Vec<_>>().join(" ");
        let message =
            if ports.is_empty() { t(cx, "run.log_conflict_any").to_string() } else { tf(cx, "run.log_conflict", &[("ports", &ports)]) };
        let holders: Vec<String> = conflict
            .holders
            .iter()
            .flatten()
            .map(|(p, _)| {
                let ports = p.ports.iter().map(|port| format!(":{port}")).collect::<Vec<_>>().join(" ");
                format!("{ports}  {}  · PID {}", p.command, p.pid)
            })
            .collect();
        // Offered only when everything holding the port belongs to this project: anything else
        // (the user's database, another project's server) is theirs to stop, from the port list.
        let all_ours = conflict.holders.as_ref().is_some_and(|h| !h.is_empty() && h.iter().all(|(_, ours)| *ours));
        let outside = conflict.holders.as_ref().is_some_and(|h| h.iter().any(|(_, ours)| !ours));
        let found = all_ours;
        let viewport = window.viewport_size();
        let root = tab.root.clone();
        Some(
            div()
                .flex_shrink_0()
                .mx_2()
                .mt_2()
                .px_3()
                .py_2()
                .rounded_md()
                .border_1()
                .border_color(hex_alpha(Chrome::WARNING, 0.6))
                .bg(hex_alpha(Chrome::WARNING, 0.1))
                .flex()
                .items_start()
                .gap_3()
                .child(div().pt_0p5().child(icon("triangle-alert", 14., hex(Chrome::WARNING))))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .child(div().t_small().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(message))
                        .children(
                            holders.into_iter().map(|line| div().t_caption().text_color(hex(Chrome::FOREGROUND)).truncate().child(line)),
                        )
                        .when(conflict.holders.as_ref().is_some_and(Vec::is_empty), |d| {
                            d.child(div().t_caption().text_color(hex(Chrome::MUTED)).child(t(cx, "run.log_conflict_unknown")))
                        })
                        .when(outside, |d| {
                            d.child(div().t_caption().text_color(hex(Chrome::MUTED)).child(t(cx, "run.log_conflict_outside")))
                        }),
                )
                .child(
                    div()
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap_1()
                        .when(found, |d| {
                            d.child(
                                div()
                                    .id("run-log-free-port")
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .t_small()
                                    .cursor_pointer()
                                    .bg(hex(Chrome::WARNING))
                                    .text_color(hex(0x1e1e1e))
                                    .hover(|s| s.opacity(0.85))
                                    .when(conflict.busy, |d| d.opacity(0.5))
                                    .child(t(cx, "run.log_conflict_kill"))
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.free_port_and_restart(id, cx))),
                            )
                        })
                        .child(crate::ui::action_button(
                            "run-log-show-ports",
                            t(cx, "run.log_conflict_show"),
                            cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.open_run_panel(gpui::point(viewport.width - px(12.), viewport.height - px(30.)), root.clone(), cx);
                            }),
                        ))
                        .child(icon_only_sized("run-log-conflict-close", "x", 22., 11., |_, _, _| {}).on_click(cx.listener(
                            move |this, _: &ClickEvent, _, cx| {
                                if let Some(index) = this.log().index_of(id) {
                                    this.runs.log.tabs[index].conflict = None;
                                    cx.notify();
                                }
                            },
                        ))),
                )
                .into_any_element(),
        )
    }

    fn render_log_lines(&self, tab: &LogTab, cx: &mut Context<Self>) -> AnyElement {
        let store = tab.store.clone();
        let matcher = self.log().matcher();
        let wrap = self.log().wrap;
        let font = SharedString::from(crate::settings::terminal_font(cx));
        let workbench = cx.entity().downgrade();
        let lines = list(tab.list.clone(), move |row, _, _| {
            let store = store.borrow();
            let dropped = store.buffer.dropped();
            let Some((number, line)) = store.shown.get(row).and_then(|n| Some((n + 1, store.buffer.get(n - dropped)?))) else {
                return div().into_any_element();
            };
            render_line(number, line, &matcher, wrap, &font, workbench.clone())
        })
        .size_full();
        let count = tab.store.borrow().shown.len();
        let empty = count == 0;
        // Following the end: it stays in view when the area changes too (a banner, a resize).
        if tab.follow.get() && count > 0 {
            tab.list.scroll_to_reveal_item(count - 1);
        }
        div()
            .id("run-log-lines")
            .group(crate::ui::SCROLL_GROUP)
            .relative()
            .flex_1()
            .min_h_0()
            .pt_1()
            .bg(hex(Chrome::EDITOR))
            .child(lines)
            .when(empty, |d| {
                let key = if tab.state.is_running() { "run.log_waiting" } else { "run.log_nothing" };
                d.child(div().absolute().top(px(12.)).left(px(16.)).t_small().text_color(hex(Chrome::MUTED)).child(t(cx, key)))
            })
            .child(crate::ui::list_scrollbar(tab.list.clone()))
            .into_any_element()
    }
}

/// A list that starts at the top, as a log reads.
fn following_list() -> ListState {
    ListState::new(0, ListAlignment::Top, px(400.))
}

fn state_color(state: TabState) -> u32 {
    match state {
        TabState::Running { .. } => Chrome::SUCCESS,
        TabState::Stopping { .. } => Chrome::WARNING,
        TabState::Exited(Some(code)) if code != 0 => Chrome::ERROR,
        TabState::Failed => Chrome::ERROR,
        TabState::Exited(_) => Chrome::MUTED,
    }
}

/// An icon button tinted for what it does: green to run, red to stop, blue to restart.
fn tool_button(id: &'static str, glyph: &'static str, color: u32, tip: SharedString) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex_shrink_0()
        .size(px(24.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .cursor_pointer()
        .hover(move |s| s.bg(hex_alpha(if color == Chrome::MUTED { Chrome::FOREGROUND } else { color }, 0.16)))
        .tooltip(Tooltip::text(tip, None))
        .child(icon(glyph, 13., hex(color)))
}

fn render_line(
    number: usize,
    line: &LogLine,
    matcher: &Matcher,
    wrap: bool,
    font: &SharedString,
    workbench: gpui::WeakEntity<Workbench>,
) -> AnyElement {
    let note = line.stream == Stream::Note;
    let matches = matcher.find(&line.text).unwrap_or_default();
    let links = run_log::links(&line.text);
    let base = match line.level {
        Level::Error => Some(Chrome::ERROR),
        Level::Warn => Some(Chrome::WARNING),
        _ => None,
    };
    let text = StyledText::new(if line.text.is_empty() { SharedString::from(" ") } else { SharedString::from(line.text.clone()) })
        .with_highlights(highlights(line, &matches, &links));
    let urls: Vec<String> = links.iter().map(|r| run_log::openable(&line.text[r.clone()])).collect();
    // ⌘-click (Ctrl-click elsewhere) opens an address, as in the terminals.
    let text = gpui::InteractiveText::new(("run-log-line", number), text).on_click(links, move |index, window, cx| {
        if !crate::keymap::link_modifier(&window.modifiers()) {
            return;
        }
        if let Some(url) = urls.get(index).cloned() {
            let _ = workbench.update(cx, |this, cx| this.open_link(url, cx));
        }
    });
    let color = if note { Chrome::MUTED } else { base.filter(|_| line.styles.is_empty()).unwrap_or(Chrome::FOREGROUND) };
    div()
        .flex()
        .w_full()
        .pr_2()
        .font_family(font.clone())
        .text_size(px(12.))
        .line_height(px(18.))
        .border_l_2()
        .border_color(match (note, base) {
            (true, _) => hex_alpha(Chrome::ACCENT, 0.7),
            (false, Some(c)) => hex_alpha(c, 0.8),
            (false, None) => hex_alpha(0, 0.),
        })
        .when(line.level == Level::Error && !note, |d| d.bg(hex_alpha(Chrome::ERROR, 0.07)))
        .when(note, |d| d.mt_1().bg(hex_alpha(Chrome::ACCENT, 0.06)))
        .child(
            div()
                .w(px(44.))
                .flex_shrink_0()
                .pr_2()
                .text_right()
                .text_size(px(11.))
                .text_color(hex_alpha(Chrome::MUTED, 0.55))
                .child(if note { String::new() } else { number.to_string() }),
        )
        .child(div().flex_1().min_w_0().text_color(hex(color)).when(!wrap, |d| d.whitespace_nowrap().overflow_hidden()).child(text))
        .into_any_element()
}

/// The line's colours, the filter's matches and its links as one list of non-overlapping
/// highlights.
fn highlights(line: &LogLine, matches: &[Range<usize>], links: &[Range<usize>]) -> Vec<(Range<usize>, HighlightStyle)> {
    let mut cuts: Vec<usize> = vec![0, line.text.len()];
    for range in line.styles.iter().map(|(r, _)| r).chain(matches.iter()).chain(links.iter()) {
        cuts.push(range.start);
        cuts.push(range.end);
    }
    cuts.sort_unstable();
    cuts.dedup();
    let mut result = Vec::new();
    for pair in cuts.windows(2) {
        let (start, end) = (pair[0], pair[1]);
        if start >= end || !line.text.is_char_boundary(start) || !line.text.is_char_boundary(end) {
            continue;
        }
        let inside = |r: &Range<usize>| r.start <= start && end <= r.end;
        let style = line.styles.iter().find(|(r, _)| inside(r)).map(|(_, s)| *s);
        let matched = matches.iter().any(inside);
        let link = links.iter().any(inside);
        if style.is_none() && !matched && !link {
            continue;
        }
        let color = if matched {
            Some(hex(Chrome::BRIGHT))
        } else if link {
            Some(hex(Chrome::BLUE))
        } else {
            style.and_then(|s| s.fg).map(|c| hex(c.rgb()))
        };
        result.push((
            start..end,
            HighlightStyle {
                color,
                font_weight: style.filter(|s| s.bold).map(|_| crate::theme::EMPHASIS),
                background_color: matched.then(|| hex_alpha(0xe2c08d, 0.45)),
                underline: link.then(|| gpui::UnderlineStyle { thickness: px(1.), color: Some(hex_alpha(Chrome::BLUE, 0.7)), wavy: false }),
                ..Default::default()
            },
        ));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run_log::{Color, Stream, Style};

    #[test]
    fn lines_scanned_in_batches_keep_their_numbers() {
        let mut buffer = LogBuffer::default();
        let filter = Filter::default();
        let matcher = filter.matcher().unwrap();
        let mut shown = Vec::new();
        let mut scanned = 0;
        for batch in [&b"a\nb\n"[..], b"c\nERROR d\n", b"e\n"] {
            buffer.push(Stream::Out, batch);
            let (more, _) = scan(&buffer, scanned, &matcher, &filter);
            shown.extend(more);
            scanned = buffer.dropped() + buffer.len();
        }
        assert_eq!(shown, [0, 1, 2, 3, 4]);
        let errors = Filter { level: Some(Level::Error), ..Default::default() };
        let (only, count) = scan(&buffer, 0, &errors.matcher().unwrap(), &errors);
        assert_eq!((only, count), (vec![3], 1));
    }

    #[test]
    fn colours_and_matches_do_not_overlap() {
        let line = LogLine {
            text: "INFO started on :8080".into(),
            styles: vec![(0..4, Style { fg: Some(Color::Indexed(2)), bold: false })],
            level: Level::Info,
            stream: Stream::Out,
        };
        let found = highlights(&line, std::slice::from_ref(&(2..12)), &[]);
        let ranges: Vec<Range<usize>> = found.iter().map(|(r, _)| r.clone()).collect();
        assert_eq!(ranges, [0..2, 2..4, 4..12]);
        assert!(found[0].1.background_color.is_none());
        assert!(found[1].1.background_color.is_some());
    }
}
