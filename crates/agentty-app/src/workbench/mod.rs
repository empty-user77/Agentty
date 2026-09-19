//! The window's root view. Hierarchy: group → workspace (one sidebar row) → tab → split panes.

mod account_usage;
mod accounts_page;
mod agent_panel;
mod browser;
mod browser_control;
mod chrome;
mod confirm;
mod context_menu;
mod drop_split;
mod files_panel;
mod find;
pub mod flow;
mod guide;
mod harness;
mod hud_settings;
mod idea;
mod install_hint;
mod layout;
pub mod mini;
mod notices;
mod onboarding;
mod palette;
pub mod panes;
mod persist;
mod picker;
mod plugin_host;
mod plugin_panel;
mod plugins_page;
mod processes;
mod prompt_dialog;
mod proxy_page;
pub mod resume_hint;
mod servers;
mod service_status;
mod session_viewer;
mod settings_page;
mod status_menus;
mod system_page;
mod tab_menu;
pub mod update;
pub mod worktrees;

use crate::agent_signal::AgentSignal;
use crate::i18n::{t, tf};
use crate::launch::{home_dir, LaunchSpec, PaneKind};
use crate::settings::{settings, update_settings};
use crate::terminal::{TerminalEvent, TerminalView};
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::{hex, Chrome};
use crate::usage_view::UsageView;
use agentty_bridge::model::{Agent, SessionInfo};
use gpui::{
    actions, div, prelude::*, px, Bounds, Context, Entity, EntityId, FocusHandle, Focusable, MouseButton, MouseMoveEvent, Pixels,
    SharedString, Subscription, Window,
};
use panes::{Axis, PaneNode};
use persist::{GroupSnapshot, LayoutState, NodeSnapshot, PaneSnapshot, TabSnapshot, WorkspaceSnapshot};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

actions!(
    workbench,
    [
        NewTerminalTab,
        NewClaudeTab,
        NewCodexTab,
        NewWorkspace,
        CloseTab,
        ClosePane,
        SplitRight,
        SplitDown,
        NextTab,
        PreviousTab,
        NextPane,
        PreviousPane,
        NextWorkspace,
        PreviousWorkspace,
        ToggleSidebar,
        ShowWorkspaces,
        ShowSessions,
        OpenFlow,
        OpenUsage,
        OpenSettings,
        OpenExtensions,
        OpenGit,
        OpenPlugins,
        ToggleBrowser,
        ToggleFiles,
        FindInTerminal,
        ZoomIn,
        ZoomOut,
        ZoomReset,
        ToggleMini,
        CheckForUpdates,
        SearchSessions,
        ShowAbout,
        GoToTab1,
        GoToTab2,
        GoToTab3,
        GoToTab4,
        GoToTab5,
        GoToTab6,
        GoToTab7,
        GoToTab8,
        GoToTab9,
        GoToWorkspace1,
        GoToWorkspace2,
        GoToWorkspace3,
        GoToWorkspace4,
        GoToWorkspace5,
        GoToWorkspace6,
        GoToWorkspace7,
        GoToWorkspace8,
        GoToWorkspace9,
        InstallUpdate,
        JumpToUnread,
        OpenPalette,
        ToggleZoom,
    ]
);

pub type Pane = Entity<TerminalView>;

pub fn saved_window_slots() -> Vec<usize> {
    LayoutState::saved_window_slots()
}

pub fn next_free_window_slot() -> usize {
    LayoutState::next_free_slot()
}

pub use account_usage::AccountUsage;
pub use persist::ClosedWindows;

pub struct Tab {
    pub root: PaneNode<Pane>,
    pub active: Pane,
}

pub struct Workspace {
    pub id: u64,
    pub name: Option<String>,
    pub group: Option<u64>,
    pub cwd: PathBuf,
    pub tabs: Vec<Tab>,
    pub active_tab: usize,
    /// Saved layout not spawned yet; restored the first time the workspace is opened.
    pub dormant: Option<WorkspaceSnapshot>,
}

