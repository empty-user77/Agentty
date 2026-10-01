//! In-app browser on Windows: a Microsoft Edge WebView2 (Chromium) placed over a GPUI element.
//! Each page lives in a window of its own (the "host") that Agentty's window owns, moved over the
//! placeholder element every frame, as the WebKit view is on macOS. Not a child window: GPUI draws
//! through DirectComposition, whose layer covers child windows, and turning that off makes Agentty
//! look like a game to overlays (NVIDIA, Steam, Discord) that then hook into it. A window the
//! main one owns always stays above it, follows it when it moves (see [`follow_owner`]) and is
//! hidden with it when it is minimized. The API is the same as
//! `webview.rs` (macOS), so the panel, the agents' `agentty browser` commands and plugins drive
//! both alike.
//!
//! WebView2 builds its pages asynchronously: a `WebView` exists at once, and what is asked of it
//! before its page is ready waits in a queue. Scripts and screenshots go through the DevTools
//! protocol (`Runtime.evaluate`, `Page.captureScreenshot`): it awaits promises, ignores the page's
//! Content-Security-Policy and reaches isolated worlds — what `callAsyncJavaScript` and
//! `WKContentWorld` give on macOS.
//!
//! The runtime ships with Windows 10 and 11; where it is missing, [`available`] says so and the
//! browser panel offers to install it ([`install_runtime`]).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use serde_json::{json, Value};
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::{
    take_pwstr, AcceleratorKeyPressedEventHandler, AddScriptToExecuteOnDocumentCreatedCompletedHandler,
    CallDevToolsProtocolMethodCompletedHandler, ClearBrowsingDataCompletedHandler, ContentLoadingEventHandler,
    CoreWebView2EnvironmentOptions, CreateCoreWebView2ControllerCompletedHandler, CreateCoreWebView2EnvironmentCompletedHandler,
    DownloadStartingEventHandler, GetCookiesCompletedHandler, GetProcessExtendedInfosCompletedHandler, HistoryChangedEventHandler,
    NavigationCompletedEventHandler, NavigationStartingEventHandler, NewWindowRequestedEventHandler, ProcessFailedEventHandler,
};
use windows::core::{Interface, BOOL, HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::Graphics::Gdi::{GetSysColorBrush, COLOR_WINDOW};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{GetCurrentProcessId, GetCurrentThreadId};
use windows::Win32::UI::Accessibility::{SetWinEventHook, HWINEVENTHOOK};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    EnableWindow, GetFocus, GetKeyState, SetActiveWindow, SetFocus, VK_CONTROL, VK_MENU, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, IsChild, RegisterClassW, SetWindowPos, ShowWindow, EVENT_OBJECT_LOCATIONCHANGE,
    OBJID_WINDOW, SWP_NOACTIVATE, SWP_NOOWNERZORDER, SWP_NOZORDER, SW_HIDE, SW_SHOWNA, WINEVENT_OUTOFCONTEXT, WNDCLASSW, WS_CLIPCHILDREN,
    WS_EX_TOOLWINDOW, WS_POPUP,
};

use crate::platform::page_scripts::{navigation_allowed, CONSOLE_CAPTURE, NETWORK_CAPTURE};
pub use crate::platform::page_scripts::{NET_DETAIL, NET_ENTRIES, NET_TOTALS};
pub use crate::platform::url::normalize_url_with;
use crate::platform::webview2_logic::{
    browser_key_for, devtools_error, evaluate_answer, major_version, profile_name, screenshot_png, script_of, signed_by_microsoft,
    world_is_gone,
};

/// Result of an asynchronous web view call, delivered on the main thread.
pub type Reply = Box<dyn FnOnce(Result<String, String>)>;

/// A page that could not be opened (no server answering, unknown host, TLS error, …).
#[derive(Clone, Debug, PartialEq)]
pub struct LoadError {
    pub url: String,
    pub message: String,
}

/// A browser shortcut pressed while the page had the keyboard. The page's window takes the keys,
/// so GPUI never sees them: they are recorded here and the browser panel acts on them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserKey {
    Reload,
    HardReload,
    NewTab,
    CloseTab,
    FocusAddress,
    Back,
    Forward,
}

/// A standard editing command, as the Edit menu names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditCommand {
    Copy,
    Paste,
    SelectAll,
}

/// A browser profile: a store of cookies and site data of its own, named by a UUID. `None` is the
/// in-app browser's own store.
pub type Profile = Option<[u8; 16]>;

/// One cookie of the in-app browser.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    /// Seconds since the Unix epoch; `None` for a cookie that ends with the browser session.
    pub expires: Option<f64>,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: Option<String>,
}

/// Size of a parked page: a laptop's window, so sites lay out their desktop version.
const PARKED_WIDTH: f64 = 1280.;
const PARKED_HEIGHT: f64 = 900.;

/// Where the Evergreen WebView2 Runtime's installer is published (Microsoft's own link).
pub const RUNTIME_DOWNLOAD: &str = "https://go.microsoft.com/fwlink/p/?LinkId=2124703";

/// The page, once WebView2 has made it.
#[derive(Clone)]
struct Page {
    controller: ICoreWebView2Controller,
    webview: ICoreWebView2,
}

type Queued = Box<dyn FnOnce(Option<&Page>)>;

/// What a page's event handlers and the queue share with its [`WebView`].
struct Inner {
    /// The page's window, and Agentty's window that owns it.
    host: HWND,
    owner: HWND,
    /// Where the page is over Agentty's window (client pixels) while it is on screen; `None`
    /// while it is parked or hidden. The host follows it when the window moves.
    frame: Option<(i32, i32, i32, i32)>,
    page: Option<Page>,
    /// WebView2 could not make the page; queued work is answered with an error.
    failed: bool,
    queue: Vec<Queued>,
    loading: bool,
    progress: f32,
    load_error: Option<LoadError>,
    popups: Vec<String>,
    /// Isolated worlds made in the current document, by name: the execution context's id.
    worlds: HashMap<String, i64>,
    zoom: f64,
    /// Size of the host window in physical pixels, which the page fills.
    size: (i32, i32),
    shown: bool,
    profile: Profile,
    private: bool,
    popups_allowed: bool,
    /// The user agent WebView2 sends on its own (the phone one replaces it while mobile is on).
    default_agent: Option<String>,
    mobile: bool,
    /// The navigation under way turned into a download: its end is not a failed load.
    downloading: bool,
    /// WebView2 said a navigation started (`NavigationStarting`) since the last load was asked for.
    navigating: bool,
}

enum Environment {
    Idle,
    Creating(Vec<Box<dyn FnOnce(Option<ICoreWebView2Environment>)>>),
    Ready(ICoreWebView2Environment),
}

thread_local! {
    /// The one WebView2 environment (browser process) all pages share.
    static ENVIRONMENT: RefCell<Environment> = const { RefCell::new(Environment::Idle) };
    /// Every web view that exists right now, by host window.
    static VIEWS: RefCell<Vec<(usize, Weak<RefCell<Inner>>)>> = const { RefCell::new(Vec::new()) };
    /// Browser shortcuts pressed inside a page, oldest first, with the view they came from.
    static KEYS: RefCell<Vec<(usize, BrowserKey)>> = const { RefCell::new(Vec::new()) };
    /// Cookies to put back once a page of the default store exists (WebView2 has no cookie store
    /// without one).
    static PENDING_COOKIES: RefCell<Vec<Cookie>> = const { RefCell::new(Vec::new()) };
    /// "Clear website data" asked for while no page of the default store existed.
    static CLEAR_PENDING: Cell<bool> = const { Cell::new(false) };
    /// Whether the runtime was found (only a yes is kept: it may be installed while Agentty runs).
    static RUNTIME_FOUND: Cell<bool> = const { Cell::new(false) };
    /// The renderer process of each page's main frame (by frame id), as last asked of WebView2.
    static RENDERERS: RefCell<HashMap<u32, i32>> = RefCell::new(HashMap::new());
    /// Work run on the next turn of the message loop (see [`defer`]).
    static DEFERRED: RefCell<Vec<Box<dyn FnOnce()>>> = const { RefCell::new(Vec::new()) };
    /// When [`RENDERERS`] was last asked for (it answers asynchronously).
    static RENDERERS_ASKED: Cell<Option<std::time::Instant>> = const { Cell::new(None) };
}

/// Asks WebView2 again which renderer process runs which frame (at most every few seconds).
fn refresh_renderers() {
    const EVERY: std::time::Duration = std::time::Duration::from_secs(5);
    if RENDERERS_ASKED.with(Cell::get).is_some_and(|at| at.elapsed() < EVERY) {
        return;
    }
    let env = ENVIRONMENT.with(|state| match &*state.borrow() {
        Environment::Ready(env) => env.cast::<ICoreWebView2Environment13>().ok(),
        _ => None,
    });
    let Some(env) = env else { return };
    RENDERERS_ASKED.with(|at| at.set(Some(std::time::Instant::now())));
    let handler = GetProcessExtendedInfosCompletedHandler::create(Box::new(|_, list| {
        let Some(list) = list else { return Ok(()) };
        let mut found = HashMap::new();
        // SAFETY: reading the collection WebView2 handed over.
        unsafe {
            let mut count = 0u32;
            list.Count(&mut count)?;
            for index in 0..count {
                let Ok(info) = list.GetValueAtIndex(index) else { continue };
                let Ok(process) = info.ProcessInfo() else { continue };
                let mut kind = COREWEBVIEW2_PROCESS_KIND::default();
                let mut pid = 0i32;
                if process.Kind(&mut kind).is_err() || kind != COREWEBVIEW2_PROCESS_KIND_RENDERER || process.ProcessId(&mut pid).is_err() {
                    continue;
                }
                let Ok(frames) = info.AssociatedFrameInfos().and_then(|f| f.GetIterator()) else { continue };
                loop {
                    let mut has = BOOL::default();
                    if frames.HasCurrent(&mut has).is_err() || !has.as_bool() {
                        break;
                    }
                    if let Ok(frame) = frames.GetCurrent().and_then(|f| f.cast::<ICoreWebView2FrameInfo2>()) {
                        let mut id = 0u32;
                        if frame.FrameId(&mut id).is_ok() {
                            found.insert(id, pid);
                        }
                    }
                    let mut next = BOOL::default();
                    if frames.MoveNext(&mut next).is_err() || !next.as_bool() {
                        break;
                    }
                }
            }
        }
        RENDERERS.with(|renderers| *renderers.borrow_mut() = found);
        Ok(())
    }));
    // SAFETY: a valid handler for the duration of the call.
    let _ = unsafe { env.GetProcessExtendedInfos(&handler) };
}

