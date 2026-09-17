// Agentty — a native, GPU-rendered multi-agent terminal.
// SPDX-License-Identifier: GPL-3.0-or-later

mod agent_signal;
mod agents;
mod assets;
mod branch_picker;
mod brand;
mod browser_cli;
mod browser_mcp;
mod debug;
mod extensions_view;
mod file_drop;
mod git_view;
mod i18n;
mod launch;
mod metrics;
mod native;
mod notifications;
mod procinfo;
mod settings;
mod shell_integration;
mod status_item;
mod statusline;
mod terminal;
mod text_input;
mod theme;
mod ui;
mod usage_view;
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
    // Dock icon right-click: AppKit lists the open windows above these items.
    cx.set_dock_menu(vec![MenuItem::action(t(cx, "new.window"), NewWindow)]);
    let mut history = vec![
        MenuItem::action(t(cx, "menu.show_sessions"), workbench::ShowSessions),
        MenuItem::action(t(cx, "shortcuts.search_sessions"), workbench::SearchSessions),
        MenuItem::separator(),
    ];
    history.extend(recent_session_items(cx));
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
                MenuItem::action(t(cx, "page.usage"), workbench::OpenUsage),
                MenuItem::action(t(cx, "page.extensions"), workbench::OpenExtensions),
                MenuItem::separator(),
                MenuItem::action(t(cx, "menu.browser"), workbench::ToggleBrowser),
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

fn bind_keys(cx: &mut App) {
    use workbench::*;
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-shift-n", NewWindow, None),
        KeyBinding::new("cmd-t", NewTerminalTab, None),
        KeyBinding::new("alt-cmd-c", NewClaudeTab, None),
        KeyBinding::new("alt-cmd-x", NewCodexTab, None),
        // ⌘N: a new tab where you are; in the workspace list, a new workspace.
        KeyBinding::new("cmd-n", NewTerminalTab, None),
        KeyBinding::new("cmd-n", NewWorkspace, Some("Sidebar")),
        KeyBinding::new("cmd-w", ClosePane, None),
        KeyBinding::new("cmd-shift-w", CloseTab, None),
        KeyBinding::new("cmd-d", SplitRight, None),
        KeyBinding::new("cmd-shift-d", SplitDown, None),
        KeyBinding::new("cmd-]", NextPane, None),
        KeyBinding::new("cmd-[", PreviousPane, None),
        KeyBinding::new("cmd-shift-]", NextTab, None),
        KeyBinding::new("cmd-shift-[", PreviousTab, None),
        KeyBinding::new("ctrl-tab", NextTab, None),
        KeyBinding::new("ctrl-shift-tab", PreviousTab, None),
        KeyBinding::new("cmd-alt-down", NextWorkspace, None),
        KeyBinding::new("cmd-alt-up", PreviousWorkspace, None),
        KeyBinding::new("cmd-b", ToggleSidebar, None),
        KeyBinding::new("cmd-shift-e", ShowWorkspaces, None),
        KeyBinding::new("cmd-shift-s", ShowSessions, None),
        KeyBinding::new("cmd-shift-f", OpenFlow, None),
        KeyBinding::new("cmd-alt-u", OpenUsage, None),
        KeyBinding::new("cmd-,", OpenSettings, None),
        KeyBinding::new("cmd-shift-x", OpenExtensions, None),
        KeyBinding::new("cmd-shift-g", OpenGit, None),
        KeyBinding::new("ctrl-cmd-m", ToggleMini, None),
        KeyBinding::new("cmd-shift-b", ToggleBrowser, None),
        KeyBinding::new("cmd-f", FindInTerminal, None),
        KeyBinding::new("cmd-=", ZoomIn, None),
        KeyBinding::new("cmd-+", ZoomIn, None),
        KeyBinding::new("cmd--", ZoomOut, None),
        KeyBinding::new("cmd-0", ZoomReset, None),
        KeyBinding::new("cmd-shift-u", JumpToUnread, None),
        KeyBinding::new("cmd-shift-p", OpenPalette, None),
        KeyBinding::new("cmd-shift-enter", ToggleZoom, None),
        KeyBinding::new("cmd-shift-o", SearchSessions, None),
        KeyBinding::new("ctrl-1", GoToTab1, None),
        KeyBinding::new("ctrl-2", GoToTab2, None),
        KeyBinding::new("ctrl-3", GoToTab3, None),
        KeyBinding::new("ctrl-4", GoToTab4, None),
        KeyBinding::new("ctrl-5", GoToTab5, None),
        KeyBinding::new("ctrl-6", GoToTab6, None),
        KeyBinding::new("ctrl-7", GoToTab7, None),
        KeyBinding::new("ctrl-8", GoToTab8, None),
        KeyBinding::new("ctrl-9", GoToTab9, None),
        KeyBinding::new("cmd-1", GoToWorkspace1, None),
        KeyBinding::new("cmd-2", GoToWorkspace2, None),
        KeyBinding::new("cmd-3", GoToWorkspace3, None),
        KeyBinding::new("cmd-4", GoToWorkspace4, None),
        KeyBinding::new("cmd-5", GoToWorkspace5, None),
        KeyBinding::new("cmd-6", GoToWorkspace6, None),
        KeyBinding::new("cmd-7", GoToWorkspace7, None),
        KeyBinding::new("cmd-8", GoToWorkspace8, None),
        KeyBinding::new("cmd-9", GoToWorkspace9, None),
        KeyBinding::new("cmd-enter", git_view::CommitChanges, Some("GitView")),
        KeyBinding::new("cmd-p", git_view::RemoteAction, Some("GitView")),
        KeyBinding::new("cmd-shift-t", git_view::Fetch, Some("GitView")),
        KeyBinding::new("cmd-r", git_view::Refresh, Some("GitView")),
        KeyBinding::new("cmd-c", terminal::Copy, Some("Terminal")),
        KeyBinding::new("cmd-v", terminal::Paste, Some("Terminal")),
        KeyBinding::new("cmd-k", terminal::Clear, Some("Terminal")),
        KeyBinding::new("cmd-a", terminal::SelectAll, Some("Terminal")),
    ]);
    text_input::bind_keys(cx);
}

