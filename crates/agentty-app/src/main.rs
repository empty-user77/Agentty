// Agentty — a native, GPU-rendered multi-agent terminal.
// SPDX-License-Identifier: GPL-3.0-or-later

// Release builds on Windows are GUI apps (no console window of their own).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod agent_signal;
mod agents;
mod ai_processes;
mod assets;
mod branch_picker;
mod brand;
mod browser_cli;
mod browser_mcp;
mod capture;
mod debug;
mod extensions_view;
// AppKit / WebKit on macOS; the same API from `platform/fallback/` on Windows and Linux.
#[cfg_attr(not(target_os = "macos"), path = "platform/fallback/file_drop.rs")]
mod file_drop;
mod git_view;
mod hud;
mod i18n;
mod idea_view;
mod instance;
mod ipc;
mod keymap;
mod launch;
mod metrics;
#[cfg_attr(not(target_os = "macos"), path = "platform/fallback/native.rs")]
mod native;
#[cfg_attr(not(target_os = "macos"), path = "platform/fallback/notifications.rs")]
mod notifications;
mod platform;
mod plugins;
mod procinfo;
mod settings;
mod setup_check;
mod shell_integration;
#[cfg_attr(not(target_os = "macos"), path = "platform/fallback/status_item.rs")]
mod status_item;
mod statusline;
mod terminal;
mod text_input;
mod theme;
mod ui;
mod usage_view;
#[cfg_attr(not(target_os = "macos"), path = "platform/fallback/webview.rs")]
mod webview;
mod workbench;

use futures::StreamExt;
use gpui::{
    actions, point, px, size, App, AppContext, Application, Bounds, KeyBinding, Menu, MenuItem, TitlebarOptions, WindowBounds,
    WindowOptions,
};
use i18n::t;
use std::borrow::Cow;
use workbench::Workbench;

actions!(agentty, [Quit, NewWindow]);

const FONTS: &[&[u8]] = &[
    include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-Bold.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-Italic.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-BoldItalic.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMonoNerdFontMono-Regular.ttf"),
];