/// The installed WebView2 Runtime's version, e.g. `128.0.2739.42`.
fn runtime_version() -> Option<String> {
    let mut version = PWSTR::null();
    // SAFETY: a null folder asks for the installed Evergreen runtime; `version` receives a string
    // the loader allocated, which `take_pwstr` frees.
    let found = unsafe { GetAvailableCoreWebView2BrowserVersionString(PCWSTR::null(), &mut version) };
    let text = if version.is_null() { String::new() } else { take_pwstr(version) };
    (found.is_ok() && !text.is_empty()).then_some(text)
}

/// Whether the WebView2 Runtime is installed, so the in-app browser can open pages.
pub fn available() -> bool {
    if RUNTIME_FOUND.with(Cell::get) {
        return true;
    }
    let found = runtime_version().is_some();
    RUNTIME_FOUND.with(|cell| cell.set(found));
    found
}

fn runtime_major() -> u32 {
    runtime_version().map_or(0, |v| major_version(&v))
}

/// Downloads Microsoft's WebView2 Runtime installer, checks that Microsoft signed it and runs it.
/// Blocking: call it off the main thread.
pub fn install_runtime() -> Result<(), String> {
    use std::io::Read;
    const LIMIT: u64 = 16 * 1024 * 1024;
    let dir = std::env::temp_dir().join("agentty-webview2-setup");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("MicrosoftEdgeWebview2Setup.exe");
    let _ = std::fs::remove_file(&path);
    let response =
        agentty_bridge::http::agent_builder().build().get(RUNTIME_DOWNLOAD).call().map_err(|e| format!("download failed: {e}"))?;
    let mut bytes = Vec::new();
    response.into_reader().take(LIMIT + 1).read_to_end(&mut bytes).map_err(|e| format!("download failed: {e}"))?;
    if bytes.len() as u64 > LIMIT || bytes.len() < 1024 || !bytes.starts_with(b"MZ") {
        return Err("the download is not an installer".into());
    }
    std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
    // Only an installer Microsoft signed runs: the Authenticode signature must be valid and its
    // signer Microsoft Corporation.
    let check = agentty_bridge::process::windows_powershell()
        .args(["-NoProfile", "-NonInteractive", "-Command"])
        .arg("$s = Get-AuthenticodeSignature -LiteralPath $env:AGENTTY_SETUP; \"$($s.Status)|$($s.SignerCertificate.Subject)\"")
        .env("AGENTTY_SETUP", &path)
        .output()
        .map_err(|e| format!("could not check the installer's signature: {e}"))?;
    let verdict = String::from_utf8_lossy(&check.stdout).trim().to_string();
    let (status, subject) = verdict.split_once('|').unwrap_or((&verdict, ""));
    if status != "Valid" || !signed_by_microsoft(subject) {
        let _ = std::fs::remove_file(&path);
        return Err("the installer is not signed by Microsoft; it was discarded".into());
    }
    let status = agentty_bridge::process::command(&path)
        .args(["/silent", "/install"])
        .status()
        .map_err(|e| format!("could not start the installer: {e}"))?;
    let _ = std::fs::remove_file(&path);
    if runtime_version().is_some() {
        Ok(())
    } else {
        Err(format!("the installer ended without installing the runtime ({status})"))
    }
}

/// Where the in-app browser keeps cookies, cache and site data.
fn data_folder() -> std::path::PathBuf {
    agentty_bridge::fsutil::data_dir().join("webview2")
}

/// Runs `then` with the shared environment once it exists (`None` when it cannot be made).
fn with_environment(then: impl FnOnce(Option<ICoreWebView2Environment>) + 'static) {
    let start = ENVIRONMENT.with(|state| {
        let mut state = state.borrow_mut();
        match &mut *state {
            Environment::Ready(env) => {
                let env = env.clone();
                drop(state);
                then(Some(env));
                false
            }
            Environment::Creating(waiting) => {
                waiting.push(Box::new(then));
                false
            }
            Environment::Idle => {
                *state = Environment::Creating(vec![Box::new(then)]);
                true
            }
        }
    });
    if start {
        create_environment();
    }
}

fn environment_done(env: Option<ICoreWebView2Environment>) {
    let waiting = ENVIRONMENT.with(|state| {
        let mut state = state.borrow_mut();
        let previous = std::mem::replace(
            &mut *state,
            match &env {
                Some(env) => Environment::Ready(env.clone()),
                // Tried again by the next page (the runtime may be installed or repaired meanwhile).
                None => Environment::Idle,
            },
        );
        match previous {
            Environment::Creating(waiting) => waiting,
            _ => Vec::new(),
        }
    });
    for then in waiting {
        then(env.clone());
    }
}

fn create_environment() {
    let folder = data_folder();
    let _ = std::fs::create_dir_all(&folder);
    let options = CoreWebView2EnvironmentOptions::default();
    // Pages out of sight keep running, as they do on macOS: an agent tests a page behind another
    // terminal's, a plugin reads one nobody looks at. Chromium would otherwise throttle timers
    // and stop drawing a page it thinks is covered.
    // SAFETY: the options object is not shared yet.
    unsafe {
        options.set_additional_browser_arguments(
            "--disable-features=CalculateNativeWinOcclusion --disable-background-timer-throttling \
             --disable-renderer-backgrounding --disable-backgrounding-occluded-windows"
                .into(),
        );
    }
    let options: ICoreWebView2EnvironmentOptions = options.into();
    let handler = CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(|result, env| {
        if let Err(error) = &result {
            eprintln!("agentty: the in-app browser could not start: {error}");
        }
        environment_done(result.ok().and(env));
        Ok(())
    }));
    // SAFETY: all arguments are valid COM objects / strings for the duration of the call.
    let started =
        unsafe { CreateCoreWebView2EnvironmentWithOptions(PCWSTR::null(), &HSTRING::from(folder.as_os_str()), &options, &handler) };
    if let Err(error) = started {
        eprintln!("agentty: the in-app browser could not start: {error}");
        environment_done(None);
    }
}

/// Whether this runtime keeps stores apart by profile (WebView2 Runtime 101 and later).
pub fn profiles_supported() -> bool {
    available() && runtime_major() >= 101
}

fn host_class() -> PCWSTR {
    static REGISTER: std::sync::Once = std::sync::Once::new();
    const NAME: PCWSTR = windows::core::w!("AgenttyWebHost");
    REGISTER.call_once(|| {
        // SAFETY: registering a window class with a static name and a valid window procedure.
        unsafe {
            let instance = GetModuleHandleW(None).map(|m| m.into()).unwrap_or_default();
            let class = WNDCLASSW {
                lpfnWndProc: Some(host_proc),
                hInstance: instance,
                lpszClassName: NAME,
                hbrBackground: GetSysColorBrush(COLOR_WINDOW),
                ..Default::default()
            };
            RegisterClassW(&class);
        }
    });
    NAME
}

