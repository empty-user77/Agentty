//! The shortcuts a user can change: what each does, where it is listed, and the keys Agentty
//! gives it. Keys are written for macOS (`cmd-t`), as every binding in the app is; `keymap`
//! turns them into this system's.
//!
//! Shortcuts that stay as they are (text editing, the terminal's copy and paste, ⌘1…⌘9, the file
//! editor's) are not here: `main.rs` binds them, and [`super::chord::RESERVED`] keeps their keys
//! from being given away.

use crate::workbench::*;
use crate::{git_view, NewWindow};
use gpui::KeyBinding;

/// One shortcut of the settings screen.
pub struct Command {
    /// Its name in the settings file. Never changes once released.
    pub id: &'static str,
    /// The `i18n` key of the group it is listed under.
    pub group: &'static str,
    /// The `i18n` key of its name.
    pub label: &'static str,
    /// Agentty's own keys for it, written for macOS.
    pub keys: &'static [&'static str],
    /// The key context it is bound in (`None`: everywhere).
    pub context: Option<&'static str>,
    /// Its binding for `keys`: `written` for macOS (translated for this system), or as pressed here.
    bind: fn(&'static str, bool) -> KeyBinding,
}

impl Command {
    /// The binding for keys written for macOS, as this system and keyboard layout need them.
    pub fn written(&self, keys: &'static str) -> KeyBinding {
        (self.bind)(keys, true)
    }

    /// The binding for keys exactly as they were pressed on this system.
    pub fn pressed(&self, keys: &'static str) -> KeyBinding {
        (self.bind)(keys, false)
    }

    /// Whether this system has the command at all. Voice input records on macOS only; elsewhere
    /// its ⇧⌘M would translate onto mini mode's chord.
    pub fn available(&self) -> bool {
        self.id != "voice" || voice_input_available()
    }
}

macro_rules! command {
    ($id:literal, $group:expr, $label:literal, [$($keys:literal),*], $action:expr) => {
        command!($id, $group, $label, [$($keys),*], $action, None)
    };
    ($id:literal, $group:expr, $label:literal, [$($keys:literal),*], $action:expr, $context:expr) => {
        Command {
            id: $id,
            group: $group,
            label: $label,
            keys: &[$($keys),*],
            context: $context,
            bind: |keys, written| {
                if written {
                    crate::key(keys, $action, $context)
                } else {
                    KeyBinding::new(keys, $action, $context)
                }
            },
        }
    };
}

pub const LAUNCH: &str = "shortcuts.group.launch";
pub const NAVIGATE: &str = "shortcuts.group.navigate";
pub const VIEWS: &str = "shortcuts.group.views";
pub const GIT: &str = "shortcuts.group.git";

/// The groups, in the order the settings screen lists them.
pub const GROUPS: [&str; 4] = [LAUNCH, NAVIGATE, VIEWS, GIT];

const SIDEBAR: Option<&str> = Some("Sidebar");
const GIT_VIEW: Option<&str> = Some("GitView");