/// Drops variables inherited from whatever launched Agentty (another terminal, an agent session)
/// so they don't leak into shells started here — e.g. Claude Code's child-session marker, which
/// disables transcript saving in nested sessions.
fn sanitize_environment() {
    let inherited = |key: &str| {
        [
            "CLAUDECODE",
            "CLAUDE_CODE_",
            "CMUX_",
            "GHOSTTY_",
            "ITERM_",
            "KITTY_",
            "WEZTERM_",
            "ALACRITTY_",
            "__CFBundle",
            "CODEX_THREAD",
            "CODEX_SANDBOX",
            // Credentials Agentty hands to agent panes; a nested Agentty must not pass them on.
            "AGENTTY_AUTH_",
        ]
        .iter()
        .any(|prefix| key.starts_with(prefix))
            || matches!(
                key,
                "CLAUDE_PID"
                    | "CLAUDE_EFFORT"
                    | "TERMINFO"
                    | "TERM_PROGRAM"
                    | "TERM_PROGRAM_VERSION"
                    | "TERM_SESSION_ID"
                    | "AGENTTY_PANE_ID"
                    | "AGENTTY_SOCKET"
                    | "AGENTTY_SOCKET_TOKEN"
                    | "AGENTTY_SHELL_DIR"
                    | "AGENTTY_USER_ZDOTDIR"
            )
    };
    for (key, value) in std::env::vars_os() {
        let Some(key) = key.to_str() else { continue };
        if inherited(key) || (key == "NODE_OPTIONS" && value.to_string_lossy().contains("cmux")) {
            std::env::remove_var(key);
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        let kept: Vec<_> = std::env::split_paths(&path).filter(|p| !p.to_string_lossy().contains("cmux-cli-shims")).collect();
        if let Ok(joined) = std::env::join_paths(kept) {
            std::env::set_var("PATH", joined);
        }
    }
}

actions!(agentty, [HideApp, HideOthers, ShowAll, MinimizeWindow, ZoomWindow]);

/// Reopens a recently closed window (its saved slot).
#[derive(Clone, PartialEq, gpui::Action)]
#[action(namespace = agentty, no_json)]
pub struct ReopenWindow {
    pub slot: usize,
}

/// Resumes one of the sessions listed under History (index into [`RECENT_SESSIONS`]).
#[derive(Clone, PartialEq, gpui::Action)]
#[action(namespace = agentty, no_json)]
pub struct OpenRecentSession {
    pub index: usize,
}

/// Most recent local sessions shown in the History menu.
pub static RECENT_SESSIONS: std::sync::Mutex<Vec<agentty_bridge::model::SessionInfo>> = std::sync::Mutex::new(Vec::new());
const RECENT_MENU_LIMIT: usize = 10;

/// Refreshes the History menu from a freshly loaded session list (newest first).
pub fn set_recent_sessions(sessions: &[agentty_bridge::model::SessionInfo], cx: &mut App) {
    let recent: Vec<_> = sessions.iter().take(RECENT_MENU_LIMIT).cloned().collect();
    let changed = {
        let Ok(mut current) = RECENT_SESSIONS.lock() else { return };
        let same = current.len() == recent.len() && current.iter().zip(&recent).all(|(a, b)| a.id == b.id && a.title == b.title);
        if !same {
            *current = recent;
        }
        !same
    };
    if changed {
        set_app_menus(cx);
    }
}

fn recent_session_items(cx: &App) -> Vec<MenuItem> {
    let Ok(recent) = RECENT_SESSIONS.lock() else { return Vec::new() };
    recent
        .iter()
        .enumerate()
        .map(|(index, session)| {
            let mut title: String = session.title.chars().take(60).collect();
            if session.title.chars().count() > 60 {
                title.push('…');
            }
            MenuItem::action(format!("{} — {title}", session.agent.display_name()), OpenRecentSession { index })
        })
        .chain(recent.is_empty().then(|| MenuItem::action(t(cx, "menu.no_recent"), workbench::ShowSessions)))
        .collect()
}

// Position of the Window menu, registered with AppKit (see `native::register_windows_menu`).
const WINDOW_MENU_INDEX: usize = 5;

pub fn set_app_menus(cx: &mut App) {
    // Dock icon right-click: AppKit lists the open windows above these items; recently closed
    // windows follow so they can be reopened.
    let closed_windows = workbench::ClosedWindows::load().windows;
    let closed_items = || -> Vec<MenuItem> {
        closed_windows
            .iter()
            .map(|w| {
                let title = if w.title.is_empty() { "Agentty".to_string() } else { format!("{} — Agentty", w.title) };
                MenuItem::action(title, ReopenWindow { slot: w.slot })
            })
            .collect()
    };
    let mut dock = closed_items();
    if !dock.is_empty() {
        dock.push(MenuItem::separator());
    }
    dock.push(MenuItem::action(t(cx, "new.window"), NewWindow));
    cx.set_dock_menu(dock);
    let mut history = vec![
        MenuItem::action(t(cx, "menu.show_sessions"), workbench::ShowSessions),
        MenuItem::action(t(cx, "shortcuts.search_sessions"), workbench::SearchSessions),
        MenuItem::separator(),
    ];
    history.extend(recent_session_items(cx));
    if !closed_windows.is_empty() {
        history.push(MenuItem::separator());
        history.push(MenuItem::submenu(Menu { name: t(cx, "menu.closed_windows").into(), items: closed_items() }));
    }
    cx.set_menus(vec![
        Menu {
            name: "Agentty".into(),
            items: vec![
                MenuItem::action(t(cx, "about.menu"), workbench::ShowAbout),
                MenuItem::action(t(cx, "update.check_menu"), workbench::CheckForUpdates),
                MenuItem::action(t(cx, "update.install_menu"), workbench::InstallUpdate),
                MenuItem::separator(),
                MenuItem::action(t(cx, "menu.settings"), workbench::OpenSettings),
                MenuItem::separator(),
                MenuItem::action(t(cx, "menu.hide"), HideApp),
                MenuItem::action(t(cx, "menu.hide_others"), HideOthers),
                MenuItem::action(t(cx, "menu.show_all"), ShowAll),
                MenuItem::separator(),
                MenuItem::action(t(cx, "menu.quit"), Quit),
            ],
        },
        Menu {
            name: t(cx, "menu.file").into(),
            items: vec![
                MenuItem::action(t(cx, "new.terminal"), workbench::NewTerminalTab),
                MenuItem::action(t(cx, "new.claude"), workbench::NewClaudeTab),
                MenuItem::action(t(cx, "new.codex"), workbench::NewCodexTab),
                MenuItem::separator(),
                MenuItem::action(t(cx, "new.workspace"), workbench::NewWorkspace),
                MenuItem::action(t(cx, "new.window"), NewWindow),
                MenuItem::separator(),
                MenuItem::action(t(cx, "split.right"), workbench::SplitRight),
                MenuItem::action(t(cx, "split.down"), workbench::SplitDown),
                MenuItem::separator(),
                MenuItem::action(t(cx, "close.pane"), workbench::ClosePane),
                MenuItem::action(t(cx, "close.tab"), workbench::CloseTab),
            ],
        },
        Menu {
            name: t(cx, "menu.edit").into(),
            items: vec![
                MenuItem::os_action(t(cx, "menu.cut"), terminal::Copy, gpui::OsAction::Cut),
                MenuItem::os_action(t(cx, "menu.copy"), terminal::Copy, gpui::OsAction::Copy),
                MenuItem::os_action(t(cx, "menu.paste"), terminal::Paste, gpui::OsAction::Paste),
                MenuItem::os_action(t(cx, "menu.select_all"), terminal::SelectAll, gpui::OsAction::SelectAll),
                MenuItem::separator(),
                MenuItem::action(t(cx, "menu.find"), workbench::FindInTerminal),
                MenuItem::action(t(cx, "shortcuts.clear"), terminal::Clear),
                MenuItem::separator(),
                MenuItem::action(t(cx, "shortcuts.palette"), workbench::OpenPalette),
            ],
        },
        Menu {
            name: t(cx, "menu.view").into(),
            items: vec![
                MenuItem::action(t(cx, "menu.toggle_sidebar"), workbench::ToggleSidebar),
                MenuItem::action(t(cx, "menu.show_workspaces"), workbench::ShowWorkspaces),
                MenuItem::separator(),
                MenuItem::action(t(cx, "page.git"), workbench::OpenGit),
                MenuItem::action(t(cx, "page.flow"), workbench::OpenFlow),
                MenuItem::action(t(cx, "page.monitoring"), workbench::OpenUsage),
                MenuItem::action(t(cx, "page.extensions"), workbench::OpenExtensions),
                MenuItem::action(t(cx, "page.plugins"), workbench::OpenPlugins),
                MenuItem::separator(),
                MenuItem::action(t(cx, "menu.browser"), workbench::ToggleBrowser),
                MenuItem::action(t(cx, "files.title"), workbench::ToggleFiles),
                MenuItem::action(t(cx, "menu.zoom_pane"), workbench::ToggleZoom),
                MenuItem::separator(),
                MenuItem::action(t(cx, "menu.font_bigger"), workbench::ZoomIn),
                MenuItem::action(t(cx, "menu.font_smaller"), workbench::ZoomOut),
                MenuItem::action(t(cx, "menu.font_reset"), workbench::ZoomReset),
            ],
        },
        Menu { name: t(cx, "menu.history").into(), items: history },
        Menu {
            name: t(cx, "menu.window").into(),
            items: vec![
                MenuItem::action(t(cx, "menu.minimize"), MinimizeWindow),
                MenuItem::action(t(cx, "menu.zoom"), ZoomWindow),
                MenuItem::action(t(cx, "mini.enter"), workbench::ToggleMini),
                MenuItem::separator(),
                MenuItem::action(t(cx, "new.window"), NewWindow),
                MenuItem::separator(),
                MenuItem::action(t(cx, "menu.next_tab"), workbench::NextTab),
                MenuItem::action(t(cx, "menu.previous_tab"), workbench::PreviousTab),
                MenuItem::action(t(cx, "menu.next_workspace"), workbench::NextWorkspace),
                MenuItem::action(t(cx, "menu.previous_workspace"), workbench::PreviousWorkspace),
            ],
        },
    ]);
    // GPUI only registers a menu literally named "Window"; register the localized one so AppKit
    // lists open windows there and in the Dock menu.
    native::register_windows_menu(WINDOW_MENU_INDEX);
}

fn register_app_actions(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &HideApp, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &MinimizeWindow, cx| {
        if let Some(handle) = cx.active_window() {
            let _ = handle.update(cx, |_, window, _| window.minimize_window());
        }
    });
    cx.on_action(|action: &ReopenWindow, cx| reopen_window(action.slot, cx));
    cx.on_action(|_: &ZoomWindow, cx| {
        if let Some(handle) = cx.active_window() {
            let _ = handle.update(cx, |_, window, _| window.zoom_window());
        }
    });
    // Load the History menu once at launch; the sessions panel keeps it fresh afterwards.
    let task = cx.background_executor().spawn(async { agentty_bridge::list(None, RECENT_MENU_LIMIT) });
    cx.spawn(async move |cx| {
        let sessions = task.await;
        let _ = cx.update(|cx| set_recent_sessions(&sessions, cx));
    })
    .detach();
}