pub struct Group {
    pub id: u64,
    pub name: String,
    pub collapsed: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SidePanel {
    Workspaces,
    Sessions,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Git,
    Flow,
    Usage,
    Processes,
    /// Monitoring → the capture proxy: what tabs talk to.
    Proxy,
    Settings,
    Extensions,
    Plugins,
    /// "Build my idea": describe an idea, an agent builds and previews it.
    Idea,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LaunchTarget {
    NewTab,
    NewWorkspace,
    /// A split of the current tab, right of / below the active pane. Goes through the same launch
    /// path as a tab, so an agent started there gets its own working tree when the project is taken.
    SplitRight,
    SplitDown,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SessionFilter {
    All,
    Only(Agent),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RenameTarget {
    Workspace(u64),
    Group(u64),
}

pub struct Rename {
    pub target: RenameTarget,
    pub input: Entity<TextInput>,
    _subscription: Subscription,
}

/// How long a status bar message stays.
const STATUS_DURATION: std::time::Duration = std::time::Duration::from_secs(6);

pub struct Workbench {
    /// Which Agentty window this is (0 = main); picks its layout file.
    pub slot: usize,
    /// Closed on purpose (not quitting): don't save it for the next launch.
    closed: bool,
    focus_handle: FocusHandle,
    pub workspaces: Vec<Workspace>,
    pub active_workspace: usize,
    pub groups: Vec<Group>,
    /// The "ungrouped" section folds away like any group does.
    pub ungrouped_collapsed: bool,
    panel: SidePanel,
    sidebar_open: bool,
    page: Option<Page>,
    launcher_open: bool,
    /// Where the + menu opens what is picked: a tab (default each time it opens) or a split.
    launcher_target: LaunchTarget,
    workspace_menu: Option<u64>,
    picker: Option<picker::Picker>,
    palette: Option<palette::Palette>,
    rename: Option<Rename>,
    sessions: Vec<SessionInfo>,
    sessions_loading: bool,
    session_filter: SessionFilter,
    status: Option<SharedString>,
    /// Bumped per status message, so an older message's timer never clears a newer one.
    status_generation: u64,
    pane_subscriptions: HashMap<EntityId, Subscription>,
    sidebar_resizing: bool,
    browser_resizing: bool,
    /// Files panel docked at the right edge (folder structure, changes, working trees).
    files_panel: Option<files_panel::FilesPanel>,
    files_resizing: bool,
    /// Dragging the handle under the files panel's working-tree list: (pointer y, height) at the start.
    files_trees_drag: Option<(f32, f32)>,
    /// Window width at the last render, for sizing the panels docked at the right.
    viewport_width: f32,
    browser_home_input: Option<(Entity<TextInput>, Subscription)>,
    split_drag: Option<layout::SplitDrag>,
    split_bounds: Rc<RefCell<HashMap<Vec<usize>, Bounds<Pixels>>>>,
    /// Last laid-out bounds of each pane (responsive headers, file drops).
    pane_bounds: Rc<RefCell<HashMap<EntityId, Bounds<Pixels>>>>,
    /// Split pane shown enlarged (focus view).
    zoomed: Option<Pane>,
    flow: flow::FlowState,
    usage: Option<Entity<UsageView>>,
    extensions: Option<Entity<crate::extensions_view::ExtensionsView>>,
    git: Option<Entity<crate::git_view::GitView>>,
    idea: Option<Entity<crate::idea_view::IdeaView>>,
    branch_menu: Option<layout::BranchMenu>,
    branch_menu_closed: Option<(gpui::EntityId, std::time::Instant)>,
    /// Installed agent CLIs and local models (`None` until detected).
    pub installed: Option<crate::agents::Installed>,
    installed_at: Option<std::time::Instant>,
    mini: Option<mini::MiniState>,
    mini_opening: bool,
    pub updates: update::Updates,
    tab_menu: Option<tab_menu::TabMenu>,
    about_open: bool,
    close_confirm: Option<confirm::CloseConfirm>,
    /// Menu closed by a click outside it, and when: the same click on its toggle must not reopen it.
    dismissed_menu: Option<(&'static str, std::time::Instant)>,
    /// Keyboard focus for the sidebar (⌘N there creates a workspace instead of a tab).
    sidebar_focus: FocusHandle,
    /// The new-workspace start page is open (with an optional name).
    new_workspace: Option<(Entity<TextInput>, Subscription)>,
    /// Group the workspace being created on the start page goes into.
    new_workspace_group: Option<u64>,
    /// Launcher opened with a right-click at this position.
    launcher_at: Option<gpui::Point<Pixels>>,
    /// Settings revision already acknowledged, and the toast shown for a newer one.
    settings_seen: u64,
    toast: Option<(SharedString, u64)>,
    /// Title last given to the native window (the Dock and Window menus list windows by it).
    native_title: String,
    /// Spend and limit resets per agent, for the menu bar.
    account_usage: Vec<account_usage::AccountUsage>,
    /// Subagents / session links popover of a pane.
    agent_panel: Option<agent_panel::AgentPanel>,
    session_viewer: Option<session_viewer::SessionViewer>,
    launcher_more: bool,
    service_status: HashMap<&'static str, agentty_bridge::service_status::ServiceStatus>,
    status_dismissed: std::collections::HashSet<String>,
    resume_dismissed: std::collections::HashSet<(u64, PathBuf)>,
    resume_menu: Option<u64>,
    harness_cache: harness::HarnessCache,
    harness_dialog: Option<harness::HarnessDialog>,
    status_menu: Option<status_menus::StatusMenu>,
    processes: processes::ProcessMonitor,
    inventory: status_menus::AgentInventory,
    browser: Option<browser::BrowserPanel>,
    browser_request: Option<String>,
    find_bar: Option<find::FindBar>,
    /// Panes already told that they share a working tree with another agent (once each).
    shared_tree_warned: std::collections::HashSet<u64>,
    /// Local servers started in panes: ports, auto-open, stopping them with their pane.
    servers: servers::ServerWatch,
    installed_fonts: Option<Vec<String>>,
    /// Settings: the "more fonts" list of every installed family is expanded.
    font_list_open: bool,
    sessions_scroll: gpui::UniformListScrollHandle,
    sidebar_scroll: gpui::ScrollHandle,
    settings_scroll: gpui::ScrollHandle,
    settings_section: settings_page::SettingsSection,
    session_search: Entity<TextInput>,
    /// Sessions whose transcript mentions the current query (filled in the background).
    session_content_hits: Option<(String, std::collections::HashSet<PathBuf>)>,
    session_search_generation: u64,
    _session_search_subscription: Subscription,
    notices: Vec<notices::Notice>,
    notices_open: bool,
    window_active: bool,
    alias_form: Option<settings_page::AliasForm>,
    accounts_form: Option<accounts_page::AccountsForm>,
    /// Settings → System check results (Windows / Linux), and whether a check is running.
    system_check: Option<Vec<crate::setup_check::Tool>>,
    system_checking: bool,
    harness_pattern_form: Option<settings_page::HarnessPatternForm>,
    /// Plugin whose panel is docked right of the terminals.
    plugin_panel: Option<String>,
    plugin_inputs: HashMap<(String, String), plugin_panel::PluginInput>,
    plugin_scroll: gpui::ScrollHandle,
    welcome_scroll: gpui::ScrollHandle,
    /// Context last sent to plugins (serialized), to send only changes.
    plugin_context_key: String,
    prompt_dialog: Option<prompt_dialog::PromptDialog>,
    /// A CLI the user picked that isn't installed: what to tell them, and where to read more.
    install_hint: Option<(&'static str, &'static str, &'static str)>,
    /// The start page is shown even though workspaces exist (opened from the sidebar).
    welcome: bool,
    /// "Pick a pane to connect": the pane the link starts from.
    connect_pick: Option<u64>,
    plugins_page: plugins_page::PluginsPage,
    /// First-run onboarding (dialog steps, then the follow-along tour card).
    onboarding: Option<onboarding::Onboarding>,
    /// Monitoring → Proxy: the capture table and its filter.
    proxy: proxy_page::ProxyPage,
    next_id: u64,
}

impl Focusable for Workbench {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Workbench {
    pub fn new(slot: usize, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let session_search = cx.new(|cx| TextInput::localized("", "sessions.search", window, cx));
        let session_search_subscription = cx.subscribe(&session_search, |this, _, event: &crate::text_input::TextInputEvent, cx| {
            if matches!(event, crate::text_input::TextInputEvent::Changed) {
                this.search_session_contents(cx);
            }
        });
        let proxy_filter = cx.new(|cx| TextInput::localized("", "proxy.filter", window, cx));
        let proxy_subscription = cx.subscribe(&proxy_filter, |_, _, event: &crate::text_input::TextInputEvent, cx| {
            if matches!(event, crate::text_input::TextInputEvent::Changed) {
                cx.notify();
            }
        });
        let mut this = Self {
            slot,
            closed: false,
            focus_handle: cx.focus_handle(),
            workspaces: Vec::new(),
            active_workspace: 0,
            groups: Vec::new(),
            ungrouped_collapsed: false,
            panel: SidePanel::Workspaces,
            sidebar_open: true,
            page: None,
            launcher_open: false,
            launcher_target: LaunchTarget::NewTab,
            workspace_menu: None,
            picker: None,
            palette: None,
            rename: None,
            sessions: Vec::new(),
            sessions_loading: false,
            session_filter: SessionFilter::All,
            status: None,
            status_generation: 0,
            pane_subscriptions: HashMap::new(),
            sidebar_resizing: false,
            browser_resizing: false,
            files_panel: None,
            files_resizing: false,
            files_trees_drag: None,
            viewport_width: 1400.,
            browser_home_input: None,
            split_drag: None,
            split_bounds: Rc::default(),
            pane_bounds: Rc::default(),
            zoomed: None,
            flow: flow::FlowState::default(),
            usage: None,
            extensions: None,
            git: None,
            idea: None,
            branch_menu: None,
            branch_menu_closed: None,
            installed: None,
            installed_at: None,
            mini: None,
            mini_opening: false,
            updates: update::Updates::default(),
            tab_menu: None,
            about_open: false,
            close_confirm: None,
            dismissed_menu: None,
            sidebar_focus: cx.focus_handle(),
            new_workspace: None,
            new_workspace_group: None,
            launcher_at: None,
            settings_seen: 0,
            toast: None,
            native_title: String::new(),
            account_usage: Vec::new(),
            agent_panel: None,
            session_viewer: None,
            launcher_more: false,
            service_status: HashMap::new(),
            status_dismissed: Default::default(),
            resume_dismissed: Default::default(),
            resume_menu: None,
            harness_cache: Default::default(),
            harness_dialog: None,
            status_menu: None,
            processes: Default::default(),
            inventory: Default::default(),
            browser: None,
            browser_request: None,
            find_bar: None,
            shared_tree_warned: Default::default(),
            servers: Default::default(),
            installed_fonts: None,
            font_list_open: false,
            sessions_scroll: gpui::UniformListScrollHandle::new(),
            sidebar_scroll: gpui::ScrollHandle::new(),
            settings_scroll: gpui::ScrollHandle::new(),
            settings_section: settings_page::SettingsSection::General,
            session_search,
            session_content_hits: None,
            session_search_generation: 0,
            _session_search_subscription: session_search_subscription,
            notices: Vec::new(),
            notices_open: false,
            window_active: true,
            alias_form: None,
            accounts_form: None,
            system_check: None,
            system_checking: false,
            harness_pattern_form: None,
            plugin_panel: None,
            plugin_inputs: HashMap::new(),
            plugin_scroll: gpui::ScrollHandle::new(),
            welcome_scroll: gpui::ScrollHandle::new(),
            plugin_context_key: String::new(),
            prompt_dialog: None,
            welcome: false,
            install_hint: None,
            connect_pick: None,
            plugins_page: Default::default(),
            onboarding: None,
            proxy: proxy_page::ProxyPage::new(proxy_filter, proxy_subscription),
            next_id: 1,
        };
        this.restore(window, cx);
        this.first_run_system_check(cx);
        this.first_run_onboarding(cx);
        this.refresh_sessions(cx);
        this.detect_agents(cx);
        this.start_update_checks(cx);
        this.start_service_status_checks(cx);
        this.start_account_usage(cx);
        this.start_server_watch(cx);
        // New sessions (for the resume bar and the sessions list) show up without a manual refresh.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(std::time::Duration::from_secs(120)).await;
            if this.update(cx, |this, cx| this.refresh_sessions(cx)).is_err() {
                break;
            }
        })
        .detach();
        // With the menu bar item on, closing the main window hides it; Agentty keeps running there.
        // Additional windows close, but stay in the recent list (Dock and History menus) to reopen.
        let entity = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            if slot > 0 {
                let _ = entity.update(cx, |this, cx| {
                    this.persist(cx);
                    let names: Vec<String> = this.workspaces.iter().map(|ws| this.workspace_title(ws, cx)).collect();
                    let title = match names.as_slice() {
                        [] => String::new(),
                        [one] => one.clone(),
                        [first, rest @ ..] => format!("{first} +{}", rest.len()),
                    };
                    ClosedWindows::remember(slot, title, !names.is_empty());
                    this.closed = true;
                });
                crate::set_app_menus(cx);
                return true;
            }
            if !crate::settings::settings(cx).menu_bar || !crate::platform::HAS_STATUS_ITEM {
                return true;
            }
            if let Some(ns) = crate::native::ns_window(window) {
                crate::native::order_out(ns);
            }
            false
        });
        cx.on_app_quit(|this, cx| {
            this.persist(cx);
            async {}
        })
        .detach();
        this
    }

    /// Re-detects installed agents at most once a minute (new installs show up without a restart).
    pub(super) fn detect_agents(&mut self, cx: &mut Context<Self>) {
        if self.installed_at.is_some_and(|at| at.elapsed().as_secs() < 60) {
            return;
        }
        self.installed_at = Some(std::time::Instant::now());
        let task = cx.background_spawn(async { crate::agents::detect() });
        cx.spawn(async move |this, cx| {
            let installed = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.installed.as_ref() != Some(&installed) {
                    this.installed = Some(installed);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Whether `binary` is installed; optimistic until detection finishes.
    pub(super) fn is_installed(&self, binary: &str) -> bool {
        self.installed.as_ref().is_none_or(|i| i.has(binary))
    }

    fn next_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    // -- panes ---------------------------------------------------------------------------------

    fn spawn_pane(&mut self, spec: LaunchSpec, cx: &mut Context<Self>) -> Pane {
        let tool = match (&spec.kind, &spec.start) {
            (PaneKind::Shell, crate::launch::Start::Command(line)) => {
                crate::agents::OTHER_AGENTS.iter().find(|a| line.split_whitespace().next() == Some(a.binary)).map_or("command", |a| a.id)
            }
            (kind, _) => crate::brand::kind_id(*kind),
        };
        crate::metrics::track(cx, "pane_opened", serde_json::json!({ "tool": tool }));
        let pane = cx.new(|cx| TerminalView::new(spec, cx));
        let subscription = cx.subscribe(&pane, |this, pane, event: &TerminalEvent, cx| match event {
            TerminalEvent::Exited => this.remove_pane(&pane, cx),
            TerminalEvent::Activated => {
                let pane_id = pane.read(cx).pane_id;
                if this.mark_pane_read(pane_id) {
                    cx.notify();
                }
                this.mark_active(&pane, cx);
            }
            TerminalEvent::Notified { kind, message } => {
                this.record_notice(&pane, *kind, message.clone(), cx);
                if *kind == crate::terminal::NoticeKind::Finished {
                    let pane_id = pane.read(cx).pane_id;
                    this.flow_agent_finished(pane_id, cx);
                }
            }
            TerminalEvent::TitleChanged | TerminalEvent::StatusChanged => {
                // A shell that changed folder may have entered a project with an agent harness.
                this.watch_harness(&pane, cx);
                this.warn_about_shared_tree(&pane, cx);
                cx.notify();
            }
            TerminalEvent::OpenLink(url) => this.open_link(url.clone(), cx),
            TerminalEvent::RevealPath(path) => crate::platform::reveal(path),
        });
        self.pane_subscriptions.insert(pane.entity_id(), subscription);
        pane
    }

    /// (workspace index, tab index) holding the pane.
    fn locate(&self, pane: &Pane) -> Option<(usize, usize)> {
        self.workspaces.iter().enumerate().find_map(|(w, ws)| ws.tabs.iter().position(|tab| tab.root.contains(pane)).map(|t| (w, t)))
    }

    pub fn all_panes(&self) -> Vec<Pane> {
        self.workspaces.iter().flat_map(|ws| ws.tabs.iter().flat_map(|tab| tab.root.leaves())).collect()
    }

    pub fn active_pane(&self) -> Option<Pane> {
        let ws = self.workspaces.get(self.active_workspace)?;
        ws.tabs.get(ws.active_tab).map(|tab| tab.active.clone())
    }

    fn mark_active(&mut self, pane: &Pane, cx: &mut Context<Self>) {
        if let Some((w, t)) = self.locate(pane) {
            let ws = &mut self.workspaces[w];
            if ws.tabs[t].active != *pane || ws.active_tab != t {
                ws.tabs[t].active = pane.clone();
                ws.active_tab = t;
                cx.notify();
            }
        }
    }

    fn focus_pane(&self, pane: &Pane, window: &mut Window, cx: &mut Context<Self>) {
        let handle = pane.read(cx).focus_handle(cx);
        window.focus(&handle);
    }

    pub(super) fn focus_active(&self, window: &mut Window, cx: &mut Context<Self>) {
        match self.active_pane() {
            Some(pane) => self.focus_pane(&pane, window, cx),
            None => window.focus(&self.focus_handle),
        }
    }

    fn remove_pane(&mut self, pane: &Pane, cx: &mut Context<Self>) {
        self.stop_servers_of(pane, cx);
        self.onboarding_event(onboarding::TourEvent::PaneClosed, cx);
        if self.connect_pick == Some(pane.read(cx).pane_id) {
            self.connect_pick = None;
        }
        self.pane_subscriptions.remove(&pane.entity_id());
        self.pane_bounds.borrow_mut().remove(&pane.entity_id());
        if self.zoomed.as_ref() == Some(pane) {
            self.zoomed = None;
        }
        if self.agent_panel.as_ref().is_some_and(|p| p.pane == pane.entity_id()) {
            self.agent_panel = None;
        }
        let removed_id = pane.read(cx).pane_id;
        self.flow_forget_pane(removed_id, cx);
        self.notices.retain(|n| n.pane_id != removed_id);
        let Some((w, t)) = self.locate(pane) else { return };
        let ws = &mut self.workspaces[w];
        let tab = ws.tabs.remove(t);
        match tab.root.remove(pane) {
            Some(root) => {
                let active = if tab.active == *pane { root.leaves()[0].clone() } else { tab.active };
                ws.tabs.insert(t, Tab { root, active });
            }
            None => {
                if ws.active_tab >= ws.tabs.len() {
                    ws.active_tab = ws.tabs.len().saturating_sub(1);
                } else if t < ws.active_tab {
                    ws.active_tab -= 1;
                }
            }
        }
        if ws.tabs.is_empty() && ws.dormant.is_none() {
            self.workspaces.remove(w);
            if self.active_workspace >= self.workspaces.len() {
                self.active_workspace = self.workspaces.len().saturating_sub(1);
            } else if w < self.active_workspace {
                self.active_workspace -= 1;
            }
        }
        self.persist(cx);
        cx.notify();
    }

    /// Files dropped at `position` (window coordinates): typed as paths into the pane under it.
    pub fn drop_files_at(&mut self, position: (f32, f32), paths: &[PathBuf], window: &mut Window, cx: &mut Context<Self>) {
        let point = gpui::point(px(position.0), px(position.1));
        let visible: Vec<Pane> =
            self.workspaces.get(self.active_workspace).and_then(|w| w.tabs.get(w.active_tab)).map(|t| t.root.leaves()).unwrap_or_default();
        let bounds = self.pane_bounds.borrow().clone();
        let target =
            visible.iter().find(|p| bounds.get(&p.entity_id()).is_some_and(|b| b.contains(&point))).cloned().or_else(|| self.active_pane());
        if self.page == Some(Page::Idea) {
            if let Some(idea) = self.idea.clone() {
                idea.update(cx, |view, cx| view.add_files(paths, cx));
            }
            return;
        }
        let Some(pane) = target.filter(|_| self.page.is_none()) else { return };
        pane.update(cx, |view, cx| {
            view.drop_paths(paths);
            cx.notify();
        });
        self.mark_active(&pane, cx);
        self.focus_pane(&pane, window, cx);
    }

    pub fn has_pane(&self, pane_id: u64, cx: &gpui::App) -> bool {
        self.all_panes().iter().any(|p| p.read(cx).pane_id == pane_id)
    }

    pub fn apply_signal(&mut self, signal: AgentSignal, cx: &mut Context<Self>) {
        if let Some(pane) = self.all_panes().into_iter().find(|p| p.read(cx).pane_id == signal.pane_id) {
            pane.update(cx, |view, cx| view.apply_signal(signal, cx));
        }
    }

    // -- workspaces and tabs -------------------------------------------------------------------

    /// Shows the start page again (from the sidebar), with the workspaces left as they are.
    pub(super) fn open_welcome(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.welcome = true;
        self.page = None;
        self.session_viewer = None;
        self.launcher_open = false;
        self.detect_agents(cx);
        window.focus(&self.focus_handle);
        cx.notify();
    }

    pub(super) fn close_welcome(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.welcome = false;
        self.focus_active(window, cx);
        cx.notify();
    }

    pub fn create_workspace(&mut self, spec: LaunchSpec, window: &mut Window, cx: &mut Context<Self>) {
        self.welcome = false;
        let cwd = spec.cwd.clone();
        let pane = self.spawn_pane(spec, cx);
        let id = self.next_id();
        // A name typed on the new-workspace page.
        let group = if self.new_workspace.is_some() {
            self.new_workspace_group.take().filter(|g| self.groups.iter().any(|x| x.id == *g))
        } else {
            None
        };
        let name = self.new_workspace.take().map(|(input, _)| input.read(cx).text().trim().to_string()).filter(|n| !n.is_empty());
        self.workspaces.push(Workspace {
            id,
            name,
            group,
            cwd,
            tabs: vec![Tab { root: PaneNode::Leaf(pane.clone()), active: pane }],
            active_tab: 0,
            dormant: None,
        });
        self.activate_workspace(self.workspaces.len() - 1, window, cx);
        self.persist(cx);
    }

    /// Opens `folder` (picked from the Dock's recent list or Finder): switches to a workspace
    /// already there, otherwise starts a terminal workspace in it.
    pub fn open_folder(&mut self, folder: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.mini.is_some() {
            self.exit_mini(None, window, cx);
        }
        // Test runs (`AGENTTY_BACKGROUND=1`) don't take focus from the app in use.
        if std::env::var("AGENTTY_BACKGROUND").as_deref() != Ok("1") {
            window.activate_window();
            cx.activate(true);
        }
        match self.workspaces.iter().position(|ws| ws.cwd == folder) {
            Some(index) => self.activate_workspace(index, window, cx),
            None => self.create_workspace(LaunchSpec::new(PaneKind::Shell, folder), window, cx),
        }
    }

    pub fn open_tab(&mut self, spec: LaunchSpec, window: &mut Window, cx: &mut Context<Self>) {
        self.welcome = false;
        if self.workspaces.is_empty() {
            return self.create_workspace(spec, window, cx);
        }
        let pane = self.spawn_pane(spec, cx);
        let ws = &mut self.workspaces[self.active_workspace];
        ws.tabs.push(Tab { root: PaneNode::Leaf(pane.clone()), active: pane });
        ws.active_tab = ws.tabs.len() - 1;
        self.page = None;
        self.session_viewer = None;
        self.launcher_open = false;
        self.focus_active(window, cx);
        self.persist(cx);
        cx.notify();
    }

    pub fn activate_workspace(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.workspaces.len() {
            return;
        }
        self.welcome = false;
        self.active_workspace = index;
        crate::native::note_recent_folder(&self.workspaces[index].cwd);
        self.page = None;
        self.session_viewer = None;
        self.new_workspace = None;
        self.launcher_open = false;
        self.workspace_menu = None;
        if let Some(snapshot) = self.workspaces[index].dormant.take() {
            self.revive(index, snapshot, cx);
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    /// Makes a workspace current while a page (AgentGit) stays on screen.
    pub(super) fn select_workspace_for_page(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.workspaces.len() {
            return;
        }
        self.active_workspace = index;
        if let Some(snapshot) = self.workspaces[index].dormant.take() {
            self.revive(index, snapshot, cx);
        }
        cx.notify();
    }

    fn activate_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.welcome = false;
        let Some(ws) = self.workspaces.get_mut(self.active_workspace) else { return };
        if index < ws.tabs.len() {
            ws.active_tab = index;
            self.page = None;
            self.session_viewer = None;
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    fn close_workspace(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.workspaces.iter().position(|w| w.id == id) else { return };
        let panes: Vec<Pane> = self.workspaces[index].tabs.iter().flat_map(|t| t.root.leaves()).collect();
        self.workspaces[index].dormant = None;
        if panes.is_empty() {
            self.workspaces.remove(index);
            self.active_workspace = self.active_workspace.min(self.workspaces.len().saturating_sub(1));
            self.persist(cx);
        }
        for pane in panes {
            self.remove_pane(&pane, cx);
        }
        self.workspace_menu = None;
        self.focus_active(window, cx);
        cx.notify();
    }

    fn split(&mut self, axis: Axis, window: &mut Window, cx: &mut Context<Self>) {
        let Some(active) = self.active_pane() else { return };
        self.onboarding_event(onboarding::TourEvent::Split, cx);
        // New panes open where the user last was, following `cd`.
        let cwd = active.read(cx).current_dir();
        self.split_with(LaunchSpec::new(PaneKind::Shell, cwd), axis, window, cx);
    }

    fn split_with(&mut self, spec: LaunchSpec, axis: Axis, window: &mut Window, cx: &mut Context<Self>) {
        let Some(active) = self.active_pane() else { return self.open_tab(spec, window, cx) };
        let pane = self.spawn_pane(spec, cx);
        let ws = &mut self.workspaces[self.active_workspace];
        let tab = &mut ws.tabs[ws.active_tab];
        tab.root.split(&active, pane.clone(), axis);
        tab.active = pane.clone();
        self.page = None;
        self.focus_pane(&pane, window, cx);
        self.persist(cx);
        cx.notify();
    }

    fn cycle_pane(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ws) = self.workspaces.get_mut(self.active_workspace) else { return };
        let Some(tab) = ws.tabs.get_mut(ws.active_tab) else { return };
        let leaves = tab.root.leaves();
        let Some(index) = leaves.iter().position(|p| *p == tab.active) else { return };
        let next = if forward { (index + 1) % leaves.len() } else { (index + leaves.len() - 1) % leaves.len() };
        tab.active = leaves[next].clone();
        self.focus_active(window, cx);
        cx.notify();
    }

    // -- launching -----------------------------------------------------------------------------

    /// Directory new tabs default to: where the user currently is, or home.
    fn default_cwd(&self, cx: &gpui::App) -> PathBuf {
        self.active_pane().map(|p| p.read(cx).current_dir()).unwrap_or_else(home_dir)
    }

    pub fn request_launch(
        &mut self,
        choice: impl Into<crate::launch::LaunchChoice>,
        target: LaunchTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let choice = choice.into();
        self.launcher_open = false;
        // New tabs open where the user is (unless set to ask); new workspaces ask by default.
        let ask = match target {
            LaunchTarget::NewTab => settings(cx).ask_directory_for_tabs || self.workspaces.is_empty() && settings(cx).ask_directory,
            LaunchTarget::NewWorkspace => settings(cx).ask_directory,
            // A split belongs to the tab it is made in: it starts where that tab is.
            LaunchTarget::SplitRight | LaunchTarget::SplitDown => false,
        };
        if ask {
            self.open_picker(choice, target, window, cx);
        } else {
            let cwd = self.default_cwd(cx);
            self.launch(choice, target, cwd, window, cx);
        }
    }

    fn launch(
        &mut self,
        choice: crate::launch::LaunchChoice,
        target: LaunchTarget,
        cwd: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.launch_in_own_tree(&choice, target, &cwd, window, cx) {
            return;
        }
        self.launch_in(choice, target, cwd, None, window, cx);
    }

    /// Starts `choice` in `cwd`. `project` is the folder to remember instead, when `cwd` is a working
    /// tree made for this session (recent folders list projects, not their session trees).
    fn launch_in(
        &mut self,
        choice: crate::launch::LaunchChoice,
        target: LaunchTarget,
        cwd: PathBuf,
        project: Option<&std::path::Path>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let remembered = project.map(std::path::Path::to_path_buf).unwrap_or_else(|| cwd.clone());
        update_settings(cx, |s| s.remember_dir(remembered));
        let spec = choice.spec(cwd);
        match target {
            LaunchTarget::NewTab => self.open_tab(spec, window, cx),
            LaunchTarget::NewWorkspace => self.create_workspace(spec, window, cx),
            LaunchTarget::SplitRight => self.split_with(spec, Axis::Horizontal, window, cx),
            LaunchTarget::SplitDown => self.split_with(spec, Axis::Vertical, window, cx),
        }
    }

    // -- sessions ------------------------------------------------------------------------------

    pub(super) fn session_query(&self, cx: &gpui::App) -> String {
        self.session_search.read(cx).text().trim().to_lowercase()
    }

    /// Full-text search over transcripts, debounced; title and path matches show immediately.
    fn search_session_contents(&mut self, cx: &mut Context<Self>) {
        self.session_search_generation += 1;
        let generation = self.session_search_generation;
        let query = self.session_query(cx);
        cx.notify();
        if query.chars().count() < 2 {
            self.session_content_hits = None;
            return;
        }
        let paths: Vec<PathBuf> = self.sessions.iter().map(|s| s.path.clone()).collect();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(250)).await;
            if this.read_with(cx, |this, _| this.session_search_generation != generation).unwrap_or(true) {
                return;
            }
            let needle = query.clone();
            let hits = cx
                .background_spawn(async move { paths.into_iter().filter(|p| agentty_bridge::transcript_contains(p, &needle)).collect() })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.session_search_generation == generation {
                    this.session_content_hits = Some((query, hits));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn focus_session_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.page = None;
        self.panel = SidePanel::Sessions;
        self.sidebar_open = true;
        if self.sessions.is_empty() {
            self.refresh_sessions(cx);
        }
        window.focus(&self.session_search.focus_handle(cx));
        cx.notify();
    }

    fn refresh_sessions(&mut self, cx: &mut Context<Self>) {
        if self.sessions_loading {
            return;
        }
        self.sessions_loading = true;
        let task = cx.background_spawn(async { agentty_bridge::list(None, 300) });
        cx.spawn(async move |this, cx| {
            let sessions = task.await;
            let _ = this.update(cx, |this, cx| {
                crate::set_recent_sessions(&sessions, cx);
                this.sessions = sessions;
                this.sessions_loading = false;
                if let Some(picker) = this.picker.as_mut() {
                    picker.add_session_dirs(&this.sessions);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn resume_session(&mut self, session: &SessionInfo, window: &mut Window, cx: &mut Context<Self>) {
        let cwd = session.cwd.as_ref().map(PathBuf::from).filter(|p| p.is_dir()).unwrap_or_else(home_dir);
        let spec = LaunchSpec::resume(session.agent, session.id.clone(), session.title.clone(), cwd);
        self.create_workspace(spec, window, cx);
        if let Some(ws) = self.workspaces.last_mut() {
            ws.name = Some(session.title.clone());
        }
    }

    /// Converts the session transcript into a handoff document and continues it in the other agent.
    fn migrate_session(&mut self, session: SessionInfo, window: &mut Window, cx: &mut Context<Self>) {
        let from = session.agent;
        let to = other_agent(from);
        self.set_status(tf(cx, "migrate.running", &[("name", &session.title), ("agent", to.display_name())]), cx);
        let task = cx.background_spawn({
            let id = session.id.clone();
            async move {
                let (cwd, turns) = agentty_bridge::load(from, &id)?;
                anyhow::ensure!(!turns.is_empty(), "session has no conversation to hand off");
                agentty_bridge::handoff::create(from, &id, to, cwd, &turns)
            }
        });
        let window_handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = cx.update_window(window_handle, |_, window, cx| {
                let _ = this.update(cx, |this, cx| match result {
                    Ok(handoff) => {
                        let cwd = handoff.cwd.as_ref().map(PathBuf::from).filter(|p| p.is_dir()).unwrap_or_else(home_dir);
                        let title = format!("{} ← {}", session.title, from.display_name());
                        this.create_workspace(LaunchSpec::with_prompt(to, handoff.prompt.clone(), title.clone(), cwd), window, cx);
                        if let Some(ws) = this.workspaces.last_mut() {
                            ws.name = Some(title);
                        }
                        let n = handoff.turn_count.to_string();
                        let path = handoff.path.display().to_string();
                        this.set_status(tf(cx, "migrate.done", &[("n", &n), ("name", &path)]), cx);
                    }
                    Err(err) => {
                        let message = format!("{err:#}");
                        this.set_status(tf(cx, "migrate.failed", &[("name", &message)]), cx);
                    }
                });
            });
        })
        .detach();
    }

    /// A message in the status bar. It reports something that just happened, so it goes away
    /// after a few seconds instead of staying until the next one (a closed link kept saying
    /// "linked with …").
    fn set_status(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.status = Some(message.into());
        self.status_generation += 1;
        let generation = self.status_generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(STATUS_DURATION).await;
            let _ = this.update(cx, |this, cx| {
                if this.status_generation == generation && this.status.take().is_some() {
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    // -- groups and renaming -------------------------------------------------------------------

    fn create_group(&mut self, window: &mut Window, cx: &mut Context<Self>) -> u64 {
        let id = self.next_id();
        let name = tf(cx, "group.default", &[("n", &(self.groups.len() + 1).to_string())]);
        self.groups.push(Group { id, name, collapsed: false });
        self.start_rename(RenameTarget::Group(id), window, cx);
        self.persist(cx);
        id
    }

    fn move_to_group(&mut self, workspace: u64, group: Option<u64>, cx: &mut Context<Self>) {
        if let Some(ws) = self.workspaces.iter_mut().find(|w| w.id == workspace) {
            ws.group = group;
        }
        self.workspace_menu = None;
        self.persist(cx);
        cx.notify();
    }

    /// Moves workspace `dragged` just above `target`, joining `target`'s group.
    fn move_workspace(&mut self, dragged: u64, target: u64, cx: &mut Context<Self>) {
        if dragged == target {
            return;
        }
        let active_id = self.workspaces.get(self.active_workspace).map(|w| w.id);
        let Some(from) = self.workspaces.iter().position(|w| w.id == dragged) else { return };
        let mut ws = self.workspaces.remove(from);
        let Some(to) = self.workspaces.iter().position(|w| w.id == target) else {
            self.workspaces.insert(from, ws);
            return;
        };
        ws.group = self.workspaces[to].group;
        self.workspaces.insert(to, ws);
        self.restore_active(active_id);
        self.persist(cx);
        cx.notify();
    }

    /// Puts a workspace first (within its group), e.g. when one of its agents just finished.
    pub(super) fn move_workspace_to_top(&mut self, id: u64, cx: &mut Context<Self>) {
        let active_id = self.workspaces.get(self.active_workspace).map(|w| w.id);
        let Some(from) = self.workspaces.iter().position(|w| w.id == id) else { return };
        if from == 0 {
            return;
        }
        let ws = self.workspaces.remove(from);
        self.workspaces.insert(0, ws);
        self.restore_active(active_id);
        self.persist(cx);
        cx.notify();
    }

    fn restore_active(&mut self, active_id: Option<u64>) {
        if let Some(index) = active_id.and_then(|id| self.workspaces.iter().position(|w| w.id == id)) {
            self.active_workspace = index;
        }
    }

    /// Moves group `dragged` to the position of `target`.
    /// Moves tab `from` of workspace `workspace` to position `to`, keeping the selected tab selected.
    pub(super) fn reorder_tab(&mut self, workspace: u64, from: usize, to: usize, cx: &mut Context<Self>) {
        let Some(ws) = self.workspaces.iter_mut().find(|w| w.id == workspace) else { return };
        if from == to || from >= ws.tabs.len() || to >= ws.tabs.len() {
            return;
        }
        let active = ws.tabs[ws.active_tab].active.clone();
        let tab = ws.tabs.remove(from);
        ws.tabs.insert(to, tab);
        ws.active_tab = ws.tabs.iter().position(|t| t.active == active).unwrap_or(0);
        self.persist(cx);
        cx.notify();
    }

    fn move_group(&mut self, dragged: u64, target: u64, cx: &mut Context<Self>) {
        let (Some(from), Some(to)) = (self.groups.iter().position(|g| g.id == dragged), self.groups.iter().position(|g| g.id == target))
        else {
            return;
        };
        if from == to {
            return;
        }
        let group = self.groups.remove(from);
        self.groups.insert(to, group);
        self.persist(cx);
        cx.notify();
    }

    fn delete_group(&mut self, id: u64, cx: &mut Context<Self>) {
        self.groups.retain(|g| g.id != id);
        for ws in self.workspaces.iter_mut().filter(|w| w.group == Some(id)) {
            ws.group = None;
        }
        self.persist(cx);
        cx.notify();
    }

    pub fn workspace_title(&self, ws: &Workspace, cx: &gpui::App) -> String {
        if let Some(name) = &ws.name {
            return name.clone();
        }
        if let Some(tab) = ws.tabs.get(ws.active_tab) {
            return tab.active.read(cx).display_title();
        }
        ws.dormant
            .as_ref()
            .and_then(|s| s.tabs.first())
            .and_then(|t| first_pane(&t.layout))
            .map(|p| p.title.clone())
            .unwrap_or_else(|| crate::ui::tilde(&ws.cwd))
    }

    fn start_rename(&mut self, target: RenameTarget, window: &mut Window, cx: &mut Context<Self>) {
        let current = match target {
            RenameTarget::Workspace(id) => self.workspaces.iter().find(|w| w.id == id).map(|w| self.workspace_title(w, cx)),
            RenameTarget::Group(id) => self.groups.iter().find(|g| g.id == id).map(|g| g.name.clone()),
        }
        .unwrap_or_default();
        let input = cx.new(|cx| TextInput::new(current, "", window, cx));
        let subscription = cx.subscribe_in(&input, window, |this, input, event: &TextInputEvent, window, cx| match event {
            TextInputEvent::Confirmed | TextInputEvent::Blurred => {
                let value = input.read(cx).text().trim().to_string();
                this.finish_rename(Some(value), window, cx);
            }
            TextInputEvent::Cancelled => this.finish_rename(None, window, cx),
            _ => {}
        });
        window.focus(&input.focus_handle(cx));
        self.workspace_menu = None;
        self.rename = Some(Rename { target, input, _subscription: subscription });
        cx.notify();
    }

    fn finish_rename(&mut self, value: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rename) = self.rename.take() else { return };
        if let Some(value) = value {
            match rename.target {
                RenameTarget::Workspace(id) => {
                    if let Some(ws) = self.workspaces.iter_mut().find(|w| w.id == id) {
                        ws.name = (!value.is_empty()).then_some(value);
                    }
                }
                RenameTarget::Group(id) => {
                    if let Some(group) = self.groups.iter_mut().find(|g| g.id == id) {
                        if !value.is_empty() {
                            group.name = value;
                        }
                    }
                }
            }
            self.persist(cx);
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    // -- persistence ---------------------------------------------------------------------------

    fn snapshot_pane(pane: &Pane, cx: &gpui::App) -> PaneSnapshot {
        let view = pane.read(cx);
        let session_id = match view.spec.kind {
            PaneKind::Codex => {
                view.spec.session_id.clone().or_else(|| agentty_bridge::codex::find_recent(&view.spec.cwd, view.launched_at_ms))
            }
            _ => view.spec.session_id.clone(),
        };
        PaneSnapshot { kind: view.spec.kind, cwd: view.current_dir(), title: view.display_title(), session_id }
    }

    fn persist(&self, cx: &gpui::App) {
        let workspaces = self
            .workspaces
            .iter()
            .map(|ws| {
                if let Some(dormant) = &ws.dormant {
                    return WorkspaceSnapshot { name: ws.name.clone(), group: ws.group, ..dormant.clone() };
                }
                WorkspaceSnapshot {
                    id: ws.id,
                    name: ws.name.clone(),
                    group: ws.group,
                    cwd: ws.cwd.clone(),
                    active_tab: ws.active_tab,
                    tabs: ws
                        .tabs
                        .iter()
                        .map(|tab| TabSnapshot {
                            layout: NodeSnapshot::from_tree(&tab.root.map(&mut |pane| Self::snapshot_pane(pane, cx))),
                            active_pane: tab.root.leaves().iter().position(|p| *p == tab.active).unwrap_or(0),
                        })
                        .collect(),
                }
            })
            .collect();
        let state = LayoutState {
            groups: self.groups.iter().map(|g| GroupSnapshot { id: g.id, name: g.name.clone(), collapsed: g.collapsed }).collect(),
            workspaces,
            active_workspace: self.active_workspace,
            ungrouped_collapsed: self.ungrouped_collapsed,
        };
        if self.closed {
            return;
        }
        if let Err(err) = state.save(self.slot) {
            eprintln!("agentty: could not save workspaces: {err:#}");
        }
    }

    fn restore(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = LayoutState::load(self.slot);
        self.groups = state.groups.iter().map(|g| Group { id: g.id, name: g.name.clone(), collapsed: g.collapsed }).collect();
        self.ungrouped_collapsed = state.ungrouped_collapsed;
        for snapshot in state.workspaces {
            self.next_id = self.next_id.max(snapshot.id + 1);
            self.workspaces.push(Workspace {
                id: snapshot.id,
                name: snapshot.name.clone(),
                group: snapshot.group.filter(|g| self.groups.iter().any(|x| x.id == *g)),
                cwd: snapshot.cwd.clone(),
                tabs: Vec::new(),
                active_tab: 0,
                dormant: Some(snapshot),
            });
        }
        for group in &self.groups {
            self.next_id = self.next_id.max(group.id + 1);
        }
        if !self.workspaces.is_empty() {
            // Only the active workspace starts processes; the rest wake up when opened.
            self.activate_workspace(state.active_workspace.min(self.workspaces.len() - 1), window, cx);
        }
    }

    fn revive(&mut self, index: usize, snapshot: WorkspaceSnapshot, cx: &mut Context<Self>) {
        let mut tabs = Vec::new();
        for tab in &snapshot.tabs {
            let Some(tree) = tab.layout.to_tree() else { continue };
            let root = tree.map(&mut |pane: &PaneSnapshot| self.spawn_pane(pane.launch_spec(), cx));
            let leaves = root.leaves();
            let active = leaves.get(tab.active_pane).unwrap_or(&leaves[0]).clone();
            tabs.push(Tab { root, active });
        }
        let ws = &mut self.workspaces[index];
        ws.active_tab = snapshot.active_tab.min(tabs.len().saturating_sub(1));
        ws.tabs = tabs;
        if ws.tabs.is_empty() {
            let pane = self.spawn_pane(LaunchSpec::new(PaneKind::Shell, snapshot.cwd.clone()), cx);
            let ws = &mut self.workspaces[index];
            ws.tabs.push(Tab { root: PaneNode::Leaf(pane.clone()), active: pane });
        }
    }

    // -- mouse drags shared by the whole window --------------------------------------------------

    fn on_root_mouse_move(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.pressed_button != Some(MouseButton::Left) {
            if self.sidebar_resizing
                || self.browser_resizing
                || self.files_resizing
                || self.files_trees_drag.is_some()
                || self.split_drag.is_some()
                || self.flow.is_dragging()
            {
                self.end_drags(cx);
            }
            return;
        }
        if let Some((start_y, start_height)) = self.files_trees_drag {
            // Down makes the working-tree list taller; `trees_height` keeps it within its rows.
            let height = (start_height + f32::from(event.position.y) - start_y).max(40.);
            gpui::BorrowAppContext::update_global::<crate::settings::SettingsStore, _>(cx, |store, _| {
                store.settings.files_panel_trees_height = height
            });
            cx.notify();
        } else if self.files_resizing {
            // The files panel is the last column: its splitter sits just left of it.
            let viewport = f32::from(window.viewport_size().width);
            let width = (viewport - f32::from(event.position.x) - 2.5)
                .clamp(files_panel::MIN_WIDTH, (viewport - 420.0).max(files_panel::MIN_WIDTH));
            gpui::BorrowAppContext::update_global::<crate::settings::SettingsStore, _>(cx, |store, _| {
                store.settings.files_panel_width = width
            });
            cx.notify();
        } else if self.browser_resizing {
            // The splitter sits just left of the panel; the plugin and files panels may sit right of it.
            let shown = self.docked_widths(cx).1;
            let right =
                self.plugin_panel.as_ref().map_or(0., |_| plugin_panel::PANEL_WIDTH) + self.files_panel.as_ref().map_or(0., |_| shown + 5.);
            let viewport = f32::from(window.viewport_size().width) - right;
            let width = (viewport - f32::from(event.position.x) - 2.5).clamp(320.0, (viewport - 420.0).max(320.0));
            gpui::BorrowAppContext::update_global::<crate::settings::SettingsStore, _>(cx, |store, _| store.settings.browser.width = width);
            cx.notify();
        } else if self.sidebar_resizing {
            let width = (f32::from(event.position.x) - chrome::ACTIVITY_BAR_WIDTH).clamp(180.0, 640.0);
            gpui::BorrowAppContext::update_global::<crate::settings::SettingsStore, _>(cx, |store, _| store.settings.sidebar_width = width);
            cx.notify();
        } else if let Some(drag) = &self.split_drag {
            self.drag_split(drag.clone(), event.position, cx);
        } else if self.flow.is_dragging() {
            self.flow.drag_to(event.position);
            cx.notify();
        }
    }

    fn end_drags(&mut self, cx: &mut Context<Self>) {
        if self.sidebar_resizing || self.browser_resizing || self.files_resizing || self.files_trees_drag.is_some() {
            self.sidebar_resizing = false;
            self.browser_resizing = false;
            self.files_resizing = false;
            self.files_trees_drag = None;
            update_settings(cx, |_| {}); // persist the final width
        }
        if self.split_drag.take().is_some() {
            self.persist(cx);
        }
        cx.notify();
    }
}

impl Render for Workbench {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.window_active = window.is_window_active();
        self.viewport_width = f32::from(window.viewport_size().width);
        if let Some(git) = self.git.clone().filter(|_| self.page != Some(Page::Git)) {
            git.update(cx, |v, _| v.set_visible(false));
        }
        self.prepare_browser(window, cx);
        self.prepare_plugin_panel(window, cx);
        self.prepare_files_panel(cx);
        self.advance_tour(cx);
        self.prepare_plugins_page(window, cx);
        self.broadcast_plugin_context(window, cx);
        self.check_settings_toast(cx);
        // Name the window after what it shows, so several Agentty windows can be told apart in the
        // Dock menu and Mission Control.
        let title = match self.workspaces.get(self.active_workspace).map(|ws| self.workspace_title(ws, cx)) {
            Some(name) if !name.is_empty() => format!("{name} — Agentty"),
            _ => "Agentty".to_string(),
        };
        if title != self.native_title {
            window.set_window_title(&title);
            self.native_title = title;
        }
        let main: gpui::AnyElement = match self.page {
            Some(Page::Usage) => {
                let usage = self.usage.get_or_insert_with(|| cx.new(UsageView::new)).clone();
                gpui::AnyView::from(usage).cached(gpui::StyleRefinement::default().size_full()).into_any_element()
            }
            Some(Page::Processes) => self.render_processes(cx).into_any_element(),
            Some(Page::Proxy) => self.render_proxy_page(cx).into_any_element(),
            Some(Page::Settings) => self.render_settings(window, cx).into_any_element(),
            Some(Page::Extensions) => gpui::AnyView::from(self.extensions_view(window, cx))
                .cached(gpui::StyleRefinement::default().size_full())
                .into_any_element(),
            Some(Page::Git) => {
                gpui::AnyView::from(self.git_view(window, cx)).cached(gpui::StyleRefinement::default().size_full()).into_any_element()
            }
            Some(Page::Flow) => self.render_flow(window, cx).into_any_element(),
            Some(Page::Plugins) => self.render_plugins_page(cx).into_any_element(),
            Some(Page::Idea) => {
                gpui::AnyView::from(self.idea_view(window, cx)).cached(gpui::StyleRefinement::default().size_full()).into_any_element()
            }
            None => {
                match (self.render_session_viewer(cx), self.workspaces.get(self.active_workspace).and_then(|ws| ws.tabs.get(ws.active_tab)))
                {
                    (Some(viewer), _) => viewer,
                    (None, Some(tab)) if self.new_workspace.is_none() && !self.welcome => self.render_tab(tab, cx),
                    (None, _) => self.render_welcome(cx).into_any_element(),
                }
            }
        };

        div()
            .id("workbench")
            .key_context("Workbench")
            .track_focus(&self.focus_handle)
            .on_action(
                cx.listener(|this, _: &NewTerminalTab, window, cx| this.request_launch(PaneKind::Shell, LaunchTarget::NewTab, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &NewClaudeTab, window, cx| this.request_launch(PaneKind::Claude, LaunchTarget::NewTab, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &NewCodexTab, window, cx| this.request_launch(PaneKind::Codex, LaunchTarget::NewTab, window, cx)),
            )
            .on_action(cx.listener(|this, _: &NewWorkspace, window, cx| this.open_new_workspace_page(window, cx)))
            .on_action(cx.listener(|this, _: &CloseTab, window, cx| {
                if this.page.take().is_some() {
                    this.focus_active(window, cx);
                    return cx.notify();
                }
                if let Some(ws) = this.workspaces.get(this.active_workspace) {
                    let index = ws.active_tab;
                    this.request_close_tabs(&[index], window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ClosePane, window, cx| {
                if this.page.take().is_some() {
                    this.focus_active(window, cx);
                    return cx.notify();
                }
                if let Some(pane) = this.active_pane() {
                    this.request_close_pane(&pane, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &SplitRight, window, cx| this.split(Axis::Horizontal, window, cx)))
            .on_action(cx.listener(|this, _: &SplitDown, window, cx| this.split(Axis::Vertical, window, cx)))
            .on_action(cx.listener(|this, _: &NextPane, window, cx| this.cycle_pane(true, window, cx)))
            .on_action(cx.listener(|this, _: &PreviousPane, window, cx| this.cycle_pane(false, window, cx)))
            .on_action(cx.listener(|this, _: &NextTab, window, cx| {
                if let Some(ws) = this.workspaces.get(this.active_workspace) {
                    let n = ws.tabs.len().max(1);
                    this.activate_tab((ws.active_tab + 1) % n, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &PreviousTab, window, cx| {
                if let Some(ws) = this.workspaces.get(this.active_workspace) {
                    let n = ws.tabs.len().max(1);
                    this.activate_tab((ws.active_tab + n - 1) % n, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &NextWorkspace, window, cx| {
                let n = this.workspaces.len().max(1);
                this.activate_workspace((this.active_workspace + 1) % n, window, cx);
            }))
            .on_action(cx.listener(|this, _: &PreviousWorkspace, window, cx| {
                let n = this.workspaces.len().max(1);
                this.activate_workspace((this.active_workspace + n - 1) % n, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| {
                this.sidebar_open = !this.sidebar_open;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ShowWorkspaces, _, cx| this.show_panel(SidePanel::Workspaces, cx)))
            .on_action(cx.listener(|this, _: &ShowSessions, _, cx| this.show_panel(SidePanel::Sessions, cx)))
            .on_action(cx.listener(|this, _: &OpenFlow, _, cx| this.open_page(Page::Flow, cx)))
            .on_action(cx.listener(|this, _: &OpenUsage, _, cx| this.open_page(Page::Usage, cx)))
            .on_action(cx.listener(|this, _: &OpenSettings, _, cx| this.open_page(Page::Settings, cx)))
            .on_action(cx.listener(|this, _: &OpenExtensions, _, cx| this.open_page(Page::Extensions, cx)))
            .on_action(cx.listener(|this, _: &OpenGit, _, cx| this.open_page(Page::Git, cx)))
            .on_action(cx.listener(|this, _: &OpenPlugins, _, cx| this.open_page(Page::Plugins, cx)))
            .on_action(cx.listener(|this, _: &ToggleBrowser, window, cx| this.toggle_browser(window, cx)))
            .on_action(cx.listener(|this, _: &ToggleFiles, _, cx| this.toggle_files_panel(cx)))
            .on_action(cx.listener(|this, _: &FindInTerminal, window, cx| this.open_find(window, cx)))
            .on_action(cx.listener(|_, _: &ZoomIn, _, cx| update_settings(cx, |s| s.font_size = (s.font_size + 1.).min(32.))))
            .on_action(cx.listener(|_, _: &ZoomOut, _, cx| update_settings(cx, |s| s.font_size = (s.font_size - 1.).max(8.))))
            .on_action(
                cx.listener(|_, _: &ZoomReset, _, cx| {
                    update_settings(cx, |s| s.font_size = crate::settings::Settings::default().font_size)
                }),
            )
            .capture_any_mouse_down(|_, window, _| crate::webview::focus_gpui_view(window))
            .on_action(cx.listener(|this, _: &ToggleMini, window, cx| this.toggle_mini(window, cx)))
            .on_action(cx.listener(|this, _: &CheckForUpdates, _, cx| this.check_for_updates(true, cx)))
            .on_action(cx.listener(|this, _: &SearchSessions, window, cx| this.focus_session_search(window, cx)))
            .on_action(cx.listener(|this, _: &ShowAbout, _, cx| {
                this.about_open = true;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &GoToTab1, window, cx| this.activate_tab(0, window, cx)))
            .on_action(cx.listener(|this, _: &GoToTab2, window, cx| this.activate_tab(1, window, cx)))
            .on_action(cx.listener(|this, _: &GoToTab3, window, cx| this.activate_tab(2, window, cx)))
            .on_action(cx.listener(|this, _: &GoToTab4, window, cx| this.activate_tab(3, window, cx)))
            .on_action(cx.listener(|this, _: &GoToTab5, window, cx| this.activate_tab(4, window, cx)))
            .on_action(cx.listener(|this, _: &GoToTab6, window, cx| this.activate_tab(5, window, cx)))
            .on_action(cx.listener(|this, _: &GoToTab7, window, cx| this.activate_tab(6, window, cx)))
            .on_action(cx.listener(|this, _: &GoToTab8, window, cx| this.activate_tab(7, window, cx)))
            .on_action(cx.listener(|this, _: &GoToTab9, window, cx| {
                let last = this.workspaces.get(this.active_workspace).map_or(0, |w| w.tabs.len().saturating_sub(1));
                this.activate_tab(last, window, cx)
            }))
            .on_action(cx.listener(|this, _: &GoToWorkspace1, window, cx| this.activate_workspace(0, window, cx)))
            .on_action(cx.listener(|this, _: &GoToWorkspace2, window, cx| this.activate_workspace(1, window, cx)))
            .on_action(cx.listener(|this, _: &GoToWorkspace3, window, cx| this.activate_workspace(2, window, cx)))
            .on_action(cx.listener(|this, _: &GoToWorkspace4, window, cx| this.activate_workspace(3, window, cx)))
            .on_action(cx.listener(|this, _: &GoToWorkspace5, window, cx| this.activate_workspace(4, window, cx)))
            .on_action(cx.listener(|this, _: &GoToWorkspace6, window, cx| this.activate_workspace(5, window, cx)))
            .on_action(cx.listener(|this, _: &GoToWorkspace7, window, cx| this.activate_workspace(6, window, cx)))
            .on_action(cx.listener(|this, _: &GoToWorkspace8, window, cx| this.activate_workspace(7, window, cx)))
            .on_action(cx.listener(|this, _: &GoToWorkspace9, window, cx| {
                let last = this.workspaces.len().saturating_sub(1);
                this.activate_workspace(last, window, cx)
            }))
            .on_action(cx.listener(|this, _: &InstallUpdate, _, cx| this.install_update(cx)))
            .on_action(cx.listener(|this, _: &JumpToUnread, window, cx| this.jump_to_unread(window, cx)))
            .on_action(cx.listener(|this, action: &crate::OpenRecentSession, window, cx| {
                let session = crate::RECENT_SESSIONS.lock().ok().and_then(|recent| recent.get(action.index).cloned());
                if let Some(session) = session {
                    this.resume_session(&session, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleZoom, window, cx| {
                if let Some(pane) = this.active_pane() {
                    this.toggle_zoom(&pane, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &OpenPalette, window, cx| {
                if this.palette.is_some() {
                    this.close_palette(window, cx);
                } else {
                    this.open_palette(window, cx);
                }
            }))
            .on_mouse_move(cx.listener(Self::on_root_mouse_move))
            // Windows / Linux: files dropped from the file manager (macOS uses a native hook, `file_drop`).
            .when(!cfg!(target_os = "macos"), |root| {
                root.on_drop(cx.listener(|this, paths: &gpui::ExternalPaths, window, cx| {
                    let position = window.mouse_position();
                    let paths: Vec<PathBuf> = paths.paths().iter().map(|p| crate::platform::drops::terminal_safe(p)).collect();
                    this.drop_files_at((f32::from(position.x), f32::from(position.y)), &paths, window, cx);
                }))
            })
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    drop_split::end_tab_drag();
                    if this.flow.is_dragging() {
                        this.finish_flow_drag(cx);
                    }
                    if this.sidebar_resizing
                        || this.browser_resizing
                        || this.files_resizing
                        || this.files_trees_drag.is_some()
                        || this.split_drag.is_some()
                    {
                        this.end_drags(cx);
                    }
                }),
            )
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(hex(Chrome::EDITOR))
            .text_color(hex(Chrome::FOREGROUND))
            .font_family(".SystemUIFont")
            .child(self.render_title_bar(window, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(self.render_activity_bar(cx))
                    // Pages take the full width, except those that work with the workspace list.
                    .when(self.sidebar_open && self.page.is_none_or(Self::page_keeps_sidebar), |d| {
                        d.child(self.render_side_bar(window, cx))
                    })
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(self.render_tab_strip(cx))
                            .children(self.render_service_banner(cx))
                            .child(div().flex_1().min_h_0().flex().child(div().flex_1().min_w_0().h_full().child(main)).when(
                                self.page.is_none(),
                                |d| {
                                    d.children(self.render_browser_splitter(cx))
                                        .children(self.render_browser(cx))
                                        .children(self.render_plugin_panel(cx))
                                        .children(self.render_files_splitter(cx))
                                        .children(self.render_files_panel(cx))
                                },
                            ))
                            .when(self.launcher_open, |d| d.child(self.render_launcher(cx)))
                            .when(self.notices_open, |d| {
                                d.child(
                                    div()
                                        .absolute()
                                        .top(px(39.))
                                        .right(px(8.))
                                        .child(gpui::deferred(self.render_notices(cx)).with_priority(3)),
                                )
                            }),
                    ),
            )
            .child(self.render_status_bar(cx))
            .when_some(self.picker.as_ref().map(|_| ()), |d, _| d.child(self.render_picker(cx)))
            .when_some(self.palette.as_ref().map(|_| ()), |d, _| d.child(self.render_palette(cx)))
            .children(self.render_tab_menu(cx))
            .when(self.updates.popup, |d| d.child(self.render_update_popup(window, cx)))
            .when(self.about_open, |d| d.child(self.render_about_dialog(cx)))
            .children((!self.updates.popup).then(|| self.render_update_badge(cx)).flatten())
            .children(self.render_connect_pick_bar(cx))
            .children(self.render_install_hint(cx))
            .children(self.render_close_confirm(cx))
            .children(self.render_prompt_dialog(cx))
            .children(self.render_harness_dialog(cx))
            .children(self.render_onboarding(cx))
            .children(self.render_toast(cx))
    }
}

impl Workbench {
    /// The start page, for a new workspace: pick what to run, optionally name it first.
    pub(super) fn open_new_workspace_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_new_workspace_page_in(None, window, cx);
    }

    /// The start page for a new workspace that goes into `group`.
    pub(super) fn open_new_workspace_page_in(&mut self, group: Option<u64>, window: &mut Window, cx: &mut Context<Self>) {
        self.new_workspace_group = group;
        self.page = None;
        self.session_viewer = None;
        self.launcher_open = false;
        let input = cx.new(|cx| TextInput::localized("", "welcome.name_placeholder", window, cx));
        let subscription = cx.subscribe_in(&input, window, |this, _, event: &TextInputEvent, window, cx| match event {
            TextInputEvent::Confirmed => this.request_launch(PaneKind::Shell, LaunchTarget::NewWorkspace, window, cx),
            TextInputEvent::Cancelled => {
                this.new_workspace = None;
                this.focus_active(window, cx);
                cx.notify();
            }
            _ => {}
        });
        window.focus(&input.focus_handle(cx));
        self.new_workspace = Some((input, subscription));
        self.detect_agents(cx);
        cx.notify();
    }

    /// A settings change made on the settings page shows "Settings updated" for a moment.
    fn check_settings_toast(&mut self, cx: &mut Context<Self>) {
        let revision = cx.global::<crate::settings::SettingsStore>().revision;
        if revision == self.settings_seen {
            return;
        }
        self.settings_seen = revision;
        if self.page == Some(Page::Settings) {
            self.show_toast(t(cx, "settings.updated"), cx);
        }
    }

    pub(super) fn show_toast(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.show_toast_for(text, 1800, cx);
    }

    /// A toast that stays for `millis` (something worth reading, not just a confirmation).
    pub(super) fn show_toast_for(&mut self, text: impl Into<SharedString>, millis: u64, cx: &mut Context<Self>) {
        let id = self.toast.as_ref().map_or(1, |(_, id)| id + 1);
        self.toast = Some((text.into(), id));
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(millis)).await;
            let _ = this.update(cx, |this, cx| {
                if this.toast.as_ref().is_some_and(|(_, current)| *current == id) {
                    this.toast = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn render_toast(&self, cx: &gpui::App) -> Option<impl IntoElement> {
        let (text, id) = self.toast.clone()?;
        // Left of the panels docked at the right: the browser is a native view, and anything drawn
        // under it (a toast about a server that just stopped, say) would never be seen.
        let docked = if self.page.is_none() {
            let (browser, files) = self.docked_widths(cx);
            self.browser.as_ref().map_or(0., |_| browser + 5.)
                + self.plugin_panel.as_ref().map_or(0., |_| plugin_panel::PANEL_WIDTH)
                + self.files_panel.as_ref().map_or(0., |_| files + 5.)
        } else {
            0.
        };
        Some(
            div().absolute().top(px(chrome::TITLE_BAR_HEIGHT + 44.)).right(px(16. + docked)).child(crate::ui::fade_in(
                SharedString::from(format!("toast-{id}")),
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .rounded_lg()
                    .bg(hex(Chrome::OVERLAY))
                    .border_1()
                    .border_color(hex(Chrome::OVERLAY_BORDER))
                    .shadow_lg()
                    .text_size(px(crate::ui::Type::BODY))
                    .text_color(hex(Chrome::BRIGHT))
                    .child(crate::ui::icon("circle-check", crate::ui::IconSize::INLINE, hex(Chrome::SUCCESS)))
                    .child(text),
            )),
        )
    }

    /// Remembers that a menu was closed by an outside click (see [`Self::just_dismissed`]).
    pub(super) fn note_dismissed(&mut self, menu: &'static str) {
        self.dismissed_menu = Some((menu, std::time::Instant::now()));
    }

    /// Whether `menu` was closed by this same click, so its toggle button shouldn't reopen it.
    pub(super) fn just_dismissed(&mut self, menu: &'static str) -> bool {
        matches!(self.dismissed_menu.take(), Some((m, at)) if m == menu && at.elapsed().as_millis() < 350)
    }

    /// Shows a side panel. The activity icons only open panels; the sidebar's own button closes it.
    fn show_panel(&mut self, panel: SidePanel, cx: &mut Context<Self>) {
        // Leaving a page returns to the terminals with this panel shown.
        self.page = None;
        self.panel = panel;
        self.sidebar_open = true;
        if panel == SidePanel::Sessions && self.sessions.is_empty() {
            self.refresh_sessions(cx);
        }
        cx.notify();
    }

    /// Pages next to which the workspace list stays useful.
    pub(super) fn page_keeps_sidebar(page: Page) -> bool {
        matches!(page, Page::Flow | Page::Git)
    }

    fn open_page(&mut self, page: Page, cx: &mut Context<Self>) {
        let feature = match page {
            Page::Git => "agentgit",
            Page::Flow => "flow",
            Page::Usage => "usage",
            Page::Processes => "processes",
            Page::Proxy => "proxy",
            Page::Settings => "settings",
            Page::Extensions => "extensions",
            Page::Plugins => "plugins",
            Page::Idea => "idea",
        };
        crate::metrics::track(cx, "feature_used", serde_json::json!({ "feature": feature }));
        self.page = if self.page == Some(page) { None } else { Some(page) };
        if self.page == Some(Page::Plugins) {
            // Pick up plugins copied into the folder by hand.
            crate::plugins::reload(cx);
        }
        // AgentGit and Session Flow work with workspaces: always show that list next to them.
        if self.page.is_some_and(Self::page_keeps_sidebar) {
            self.panel = SidePanel::Workspaces;
            self.sidebar_open = true;
        }
        self.launcher_open = false;
        cx.notify();
    }
}

impl Workbench {
    fn git_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<crate::git_view::GitView> {
        // Repositories open in the selected workspace, its active pane first (picked by default).
        let workspace = self.workspaces.get(self.active_workspace);
        let workspace_id = workspace.map(|w| w.id);
        let workspace_panes: Vec<Pane> = workspace.map(|w| w.tabs.iter().flat_map(|t| t.root.leaves()).collect()).unwrap_or_default();
        let mut cwds: Vec<PathBuf> = self.active_pane().map(|p| p.read(cx).display_cwd()).into_iter().collect();
        if let Some(dormant) = workspace.filter(|w| w.tabs.is_empty()) {
            cwds.push(dormant.cwd.clone());
        }
        for pane in workspace_panes {
            let cwd = pane.read(cx).display_cwd();
            if !cwds.contains(&cwd) {
                cwds.push(cwd);
            }
        }
        let view = match &self.git {
            Some(view) => view.clone(),
            None => {
                let view = cx.new(|cx| crate::git_view::GitView::new(window, cx));
                cx.subscribe_in(&view, window, |this, _, event: &crate::git_view::GitEvent, window, cx| match event {
                    crate::git_view::GitEvent::OpenTerminal(path) => {
                        this.page = None;
                        this.launch(PaneKind::Shell.into(), LaunchTarget::NewTab, path.clone(), window, cx);
                    }
                })
                .detach();
                self.git = Some(view.clone());
                view
            }
        };
        view.update(cx, |v, cx| v.set_cwds(workspace_id, cwds, cx));
        view
    }

    fn extensions_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<crate::extensions_view::ExtensionsView> {
        let project = self.active_pane().map(|p| p.read(cx).display_cwd());
        if let Some(view) = &self.extensions {
            let view = view.clone();
            view.update(cx, |v, cx| v.set_project(project, cx));
            return view;
        }
        let view = cx.new(|cx| crate::extensions_view::ExtensionsView::new(project, window, cx));
        cx.subscribe_in(&view, window, |this, _, event: &crate::extensions_view::ExtensionsEvent, window, cx| match event {
            crate::extensions_view::ExtensionsEvent::Use { agent, text, submit } => {
                this.use_in_agent(*agent, text.clone(), *submit, window, cx)
            }
        })
        .detach();
        self.extensions = Some(view.clone());
        view
    }

    /// Sends text to a pane running `agent`: the active pane if it matches, else another pane in
    /// the active workspace, else a new agent tab (typed once the agent has started).
    fn use_in_agent(&mut self, agent: Agent, text: String, submit: bool, window: &mut Window, cx: &mut Context<Self>) {
        let kind = PaneKind::from(agent);
        let matches = |pane: &Pane, cx: &gpui::App| pane.read(cx).agent_kind() == Some(kind) && pane.read(cx).is_running();
        let in_workspace = self
            .workspaces
            .get(self.active_workspace)
            .map(|ws| ws.tabs.iter().flat_map(|t| t.root.leaves()).collect::<Vec<_>>())
            .unwrap_or_default();
        let target = self.active_pane().filter(|p| matches(p, cx)).or_else(|| in_workspace.into_iter().find(|p| matches(p, cx)));
        let deliver = move |pane: &Pane, cx: &mut gpui::App| {
            pane.update(cx, |view, cx| if submit { view.submit_prompt(text.clone(), cx) } else { view.insert_text(&text) });
        };
        self.page = None;
        match target {
            Some(pane) => {
                deliver(&pane, cx);
                self.mark_active(&pane, cx);
                self.focus_pane(&pane, window, cx);
            }
            None => {
                let cwd = self.default_cwd(cx);
                self.open_tab(LaunchSpec::new(kind, cwd), window, cx);
                if let Some(pane) = self.active_pane() {
                    cx.spawn(async move |_, cx| {
                        // Give the agent's TUI time to start before typing.
                        cx.background_executor().timer(std::time::Duration::from_secs(4)).await;
                        let _ = cx.update(|cx| deliver(&pane, cx));
                    })
                    .detach();
                }
            }
        }
        cx.notify();
    }
}

pub fn other_agent(agent: Agent) -> Agent {
    match agent {
        Agent::Claude => Agent::Codex,
        // Sessions of other CLIs are handed to Claude Code.
        _ => Agent::Claude,
    }
}

fn first_pane(node: &NodeSnapshot) -> Option<&PaneSnapshot> {
    match node {
        NodeSnapshot::Pane(p) => Some(p),
        NodeSnapshot::Split { children, .. } => children.iter().find_map(first_pane),
    }
}

pub fn status_label(view: &TerminalView, cx: &gpui::App) -> (String, u32) {
    use crate::terminal::AgentStatus;
    if !view.is_running() {
        return (t(cx, "status.exited").into(), Chrome::MUTED);
    }
    if !view.is_agent() {
        return (t(cx, "status.shell").into(), Chrome::MUTED);
    }
    match &view.status {
        AgentStatus::Idle => (t(cx, "status.idle").into(), Chrome::MUTED),
        AgentStatus::Working => (t(cx, "status.working").into(), Chrome::ORANGE),
        AgentStatus::Thinking => (t(cx, "status.thinking").into(), Chrome::BLUE),
        AgentStatus::Finished(_) => (t(cx, "status.finished").into(), Chrome::SUCCESS),
        AgentStatus::Permission(detail) => (
            match detail {
                Some(detail) => format!("{} · {detail}", t(cx, "status.permission")),
                None => t(cx, "status.permission").into(),
            },
            Chrome::ATTENTION,
        ),
        AgentStatus::Question(_) => (t(cx, "status.question").into(), Chrome::ATTENTION),
        AgentStatus::Interrupted => (t(cx, "status.interrupted").into(), Chrome::WARNING),
    }
}

impl Workbench {
    /// Remote UI control for development (`AGENTTY_DEBUG=1`), used to drive and snapshot the app.
    pub fn debug_command(&mut self, command: &str, argument: &str, window: &mut Window, cx: &mut Context<Self>) {
        let kind = |name: &str| match name {
            "claude" => PaneKind::Claude,
            "codex" => PaneKind::Codex,
            _ => PaneKind::Shell,
        };
        match command {
            "snapshot" => {
                let path = PathBuf::from(argument);
                // Hidden windows don't repaint; raise it without taking keyboard focus.
                if let Some(ns) = crate::native::ns_window(window) {
                    crate::native::order_front_regardless(ns);
                }
                window.refresh();
                cx.spawn(async move |_, cx| {
                    cx.background_executor().timer(std::time::Duration::from_millis(900)).await;
                    let _ = cx.update(|_| {
                        if let Err(err) = crate::debug::capture_own_window(&path) {
                            eprintln!("agentty: snapshot failed: {err:#}");
                        }
                    });
                })
                .detach();
            }
            // A window covered by others does not paint, and a pane only starts on its first paint:
            // come to the front for a moment (no keyboard focus), then step back behind everything.
            "paint" => {
                if let Some(ns) = crate::native::ns_window(window) {
                    crate::native::order_front_regardless(ns);
                    window.refresh();
                    cx.spawn(async move |_, cx| {
                        cx.background_executor().timer(std::time::Duration::from_millis(500)).await;
                        crate::native::order_back(ns);
                    })
                    .detach();
                }
            }
            "onboarding" => self.open_onboarding(cx),
            "tour" => {
                let state = self.debug_tour(argument, window, cx);
                eprintln!("tour: {state}");
            }
            // One JSON line on stderr: what the new panels and watchers think is going on.
            "probe" => {
                let panes: Vec<serde_json::Value> = self
                    .all_panes()
                    .iter()
                    .map(|p| {
                        let v = p.read(cx);
                        serde_json::json!({ "id": v.pane_id, "tool": v.tool_id(), "cwd": v.display_cwd(), "worktree": v.worktree, "branch": v.git_branch })
                    })
                    .collect();
                let listeners: Vec<(u64, Vec<u16>)> =
                    self.servers.listeners.iter().map(|(pane, l)| (*pane, l.iter().map(|l| l.port).collect())).collect();
                let records: Vec<serde_json::Value> = crate::capture::records()
                    .iter()
                    .map(|r| serde_json::json!({ "pane": r.pane, "method": r.method, "endpoint": r.endpoint(), "path": r.path, "status": r.status, "sent": r.sent, "received": r.received, "open": r.duration_ms.is_none(), "error": r.error }))
                    .collect();
                eprintln!(
                    "probe: {}",
                    serde_json::json!({
                        "panes": panes,
                        "listeners": listeners,
                        "browser": self.browser.as_ref().and_then(|b| b.current_url()),
                        "files": self.files_panel.as_ref().map(|p| p.debug_state()),
                        "capture": { "recording": crate::capture::is_recording(), "port": crate::capture::port(), "records": records },
                        "toast": self.toast.as_ref().map(|(text, _)| text.to_string()),
                    })
                );
            }
            "capture" => match argument {
                "stop" => crate::capture::stop(),
                _ => {
                    let _ = crate::capture::start();
                }
            },
            "files" => match argument {
                "" => self.toggle_files_panel(cx),
                path => self.open_files_panel(Some(PathBuf::from(path)), cx),
            },
            "page" => {
                let page = match argument {
                    "flow" => Some(Page::Flow),
                    "usage" => Some(Page::Usage),
                    "processes" => Some(Page::Processes),
                    "proxy" => Some(Page::Proxy),
                    "settings" => Some(Page::Settings),
                    "extensions" => Some(Page::Extensions),
                    "plugins" => Some(Page::Plugins),
                    "idea" => Some(Page::Idea),
                    "git" => Some(Page::Git),
                    _ => None,
                };
                match page {
                    Some(page) if self.page != Some(page) => self.open_page(page, cx),
                    Some(_) => {}
                    None => self.page = None,
                }
            }
            "panel" => self.show_panel(if argument == "sessions" { SidePanel::Sessions } else { SidePanel::Workspaces }, cx),
            "git" => {
                self.page = Some(Page::Git);
                let view = self.git_view(window, cx);
                let dump = view.update(cx, |v, cx| v.debug(argument, window, cx));
                eprintln!("git: {dump}");
                cx.notify();
            }
            "tab" => {
                let cwd = self.default_cwd(cx);
                self.open_tab(LaunchSpec::new(kind(argument), cwd), window, cx);
            }
            "branch-menu" => {
                if let Some(pane) = self.active_pane() {
                    self.toggle_branch_menu(&pane, window, cx);
                }
            }
            "branch-new" => {
                if let Some(repo) = self.branch_menu.as_ref().map(|m| m.repo.clone()) {
                    self.create_branch(repo, argument.to_string(), cx);
                }
            }
            "branch-switch" => {
                if let Some(repo) = self.branch_menu.as_ref().map(|m| m.repo.clone()) {
                    let remote = argument.contains('/');
                    self.switch_branch(repo, argument.to_string(), remote, cx);
                }
            }
            "mini" => self.toggle_mini(window, cx),
            "live" => {
                let ids: Vec<u64> = argument.split_whitespace().filter_map(|s| s.parse().ok()).collect();
                if let [from, to, ..] = ids[..] {
                    self.debug_live(from, to, argument.ends_with("both"), cx);
                }
                let edges: Vec<String> =
                    self.flow.edges().iter().map(|e| format!("{}->{} live={} sent={}", e.from, e.to, e.live, e.sent)).collect();
                eprintln!("flow: {edges:?}");
            }
            "update" => match argument {
                path if path.ends_with(".dmg") => self.install_local_dmg(PathBuf::from(path), cx),
                "check-failed" => {
                    self.updates.state = update::UpdateState::CheckFailed(
                        "update check failed: https://api.github.com/repos/empty-user77/agentty-releases/releases/latest: status code 403"
                            .into(),
                    );
                    self.updates.popup = true;
                    cx.notify();
                }
                "fake" => {
                    self.updates.state = update::UpdateState::Available(agentty_bridge::update::Release {
                        version: "0.2.0".into(),
                        notes: "- Faster startup\n- Git page".into(),
                        page_url: "https://github.com/empty-user77/agentty-releases/releases".into(),
                        dmg_url: None,
                        dmg_name: None,
                        checksums_url: None,
                    });
                    self.updates.popup = true;
                    cx.notify();
                }
                _ => self.check_for_updates(true, cx),
            },
            "session-search" => {
                self.focus_session_search(window, cx);
                self.session_search.update(cx, |i, cx| i.set_text(argument.to_string(), cx));
            }
            "sessions" => {
                let rows: Vec<String> = self.visible_sessions(cx).iter().take(8).map(|&i| self.sessions[i].title.clone()).collect();
                eprintln!(
                    "sessions: {} {rows:?} hits={:?}",
                    self.visible_sessions(cx).len(),
                    self.session_content_hits.as_ref().map(|h| h.1.len())
                );
            }
            "about" => {
                self.about_open = true;
                cx.notify();
            }
            "service-status" => {
                // `service-status claude|Elevated errors` injects a degraded status for UI testing.
                let (provider, detail) = argument.split_once('|').unwrap_or((argument, ""));
                if let Some(p) = agentty_bridge::service_status::provider(provider) {
                    self.service_status.insert(
                        p.id,
                        agentty_bridge::service_status::ServiceStatus {
                            provider: p.id,
                            degraded: true,
                            detail: detail.into(),
                            severity: "degraded_performance".into(),
                        },
                    );
                    cx.notify();
                }
            }
            "agents" => eprintln!("agents: {:?}", self.installed),
            "launcher" => {
                self.launcher_open = true;
                cx.notify();
            }
            "tab-move" => {
                let target =
                    self.workspaces.iter().find(|w| Some(w.id.to_string().as_str()) == argument.split_whitespace().nth(1)).map(|w| w.id);
                self.move_tab(argument.split_whitespace().next().and_then(|i| i.parse().ok()).unwrap_or(0), target, window, cx);
                let layout: Vec<String> = self.workspaces.iter().map(|w| format!("{}:{}", w.id, w.tabs.len())).collect();
                eprintln!("workspaces: {layout:?} active={}", self.active_workspace);
            }
            "tab-order" => {
                // `tab-order <from> <to>` in the active workspace.
                let mut parts = argument.split_whitespace().filter_map(|v| v.parse::<usize>().ok());
                if let (Some(from), Some(to), Some(ws)) =
                    (parts.next(), parts.next(), self.workspaces.get(self.active_workspace).map(|w| w.id))
                {
                    self.reorder_tab(ws, from, to, cx);
                }
            }
            "tab-menu" => self.open_tab_menu(argument.parse().unwrap_or(0), gpui::point(gpui::px(420.), gpui::px(60.)), cx),
            "resume-here" => {
                if let Some(pane) = self.active_pane() {
                    let dir = pane.read(cx).display_cwd();
                    if let Some(session) = self.sessions_in(&dir).first().map(|s| (*s).clone()) {
                        self.resume_in_pane(&pane, session, window, cx);
                    }
                }
            }
            "settings-section" => {
                self.page = Some(Page::Settings);
                self.settings_section = match argument {
                    "appearance" => settings_page::SettingsSection::Appearance,
                    "shortcuts" => settings_page::SettingsSection::Shortcuts,
                    "about" => settings_page::SettingsSection::About,
                    "browser" => settings_page::SettingsSection::Browser,
                    "project" => settings_page::SettingsSection::Project,
                    "accounts" => settings_page::SettingsSection::Accounts,
                    "system" => settings_page::SettingsSection::System,
                    _ => settings_page::SettingsSection::General,
                };
                cx.notify();
            }
            "status-menu" => self.toggle_status_menu(
                match argument {
                    "agents" => status_menus::StatusMenu::Agents,
                    "mcp" => status_menus::StatusMenu::Mcp,
                    "context" => status_menus::StatusMenu::Context,
                    _ => status_menus::StatusMenu::Skills,
                },
                cx,
            ),
            "browser" => {
                if argument.is_empty() {
                    self.toggle_browser(window, cx);
                } else {
                    self.open_browser(Some(browser::browser_url(argument, cx)), cx);
                }
            }
            "browser-reload" => self.reload_browser(cx),
            "link" => self.open_link(argument.to_string(), cx),
            // `agentty-link agentty://…`: as if another app opened the link.
            "agentty-link" => self.open_agentty_link(argument, window, cx),
            // `plugin-panel <id>` / `plugin-command <id> <command>`.
            "plugin-panel" => self.toggle_plugin_panel(argument, cx),
            "plugin-command" => {
                if let Some((plugin, command)) = argument.split_once(' ') {
                    self.run_plugin_command(plugin, command, None, cx);
                }
            }
            "plugin-install" => self.install_builtin_plugin(argument.to_string(), window, cx),
            // `prompt-agent claude|codex|shell` picks the agent in the open "Send to…" dialog.
            "prompt-agent" => {
                if let Some(dialog) = self.prompt_dialog.as_mut() {
                    dialog.kind = kind(argument);
                }
            }
            // `harness` opens the harness dialog for the active pane; `harness-start` starts it.
            "harness" => {
                if let Some(pane) = self.active_pane() {
                    let dir = pane.read(cx).display_cwd();
                    match self.harness_for(&dir) {
                        Some(harness) => self.open_harness_dialog(pane, harness, window, cx),
                        None => eprintln!("harness: none detected for {}", dir.display()),
                    }
                }
            }
            "harness-select" => {
                let index = argument.parse::<usize>().ok();
                self.debug_select_harness_entry(index, cx);
            }
            "harness-input" => self.debug_harness_input(argument, cx),
            "prompt-confirm" => {
                if self.prompt_dialog.is_some() {
                    self.confirm_prompt_dialog(window, cx);
                }
            }
            "plugins" => {
                for plugin in &crate::plugins::host(cx).installed {
                    let runtime = crate::plugins::runtime(cx, &plugin.id);
                    let panel = runtime.and_then(|r| r.panel.as_ref()).and_then(|p| serde_json::to_string(p).ok()).unwrap_or_default();
                    eprintln!(
                        "plugins: {} enabled={} state={:?} panel={} logs={:?}",
                        plugin.id,
                        plugin.enabled,
                        runtime.map(|r| r.state.clone()),
                        panel.chars().take(600).collect::<String>(),
                        runtime.map(|r| r.logs.iter().rev().take(6).cloned().collect::<Vec<_>>())
                    );
                }
                let dialog = self.prompt_dialog.as_ref().map(|d| {
                    format!(
                        "kind={:?} cwd={} title={:?} text={:?}",
                        d.kind,
                        d.cwd.display(),
                        d.request.title,
                        d.request.text.chars().take(160).collect::<String>()
                    )
                });
                eprintln!(
                    "plugins: dialog={dialog:?} panel={:?} page={} pending={:?} message={:?} toast={:?} status={:?}",
                    self.plugin_panel,
                    self.page == Some(Page::Plugins),
                    self.plugins_page.pending_link,
                    self.plugins_page.message,
                    self.toast.as_ref().map(|t| t.0.clone()),
                    self.status
                );
            }
            "find" => {
                self.open_find(window, cx);
                if let Some(bar) = self.find_bar.as_ref() {
                    let pane = bar.pane.clone();
                    pane.update(cx, |v, cx| v.set_search(argument, cx));
                    eprintln!("find: {:?}", pane.read(cx).search_position());
                }
            }
            #[cfg(target_os = "macos")]
            "responder" => {
                #[allow(unexpected_cfgs)]
                unsafe {
                    use objc::{msg_send, sel, sel_impl};
                    if let Some(w) = crate::native::ns_window(window) {
                        let r: *mut objc::runtime::Object = msg_send![w, firstResponder];
                        let class = if r.is_null() { "nil".to_string() } else { (*r).class().name().to_string() };
                        eprintln!("responder: {class} gpui_view={}", crate::native::ns_view(window).is_some_and(|v| v == r));
                    }
                }
            }
            // `drag x1 y1 x2 y2`: a paced drag, so the window paints between steps (drop targets
            // only exist while a drag is running).
            "drag" => {
                let points: Vec<f32> = argument.split_whitespace().filter_map(|v| v.parse().ok()).collect();
                let [x1, y1, x2, y2] = points[..] else { return };
                if let Some(ns) = crate::native::ns_window(window) {
                    cx.spawn(async move |_, cx| {
                        crate::debug::synthetic_input(ns, "press", &format!("{x1} {y1}"));
                        for step in 1..=12 {
                            cx.background_executor().timer(std::time::Duration::from_millis(40)).await;
                            let t = step as f32 / 12.0;
                            let (x, y) = (x1 + (x2 - x1) * t, y1 + (y2 - y1) * t);
                            crate::debug::synthetic_input(ns, "drag-to", &format!("{x} {y}"));
                        }
                        cx.background_executor().timer(std::time::Duration::from_millis(80)).await;
                        crate::debug::synthetic_input(ns, "release", &format!("{x2} {y2}"));
                    })
                    .detach();
                }
            }
            // `click x y [right]`, `key cmd-n`, `text 한글abc`: synthetic input, dispatched after this update.
            "click" | "move" | "scroll" | "key" | "text" | "press" | "drag-to" | "release" => {
                let (command, argument) = (command.to_string(), argument.to_string());
                if let Some(ns) = crate::native::ns_window(window) {
                    cx.spawn(async move |_, _| crate::debug::synthetic_input(ns, &command, &argument)).detach();
                }
            }
            "new-window" => cx.defer(crate::new_window),
            "windows" => {
                let slots: Vec<usize> = cx
                    .windows()
                    .into_iter()
                    .filter_map(|w| w.downcast::<Workbench>())
                    .filter_map(|w| w.read(cx).ok().map(|wb| wb.slot))
                    .collect();
                eprintln!("windows: {slots:?}");
            }
            "tray" => {
                let lines: Vec<String> = self.account_usage.iter().map(|u| u.menu_line(cx)).collect();
                eprintln!("tray: {:?} {lines:?}", self.tray_state(cx));
            }
            "tray-popover" => crate::status_item::push_action(crate::status_item::TrayAction::TogglePopover),
            "tray-snapshot" => crate::tray_popover::debug_snapshot(PathBuf::from(argument), cx),
            "hover" => {
                if let Some(pane) = self.active_pane() {
                    eprintln!("hover: {:?}", pane.read(cx).debug_hover());
                }
            }
            "focus-info" => {
                let alias = self
                    .alias_form
                    .as_ref()
                    .map(|f| (f.keyword.focus_handle(cx).is_focused(window), f.expansion.focus_handle(cx).is_focused(window)));
                eprintln!(
                    "focus: alias(keyword, expansion)={alias:?} sidebar={} workbench={}",
                    self.sidebar_focus.is_focused(window),
                    self.focus_handle.is_focused(window)
                );
            }
            "frame" => eprintln!(
                "frame: {:?} screen: {:?}",
                crate::native::ns_window(window).map(crate::native::frame),
                crate::native::ns_window(window).and_then(crate::native::visible_frame)
            ),
            "mouse" => eprintln!(
                "mouse: {:?} viewport={:?} browser_width={}",
                window.mouse_position(),
                window.viewport_size(),
                settings(cx).browser.width
            ),
            "select-workspace" => self.activate_workspace(argument.parse().unwrap_or(0), window, cx),
            "select-tab" => self.activate_tab(argument.parse().unwrap_or(0), window, cx),
            "close-tab" => self.request_close_tabs(&[argument.parse().unwrap_or(0)], window, cx),
            "close-confirm" => {
                if let Some(confirm) = self.close_confirm.take() {
                    eprintln!("layout: confirming removes_workspace={}", confirm.removes_workspace);
                    self.perform_close(confirm.target, window, cx);
                }
            }
            // Split sizes of the active tab, where each split sits on screen, and the zoomed pane.
            "splits" => {
                if let Some(tab) = self.workspaces.get(self.active_workspace).and_then(|ws| ws.tabs.get(ws.active_tab)) {
                    eprintln!("splits: sizes={:?}", tab.root.all_sizes());
                }
                let mut bounds: Vec<_> = self.split_bounds.borrow().iter().map(|(k, b)| (k.clone(), *b)).collect();
                bounds.sort_by(|a, b| a.0.cmp(&b.0));
                for (path, b) in bounds {
                    eprintln!(
                        "splits: path={path:?} origin=({:.0},{:.0}) size=({:.0},{:.0})",
                        f32::from(b.origin.x),
                        f32::from(b.origin.y),
                        f32::from(b.size.width),
                        f32::from(b.size.height)
                    );
                }
                eprintln!("splits: zoomed={:?}", self.zoomed.as_ref().map(|p| p.read(cx).pane_id));
            }
            "zoom" => {
                if let Some(pane) = self.active_pane() {
                    self.toggle_zoom(&pane, window, cx);
                }
            }
            "layout" => {
                for ws in &self.workspaces {
                    let panes: Vec<usize> = ws.tabs.iter().map(|t| t.root.leaves().len()).collect();
                    eprintln!("layout: ws={} tabs={panes:?} active_tab={}", ws.id, ws.active_tab);
                }
                eprintln!("layout: confirm={}", self.close_confirm.is_some());
            }
            // `workspace <kind> [folder]`
            "workspace" => {
                let (name, dir) = argument.split_once(' ').map(|(k, d)| (k, PathBuf::from(d))).unwrap_or((argument, home_dir()));
                self.create_workspace(LaunchSpec::new(kind(name), dir), window, cx)
            }
            // Through the same path as the + menu (a session tree of its own when the project is taken).
            "launch-agent" | "launch-split" | "launch-split-down" | "launch-workspace" => {
                let (name, dir) = argument.split_once(' ').map(|(k, d)| (k, PathBuf::from(d))).unwrap_or((argument, home_dir()));
                let target = match command {
                    "launch-split" => LaunchTarget::SplitRight,
                    "launch-split-down" => LaunchTarget::SplitDown,
                    "launch-workspace" => LaunchTarget::NewWorkspace,
                    _ => LaunchTarget::NewTab,
                };
                self.launch(kind(name).into(), target, dir, window, cx)
            }
            "picker" => self.open_picker(kind(argument).into(), LaunchTarget::NewTab, window, cx),
            "split" => self.split(if argument == "down" { Axis::Vertical } else { Axis::Horizontal }, window, cx),
            "idea" => {
                if self.page != Some(Page::Idea) {
                    self.open_idea_page(window, cx);
                }
                let view = self.idea_view(window, cx);
                view.update(cx, |view, cx| view.debug_action(argument, cx));
            }
            "launch" => self.open_launch(cx),
            "type" => {
                if let Some(pane) = self.active_pane() {
                    let bytes = crate::debug::unescape(argument).into_bytes();
                    pane.update(cx, |view, _| view.write(bytes));
                }
            }
            // Drives the input-method path the way macOS does: `mark:<text>` shows a composition,
            // `commit:<text>` replaces it with committed text. Korean composes character by
            // character, so this is the only way to test it without a keyboard.
            "ime" => {
                use gpui::EntityInputHandler;
                if let Some(pane) = self.active_pane() {
                    let (kind, text) = argument.split_once(':').unwrap_or(("mark", argument));
                    let text = text.to_string();
                    pane.update(cx, |view, cx| match kind {
                        "commit" => view.replace_text_in_range(None, &text, window, cx),
                        "unmark" => view.unmark_text(window, cx),
                        _ => view.replace_and_mark_text_in_range(None, &text, None, window, cx),
                    });
                }
            }
            "group" => {
                let id = self.create_group(window, cx);
                self.finish_rename(Some(argument.to_string()), window, cx);
                if let Some(ws) = self.workspaces.get(self.active_workspace) {
                    let ws_id = ws.id;
                    self.move_to_group(ws_id, Some(id), cx);
                }
            }
            "signal" => {
                // `signal <kind> [json payload]`, exactly as a hook would send it.
                if let Some(pane) = self.active_pane() {
                    let pane_id = pane.read(cx).pane_id;
                    let (kind, payload) = argument.split_once(' ').unwrap_or((argument, ""));
                    if let Some(signal) = crate::agent_signal::parse_line(&format!("{pane_id}\t{kind}\t{payload}")) {
                        pane.update(cx, |view, cx| view.apply_signal(signal, cx));
                    }
                }
            }
            "sidebar-width" => {
                if let Ok(width) = argument.parse::<f32>() {
                    update_settings(cx, |s| s.sidebar_width = width);
                }
            }
            "setting" => {
                if let Some((key, value)) = argument.split_once('=') {
                    let (key, value) = (key.to_string(), value.to_string());
                    update_settings(cx, move |s| {
                        let mut json = serde_json::to_value(&*s).unwrap_or_default();
                        json[key] = serde_json::from_str(&value).unwrap_or(serde_json::Value::String(value));
                        if let Ok(updated) = serde_json::from_value(json) {
                            *s = updated;
                        }
                    });
                }
            }
            "fonts" => {
                let names = cx.text_system().all_font_names();
                eprintln!("fonts: {:?}", names.iter().filter(|n| n.contains("Symbols") || n.contains("JetBrains")).collect::<Vec<_>>());
            }
            "screen-lines" => {
                if let Some(pane) = self.active_pane() {
                    let lines = pane.read(cx).screen_lines(80);
                    let state = crate::terminal::classify_screen(&lines);
                    let shown: Vec<String> = lines
                        .iter()
                        .enumerate()
                        .filter(|(_, l)| !l.trim().is_empty())
                        .map(|(i, l)| format!("{i}:{}", l.chars().take(60).collect::<String>()))
                        .collect();
                    eprintln!("screen-lines: total={} {:?} state={state:?}", lines.len(), &shown[shown.len().saturating_sub(10)..]);
                }
            }
            "dump" => {
                if let Some(pane) = self.active_pane() {
                    eprintln!("screen: {}", pane.read(cx).debug_screen_text());
                }
            }
            "resize" => {
                if let Some((w, h)) = argument.split_once('x').and_then(|(w, h)| Some((w.parse::<f32>().ok()?, h.parse::<f32>().ok()?))) {
                    window.resize(gpui::size(gpui::px(w), gpui::px(h)));
                }
            }
            "notices" => {
                self.notices_open = !self.notices_open;
            }
            "unread" => self.jump_to_unread(window, cx),
            "palette" => self.open_palette(window, cx),
            "attention" => {
                if let Some(pane) = self.active_pane() {
                    let pane_id = pane.read(cx).pane_id;
                    let text = if argument.is_empty() { "Done" } else { argument };
                    let payload = serde_json::json!({ "last_assistant_message": text }).to_string();
                    if let Some(signal) = crate::agent_signal::parse_line(&format!("{pane_id}\tstop\t{payload}")) {
                        pane.update(cx, |view, cx| view.apply_signal(signal, cx));
                    }
                }
            }
            "resume" => {
                if let Some((agent, id)) = argument.split_once(':') {
                    let agent = if agent == "codex" { Agent::Codex } else { Agent::Claude };
                    let spec = LaunchSpec::resume(agent, id.to_string(), "Resumed".into(), self.default_cwd(cx));
                    self.open_tab(spec, window, cx);
                }
            }
            "state" => {
                for pane in self.all_panes() {
                    let v = pane.read(cx);
                    eprintln!(
                        "state: pane={} tool={} mode={:?} usage={:?} stats={:?} kind={:?} live_agent={:?} status={:?} attention={} cwd={} branch={:?} working={:?}",
                        v.pane_id,
                        v.tool_id(),
                        v.mode(),
                        v.live_usage,
                        v.stats,
                        v.spec.kind,
                        v.live_agent,
                        v.status,
                        v.attention,
                        v.display_cwd().display(),
                        v.git_branch,
                        v.working_since.map(|t| t.elapsed().as_secs())
                    );
                }
            }
            "ext" => {
                self.page = Some(Page::Extensions);
                let view = self.extensions_view(window, cx);
                view.update(cx, |v, cx| v.debug_select(argument, cx));
            }
            "use" => {
                // `use claude:/help` inserts without submitting.
                if let Some((agent, text)) = argument.split_once(':') {
                    let agent = if agent == "codex" { Agent::Codex } else { Agent::Claude };
                    self.use_in_agent(agent, text.to_string(), false, window, cx);
                }
            }
            "picker-close" => self.close_picker(window, cx),
            "connect" => {
                // `connect <from index>,<to index>` over the agent panes, in display order.
                let agents: Vec<u64> = self.all_panes().iter().filter(|p| p.read(cx).is_agent()).map(|p| p.read(cx).pane_id).collect();
                if let Some((a, b)) = argument.split_once(',').and_then(|(a, b)| Some((a.parse::<usize>().ok()?, b.parse::<usize>().ok()?)))
                {
                    if let (Some(from), Some(to)) = (agents.get(a), agents.get(b)) {
                        self.debug_connect(*from, *to, cx);
                    }
                }
            }
            "quit" => cx.quit(),
            _ => eprintln!("agentty: unknown debug command {command}"),
        }
        cx.notify();
    }
}