fn main() {
    // `agentty mcp-connector --id <id>`: serve an API connector to an agent over stdio, no UI.
    let args: Vec<String> = std::env::args().collect();
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

    if args.get(1).map(String::as_str) == Some("notify") {
        if let Err(err) = agent_signal::send_notify(&args[2..].join(" ")) {
            eprintln!("agentty: {err:#}");
            std::process::exit(1);
        }
        return;
    }

    sanitize_environment();

    let app = Application::new().with_assets(assets::Assets);
    // Clicking the Dock icon brings the window back (after closing to the menu bar or mini mode).
    app.on_open_urls(|urls| {
        let folders = urls.iter().filter_map(|url| file_url_path(url)).filter(|path| path.is_dir());
        if let Ok(mut queue) = OPENED_FOLDERS.lock() {
            queue.extend(folders);
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
                cx.set_global(socket);
                Some(rx)
            }
            Err(err) => {
                eprintln!("agentty: agent status socket unavailable: {err:#}");
                None
            }
        };

        notifications::prepare();
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
            NEXT_WINDOW_SLOT.fetch_max(slot + 1, std::sync::atomic::Ordering::Relaxed);
        }
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
                    if !settings::settings(cx).menu_bar {
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
        titlebar: Some(TitlebarOptions {
            title: Some("Agentty".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(12.), px(11.))),
        }),
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
    fn decodes_file_urls() {
        assert_eq!(file_url_path("file:///Users/me/My%20Project/"), Some(PathBuf::from("/Users/me/My Project")));
        assert_eq!(file_url_path("file://localhost/tmp/%ED%95%9C"), Some(PathBuf::from("/tmp/한")));
        assert_eq!(file_url_path("file:///"), Some(PathBuf::from("/")));
        assert_eq!(file_url_path("https://agentty.run"), None);
    }
}