extern "system" fn host_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::{GetWindow, PostMessageW, GW_OWNER, WM_CLOSE};
    // Alt+F4 while a page has the keyboard closes the page's window, which is the active one: it
    // goes to Agentty's window instead, as it would on macOS. The page's window is only ever
    // destroyed by its view.
    if message == WM_CLOSE {
        // SAFETY: plain calls on our own window and the one that owns it.
        unsafe {
            if let Ok(owner) = GetWindow(hwnd, GW_OWNER) {
                let _ = PostMessageW(Some(owner), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
        return LRESULT(0);
    }
    // SAFETY: forwarding the message this window received.
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

fn window_hwnd(window: &gpui::Window) -> Option<HWND> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(HWND(handle.hwnd.get() as *mut _)),
        _ => None,
    }
}

/// Physical pixels per GPUI pixel in `hwnd`'s monitor.
fn scale_of(hwnd: HWND) -> f64 {
    // SAFETY: a plain query on a window handle.
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    if dpi == 0 {
        1.
    } else {
        dpi as f64 / 96.
    }
}

/// A point of `window`'s client area on the screen.
fn client_to_screen(window: HWND, x: i32, y: i32) -> (i32, i32) {
    let mut point = POINT { x, y };
    // SAFETY: a plain conversion on a window handle.
    let _ = unsafe { ClientToScreen(window, &mut point) };
    (point.x, point.y)
}

/// Makes Agentty's window the active one again and gives it the keyboard. The page's window is a
/// top-level window of its own, so clicking into it made it the active one.
fn give_keyboard_to(owner: HWND) {
    // SAFETY: activating and focusing Agentty's own window, on its thread.
    unsafe {
        let _ = SetActiveWindow(owner);
        let _ = SetFocus(Some(owner));
    }
}

/// Starts following Agentty's windows (once per thread): when one moves, the pages over it move
/// with it. Its frames are drawn only when something changes, not while it is dragged around, so
/// the frame-by-frame placement alone would leave the pages behind.
fn follow_owner() {
    thread_local! {
        static HOOK: Cell<bool> = const { Cell::new(false) };
    }
    if HOOK.with(|hook| hook.replace(true)) {
        return;
    }
    // SAFETY: an out-of-context hook for this thread's windows only; its callback runs on this
    // thread, from the message loop, and lives as long as the process.
    unsafe {
        SetWinEventHook(
            EVENT_OBJECT_LOCATIONCHANGE,
            EVENT_OBJECT_LOCATIONCHANGE,
            None,
            Some(owner_moved),
            GetCurrentProcessId(),
            GetCurrentThreadId(),
            WINEVENT_OUTOFCONTEXT,
        );
    }
}

unsafe extern "system" fn owner_moved(_: HWINEVENTHOOK, _: u32, window: HWND, object: i32, child: i32, _: u32, _: u32) {
    // The window itself (not its caret or a part of it).
    if object != OBJID_WINDOW.0 || child != 0 || window.is_invalid() {
        return;
    }
    for (_, inner) in live_views() {
        let Ok(state) = inner.try_borrow() else { continue };
        if state.owner != window {
            continue;
        }
        let page = state.page.clone();
        if let Some((x, y, w, h)) = state.frame {
            let (x, y) = client_to_screen(window, x, y);
            let host = state.host;
            drop(state);
            // SAFETY: moving our own window along with the one that owns it.
            let _ = unsafe { SetWindowPos(host, None, x, y, w, h, SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOOWNERZORDER) };
        }
        // Drop-downs and other pop-ups a page opens are placed from where its window is.
        if let Some(page) = page {
            // SAFETY: a plain call on a live page.
            let _ = unsafe { page.controller.NotifyParentWindowPositionChanged() };
        }
    }
}

fn is_inside(host: HWND, focus: HWND) -> bool {
    // SAFETY: plain queries on window handles.
    !focus.is_invalid() && (focus == host || unsafe { IsChild(host, focus) }.as_bool())
}

fn read(get: impl FnOnce(&mut PWSTR) -> windows::core::Result<()>) -> Option<String> {
    let mut value = PWSTR::null();
    get(&mut value).ok()?;
    (!value.is_null()).then(|| take_pwstr(value))
}

fn push_key(view: usize, key: BrowserKey) {
    KEYS.with(|keys| {
        let mut keys = keys.borrow_mut();
        if keys.len() < 32 {
            keys.push((view, key));
        }
    });
}

/// Shortcuts pressed in one of `views` since the last call, oldest first.
pub fn take_key_commands(views: &[usize]) -> Vec<(usize, BrowserKey)> {
    KEYS.with(|keys| {
        let mut keys = keys.borrow_mut();
        let (mine, others) = keys.iter().partition(|(view, _)| views.contains(view));
        *keys = others;
        mine
    })
}

/// Live pages, oldest first.
fn live_views() -> Vec<(usize, Rc<RefCell<Inner>>)> {
    VIEWS.with(|views| views.borrow().iter().filter_map(|(id, inner)| Some((*id, inner.upgrade()?))).collect())
}

/// Whether any web view exists.
pub fn any_view() -> bool {
    !live_views().is_empty()
}

/// A ready page of `profile`'s store (not a private one), to reach that store's cookies through.
fn page_of(profile: Profile) -> Option<Page> {
    live_views().into_iter().find_map(|(_, inner)| {
        let inner = inner.try_borrow().ok()?;
        (inner.profile == profile && !inner.private).then(|| inner.page.clone()).flatten()
    })
}

/// Calls a DevTools protocol method on `webview`; `done` gets the result object.
fn devtools(webview: &ICoreWebView2, method: &str, params: Value, done: impl FnOnce(Result<Value, String>) + 'static) {
    let done = Rc::new(RefCell::new(Some(done)));
    let answer = done.clone();
    let handler = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |result, json| {
        let Some(done) = answer.borrow_mut().take() else { return Ok(()) };
        done(match result {
            Ok(()) => Ok(serde_json::from_str(&json).unwrap_or(Value::Null)),
            Err(error) => Err(devtools_error(&json).unwrap_or_else(|| error.message().to_string())),
        });
        Ok(())
    }));
    // SAFETY: valid strings and handler for the duration of the call.
    let sent = unsafe { webview.CallDevToolsProtocolMethod(&HSTRING::from(method), &HSTRING::from(params.to_string()), &handler) };
    if let Err(error) = sent {
        if let Some(done) = done.borrow_mut().take() {
            done(Err(error.message().to_string()));
        }
    }
}

/// `Runtime.evaluate` of `expression` (awaited), in `context` or the page's own world. Answers
/// the returned string, or `null` for anything else — what WebKit's `callAsyncJavaScript` gives.
fn evaluate(webview: &ICoreWebView2, expression: &str, context: Option<i64>, done: impl FnOnce(Result<String, String>) + 'static) {
    let mut params = json!({ "expression": expression, "awaitPromise": true, "returnByValue": true, "userGesture": true });
    if let Some(context) = context {
        params["contextId"] = json!(context);
    }
    devtools(webview, "Runtime.evaluate", params, move |result| done(result.and_then(|value| evaluate_answer(&value))));
}

/// The isolated world `name` of the page's current document, made the first time it is asked for.
fn world_context(page: &Page, inner: &Weak<RefCell<Inner>>, name: &str, done: impl FnOnce(Result<i64, String>) + 'static) {
    if let Some(id) = inner.upgrade().and_then(|i| i.try_borrow().ok()?.worlds.get(name).copied()) {
        return done(Ok(id));
    }
    let webview = page.webview.clone();
    let inner = inner.clone();
    let name = name.to_string();
    devtools(&page.webview, "Page.getFrameTree", json!({}), move |tree| {
        let frame = match tree.map(|t| t.pointer("/frameTree/frame/id").and_then(Value::as_str).map(str::to_string)) {
            Ok(Some(frame)) => frame,
            Ok(None) => return done(Err("the page has no frame".into())),
            Err(error) => return done(Err(error)),
        };
        let params = json!({ "frameId": frame, "worldName": name, "grantUniveralAccess": false });
        devtools(&webview, "Page.createIsolatedWorld", params, move |made| {
            let id = match made.map(|m| m.get("executionContextId").and_then(Value::as_i64)) {
                Ok(Some(id)) => id,
                Ok(None) => return done(Err("the page refused a script world".into())),
                Err(error) => return done(Err(error)),
            };
            if let Some(inner) = inner.upgrade() {
                if let Ok(mut state) = inner.try_borrow_mut() {
                    state.worlds.insert(name, id);
                }
            }
            done(Ok(id))
        });
    });
}

/// Runs `expression` in world `name` (or the page's), making the world again once if the
/// document it lived in is gone.
fn run_script(page: Page, inner: Weak<RefCell<Inner>>, expression: String, world: Option<String>, retry: bool, reply: Reply) {
    let Some(name) = world else {
        return evaluate(&page.webview, &expression, None, reply);
    };
    let again = page.clone();
    let (weak, world_name) = (inner.clone(), name.clone());
    world_context(&page, &weak, &world_name, move |context| {
        let context = match context {
            Ok(context) => context,
            Err(error) => return reply(Err(error)),
        };
        let webview = again.webview.clone();
        evaluate(&webview, &expression.clone(), Some(context), move |result| match result {
            // The world's document is gone (the script did not run): made again once. Any other
            // error, "context destroyed" during the run included, is the script's answer.
            Err(error) if retry && world_is_gone(&error) => {
                if let Some(state) = inner.upgrade() {
                    if let Ok(mut state) = state.try_borrow_mut() {
                        state.worlds.remove(&name);
                    }
                }
                run_script(again, inner, expression, Some(name), false, reply)
            }
            other => reply(other),
        })
    });
}

/// Writes a `Page.captureScreenshot` answer to `path`; answers the path as JSON, as on macOS.
fn write_png(path: std::path::PathBuf, result: Result<Value, String>) -> Result<String, String> {
    let bytes = screenshot_png(&result?)?;
    std::fs::write(&path, bytes).map_err(|_| "could not write the PNG".to_string())?;
    Ok(json!(path.display().to_string()).to_string())
}

/// Why a load failed, in words.
fn error_message(status: COREWEBVIEW2_WEB_ERROR_STATUS) -> &'static str {
    match status {
        COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_COMMON_NAME_IS_INCORRECT
        | COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_EXPIRED
        | COREWEBVIEW2_WEB_ERROR_STATUS_CLIENT_CERTIFICATE_CONTAINS_ERRORS
        | COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_REVOKED
        | COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_IS_INVALID => "The site's certificate is not valid.",
        COREWEBVIEW2_WEB_ERROR_STATUS_SERVER_UNREACHABLE | COREWEBVIEW2_WEB_ERROR_STATUS_CANNOT_CONNECT => {
            "Could not connect to the server."
        }
        COREWEBVIEW2_WEB_ERROR_STATUS_TIMEOUT => "The request timed out.",
        COREWEBVIEW2_WEB_ERROR_STATUS_HOST_NAME_NOT_RESOLVED => "A server with the specified hostname could not be found.",
        COREWEBVIEW2_WEB_ERROR_STATUS_DISCONNECTED | COREWEBVIEW2_WEB_ERROR_STATUS_CONNECTION_RESET => "The network connection was lost.",
        COREWEBVIEW2_WEB_ERROR_STATUS_CONNECTION_ABORTED => "The connection was aborted.",
        COREWEBVIEW2_WEB_ERROR_STATUS_ERROR_HTTP_INVALID_SERVER_RESPONSE => "The server's response was not valid.",
        COREWEBVIEW2_WEB_ERROR_STATUS_REDIRECT_FAILED => "Too many redirects.",
        _ => "The page could not be opened.",
    }
}