/// A binding written for macOS, translated for this platform (see `keymap`).
pub fn key<A: gpui::Action>(keys: &str, action: A, context: Option<&str>) -> KeyBinding {
    KeyBinding::new(&keymap::binding(keys, context), action, context)
}

fn bind_keys(cx: &mut App) {
    use workbench::*;
    cx.bind_keys([
        key("cmd-q", Quit, None),
        key("cmd-shift-n", NewWindow, None),
        key("cmd-t", NewTerminalTab, None),
        key("alt-cmd-c", NewClaudeTab, None),
        key("alt-cmd-x", NewCodexTab, None),
        // ⌘N: a new tab where you are; in the workspace list, a new workspace.
        key("cmd-n", NewTerminalTab, None),
        key("cmd-n", NewWorkspace, Some("Sidebar")),
        key("cmd-w", ClosePane, None),
        key("cmd-shift-w", CloseTab, None),
        key("cmd-d", SplitRight, None),
        key("cmd-shift-d", SplitDown, None),
        key("cmd-]", NextPane, None),
        key("cmd-[", PreviousPane, None),
        key("cmd-shift-]", NextTab, None),
        key("cmd-shift-[", PreviousTab, None),
        key("ctrl-tab", NextTab, None),
        key("ctrl-shift-tab", PreviousTab, None),
        key("cmd-alt-down", NextWorkspace, None),
        key("cmd-alt-up", PreviousWorkspace, None),
        key("cmd-b", ToggleSidebar, None),
        key("cmd-shift-e", ShowWorkspaces, None),
        key("cmd-shift-s", ShowSessions, None),
        key("cmd-shift-f", OpenFlow, None),
        key("cmd-alt-u", OpenUsage, None),
        key("cmd-,", OpenSettings, None),
        key("cmd-shift-x", OpenExtensions, None),
        key("cmd-shift-g", OpenGit, None),
        key("ctrl-cmd-m", ToggleMini, None),
        key("cmd-shift-b", ToggleBrowser, None),
        key("cmd-alt-b", ToggleFiles, None),
        key("cmd-f", FindInTerminal, None),
        key("cmd-=", ZoomIn, None),
        key("cmd-+", ZoomIn, None),
        key("cmd--", ZoomOut, None),
        key("cmd-0", ZoomReset, None),
        key("cmd-shift-u", JumpToUnread, None),
        key("cmd-shift-p", OpenPalette, None),
        key("cmd-shift-enter", ToggleZoom, None),
        key("cmd-shift-o", SearchSessions, None),
        key("ctrl-1", GoToTab1, None),
        key("ctrl-2", GoToTab2, None),
        key("ctrl-3", GoToTab3, None),
        key("ctrl-4", GoToTab4, None),
        key("ctrl-5", GoToTab5, None),
        key("ctrl-6", GoToTab6, None),
        key("ctrl-7", GoToTab7, None),
        key("ctrl-8", GoToTab8, None),
        key("ctrl-9", GoToTab9, None),
        key("cmd-1", GoToWorkspace1, None),
        key("cmd-2", GoToWorkspace2, None),
        key("cmd-3", GoToWorkspace3, None),
        key("cmd-4", GoToWorkspace4, None),
        key("cmd-5", GoToWorkspace5, None),
        key("cmd-6", GoToWorkspace6, None),
        key("cmd-7", GoToWorkspace7, None),
        key("cmd-8", GoToWorkspace8, None),
        key("cmd-9", GoToWorkspace9, None),
        key("cmd-enter", git_view::CommitChanges, Some("GitView")),
        key("cmd-p", git_view::RemoteAction, Some("GitView")),
        key("cmd-shift-t", git_view::Fetch, Some("GitView")),
        key("cmd-r", git_view::Refresh, Some("GitView")),
        key("cmd-c", terminal::Copy, Some("Terminal")),
        key("cmd-v", terminal::Paste, Some("Terminal")),
        key("cmd-k", terminal::Clear, Some("Terminal")),
        key("cmd-a", terminal::SelectAll, Some("Terminal")),
    ]);
    text_input::bind_keys(cx);
}

