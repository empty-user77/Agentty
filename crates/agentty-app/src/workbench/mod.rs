//! The window's root view. Hierarchy: group → workspace (one sidebar row) → tab → split panes.

mod account_usage;
mod accounts_page;
mod agent_panel;
mod ask;
mod browser;
mod browser_control;
mod chrome;
mod confirm;
mod context_menu;
mod db_page;
mod disk_page;
mod docker_panel;
mod drop_split;
mod editor_host;
mod file_diff;
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
mod notify_settings;
mod onboarding;
mod palette;
pub mod panes;
mod persist;
mod picker;
pub mod plugin_browser;
mod plugin_host;
mod plugin_panel;
mod plugin_window;
mod plugin_workspace;
mod plugins_page;
mod processes;
mod prompt_dialog;
mod proxy_page;
mod responsive;
pub mod resume_hint;
mod servers;
mod service_status;
mod session_viewer;
mod settings_page;
pub mod side_panels;
mod status_menus;
mod system_page;
mod tab_menu;
mod tasks;
mod tree_manager;
pub mod update;
pub mod worktrees;

use crate::agent_signal::AgentSignal;
use crate::i18n::{t, tf};
use crate::launch::{home_dir, LaunchSpec, PaneKind};
use crate::settings::{settings, update_settings};
use crate::terminal::{TerminalEvent, TerminalView};
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::{hex, Chrome};
use crate::ui::TypeScale as _;
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
use std::path::{Path, PathBuf};
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
        HardReloadBrowser,
        ToggleFiles,
        FindInTerminal,
        ZoomIn,
        ZoomOut,
        ZoomReset,
        ToggleMini,
        CheckForUpdates,
        SearchSessions,
        ShowAbout,
        OpenPrivacyPolicy,
        OpenTerms,
        OpenEula,
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

/// The place and size window `slot` had at the last save.
pub fn saved_window(slot: usize) -> Option<persist::WindowState> {
    LayoutState::load(slot).window
}

pub struct Tab {
    pub root: PaneNode<Pane>,
    pub active: Pane,
    /// In a plugin's workspace, the automation this tab is: its panel, its browser pages and
    /// these terminals go together, and each tab runs on its own.
    pub instance: Option<persist::TabInstance>,
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
    /// `dormant` is only the tab that was closed last (the workspace fell asleep when its last tab
    /// closed), which is still in `closed_tabs`: a new tab there needs nothing brought back.
    pub asleep_on_close: bool,
    /// Tabs closed here, newest first, with their splits: a workspace keeps its tab history until
    /// the workspace itself is removed.
    pub closed_tabs: Vec<TabSnapshot>,
    /// Colour the card is filled with.
    pub color: Option<u32>,
    /// The plugin this workspace belongs to (`mode: "workspace"`): its panel and the browser come
    /// up while it is in front.
    pub plugin: Option<String>,
}

/// How many closed tabs a workspace remembers.
pub const CLOSED_TAB_HISTORY: usize = 10;

/// The last state of a workspace with nothing running, as [`Workbench::dormant_info`] reads it
/// back from the saved layout. The sidebar card is drawn from this while the workspace is closed.
pub(super) struct DormantInfo {
    pub cwd: PathBuf,
    /// Tools that were open, the last used one first.
    pub tools: Vec<String>,
    pub tabs: usize,
    pub panes: usize,
    pub branch: Option<String>,
    pub last_activity_ms: Option<u64>,
}

pub struct Group {
    pub id: u64,
    pub name: String,
    pub collapsed: bool,
    /// Colour the group is drawn with; `None` uses the neutral chrome.
    pub color: Option<u32>,
}

/// Colours a workspace or a group can be marked with, in the order the picker shows them.
/// Saturated on purpose: they fill a whole card, and a washed-out fill over the dark chrome reads
/// as dirt rather than as a colour. White text stays legible on every one of them.
pub const ACCENTS: [u32; 8] = [
    0x2f6fed, // blue
    0x1f9d55, // green
    0xd98324, // amber
    0xd04545, // red
    0x8b5cf6, // purple
    0x0d9aa8, // teal
    0xd8458f, // pink
    0x5b6b80, // slate
];

/// The full palette behind "more colours": the same hues in three shades, so a sidebar full of
/// workspaces can still give each one a colour of its own.
pub const PALETTE: [u32; 24] = [
    0x7aa2f7, 0x2f6fed, 0x1b46a8, // blue
    0x5fd1a0, 0x1f9d55, 0x136b3a, // green
    0xf0b357, 0xd98324, 0x9a5a12, // amber
    0xe97b7b, 0xd04545, 0x8f2c2c, // red
    0xb69cfb, 0x8b5cf6, 0x5b34c2, // purple
    0x4fc7d3, 0x0d9aa8, 0x076a75, // teal
    0xe887b6, 0xd8458f, 0x9a2c63, // pink
    0x94a3b8, 0x5b6b80, 0x3a4553, // slate
];

/// A colour a workspace or group carries, if it has one. Older layouts stored an index into
/// [`ACCENTS`]; both are read, and what is written from now on is the colour itself.
pub fn accent_color(color: Option<u32>) -> Option<u32> {
    color
}

