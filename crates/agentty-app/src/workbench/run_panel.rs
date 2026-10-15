//! The run button, like an IDE's: a project's run configurations (`package.json` scripts with its
//! package manager, Spring Boot applications with Gradle or Maven, commands the user added) offered
//! in the bar of every pane working in it, terminal or agent alike. A run goes to a tab of the run
//! log under the terminals (`log_panel`), never typed into a pane, so nothing lands over what a
//! pane is doing. The button turns into stop and restart while it runs.
//!
//! The status bar has a chip for the active pane's project that opens the run panel: every run
//! configuration with its state and ports, and every process serving a port in that project —
//! started here or anywhere else — each with a button that stops it.

use super::{Pane, Workbench};
use crate::i18n::{t, tf};
use crate::procinfo::ServingProcess;
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, icon_only_sized, popover, tilde, IconSize, Tooltip, TypeScale};
use agentty_bridge::run_configs::{self, CustomRun, RunConfig, Tool};
use gpui::{
    div, prelude::*, px, AnyElement, ClickEvent, Context, Entity, Focusable, FontWeight, Pixels, Point, SharedString, Subscription, Window,
};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// A project's configurations are read again after this, so a `package.json` an agent just wrote
/// shows up without a restart.
const REREAD_AFTER: Duration = Duration::from_secs(15);
/// How often the open panel looks for processes serving ports.
const PANEL_EVERY: Duration = Duration::from_secs(5);

struct Loaded {
    read_at: Instant,
    list: Vec<RunConfig>,
}

/// The "Add command" dialog.
pub(super) struct AddRun {
    root: PathBuf,
    /// Folder the command runs in, relative to the project.
    dir: String,
    name: Entity<TextInput>,
    command: Entity<TextInput>,
    _subscriptions: Vec<Subscription>,
}

#[derive(Default)]
pub(super) struct RunState {
    /// A pane folder's project (its working tree, or the folder itself), or `None` where nothing is
    /// offered (the home folder, `/`). Kept so a render doesn't look at the disk.
    roots: RefCell<HashMap<PathBuf, Option<PathBuf>>>,
    configs: HashMap<PathBuf, Loaded>,
    loading: HashSet<PathBuf>,
    /// Projects whose configurations changed while they were being read: read again after.
    stale: HashSet<PathBuf>,
    /// The configuration last started in each project: what the button starts.
    last: HashMap<PathBuf, String>,
    /// The run button's menu: the pane it belongs to, its project, where it opens.
    menu: Option<(Option<Pane>, PathBuf, Point<Pixels>)>,
    /// The status bar's run panel: where it opens, the project it shows.
    panel: Option<(Point<Pixels>, PathBuf)>,
    panel_generation: u64,
    /// Processes serving ports in the panel's project (`None` until first read).
    processes: Option<Vec<ServingProcess>>,
    /// Processes being stopped from the panel.
    stopping: HashSet<u32>,
    pub(super) adding: Option<AddRun>,
    /// The run log under the terminals.
    pub(super) log: super::log_panel::LogPanel,
}

impl RunState {
    /// For the debug driver's `probe`.
    pub(super) fn debug_state(&self) -> serde_json::Value {
        serde_json::json!({
            "configs": self.configs.iter().map(|(root, loaded)| (root.to_string_lossy().to_string(), loaded.list.iter().map(|c| serde_json::json!({
                "id": c.id, "title": c.title(), "command": c.command, "dir": c.dir, "tool": c.tool,
            })).collect::<Vec<_>>())).collect::<HashMap<_, _>>(),
            "panel": self.panel.as_ref().map(|(_, root)| root),
            "log": self.log.debug_state(),
            "processes": self.processes.as_ref().map(|list| list.iter().map(|p| serde_json::json!({
                "pid": p.pid, "ports": p.ports, "command": p.command,
            })).collect::<Vec<_>>()),
        })
    }
}

fn tool_icon_color(tool: Tool) -> u32 {
    match tool {
        Tool::Gradle | Tool::Maven => Chrome::GREEN,
        Tool::Custom => Chrome::MUTED,
        _ => Chrome::ORANGE,
    }
}

/// What a configuration is, in a few words: "pnpm · run dev", "Spring Boot · Gradle (Kotlin)".
fn kind_label(config: &RunConfig) -> String {
    match config.tool {
        Tool::Gradle | Tool::Maven => {
            format!("Spring Boot · {}{}", config.tool.label(), if config.kotlin { " (Kotlin)" } else { " (Java)" })
        }
        _ => config.command.clone(),
    }
}

impl Workbench {
    // -- projects and their configurations --------------------------------------------------------

    /// The project `pane` works in, where the run button has something to offer.
    fn run_root(&self, pane: &Pane, cx: &gpui::App) -> Option<PathBuf> {
        let cwd = pane.read(cx).display_cwd();
        self.run_root_of(&cwd)
    }

    fn run_root_of(&self, cwd: &Path) -> Option<PathBuf> {
        if let Some(root) = self.runs_state().roots.borrow().get(cwd) {
            return root.clone();
        }
        let home = crate::launch::home_dir();
        let root = agentty_bridge::worktree::tree_root(cwd).unwrap_or_else(|| project_root(cwd, &home));
        // Not the home folder or anything above it: a scan there finds everything and nothing.
        let root = (!home.starts_with(&root) && root.parent().is_some()).then_some(root);
        self.runs_state().roots.borrow_mut().insert(cwd.to_path_buf(), root.clone());
        root
    }