/// Which browser shortcut a key down in a page is, if any.
fn browser_key(key: u32) -> Option<BrowserKey> {
    // SAFETY: plain keyboard state queries on the thread that received the key.
    let down = |vk: u16| unsafe { GetKeyState(vk as i32) } < 0;
    browser_key_for(key, down(VK_CONTROL.0), down(VK_SHIFT.0), down(VK_MENU.0))
}

/// A phone's user agent with this runtime's Chromium version.
fn mobile_agent() -> String {
    let major = runtime_major().max(120);
    format!("Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{major}.0.0.0 Mobile Safari/537.36")
}

pub struct WebView {
    host: HWND,
    parent: HWND,
    inner: Rc<RefCell<Inner>>,
    visible: bool,
    /// Outside the window but shown: see [`WebView::park`].
    parked: bool,
    /// A plugin's page: taken out of sight it is parked, never hidden, so it keeps running.
    keep_running: bool,
    locked: bool,
    /// Size a terminal's page is laid out at while parked, at zoom 1 (see [`WebView::park_as`]).
    parked_layout: Option<(f64, f64)>,
    /// Where the host was last put (client pixels on screen, or the parked size), so an unchanged
    /// frame costs nothing.
    placed: Option<Placement>,
}

#[derive(Clone, Copy, PartialEq)]
enum Placement {
    /// Over Agentty's window: `x, y` in its client area.
    Over(i32, i32, i32, i32),
    /// Parked off every screen, `w × h`.
    Parked(i32, i32),
}

/// Screen position of parked pages: far from any monitor, but on the coordinate range Windows
/// keeps windows in.
const PARK_AT: i32 = -30000;

impl WebView {
    /// Creates the web view inside `window` (hidden until `set_frame`).
    pub fn new(window: &gpui::Window, prefs: &crate::settings::BrowserSettings) -> Option<Self> {
        Self::create(window, prefs, None)
    }

    /// A web view a plugin drives while nobody looks at it: parked outside the window, where it is
    /// never drawn but keeps running.
    pub fn new_background(window: &gpui::Window, prefs: &crate::settings::BrowserSettings) -> Option<Self> {
        Self::new_background_in(window, prefs, None)
    }

    /// [`new_background`] with `profile`'s cookies and site data (see [`Profile`]).
    pub fn new_background_in(window: &gpui::Window, prefs: &crate::settings::BrowserSettings, profile: Profile) -> Option<Self> {
        let mut view = Self::create(window, prefs, profile)?;
        view.keep_running = true;
        view.park();
        Some(view)
    }