/// Windows: a GUI-subsystem program started from a terminal has no console; subcommands that
/// print (`agentty browser`, `agentty notify`) attach to the terminal they were started from.
/// Pipes (hooks, MCP servers) are inherited as usual and left alone.
#[cfg(windows)]
fn attach_parent_console() {
    use windows_sys::Win32::System::Console::{AttachConsole, GetStdHandle, ATTACH_PARENT_PROCESS, STD_OUTPUT_HANDLE};
    // SAFETY: both calls only query / change this process's console attachment.
    unsafe {
        if GetStdHandle(STD_OUTPUT_HANDLE).is_null() {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

fn main() {
    // `agentty mcp-connector --id <id>`: serve an API connector to an agent over stdio, no UI.
    let args: Vec<String> = std::env::args().collect();
    #[cfg(windows)]
    if args.len() > 1 {
        attach_parent_console();
    }
    if args.get(1).map(String::as_str) == Some("mcp-connector") {
        let id = args.iter().position(|a| a == "--id").and_then(|i| args.get(i + 1));
        let result = match id {
            Some(id) => agentty_bridge::connectors::serve(id),
            None => Err(anyhow::anyhow!("usage: agentty mcp-connector --id <connector id>")),
        };
        if let Err(err) = result {
            eprintln!("agentty: {err:#}");
            std::process::exit(1);
        }
        return;
    }

    if args.get(1).map(String::as_str) == Some("statusline") {
        std::process::exit(statusline::run());
    }

    if args.get(1).map(String::as_str) == Some("mcp-browser") {
        std::process::exit(browser_mcp::serve());
    }

    if args.get(1).map(String::as_str) == Some("browser") {
        std::process::exit(browser_cli::run(&args[2..]));
    }

    // `agentty signal <kind> [payload]`: agent hook → pane status (Linux / Windows hooks).
    if args.get(1).map(String::as_str) == Some("signal") {
        std::process::exit(agent_signal::forward_signal(&args[2..]));
    }

    // `agentty worktree-for <agent>`: the shell wrappers ask for a working tree of the agent's own.
    if args.get(1).map(String::as_str) == Some("worktree-for") {
        std::process::exit(agent_signal::worktree_for(&args[2..]));
    }

    if args.get(1).map(String::as_str) == Some("notify") {
        if let Err(err) = agent_signal::send_notify(&args[2..].join(" ")) {
            eprintln!("agentty: {err:#}");
            std::process::exit(1);
        }
        return;
    }

    sanitize_environment();
    // Windows / Linux hand links (`agentty://…`, registered by the installer / .desktop file) and
    // folders ("Open with Agentty") over as arguments; macOS sends them as open events below.
    if !cfg!(target_os = "macos") {
        // Already running: hand the links / folders over (or just bring it forward) and exit.
        if std::env::var("AGENTTY_BACKGROUND").as_deref() != Ok("1") && instance::forward(&args[1..]) {
            return;
        }
        queue_launch_arguments(&args[1..]);
    }

    let app = Application::new().with_assets(assets::Assets);
    // Clicking the Dock icon brings the window back (after closing to the menu bar or mini mode).
    app.on_open_urls(|urls| {
        let folders = urls.iter().filter_map(|url| file_url_path(url)).filter(|path| path.is_dir());
        if let Ok(mut queue) = OPENED_FOLDERS.lock() {
            queue.extend(folders);
        }
        // `agentty://…` links from other apps (e.g. Cosmica's "Continue in Agentty").
        let links = urls.iter().filter(|url| url.starts_with("agentty://")).cloned();
        if let Ok(mut queue) = OPENED_LINKS.lock() {
            queue.extend(links);
        }
    });
    app.on_reopen(|cx| with_workbench(cx, |wb, window, cx| wb.handle_tray(status_item::TrayAction::Show, window, cx)));
    app.run(|cx: &mut App| {
        if let Err(err) = cx.text_system().add_fonts(FONTS.iter().map(|f| Cow::Borrowed(*f)).collect()) {
            eprintln!("agentty: failed to load bundled fonts: {err:#}");
        }
        settings::SettingsStore::init(cx);
        let signals = match agent_signal::start() {
            Ok((socket, rx)) => {
                if !cfg!(target_os = "macos") {
                    instance::register(&socket);
                    cx.on_app_quit(|_| async { instance::unregister() }).detach();
                }
                cx.set_global(socket);
                Some(rx)
            }
            Err(err) => {
                eprintln!("agentty: agent status socket unavailable: {err:#}");
                None
            }
        };

        notifications::prepare();
        let mut plugin_events = plugins::init(cx);
        cx.spawn(async move |cx| {
            while let Some(envelope) = plugin_events.next().await {
                if cx.update(|cx| plugins::handle(envelope, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        bind_keys(cx);
        register_app_actions(cx);
        set_app_menus(cx);

        let Some(_) = open_window(0, cx) else {
            eprintln!("agentty: failed to open the window");
            return;
        };
        // Windows that were open at the last quit.
        for slot in workbench::saved_window_slots() {
            open_window(slot, cx);
        }
        // New windows never take the slot of a saved (open or recently closed) one.
        NEXT_WINDOW_SLOT.fetch_max(workbench::next_free_window_slot(), std::sync::atomic::Ordering::Relaxed);
        cx.on_action(|_: &NewWindow, cx| new_window(cx));
        metrics::start_uploads(cx);
        metrics::track(cx, "app_launched", serde_json::json!({ "windows": workbenches(cx).len() }));

        // Files dropped on a window go to the pane under the pointer.
        cx.spawn(async move |cx| loop {
            cx.background_executor().timer(std::time::Duration::from_millis(100)).await;
            let alive = cx.update(|cx| {
                let opened = OPENED_FOLDERS.lock().map(|mut queue| std::mem::take(&mut *queue)).unwrap_or_default();
                for folder in opened {
                    // The frontmost Agentty window, else the main one.
                    let windows = workbenches(cx);
                    let active = cx.active_window().and_then(|w| w.downcast::<Workbench>());
                    if let Some(target) = active.or(windows.first().copied()) {
                        let _ = target.update(cx, |workbench, window, cx| workbench.open_folder(folder, window, cx));
                    }
                }
                let links = OPENED_LINKS.lock().map(|mut queue| std::mem::take(&mut *queue)).unwrap_or_default();
                for link in links {
                    with_active_workbench(cx, |workbench, window, cx| workbench.open_agentty_link(&link, window, cx));
                }
                for dropped in file_drop::drain() {
                    for window in workbenches(cx) {
                        let _ = window.update(cx, |workbench, window, cx| {
                            if native::ns_window(window).is_some_and(|w| native::window_number(w) == dropped.window_number) {
                                workbench.drop_files_at(dropped.position, &dropped.paths, window, cx);
                            }
                        });
                    }
                }
            });
            if alive.is_err() {
                break;
            }
        })
        .detach();

        // Menu bar item: animation while agents work (in any window), and its menu's actions.
        cx.spawn(async move |cx| {
            let mut tray: Option<status_item::StatusItem> = None;
            loop {
                let working = cx.update(|cx| {
                    if !settings::settings(cx).menu_bar || !platform::HAS_STATUS_ITEM {
                        tray = None;
                        return false;
                    }
                    let tray = tray.get_or_insert_with(status_item::StatusItem::new);
                    let windows = workbenches(cx);
                    for action in status_item::drain_actions() {
                        // Focus goes to the window holding that pane; everything else to the main window.
                        let target = match action {
                            status_item::TrayAction::Focus(pane) => {
                                windows.iter().find(|w| w.read(cx).is_ok_and(|wb| wb.has_pane(pane, cx))).or(windows.first()).copied()
                            }
                            _ => windows.first().copied(),
                        };
                        if let Some(window) = target {
                            let _ = window.update(cx, |workbench, window, cx| workbench.handle_tray(action, window, cx));
                        }
                    }
                    let mut state: Option<status_item::TrayState> = None;
                    for window in &windows {
                        let Ok(next) = window.read(cx).map(|wb| wb.tray_state(cx)) else { continue };
                        match state.as_mut() {
                            Some(state) => {
                                state.panes.extend(next.panes);
                                state.mini |= next.mini;
                            }
                            None => state = Some(next),
                        }
                    }
                    let state = state.unwrap_or_default();
                    tray.update(&state);
                    state.working() > 0
                });
                let Ok(working) = working else { break };
                cx.background_executor().timer(std::time::Duration::from_millis(if working { 120 } else { 700 })).await;
            }
        })
        .detach();

        if let Some(mut rx) = signals {
            cx.spawn(async move |cx| {
                while let Some(message) = rx.next().await {
                    let delivered = cx.update(|cx| {
                        let windows = workbenches(cx);
                        match message {
                            // Every window applies it; only the one holding the pane has it.
                            agent_signal::SocketMessage::Signal(signal) => {
                                for window in windows {
                                    let _ = window.update(cx, |workbench, _, cx| workbench.apply_signal(signal.clone(), cx));
                                }
                            }
                            agent_signal::SocketMessage::Browser(request) => {
                                // The window holding the asking pane, else the main window.
                                let target = request
                                    .pane
                                    .and_then(|pane| windows.iter().find(|w| w.read(cx).is_ok_and(|wb| wb.has_pane(pane, cx))))
                                    .or(windows.first())
                                    .copied();
                                match target {
                                    Some(window) => {
                                        let _ = window.update(cx, |workbench, window, cx| workbench.browser_command(request, window, cx));
                                    }
                                    None => {
                                        let _ = request.reply.send(agent_signal::browser_reply(Err("no Agentty window".into())));
                                    }
                                }
                            }
                            agent_signal::SocketMessage::Worktree(request) => {
                                workbench::worktrees::answer_worktree_request(&windows, request, cx)
                            }
                            agent_signal::SocketMessage::Open(arguments) => {
                                queue_launch_arguments(&arguments);
                                if let Some(window) = windows.first() {
                                    let _ = window.update(cx, |_, window, _| window.activate_window());
                                }
                                cx.activate(true);
                            }
                            agent_signal::SocketMessage::Debug(command, argument) => {
                                if let Some(window) = windows.first() {
                                    let _ =
                                        window.update(cx, |workbench, window, cx| workbench.debug_command(&command, &argument, window, cx));
                                }
                            }
                        }
                    });
                    if delivered.is_err() {
                        break;
                    }
                }
            })
            .detach();
        }
        // Test runs (`AGENTTY_BACKGROUND=1`) open without taking focus from the app in use.
        if std::env::var("AGENTTY_BACKGROUND").as_deref() != Ok("1") {
            cx.activate(true);
        }
    });
}

/// Folders macOS asked us to open (Dock recent list, Finder "Open With"), handled by the app loop.
static OPENED_FOLDERS: std::sync::Mutex<Vec<std::path::PathBuf>> = std::sync::Mutex::new(Vec::new());
/// `agentty://` links opened by other apps, handled by the app loop.
static OPENED_LINKS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Links and folders among the launch arguments, queued like macOS open events.
fn queue_launch_arguments(arguments: &[String]) {
    let (links, folders) = launch_arguments(arguments);
    if let Ok(mut queue) = OPENED_LINKS.lock() {
        queue.extend(links);
    }
    if let Ok(mut queue) = OPENED_FOLDERS.lock() {
        queue.extend(folders);
    }
}

/// `agentty://` links and existing folders (given as paths or `file://` URLs); anything else is
/// ignored, so stray arguments can't make Agentty open arbitrary things.
fn launch_arguments(arguments: &[String]) -> (Vec<String>, Vec<std::path::PathBuf>) {
    let mut links = Vec::new();
    let mut folders = Vec::new();
    for argument in arguments {
        if argument.starts_with("agentty://") {
            links.push(argument.clone());
        } else if let Some(path) = file_url_path(argument).or_else(|| Some(std::path::PathBuf::from(argument))) {
            if path.is_absolute() && path.is_dir() {
                folders.push(path);
            }
        }
    }
    (links, folders)
}

/// `file:///Users/me/My%20Project/` → `/Users/me/My Project`.
fn file_url_path(url: &str) -> Option<std::path::PathBuf> {
    let encoded = url.strip_prefix("file://")?;
    let encoded = encoded.strip_prefix("localhost").unwrap_or(encoded);
    let bytes = encoded.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Some(byte) = std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                decoded.push(byte);
                i += 3;
                continue;
            }
        }
        decoded.push(bytes[i]);
        i += 1;
    }
    let path = String::from_utf8(decoded).ok()?;
    let trimmed = path.trim_end_matches('/');
    Some(std::path::PathBuf::from(if trimmed.is_empty() { "/" } else { trimmed }))
}

static NEXT_WINDOW_SLOT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);

/// Opens another Agentty window with its own workspaces.
pub fn new_window(cx: &mut App) {
    // Slots are never reused within a run (windows can't always be read while one is updating).
    let slot = NEXT_WINDOW_SLOT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    match open_window(slot, cx) {
        Some(window) => {
            metrics::track(cx, "window_opened", serde_json::json!({}));
            let _ = window.update(cx, |_, window, _| window.activate_window());
        }
        None => eprintln!("agentty: could not open window {slot}"),
    }
}

/// Opens a recently closed window again, or brings it forward if it is already open.
fn reopen_window(slot: usize, cx: &mut App) {
    if let Some(open) = workbenches(cx).into_iter().find(|w| w.read(cx).is_ok_and(|wb| wb.slot == slot)) {
        let _ = open.update(cx, |_, window, _| window.activate_window());
        return;
    }
    workbench::ClosedWindows::reopen(slot);
    if let Some(window) = open_window(slot, cx) {
        let _ = window.update(cx, |_, window, _| window.activate_window());
        cx.activate(true);
    }
    set_app_menus(cx);
}

/// Agentty windows, main window first.
fn workbenches(cx: &App) -> Vec<gpui::WindowHandle<Workbench>> {
    let mut windows: Vec<_> = cx.windows().into_iter().filter_map(|w| w.downcast::<Workbench>()).collect();
    windows.sort_by_key(|w| w.read(cx).map(|wb| wb.slot).unwrap_or(usize::MAX));
    windows
}

/// Opens Agentty window `slot` (its workspaces come from that slot's layout file).
fn open_window(slot: usize, cx: &mut App) -> Option<gpui::WindowHandle<Workbench>> {
    let offset = slot as f32 * 28.;
    let mut bounds = Bounds::centered(None, size(px(1400.), px(880.)), cx);
    bounds.origin.x += px(offset);
    bounds.origin.y += px(offset);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        // macOS draws the traffic lights over Agentty's own title bar; Windows and Linux keep the
        // system title bar (Linux asks for server-side decorations; see `render_title_bar`).
        titlebar: Some(TitlebarOptions {
            title: Some("Agentty".into()),
            appears_transparent: cfg!(target_os = "macos"),
            traffic_light_position: Some(point(px(12.), px(11.))),
        }),
        window_decorations: (!cfg!(target_os = "macos")).then_some(gpui::WindowDecorations::Server),
        window_min_size: Some(size(px(720.), px(440.))),
        ..Default::default()
    };
    cx.open_window(options, |window, cx| {
        if let Some(ns) = native::ns_window(window) {
            file_drop::install(ns);
        }
        cx.new(|cx| Workbench::new(slot, window, cx))
    })
    .ok()
}

/// The frontmost Agentty window, else the main one.
fn active_workbench(cx: &App) -> Option<gpui::WindowHandle<Workbench>> {
    let active = cx.active_window().and_then(|w| w.downcast::<Workbench>());
    active.or_else(|| workbenches(cx).into_iter().next())
}

/// Runs `f` with the frontmost workbench; false when no window is open.
pub fn with_active_workbench(cx: &mut App, f: impl FnOnce(&mut Workbench, &mut gpui::Window, &mut gpui::Context<Workbench>)) -> bool {
    let Some(handle) = active_workbench(cx) else { return false };
    handle.update(cx, |workbench, window, cx| f(workbench, window, cx)).is_ok()
}

/// Runs `f` with the main window's workbench, wherever it is called from.
fn with_workbench(cx: &mut App, f: impl FnOnce(&mut Workbench, &mut gpui::Window, &mut gpui::Context<Workbench>)) {
    let Some(handle) = workbenches(cx).into_iter().next() else { return };
    let _ = handle.update(cx, |workbench, window, cx| f(workbench, window, cx));
}

#[cfg(test)]
mod tests {
    use super::file_url_path;
    use std::path::PathBuf;

    #[test]
    fn reads_links_and_folders_from_arguments() {
        let folder = std::env::temp_dir();
        let args = vec![
            "agentty://prompt?text=hi".to_string(),
            folder.display().to_string(),
            "relative/dir".to_string(),
            "--flag".to_string(),
            folder.join("agentty-missing-dir").display().to_string(),
        ];
        let (links, folders) = super::launch_arguments(&args);
        assert_eq!(links, vec!["agentty://prompt?text=hi".to_string()]);
        assert_eq!(folders, vec![folder]);
    }

    #[test]
    fn decodes_file_urls() {
        assert_eq!(file_url_path("file:///Users/me/My%20Project/"), Some(PathBuf::from("/Users/me/My Project")));
        assert_eq!(file_url_path("file://localhost/tmp/%ED%95%9C"), Some(PathBuf::from("/tmp/한")));
        assert_eq!(file_url_path("file:///"), Some(PathBuf::from("/")));
        assert_eq!(file_url_path("https://agentty.run"), None);
    }
}