    /// The pane of the tab in front when it is the tab's only one (it has no pane bar then), with
    /// the terminals shown rather than a page or the editor.
    pub(super) fn single_pane_tab(&self) -> Option<Pane> {
        if self.page.is_some() {
            return None;
        }
        let ws = self.workspaces.get(self.active_workspace)?;
        let tab = ws.tabs.get(ws.active_tab).filter(|tab| !tab.todo)?;
        matches!(tab.root, super::panes::PaneNode::Leaf(_)).then(|| tab.active.clone())
    }

    fn runs_state(&self) -> &RunState {
        &self.runs
    }

    pub(super) fn run_configs(&self, root: &Path) -> &[RunConfig] {
        self.runs.configs.get(root).map(|l| l.list.as_slice()).unwrap_or_default()
    }

    /// Reads the configurations of the projects shown now (the panes of the active tab) when they
    /// are not known yet or were read a while ago. Called on render.
    pub(super) fn prepare_runs(&mut self, cx: &mut Context<Self>) {
        let mut roots: Vec<PathBuf> = Vec::new();
        let panes: Vec<Pane> = self
            .workspaces
            .get(self.active_workspace)
            .and_then(|ws| ws.tabs.get(ws.active_tab))
            .map(|tab| tab.root.leaves())
            .unwrap_or_default();
        for pane in &panes {
            if let Some(root) = self.run_root(pane, cx).filter(|r| !roots.contains(r)) {
                roots.push(root);
            }
        }
        if let Some((_, root)) = &self.runs.panel {
            if !roots.contains(root) {
                roots.push(root.clone());
            }
        }
        for root in roots {
            let fresh = self.runs.configs.get(&root).is_some_and(|l| l.read_at.elapsed() < REREAD_AFTER);
            if !fresh && !self.runs.loading.contains(&root) {
                self.load_run_configs(root, cx);
            }
        }
    }