    fn create(window: &gpui::Window, prefs: &crate::settings::BrowserSettings, profile: Profile) -> Option<Self> {
        if !available() || (profile.is_some() && !profiles_supported()) {
            return None;
        }
        let parent = window_hwnd(window)?;
        follow_owner();
        // SAFETY: a window Agentty's window owns (a pop-up, not a child), with a registered class;
        // a tool window, so it is never in the taskbar or Alt+Tab.
        let host = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                host_class(),
                PCWSTR::null(),
                WS_POPUP | WS_CLIPCHILDREN,
                PARK_AT,
                PARK_AT,
                10,
                10,
                Some(parent),
                None,
                GetModuleHandleW(None).ok().map(Into::into),
                None,
            )
        }
        .ok()?;
        let zoom = prefs.zoom.clamp(0.3, 3.0) as f64;
        let inner = Rc::new(RefCell::new(Inner {
            host,
            owner: parent,
            frame: None,
            page: None,
            failed: false,
            queue: Vec::new(),
            loading: false,
            progress: 0.,
            load_error: None,
            popups: Vec::new(),
            worlds: HashMap::new(),
            zoom,
            size: (10, 10),
            shown: false,
            profile,
            private: prefs.private_mode,
            popups_allowed: prefs.popups,
            default_agent: None,
            mobile: prefs.mobile,
            downloading: false,
            navigating: false,
        }));
        VIEWS.with(|views| views.borrow_mut().push((host.0 as usize, Rc::downgrade(&inner))));
        start_page(host, Rc::downgrade(&inner), prefs.clone());
        Some(Self {
            host,
            parent,
            inner,
            visible: false,
            parked: false,
            keep_running: false,
            locked: false,
            parked_layout: None,
            placed: None,
        })
    }

    /// Runs `op` on the page now, or once it is ready.
    fn with_page(&self, op: impl FnOnce(&Page) + 'static) {
        self.with_page_or(move |page| {
            if let Some(page) = page {
                op(page)
            }
        })
    }

    /// Like [`with_page`], but `op` also hears (with `None`) that the page could not be made.
    fn with_page_or(&self, op: impl FnOnce(Option<&Page>) + 'static) {
        let mut inner = self.inner.borrow_mut();
        if let Some(page) = inner.page.clone() {
            drop(inner);
            op(Some(&page));
        } else if inner.failed {
            drop(inner);
            op(None);
        } else {
            inner.queue.push(Box::new(op));
        }
    }

    /// Identifies this view in [`take_key_commands`].
    pub fn id(&self) -> usize {
        self.host.0 as usize
    }

    /// The process WebView2 runs this page's content in: what the in-app browser's memory limit
    /// measures. `None` until WebView2 has said (it answers asynchronously), or on a runtime too
    /// old to say.
    pub fn web_process_id(&self) -> Option<i32> {
        let page = self.inner.borrow().page.clone()?;
        let mut frame = 0u32;
        // SAFETY: plain queries on a live page.
        unsafe { page.webview.cast::<ICoreWebView2_20>().ok()?.FrameId(&mut frame).ok()? };
        refresh_renderers();
        RENDERERS.with(|renderers| renderers.borrow().get(&frame).copied())
    }

    /// Whether the keyboard is inside this page (it then gets the browser shortcuts).
    pub fn has_keyboard(&self) -> bool {
        // SAFETY: a plain query.
        is_inside(self.host, unsafe { GetFocus() })
    }

    /// Marks a load as started now, as WebKit's `isLoading` turns true as soon as it is asked to
    /// load: WebView2 says so only a moment later (`NavigationStarting`), and an agent's
    /// `wait-load` right after `navigate` would otherwise find nothing loading and return at once.
    /// `op` starts the navigation; when it cannot, the mark is taken back.
    fn navigate(&self, op: impl FnOnce(&Page) -> bool + 'static) {
        {
            let mut inner = self.inner.borrow_mut();
            if inner.failed {
                return;
            }
            inner.loading = true;
            inner.navigating = false;
            inner.progress = inner.progress.min(0.05);
        }
        let weak = Rc::downgrade(&self.inner);
        self.with_page(move |page| {
            if !op(page) {
                if let Some(state) = weak.upgrade() {
                    if let Ok(mut state) = state.try_borrow_mut() {
                        state.loading = false;
                    }
                }
            }
        });
    }

    pub fn load(&self, url: &str) {
        let url = HSTRING::from(url);
        // SAFETY: a valid string for the duration of the call.
        self.navigate(move |page| unsafe { page.webview.Navigate(&url) }.is_ok());
    }

    /// Shows `html` as a page of its own, with no address: what it may load is up to its
    /// Content-Security-Policy, and it reads no local files.
    pub fn load_html(&self, html: &str) {
        let html = HSTRING::from(html);
        // SAFETY: a valid string for the duration of the call.
        self.navigate(move |page| unsafe { page.webview.NavigateToString(&html) }.is_ok());
    }

    pub fn back(&self) {
        if self.can_go(true) {
            // SAFETY: a plain call on a live page.
            self.navigate(|page| unsafe { page.webview.GoBack() }.is_ok());
        }
    }

    pub fn forward(&self) {
        if self.can_go(false) {
            // SAFETY: a plain call on a live page.
            self.navigate(|page| unsafe { page.webview.GoForward() }.is_ok());
        }
    }

    /// Whether there is a page to go back (or forward) to: with none, nothing loads.
    fn can_go(&self, back: bool) -> bool {
        let Some(page) = self.inner.borrow().page.clone() else { return false };
        let mut can = BOOL::default();
        // SAFETY: plain queries on a live page.
        let asked = unsafe {
            if back {
                page.webview.CanGoBack(&mut can)
            } else {
                page.webview.CanGoForward(&mut can)
            }
        };
        asked.is_ok() && can.as_bool()
    }

    /// Reloads from the server, not the cache: pages here are mostly dev servers that just changed.
    pub fn reload(&self) {
        let weak = Rc::downgrade(&self.inner);
        self.navigate(move |page| {
            devtools(&page.webview, "Page.reload", json!({ "ignoreCache": true }), move |result| {
                if result.is_err() {
                    stop_loading(&weak);
                }
            });
            true
        });
    }

    /// Ctrl+Shift+R: throws away the caches first, so scripts, styles and images are fetched again.
    pub fn hard_reload(&self) {
        let weak = Rc::downgrade(&self.inner);
        self.navigate(move |page| {
            let webview = page.webview.clone();
            devtools(&page.webview, "Network.clearBrowserCache", json!({}), move |_| {
                devtools(&webview, "Page.reload", json!({ "ignoreCache": true }), move |result| {
                    if result.is_err() {
                        stop_loading(&weak);
                    }
                });
            });
            true
        });
    }

    pub fn current_url(&self) -> Option<String> {
        let page = self.inner.borrow().page.clone()?;
        // SAFETY: a plain query on a live page.
        read(|value| unsafe { page.webview.Source(value) }).filter(|url| !url.is_empty())
    }

    /// Why the last page could not be opened, until the next navigation starts.
    pub fn load_error(&self) -> Option<LoadError> {
        self.inner.borrow().load_error.clone()
    }

    /// Addresses this page asked to open in a new window since the last call.
    pub fn take_popups(&self) -> Vec<String> {
        std::mem::take(&mut self.inner.borrow_mut().popups)
    }

    pub fn title(&self) -> Option<String> {
        let page = self.inner.borrow().page.clone()?;
        // SAFETY: a plain query on a live page.
        read(|value| unsafe { page.webview.DocumentTitle(value) }).filter(|t| !t.is_empty())
    }

    /// Runs `body` as the body of an async JavaScript function in the page (not subject to the
    /// page's CSP) with `args` as its variables; the returned value is sent as JSON.
    pub fn call_async(&self, body: &str, args: &[(&str, &str)], reply: Reply) {
        self.call_in_world(body, args, None, reply)
    }

    /// [`call_async`] in a world of its own (`Some(name)`): the page's scripts share the DOM with
    /// it but cannot see or replace its variables and functions.
    pub fn call_in_world(&self, body: &str, args: &[(&str, &str)], world: Option<&str>, reply: Reply) {
        let reply = later(reply);
        let expression = match script_of(body, args) {
            Ok(expression) => expression,
            Err(error) => return reply(Err(error)),
        };
        let world = world.map(str::to_string);
        let inner = Rc::downgrade(&self.inner);
        self.with_page_or(move |page| match page {
            Some(page) => run_script(page.clone(), inner, expression, world, true, reply),
            None => reply(Err("the in-app browser could not open this page".into())),
        });
    }

    /// Saves what the page shows as a PNG.
    pub fn snapshot_png(&self, path: std::path::PathBuf, reply: Reply) {
        let reply = later(reply);
        self.with_page_or(move |page| match page {
            Some(page) => {
                devtools(&page.webview, "Page.captureScreenshot", json!({ "format": "png" }), move |result| reply(write_png(path, result)))
            }
            None => reply(Err("snapshot failed".into())),
        });
    }

    /// A small picture of the page, `width` points wide (the Monitoring page's previews).
    pub fn snapshot_png_sized(&self, path: std::path::PathBuf, width: f64, reply: Reply) {
        let reply = later(reply);
        self.with_page_or(move |page| {
            let Some(page) = page else { return reply(Err("snapshot failed".into())) };
            let webview = page.webview.clone();
            devtools(&page.webview, "Page.getLayoutMetrics", json!({}), move |metrics| {
                let metrics = match metrics {
                    Ok(metrics) => metrics,
                    Err(error) => return reply(Err(error)),
                };
                let number = |pointer: &str| metrics.pointer(pointer).and_then(Value::as_f64).unwrap_or(0.);
                let (w, h) = (number("/cssLayoutViewport/clientWidth"), number("/cssLayoutViewport/clientHeight"));
                let (x, y) = (number("/cssVisualViewport/pageX"), number("/cssVisualViewport/pageY"));
                let params = if w > 0. && h > 0. {
                    json!({ "format": "png", "clip": { "x": x, "y": y, "width": w, "height": h, "scale": (width / w).clamp(0.05, 4.) } })
                } else {
                    json!({ "format": "png" })
                };
                devtools(&webview, "Page.captureScreenshot", params, move |result| reply(write_png(path, result)));
            });
        });
    }

    pub fn is_loading(&self) -> bool {
        self.inner.borrow().loading
    }

    /// How far the current load is, 0.0 to 1.0 (an estimate: started, content arriving, done).
    pub fn estimated_progress(&self) -> f32 {
        self.inner.borrow().progress
    }

    /// Puts the host where `placement` says, and the page inside it.
    fn place(&mut self, placement: Placement) {
        if self.placed != Some(placement) {
            self.placed = Some(placement);
            let (frame, (x, y), (w, h)) = match placement {
                Placement::Over(x, y, w, h) => {
                    let (w, h) = (w.max(1), h.max(1));
                    (Some((x, y, w, h)), client_to_screen(self.parent, x, y), (w, h))
                }
                Placement::Parked(w, h) => (None, (PARK_AT, PARK_AT), (w.max(1), h.max(1))),
            };
            // SAFETY: moving our own window; it keeps its place above the window that owns it.
            let _ = unsafe { SetWindowPos(self.host, None, x, y, w, h, SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOOWNERZORDER) };
            let resized = {
                let mut inner = self.inner.borrow_mut();
                inner.frame = frame;
                let resized = inner.size != (w, h);
                inner.size = (w, h);
                resized.then(|| inner.page.clone()).flatten()
            };
            if let Some(page) = resized {
                // SAFETY: a plain call on a live page.
                let _ = unsafe { page.controller.SetBounds(RECT { left: 0, top: 0, right: w, bottom: h }) };
            }
        }
        self.show();
    }

    fn show(&mut self) {
        let (was_shown, page) = {
            let mut inner = self.inner.borrow_mut();
            let was_shown = inner.shown;
            inner.shown = true;
            (was_shown, if was_shown { None } else { inner.page.clone() })
        };
        if !was_shown {
            // SAFETY: showing our own child window without taking the keyboard.
            unsafe {
                let _ = ShowWindow(self.host, SW_SHOWNA);
            }
        }
        if let Some(page) = page {
            // SAFETY: a plain call on a live page.
            let _ = unsafe { page.controller.SetIsVisible(true) };
        }
    }

    /// Places the view over `bounds` (window coordinates, top-left origin) and shows it.
    pub fn set_frame(&mut self, bounds: gpui::Bounds<gpui::Pixels>) {
        let scale = scale_of(self.parent);
        let px = |v: gpui::Pixels| (f32::from(v) as f64 * scale).round() as i32;
        self.parked = false;
        self.visible = true;
        self.place(Placement::Over(px(bounds.origin.x), px(bounds.origin.y), px(bounds.size.width), px(bounds.size.height)));
    }

    /// Page zoom. Responsive mode uses it to lay the page out at a device's width in a smaller
    /// frame: at zoom `z`, a frame `w` pixels wide holds `w / z` CSS pixels.
    pub fn set_zoom(&mut self, zoom: f64) {
        let zoom = zoom.clamp(0.1, 3.0);
        let page = {
            let mut inner = self.inner.borrow_mut();
            if (zoom - inner.zoom).abs() <= 1e-4 {
                return;
            }
            inner.zoom = zoom;
            inner.page.clone()
        };
        if let Some(page) = page {
            // SAFETY: a plain call on a live page.
            let _ = unsafe { page.controller.SetZoomFactor(zoom) };
        }
    }

    /// Asks sites for their phone pages (`true`) or their desktop ones. A page already open is
    /// loaded again to be asked anew.
    pub fn set_mobile(&mut self, mobile: bool) {
        {
            let mut inner = self.inner.borrow_mut();
            if inner.mobile == mobile {
                return;
            }
            inner.mobile = mobile;
        }
        let inner = Rc::downgrade(&self.inner);
        self.with_page(move |page| {
            let Some(inner) = inner.upgrade() else { return };
            let agent = if mobile { Some(mobile_agent()) } else { inner.borrow().default_agent.clone() };
            apply_agent(page, agent.as_deref());
        });
        if self.current_url().is_some_and(|url| url.starts_with("http")) {
            self.reload();
        }
    }

    /// Moves the view outside the window at a desktop size: never drawn, never in the way, but
    /// still shown, so the page keeps running.
    pub fn park(&mut self) {
        if self.parked {
            return;
        }
        self.release_keyboard();
        let scale = scale_of(self.parent);
        let (w, h) = self.parked_layout.unwrap_or((PARKED_WIDTH, PARKED_HEIGHT));
        self.place(Placement::Parked((w * scale).round() as i32, (h * scale).round() as i32));
        self.visible = false;
        self.parked = true;
        // A terminal's page keeps the width it is laid out at, on screen or off.
        if self.parked_layout.is_some() {
            self.set_zoom(1.0);
        }
    }

    /// Lays a terminal's page out at `width` × `height` whenever it is out of sight (parked again
    /// now if it is). `None`: the default desktop size.
    pub fn park_as(&mut self, size: Option<(f64, f64)>) {
        self.parked_layout = Some(size.unwrap_or((PARKED_WIDTH, PARKED_HEIGHT)));
        if self.parked {
            self.parked = false;
            self.park();
        }
    }

    /// Width a terminal's page is laid out at on screen: the desktop size, or the one it was given.
    pub fn layout_width(&self) -> f64 {
        self.parked_layout.map_or(PARKED_WIDTH, |(width, _)| width)
    }

    pub fn hide(&mut self) {
        if self.keep_running {
            // Hiding would suspend the page a plugin is working in: it goes out of the window.
            return self.park();
        }
        if !self.visible && !self.parked {
            return;
        }
        self.release_keyboard();
        // SAFETY: hiding our own child window.
        unsafe {
            let _ = ShowWindow(self.host, SW_HIDE);
        }
        let page = {
            let mut inner = self.inner.borrow_mut();
            inner.shown = false;
            inner.frame = None;
            inner.page.clone()
        };
        if let Some(page) = page {
            // SAFETY: a plain call on a live page.
            let _ = unsafe { page.controller.SetIsVisible(false) };
        }
        self.visible = false;
        self.parked = false;
        self.placed = None;
    }

    /// Locked, the page takes no click, drag, scroll or key from the user: an AI is driving it and
    /// a stray click would get in its way. Its scripts are not touched.
    pub fn set_locked(&mut self, locked: bool) {
        if locked == self.locked {
            return;
        }
        self.locked = locked;
        if locked {
            self.release_keyboard();
        }
        // A disabled window and everything in it take no input.
        // SAFETY: enabling or disabling our own child window.
        unsafe {
            let _ = EnableWindow(self.host, !locked);
        }
    }

    /// Hands the keyboard back to Agentty's window when this page holds it.
    fn release_keyboard(&self) {
        if self.has_keyboard() {
            give_keyboard_to(self.parent);
        }
    }
}