/// Every shortcut that can be changed. The order is the order they are bound in: the first key of
/// an action is the one the menu bar shows.
pub static COMMANDS: &[Command] = &[
    command!("new_terminal", LAUNCH, "new.terminal", ["cmd-t"], NewTerminalTab),
    command!("new_claude", LAUNCH, "new.claude", ["alt-cmd-c"], NewClaudeTab),
    command!("new_codex", LAUNCH, "new.codex", ["alt-cmd-x"], NewCodexTab),
    // ⌘N: a new tab where you are; in the workspace list, a new workspace.
    command!("new_tab_here", LAUNCH, "shortcuts.new_tab_here", ["cmd-n"], NewTerminalTab),
    command!("new_workspace", LAUNCH, "shortcuts.new_workspace_sidebar", ["cmd-n"], NewWorkspace, SIDEBAR),
    command!("new_window", LAUNCH, "new.window", ["cmd-shift-n"], NewWindow),
    command!("split_right", LAUNCH, "split.right", ["cmd-d"], SplitRight),
    command!("split_down", LAUNCH, "split.down", ["cmd-shift-d"], SplitDown),
    command!("next_pane", NAVIGATE, "keys.next_pane", ["cmd-]"], NextPane),
    command!("previous_pane", NAVIGATE, "keys.previous_pane", ["cmd-["], PreviousPane),
    command!("next_tab", NAVIGATE, "menu.next_tab", ["cmd-shift-]", "ctrl-tab"], NextTab),
    command!("previous_tab", NAVIGATE, "menu.previous_tab", ["cmd-shift-[", "ctrl-shift-tab"], PreviousTab),
    command!("next_workspace", NAVIGATE, "menu.next_workspace", ["cmd-alt-down"], NextWorkspace),
    command!("previous_workspace", NAVIGATE, "menu.previous_workspace", ["cmd-alt-up"], PreviousWorkspace),
    command!("jump_unread", NAVIGATE, "shortcuts.jump_unread", ["cmd-shift-u"], JumpToUnread),
    command!("zoom_pane", NAVIGATE, "tooltip.zoom", ["cmd-shift-enter"], ToggleZoom),
    command!("close_pane", NAVIGATE, "close.pane", ["cmd-w"], ClosePane),
    command!("close_tab", NAVIGATE, "close.tab", ["cmd-shift-w"], CloseTab),
    command!("palette", VIEWS, "shortcuts.palette", ["cmd-shift-p"], OpenPalette),
    command!("voice", VIEWS, "shortcuts.voice", ["cmd-shift-m"], ToggleVoiceInput),
    command!("search_sessions", VIEWS, "shortcuts.search_sessions", ["cmd-shift-o"], SearchSessions),
    command!("go_to_file", VIEWS, "keys.go_to_file", ["cmd-p"], GoToFile),
    command!("toggle_sidebar", VIEWS, "shortcuts.toggle_sidebar", ["cmd-b"], ToggleSidebar),
    command!("show_workspaces", VIEWS, "panel.workspaces", ["cmd-shift-e"], ShowWorkspaces),
    command!("show_sessions", VIEWS, "panel.sessions", ["cmd-shift-s"], ShowSessions),
    command!("files", VIEWS, "files.title", ["cmd-alt-b"], ToggleFiles),
    command!("browser", VIEWS, "shortcuts.browser", ["cmd-shift-b"], ToggleBrowser),
    command!("browser_hard_reload", VIEWS, "keys.browser_hard_reload", ["cmd-shift-r"], HardReloadBrowser),
    command!("chat_view", VIEWS, "menu.chat_view", ["cmd-shift-j"], ToggleChatView),
    command!("mini", VIEWS, "mini.enter", ["ctrl-cmd-m"], ToggleMini),
    command!("git", VIEWS, "page.git", ["cmd-shift-g"], OpenGit),
    command!("flow", VIEWS, "page.flow", ["cmd-shift-f"], OpenFlow),
    command!("monitoring", VIEWS, "page.monitoring", ["cmd-alt-u"], OpenUsage),
    command!("extensions", VIEWS, "page.extensions", ["cmd-shift-x"], OpenExtensions),
    command!("settings", VIEWS, "page.settings", ["cmd-,"], OpenSettings),
    command!("git_commit", GIT, "shortcuts.git_commit", ["cmd-enter"], git_view::CommitChanges, GIT_VIEW),
    command!("git_push", GIT, "shortcuts.git_push", ["cmd-p"], git_view::RemoteAction, GIT_VIEW),
    command!("git_fetch", GIT, "shortcuts.git_fetch", ["cmd-shift-t"], git_view::Fetch, GIT_VIEW),
    command!("git_refresh", GIT, "shortcuts.git_refresh", ["cmd-r"], git_view::Refresh, GIT_VIEW),
];

pub fn command(id: &str) -> Option<&'static Command> {
    COMMANDS.iter().find(|command| command.id == id)
}