    fn load_run_configs(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        let custom = crate::settings::settings(cx).run_configs.get(&root.to_string_lossy().to_string()).cloned().unwrap_or_default();
        self.runs.loading.insert(root.clone());
        cx.spawn(async move |this, cx| {
            let scanned = root.clone();
            let list = cx.background_spawn(async move { run_configs::configs(&scanned, &custom) }).await;
            let _ = this.update(cx, |this, cx| {
                this.runs.loading.remove(&root);
                // Folders are looked at again now and then: one that became a project (`git init`,
                // a new `package.json`) is noticed. Never more than a few hundred are kept.
                {
                    let mut roots = this.runs.roots.borrow_mut();
                    if roots.len() > 256 {
                        roots.clear();
                    } else {
                        roots.retain(|_, found| found.as_ref() == Some(&root));
                    }
                }
                let changed = this.runs.configs.get(&root).is_none_or(|l| l.list != list);
                this.runs.configs.insert(root.clone(), Loaded { read_at: Instant::now(), list });
                // A command was added or removed while this was read: what was read is already old.
                if this.runs.stale.remove(&root) {
                    this.load_run_configs(root, cx);
                }
                if changed {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Reads `root` again at once (a command was added or removed).
    fn reload_run_configs(&mut self, root: &Path, cx: &mut Context<Self>) {
        if self.runs.loading.contains(root) {
            self.runs.stale.insert(root.to_path_buf());
        } else {
            self.load_run_configs(root.to_path_buf(), cx);
        }
    }

    /// The configuration the button starts: the one last started here, else the first.
    /// The configuration `pane`'s button stands for: the one last started in the project, else the
    /// one of the pane's folder, else the first.
    fn primary_config<'a>(&'a self, root: &Path, pane: Option<&Pane>, cx: &gpui::App) -> Option<&'a RunConfig> {
        let list = self.run_configs(root);
        let cwd = pane.map(|p| p.read(cx).current_dir());
        self.runs
            .last
            .get(root)
            .and_then(|id| list.iter().find(|c| &c.id == id))
            .or_else(|| list.iter().find(|c| Some(&c.dir) == cwd.as_ref()))
            .or_else(|| list.first())
    }

    /// The log tab `config` of `root` ran in, if it was run.
    fn run_of(&self, root: &Path, config: &str) -> Option<u64> {
        self.log_tab_of(root, config)
    }

    fn run_alive(&self, tab: u64) -> bool {
        self.log_tab_running(tab)
    }

    /// Asked to stop (or restart) and not ended yet: shown with a spinner.
    fn stopping_run(&self, tab: u64) -> bool {
        self.log().tab(tab).is_some_and(|t| matches!(t.state, super::log_panel::TabState::Stopping { .. }))
    }

    /// Ports the processes of log tab `tab` serve.
    fn ports_of_run(&self, tab: u64) -> Vec<u16> {
        let group = match self.log().tab(tab).map(|t| t.state) {
            Some(super::log_panel::TabState::Running { pid } | super::log_panel::TabState::Stopping { pid }) => pid,
            _ => return Vec::new(),
        };
        let mut ports: Vec<u16> =
            self.runs.processes.iter().flatten().filter(|p| p.group == Some(group)).flat_map(|p| p.ports.clone()).collect();
        ports.sort_unstable();
        ports.dedup();
        ports
    }

    fn show_run(&mut self, tab: u64, cx: &mut Context<Self>) {
        self.runs.panel = None;
        self.show_log_tab(tab, cx);
    }

    // -- starting -----------------------------------------------------------------------------------

    /// Starts configuration `id` of `root` in its tab of the run log, whatever pane the button was
    /// in: nothing is typed into a terminal (and never over an agent). A run already going is shown.
    pub(super) fn start_run(&mut self, root: PathBuf, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(config) = self.run_configs(&root).iter().find(|c| c.id == id).cloned() else { return };
        self.runs.last.insert(root.clone(), config.id.clone());
        self.runs.menu = None;
        crate::metrics::track(cx, "feature_used", serde_json::json!({ "feature": "run" }));
        self.run_in_log(root, &config, window, cx);
    }

    // -- the panel's processes ----------------------------------------------------------------------

    fn toggle_run_panel(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if self.runs.panel.is_some() || self.just_dismissed("run-panel") {
            self.runs.panel = None;
            return cx.notify();
        }
        let Some(root) = self.active_pane().and_then(|p| self.run_root(&p, cx)) else { return };
        self.open_run_panel(position, root, cx);
    }

    pub(super) fn open_run_panel(&mut self, position: Point<Pixels>, root: PathBuf, cx: &mut Context<Self>) {
        self.runs.menu = None;
        // The same project again (a refresh): its list stays up until the new one is in.
        if self.runs.panel.as_ref().is_none_or(|(_, shown)| *shown != root) {
            self.runs.processes = None;
        }
        self.runs.panel = Some((position, root));
        self.runs.panel_generation += 1;
        let generation = self.runs.panel_generation;
        let handle = self.window_handle;
        cx.spawn(async move |this, cx| loop {
            let Ok(Some((root, shells))) = this.read_with(cx, |this, cx| {
                let (_, root) = this.runs.panel.clone().filter(|_| this.runs.panel_generation == generation)?;
                let shells: Vec<u32> = this
                    .all_panes()
                    .iter()
                    .filter(|p| this.run_root(p, cx).as_ref() == Some(&root))
                    .filter_map(|p| p.read(cx).shell_pid())
                    .collect();
                Some((root, shells))
            }) else {
                break;
            };
            if Self::on_screen(handle, cx) {
                let found = cx.background_spawn(async move { crate::procinfo::serving_in(&root, &shells) }).await;
                let _ = this.update(cx, |this, cx| {
                    if this.runs.panel_generation == generation && this.runs.processes.as_ref() != Some(&found) {
                        this.runs.processes = Some(found);
                        cx.notify();
                    }
                });
            }
            cx.background_executor().timer(PANEL_EVERY).await;
        })
        .detach();
        cx.notify();
    }

    fn refresh_run_panel(&mut self, cx: &mut Context<Self>) {
        if let Some((position, root)) = self.runs.panel.clone() {
            self.reload_run_configs(&root, cx);
            self.open_run_panel(position, root, cx);
        }
    }

    /// Stops a process serving a port in the panel's project.
    fn stop_process(&mut self, pid: u32, port: u16, cx: &mut Context<Self>) {
        if !self.runs.stopping.insert(pid) {
            return;
        }
        cx.notify();
        cx.spawn(async move |this, cx| {
            let stopped = cx.background_spawn(async move { crate::procinfo::stop_serving(pid, port) }).await;
            let _ = this.update(cx, |this, cx| {
                this.runs.stopping.remove(&pid);
                if let Some(list) = this.runs.processes.as_mut().filter(|_| stopped) {
                    list.retain(|p| p.pid != pid);
                }
                let text = if stopped {
                    tf(cx, "run.process_stopped", &[("port", &port.to_string())])
                } else {
                    tf(cx, "run.process_not_stopped", &[("pid", &pid.to_string())])
                };
                this.show_toast(text, cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn menu_closed(&mut self, cx: &mut Context<Self>) {
        self.runs.menu = None;
        cx.notify();
    }

    // -- commands added by hand ---------------------------------------------------------------------

    fn open_add_run(&mut self, root: PathBuf, cwd: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        self.runs.menu = None;
        self.runs.panel = None;
        let dir = cwd
            .as_deref()
            .and_then(|cwd| cwd.strip_prefix(&root).ok())
            .map(|rel| rel.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        let name = cx.new(|cx| TextInput::new("", t(cx, "run.add_name_placeholder"), window, cx));
        let command = cx.new(|cx| TextInput::new("", "npm run dev", window, cx));
        let mut subscriptions = Vec::new();
        for (input, other) in [(name.clone(), command.clone()), (command.clone(), name.clone())] {
            subscriptions.push(cx.subscribe_in(&input, window, move |this, _, event: &TextInputEvent, window, cx| match event {
                TextInputEvent::Confirmed => this.finish_add_run(true, window, cx),
                TextInputEvent::Cancelled => this.finish_add_run(false, window, cx),
                TextInputEvent::Next | TextInputEvent::Previous => window.focus(&other.focus_handle(cx)),
                _ => {}
            }));
        }
        window.focus(&command.focus_handle(cx));
        self.runs.adding = Some(AddRun { root, dir, name, command, _subscriptions: subscriptions });
        cx.notify();
    }

    fn finish_add_run(&mut self, save: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(adding) = self.runs.adding.take() else { return };
        let command = adding.command.read(cx).text().trim().to_string();
        if save && command.is_empty() {
            // Nothing to run: the dialog stays, with the field to fill.
            window.focus(&adding.command.focus_handle(cx));
            self.runs.adding = Some(adding);
            return;
        }
        if save {
            let run = CustomRun { name: adding.name.read(cx).text().trim().to_string(), command, dir: adding.dir.clone() };
            let key = adding.root.to_string_lossy().to_string();
            let index = crate::settings::settings(cx).run_configs.get(&key).map_or(0, Vec::len);
            crate::settings::update_settings(cx, |settings| settings.run_configs.entry(key).or_default().push(run));
            // What was just added is what the button starts.
            self.runs.last.insert(adding.root.clone(), format!("custom#{index}"));
            self.reload_run_configs(&adding.root, cx);
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    fn remove_custom_run(&mut self, root: PathBuf, config: &RunConfig, cx: &mut Context<Self>) {
        let key = root.to_string_lossy().to_string();
        // Found by what it is, not by its place: the list shown may be older than the settings.
        let same = |run: &CustomRun| {
            run.command == config.command
                && (if run.name.trim().is_empty() { &run.command } else { &run.name }) == &config.name
                && (if run.dir.is_empty() { root.clone() } else { root.join(&run.dir) }) == config.dir
        };
        let Some(index) = crate::settings::settings(cx).run_configs.get(&key).and_then(|list| list.iter().position(same)) else {
            return;
        };
        crate::settings::update_settings(cx, |settings| {
            if let Some(list) = settings.run_configs.get_mut(&key) {
                if index < list.len() {
                    list.remove(index);
                }
                if list.is_empty() {
                    settings.run_configs.remove(&key);
                }
            }
        });
        // The ones after it moved up: their log tabs and the button's choice follow.
        for tab in self.runs.log.tabs.iter_mut().filter(|t| t.root == root) {
            if let Some(n) = tab.config.strip_prefix("custom#").and_then(|n| n.parse::<usize>().ok()) {
                if n == index {
                    tab.config = String::from("custom#removed");
                } else if n > index {
                    tab.config = format!("custom#{}", n - 1);
                }
            }
        }
        self.runs.last.remove(&root);
        self.reload_run_configs(&root, cx);
        cx.notify();
    }

    /// The debug driver's `run` command: `` (toggle the panel), `reload`, `start:<id>`, `stop:<id>`,
    /// `restart:<id>`, `kill:<pid>:<port>`.
    pub(super) fn debug_run(&mut self, argument: &str, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.active_pane().and_then(|p| self.run_root(&p, cx));
        let run_of = |this: &Self, id: &str| root.as_ref().and_then(|root| this.run_of(root, id));
        match argument.split_once(':') {
            None if argument == "reload" => {
                if let Some(root) = root {
                    self.reload_run_configs(&root, cx);
                }
            }
            None => self.toggle_run_panel(gpui::point(px(1200.), px(760.)), cx),
            Some(("start", id)) => {
                if let Some(root) = root.clone() {
                    self.start_run(root, id, window, cx);
                }
            }
            Some(("stop", id)) => {
                if let Some(run) = run_of(self, id) {
                    self.stop_log_tab(run, cx);
                }
            }
            Some(("restart", id)) => {
                if let Some(run) = run_of(self, id) {
                    self.restart_log_tab(run, cx);
                }
            }
            Some(("kill", rest)) => {
                if let Some((pid, port)) = rest.split_once(':').and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?))) {
                    self.stop_process(pid, port, cx);
                }
            }
            _ => {}
        }
    }

    // -- rendering ----------------------------------------------------------------------------------

    /// The run button in a pane's bar (or the tab strip, for a tab of one pane): ▶ and the
    /// configuration's name, ■ and ↻ while it runs, and ▾ for the others.
    pub(super) fn render_run_control(&self, pane: &Pane, cx: &mut Context<Self>) -> Option<AnyElement> {
        let root = self.run_root(pane, cx)?;
        let primary = self.primary_config(&root, Some(pane), cx)?.clone();
        let key = pane.entity_id().as_u64() as usize;
        let running = self.run_of(&root, &primary.id).filter(|run| self.run_alive(*run));
        let restarting = running.is_some_and(|run| self.stopping_run(run));
        let height = px(20.);
        let segment = |id: &'static str| {
            div()
                .id((id, key))
                .h(height)
                .flex()
                .items_center()
                .gap_1()
                .px_1p5()
                .cursor_pointer()
                .hover(|s| s.bg(hex(Chrome::HOVER)))
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
        };
        let start_root = root.clone();
        let start_id = primary.id.clone();
        let main = match running {
            // Asked to stop and still ending (or restarting): a spinner, nothing to click.
            Some(_) if restarting => segment("run-stopping")
                .cursor_default()
                .tooltip(Tooltip::text(t(cx, "run.state.stopping"), None))
                .child(crate::ui::dot_spinner(("run-stopping-spin", key), 11., hex(Chrome::WARNING)))
                .child(div().max_w(px(110.)).truncate().text_color(hex(Chrome::MUTED)).child(primary.title())),
            Some(run) => segment("run-stop")
                .tooltip(Tooltip::text(tf(cx, "run.stop_named", &[("name", &primary.title())]), None))
                .child(icon("square", 11., hex(Chrome::ERROR)))
                .child(div().max_w(px(110.)).truncate().text_color(hex(Chrome::BRIGHT)).child(primary.title()))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    this.stop_log_tab(run, cx);
                })),
            None => segment("run-start")
                .tooltip(Tooltip::text(tf(cx, "run.start_named", &[("command", &primary.command)]), None))
                .child(icon("play", 11., hex(Chrome::GREEN)))
                .child(div().max_w(px(110.)).truncate().child(primary.title()))
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    cx.stop_propagation();
                    this.start_run(start_root.clone(), &start_id, window, cx);
                })),
        };
        let restart = running.filter(|_| !restarting).map(|run| {
            segment("run-restart")
                .tooltip(Tooltip::text(t(cx, "run.restart"), None))
                .when(restarting, |d| d.opacity(0.5))
                .child(icon("rotate-cw", 11., hex(Chrome::BLUE)))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    this.restart_log_tab(run, cx);
                }))
        });
        let menu_pane = pane.clone();
        let more = segment("run-more")
            .px_0p5()
            .tooltip(Tooltip::text(t(cx, "run.more"), None))
            .child(icon("chevron-down", 11., hex(Chrome::MUTED)))
            .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                cx.stop_propagation();
                if this.just_dismissed("run-menu") {
                    return;
                }
                let position = event.position();
                this.runs.menu = Some((Some(menu_pane.clone()), root.clone(), gpui::point(position.x - px(200.), position.y + px(12.))));
                cx.notify();
            }));
        Some(
            div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .rounded_md()
                .overflow_hidden()
                .border_1()
                .border_color(hex(match running {
                    Some(_) if restarting => Chrome::WARNING,
                    Some(_) => Chrome::GREEN,
                    None => Chrome::BORDER,
                }))
                .when(running.is_some(), |d| d.bg(hex_alpha(Chrome::GREEN, 0.10)))
                .t_small()
                .text_color(hex(Chrome::FOREGROUND))
                .child(main)
                .children(restart)
                .child(div().w(px(1.)).h(px(12.)).bg(hex(Chrome::BORDER)))
                .child(more)
                .into_any_element(),
        )
    }

    /// The run button's menu: every configuration of the project, and a way to add one.
    pub(super) fn render_run_menu(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (pane, root, position) = self.runs.menu.clone()?;
        let mut list = popover().w(px(320.)).on_mouse_down_out(cx.listener(|this, _, _, cx| {
            this.note_dismissed("run-menu");
            this.menu_closed(cx);
        }));
        for (index, config) in self.run_configs(&root).iter().enumerate() {
            let run = self.run_of(&root, &config.id).filter(|run| self.run_alive(*run));
            let (start_root, id) = (root.clone(), config.id.clone());
            list = list.child(
                div()
                    .id(("run-menu-item", index))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.start_run(start_root.clone(), &id, window, cx);
                    }))
                    .child(icon(
                        if run.is_some() { "square" } else { "play" },
                        11.,
                        hex(if run.is_some() { Chrome::ERROR } else { tool_icon_color(config.tool) }),
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().truncate().t_body().text_color(hex(Chrome::BRIGHT)).child(config.title()))
                            .child(div().truncate().t_caption().text_color(hex(Chrome::MUTED)).child(kind_label(config))),
                    )
                    .when_some(run, |d, run| {
                        d.child(
                            icon_only_sized(("run-menu-stop", index), "square", 20., 11., |_, _, _| {})
                                .tooltip(Tooltip::text(t(cx, "run.stop"), None))
                                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    cx.stop_propagation();
                                    this.stop_log_tab(run, cx);
                                })),
                        )
                    }),
            );
        }
        let cwd = pane.as_ref().map(|p| p.read(cx).display_cwd());
        let (add_root, panel_root) = (root.clone(), root.clone());
        list = list
            .child(div().my_1().h(px(1.)).bg(hex(Chrome::BORDER)))
            .child(crate::ui::menu_item(
                "run-menu-add",
                t(cx, "run.add"),
                cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.open_add_run(add_root.clone(), cwd.clone(), window, cx);
                }),
            ))
            .child(crate::ui::menu_item(
                "run-menu-panel",
                t(cx, "run.manage"),
                cx.listener(move |this, _: &ClickEvent, window, cx| {
                    let bottom = window.viewport_size();
                    this.open_run_panel(gpui::point(bottom.width - px(12.), bottom.height - px(30.)), panel_root.clone(), cx);
                }),
            ));
        let list = crate::ui::fit_to_window(list, "run-menu", window);
        Some(
            gpui::deferred(gpui::anchored().position(position).snap_to_window_with_margin(px(8.)).child(list))
                .with_priority(3)
                .into_any_element(),
        )
    }

    /// "▶ 2 running" in the status bar, for the active pane's project. Opens the run panel.
    pub(super) fn render_run_chip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let pane = self.active_pane()?;
        let root = self.run_root(&pane, cx)?;
        let configs = self.run_configs(&root);
        // Runs of the log going, and panes of the project serving a port (`npm run dev` typed by hand).
        let log_runs = self.log().tabs.iter().filter(|t| t.root == root && t.state.is_running()).count();
        let others: HashSet<u32> = self
            .all_panes()
            .iter()
            .filter(|p| self.run_root(p, cx).as_ref() == Some(&root))
            .flat_map(|p| self.servers.listeners.get(&p.read(cx).pane_id).into_iter().flatten().map(|l| l.pid))
            .collect();
        let running = log_runs + others.len();
        if configs.is_empty() && running == 0 {
            return None;
        }
        let open = self.runs.panel.is_some();
        let label = if running > 0 { tf(cx, "run.chip_running", &[("n", &running.to_string())]) } else { t(cx, "run.chip").to_string() };
        Some(
            div()
                .id("status-run")
                .tooltip(Tooltip::text(t(cx, "run.chip_tooltip"), None))
                .h_full()
                .px_1p5()
                .flex()
                .items_center()
                .gap_1()
                .cursor_pointer()
                .when(open, |d| d.bg(hex(Chrome::SELECTED)))
                .hover(|s| s.bg(hex(Chrome::HOVER)))
                .child(icon("play", 12., hex(if running > 0 { Chrome::GREEN } else { Chrome::MUTED })))
                .child(div().text_color(hex(if running > 0 || open { Chrome::FOREGROUND } else { Chrome::MUTED })).child(label))
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|this, event: &ClickEvent, window, cx| {
                    let viewport = window.viewport_size();
                    let x = event.position().x.max(px(380.)).min(viewport.width - px(8.));
                    this.toggle_run_panel(gpui::point(x, viewport.height - px(30.)), cx);
                }))
                .into_any_element(),
        )
    }

    /// The run panel over the status bar: configurations with their state and ports, and every
    /// process serving a port in the project, each with a button that stops it.
    pub(super) fn render_run_panel(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (position, root) = self.runs.panel.clone()?;
        let project = root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| tilde(&root));
        let section = |text: String| {
            div()
                .px_2()
                .pt_2()
                .pb_1()
                .t_caption()
                .font_weight(crate::theme::EMPHASIS)
                .text_color(hex(Chrome::MUTED))
                .child(text.to_uppercase())
        };
        let header = div()
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .pb_1()
            .border_b_1()
            .border_color(hex(Chrome::BORDER))
            .child(icon("play", IconSize::BUTTON, hex(Chrome::GREEN)))
            .child(div().t_body().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(t(cx, "run.title")))
            .child(
                div()
                    .id("run-panel-project")
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .t_small()
                    .text_color(hex(Chrome::MUTED))
                    .tooltip(Tooltip::text(tilde(&root), None))
                    .child(project),
            )
            .child(
                icon_only_sized("run-panel-refresh", "refresh-cw", 22., 12., |_, _, _| {})
                    .tooltip(Tooltip::text(t(cx, "usage.refresh"), None))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.refresh_run_panel(cx))),
            )
            .child(icon_only_sized("run-panel-close", "x", 22., 12., |_, _, _| {}).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.runs.panel = None;
                cx.notify();
            })));

        let mut body = div().id("run-panel-body").flex().flex_col().max_h(px(460.)).overflow_y_scroll();
        // Configurations.
        body = body.child(section(t(cx, "run.configs").to_string()));
        let configs = self.run_configs(&root);
        if configs.is_empty() {
            let key = if self.runs.loading.contains(&root) { "run.loading" } else { "run.none" };
            body = body.child(div().px_2().py_1().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, key)));
        }
        for (index, config) in configs.iter().enumerate() {
            let ports = self.run_of(&root, &config.id).map(|run| self.ports_of_run(run)).unwrap_or_default();
            body = body.child(self.render_run_row(index, &root, config, &ports, cx));
        }
        let cwd = self.active_pane().map(|p| p.read(cx).display_cwd());
        let add_root = root.clone();
        body = body.child(div().px_2().py_1().child(crate::ui::action_button_with_icon(
            "run-panel-add",
            "plus",
            t(cx, "run.add"),
            cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.open_add_run(add_root.clone(), cwd.clone(), window, cx);
            }),
        )));

        // Processes serving ports.
        body = body.child(section(t(cx, "run.ports").to_string()));
        match &self.runs.processes {
            _ if cfg!(windows) => {
                body = body.child(div().px_2().py_1().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "run.ports_unsupported")));
            }
            None => body = body.child(crate::ui::loading_row(t(cx, "run.loading"))),
            Some(list) if list.is_empty() => {
                body = body.child(div().px_2().py_1().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "run.no_ports")));
            }
            Some(list) => {
                for (index, process) in list.iter().enumerate() {
                    body = body.child(self.render_process_row(index, process, cx));
                }
            }
        }

        let panel = popover()
            .w(px(420.))
            .p_2()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                // The add dialog opened from here is not a click outside.
                if this.runs.adding.is_none() {
                    this.note_dismissed("run-panel");
                    this.runs.panel = None;
                    cx.notify();
                }
            }))
            .child(header)
            .child(body);
        let _ = window;
        Some(
            gpui::deferred(
                gpui::anchored().anchor(gpui::Corner::BottomRight).position(position).snap_to_window_with_margin(px(8.)).child(panel),
            )
            .with_priority(3)
            .into_any_element(),
        )
    }

    fn render_run_row(&self, index: usize, root: &Path, config: &RunConfig, ports: &[u16], cx: &mut Context<Self>) -> AnyElement {
        let run = self.run_of(root, &config.id);
        let alive = run.filter(|run| self.run_alive(*run));
        let restarting = alive.is_some_and(|run| self.stopping_run(run));
        let failed = match run {
            Some(tab) => self.log().tab(tab).and_then(|t| match t.state {
                super::log_panel::TabState::Exited(Some(code)) if code != 0 => Some(code.to_string()),
                super::log_panel::TabState::Failed => Some("?".into()),
                _ => None,
            }),
            None => None,
        };
        let failed_label = failed.map(|code| tf(cx, "run.state.failed", &[("code", &code)]));
        let (state_key, color) = match (alive, run) {
            (Some(_), _) if restarting => ("run.state.stopping", Chrome::WARNING),
            (Some(_), _) => ("run.state.running", Chrome::SUCCESS),
            (None, Some(_)) if failed_label.is_some() => ("run.state.failed", Chrome::ERROR),
            (None, Some(_)) => ("run.state.stopped", Chrome::MUTED),
            (None, None) => ("run.state.idle", Chrome::MUTED),
        };
        let button = |id: &'static str, glyph: &'static str, tip: String| {
            icon_only_sized((id, index), glyph, 22., 12., |_, _, _| {}).tooltip(Tooltip::text(tip, None))
        };
        let mut actions = div().flex().flex_shrink_0().items_center();
        match alive {
            Some(_) if restarting => {
                actions = actions.child(div().size(px(22.)).flex().items_center().justify_center().child(crate::ui::dot_spinner(
                    ("run-row-stopping", index),
                    12.,
                    hex(Chrome::WARNING),
                )));
            }
            Some(run) => {
                actions = actions
                    .child(
                        button("run-row-stop", "square", t(cx, "run.stop").to_string())
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.stop_log_tab(run, cx))),
                    )
                    .child(
                        button("run-row-restart", "rotate-cw", t(cx, "run.restart").to_string())
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.restart_log_tab(run, cx))),
                    );
            }
            None => {
                let (start_root, id) = (root.to_path_buf(), config.id.clone());
                actions = actions.child(button("run-row-start", "play", t(cx, "run.start").to_string()).on_click(cx.listener(
                    move |this, _: &ClickEvent, window, cx| {
                        this.runs.panel = None;
                        this.start_run(start_root.clone(), &id, window, cx);
                    },
                )));
            }
        }
        if let Some(run) = run {
            actions = actions.child(
                button("run-row-logs", "scroll-text", t(cx, "run.logs").to_string())
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.show_run(run, cx))),
            );
        }
        if config.is_custom() {
            let (remove_root, removed) = (root.to_path_buf(), config.clone());
            actions = actions.child(
                button("run-row-remove", "trash-2", t(cx, "run.remove").to_string())
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.remove_custom_run(remove_root.clone(), &removed, cx))),
            );
        }
        let mut meta = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_1()
            .t_caption()
            .text_color(hex(Chrome::MUTED))
            .child(div().text_color(hex(color)).child(failed_label.clone().unwrap_or_else(|| t(cx, state_key).to_string())))
            .child(div().min_w_0().max_w(px(220.)).truncate().child(format!("· {}", kind_label(config))));
        for port in ports.iter().filter(|_| alive.is_some()) {
            let port = *port;
            meta = meta.child(
                div()
                    .id(SharedString::from(format!("run-row-port-{index}-{port}")))
                    .px_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .bg(hex_alpha(Chrome::GREEN, 0.15))
                    .text_color(hex(Chrome::GREEN))
                    .hover(|s| s.bg(hex_alpha(Chrome::GREEN, 0.3)))
                    .tooltip(Tooltip::text(t(cx, "run.open_port"), None))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.open_link(format!("http://localhost:{port}"), cx)))
                    .child(format!(":{port}")),
            );
        }
        div()
            .id(("run-row", index))
            .px_2()
            .py_1p5()
            .rounded_md()
            .flex()
            .items_center()
            .gap_2()
            .hover(|s| s.bg(hex(Chrome::HOVER)))
            .tooltip(Tooltip::text(format!("{}\n{}", config.command, tilde(&config.dir)), None))
            .child(div().flex_shrink_0().size(px(8.)).rounded_full().map(|d| {
                if alive.is_some() {
                    d.bg(hex(color))
                } else {
                    d.border_1().border_color(hex(color))
                }
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(
                        div()
                            .w_full()
                            .truncate()
                            .t_small()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(hex(Chrome::BRIGHT))
                            .child(config.title()),
                    )
                    .child(meta),
            )
            .child(actions)
            .into_any_element()
    }

    fn render_process_row(&self, index: usize, process: &ServingProcess, cx: &mut Context<Self>) -> AnyElement {
        let stopping = self.runs.stopping.contains(&process.pid);
        let pid = process.pid;
        let first = process.ports.first().copied().unwrap_or_default();
        let mut ports = div().flex().flex_wrap().items_center().gap_1();
        for port in &process.ports {
            let port = *port;
            ports = ports.child(
                div()
                    .id(SharedString::from(format!("run-process-port-{pid}-{port}")))
                    .px_1()
                    .rounded_sm()
                    .t_small()
                    .cursor_pointer()
                    .bg(hex_alpha(Chrome::BLUE, 0.15))
                    .text_color(hex(Chrome::BLUE))
                    .hover(|s| s.bg(hex_alpha(Chrome::BLUE, 0.3)))
                    .tooltip(Tooltip::text(t(cx, "run.open_port"), None))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.open_link(format!("http://localhost:{port}"), cx)))
                    .child(format!(":{port}")),
            );
        }
        div()
            .id(("run-process", index))
            .px_2()
            .py_1p5()
            .rounded_md()
            .flex()
            .items_center()
            .gap_2()
            .hover(|s| s.bg(hex(Chrome::HOVER)))
            .child(ports.flex_shrink_0())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().w_full().truncate().t_small().text_color(hex(Chrome::BRIGHT)).child(process.command.clone()))
                    .child(div().t_caption().text_color(hex(Chrome::MUTED)).child(format!("PID {pid}"))),
            )
            .child(
                icon_only_sized(("run-process-kill", index), "x", 22., 13., |_, _, _| {})
                    .tooltip(Tooltip::text(tf(cx, "run.kill", &[("port", &first.to_string())]), None))
                    .when(stopping, |d| d.opacity(0.4))
                    .when(!stopping, |d| d.hover(|s| s.bg(hex_alpha(Chrome::ERROR, 0.35))))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.stop_process(pid, first, cx))),
            )
            .into_any_element()
    }

    /// The "Add command" dialog.
    pub(super) fn render_run_add_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let adding = self.runs.adding.as_ref()?;
        let folder = if adding.dir.is_empty() { tilde(&adding.root) } else { tilde(&adding.root.join(&adding.dir)) };
        let field = |label: &'static str, input: &Entity<TextInput>, cx: &mut Context<Self>| {
            div().flex().flex_col().gap_1().child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, label))).child(
                div()
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .bg(hex(Chrome::PANEL))
                    .border_1()
                    .border_color(hex(Chrome::BORDER))
                    .t_body()
                    .text_color(hex(Chrome::BRIGHT))
                    .child(input.clone()),
            )
        };
        let button = |id: &'static str, label: &'static str, primary: bool, cx: &mut Context<Self>| {
            div()
                .id(id)
                .px_3()
                .py_1p5()
                .rounded_md()
                .t_body()
                .cursor_pointer()
                .when(primary, |d| d.bg(hex(Chrome::ACCENT)).text_color(hex(Chrome::ON_ACCENT)))
                .when(!primary, |d| d.bg(hex(0x2d2d30)).when(crate::theme::is_light(), crate::ui::outlined).text_color(hex(Chrome::BRIGHT)))
                .hover(|s| s.opacity(0.85))
                .child(t(cx, label))
        };
        Some(
            div()
                .id("run-add-overlay")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::hsla(0., 0., 0., 0.45))
                .occlude()
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .id("run-add-dialog")
                        .w(px(440.))
                        .p_5()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .rounded_xl()
                        .bg(hex(Chrome::OVERLAY))
                        .border_1()
                        .border_color(hex(Chrome::OVERLAY_BORDER))
                        .shadow_lg()
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .t_title()
                                .font_weight(crate::theme::EMPHASIS)
                                .text_color(hex(Chrome::BRIGHT))
                                .child(t(cx, "run.add_title")),
                        )
                        .child(field("run.add_command", &adding.command, cx))
                        .child(field("run.add_name", &adding.name, cx))
                        .child(div().t_caption().text_color(hex(Chrome::MUTED)).child(tf(cx, "run.add_folder", &[("folder", &folder)])))
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .child(
                                    button("run-add-cancel", "confirm.cancel", false, cx)
                                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.finish_add_run(false, window, cx))),
                                )
                                .child(
                                    button("run-add-save", "save", true, cx)
                                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.finish_add_run(true, window, cx))),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}