impl Drop for WebView {
    fn drop(&mut self) {
        let id = self.id();
        KEYS.with(|keys| keys.borrow_mut().retain(|(view, _)| *view != id));
        VIEWS.with(|views| views.borrow_mut().retain(|(view, _)| *view != id));
        self.release_keyboard();
        let (page, queue) = {
            let mut inner = self.inner.borrow_mut();
            (inner.page.take(), std::mem::take(&mut inner.queue))
        };
        let making = page.is_none() && !self.inner.borrow().failed;
        // What still waited for the page hears that there will be none (a script's caller gets an
        // error instead of never an answer) — a moment later, not from inside this drop, where the
        // caller may still hold the cell this view is being taken out of.
        answer_later(queue);
        if let Some(page) = page {
            // SAFETY: closing a page we own; its window goes with it.
            let _ = unsafe { page.controller.Close() };
        }
        if making {
            // WebView2 is still making the page inside the host: the host stays (hidden) until
            // it is done, and is destroyed then (`release_host`), so the page is never bound to
            // a window that is gone.
            // SAFETY: hiding our own child window.
            let _ = unsafe { ShowWindow(self.host, SW_HIDE) };
        } else {
            release_host(self.host);
        }
    }
}

/// Runs `ops` with no page on the next turn of the message loop.
fn answer_later(ops: Vec<Queued>) {
    for op in ops {
        defer(move || op(None));
    }
}

/// Runs `work` on the next turn of the message loop, never from inside the call that asked for
/// it. Callers hold `RefCell` borrows around their calls into the web view (the browser panel keeps
/// its network inbox borrowed while it asks for the next poll) and borrow again in their replies:
/// a reply delivered on the spot would find the cell still borrowed, and the app would abort.
/// WebKit never answers on the spot either.
fn defer(work: impl FnOnce() + 'static) {
    use windows::Win32::UI::WindowsAndMessaging::SetTimer;
    let first = DEFERRED.with(|deferred| {
        let mut deferred = deferred.borrow_mut();
        deferred.push(Box::new(work));
        deferred.len() == 1
    });
    if first {
        // SAFETY: a thread timer with a callback; `run_deferred` kills it when it fires.
        unsafe { SetTimer(None, 0, 0, Some(run_deferred)) };
    }
}

extern "system" fn run_deferred(_: HWND, _: u32, timer: usize, _: u32) {
    use windows::Win32::UI::WindowsAndMessaging::KillTimer;
    // SAFETY: killing the thread timer that called this.
    let _ = unsafe { KillTimer(None, timer) };
    let work = DEFERRED.with(|deferred| std::mem::take(&mut *deferred.borrow_mut()));
    for work in work {
        work();
    }
}

/// `reply`, always delivered on a later turn of the message loop (see [`defer`]).
fn later(reply: Reply) -> Reply {
    Box::new(move |result| defer(move || reply(result)))
}

/// A load that could not start after all.
fn stop_loading(inner: &Weak<RefCell<Inner>>) {
    if let Some(state) = inner.upgrade() {
        if let Ok(mut state) = state.try_borrow_mut() {
            state.loading = false;
        }
    }
}

/// Destroys a page's host window.
fn release_host(host: HWND) {
    // SAFETY: destroying our own child window.
    let _ = unsafe { DestroyWindow(host) };
}

/// Page creation failed: the view hears it, or, when the view is already gone, its host goes.
fn fail_or_release(host: HWND, inner: &Weak<RefCell<Inner>>) {
    if inner.strong_count() == 0 {
        release_host(host);
    } else {
        fail(inner);
    }
}

fn apply_agent(page: &Page, agent: Option<&str>) {
    let Some(agent) = agent else { return };
    // SAFETY: plain calls on a live page.
    unsafe {
        if let Ok(settings) = page.webview.Settings().and_then(|s| s.cast::<ICoreWebView2Settings2>()) {
            let _ = settings.SetUserAgent(&HSTRING::from(agent));
        }
    }
}

/// Makes the page for `host` and, once it is ready, runs what waited for it.
fn start_page(host: HWND, inner: Weak<RefCell<Inner>>, prefs: crate::settings::BrowserSettings) {
    with_environment(move |env| {
        let Some(env) = env else { return fail_or_release(host, &inner) };
        let Some(state) = inner.upgrade() else { return release_host(host) };
        let (profile, private) = {
            let state = state.borrow();
            (state.profile, state.private)
        };
        drop(state);
        let made = inner.clone();
        let handler = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(move |result, controller| {
            match result.ok().and(controller) {
                Some(controller) => {
                    if setup_page(host, &made, controller.clone(), &prefs).is_err() {
                        // SAFETY: closing the page that could not be set up, before its host goes.
                        let _ = unsafe { controller.Close() };
                        fail_or_release(host, &made);
                    }
                }
                None => fail_or_release(host, &made),
            }
            Ok(())
        }));
        // SAFETY: `host` is our live child window; the handler lives as long as WebView2 needs it.
        let started = unsafe {
            match env.cast::<ICoreWebView2Environment10>() {
                Ok(env10) => {
                    let options = env10.CreateCoreWebView2ControllerOptions();
                    match options {
                        Ok(options) => {
                            if let Some(bytes) = profile {
                                let _ = options.SetProfileName(&HSTRING::from(profile_name(&bytes)));
                            }
                            let _ = options.SetIsInPrivateModeEnabled(private);
                            env10.CreateCoreWebView2ControllerWithOptions(host, &options, &handler)
                        }
                        Err(error) => Err(error),
                    }
                }
                // A runtime older than profiles: the default store only.
                Err(_) if profile.is_none() => env.CreateCoreWebView2Controller(host, &handler),
                Err(error) => Err(error),
            }
        };
        if started.is_err() {
            fail_or_release(host, &inner);
        }
    });
}

fn fail(inner: &Weak<RefCell<Inner>>) {
    let Some(inner) = inner.upgrade() else { return };
    let queue = {
        let mut state = inner.borrow_mut();
        state.failed = true;
        state.loading = false;
        state.load_error =
            Some(LoadError { url: String::new(), message: "The in-app browser (Microsoft Edge WebView2) could not start.".into() });
        std::mem::take(&mut state.queue)
    };
    for op in queue {
        op(None);
    }
}