/// Colour of a saved workspace or group: the stored value, else the legacy accent index.
pub fn stored_color(value: Option<u32>, legacy_index: Option<usize>) -> Option<u32> {
    value.or_else(|| legacy_index.and_then(|i| ACCENTS.get(i).copied()))
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
    /// Monitoring → every git working tree on this computer.
    Worktrees,
    /// Monitoring → what fills the disk, and clearing build output and caches.
    Disk,
    Settings,
    Extensions,
    Plugins,
    /// "Build my idea": describe an idea, an agent builds and previews it.
    Idea,
    /// The active project's databases (only offered when it has some).
    Database,
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

/// How long a branch's pull request is remembered before asking GitHub again.
const PR_REFRESH: std::time::Duration = std::time::Duration::from_secs(180);
/// Pull-request lookups started in one pass; the rest wait for the next one. Each is a `gh`
/// process talking to GitHub, so a window full of branches asks for a few at a time.
const PR_LOOKUPS_AT_ONCE: usize = 4;

/// A context this full is what the meter already draws in orange, and where compacting starts
/// being worth offering: the bar's own compact button appears, and continuing a session offers to
/// compact it first.
const COMPACT_OFFER_AT: f64 = 70.0;

/// How long a status bar message stays.
const STATUS_DURATION: std::time::Duration = std::time::Duration::from_secs(6);
/// How long a toast stays: long enough to read it after looking away from the pane.
const DEFAULT_TOAST_MS: u64 = 5_000;

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
    /// Where in the window the open workspace menu was asked for, so a card near the bottom of the
    /// sidebar opens its menu upwards instead of off the edge.
    workspace_menu_at: f32,
    /// Group whose right-click menu is open.
    group_menu: Option<u64>,
    /// Whose colour menu has the full palette unfolded.
    color_palette_open: Option<chrome::ColorTarget>,
    /// Whose colour the system colour panel is currently driving.
    color_picking: Option<chrome::ColorTarget>,
    /// Branch of a workspace that has not been opened yet, by folder: it has no pane to ask.
    folder_branches: HashMap<PathBuf, (Option<String>, std::time::Instant)>,
    /// Pull request per (repository, branch), and when it was last looked up.
    pull_requests: HashMap<(PathBuf, String), (Option<agentty_bridge::github::PullRequest>, std::time::Instant)>,
    /// Which repository a folder belongs to, and when that was worked out. Answering it means
    /// running `git rev-parse`, so it is answered once in the background and read from here after.
    repo_roots: HashMap<PathBuf, (Option<PathBuf>, std::time::Instant)>,
    /// The workspace being removed on purpose: closing its panes must not keep it around.
    removing_workspace: Option<u64>,
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
    /// Sidebar auto-scroll while dragging near its top or bottom edge: pixels per tick, and the
    /// task applying them (a held pointer has no way to scroll otherwise).
    drag_scroll_delta: f32,
    drag_scroll: Option<gpui::Task<()>>,
    browser_resizing: bool,
    /// Files panel docked at the right edge (folder structure, changes, working trees).
    files_panel: Option<files_panel::FilesPanel>,
    files_resizing: bool,
    /// A side panel (plugin, Docker) whose left edge is being dragged.
    side_resizing: Option<side_panels::SidePanel>,
    /// Dragging the handle under the files panel's working-tree list: (pointer y, height) at the start.
    files_trees_drag: Option<(f32, f32)>,
    /// Dragging the top edge of the browser's network panel: (mouse y at the start, its height).
    browser_net_drag: Option<(f32, f32)>,
    /// Docker of the active pane's project: the status bar chip and the panel docked at the right.
    docker: docker_panel::DockerState,
    db: db_page::DbState,
    /// Window width at the last render, for sizing the panels docked at the right.
    viewport_width: f32,
    browser_home_input: Option<(Entity<TextInput>, Subscription)>,
    chat_notify: notify_settings::ChatNotifyState,
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
    /// A folder change is waiting to be saved; see `persist_soon`.
    persist_pending: bool,
    /// The window's place and size, saved with the layout so a restart opens it the same.
    window_state: Option<persist::WindowState>,
    /// Panes whose process ended, waiting to close; see `pane_exited`.
    exited_panes: Vec<Pane>,
    exit_generation: u64,
    /// The panels as last saved; a render that finds them changed saves again.
    saved_panels: persist::PanelState,
    /// Browser tabs to bring back (addresses, the one in front) when the panel is next built.
    browser_restore: Option<(Vec<String>, usize)>,
    mini_opening: bool,
    /// Counts folds into the mini panel. A fold waits out its fade in a task, and the press that
    /// cancels it counts a new one, so that task can tell it is no longer the fold being asked for.
    mini_generation: u64,
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
    /// Last toast id handed out. Never reset, so a closed toast's timer can't close a newer one.
    toast_seq: u64,
    /// Title last given to the native window (the Dock and Window menus list windows by it).
    native_title: String,
    /// Spend and limit resets per agent, for the menu bar.
    account_usage: Vec<account_usage::AccountUsage>,
    /// Incremental transcript scanner behind `account_usage`, shared with the refresh button.
    usage_scanner: std::sync::Arc<std::sync::Mutex<agentty_bridge::usage::UsageScanner>>,
    /// Subagents / session links popover of a pane.
    agent_panel: Option<agent_panel::AgentPanel>,
    session_viewer: Option<session_viewer::SessionViewer>,
    /// File editor (files opened from the files panel), and whether it is what the main area shows.
    editor: Option<Entity<crate::editor::CodeEditor>>,
    editor_shown: bool,
    /// What changed about a file, shown where the editor is (the files panel's "changes" tab).
    file_diff: Option<file_diff::FileDiff>,
    editor_subscription: Option<Subscription>,
    /// The user already answered "unsaved files — quit / close anyway?".
    discard_confirmed: bool,
    /// File ⌘-clicked in a terminal, opened in the editor on the next frame (which has a window).
    pending_editor_open: Option<PathBuf>,
    /// A short question waiting for an answer (migrate, delete, compact before resuming).
    ask: Option<ask::Ask>,
    launcher_more: bool,
    service_status: HashMap<&'static str, agentty_bridge::service_status::ServiceStatus>,
    status_dismissed: std::collections::HashSet<String>,
    resume_dismissed: std::collections::HashSet<(u64, PathBuf)>,
    resume_menu: Option<u64>,
    harness_cache: harness::HarnessCache,
    harness_dialog: Option<harness::HarnessDialog>,
    status_menu: Option<status_menus::StatusMenu>,
    /// The pane whose bar opened `status_menu`. A menu belongs to the chip it was opened from —
    /// every split pane draws the same bar, and what the menu reads comes from this pane, not from
    /// whichever one happens to have the keyboard.
    status_menu_pane: Option<Pane>,
    processes: processes::ProcessMonitor,
    tree_manager: tree_manager::TreeManager,
    disk_manager: disk_page::DiskManager,
    inventory: status_menus::AgentInventory,
    browser: Option<browser::BrowserPanel>,
    browser_request: Option<String>,
    /// The workspace the user was in before going to a plugin's (its icon, pressed again, goes back).
    before_plugin_workspace: Option<u64>,
    /// The plugin whose workspace is in front and whose panel it brought up.
    plugin_workspace_shown: Option<String>,
    /// The user's own browser, set aside while a plugin's workspace (whose browser shows its
    /// automation's pages) is in front.
    stashed_browser: Option<browser::BrowserPanel>,
    /// The plugin and automation whose pages the browser shows now.
    instance_shown: Option<(String, String)>,
    /// Automations each plugin has been told about, and the run it was told in.
    known_instances: HashMap<String, (Option<u64>, Vec<String>)>,
    /// Whether the workspace list was open before a plugin's workspace folded it away.
    plugin_workspace_sidebar: Option<bool>,
    /// An edge of a plugin's workspace being dragged.
    plugin_ws_drag: Option<plugin_workspace::PluginWorkspaceDrag>,
    /// Pages plugins drive (`browser/*`), in sight or not.
    plugin_browsers: Vec<plugin_browser::PluginBrowser>,
    /// Plugin pages to put in the browser panel at the next render.
    plugin_pages_to_show: Vec<u64>,
    /// Plugins whose browser consent dialog is open, with the calls waiting for the answer.
    browser_asking: std::collections::HashMap<String, Vec<crate::plugins::PluginCall>>,
    /// Plugins the user said no to, with the run the answer holds for.
    browser_refused: std::collections::HashMap<String, u64>,
    /// Out-of-sight visits that keep a site's session going (`browser_keeper`).
    site_refreshes: Vec<(u64, crate::webview::WebView)>,
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
    /// A real terminal shown in Settings → Appearance, so the font, spacing, weight and colours
    /// are judged on the thing itself rather than on a mock-up. Started when that page is opened
    /// and dropped when it is left, so it costs nothing the rest of the time.
    preview_terminal: Option<Pane>,
    session_search: Entity<TextInput>,
    /// Sessions whose transcript mentions the current query (filled in the background).
    session_content_hits: Option<(String, std::collections::HashSet<PathBuf>)>,
    session_search_generation: u64,
    _session_search_subscription: Subscription,
    workspace_search: Entity<TextInput>,
    /// Workspaces whose session transcripts mention the current query (filled in the background).
    /// Title matches show immediately from `self.workspaces`, without waiting on this.
    workspace_content_hits: Option<(String, std::collections::HashSet<u64>)>,
    workspace_search_generation: u64,
    _workspace_search_subscription: Subscription,
    notices: Vec<notices::Notice>,
    notices_open: bool,
    window_active: bool,
    alias_form: Option<settings_page::AliasForm>,
    accounts_form: Option<accounts_page::AccountsForm>,
    /// Settings → System check results (Windows / Linux), and whether a check is running.
    system_check: Option<Vec<crate::setup_check::Tool>>,
    system_checking: bool,
    /// Tool id → the pane its install command runs in, so a second click goes to that tab instead
    /// of starting the install again. The entity id, never the entity itself: holding a pane here
    /// would keep it alive after its tab was closed, and dropping the pane is what ends what runs
    /// in it (`terminal::backend`'s `Drop` signals the process group).
    system_installs: Vec<(&'static str, gpui::EntityId)>,
    harness_pattern_form: Option<settings_page::HarnessPatternForm>,
    /// Plugin whose panel is docked right of the terminals.
    plugin_panel: Option<String>,
    /// The panel's layout menu is open.
    plugin_mode_menu: bool,
    /// Plugin panels that have a window of their own, by plugin id.
    plugin_windows: HashMap<String, gpui::WindowHandle<plugin_window::PluginWindow>>,
    /// Panes a plugin started, and the status each was last told about: how a plugin hears that
    /// the agent it set to work has finished.
    plugin_panes: HashMap<u64, (String, &'static str)>,
    /// Panes each plugin launched itself (prompt/inject into a new tab, split or workspace): the
    /// ones it may type into without the user having just asked it to.
    plugin_launched: HashMap<u64, String>,
    /// The plugin whose permissions the user is being asked about.
    plugin_consent_open: Option<String>,
    /// When the user last used each plugin's UI (a panel control, one of its commands): for a
    /// short while after that, the plugin may act on the terminal the user is in.
    plugin_gesture: HashMap<String, std::time::Instant>,
    /// Whether the loop that looks at those panes while nothing is drawn is already running.
    plugin_pane_poll: bool,
    /// Plugins whose own window has been asked for but not yet opened — opening is deferred, and
    /// without this the next frame would ask for a second one.
    plugin_windows_opening: std::collections::HashSet<String>,
    /// Plugins whose own window Agentty is closing itself, so the release observer does not read
    /// it as the user closing the panel.
    plugin_windows_closing: std::collections::HashSet<String>,
    plugin_inputs: HashMap<(String, String), plugin_panel::PluginInput>,
    plugin_scroll: gpui::ScrollHandle,
    welcome_scroll: gpui::ScrollHandle,
    /// Context last sent to plugins (serialized), to send only changes.
    /// The context last sent to plugins, kept to notice when it changed.
    plugin_context_key: serde_json::Value,
    prompt_dialog: Option<prompt_dialog::PromptDialog>,
    /// Prompts (links, plugins) that arrived while the dialog showed another one.
    prompt_queue: std::collections::VecDeque<agentty_bridge::plugins::PromptRequest>,
    /// Parallel tasks agents asked for (`agentty tasks`), waiting for the user; the first is shown.
    task_requests: std::collections::VecDeque<crate::agent_signal::TasksRequest>,
    /// A CLI the user picked that isn't installed: what to tell them, and where to read more.
    install_hint: Option<(&'static str, &'static str, &'static str)>,
    /// The start page is shown even though workspaces exist (opened from the sidebar).
    welcome: bool,
    /// "Pick a pane to connect": the pane the link starts from.
    connect_pick: Option<u64>,
    plugins_page: plugins_page::PluginsPage,
    /// First-run onboarding (dialog steps, then the follow-along tour card).
    onboarding: Option<onboarding::Onboarding>,
    /// The first-run tour opens once the system check finds the environment ready.
    onboarding_waits_for_setup: bool,
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
        let workspace_search = cx.new(|cx| TextInput::localized("", "workspaces.search", window, cx));
        let workspace_search_subscription = cx.subscribe(&workspace_search, |this, _, event: &crate::text_input::TextInputEvent, cx| {
            if matches!(event, crate::text_input::TextInputEvent::Changed) {
                this.search_workspace_contents(cx);
                cx.notify();
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
            workspace_menu_at: 0.,
            group_menu: None,
            color_palette_open: None,
            color_picking: None,
            folder_branches: HashMap::new(),
            pull_requests: HashMap::new(),
            repo_roots: HashMap::new(),
            removing_workspace: None,
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
            drag_scroll_delta: 0.,
            drag_scroll: None,
            browser_resizing: false,
            files_panel: None,
            files_resizing: false,
            side_resizing: None,
            files_trees_drag: None,
            browser_net_drag: None,
            docker: Default::default(),
            db: Default::default(),
            viewport_width: 1400.,
            browser_home_input: None,
            chat_notify: Default::default(),
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
            persist_pending: false,
            window_state: Some(persist::WindowState::from_bounds(window.window_bounds())),
            saved_panels: persist::PanelState::default(),
            exited_panes: Vec::new(),
            exit_generation: 0,
            browser_restore: None,
            mini_opening: false,
            mini_generation: 0,
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
            toast_seq: 0,
            native_title: String::new(),
            account_usage: Vec::new(),
            usage_scanner: Default::default(),
            agent_panel: None,
            session_viewer: None,
            editor: None,
            editor_shown: false,
            file_diff: None,
            editor_subscription: None,
            discard_confirmed: false,
            pending_editor_open: None,
            ask: None,
            launcher_more: false,
            service_status: HashMap::new(),
            status_dismissed: Default::default(),
            resume_dismissed: Default::default(),
            resume_menu: None,
            harness_cache: Default::default(),
            harness_dialog: None,
            status_menu: None,
            status_menu_pane: None,
            processes: Default::default(),
            tree_manager: Default::default(),
            disk_manager: Default::default(),
            inventory: Default::default(),
            browser: None,
            browser_request: None,
            plugin_browsers: Vec::new(),
            before_plugin_workspace: None,
            plugin_workspace_shown: None,
            stashed_browser: None,
            instance_shown: None,
            known_instances: HashMap::new(),
            plugin_workspace_sidebar: None,
            plugin_ws_drag: None,
            plugin_pages_to_show: Vec::new(),
            browser_asking: Default::default(),
            browser_refused: Default::default(),
            site_refreshes: Vec::new(),
            find_bar: None,
            shared_tree_warned: Default::default(),
            servers: Default::default(),
            installed_fonts: None,
            font_list_open: false,
            sessions_scroll: gpui::UniformListScrollHandle::new(),
            sidebar_scroll: gpui::ScrollHandle::new(),
            settings_scroll: gpui::ScrollHandle::new(),
            settings_section: settings_page::SettingsSection::General,
            preview_terminal: None,
            session_search,
            session_content_hits: None,
            session_search_generation: 0,
            _session_search_subscription: session_search_subscription,
            workspace_search,
            workspace_content_hits: None,
            workspace_search_generation: 0,
            _workspace_search_subscription: workspace_search_subscription,
            notices: Vec::new(),
            notices_open: false,
            window_active: true,
            alias_form: None,
            accounts_form: None,
            system_check: None,
            system_checking: false,
            system_installs: Vec::new(),
            harness_pattern_form: None,
            plugin_panel: None,
            plugin_mode_menu: false,
            plugin_windows: HashMap::new(),
            plugin_panes: HashMap::new(),
            plugin_launched: HashMap::new(),
            plugin_consent_open: None,
            plugin_gesture: HashMap::new(),
            plugin_pane_poll: false,
            plugin_windows_opening: std::collections::HashSet::new(),
            plugin_windows_closing: std::collections::HashSet::new(),
            plugin_inputs: HashMap::new(),
            plugin_scroll: gpui::ScrollHandle::new(),
            welcome_scroll: gpui::ScrollHandle::new(),
            plugin_context_key: serde_json::Value::Null,
            prompt_dialog: None,
            prompt_queue: std::collections::VecDeque::new(),
            task_requests: std::collections::VecDeque::new(),
            welcome: false,
            install_hint: None,
            connect_pick: None,
            plugins_page: Default::default(),
            onboarding: None,
            onboarding_waits_for_setup: false,
            proxy: proxy_page::ProxyPage::new(proxy_filter, proxy_subscription),
            next_id: 1,
        };
        this.restore(window, cx);
        // Moving or resizing the window is saved like a `cd`: in a moment, once it stops.
        cx.observe_window_bounds(window, |this, window, cx| {
            let state = persist::WindowState::from_bounds(window.window_bounds());
            if this.window_state != Some(state) {
                this.window_state = Some(state);
                this.persist_soon(cx);
            }
        })
        .detach();
        this.first_run_onboarding(cx);
        this.startup_system_check(cx);
        this.refresh_sessions(cx);
        this.refresh_folder_branches(cx);
        this.detect_agents(cx);
        this.start_update_checks(cx);
        this.start_service_status_checks(cx);
        this.start_account_usage(cx);
        this.start_server_watch(cx);
        // New sessions (for the resume bar and the sessions list) show up without a manual refresh.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(std::time::Duration::from_secs(120)).await;
            if this
                .update(cx, |this, cx| {
                    this.refresh_sessions(cx);
                    this.refresh_pull_requests(cx);
                    this.refresh_folder_branches(cx);
                })
                .is_err()
            {
                break;
            }
        })
        .detach();
        // With the menu bar item on, closing the main window hides it; Agentty keeps running there.
        // Additional windows close, but stay in the recent list (Dock and History menus) to reopen.
        let entity = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            let closes = slot > 0 || !crate::settings::settings(cx).menu_bar || !crate::platform::HAS_STATUS_ITEM;
            // Unsaved files in the editor: ask first (closing the main window ends Agentty).
            let then = if slot > 0 { crate::editor::AfterDiscard::CloseWindow } else { crate::editor::AfterDiscard::Quit };
            if closes && entity.update(cx, |this, cx| this.ask_about_unsaved_files(then, window, cx)).unwrap_or(false) {
                return false;
            }
            if slot > 0 {
                let _ = entity.update(cx, |this, cx| this.remember_closed_window(cx));
                crate::set_app_menus(cx);
                return true;
            }
            if closes {
                // Closing the main window ends Agentty and every terminal in it: while one runs,
                // ask first (the answer quits through the usual path).
                let running =
                    entity.read_with(cx, |this, cx| this.all_panes().iter().any(|pane| pane.read(cx).is_running())).unwrap_or(false);
                if running {
                    cx.defer(crate::request_quit_asking);
                    return false;
                }
                // The last window closing ends Agentty, and the quit hook holds only a weak
                // handle to this workbench: by the time it runs the window is gone and nothing
                // would be written. Save while there is still something to save.
                let _ = entity.update(cx, |this, cx| this.persist(cx));
                return true;
            }
            if let Some(ns) = crate::native::ns_window(window) {
                crate::native::order_out(ns);
            }
            false
        });
        cx.on_app_quit(|this, cx| {
            this.persist(cx);
            // Whichever way the app is going down: the sleep lock is a child process and would
            // outlive it. Letting go of it twice is a no-op.
            crate::platform::wakelock::set(false);
            async {}
        })
        .detach();
        this
    }

    /// A second window is closing: saves it and lists it in the recent windows to reopen.
    pub(super) fn remember_closed_window(&mut self, cx: &mut Context<Self>) {
        self.persist(cx);
        let names: Vec<String> = self.workspaces.iter().map(|ws| self.workspace_title(ws, cx)).collect();
        let title = match names.as_slice() {
            [] => String::new(),
            [one] => one.clone(),
            [first, rest @ ..] => format!("{first} +{}", rest.len()),
        };
        ClosedWindows::remember(self.slot, title, !names.is_empty());
        self.closed = true;
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
            TerminalEvent::Exited => this.pane_exited(pane.clone(), cx),
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
            // A `cd` moves where the pane works, and that is part of the saved layout. Saving it
            // here rather than at quit is what makes it survive every way Agentty can go down —
            // a closed window, a force quit or a crash never reach the quit hook.
            TerminalEvent::DirectoryChanged | TerminalEvent::SessionChanged => this.persist_soon(cx),
            TerminalEvent::TitleChanged | TerminalEvent::StatusChanged => {
                // Looked at, or answered (the agent is at work again): its notifications stop
                // calling the user. A finished turn's stays until then.
                let (pane_id, moved_on, seen) = {
                    let view = pane.read(cx);
                    (view.pane_id, view.status.in_turn(), !view.attention)
                };
                if moved_on || seen {
                    crate::notifications::withdraw(pane_id);
                }
                // A shell that changed folder may have entered a project with an agent harness.
                this.watch_harness(&pane, cx);
                // It may also have landed on another branch, whose pull request the card shows.
                this.refresh_pull_requests(cx);
                this.warn_about_shared_tree(&pane, cx);
                cx.notify();
            }
            TerminalEvent::OpenLink(url) => this.open_link(url.clone(), cx),
            // A file the agent named: open it right here. Folders still go to the file manager.
            TerminalEvent::RevealPath(path) => {
                if path.is_dir() {
                    crate::platform::reveal(path);
                } else {
                    this.pending_editor_open = Some(path.clone());
                    cx.notify();
                }
            }
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
                self.persist_soon(cx);
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
        // Read off the pane before it goes: a workspace that falls dormant on this close keeps
        // what its card was showing (branch, last activity) instead of emptying out.
        let last_branch = pane.read(cx).git_branch.clone();
        let last_activity = pane.read(cx).last_activity_ms;
        self.flow_forget_pane(removed_id, cx);
        self.notices.retain(|n| n.pane_id != removed_id);
        let Some((w, t)) = self.locate(pane) else { return };
        let ws = &mut self.workspaces[w];
        let tab = ws.tabs.remove(t);
        match tab.root.remove(pane) {
            Some(root) => {
                let active = if tab.active == *pane { root.leaves()[0].clone() } else { tab.active };
                ws.tabs.insert(t, Tab { root, active, instance: tab.instance });
            }
            None => {
                if ws.active_tab >= ws.tabs.len() {
                    ws.active_tab = ws.tabs.len().saturating_sub(1);
                } else if t < ws.active_tab {
                    ws.active_tab -= 1;
                }
            }
        }
        // Closing the last tab is not deleting the workspace: it goes dormant on the tab that just
        // closed, so opening it again brings that tab back. Removing it is an explicit menu action.
        if ws.tabs.is_empty() && ws.dormant.is_none() {
            let closed = ws.closed_tabs.first().cloned();
            let removing = self.removing_workspace == Some(ws.id);
            match closed.filter(|_| !removing) {
                Some(tab) => {
                    let ws = &mut self.workspaces[w];
                    ws.asleep_on_close = true;
                    ws.dormant = Some(WorkspaceSnapshot {
                        id: ws.id,
                        name: ws.name.clone(),
                        group: ws.group,
                        cwd: ws.cwd.clone(),
                        tabs: vec![tab],
                        active_tab: 0,
                        closed_tabs: ws.closed_tabs.clone(),
                        color: None,
                        color_value: ws.color,
                        branch: last_branch,
                        last_activity_ms: Some(last_activity),
                        plugin: ws.plugin.clone(),
                    });
                }
                None => {
                    self.workspaces.remove(w);
                    if self.active_workspace >= self.workspaces.len() {
                        self.active_workspace = self.workspaces.len().saturating_sub(1);
                    } else if w < self.active_workspace {
                        self.active_workspace -= 1;
                    }
                }
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
        self.hide_editor();
        self.launcher_open = false;
        self.detect_agents(cx);
        // Tools may have been installed since: the start page's setup bar follows.
        self.run_system_check(false, cx);
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
            tabs: vec![Tab { root: PaneNode::Leaf(pane.clone()), active: pane, instance: None }],
            active_tab: 0,
            dormant: None,
            asleep_on_close: false,
            closed_tabs: Vec::new(),
            color: None,
            plugin: None,
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
        self.wake_for_new_tab(self.active_workspace, cx);
        let pane = self.spawn_pane(spec, cx);
        let ws = &mut self.workspaces[self.active_workspace];
        ws.tabs.push(Tab { root: PaneNode::Leaf(pane.clone()), active: pane, instance: None });
        ws.active_tab = ws.tabs.len() - 1;
        self.page = None;
        self.session_viewer = None;
        self.hide_editor();
        self.launcher_open = false;
        self.focus_active(window, cx);
        self.persist(cx);
        cx.notify();
    }

    /// Adds a tab and makes it the workspace's active one, but leaves the page that is on screen
    /// where it is. Settings → System check uses it: installing one tool must not throw the user
    /// off the page when two more are still missing. Returns the new pane, or `None` when there
    /// was no workspace yet and the tab had to take over.
    pub(super) fn open_tab_behind(&mut self, spec: LaunchSpec, window: &mut Window, cx: &mut Context<Self>) -> Option<Pane> {
        self.welcome = false;
        if self.workspaces.is_empty() {
            self.create_workspace(spec, window, cx);
            return None;
        }
        self.wake_for_new_tab(self.active_workspace, cx);
        let pane = self.spawn_pane(spec, cx);
        let ws = &mut self.workspaces[self.active_workspace];
        ws.tabs.push(Tab { root: PaneNode::Leaf(pane.clone()), active: pane.clone(), instance: None });
        ws.active_tab = ws.tabs.len() - 1;
        self.persist(cx);
        cx.notify();
        Some(pane)
    }

    /// Brings a pane to the front and leaves the keyboard in it: its workspace, its tab, and the
    /// pane itself when the tab is split.
    pub(super) fn go_to_pane(&mut self, pane: &Pane, window: &mut Window, cx: &mut Context<Self>) {
        self.reveal_pane(pane, window, cx);
        self.mark_active(pane, cx);
        self.focus_pane(pane, window, cx);
    }

    /// Brings the tab holding a pane to the front, leaving whatever page was on screen.
    pub(super) fn reveal_pane(&mut self, pane: &Pane, window: &mut Window, cx: &mut Context<Self>) {
        let Some((workspace, tab)) = self.locate(pane) else { return };
        if workspace != self.active_workspace {
            self.activate_workspace(workspace, window, cx);
        }
        self.activate_tab(tab, window, cx);
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
        self.hide_editor();
        self.new_workspace = None;
        self.launcher_open = false;
        self.workspace_menu = None;
        if let Some(snapshot) = self.workspaces[index].dormant.take() {
            self.revive(index, snapshot, cx);
        }
        self.sync_plugin_workspace(cx);
        self.focus_active(window, cx);
        cx.notify();
    }

    /// A pane's process ended. It closes a moment later, with any others that end meanwhile.
    ///
    /// Several ending together is not the user closing them: a logout or shutdown ends the
    /// shells before Agentty hears it is quitting, and `killall` ends every one. Closing them at
    /// once would save the layout without them, and the quit that follows would keep it that way;
    /// waiting lets the quit save them as they were. After the wait (no quit came) they close.
    fn pane_exited(&mut self, pane: Pane, cx: &mut Context<Self>) {
        self.exited_panes.push(pane);
        self.exit_generation += 1;
        let generation = self.exit_generation;
        let wait = std::time::Duration::from_millis(if self.exited_panes.len() > 1 { 3000 } else { 300 });
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            let _ = this.update(cx, |this, cx| {
                if this.exit_generation != generation || this.closed {
                    return;
                }
                for pane in std::mem::take(&mut this.exited_panes) {
                    if this.locate(&pane).is_none() {
                        continue;
                    }
                    // A process ending on its own closes its tab as surely as the ✕ does, so the
                    // tab is remembered here too. Without it the workspace would fall dormant on
                    // whatever tab happened to be first in its history, and the one that just
                    // ended was lost. A single split ending is not a tab closing, and records
                    // nothing.
                    this.remember_closed_tabs(std::slice::from_ref(&pane), cx);
                    this.remove_pane(&pane, cx);
                }
            });
        })
        .detach();
    }

    /// Makes workspace `index` live before a tab is added to it. A sleeping workspace is saved
    /// from its snapshot, so a tab added beside the snapshot would never be written, and opening
    /// the workspace later would replace it. Its saved tabs come back first; one that fell
    /// asleep on its closed tab just wakes up empty (that tab stays in the closed-tab history).
    pub(super) fn wake_for_new_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(ws) = self.workspaces.get_mut(index) else { return };
        let Some(snapshot) = ws.dormant.take() else { return };
        if std::mem::take(&mut ws.asleep_on_close) {
            return;
        }
        self.revive(index, snapshot, cx);
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
            self.hide_editor();
            self.focus_active(window, cx);
            self.persist_soon(cx);
            cx.notify();
        }
    }

    /// Removes a workspace for good, with its tabs and its tab history.
    fn close_workspace(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.workspaces.iter().position(|w| w.id == id) else { return };
        let panes: Vec<Pane> = self.workspaces[index].tabs.iter().flat_map(|t| t.root.leaves()).collect();
        self.workspaces[index].dormant = None;
        self.removing_workspace = Some(id);
        for pane in panes {
            self.remove_pane(&pane, cx);
        }
        self.removing_workspace = None;
        if let Some(index) = self.workspaces.iter().position(|w| w.id == id) {
            self.workspaces.remove(index);
            self.active_workspace = self.active_workspace.min(self.workspaces.len().saturating_sub(1));
        }
        self.persist(cx);
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
        self.hide_editor();
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
        self.hide_editor();
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
        if self.launch_in_own_tree(&choice, target, &cwd, cx) {
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

    // -- workspace search ------------------------------------------------------------------

    /// What the workspace list is filtered by. A hidden search box filters nothing, whatever was
    /// left in it — turning the box off must not leave workspaces missing with no way to see why.
    pub(super) fn workspace_query(&self, cx: &gpui::App) -> String {
        if !crate::settings::settings(cx).workspace_search_bar {
            return String::new();
        }
        self.workspace_search.read(cx).text().trim().to_lowercase()
    }

    /// Every folder this workspace works in: its own root, and any open pane that has wandered
    /// elsewhere (a split into a sibling project, a `cd`). Session content search matches
    /// against these — the same folders `resume_hint` uses to offer a session back.
    fn workspace_dirs(&self, ws: &Workspace, cx: &gpui::App) -> Vec<PathBuf> {
        let mut dirs = vec![ws.cwd.clone()];
        for pane in ws.tabs.iter().flat_map(|t| t.root.leaves()) {
            let cwd = pane.read(cx).display_cwd();
            if !dirs.contains(&cwd) {
                dirs.push(cwd);
            }
        }
        dirs
    }

    /// Whether a workspace's title or session content matches the search box; a blank box
    /// matches everything. Title matches are instant; content matches wait on the background
    /// search below, so a stale query never wrongly hides a workspace.
    pub(super) fn workspace_matches(&self, index: usize, cx: &gpui::App) -> bool {
        let query = self.workspace_query(cx);
        if query.is_empty() {
            return true;
        }
        let ws = &self.workspaces[index];
        if self.workspace_title(ws, cx).to_lowercase().contains(&query) {
            return true;
        }
        self.workspace_content_hits.as_ref().is_some_and(|(q, hits)| *q == query && hits.contains(&ws.id))
    }

    /// Full-text search over the transcripts of each workspace's sessions, debounced; title
    /// matches show immediately and do not wait on this.
    fn search_workspace_contents(&mut self, cx: &mut Context<Self>) {
        self.workspace_search_generation += 1;
        let generation = self.workspace_search_generation;
        let query = self.workspace_query(cx);
        if query.chars().count() < 2 {
            self.workspace_content_hits = None;
            return;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(250)).await;
            if this.read_with(cx, |this, _| this.workspace_search_generation != generation).unwrap_or(true) {
                return;
            }
            // Which transcripts to read is worked out after the wait, not on every keystroke:
            // pairing every workspace's folders with every session grows with both.
            // (workspace id, transcript path) for every session run in one of that workspace's folders.
            let Ok(candidates) = this.update(cx, |this, cx| {
                let mut candidates: Vec<(u64, PathBuf)> = Vec::new();
                for index in 0..this.workspaces.len() {
                    let dirs = this.workspace_dirs(&this.workspaces[index], cx);
                    let id = this.workspaces[index].id;
                    for session in &this.sessions {
                        if session.cwd.as_deref().is_some_and(|c| dirs.iter().any(|d| Path::new(c) == d.as_path())) {
                            candidates.push((id, session.path.clone()));
                        }
                    }
                }
                candidates
            }) else {
                return;
            };
            let needle = query.clone();
            let hits: std::collections::HashSet<u64> = cx
                .background_spawn(async move {
                    candidates
                        .into_iter()
                        .filter(|(_, path)| agentty_bridge::transcript_contains(path, &needle))
                        .map(|(id, _)| id)
                        .collect()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.workspace_search_generation == generation {
                    this.workspace_content_hits = Some((query, hits));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Starts the terminal shown in Settings → Appearance while that page is open, and drops it
    /// when it is not. It runs one command that prints a sample — colours, bold, CJK, a Powerline
    /// glyph — and then sits idle, so nothing of the user's is running in it.
    fn prepare_style_preview(&mut self, cx: &mut Context<Self>) {
        let wanted = self.page == Some(Page::Settings) && self.settings_section == settings_page::SettingsSection::Appearance;
        match (wanted, self.preview_terminal.is_some()) {
            (true, false) => {
                let script = concat!(
                    "printf '\x1b[34m~/agentty\x1b[0m  \x1b[35mmain\x1b[0m\n';",
                    "printf '$ cargo test --workspace\n';",
                    "printf '\x1b[32m   Compiling\x1b[0m agentty-app v0.1.14\n';",
                    "printf '\x1b[36mtest result\x1b[0m: \x1b[32mok\x1b[0m. 464 passed; 0 failed\n';",
                    "printf '\x1b[33mwarning\x1b[0m: unused variable \x1b[1mx\x1b[0m\n';",
                    "printf '\x1b[31merror\x1b[0m: could not compile\n';",
                    "printf '\x1b[1mbold\x1b[0m normal \x1b[4munderline\x1b[0m 한글 日本語 中文 -> => !=\n';",
                    "exec cat",
                );
                let spec = crate::launch::LaunchSpec::shell_command(script.to_string(), "preview".to_string(), crate::launch::home_dir());
                self.preview_terminal = Some(cx.new(|cx| crate::terminal::TerminalView::new(spec, cx)));
            }
            (false, true) => self.preview_terminal = None,
            _ => {}
        }
    }

    /// The preview terminal, for the settings page to draw.
    pub(super) fn style_preview(&self) -> Option<Pane> {
        self.preview_terminal.clone()
    }

    pub(super) fn focus_workspace_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.page = None;
        self.panel = SidePanel::Workspaces;
        self.sidebar_open = true;
        window.focus(&self.workspace_search.focus_handle(cx));
        cx.notify();
    }

    /// The branch of a folder no pane is open in (a workspace still folded away).
    pub(super) fn folder_branch(&self, cwd: &std::path::Path) -> Option<&str> {
        self.folder_branches.get(cwd).and_then(|(branch, _)| branch.as_deref())
    }

    /// Reads the branch of every workspace that has no pane to ask, at most once per [`PR_REFRESH`].
    /// Without it a folded-away workspace showed no branch until it was opened.
    pub(super) fn refresh_folder_branches(&mut self, cx: &mut Context<Self>) {
        let wanted: Vec<PathBuf> = self
            .workspaces
            .iter()
            .filter(|ws| ws.tabs.is_empty())
            .map(|ws| self.dormant_cwd(ws))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let now = std::time::Instant::now();
        for cwd in wanted {
            if self.folder_branches.get(&cwd).is_some_and(|(_, at)| now.duration_since(*at) < PR_REFRESH) {
                continue;
            }
            self.folder_branches.entry(cwd.clone()).or_insert((None, now)).1 = now;
            let folder = cwd.clone();
            let task = cx.background_spawn(async move { agentty_bridge::git::status(&folder).ok().and_then(|s| s.branch) });
            cx.spawn(async move |this, cx| {
                let found = task.await;
                let _ = this.update(cx, |this, cx| {
                    let entry = this.folder_branches.entry(cwd).or_insert((None, std::time::Instant::now()));
                    if entry.0 != found {
                        entry.0 = found;
                        cx.notify();
                    }
                });
            })
            .detach();
        }
    }

    /// What a folded-away workspace last looked like, read off its saved layout: the same things
    /// its card shows while it is open, so opening it changes nothing but the colours.
    pub(super) fn dormant_info(&self, ws: &Workspace) -> DormantInfo {
        let snapshot = ws.dormant.as_ref();
        let all: Vec<&PaneSnapshot> = snapshot.map(|s| s.tabs.iter().flat_map(|t| snapshot_panes(&t.layout)).collect()).unwrap_or_default();
        let active = snapshot.and_then(|s| s.tabs.get(s.active_tab).or_else(|| s.tabs.first()));
        // The tab the workspace was left on leads, so the one logo the card shows is the one the
        // user last worked in; AI agents before shells, as on an open card.
        let ordered = active.map(|t| snapshot_panes(&t.layout)).unwrap_or_default().into_iter().chain(all.iter().copied());
        let mut tools: Vec<String> = Vec::new();
        for pane in ordered {
            let tool = pane.tool.clone().unwrap_or_else(|| crate::brand::kind_id(pane.kind).to_string());
            if !tools.contains(&tool) {
                tools.push(tool);
            }
        }
        tools.sort_by_key(|t| t.as_str() == "shell");
        DormantInfo {
            cwd: self.dormant_cwd(ws),
            tools,
            tabs: snapshot.map_or(0, |s| s.tabs.len()),
            panes: all.len(),
            branch: snapshot.and_then(|s| s.branch.clone()),
            last_activity_ms: snapshot.and_then(|s| s.last_activity_ms),
        }
    }

    /// Where a workspace works, taken from its saved layout when no pane is running.
    pub(super) fn dormant_cwd(&self, ws: &Workspace) -> PathBuf {
        ws.dormant
            .as_ref()
            .and_then(|snapshot| snapshot.tabs.get(snapshot.active_tab).or_else(|| snapshot.tabs.first()))
            .and_then(|tab| first_pane(&tab.layout))
            .map(|pane| pane.cwd.clone())
            .unwrap_or_else(|| ws.cwd.clone())
    }

    /// The pull request of a branch, as far as it is known; `None` while it has not been looked up.
    /// The repository a folder is in, as far as has been worked out already.
    ///
    /// Nothing is asked of git here. Answering it means running `git rev-parse` — a whole process,
    /// started and waited for — and this is read while a card is drawn, once per workspace, on
    /// every frame. Typing repaints, so every keystroke forked a git per card and waited for it,
    /// which is exactly what made typing feel slow. The answer barely ever changes, so it is
    /// looked up in the background and read from the map afterwards.
    pub(super) fn repo_root_of(&self, cwd: &std::path::Path) -> Option<&std::path::Path> {
        self.repo_roots.get(cwd).and_then(|(root, _)| root.as_deref())
    }

    /// Works out which repository each of `folders` is in, off the main thread, once per folder.
    /// A folder that becomes a repository later is picked up on the next refresh.
    fn refresh_repo_roots(&mut self, folders: Vec<PathBuf>, cx: &mut Context<Self>) {
        let now = std::time::Instant::now();
        for cwd in folders {
            if self.repo_roots.get(&cwd).is_some_and(|(root, at)| root.is_some() || now.duration_since(*at) < PR_REFRESH) {
                continue;
            }
            // Marked as asked right away, so a slow answer is not asked for again every frame.
            self.repo_roots.entry(cwd.clone()).or_insert((None, now)).1 = now;
            let folder = cwd.clone();
            let task = cx.background_spawn(async move { agentty_bridge::git::repo_root(&folder) });
            cx.spawn(async move |this, cx| {
                let found = task.await;
                let _ = this.update(cx, |this, cx| {
                    let entry = this.repo_roots.entry(cwd).or_insert((None, std::time::Instant::now()));
                    if entry.0 != found {
                        entry.0 = found;
                        // Now that the repository is known, what hangs off it can be looked up.
                        this.refresh_pull_requests(cx);
                        cx.notify();
                    }
                });
            })
            .detach();
        }
    }

    pub(super) fn pull_request_of(&self, repo: &std::path::Path, branch: &str) -> Option<&agentty_bridge::github::PullRequest> {
        self.pull_requests.get(&(repo.to_path_buf(), branch.to_string())).and_then(|(pr, _)| pr.as_ref())
    }

    /// Looks up the pull request of every branch a pane is on, at most once per [`PR_REFRESH`].
    pub(super) fn refresh_pull_requests(&mut self, cx: &mut Context<Self>) {
        // The folder each branch is in, and then the repository it belongs to — which is read from
        // what was worked out in the background, never asked of git here: this runs on the main
        // thread, and starting a process per pane on it is what the user feels as a stutter.
        let mut folders: Vec<PathBuf> = Vec::new();
        let mut wanted: Vec<(PathBuf, String)> = Vec::new();
        for pane in self.all_panes() {
            let view = pane.read(cx);
            let Some(branch) = view.git_branch.clone() else { continue };
            let cwd = view.display_cwd();
            let Some(repo) = self.repo_root_of(&cwd).map(std::path::Path::to_path_buf) else {
                if !folders.contains(&cwd) {
                    folders.push(cwd);
                }
                continue;
            };
            let key = (repo, branch);
            if !wanted.contains(&key) {
                wanted.push(key);
            }
        }
        // Folded-away workspaces too: their card shows a pull request as well, and without this it
        // only appeared once the workspace had been opened.
        for ws in &self.workspaces {
            if !ws.tabs.is_empty() {
                continue;
            }
            let cwd = self.dormant_cwd(ws);
            let Some(branch) = self.folder_branch(&cwd).map(str::to_string).or_else(|| ws.dormant.as_ref()?.branch.clone()) else {
                continue;
            };
            let Some(repo) = self.repo_root_of(&cwd).map(std::path::Path::to_path_buf) else {
                if !folders.contains(&cwd) {
                    folders.push(cwd);
                }
                continue;
            };
            let key = (repo, branch);
            if !wanted.contains(&key) {
                wanted.push(key);
            }
        }
        self.refresh_repo_roots(folders, cx);
        let now = std::time::Instant::now();
        let mut started = 0;
        for key in wanted {
            if self.pull_requests.get(&key).is_some_and(|(_, at)| now.duration_since(*at) < PR_REFRESH) {
                continue;
            }
            // A few at a time: the rest are picked up on the next pass, since only the ones
            // actually started are marked as looked up.
            if started >= PR_LOOKUPS_AT_ONCE {
                break;
            }
            started += 1;
            // Marked as looked up right away, so a slow `gh` is not started again every frame.
            self.pull_requests.entry(key.clone()).or_insert((None, now)).1 = now;
            let (repo, branch) = key.clone();
            let task = cx.background_spawn(async move { agentty_bridge::github::pull_request(&repo, &branch) });
            cx.spawn(async move |this, cx| {
                let found = task.await;
                let _ = this.update(cx, |this, cx| {
                    let entry = this.pull_requests.entry(key).or_insert((None, std::time::Instant::now()));
                    if entry.0 != found {
                        entry.0 = found;
                        cx.notify();
                    }
                });
            })
            .detach();
        }
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

    /// Continues a session. A pane already running it is brought to the front instead of starting
    /// a second copy, and a nearly full context window is offered a compaction first.
    fn resume_session(&mut self, session: &SessionInfo, window: &mut Window, cx: &mut Context<Self>) {
        if self.jump_to_session(&session.id, window, cx) {
            self.show_resumed_terminal(cx);
            return;
        }
        let nearly_full = agentty_bridge::session_stats(session.agent, &session.id)
            .and_then(|stats| stats.context_percent())
            .is_some_and(|percent| percent >= COMPACT_OFFER_AT);
        let compactable = crate::launch::PaneKind::from(session.agent).compact_command().is_some();
        if nearly_full && compactable {
            let title = tf(cx, "sessions.compact_title", &[("name", &session.title)]);
            return self.ask(
                ask::Ask {
                    title: title.into(),
                    body: Some(t(cx, "sessions.compact_body").into()),
                    choices: vec![
                        ask::AskChoice {
                            label: t(cx, "sessions.compact_then_run").into(),
                            action: ask::AskAction::Resume { session: session.clone(), compact: true },
                            primary: true,
                            danger: false,
                        },
                        ask::AskChoice {
                            label: t(cx, "sessions.just_run").into(),
                            action: ask::AskAction::Resume { session: session.clone(), compact: false },
                            primary: false,
                            danger: false,
                        },
                    ],
                    cancel: true,
                },
                cx,
            );
        }
        self.resume_session_compacting(session, false, window, cx);
    }

    /// Starts the session in a new workspace; with `compact`, the agent is asked to compact its
    /// context as its first instruction, once it is actually running.
    pub(super) fn resume_session_compacting(&mut self, session: &SessionInfo, compact: bool, window: &mut Window, cx: &mut Context<Self>) {
        let cwd = session.cwd.as_ref().map(PathBuf::from).filter(|p| p.is_dir()).unwrap_or_else(home_dir);
        let spec = LaunchSpec::resume(session.agent, session.id.clone(), session.title.clone(), cwd);
        self.create_workspace(spec, window, cx);
        if let Some(ws) = self.workspaces.last_mut() {
            ws.name = Some(session.title.clone());
        }
        self.show_resumed_terminal(cx);
        if !compact {
            return;
        }
        let Some(pane) = self.active_pane() else { return };
        let Some(command) = crate::launch::PaneKind::from(session.agent).compact_command() else { return };
        // The CLI takes a moment to come up; waiting for it to be seen beats guessing a delay.
        cx.spawn(async move |_, cx| {
            for _ in 0..60 {
                cx.background_executor().timer(std::time::Duration::from_millis(500)).await;
                let ready = pane.read_with(cx, |view, _| view.live_agent.is_some()).unwrap_or(false);
                if ready {
                    cx.background_executor().timer(std::time::Duration::from_millis(1_200)).await;
                    let _ = pane.update(cx, |view, cx| view.submit_prompt(command.to_string(), cx));
                    return;
                }
            }
        })
        .detach();
    }

    /// Focuses the pane already running `session_id`, if one is open in this window.
    /// Continuing a session lands on its terminal: the sidebar goes back to the workspaces, and
    /// the conversation the session list was showing closes behind it.
    fn show_resumed_terminal(&mut self, cx: &mut Context<Self>) {
        self.panel = SidePanel::Workspaces;
        self.session_viewer = None;
        cx.notify();
    }

    pub(super) fn jump_to_session(&mut self, session_id: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let found = self.all_panes().into_iter().find(|pane| {
            let view = pane.read(cx);
            view.session_id_live.as_deref() == Some(session_id) || view.spec.session_id.as_deref() == Some(session_id)
        });
        let Some(pane) = found else { return false };
        let pane_id = pane.read(cx).pane_id;
        self.jump_to_pane_id(pane_id, window, cx)
    }

    /// Opens the system colour panel and follows it: whatever it shows becomes the colour of
    /// `target` until the panel is closed.
    pub(super) fn pick_custom_color(&mut self, target: chrome::ColorTarget, cx: &mut Context<Self>) {
        let current = match target {
            chrome::ColorTarget::Workspace(id) => self.workspaces.iter().find(|w| w.id == id).and_then(|w| w.color),
            chrome::ColorTarget::Group(id) => self.groups.iter().find(|g| g.id == id).and_then(|g| g.color),
        };
        crate::native::open_color_panel(current.unwrap_or(ACCENTS[0]));
        self.color_picking = Some(target);
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(std::time::Duration::from_millis(120)).await;
                let keep_going = this
                    .update(cx, |this, cx| {
                        // Another target took the panel over: this loop is the old one and leaves
                        // quietly. Clearing the flag here would stop the new loop as well.
                        if this.color_picking != Some(target) {
                            return false;
                        }
                        if !crate::native::color_panel_visible() {
                            this.color_picking = None;
                            return false;
                        }
                        if let Some(color) = crate::native::color_panel_color() {
                            this.apply_color(target, Some(color), cx);
                        }
                        true
                    })
                    .unwrap_or(false);
                if !keep_going {
                    break;
                }
            }
            // The colour the panel was left on is the one that is kept.
            let _ = this.update(cx, |this, cx| this.persist(cx));
        })
        .detach();
        cx.notify();
    }

    /// Paints `target` with `color` (`None` clears it) and saves the layout.
    pub(super) fn set_color(&mut self, target: chrome::ColorTarget, color: Option<u32>, cx: &mut Context<Self>) {
        if self.apply_color(target, color, cx) {
            self.persist(cx);
        }
    }

    /// Colours the target without saving, for the live preview while the colour panel is open:
    /// following a slider would otherwise write the whole layout to disk several times a second.
    /// Returns whether anything changed.
    fn apply_color(&mut self, target: chrome::ColorTarget, color: Option<u32>, cx: &mut Context<Self>) -> bool {
        let changed = match target {
            chrome::ColorTarget::Workspace(id) => match self.workspaces.iter_mut().find(|w| w.id == id) {
                Some(ws) if ws.color != color => {
                    ws.color = color;
                    true
                }
                _ => false,
            },
            chrome::ColorTarget::Group(id) => match self.groups.iter_mut().find(|g| g.id == id) {
                Some(group) if group.color != color => {
                    group.color = color;
                    true
                }
                _ => false,
            },
        };
        if changed {
            cx.notify();
        }
        changed
    }

    /// Records every tab whose panes are all in `closing` as a recently closed tab of its
    /// workspace, with the layout of its splits.
    pub(super) fn remember_closed_tabs(&mut self, closing: &[Pane], cx: &gpui::App) {
        let mut history: Vec<(usize, TabSnapshot)> = Vec::new();
        for (w, ws) in self.workspaces.iter().enumerate() {
            for tab in &ws.tabs {
                let leaves = tab.root.leaves();
                if leaves.iter().all(|leaf| closing.contains(leaf)) {
                    history.push((w, self.snapshot_tab(tab, cx)));
                }
            }
        }
        for (w, snapshot) in history {
            let closed = &mut self.workspaces[w].closed_tabs;
            closed.insert(0, snapshot);
            closed.truncate(CLOSED_TAB_HISTORY);
        }
    }

    /// Opens a tab that was closed here again, splits and all.
    pub(super) fn reopen_closed_tab(&mut self, workspace: u64, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(w) = self.workspaces.iter().position(|ws| ws.id == workspace) else { return };
        if index >= self.workspaces[w].closed_tabs.len() {
            return;
        }
        self.wake_for_new_tab(w, cx);
        let snapshot = self.workspaces[w].closed_tabs.remove(index);
        let Some(tree) = snapshot.layout.to_tree() else { return };
        let root = tree.map(&mut |pane: &PaneSnapshot| self.spawn_pane(pane.launch_spec(), cx));
        let leaves = root.leaves();
        let active = leaves.get(snapshot.active_pane).unwrap_or(&leaves[0]).clone();
        let ws = &mut self.workspaces[w];
        ws.tabs.push(Tab { root, active, instance: snapshot.instance.clone() });
        ws.active_tab = ws.tabs.len() - 1;
        self.split_shared_instances(w);
        self.active_workspace = w;
        self.page = None;
        self.persist(cx);
        self.focus_active(window, cx);
        cx.notify();
    }

    /// The pane running `session_id` in this window, if any.
    pub(super) fn pane_for_session(&self, session_id: &str, cx: &gpui::App) -> Option<Pane> {
        self.all_panes().into_iter().find(|pane| {
            let view = pane.read(cx);
            view.session_id_live.as_deref() == Some(session_id) || view.spec.session_id.as_deref() == Some(session_id)
        })
    }

    pub(super) fn ask_migrate_session(&mut self, session: SessionInfo, cx: &mut Context<Self>) {
        let to = other_agent(session.agent);
        let title = tf(
            cx,
            "sessions.migrate_confirm",
            &[("from", session.agent.display_name()), ("to", to.display_name()), ("name", &session.title)],
        );
        self.ask(
            ask::Ask {
                title: title.into(),
                body: Some(t(cx, "sessions.migrate_body").into()),
                choices: vec![ask::AskChoice {
                    label: t(cx, "sessions.migrate_run").into(),
                    action: ask::AskAction::Migrate(session),
                    primary: true,
                    danger: false,
                }],
                cancel: true,
            },
            cx,
        );
    }

    pub(super) fn ask_delete_session(&mut self, session: SessionInfo, cx: &mut Context<Self>) {
        let title = tf(cx, "sessions.delete_confirm", &[("name", &session.title)]);
        self.ask(
            ask::Ask {
                title: title.into(),
                body: Some(t(cx, "sessions.delete_body").into()),
                choices: vec![ask::AskChoice {
                    label: t(cx, "sessions.delete").into(),
                    action: ask::AskAction::Delete(session),
                    primary: false,
                    danger: true,
                }],
                cancel: true,
            },
            cx,
        );
    }

    /// Deletes a session's transcript and takes it out of the list. A session open in a pane is
    /// left alone: its agent is still writing to that file.
    pub(super) fn delete_session(&mut self, session: SessionInfo, cx: &mut Context<Self>) {
        if self.pane_for_session(&session.id, cx).is_some() {
            let message = t(cx, "sessions.delete_open");
            return self.show_toast(message, cx);
        }
        match agentty_bridge::delete(&session) {
            Ok(()) => {
                self.sessions.retain(|s| s.path != session.path);
                if self.session_viewer.as_ref().is_some_and(|v| v.session.path == session.path) {
                    self.session_viewer = None;
                }
                let message = tf(cx, "sessions.deleted", &[("name", &session.title)]);
                self.show_toast(message, cx);
            }
            Err(err) => self.show_toast(format!("{err:#}"), cx),
        }
        cx.notify();
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
        self.groups.push(Group { id, name, collapsed: false, color: None });
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
        // The agent's own mark ("✳ Claude Code") is dropped wherever the name came from: a card
        // shows the logo beside it, and the two together read as one icon too many.
        if let Some(name) = &ws.name {
            return crate::terminal::strip_agent_mark(name).to_string();
        }
        if let Some(tab) = ws.tabs.get(ws.active_tab) {
            return tab.active.read(cx).display_title();
        }
        ws.dormant
            .as_ref()
            .and_then(|s| s.tabs.first())
            .and_then(|t| first_pane(&t.layout))
            .map(|p| crate::terminal::strip_agent_mark(&p.title).to_string())
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
            TextInputEvent::Confirmed => {
                let value = input.read(cx).text().trim().to_string();
                this.finish_rename(Some(value), window, cx);
            }
            TextInputEvent::Cancelled => this.finish_rename(None, window, cx),
            // Losing focus is not an answer: the Emoji & Symbols palette takes it, and the dialog
            // closing under it sent what was picked to the terminal instead of the field.
            _ => {}
        });
        window.focus(&input.focus_handle(cx));
        self.workspace_menu = None;
        self.rename = Some(Rename { target, input, _subscription: subscription });
        cx.notify();
    }

    /// The rename dialog. A modal rather than an inline field: dragging inside the card used to
    /// start a drag and drop, and the row's selection colour swallowed what was being typed.
    pub(super) fn render_rename_dialog(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let rename = self.rename.as_ref()?;
        let title = match rename.target {
            RenameTarget::Workspace(_) => t(cx, "rename.workspace"),
            RenameTarget::Group(_) => t(cx, "rename.group"),
        };
        Some(
            div()
                .id("rename-overlay")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::hsla(0., 0., 0., 0.45))
                .occlude()
                // A click outside does nothing: losing a half-typed name to a stray click is the
                // thing this dialog was made to stop. Save and Cancel are the way out.
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .id("rename-dialog")
                        .w(px(380.))
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
                        .child(div().t_title().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(title))
                        .child(
                            div()
                                .px_2()
                                .py_1p5()
                                .rounded_md()
                                .bg(hex(Chrome::PANEL))
                                .border_1()
                                .border_color(hex(Chrome::BORDER))
                                .t_body()
                                .text_color(hex(Chrome::BRIGHT))
                                .child(rename.input.clone()),
                        )
                        .child(div().t_caption().text_color(hex(Chrome::MUTED)).child(t(cx, "rename.hint")))
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .child(
                                    div()
                                        .id("rename-cancel")
                                        .px_3()
                                        .py_1p5()
                                        .rounded_md()
                                        .t_body()
                                        .cursor_pointer()
                                        .bg(hex(0x2d2d30))
                                        .text_color(hex(Chrome::BRIGHT))
                                        .hover(|s| s.opacity(0.85))
                                        .on_click(
                                            cx.listener(|this, _: &gpui::ClickEvent, window, cx| this.finish_rename(None, window, cx)),
                                        )
                                        .child(t(cx, "confirm.cancel")),
                                )
                                .child(
                                    div()
                                        .id("rename-save")
                                        .px_3()
                                        .py_1p5()
                                        .rounded_md()
                                        .t_body()
                                        .cursor_pointer()
                                        .bg(hex(Chrome::ACCENT))
                                        .text_color(hex(Chrome::BRIGHT))
                                        .hover(|s| s.opacity(0.85))
                                        .on_click(cx.listener(|this, _: &gpui::ClickEvent, window, cx| {
                                            let value = this.rename.as_ref().map(|r| r.input.read(cx).text().trim().to_string());
                                            this.finish_rename(value, window, cx);
                                        }))
                                        .child(t(cx, "save")),
                                ),
                        ),
                ),
        )
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
        let running = view.agent_kind().filter(|kind| *kind != PaneKind::Shell);
        let (kind, session_id) =
            persist::saved_session(view.spec.kind, running, view.session_id_live.clone(), view.spec.session_id.clone());
        let session_id = match kind {
            PaneKind::Codex => session_id.or_else(|| agentty_bridge::codex::find_recent(&view.spec.cwd, view.launched_at_ms)),
            _ => session_id,
        };
        PaneSnapshot {
            kind,
            // A session resumes only in the folder it was held in: the agent's own, not the shell's.
            cwd: match (&view.spec.missing_cwd, if running.is_some() { view.display_cwd() } else { view.current_dir() }) {
                // Still where it was put for want of its own folder: that folder is what to keep.
                (Some(missing), here) if here == view.spec.cwd => missing.clone(),
                (_, here) => here,
            },
            title: view.display_title(),
            session_id,
            tool: Some(view.tool_id().to_string()),
        }
    }

    fn snapshot_tab(&self, tab: &Tab, cx: &gpui::App) -> TabSnapshot {
        TabSnapshot {
            instance: tab.instance.clone(),
            layout: NodeSnapshot::from_tree(&tab.root.map(&mut |pane| Self::snapshot_pane(pane, cx))),
            active_pane: tab.root.leaves().iter().position(|p| *p == tab.active).unwrap_or(0),
            zoomed_pane: self.zoomed.as_ref().and_then(|zoomed| tab.root.leaves().iter().position(|p| p == zoomed)),
        }
    }

    /// What is open around the terminals, as saved with the layout.
    fn panel_state(&self) -> persist::PanelState {
        let browser = self.browser.as_ref();
        persist::PanelState {
            // What the user chose, not what a plugin's workspace folded away for the moment.
            sidebar_hidden: !self.plugin_workspace_sidebar.unwrap_or(self.sidebar_open),
            files_open: self.files_panel.is_some(),
            files_pinned: self.files_panel.as_ref().and_then(|panel| panel.pinned.clone()),
            // Written to disk: an address carrying a credential keeps only its origin.
            browser_tabs: browser
                .map(|b| {
                    b.tab_urls().iter().map(|url| agentty_bridge::extensions::url_to_keep(url)).filter(|url| !url.is_empty()).collect()
                })
                .unwrap_or_default(),
            browser_active: browser.map_or(0, |b| b.active),
            plugin: self.plugin_panel.clone(),
            docker_open: self.docker.open,
            database_open: self.db.panel_open,
        }
    }

    /// Opens what was open around the terminals at the last save. Nothing here is counted as
    /// a use of the feature: it is the same session coming back.
    fn restore_panels(&mut self, panels: persist::PanelState, cx: &mut Context<Self>) {
        // A plugin's workspace in front keeps the list folded; it opens as saved on leaving.
        match self.plugin_workspace_sidebar.as_mut() {
            Some(saved) => *saved = !panels.sidebar_hidden,
            None => self.sidebar_open = !panels.sidebar_hidden,
        }
        if panels.files_open {
            self.show_files_panel(panels.files_pinned.filter(|p| p.is_dir()), cx);
        }
        if !panels.browser_tabs.is_empty() && crate::platform::HAS_WEBVIEW {
            let active = panels.browser_active.min(panels.browser_tabs.len() - 1);
            self.browser_request = Some(panels.browser_tabs[active].clone());
            self.browser_restore = Some((panels.browser_tabs, active));
        }
        // A workspace plugin's panel comes back with its workspace, not by itself.
        if let Some(plugin) =
            panels.plugin.filter(|id| crate::plugins::plugin(cx, id).is_some_and(|p| p.enabled) && !self.wants_workspace(id, cx))
        {
            self.open_plugin_panel(&plugin, cx);
        }
        if panels.docker_open && !self.docker.open {
            self.toggle_docker_panel(cx);
        }
        if panels.database_open && !self.db.panel_open {
            self.toggle_db_panel(cx);
        }
        self.saved_panels = self.panel_state();
    }

    /// Saves the layout in a moment, not now. A save reads every pane's folder and writes the
    /// whole layout, and the folder a pane works in changes on its own: a build script or an agent
    /// hopping directories would otherwise write the file over and over while it runs. Presses of
    /// a button still save straight away — this is only for what the terminal does by itself. What
    /// is waiting is never lost on a clean exit: closing the window and quitting both save then.
    fn persist_soon(&mut self, cx: &mut Context<Self>) {
        if self.persist_pending {
            return;
        }
        self.persist_pending = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_secs(1)).await;
            let _ = this.update(cx, |this, cx| {
                this.persist_pending = false;
                this.persist(cx);
            });
        })
        .detach();
    }

    fn persist(&self, cx: &gpui::App) {
        let workspaces = self
            .workspaces
            .iter()
            .map(|ws| {
                if let Some(dormant) = &ws.dormant {
                    // Live tabs beside a snapshot are saved with it, never dropped.
                    let mut tabs = if ws.asleep_on_close && !ws.tabs.is_empty() { Vec::new() } else { dormant.tabs.clone() };
                    tabs.extend(ws.tabs.iter().map(|tab| self.snapshot_tab(tab, cx)));
                    return WorkspaceSnapshot {
                        name: ws.name.clone(),
                        group: ws.group,
                        closed_tabs: ws.closed_tabs.clone(),
                        color: None,
                        color_value: ws.color,
                        tabs,
                        ..dormant.clone()
                    };
                }
                let panes: Vec<Pane> = ws.tabs.iter().flat_map(|t| t.root.leaves()).collect();
                WorkspaceSnapshot {
                    id: ws.id,
                    name: ws.name.clone(),
                    group: ws.group,
                    cwd: ws.cwd.clone(),
                    active_tab: ws.active_tab,
                    closed_tabs: ws.closed_tabs.clone(),
                    color: None,
                    color_value: ws.color,
                    plugin: ws.plugin.clone(),
                    tabs: ws.tabs.iter().map(|tab| self.snapshot_tab(tab, cx)).collect(),
                    // What the card says while the workspace is open, so it says the same once it
                    // is folded away: the branch it is on and when it last did something.
                    branch: ws
                        .tabs
                        .get(ws.active_tab)
                        .map(|t| t.active.clone())
                        .or_else(|| panes.first().cloned())
                        .and_then(|p| p.read(cx).git_branch.clone()),
                    last_activity_ms: panes.iter().map(|p| p.read(cx).last_activity_ms).max(),
                }
            })
            .collect();
        let state = LayoutState {
            groups: self
                .groups
                .iter()
                .map(|g| GroupSnapshot { id: g.id, name: g.name.clone(), collapsed: g.collapsed, color: None, color_value: g.color })
                .collect(),
            workspaces,
            active_workspace: self.active_workspace,
            ungrouped_collapsed: self.ungrouped_collapsed,
            window: self.window_state,
            panels: self.panel_state(),
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
        self.groups = state
            .groups
            .iter()
            .map(|g| Group { id: g.id, name: g.name.clone(), collapsed: g.collapsed, color: stored_color(g.color_value, g.color) })
            .collect();
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
                closed_tabs: snapshot.closed_tabs.clone(),
                color: stored_color(snapshot.color_value, snapshot.color),
                plugin: snapshot.plugin.clone(),
                dormant: Some(snapshot),
                asleep_on_close: false,
            });
        }
        for group in &self.groups {
            self.next_id = self.next_id.max(group.id + 1);
        }
        if !self.workspaces.is_empty() {
            // Only the active workspace starts processes; the rest wake up when opened.
            self.activate_workspace(state.active_workspace.min(self.workspaces.len() - 1), window, cx);
        }
        self.restore_panels(state.panels, cx);
    }

    fn revive(&mut self, index: usize, snapshot: WorkspaceSnapshot, cx: &mut Context<Self>) {
        let mut tabs = Vec::new();
        for tab in &snapshot.tabs {
            let Some(tree) = tab.layout.to_tree() else { continue };
            let root = tree.map(&mut |pane: &PaneSnapshot| self.spawn_pane(pane.launch_spec(), cx));
            let leaves = root.leaves();
            let active = leaves.get(tab.active_pane).unwrap_or(&leaves[0]).clone();
            if self.zoomed.is_none() {
                self.zoomed = tab.zoomed_pane.and_then(|index| leaves.get(index)).cloned();
            }
            tabs.push(Tab { root, active, instance: tab.instance.clone() });
        }
        let ws = &mut self.workspaces[index];
        ws.active_tab = snapshot.active_tab.min(tabs.len().saturating_sub(1));
        ws.asleep_on_close = false;
        // Tabs that are already live stay, after the ones brought back.
        tabs.append(&mut ws.tabs);
        ws.tabs = tabs;
        if ws.tabs.is_empty() {
            let pane = self.spawn_pane(LaunchSpec::new(PaneKind::Shell, snapshot.cwd.clone()), cx);
            let ws = &mut self.workspaces[index];
            ws.tabs.push(Tab { root: PaneNode::Leaf(pane.clone()), active: pane, instance: None });
        }
        self.split_shared_instances(index);
    }

    /// Every tab is an automation of its own: a tab whose id another tab already has (a layout
    /// saved before ids were checked, or a closed tab reopened) gets a new one.
    fn split_shared_instances(&mut self, index: usize) {
        let mut seen = std::collections::HashSet::new();
        let shared: Vec<usize> = self.workspaces[index]
            .tabs
            .iter()
            .enumerate()
            .filter_map(|(t, tab)| tab.instance.as_ref().filter(|i| !seen.insert(i.id.clone())).map(|_| t))
            .collect();
        for t in shared {
            let fresh = self.new_instance();
            self.workspaces[index].tabs[t].instance = Some(fresh);
        }
    }

    // -- mouse drags shared by the whole window --------------------------------------------------

    fn on_root_mouse_move(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.pressed_button != Some(MouseButton::Left) {
            if self.sidebar_resizing
                || self.browser_resizing
                || self.files_resizing
                || self.side_resizing.is_some()
                || self.files_trees_drag.is_some()
                || self.browser_net_drag.is_some()
                || self.split_drag.is_some()
                || self.plugin_ws_drag.is_some()
                || self.flow.is_dragging()
            {
                self.end_drags(cx);
            }
            return;
        }
        if let Some(drag) = self.plugin_ws_drag {
            let viewport = window.viewport_size();
            return self.drag_plugin_workspace(drag, f32::from(event.position.x), f32::from(event.position.y), viewport, cx);
        }
        if let Some((start_y, start_height)) = self.browser_net_drag {
            // Dragging up makes the network panel taller; the page above keeps a readable height.
            let viewport = f32::from(window.viewport_size().height);
            let height = (start_height + start_y - f32::from(event.position.y)).clamp(90., (viewport - 260.).max(90.));
            gpui::BorrowAppContext::update_global::<crate::settings::SettingsStore, _>(cx, |store, _| {
                store.settings.browser.network_height = height
            });
            cx.notify();
        } else if let Some((start_y, start_height)) = self.files_trees_drag {
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
        } else if let Some(panel) = self.side_resizing {
            let viewport = f32::from(window.viewport_size().width);
            self.drag_side_panel(panel, f32::from(event.position.x), viewport, window, cx);
        } else if self.browser_resizing {
            // The splitter sits just left of the panel; the plugin and files panels may sit right of it.
            let shown = self.docked_widths(cx).1;
            let right = self.side_panels_total(cx) + self.files_panel.as_ref().map_or(0., |_| shown + 5.);
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
        if self.sidebar_resizing
            || self.browser_resizing
            || self.files_resizing
            || self.side_resizing.is_some()
            || self.files_trees_drag.is_some()
            || self.browser_net_drag.is_some()
            || self.plugin_ws_drag.is_some()
        {
            self.plugin_ws_drag = None;
            self.sidebar_resizing = false;
            self.browser_resizing = false;
            self.files_resizing = false;
            self.side_resizing = None;
            self.files_trees_drag = None;
            self.browser_net_drag = None;
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
        let activated = window.is_window_active() && !self.window_active;
        self.window_active = window.is_window_active();
        if activated {
            self.docker_window_activated();
        }
        self.viewport_width = f32::from(window.viewport_size().width);
        // Opening or closing a panel or the sidebar is part of the layout. The toggles are many
        // (buttons, menus, shortcuts, agents); watching the outcome here catches them all.
        let panels = self.panel_state();
        if panels != self.saved_panels {
            self.saved_panels = panels;
            self.persist_soon(cx);
        }
        if let Some(git) = self.git.clone().filter(|_| self.page != Some(Page::Git)) {
            git.update(cx, |v, _| v.set_visible(false));
        }
        self.open_pending_file(window, cx);
        self.prepare_browser(window, cx);
        self.prepare_plugin_panel(window, cx);
        self.prepare_plugin_consent(window, cx);
        self.prepare_files_panel(cx);
        self.prepare_docker(cx);
        self.prepare_db(window, cx);
        self.advance_tour(cx);
        self.prepare_plugins_page(window, cx);
        self.prepare_style_preview(cx);
        self.broadcast_plugin_context(window, cx);
        self.broadcast_pane_status(cx);
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
            Some(Page::Worktrees) => self.render_tree_manager(cx).into_any_element(),
            Some(Page::Disk) => self.render_disk_page(cx).into_any_element(),
            Some(Page::Settings) => self.render_settings(window, cx).into_any_element(),
            Some(Page::Extensions) => gpui::AnyView::from(self.extensions_view(window, cx))
                .cached(gpui::StyleRefinement::default().size_full())
                .into_any_element(),
            Some(Page::Git) => {
                gpui::AnyView::from(self.git_view(window, cx)).cached(gpui::StyleRefinement::default().size_full()).into_any_element()
            }
            Some(Page::Flow) => self.render_flow(window, cx).into_any_element(),
            Some(Page::Plugins) => self.render_plugins_page(cx).into_any_element(),
            Some(Page::Database) => self.render_db_page(cx),
            Some(Page::Idea) => {
                gpui::AnyView::from(self.idea_view(window, cx)).cached(gpui::StyleRefinement::default().size_full()).into_any_element()
            }
            None if self.editor_visible(cx) => self.render_editor(cx).expect("editor is visible"),
            None => {
                match (self.render_session_viewer(cx), self.workspaces.get(self.active_workspace).and_then(|ws| ws.tabs.get(ws.active_tab)))
                {
                    (Some(viewer), _) => viewer,
                    (None, Some(tab)) if self.new_workspace.is_none() && !self.welcome => self.render_tab(tab, cx),
                    (None, _) => self.render_welcome(cx).into_any_element(),
                }
            }
        };
        let mut main = Some(main);
        // A plugin's own workspace in front, drawn in its own layout.
        let plugin_workspace = self.front_plugin_workspace(cx).filter(|_| self.page.is_none());

        div()
            .id("workbench")
            .key_context("Workbench")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &NewTerminalTab, window, cx| {
                // In a plugin's workspace a new tab is a new automation.
                match this.front_plugin_workspace(cx).filter(|_| this.page.is_none()) {
                    Some(plugin) => this.new_plugin_instance(&plugin, cx),
                    None => this.request_launch(PaneKind::Shell, LaunchTarget::NewTab, window, cx),
                }
            }))
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
                if this.close_active_file(cx) {
                    return;
                }
                if let Some(ws) = this.workspaces.get(this.active_workspace) {
                    let index = ws.active_tab;
                    this.request_close_tabs(&[index], window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ClosePane, window, cx| {
                // ⌘W in the browser's address bar closes that page, not a terminal.
                if this.browser_address_focused(window, cx) {
                    let index = this.browser.as_ref().map(|b| b.active).unwrap_or_default();
                    return this.close_browser_tab(index, window, cx);
                }
                if this.page.take().is_some() {
                    this.focus_active(window, cx);
                    return cx.notify();
                }
                if this.close_active_file(cx) {
                    return;
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
            // The Edit menu's ⌘C / ⌘V / ⌘A land here when no terminal is focused; a page in the
            // in-app browser still needs them, since a menu key equivalent never reaches it.
            .on_action(cx.listener(|_, _: &crate::terminal::Copy, _, _| {
                crate::webview::perform_in_page(crate::webview::EditCommand::Copy);
            }))
            .on_action(cx.listener(|_, _: &crate::terminal::Paste, _, _| {
                crate::webview::perform_in_page(crate::webview::EditCommand::Paste);
            }))
            .on_action(cx.listener(|_, _: &crate::terminal::SelectAll, _, _| {
                crate::webview::perform_in_page(crate::webview::EditCommand::SelectAll);
            }))
            .on_action(cx.listener(|this, _: &ToggleBrowser, window, cx| this.toggle_browser(window, cx)))
            // ⌘⇧R inside the page is caught by the web view itself; here it works from the terminals too.
            .on_action(cx.listener(|this, _: &HardReloadBrowser, _, cx| {
                if this.browser.is_some() {
                    this.reload_browser(true, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleFiles, _, cx| this.toggle_files_panel(cx)))
            .on_action(cx.listener(|this, _: &FindInTerminal, window, cx| {
                // The terminal behind the editor is not on screen to search.
                if !this.editor_visible(cx) {
                    this.open_find(window, cx)
                }
            }))
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
            .on_action(|_: &OpenPrivacyPolicy, _, cx| cx.open_url(update::PRIVACY_URL))
            .on_action(|_: &OpenTerms, _, cx| cx.open_url(update::TERMS_URL))
            .on_action(|_: &OpenEula, _, cx| cx.open_url(update::EULA_URL))
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
                        || this.side_resizing.is_some()
                        || this.files_trees_drag.is_some()
                        || this.browser_net_drag.is_some()
                        || this.split_drag.is_some()
                        || this.plugin_ws_drag.is_some()
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
                            // A plugin's workspace has a layout of its own (its tabs sit with its terminals).
                            .when(plugin_workspace.is_none(), |d| d.child(self.render_tab_strip(cx)))
                            .children(self.render_service_banner(cx))
                            .when_some(plugin_workspace.clone(), |d, plugin| {
                                d.child(self.render_plugin_workspace(&plugin, main.take(), cx))
                            })
                            .when(plugin_workspace.is_none(), |d| {
                                d.child(
                                    div()
                                        .flex_1()
                                        .min_h_0()
                                        .flex()
                                        // A floating or full-area panel is drawn over this row.
                                        .relative()
                                        .child(div().flex_1().min_w_0().h_full().children(main.take()))
                                        .when(self.page.is_none(), |d| {
                                            let docked = self.plugin_panel_width(cx) > 0.;
                                            d.children(self.render_browser_splitter(cx))
                                                .children(self.render_browser(cx))
                                                .when(docked, |d| d.child(self.render_side_splitter(side_panels::SidePanel::Plugin, cx)))
                                                .children(self.render_plugin_panel(cx))
                                                .when(self.docker.open, |d| {
                                                    d.child(self.render_side_splitter(side_panels::SidePanel::Docker, cx))
                                                })
                                                .children(self.render_docker_panel(cx))
                                                .when(self.db.panel_open, |d| {
                                                    d.child(self.render_side_splitter(side_panels::SidePanel::Database, cx))
                                                })
                                                .children(self.render_db_panel(cx))
                                                .children(self.render_files_splitter(cx))
                                                .children(self.render_files_panel(cx))
                                                // Over everything on this row, whatever else is docked.
                                                .children(self.render_plugin_overlay(cx))
                                        }),
                                )
                            })
                            .when(self.launcher_open, |d| d.child(self.render_launcher(cx)))
                            // Opens under the bell, at the right end of the title bar.
                            .when(self.notices_open, |d| {
                                d.child(
                                    div()
                                        .absolute()
                                        .top(px(4.))
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
            .children(self.render_tree_menu(cx))
            .when(self.updates.popup, |d| d.child(self.render_update_popup(window, cx)))
            .when(self.about_open, |d| d.child(self.render_about_dialog(cx)))
            .children(self.render_connect_pick_bar(cx))
            .children(self.render_install_hint(cx))
            .children(self.render_close_confirm(cx))
            .children(self.render_tree_remove_confirm(cx))
            .children(self.render_ask(cx))
            .children(self.render_rename_dialog(cx))
            .children(self.render_prompt_dialog(cx))
            .children(self.render_tasks_dialog(cx))
            .children(self.render_db_approval(cx))
            .children(self.render_harness_dialog(cx))
            .children(self.render_onboarding(cx))
            .children(self.render_waiting_banner(cx))
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
        self.hide_editor();
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
        self.show_toast_for(text, DEFAULT_TOAST_MS, cx);
    }

    /// A toast that stays for `millis` (something worth reading, not just a confirmation).
    pub(super) fn show_toast_for(&mut self, text: impl Into<SharedString>, millis: u64, cx: &mut Context<Self>) {
        self.toast_seq += 1;
        let id = self.toast_seq;
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

    /// How far from the right edge an overlay must stay: left of the panels docked at the right.
    /// The browser is a native view, and anything drawn under it would never be seen.
    pub(super) fn overlay_right_inset(&self, cx: &gpui::App) -> f32 {
        if self.page.is_some() {
            return 0.;
        }
        let (browser, files) = self.docked_widths(cx);
        self.browser.as_ref().map_or(0., |_| browser + 5.)
            + self.side_panels_total(cx)
            + self.files_panel.as_ref().map_or(0., |_| files + 5.)
    }

    fn render_toast(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let (text, id) = self.toast.clone()?;
        let docked = self.overlay_right_inset(cx);
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
                    .child(text)
                    // Closes it now instead of waiting out its timer.
                    .child(
                        div()
                            .id("toast-close")
                            .px_1()
                            .rounded_sm()
                            .cursor_pointer()
                            .hover(|s| s.bg(hex(Chrome::HOVER)))
                            .child(crate::ui::icon("x", crate::ui::IconSize::INLINE, hex(Chrome::MUTED)))
                            .on_click(cx.listener(|this, _: &gpui::ClickEvent, _, cx| {
                                this.toast = None;
                                cx.notify();
                            })),
                    ),
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
            Page::Worktrees => "worktrees",
            Page::Disk => "disk",
            Page::Settings => "settings",
            Page::Extensions => "extensions",
            Page::Plugins => "plugins",
            Page::Idea => "idea",
            Page::Database => "database",
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

    /// Opens the extensions page on one category (the Skills / Agents / MCP page tabs).
    pub(super) fn open_extensions(&mut self, category: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        self.page = Some(Page::Extensions);
        let view = self.extensions_view(window, cx);
        view.update(cx, |view, cx| view.show_category(category, cx));
        cx.notify();
    }

    /// The category the extensions page is on, for the tab that is drawn as active.
    pub(super) fn extensions_category(&self, cx: &gpui::App) -> &'static str {
        self.extensions.as_ref().map_or("all", |view| view.read(cx).category_id())
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

/// Every pane of a saved tab, left to right.
fn snapshot_panes(node: &NodeSnapshot) -> Vec<&PaneSnapshot> {
    match node {
        NodeSnapshot::Pane(p) => vec![p],
        NodeSnapshot::Split { children, .. } => children.iter().flat_map(snapshot_panes).collect(),
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
                        "browserTabs": self.browser.as_ref().map(|b| b.debug_state()),
                        "files": self.files_panel.as_ref().map(|p| p.debug_state()),
                        "editor": self.editor_debug_state(cx),
                        "docker": self.docker.debug_state(),
                        "db": self.db.debug_state(),
                        "chatNotify": self.chat_notify.debug_state(),
                        "capture": { "recording": crate::capture::is_recording(), "port": crate::capture::port(), "records": records },
                        "toast": self.toast.as_ref().map(|(text, _)| text.to_string()),
                        "plugins": {
                            "panel": self.plugin_panel,
                            // A panel in `window` mode has one; nothing else should.
                            "windows": self.plugin_windows.keys().cloned().collect::<Vec<String>>(),
                            "opening": self.plugin_windows_opening.iter().cloned().collect::<Vec<String>>(),
                            "closing": self.plugin_windows_closing.iter().cloned().collect::<Vec<String>>(),
                            "mode": self.plugin_panel.as_ref().map(|p| self.plugin_panel_mode(p, cx).id()),
                            "installed": crate::plugins::host(cx)
                                .installed
                                .iter()
                                .map(|p| serde_json::json!({ "id": p.id, "enabled": p.enabled, "active": p.active() }))
                                .collect::<Vec<_>>(),
                        },
                    })
                );
            }
            "capture" => match argument {
                "stop" => crate::capture::stop(),
                _ => {
                    let _ = crate::capture::start();
                }
            },
            "edit" => self.debug_editor("edit", argument, window, cx),
            "editor" => {
                let (command, argument) = argument.split_once(' ').unwrap_or((argument, ""));
                self.debug_editor(command, argument, window, cx);
            }
            "tree-menu" => self.debug_tree_menu(argument.parse().unwrap_or(0), cx),
            "tree-remove" => self.debug_tree_remove(argument, cx),
            "docker" => self.debug_docker(argument, window, cx),
            "db" => self.debug_db(argument, window, cx),
            "chat-notify" => self.debug_chat_notify(argument, cx),
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
                    "worktrees" => Some(Page::Worktrees),
                    "disk" => Some(Page::Disk),
                    "settings" => Some(Page::Settings),
                    "extensions" => Some(Page::Extensions),
                    "plugins" => Some(Page::Plugins),
                    "idea" => Some(Page::Idea),
                    "db" => Some(Page::Database),
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
            // `mini` folds the window away; `mini peek [pane id]` opens a terminal beside the panel.
            "mini" => match argument.split_whitespace().next() {
                Some("peek") => {
                    let id = argument.split_whitespace().nth(1).and_then(|id| id.parse().ok());
                    let pane = id.or_else(|| self.all_panes().first().map(|p| p.read(cx).pane_id));
                    if let Some(pane) = pane {
                        self.open_mini_peek(pane, cx);
                    }
                }
                _ => self.toggle_mini(window, cx),
            },
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
                        "update check failed: https://api.github.com/repos/empty-user77/Agentty/releases/latest: status code 403".into(),
                    );
                    self.updates.popup = true;
                    cx.notify();
                }
                "fake" => {
                    self.updates.state = update::UpdateState::Available(agentty_bridge::update::Release {
                        version: "0.2.0".into(),
                        notes: "- Faster startup\n- Git page".into(),
                        page_url: "https://github.com/empty-user77/Agentty/releases".into(),
                        installer_url: None,
                        installer_name: None,
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
            "workspace-search" => {
                self.focus_workspace_search(window, cx);
                self.workspace_search.update(cx, |i, cx| i.set_text(argument.to_string(), cx));
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
            // What Recheck on Settings → System check does; prints what was found once it is done.
            "system-check" => {
                self.run_system_check(false, cx);
                cx.spawn(async move |this, cx| {
                    while this.update(cx, |this, _| this.system_checking).unwrap_or(false) {
                        cx.background_executor().timer(std::time::Duration::from_millis(100)).await;
                    }
                    let _ = this.update(cx, |this, _| {
                        for tool in this.system_check.iter().flatten() {
                            eprintln!("system-check: {} = {:?}", tool.id, tool.found);
                        }
                        eprintln!("system-check: tour open = {}", this.onboarding.is_some());
                    });
                })
                .detach();
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
                    "notifications" => settings_page::SettingsSection::Notifications,
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
            "browser-reload" => self.reload_browser(argument == "hard", cx),
            // `browser-js <body>`: runs an async function body in the tab in front, prints the result.
            "browser-js" => {
                let view = self.browser.as_ref().map(|browser| browser.webview());
                let ran = view.is_some_and(|view| match view.borrow().as_ref() {
                    Some(page) => {
                        page.call_async(argument, &[], Box::new(|result| eprintln!("browser-js: {result:?}")));
                        true
                    }
                    None => false,
                });
                if !ran {
                    eprintln!("browser-js: no page");
                }
            }
            // `browser-keeper remember|restore|forget|sites`: the sign-in keeper by hand.
            "browser-keeper" => match argument {
                "remember" => crate::browser_keeper::remember(cx),
                "restore" => crate::browser_keeper::restore(cx),
                "forget" => crate::browser_keeper::forget_all(),
                _ => {
                    for site in crate::browser_keeper::kept_sites(cx) {
                        let host = site.host.clone();
                        let refreshed = crate::browser_keeper::refreshed_at(&host);
                        plugin_browser::site_status(&site, move |status| {
                            eprintln!("browser-keeper: {host} status={status:?} refreshed={refreshed:?}")
                        });
                    }
                }
            },
            // `browser-refresh <host>`: a keep-alive visit to one kept site now.
            "browser-refresh" => {
                if let Some(site) = crate::browser_keeper::kept_sites(cx).into_iter().find(|site| site.host == argument) {
                    self.refresh_site(&site, window, cx);
                }
            }
            // `plugin-browsers`: the pages plugins drive.
            "plugin-browsers" => {
                for page in &self.plugin_browsers {
                    let borrowed = page.webview.borrow();
                    let view = borrowed.as_ref();
                    eprintln!(
                        "plugin-browsers: id={} plugin={} shown={} url={:?} loading={:?}",
                        page.id,
                        page.plugin,
                        page.shown,
                        view.and_then(crate::webview::WebView::current_url),
                        view.map(crate::webview::WebView::is_loading)
                    );
                }
                eprintln!("plugin-browsers: refreshes={}", self.site_refreshes.len());
            }
            // `browser-tab [url]`, `browser-tab close <index>`, `browser-tab select <index>`.
            "browser-tab" => match argument.split_once(' ') {
                Some(("close", index)) => self.close_browser_tab(index.parse().unwrap_or(0), window, cx),
                Some(("select", index)) => self.select_browser_tab(index.parse().unwrap_or(0), cx),
                _ if argument.is_empty() => self.open_browser_tab(None, cx),
                _ => self.open_browser_tab(Some(browser::browser_url(argument, cx)), cx),
            },
            // `browser-net` opens or closes the network panel; `browser-net <id>` opens one call.
            "browser-net" => self.debug_browser_network(argument, cx),
            // `browser-viewport 375x667|iphone-se|off`: responsive mode, as the toolbar sets it.
            "browser-viewport" => {
                let next = if argument == "off" { None } else { responsive::Viewport::parse(argument) };
                self.set_viewport(next, cx);
            }
            "link" => self.open_link(argument.to_string(), cx),
            // `agentty-link agentty://…`: as if another app opened the link.
            "agentty-link" => self.open_agentty_link(argument, window, cx),
            // `plugin-panel <id>` / `plugin-command <id> <command>`.
            "plugin-panel" => self.toggle_plugin(argument, window, cx),
            "plugin-command" => {
                if let Some((plugin, command)) = argument.split_once(' ') {
                    self.run_plugin_command(plugin, command, None, cx);
                }
            }
            "plugin-install" => self.install_builtin_plugin(argument.to_string(), window, cx),
            // `plugin-market refresh|install <id>|update-all|uninstall <id>`: the marketplace's
            // own buttons, so its paths can be driven without the mouse.
            "plugin-market" => self.debug_market(argument, window, cx),
            // `plugin-enable <plugin> on|off`: the switch on the Plugins page, which is also how a
            // panel (and a panel's own window) is meant to go away when its plugin does.
            // `plugin-consent <plugin> allow|deny`: the first-run permission question, answered as
            // the user would (the native dialog stays open; its answer then changes nothing).
            "plugin-consent" => {
                if let Some((plugin, answer)) = argument.split_once(' ') {
                    crate::plugins::answer_consent(plugin, answer.trim() == "allow", cx);
                    eprintln!("plugin-consent: {plugin} {answer}");
                }
            }
            "plugin-enable" => {
                if let Some((plugin, state)) = argument.split_once(' ') {
                    self.set_plugin_enabled_debug(plugin, state.trim() == "on", cx);
                }
            }
            // `plugin-mode <plugin> push|overlay|window|full`: how its panel opens.
            "plugin-mode" => {
                if let Some((plugin, mode)) = argument.split_once(' ') {
                    if let Some(mode) = agentty_bridge::plugins::manifest::PanelMode::from_id(mode.trim()) {
                        self.set_plugin_panel_mode(plugin, mode, window, cx);
                    }
                }
            }
            // `plugin-event <plugin> <element> <event> [value]`: what a click or a keystroke in a
            // plugin's panel sends, without the mouse. `plugin-event <plugin> <list> action
            // <item>/<action>` presses a button on a list row.
            "plugin-event" => {
                let mut parts = argument.splitn(4, ' ');
                if let (Some(plugin), Some(element), Some(event)) = (parts.next(), parts.next(), parts.next()) {
                    let rest = parts.next();
                    let row =
                        rest.filter(|_| event == "action" || event == "select").map(|rest| rest.split_once('/').unwrap_or((rest, "")));
                    let value = rest.filter(|_| row.is_none()).map(|value| serde_json::Value::String(value.to_string()));
                    let event = agentty_bridge::plugins::ui::UiEvent {
                        element: element.to_string(),
                        event: event.to_string(),
                        value,
                        item: row.map(|(item, _)| item.to_string()),
                        action: row.map(|(_, action)| action.to_string()).filter(|action| !action.is_empty()),
                    };
                    self.send_plugin_event(plugin, event, cx);
                }
            }
            // `plugin-folder <path>`: the "Install from folder" button without its file picker.
            "plugin-folder" => {
                let result = agentty_bridge::plugins::store::install_from_folder(std::path::Path::new(argument));
                eprintln!("plugin-folder: {:?}", result.as_ref().map(|p| p.id.clone()).map_err(|e| format!("{e:#}")));
                self.after_install_debug(result, window, cx);
            }
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
                        panel.chars().take(8000).collect::<String>(),
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
            // `activate`: the app and this window in front, so typed keys reach the field clicked.
            "activate" => {
                cx.activate(true);
                window.activate_window();
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
            // Like ⌘Q: asks about unsaved files in the editor first.
            "request-quit" => cx.defer(crate::request_quit),
            "quit-ask" => cx.defer(crate::request_quit_asking),
            _ => eprintln!("agentty: unknown debug command {command}"),
        }
        cx.notify();
    }
}