/// The project a folder outside git belongs to: the highest of it and the two folders above it
/// that holds a project file, so `demo/web` belongs to `demo` when `demo` has a `package.json`.
/// Decided from the files alone, so the answer never depends on what was read before.
fn project_root(cwd: &Path, home: &Path) -> PathBuf {
    const MARKERS: &[&str] =
        &["package.json", "pnpm-workspace.yaml", "build.gradle", "build.gradle.kts", "settings.gradle", "settings.gradle.kts", "pom.xml"];
    cwd.ancestors()
        .take(3)
        .take_while(|dir| !home.starts_with(dir))
        .filter(|dir| MARKERS.iter().any(|m| dir.join(m).is_file()))
        .last()
        .map_or_else(|| cwd.to_path_buf(), Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_outside_git_belongs_to_the_project_above_it() {
        let base = std::env::temp_dir().join(format!("agentty-run-root-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (demo, web, lone) = (base.join("demo"), base.join("demo/web"), base.join("lone/src"));
        for dir in [&web, &lone] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(demo.join("package.json"), "{}").unwrap();
        std::fs::write(web.join("package.json"), "{}").unwrap();
        let home = PathBuf::from("/nonexistent-home");
        assert_eq!(project_root(&web, &home), demo);
        assert_eq!(project_root(&demo, &home), demo);
        // No project file around: the folder itself.
        assert_eq!(project_root(&lone, &home), lone);
        // Never the home folder.
        assert_eq!(project_root(&web, &demo), web);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn spring_and_node_configs_say_what_they_are() {
        let node = RunConfig {
            id: "#dev".into(),
            name: "dev".into(),
            scope: String::new(),
            tool: Tool::Pnpm,
            command: "pnpm run dev".into(),
            dir: PathBuf::from("/p"),
            kotlin: false,
            script: String::new(),
        };
        assert_eq!(kind_label(&node), "pnpm run dev");
        let spring = RunConfig { tool: Tool::Gradle, kotlin: true, command: "./gradlew bootRun".into(), ..node };
        assert_eq!(kind_label(&spring), "Spring Boot · Gradle (Kotlin)");
    }

    #[test]
    fn every_label_is_translated() {
        for key in [
            "run.chip",
            "run.chip_running",
            "run.chip_tooltip",
            "run.title",
            "run.configs",
            "run.ports",
            "run.none",
            "run.no_ports",
            "run.ports_unsupported",
            "run.start",
            "run.start_named",
            "run.stop",
            "run.stop_named",
            "run.restart",
            "run.logs",
            "run.remove",
            "run.more",
            "run.add",
            "run.manage",
            "run.kill",
            "run.open_port",
            "run.process_stopped",
            "run.process_not_stopped",
            "run.add_title",
            "run.add_command",
            "run.add_name",
            "run.add_name_placeholder",
            "run.add_folder",
            "run.state.running",
            "run.state.stopped",
            "run.state.idle",
            "run.state.restarting",
            "run.state.stopping",
            "run.state.failed",
            "run.log_waiting",
            "run.log_nothing",
            "run.log_conflict",
            "run.log_conflict_any",
            "run.log_conflict_unknown",
            "run.log_conflict_outside",
            "run.log_conflict_kill",
            "run.log_conflict_show",
        ] {
            for language in [crate::settings::Language::En, crate::settings::Language::Ko] {
                assert_ne!(crate::i18n::tr(language, key), key, "{key}");
            }
        }
    }
}