fn setup_page(
    host: HWND,
    inner: &Weak<RefCell<Inner>>,
    controller: ICoreWebView2Controller,
    prefs: &crate::settings::BrowserSettings,
) -> windows::core::Result<()> {
    let Some(state) = inner.upgrade() else {
        // The view went away while its page was being made: the page closes first, then the
        // host that was kept for it.
        // SAFETY: closing the page WebView2 just made.
        let _ = unsafe { controller.Close() };
        release_host(host);
        return Ok(());
    };
    // SAFETY: calls on the page WebView2 just made, on its thread.
    unsafe {
        let webview = controller.CoreWebView2()?;
        let settings = webview.Settings()?;
        settings.SetIsScriptEnabled(prefs.javascript)?;
        settings.SetAreDevToolsEnabled(prefs.inspectable)?;
        settings.SetIsStatusBarEnabled(false)?;
        // `alert`, `confirm`, `prompt` and "leave this page?" answer at once (dismissed), as in the
        // WebKit view on macOS, rather than stopping the page behind a window an agent cannot see.
        settings.SetAreDefaultScriptDialogsEnabled(false)?;
        // Ctrl+wheel would zoom the page behind the panel's back (responsive mode sets the zoom).
        settings.SetIsZoomControlEnabled(false)?;
        let default_agent = settings.cast::<ICoreWebView2Settings2>().ok().and_then(|s| read(|value| s.UserAgent(value)));
        {
            let mut state = state.borrow_mut();
            state.default_agent = default_agent;
            controller.SetZoomFactor(state.zoom)?;
            let (w, h) = state.size;
            controller.SetBounds(RECT { left: 0, top: 0, right: w, bottom: h })?;
            controller.SetIsVisible(state.shown)?;
        }
        let id = host.0 as usize;
        let mut token = 0i64;

        let weak = inner.clone();
        webview.add_NavigationStarting(
            &NavigationStartingEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let url = read(|value| args.Uri(value)).unwrap_or_default();
                if !navigation_allowed(&url) {
                    return args.SetCancel(true);
                }
                if let Some(state) = weak.upgrade() {
                    let Ok(mut state) = state.try_borrow_mut() else { return Ok(()) };
                    state.load_error = None;
                    state.loading = true;
                    state.navigating = true;
                    state.downloading = false;
                    state.progress = 0.1;
                    state.worlds.clear();
                }
                Ok(())
            })),
            &mut token,
        )?;

        let weak = inner.clone();
        webview.add_ContentLoading(
            &ContentLoadingEventHandler::create(Box::new(move |_, _| {
                if let Some(state) = weak.upgrade() {
                    let Ok(mut state) = state.try_borrow_mut() else { return Ok(()) };
                    state.progress = state.progress.max(0.5);
                    // A new document: the worlds made in the last one are gone.
                    state.worlds.clear();
                }
                Ok(())
            })),
            &mut token,
        )?;

        // A move within the same document (`#fragment`, `history.pushState`) raises neither
        // `NavigationStarting` nor `NavigationCompleted`: a load asked for that ends up being one is
        // over when the history changes.
        let weak = inner.clone();
        webview.add_HistoryChanged(
            &HistoryChangedEventHandler::create(Box::new(move |_, _| {
                if let Some(state) = weak.upgrade() {
                    let Ok(mut state) = state.try_borrow_mut() else { return Ok(()) };
                    if state.loading && !state.navigating {
                        state.loading = false;
                        state.progress = 1.;
                    }
                }
                Ok(())
            })),
            &mut token,
        )?;

        let weak = inner.clone();
        webview.add_NavigationCompleted(
            &NavigationCompletedEventHandler::create(Box::new(move |sender, args| {
                let Some(state) = weak.upgrade() else { return Ok(()) };
                let mut failure = None;
                if let Some(args) = args {
                    let mut success = BOOL::default();
                    args.IsSuccess(&mut success)?;
                    let mut status = COREWEBVIEW2_WEB_ERROR_STATUS::default();
                    args.WebErrorStatus(&mut status)?;
                    // Cancelled by a newer navigation (a click during a load) is not a failure.
                    if !success.as_bool() && status != COREWEBVIEW2_WEB_ERROR_STATUS_OPERATION_CANCELED {
                        let url = sender.as_ref().and_then(|w| read(|value| w.Source(value))).unwrap_or_default();
                        failure = Some(LoadError { url, message: error_message(status).into() });
                    }
                }
                let Ok(mut state) = state.try_borrow_mut() else { return Ok(()) };
                state.loading = false;
                state.navigating = false;
                state.progress = 1.;
                // A link that turned into a download ends its navigation "aborted"; the page stays.
                let download = std::mem::take(&mut state.downloading);
                if failure.is_some() && !download {
                    state.load_error = failure;
                }
                // Chromium may keep a zoom per site: the page is held at the one Agentty set.
                let (zoom, page) = (state.zoom, state.page.clone());
                drop(state);
                if let Some(page) = page {
                    let mut now = 0f64;
                    if page.controller.ZoomFactor(&mut now).is_ok() && (now - zoom).abs() > 1e-4 {
                        page.controller.SetZoomFactor(zoom)?;
                    }
                }
                Ok(())
            })),
            &mut token,
        )?;

        // A page asked for a new window (`window.open`, `target="_blank"`): no window is made, the
        // address opens in a new tab of the panel instead. Only the web: a page must not put
        // `file:`, `javascript:` or an app's own scheme into a tab of its own accord.
        let weak = inner.clone();
        webview.add_NewWindowRequested(
            &NewWindowRequestedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                args.SetHandled(true)?;
                let url = read(|value| args.Uri(value)).unwrap_or_default();
                let mut clicked = BOOL::default();
                args.IsUserInitiated(&mut clicked)?;
                if let Some(state) = weak.upgrade() {
                    let Ok(mut state) = state.try_borrow_mut() else { return Ok(()) };
                    let allowed = clicked.as_bool() || state.popups_allowed;
                    if allowed && (url.starts_with("http://") || url.starts_with("https://")) && state.popups.len() < 16 {
                        state.popups.push(url);
                    }
                }
                Ok(())
            })),
            &mut token,
        )?;

        if let Ok(webview4) = webview.cast::<ICoreWebView2_4>() {
            let weak = inner.clone();
            webview4.add_DownloadStarting(
                &DownloadStartingEventHandler::create(Box::new(move |_, args| {
                    // Nothing is downloaded, as in WebKit's view on macOS (which has no downloads):
                    // a page an agent drives, signed in as the user, must not drop files on disk.
                    if let Some(args) = args {
                        args.SetCancel(true)?;
                    }
                    if let Some(state) = weak.upgrade() {
                        if let Ok(mut state) = state.try_borrow_mut() {
                            state.downloading = true;
                        }
                    }
                    Ok(())
                })),
                &mut token,
            )?;
        }

        // The page's own process ended or hangs (the page is gone until it is loaded again), or the
        // whole browser process did (then the next page starts a new one). Other helper processes
        // (GPU, utilities) restart by themselves and leave the page alone.
        let weak = inner.clone();
        webview.add_ProcessFailed(
            &ProcessFailedEventHandler::create(Box::new(move |sender, args| {
                let mut kind = COREWEBVIEW2_PROCESS_FAILED_KIND::default();
                if let Some(args) = &args {
                    args.ProcessFailedKind(&mut kind)?;
                }
                let browser_gone = kind == COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED;
                if browser_gone {
                    ENVIRONMENT.with(|state| {
                        if let Ok(mut state) = state.try_borrow_mut() {
                            if matches!(*state, Environment::Ready(_)) {
                                *state = Environment::Idle;
                            }
                        }
                    });
                }
                let page_gone = browser_gone
                    || kind == COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED
                    || kind == COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE;
                if !page_gone {
                    return Ok(());
                }
                if let Some(state) = weak.upgrade() {
                    let url = sender.as_ref().and_then(|w| read(|value| w.Source(value))).unwrap_or_default();
                    let Ok(mut state) = state.try_borrow_mut() else { return Ok(()) };
                    state.loading = false;
                    state.worlds.clear();
                    state.load_error = Some(LoadError { url, message: "The page stopped working.".into() });
                }
                Ok(())
            })),
            &mut token,
        )?;

        controller.add_AcceleratorKeyPressed(
            &AcceleratorKeyPressedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut kind = COREWEBVIEW2_KEY_EVENT_KIND::default();
                args.KeyEventKind(&mut kind)?;
                if kind != COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN && kind != COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN {
                    return Ok(());
                }
                let mut key = 0u32;
                args.VirtualKey(&mut key)?;
                if let Some(command) = browser_key(key) {
                    if crate::debug::enabled() {
                        eprintln!("browser-key: {command:?}");
                    }
                    push_key(id, command);
                    args.SetHandled(true)?;
                }
                Ok(())
            })),
            &mut token,
        )?;

        let page = Page { controller: controller.clone(), webview: webview.clone() };
        // Console output and network calls are kept in the page: `agentty browser console` reads
        // the first, the browser's network panel the second (the top document only).
        let waiting = Rc::new(Cell::new(2u8));
        let network = format!("if (window === window.top) {NETWORK_CAPTURE}");
        for source in [CONSOLE_CAPTURE.to_string(), network] {
            let waiting = waiting.clone();
            let weak = inner.clone();
            let page = page.clone();
            webview.AddScriptToExecuteOnDocumentCreated(
                &HSTRING::from(source),
                &AddScriptToExecuteOnDocumentCreatedCompletedHandler::create(Box::new(move |_, _| {
                    waiting.set(waiting.get().saturating_sub(1));
                    if waiting.get() == 0 {
                        ready(host, &weak, page.clone());
                    }
                    Ok(())
                })),
            )?;
        }
    }
    let _ = &state;
    Ok(())
}

/// The page is ready: what waited for it runs, in the order it was asked.
fn ready(host: HWND, inner: &Weak<RefCell<Inner>>, page: Page) {
    let Some(state) = inner.upgrade() else {
        // The view went away while the page was being set up.
        // SAFETY: closing the page, then the host that was kept for it.
        let _ = unsafe { page.controller.Close() };
        return release_host(host);
    };
    let (queue, agent, default_store, zoom, (w, h), shown) = {
        let mut state = state.borrow_mut();
        state.page = Some(page.clone());
        let agent = state.mobile.then(mobile_agent);
        (std::mem::take(&mut state.queue), agent, state.profile.is_none() && !state.private, state.zoom, state.size, state.shown)
    };
    // Frame, zoom and visibility may have changed while the page was being set up.
    // SAFETY: plain calls on the page that just became ready.
    unsafe {
        let _ = page.controller.SetZoomFactor(zoom);
        let _ = page.controller.SetBounds(RECT { left: 0, top: 0, right: w, bottom: h });
        let _ = page.controller.SetIsVisible(shown);
    }
    apply_agent(&page, agent.as_deref());
    if default_store {
        let cookies = PENDING_COOKIES.with(|pending| std::mem::take(&mut *pending.borrow_mut()));
        put_cookies(&page, &cookies);
        if CLEAR_PENDING.with(|pending| pending.replace(false)) {
            clear_page_data(&page);
        }
    }
    for op in queue {
        op(Some(&page));
    }
}

fn cookie_manager(page: &Page) -> Option<ICoreWebView2CookieManager> {
    // SAFETY: plain calls on a live page.
    unsafe { page.webview.cast::<ICoreWebView2_2>().ok()?.CookieManager().ok() }
}

/// The in-app browser's cookies whose domain `keep` accepts, delivered on the main thread.
pub fn cookies(keep: impl Fn(&str) -> bool + 'static, reply: impl FnOnce(Vec<Cookie>) + 'static) {
    cookies_of(None, keep, reply)
}

/// [`cookies`] of `profile`'s store. WebView2 reaches a store only through one of its pages:
/// with none open, the answer is empty (as WebKit's is before any page exists).
pub fn cookies_of(profile: Profile, keep: impl Fn(&str) -> bool + 'static, reply: impl FnOnce(Vec<Cookie>) + 'static) {
    if let Some(page) = page_of(profile) {
        return read_cookies(&page, keep, reply);
    }
    // A page of the store is still being made (the first one takes WebView2 a moment to start):
    // the cookies are read once it is ready, not answered as "none" before.
    let making = live_views().into_iter().find(|(_, inner)| {
        inner.try_borrow().is_ok_and(|state| state.profile == profile && !state.private && !state.failed && state.page.is_none())
    });
    let Some((_, inner)) = making else { return reply(Vec::new()) };
    let Ok(mut state) = inner.try_borrow_mut() else { return reply(Vec::new()) };
    state.queue.push(Box::new(move |page| match page {
        Some(page) => read_cookies(page, keep, reply),
        None => reply(Vec::new()),
    }));
}

fn read_cookies(page: &Page, keep: impl Fn(&str) -> bool + 'static, reply: impl FnOnce(Vec<Cookie>) + 'static) {
    let Some(manager) = cookie_manager(page) else { return reply(Vec::new()) };
    let reply = Rc::new(RefCell::new(Some(reply)));
    let answer = reply.clone();
    let handler = GetCookiesCompletedHandler::create(Box::new(move |_, list| {
        let Some(reply) = answer.borrow_mut().take() else { return Ok(()) };
        let mut out = Vec::new();
        if let Some(list) = list {
            let mut count = 0u32;
            // SAFETY: reading the list WebView2 handed over.
            unsafe {
                list.Count(&mut count)?;
                for index in 0..count {
                    let Ok(cookie) = list.GetValueAtIndex(index) else { continue };
                    let domain = read(|v| cookie.Domain(v)).unwrap_or_default();
                    if !keep(&domain) {
                        continue;
                    }
                    let flag = |get: &dyn Fn(&mut BOOL) -> windows::core::Result<()>| {
                        let mut value = BOOL::default();
                        get(&mut value).is_ok() && value.as_bool()
                    };
                    let session = flag(&|v| cookie.IsSession(v));
                    let mut expires = 0f64;
                    let _ = cookie.Expires(&mut expires);
                    let mut same_site = COREWEBVIEW2_COOKIE_SAME_SITE_KIND::default();
                    let _ = cookie.SameSite(&mut same_site);
                    out.push(Cookie {
                        name: read(|v| cookie.Name(v)).unwrap_or_default(),
                        value: read(|v| cookie.Value(v)).unwrap_or_default(),
                        domain,
                        path: read(|v| cookie.Path(v)).unwrap_or_else(|| "/".into()),
                        expires: (!session && expires > 0.).then_some(expires),
                        secure: flag(&|v| cookie.IsSecure(v)),
                        http_only: flag(&|v| cookie.IsHttpOnly(v)),
                        same_site: Some(
                            match same_site {
                                COREWEBVIEW2_COOKIE_SAME_SITE_KIND_STRICT => "Strict",
                                COREWEBVIEW2_COOKIE_SAME_SITE_KIND_NONE => "None",
                                _ => "Lax",
                            }
                            .to_string(),
                        ),
                    });
                }
            }
        }
        reply(out);
        Ok(())
    }));
    // SAFETY: an empty URI asks for every cookie of the store.
    if unsafe { manager.GetCookies(&HSTRING::new(), &handler) }.is_err() {
        if let Some(reply) = reply.borrow_mut().take() {
            reply(Vec::new());
        }
    }
}

fn put_cookies(page: &Page, cookies: &[Cookie]) {
    let Some(manager) = cookie_manager(page) else { return };
    for cookie in cookies {
        // SAFETY: plain calls on the store's cookie manager.
        unsafe {
            let Ok(made) = manager.CreateCookie(
                &HSTRING::from(cookie.name.as_str()),
                &HSTRING::from(cookie.value.as_str()),
                &HSTRING::from(cookie.domain.as_str()),
                &HSTRING::from(cookie.path.as_str()),
            ) else {
                continue;
            };
            if let Some(expires) = cookie.expires {
                let _ = made.SetExpires(expires);
            }
            let _ = made.SetIsSecure(cookie.secure);
            let _ = made.SetIsHttpOnly(cookie.http_only);
            let kind = match cookie.same_site.as_deref().map(str::to_ascii_lowercase).as_deref() {
                Some("strict") => Some(COREWEBVIEW2_COOKIE_SAME_SITE_KIND_STRICT),
                Some("none") => Some(COREWEBVIEW2_COOKIE_SAME_SITE_KIND_NONE),
                Some("lax") => Some(COREWEBVIEW2_COOKIE_SAME_SITE_KIND_LAX),
                _ => None,
            };
            if let Some(kind) = kind {
                let _ = made.SetSameSite(kind);
            }
            let _ = manager.AddOrUpdateCookie(&made);
        }
    }
}

/// Puts cookies back into the in-app browser (a session carried over a restart).
pub fn set_cookies(cookies: &[Cookie]) {
    match page_of(None) {
        Some(page) => put_cookies(&page, cookies),
        None => PENDING_COOKIES.with(|pending| pending.borrow_mut().extend(cookies.iter().cloned())),
    }
}

/// Deletes a profile's store: its cookies, cache and site data.
pub fn remove_profile(bytes: [u8; 16]) {
    if let Some(page) = page_of(Some(bytes)) {
        // SAFETY: plain calls on a live page; deleting closes the profile's pages.
        unsafe {
            if let Ok(profile) = page.webview.cast::<ICoreWebView2_13>().and_then(|w| w.Profile()) {
                if let Ok(profile) = profile.cast::<ICoreWebView2Profile8>() {
                    let _ = profile.Delete();
                    return;
                }
            }
        }
    }
    // No page of it is open: its folder goes (it may still be in use until the browser process
    // lets it go, and then stays until the next time).
    let _ = std::fs::remove_dir_all(data_folder().join("EBWebView").join(profile_name(&bytes)));
}

fn clear_page_data(page: &Page) {
    // SAFETY: plain calls on a live page.
    unsafe {
        if let Ok(profile) = page.webview.cast::<ICoreWebView2_13>().and_then(|w| w.Profile()) {
            if let Ok(profile) = profile.cast::<ICoreWebView2Profile2>() {
                let _ = profile.ClearBrowsingDataAll(&ClearBrowsingDataCompletedHandler::create(Box::new(|_| Ok(()))));
            }
        }
    }
}

/// Deletes cookies, cache, local storage and history of the in-app browser.
pub fn clear_website_data() {
    match page_of(None) {
        Some(page) => clear_page_data(&page),
        None => CLEAR_PENDING.with(|pending| pending.set(true)),
    }
}

/// Runs an editing command on the in-app browser page that has the keyboard, and says whether one
/// did (the terminal behind it must not act on the same shortcut then).
pub fn perform_in_page(command: EditCommand) -> bool {
    // SAFETY: a plain query.
    let focus = unsafe { GetFocus() };
    let Some(page) = live_views()
        .into_iter()
        .find_map(|(id, inner)| is_inside(HWND(id as *mut _), focus).then(|| inner.borrow().page.clone()).flatten())
    else {
        return false;
    };
    match command {
        EditCommand::Copy => evaluate(&page.webview, "document.execCommand('copy')", None, |_| {}),
        EditCommand::SelectAll => evaluate(&page.webview, "document.execCommand('selectAll')", None, |_| {}),
        EditCommand::Paste => {
            if let Some(text) = clipboard_text() {
                devtools(&page.webview, "Input.insertText", json!({ "text": text }), |_| {});
            }
        }
    }
    true
}

fn clipboard_text() -> Option<String> {
    use windows::Win32::Foundation::HGLOBAL;
    use windows::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard};
    use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
    const CF_UNICODETEXT: u32 = 13;
    // SAFETY: the clipboard is opened, read and closed again; the text is copied out while locked.
    unsafe {
        OpenClipboard(None).ok()?;
        let text = GetClipboardData(CF_UNICODETEXT).ok().and_then(|handle| {
            let memory = HGLOBAL(handle.0);
            let pointer = GlobalLock(memory) as *const u16;
            if pointer.is_null() {
                return None;
            }
            let mut len = 0;
            while *pointer.add(len) != 0 {
                len += 1;
            }
            let text = String::from_utf16_lossy(std::slice::from_raw_parts(pointer, len));
            let _ = GlobalUnlock(memory);
            Some(text)
        });
        let _ = CloseClipboard();
        text
    }
}

/// Gives the keyboard back to Agentty's window when a page holds it.
pub fn focus_gpui_view(window: &gpui::Window) {
    let Some(parent) = window_hwnd(window) else { return };
    // SAFETY: plain queries and focusing Agentty's own window.
    unsafe {
        let focus = GetFocus();
        if focus == parent || focus.is_invalid() {
            return;
        }
        if live_views().iter().any(|(id, _)| is_inside(HWND(*id as *mut _), focus)) {
            give_keyboard_to(parent);
        }
    }
}
