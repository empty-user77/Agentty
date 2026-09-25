//! In-app browser: a WebKit `WKWebView` placed over a GPUI element. GPUI has no web view of its
//! own, so the native view is added to the window's content view and its frame is kept in sync
//! with the placeholder element's bounds every frame.

#![allow(unexpected_cfgs)] // objc 0.2 macros check a `cargo-clippy` cfg

use block::Block;
use cocoa::foundation::{NSPoint, NSRect, NSSize};
use objc::declare::ClassDecl;
use objc::runtime::{Class, Object, Sel, BOOL, NO, YES};
use objc::{class, msg_send, sel, sel_impl};
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::sync::{Mutex, Once};

type Id = *mut Object;

#[link(name = "WebKit", kind = "framework")]
extern "C" {}

fn ns_string(text: &str) -> Id {
    let c = CString::new(text.replace('\0', "")).unwrap_or_default();
    unsafe { msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()] }
}

fn rust_string(value: Id) -> Option<String> {
    if value.is_null() {
        return None;
    }
    let utf8: *const std::os::raw::c_char = unsafe { msg_send![value, UTF8String] };
    (!utf8.is_null()).then(|| unsafe { CStr::from_ptr(utf8) }.to_string_lossy().to_string())
}

pub use crate::platform::url::normalize_url_with;

/// Records `console.*` calls, uncaught errors and rejections (last 500) in `window.__agenttyConsole`.
const CONSOLE_CAPTURE: &str = r#"(() => { if (window.__agenttyConsole) return; const logs = []; window.__agenttyConsole = logs;
const push = (level, text) => { logs.push({ level, time: Date.now(), text }); if (logs.length > 500) logs.shift(); };
const show = (x) => { try { return typeof x === 'string' ? x : JSON.stringify(x); } catch (e) { return String(x); } };
for (const level of ['log', 'info', 'warn', 'error', 'debug']) { const original = console[level];
  console[level] = function (...args) { try { push(level, args.map(show).join(' ')); } catch (e) {} return original.apply(this, args); }; }
window.addEventListener('error', (e) => { if (e.filename) push('error', `${e.message} @ ${e.filename}:${e.lineno}`); });
window.addEventListener('unhandledrejection', (e) => push('error', `Unhandled rejection: ${show(e.reason)}`)); })();"#;

/// Records `fetch` and `XMLHttpRequest` calls (request, response, status, timing, body) in
/// `window.__agenttyNet` for the network panel at the bottom of the browser. Bodies are kept
/// truncated and only for text-like responses, so a page that streams megabytes stays cheap.
const NETWORK_CAPTURE: &str = r#"(() => { if (window.__agenttyNet) return;
const MAX = 300, BODY = 12000, FIELD = 2000;
const cut = (text) => { text = String(text == null ? '' : text); return text.length > FIELD ? text.slice(0, FIELD) + '\u2026' : text; };
const state = { seq: 0, entries: [] }; window.__agenttyNet = state;
const add = (entry) => { entry.id = ++state.seq; state.entries.push(entry);
  if (state.entries.length > MAX) state.entries.shift(); return entry; };
const trim = (text) => typeof text === 'string' ? (text.length > BODY ? text.slice(0, BODY) + '…' : text) : '';
const textLike = (type) => /json|text|xml|javascript|html|form-urlencoded/i.test(type || '');
const headerList = (source) => { const out = []; try {
    if (!source) return out;
    if (typeof source.forEach === 'function' && typeof source.get === 'function') { source.forEach((v, k) => out.push([cut(k), cut(v)])); return out; }
    if (Array.isArray(source)) { for (const pair of source) out.push([cut(pair[0]), cut(pair[1])]); return out; }
    for (const key of Object.keys(source)) out.push([cut(key), cut(source[key])]);
  } catch (e) {} return out; };
const absolute = (url) => { try { return cut(new URL(url, location.href).href); } catch (e) { return cut(url); } };
const originalFetch = window.fetch;
if (originalFetch) window.fetch = function (input, init) {
  const request = (typeof Request !== 'undefined' && input instanceof Request) ? input : null;
  const url = absolute(request ? request.url : input);
  const method = cut(String((init && init.method) || (request && request.method) || 'GET').toUpperCase());
  const started = performance.now();
  const body = init && typeof init.body === 'string' ? init.body : '';
  const entry = add({ kind: 'fetch', method, url, status: 0, start: Date.now(), reqBody: trim(body),
    reqHeaders: headerList((init && init.headers) || (request && request.headers)) });
  return originalFetch.apply(this, arguments).then((response) => {
    entry.status = response.status; entry.statusText = cut(response.statusText || '');
    entry.ok = response.ok; entry.duration = Math.round(performance.now() - started);
    entry.resHeaders = headerList(response.headers);
    try { entry.type = cut(response.headers.get('content-type') || ''); } catch (e) { entry.type = ''; }
    let length = 0; try { length = Number(response.headers.get('content-length')) || 0; } catch (e) {}
    if (length) entry.size = length;
    if (textLike(entry.type)) { try { response.clone().text().then((text) => {
      entry.body = trim(text); if (!entry.size) entry.size = text.length; }).catch(() => {}); } catch (e) {} }
    return response;
  }).catch((error) => { entry.error = cut((error && error.message) || error);
    entry.duration = Math.round(performance.now() - started); throw error; });
};
const XHR = window.XMLHttpRequest;
if (XHR && XHR.prototype) { const open = XHR.prototype.open, send = XHR.prototype.send;
  XHR.prototype.open = function (method, url) { try { this.__agentty = { method: cut(String(method).toUpperCase()), url: absolute(url) }; } catch (e) {}
    return open.apply(this, arguments); };
  XHR.prototype.send = function (body) { const info = this.__agentty;
    if (info) { const started = performance.now();
      const entry = add({ kind: 'xhr', method: info.method, url: info.url, status: 0, start: Date.now(),
        reqBody: trim(typeof body === 'string' ? body : ''), reqHeaders: [] });
      this.addEventListener('loadend', () => {
        entry.status = this.status; entry.ok = this.status >= 200 && this.status < 400;
        entry.duration = Math.round(performance.now() - started);
        try { entry.type = cut(this.getResponseHeader('content-type') || ''); } catch (e) { entry.type = ''; }
        try { entry.resHeaders = (this.getAllResponseHeaders() || '').trim().split(/\r?\n/).filter(Boolean).slice(0, 100)
          .map((line) => { const at = line.indexOf(':'); return at < 0 ? [cut(line), ''] : [cut(line.slice(0, at)), cut(line.slice(at + 1).trim())]; }); } catch (e) {}
        if (!this.status) entry.error = 'network error';
        try { if (this.responseType === '' || this.responseType === 'text') {
          entry.body = trim(this.responseText); entry.size = this.responseText.length; } } catch (e) {}
      });
    }
    return send.apply(this, arguments); }; }
})();"#;

/// How much the page has pulled over the network so far, read from the Resource Timing API (the
/// real transfer, compression included) — the summary shown in the browser's status bar.
pub const NET_TOTALS: &str = r#"let transferred = 0, decoded = 0, count = 0;
const take = (entry) => { transferred += entry.transferSize || 0; decoded += entry.decodedBodySize || 0; count += 1; };
try { for (const entry of performance.getEntriesByType('navigation')) take(entry); } catch (e) {}
try { for (const entry of performance.getEntriesByType('resource')) take(entry); } catch (e) {}
const state = window.__agenttyNet;
return JSON.stringify({ transferred, decoded, count, calls: state ? state.entries.length : 0 });"#;

/// The API calls the page made, newest last and without bodies (those are fetched per request).
/// `window.__agenttyNet` lives in the page and the page may write anything into it, so everything
/// read back out is cut to a length the panel can show before it crosses into Agentty.
pub const NET_ENTRIES: &str = r#"const state = window.__agenttyNet; if (!state) return JSON.stringify([]);
const cut = (v) => String(v == null ? '' : v).slice(0, 2000);
const num = (v) => { const n = Number(v); return Number.isFinite(n) ? Math.max(0, Math.min(n, 1e12)) : 0; };
return JSON.stringify(state.entries.slice(-200).map((e) => ({ id: num(e.id), kind: cut(e.kind), method: cut(e.method), url: cut(e.url),
  status: num(e.status), ok: !!e.ok, duration: num(e.duration), size: num(e.size), type: cut(e.type), error: cut(e.error) })));"#;

/// Everything about one recorded call: headers and the (truncated) request and response bodies.
pub const NET_DETAIL: &str = r#"const state = window.__agenttyNet; if (!state) throw new Error('nothing recorded');
const entry = state.entries.find((e) => String(e.id) === String(id));
if (!entry) throw new Error('this request is no longer recorded');
const cut = (v, max) => String(v == null ? '' : v).slice(0, max);
const num = (v) => { const n = Number(v); return Number.isFinite(n) ? Math.max(0, Math.min(n, 1e12)) : 0; };
const headers = (list) => (Array.isArray(list) ? list : []).slice(0, 100).map((p) => [cut(p && p[0], 2000), cut(p && p[1], 2000)]);
return JSON.stringify({ id: num(entry.id), method: cut(entry.method, 2000), url: cut(entry.url, 2000), status: num(entry.status),
  statusText: cut(entry.statusText, 2000), duration: num(entry.duration), size: num(entry.size), type: cut(entry.type, 2000),
  error: cut(entry.error, 2000), reqBody: cut(entry.reqBody, 16000), body: cut(entry.body, 16000),
  reqHeaders: headers(entry.reqHeaders), resHeaders: headers(entry.resHeaders) });"#;

/// Result of an asynchronous web view call, delivered on the main thread.
pub type Reply = Box<dyn FnOnce(Result<String, String>)>;

/// A page that could not be opened (no server answering, unknown host, TLS error, …).
#[derive(Clone, Debug, PartialEq)]
pub struct LoadError {
    pub url: String,
    pub message: String,
}

/// A browser shortcut pressed while the web view had the keyboard. The native view swallows key
/// events, so GPUI never sees them: the view records them here and the browser panel acts on them.
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

/// Failed loads by web view (pointer), written by the navigation delegate on the main thread.
static LOAD_ERRORS: Mutex<Option<HashMap<usize, LoadError>>> = Mutex::new(None);
/// Pages a site asked to open in a new window (`window.open`, `target="_blank"`), by web view.
static POPUPS: Mutex<Option<HashMap<usize, Vec<String>>>> = Mutex::new(None);
/// Browser shortcuts pressed inside a web view, oldest first, with the view they came from.
static KEYS: Mutex<Vec<(usize, BrowserKey)>> = Mutex::new(Vec::new());
/// Every web view that exists right now, so [`perform_in_page`] can find the focused page.
static VIEWS: Mutex<Vec<usize>> = Mutex::new(Vec::new());

/// A standard editing command, as the Edit menu names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditCommand {
    Copy,
    Paste,
    SelectAll,
}

/// Runs an editing command on the in-app browser page that has the keyboard, and says whether one
/// did. The Edit menu's ⌘C carries a GPUI action, and a menu key equivalent never reaches a native
/// view, so without this a selection on a page could not be copied at all.
pub fn perform_in_page(command: EditCommand) -> bool {
    let views = VIEWS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    for view in views {
        let view = view as Id;
        unsafe {
            let Some(responder) = keyboard_responder(&*view) else { continue };
            let selector = match command {
                // The Edit menu wires ⌘X to the same action as ⌘C, so a page's cut copies.
                EditCommand::Copy => sel!(copy:),
                EditCommand::Paste => sel!(paste:),
                EditCommand::SelectAll => sel!(selectAll:),
            };
            // Whether or not there was anything to copy, the page had the keyboard: the terminal
            // behind it must not act on the same ⌘C.
            let handled: BOOL = msg_send![responder, tryToPerform: selector with: std::ptr::null_mut::<Object>()];
            if crate::debug::enabled() {
                eprintln!("browser-edit: {command:?} handled={}", handled == YES);
            }
            return true;
        }
    }
    false
}

fn set_load_error(view: Id, error: Option<LoadError>) {
    let mut errors = LOAD_ERRORS.lock().unwrap_or_else(|e| e.into_inner());
    let errors = errors.get_or_insert_with(HashMap::new);
    match error {
        Some(error) => errors.insert(view as usize, error),
        None => errors.remove(&(view as usize)),
    };
}

fn push_popup(view: Id, url: String) {
    let mut popups = POPUPS.lock().unwrap_or_else(|e| e.into_inner());
    let popups = popups.get_or_insert_with(HashMap::new);
    let list = popups.entry(view as usize).or_default();
    if list.len() < 16 {
        list.push(url);
    }
}

fn push_key(view: Id, key: BrowserKey) {
    let mut keys = KEYS.lock().unwrap_or_else(|e| e.into_inner());
    if keys.len() < 32 {
        keys.push((view as usize, key));
    }
}

/// Shortcuts pressed in one of `views` since the last call, oldest first. Only those views'
/// keys are taken, so a second Agentty window does not swallow this one's.
pub fn take_key_commands(views: &[usize]) -> Vec<(usize, BrowserKey)> {
    let mut keys = KEYS.lock().unwrap_or_else(|e| e.into_inner());
    let (mine, others) = keys.iter().partition(|(view, _)| views.contains(view));
    *keys = others;
    mine
}

extern "C" fn did_start(_: &Object, _: Sel, view: Id, _navigation: Id) {
    set_load_error(view, None);
}

extern "C" fn did_fail(_: &Object, _: Sel, view: Id, _navigation: Id, error: Id) {
    let (code, message, url) = unsafe {
        let code: isize = msg_send![error, code];
        let user_info: Id = msg_send![error, userInfo];
        let url = if user_info.is_null() {
            None
        } else {
            let text: Id = msg_send![user_info, objectForKey: ns_string("NSErrorFailingURLStringKey")];
            let url: Id = msg_send![user_info, objectForKey: ns_string("NSErrorFailingURLKey")];
            rust_string(text).or_else(|| if url.is_null() { None } else { rust_string(msg_send![url, absoluteString]) })
        };
        (code, rust_string(msg_send![error, localizedDescription]), url)
    };
    // -999: cancelled by a newer navigation (a click during a load), not a failure.
    // 102: "frame load interrupted", which WebKit reports for downloads.
    if code == -999 || code == 102 {
        return;
    }
    let url = url.or_else(|| unsafe { current_url_of(view) }).unwrap_or_default();
    set_load_error(view, Some(LoadError { url, message: message.unwrap_or_default() }));
}

/// A page asked for a new window (`window.open`, a link with `target="_blank"`). Returning no web
/// view tells WebKit not to open one; the address is opened in a new tab of the panel instead.
extern "C" fn create_web_view(_: &Object, _: Sel, view: Id, _config: Id, action: Id, _features: Id) -> Id {
    let url = unsafe {
        let request: Id = msg_send![action, request];
        if request.is_null() {
            None
        } else {
            let url: Id = msg_send![request, URL];
            if url.is_null() {
                None
            } else {
                rust_string(msg_send![url, absoluteString])
            }
        }
    };
    // Only the web: a page must not be able to put `file:`, `javascript:` or an app's custom
    // scheme into a tab of its own accord.
    if let Some(url) = url.filter(|u| u.starts_with("http://") || u.starts_with("https://")) {
        push_popup(view, url);
    }
    std::ptr::null_mut()
}

/// The content world called `name`, kept for the app's lifetime. WebKit drops a world nothing
/// holds on to, and its scripts' state with it: a plugin's scripts must find, in the next call,
/// what they left in the page in the last one.
unsafe fn named_world(name: &str) -> Id {
    static WORLDS: Mutex<Vec<(String, usize)>> = Mutex::new(Vec::new());
    let mut worlds = WORLDS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, world)) = worlds.iter().find(|(n, _)| n == name) {
        return *world as Id;
    }
    let world: Id = msg_send![class!(WKContentWorld), worldWithName: ns_string(name)];
    let world: Id = msg_send![world, retain];
    worlds.push((name.to_string(), world as usize));
    world
}

/// Whether a page may go to `url` by itself (a link, a redirect, a script, a form): the web and
/// what a page builds in memory. `file:`, `javascript:` and apps' own schemes (`zoommtg:`,
/// `agentty:`, …) are refused outright rather than left to what WebKit happens to do with them.
fn navigation_allowed(url: &str) -> bool {
    let scheme = url.split(':').next().unwrap_or_default().to_ascii_lowercase();
    matches!(scheme.as_str(), "http" | "https" | "about" | "data" | "blob")
}

/// `webView:decidePolicyForNavigationAction:decisionHandler:`.
extern "C" fn decide_navigation(_: &Object, _: Sel, _view: Id, action: Id, handler: Id) {
    let url = unsafe {
        let request: Id = msg_send![action, request];
        let url: Id = if request.is_null() { std::ptr::null_mut() } else { msg_send![request, URL] };
        if url.is_null() {
            None
        } else {
            rust_string(msg_send![url, absoluteString])
        }
    };
    // WKNavigationActionPolicyCancel = 0, Allow = 1.
    let policy: isize = if url.as_deref().is_some_and(navigation_allowed) { 1 } else { 0 };
    let handler = handler as *mut Block<(isize,), ()>;
    unsafe { (*handler).call((policy,)) };
}

/// `WKNavigationDelegate` + `WKUIDelegate`: records failed loads and requests for new windows.
fn delegate_class() -> &'static Class {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| {
        let mut decl = ClassDecl::new("AgenttyWebNavigation", class!(NSObject)).expect("AgenttyWebNavigation registered twice");
        unsafe {
            decl.add_method(sel!(webView:didStartProvisionalNavigation:), did_start as extern "C" fn(&Object, Sel, Id, Id));
            decl.add_method(
                sel!(webView:decidePolicyForNavigationAction:decisionHandler:),
                decide_navigation as extern "C" fn(&Object, Sel, Id, Id, Id),
            );
            decl.add_method(sel!(webView:didFailProvisionalNavigation:withError:), did_fail as extern "C" fn(&Object, Sel, Id, Id, Id));
            decl.add_method(sel!(webView:didFailNavigation:withError:), did_fail as extern "C" fn(&Object, Sel, Id, Id, Id));
            decl.add_method(
                sel!(webView:createWebViewWithConfiguration:forNavigationAction:windowFeatures:),
                create_web_view as extern "C" fn(&Object, Sel, Id, Id, Id, Id) -> Id,
            );
        }
        decl.register();
    });
    Class::get("AgenttyWebNavigation").expect("AgenttyWebNavigation class")
}

/// The first responder, when the keyboard is inside this web view (it or one of its subviews).
unsafe fn keyboard_responder(view: &Object) -> Option<Id> {
    let window: Id = msg_send![view, window];
    if window.is_null() {
        return None;
    }
    let responder: Id = msg_send![window, firstResponder];
    if responder.is_null() {
        return None;
    }
    let is_view: BOOL = msg_send![responder, isKindOfClass: class!(NSView)];
    if is_view != YES {
        return None;
    }
    let target = view as *const Object as Id;
    let mut current = responder;
    while !current.is_null() {
        if current == target {
            return Some(responder);
        }
        current = msg_send![current, superview];
    }
    None
}

/// Whether the keyboard is inside this web view.
unsafe fn holds_keyboard(view: &Object) -> bool {
    keyboard_responder(view).is_some()
}

/// Browser shortcuts (reload, hard reload, new/close tab, back, forward, address bar). WebKit
/// would otherwise eat them, or the app's own ⌘T/⌘W would act on terminals while the user is
/// looking at a web page. Only while the web view actually has the keyboard — `performKeyEquivalent:`
/// is offered to every view in the window, focused or not.
extern "C" fn perform_key_equivalent(this: &Object, _: Sel, event: Id) -> BOOL {
    const COMMAND: u64 = 1 << 20;
    const SHIFT: u64 = 1 << 17;
    const CONTROL: u64 = 1 << 18;
    const ALTERNATE: u64 = 1 << 19;
    unsafe {
        let flags: u64 = msg_send![event, modifierFlags];
        let responder = if flags & COMMAND != 0 && flags & (CONTROL | ALTERNATE) == 0 { keyboard_responder(this) } else { None };
        if let Some(responder) = responder {
            let characters: Id = msg_send![event, charactersIgnoringModifiers];
            let key = rust_string(characters).unwrap_or_default().to_lowercase();
            let shift = flags & SHIFT != 0;
            // Copy, cut, paste, select all and undo: the app's Edit menu only carries GPUI
            // actions, which go to the GPUI focus tree — never to a native view. Handing the
            // standard selector to the responder chain is what makes ⌘C work on a page.
            let editing = match (key.as_str(), shift) {
                ("c", false) => Some(sel!(copy:)),
                ("x", false) => Some(sel!(cut:)),
                ("v", false) => Some(sel!(paste:)),
                ("a", false) => Some(sel!(selectAll:)),
                ("z", false) => Some(sel!(undo:)),
                ("z", true) => Some(sel!(redo:)),
                _ => None,
            };
            if let Some(selector) = editing {
                // Walked from the responder that has the keyboard, not from `NSApp`: the app may
                // not be the active one, and then it has no key window to start from.
                let handled: BOOL = msg_send![responder, tryToPerform: selector with: std::ptr::null_mut::<Object>()];
                if crate::debug::enabled() {
                    eprintln!("browser-key: editing {key} handled={}", handled == YES);
                }
                if handled == YES {
                    return YES;
                }
            }
            let command = match (key.as_str(), shift) {
                ("r", true) => Some(BrowserKey::HardReload),
                ("r", false) => Some(BrowserKey::Reload),
                ("t", false) => Some(BrowserKey::NewTab),
                ("w", false) => Some(BrowserKey::CloseTab),
                ("l", false) => Some(BrowserKey::FocusAddress),
                ("[", false) => Some(BrowserKey::Back),
                ("]", false) => Some(BrowserKey::Forward),
                _ => None,
            };
            if let Some(command) = command {
                if crate::debug::enabled() {
                    eprintln!("browser-key: {command:?}");
                }
                push_key(this as *const Object as Id, command);
                return YES;
            }
        }
        msg_send![super(this, class!(WKWebView)), performKeyEquivalent: event]
    }
}

/// `WKWebView` that hands browser shortcuts to Agentty instead of letting them fall through.
fn web_view_class() -> &'static Class {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| {
        let mut decl = ClassDecl::new("AgenttyWebView", class!(WKWebView)).expect("AgenttyWebView registered twice");
        unsafe {
            decl.add_method(sel!(performKeyEquivalent:), perform_key_equivalent as extern "C" fn(&Object, Sel, Id) -> BOOL);
        }
        decl.register();
    });
    Class::get("AgenttyWebView").expect("AgenttyWebView class")
}

unsafe fn current_url_of(view: Id) -> Option<String> {
    let url: Id = msg_send![view, URL];
    if url.is_null() {
        return None;
    }
    rust_string(msg_send![url, absoluteString])
}

/// Size of a parked page: a laptop's window, so sites lay out their desktop version.
const PARKED_WIDTH: f64 = 1280.;
const PARKED_HEIGHT: f64 = 900.;

/// `Version/<Safari's version> Safari/605.1.15`, read from the Safari on this Mac.
fn safari_suffix() -> String {
    static SUFFIX: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SUFFIX
        .get_or_init(|| {
            let version = unsafe {
                let bundle: Id = msg_send![class!(NSBundle), bundleWithPath: ns_string("/Applications/Safari.app")];
                if bundle.is_null() {
                    None
                } else {
                    let value: Id = msg_send![bundle, objectForInfoDictionaryKey: ns_string("CFBundleShortVersionString")];
                    rust_string(value)
                }
            };
            let version = version.filter(|v| !v.is_empty() && v.chars().all(|c| c.is_ascii_digit() || c == '.'));
            format!("Version/{} Safari/605.1.15", version.as_deref().unwrap_or("18.0"))
        })
        .clone()
}

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

/// Whether any web view exists. WebKit loads the saved cookies only once one does: before that
/// the cookie store answers as if it were empty.
pub fn any_view() -> bool {
    !VIEWS.lock().unwrap_or_else(|e| e.into_inner()).is_empty()
}

/// A browser profile: a store of cookies and site data of its own (a second sign-in to the same
/// site), named by a UUID. `None` is the in-app browser's own store.
pub type Profile = Option<[u8; 16]>;

/// Whether this macOS keeps stores apart by profile (`dataStoreForIdentifier:`, macOS 14).
pub fn profiles_supported() -> bool {
    unsafe {
        let responds: BOOL = msg_send![class!(WKWebsiteDataStore), respondsToSelector: sel!(dataStoreForIdentifier:)];
        responds == YES
    }
}

unsafe fn ns_uuid(bytes: &[u8; 16]) -> Id {
    let uuid: Id = msg_send![class!(NSUUID), alloc];
    let uuid: Id = msg_send![uuid, initWithUUIDBytes: bytes.as_ptr()];
    msg_send![uuid, autorelease]
}

/// The store `profile` keeps its cookies in (null when this macOS has none for profiles).
unsafe fn data_store(profile: Profile) -> Id {
    match profile {
        None => msg_send![class!(WKWebsiteDataStore), defaultDataStore],
        Some(_) if !profiles_supported() => std::ptr::null_mut(),
        Some(bytes) => msg_send![class!(WKWebsiteDataStore), dataStoreForIdentifier: ns_uuid(&bytes)],
    }
}

unsafe fn cookie_store() -> Id {
    cookie_store_of(None)
}

unsafe fn cookie_store_of(profile: Profile) -> Id {
    let store = data_store(profile);
    if store.is_null() {
        return std::ptr::null_mut();
    }
    msg_send![store, httpCookieStore]
}

/// Deletes a profile's store: its cookies, cache and site data (macOS 14).
pub fn remove_profile(bytes: [u8; 16]) {
    use block::ConcreteBlock;
    if !profiles_supported() {
        return;
    }
    unsafe {
        let done = ConcreteBlock::new(|_error: Id| {}).copy();
        let _: () = msg_send![class!(WKWebsiteDataStore), removeDataStoreForIdentifier: ns_uuid(&bytes) completionHandler: &*done];
    }
}

/// The in-app browser's cookies whose domain `keep` accepts, delivered on the main thread.
pub fn cookies(keep: impl Fn(&str) -> bool + 'static, reply: impl FnOnce(Vec<Cookie>) + 'static) {
    cookies_of(None, keep, reply)
}

/// [`cookies`] of `profile`'s store.
pub fn cookies_of(profile: Profile, keep: impl Fn(&str) -> bool + 'static, reply: impl FnOnce(Vec<Cookie>) + 'static) {
    use block::ConcreteBlock;
    unsafe {
        let store = cookie_store_of(profile);
        if store.is_null() {
            return reply(Vec::new());
        }
        let reply = std::cell::RefCell::new(Some(reply));
        let done = ConcreteBlock::new(move |list: Id| {
            let Some(reply) = reply.borrow_mut().take() else { return };
            let mut out = Vec::new();
            let count: usize = if list.is_null() { 0 } else { msg_send![list, count] };
            for index in 0..count {
                let cookie: Id = msg_send![list, objectAtIndex: index];
                let domain = rust_string(msg_send![cookie, domain]).unwrap_or_default();
                if !keep(&domain) {
                    continue;
                }
                let session: BOOL = msg_send![cookie, isSessionOnly];
                let expires: Id = msg_send![cookie, expiresDate];
                let expires = if session == YES || expires.is_null() {
                    None
                } else {
                    let seconds: f64 = msg_send![expires, timeIntervalSince1970];
                    Some(seconds)
                };
                let secure: BOOL = msg_send![cookie, isSecure];
                let http_only: BOOL = msg_send![cookie, isHTTPOnly];
                let has_policy: BOOL = msg_send![cookie, respondsToSelector: sel!(sameSitePolicy)];
                let same_site = if has_policy == YES { rust_string(msg_send![cookie, sameSitePolicy]) } else { None };
                out.push(Cookie {
                    name: rust_string(msg_send![cookie, name]).unwrap_or_default(),
                    value: rust_string(msg_send![cookie, value]).unwrap_or_default(),
                    domain,
                    path: rust_string(msg_send![cookie, path]).unwrap_or_else(|| "/".into()),
                    expires,
                    secure: secure == YES,
                    http_only: http_only == YES,
                    same_site,
                });
            }
            reply(out);
        })
        .copy();
        let _: () = msg_send![store, getAllCookies: &*done];
    }
}

/// Puts cookies back into the in-app browser (a session carried over a restart).
pub fn set_cookies(cookies: &[Cookie]) {
    use block::ConcreteBlock;
    unsafe {
        let store = cookie_store();
        for cookie in cookies {
            let properties: Id = msg_send![class!(NSMutableDictionary), dictionary];
            let put = |key: &str, value: Id| {
                if !value.is_null() {
                    let _: () = msg_send![properties, setObject: value forKey: ns_string(key)];
                }
            };
            put("Name", ns_string(&cookie.name));
            put("Value", ns_string(&cookie.value));
            put("Domain", ns_string(&cookie.domain));
            put("Path", ns_string(&cookie.path));
            if cookie.secure {
                put("Secure", ns_string("TRUE"));
            }
            if cookie.http_only {
                put("HttpOnly", ns_string("TRUE"));
            }
            if let Some(policy) = &cookie.same_site {
                put("SameSite", ns_string(policy));
            }
            if let Some(expires) = cookie.expires {
                put("Expires", msg_send![class!(NSDate), dateWithTimeIntervalSince1970: expires]);
            }
            let made: Id = msg_send![class!(NSHTTPCookie), cookieWithProperties: properties];
            if made.is_null() {
                continue;
            }
            let done = ConcreteBlock::new(|| {}).copy();
            let _: () = msg_send![store, setCookie: made completionHandler: &*done];
        }
    }
}

pub struct WebView {
    view: Id,
    parent: Id,
    /// GPUI's own view, which gets the keyboard back when this one is hidden or goes away.
    gpui_view: Id,
    /// The navigation and UI delegate (WebKit holds it weakly, so it is owned here).
    delegate: Id,
    visible: bool,
    /// Outside the window but shown: see [`WebView::park`].
    parked: bool,
    /// A plugin's page: taken out of sight it is parked, never hidden, so it keeps running.
    keep_running: bool,
    /// Page zoom last set (the browser setting, or the responsive mode's scale).
    zoom: f64,
}

impl WebView {
    /// Creates the web view inside `window`'s content view (hidden until `set_frame`).
    pub fn new(window: &gpui::Window, prefs: &crate::settings::BrowserSettings) -> Option<Self> {
        Self::create(window, prefs, false, None)
    }

    /// A web view a plugin drives while nobody looks at it: parked outside the window, where it is
    /// never drawn but still counts as shown, and never throttled for being out of sight — a page
    /// that loads more as it is scrolled would otherwise stop halfway.
    pub fn new_background(window: &gpui::Window, prefs: &crate::settings::BrowserSettings) -> Option<Self> {
        Self::new_background_in(window, prefs, None)
    }

    /// [`new_background`] with `profile`'s cookies and site data (see [`Profile`]).
    pub fn new_background_in(window: &gpui::Window, prefs: &crate::settings::BrowserSettings, profile: Profile) -> Option<Self> {
        if profile.is_some() && !profiles_supported() {
            return None;
        }
        let mut view = Self::create(window, prefs, true, profile)?;
        view.keep_running = true;
        view.park();
        Some(view)
    }

    fn create(window: &gpui::Window, prefs: &crate::settings::BrowserSettings, background: bool, profile: Profile) -> Option<Self> {
        let ns_window = crate::native::ns_window(window)?;
        let gpui_view = crate::native::ns_view(window)?;
        unsafe {
            let parent: Id = msg_send![ns_window, contentView];
            if parent.is_null() {
                return None;
            }
            let config: Id = msg_send![class!(WKWebViewConfiguration), new];
            let bool_of = |on: bool| if on { YES } else { NO };
            let preferences: Id = msg_send![config, preferences];
            let _: () = msg_send![preferences, setJavaScriptCanOpenWindowsAutomatically: bool_of(prefs.popups)];
            // `WKInactiveSchedulingPolicyNone` (macOS 14): a page out of sight keeps its timers.
            let unthrottled: BOOL = msg_send![preferences, respondsToSelector: sel!(setInactiveSchedulingPolicy:)];
            if background && unthrottled == YES {
                let _: () = msg_send![preferences, setInactiveSchedulingPolicy: 2isize];
            }
            // Safari's own tail on the user agent. Without it the page sees an app's embedded
            // browser, and some sites refuse to sign anyone in there.
            if !prefs.mobile {
                let _: () = msg_send![config, setApplicationNameForUserAgent: ns_string(&safari_suffix())];
            }
            let page_prefs: Id = msg_send![config, defaultWebpagePreferences];
            if !page_prefs.is_null() {
                let _: () = msg_send![page_prefs, setAllowsContentJavaScript: bool_of(prefs.javascript)];
            }
            // Console output and network calls are kept in the page: `agentty browser console`
            // reads the first, the browser's network panel the second.
            let controller: Id = msg_send![config, userContentController];
            if !controller.is_null() {
                for (source, main_frame_only) in [(CONSOLE_CAPTURE, NO), (NETWORK_CAPTURE, YES)] {
                    let script: Id = msg_send![class!(WKUserScript), alloc];
                    let script: Id =
                        msg_send![script, initWithSource: ns_string(source) injectionTime: 0isize forMainFrameOnly: main_frame_only];
                    let _: () = msg_send![controller, addUserScript: script];
                    let _: () = msg_send![script, release];
                }
            }
            if prefs.private_mode {
                let store: Id = msg_send![class!(WKWebsiteDataStore), nonPersistentDataStore];
                let _: () = msg_send![config, setWebsiteDataStore: store];
            } else if profile.is_some() {
                let store = data_store(profile);
                if store.is_null() {
                    return None;
                }
                let _: () = msg_send![config, setWebsiteDataStore: store];
            }
            let view: Id = msg_send![web_view_class(), alloc];
            let frame = NSRect::new(NSPoint::new(0., 0.), NSSize::new(10., 10.));
            let view: Id = msg_send![view, initWithFrame: frame configuration: config];
            let _: () = msg_send![config, release];
            if view.is_null() {
                return None;
            }
            let _: () = msg_send![view, setAllowsBackForwardNavigationGestures: YES];
            let responds: BOOL = msg_send![view, respondsToSelector: sel!(setInspectable:)];
            if responds == YES {
                let _: () = msg_send![view, setInspectable: bool_of(prefs.inspectable)];
            }
            let _: () = msg_send![view, setPageZoom: prefs.zoom.clamp(0.3, 3.0) as f64];
            if prefs.mobile {
                let agent = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1";
                let _: () = msg_send![view, setCustomUserAgent: ns_string(agent)];
            }
            let delegate: Id = msg_send![delegate_class(), new];
            let _: () = msg_send![view, setNavigationDelegate: delegate];
            let _: () = msg_send![view, setUIDelegate: delegate];
            let _: () = msg_send![view, setHidden: YES];
            let _: () = msg_send![parent, addSubview: view];
            VIEWS.lock().unwrap_or_else(|e| e.into_inner()).push(view as usize);
            Some(Self {
                view,
                parent,
                gpui_view,
                delegate,
                visible: false,
                parked: false,
                keep_running: false,
                zoom: prefs.zoom.clamp(0.3, 3.0) as f64,
            })
        }
    }

    /// Identifies this view in [`take_key_commands`].
    pub fn id(&self) -> usize {
        self.view as usize
    }

    /// Whether the keyboard is inside this page (it then gets the browser shortcuts).
    pub fn has_keyboard(&self) -> bool {
        unsafe { holds_keyboard(&*self.view) }
    }

    pub fn load(&self, url: &str) {
        unsafe {
            let ns_url: Id = msg_send![class!(NSURL), URLWithString: ns_string(url)];
            if ns_url.is_null() {
                return;
            }
            let request: Id = msg_send![class!(NSURLRequest), requestWithURL: ns_url];
            let _: Id = msg_send![self.view, loadRequest: request];
        }
    }

    pub fn back(&self) {
        unsafe {
            let _: Id = msg_send![self.view, goBack];
        }
    }

    pub fn forward(&self) {
        unsafe {
            let _: Id = msg_send![self.view, goForward];
        }
    }

    /// Reloads from the server, not the cache: pages here are mostly dev servers that just changed.
    pub fn reload(&self) {
        unsafe {
            let _: Id = msg_send![self.view, reloadFromOrigin];
        }
    }

    /// ⌘⇧R: throws away the caches first, so scripts, styles and images are fetched again — what
    /// a browser's hard reload does, and what [`reload`] (revalidation only) cannot guarantee.
    pub fn hard_reload(&self) {
        use block::ConcreteBlock;
        unsafe {
            let types: Id = msg_send![class!(NSMutableSet), set];
            for name in ["WKWebsiteDataTypeDiskCache", "WKWebsiteDataTypeMemoryCache", "WKWebsiteDataTypeOfflineWebApplicationCache"] {
                let _: () = msg_send![types, addObject: ns_string(name)];
            }
            let config: Id = msg_send![self.view, configuration];
            let store: Id = msg_send![config, websiteDataStore];
            let since: Id = msg_send![class!(NSDate), distantPast];
            // The view is retained for the callback: the tab may be closed while the cache clears.
            let view: Id = msg_send![self.view, retain];
            let done = ConcreteBlock::new(move || {
                let _: Id = msg_send![view, reloadFromOrigin];
                let _: () = msg_send![view, release];
            })
            .copy();
            let _: () = msg_send![store, removeDataOfTypes: types modifiedSince: since completionHandler: &*done];
        }
    }

    pub fn current_url(&self) -> Option<String> {
        unsafe { current_url_of(self.view) }
    }

    /// Why the last page could not be opened, until the next navigation starts.
    pub fn load_error(&self) -> Option<LoadError> {
        LOAD_ERRORS.lock().unwrap_or_else(|e| e.into_inner()).as_ref()?.get(&(self.view as usize)).cloned()
    }

    /// Addresses this page asked to open in a new window since the last call.
    pub fn take_popups(&self) -> Vec<String> {
        let mut popups = POPUPS.lock().unwrap_or_else(|e| e.into_inner());
        popups.as_mut().and_then(|popups| popups.remove(&(self.view as usize))).unwrap_or_default()
    }

    pub fn title(&self) -> Option<String> {
        unsafe { rust_string(msg_send![self.view, title]) }.filter(|t| !t.is_empty())
    }

    /// Runs `body` as the body of an async JavaScript function in the page (not subject to the
    /// page's CSP) with `args` as its variables; the returned value is sent as JSON.
    pub fn call_async(&self, body: &str, args: &[(&str, &str)], reply: Reply) {
        self.call_in_world(body, args, None, reply)
    }

    /// [`call_async`] in a content world of its own (`Some(name)`): the page's scripts share the
    /// DOM with it but cannot see or replace its variables and functions.
    pub fn call_in_world(&self, body: &str, args: &[(&str, &str)], world: Option<&str>, reply: Reply) {
        use block::ConcreteBlock;
        unsafe {
            let dictionary: Id = msg_send![class!(NSMutableDictionary), dictionary];
            for (key, value) in args {
                let _: () = msg_send![dictionary, setObject: ns_string(value) forKey: ns_string(key)];
            }
            let world: Id = match world {
                Some(name) => named_world(name),
                None => msg_send![class!(WKContentWorld), pageWorld],
            };
            let reply = std::cell::RefCell::new(Some(reply));
            let completion = ConcreteBlock::new(move |result: Id, error: Id| {
                let Some(reply) = reply.borrow_mut().take() else { return };
                if !error.is_null() {
                    let description: Id = msg_send![error, localizedDescription];
                    let user_info: Id = msg_send![error, userInfo];
                    let detail: Id = if user_info.is_null() {
                        std::ptr::null_mut()
                    } else {
                        msg_send![user_info, objectForKey: ns_string("WKJavaScriptExceptionMessage")]
                    };
                    return reply(Err(rust_string(detail)
                        .or_else(|| rust_string(description))
                        .unwrap_or_else(|| "JavaScript error".into())));
                }
                let is_string: BOOL = if result.is_null() { NO } else { msg_send![result, isKindOfClass: class!(NSString)] };
                reply(Ok(if is_string == YES { rust_string(result).unwrap_or_default() } else { "null".into() }))
            })
            .copy();
            let _: () = msg_send![self.view, callAsyncJavaScript: ns_string(body) arguments: dictionary inFrame: std::ptr::null_mut::<Object>() inContentWorld: world completionHandler: &*completion];
        }
    }

    /// Saves what the page shows as a PNG.
    pub fn snapshot_png(&self, path: std::path::PathBuf, reply: Reply) {
        use block::ConcreteBlock;
        unsafe {
            let reply = std::cell::RefCell::new(Some(reply));
            let completion = ConcreteBlock::new(move |image: Id, error: Id| {
                let Some(reply) = reply.borrow_mut().take() else { return };
                if image.is_null() || !error.is_null() {
                    let description: Id = if error.is_null() { std::ptr::null_mut() } else { msg_send![error, localizedDescription] };
                    return reply(Err(rust_string(description).unwrap_or_else(|| "snapshot failed".into())));
                }
                let tiff: Id = msg_send![image, TIFFRepresentation];
                let rep: Id = msg_send![class!(NSBitmapImageRep), imageRepWithData: tiff];
                let properties: Id = msg_send![class!(NSDictionary), dictionary];
                let png: Id = if rep.is_null() {
                    std::ptr::null_mut()
                } else {
                    msg_send![rep, representationUsingType: 4usize properties: properties]
                };
                let written: BOOL =
                    if png.is_null() { NO } else { msg_send![png, writeToFile: ns_string(&path.display().to_string()) atomically: YES] };
                reply(if written == YES {
                    Ok(serde_json::json!(path.display().to_string()).to_string())
                } else {
                    Err("could not write the PNG".into())
                })
            })
            .copy();
            let _: () = msg_send![self.view, takeSnapshotWithConfiguration: std::ptr::null_mut::<Object>() completionHandler: &*completion];
        }
    }

    pub fn is_loading(&self) -> bool {
        let loading: BOOL = unsafe { msg_send![self.view, isLoading] };
        loading == YES
    }

    /// How far the current load is, 0.0 to 1.0 (WebKit's estimate).
    pub fn estimated_progress(&self) -> f32 {
        let progress: f64 = unsafe { msg_send![self.view, estimatedProgress] };
        progress.clamp(0., 1.) as f32
    }

    /// Places the view over `bounds` (window coordinates, top-left origin) and shows it.
    pub fn set_frame(&mut self, bounds: gpui::Bounds<gpui::Pixels>) {
        unsafe {
            let parent_frame: NSRect = msg_send![self.parent, frame];
            let flipped: BOOL = msg_send![self.parent, isFlipped];
            let (x, y, w, h) = (
                f32::from(bounds.origin.x) as f64,
                f32::from(bounds.origin.y) as f64,
                f32::from(bounds.size.width) as f64,
                f32::from(bounds.size.height) as f64,
            );
            let origin_y = if flipped == YES { y } else { parent_frame.size.height - y - h };
            let _: () = msg_send![self.view, setFrame: NSRect::new(NSPoint::new(x, origin_y), NSSize::new(w.max(1.), h.max(1.)))];
            self.parked = false;
            if !self.visible {
                let _: () = msg_send![self.view, setHidden: NO];
                self.visible = true;
            }
        }
    }

    /// CSS zoom of the page. Responsive mode uses it to lay the page out at a device's width in a
    /// smaller frame: at zoom `z`, a frame `w` points wide holds `w / z` CSS pixels.
    pub fn set_zoom(&mut self, zoom: f64) {
        let zoom = zoom.clamp(0.1, 3.0);
        if (zoom - self.zoom).abs() > 1e-4 {
            self.zoom = zoom;
            unsafe {
                let _: () = msg_send![self.view, setPageZoom: zoom];
            }
        }
    }

    /// Moves the view outside the window at a desktop size: never drawn, never in the way, but
    /// not hidden either — WebKit suspends a hidden page, and a plugin reading one must not have it
    /// stop while it scrolls.
    pub fn park(&mut self) {
        if self.parked {
            return;
        }
        unsafe {
            // Out of the window it can still hold the keyboard, and then every key only beeps.
            self.release_keyboard();
            let parent_frame: NSRect = msg_send![self.parent, frame];
            let (w, h) = (PARKED_WIDTH, PARKED_HEIGHT);
            let origin = NSPoint::new(-(w + parent_frame.size.width + 400.), -(h + parent_frame.size.height + 400.));
            let _: () = msg_send![self.view, setFrame: NSRect::new(origin, NSSize::new(w, h))];
            let _: () = msg_send![self.view, setHidden: NO];
        }
        self.visible = false;
        self.parked = true;
    }

    pub fn hide(&mut self) {
        if self.keep_running {
            // Hiding would suspend the page a plugin is working in: it goes out of the window.
            return self.park();
        }
        if self.visible {
            unsafe {
                self.release_keyboard();
                let _: () = msg_send![self.view, setHidden: YES];
            }
            self.visible = false;
        }
    }

    /// Hands the keyboard to GPUI's view when this page holds it. Hiding or removing the first
    /// responder makes AppKit give the keyboard to the window itself (GPUI's view does not accept
    /// first responder from a click), and from then on every key only beeps.
    unsafe fn release_keyboard(&self) {
        if holds_keyboard(&*self.view) {
            let window: Id = msg_send![self.view, window];
            let _: BOOL = msg_send![window, makeFirstResponder: self.gpui_view];
        }
    }
}

impl Drop for WebView {
    fn drop(&mut self) {
        set_load_error(self.view, None);
        let id = self.view as usize;
        if let Some(popups) = POPUPS.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            popups.remove(&id);
        }
        KEYS.lock().unwrap_or_else(|e| e.into_inner()).retain(|(view, _)| *view != id);
        VIEWS.lock().unwrap_or_else(|e| e.into_inner()).retain(|view| *view != id);
        unsafe {
            let _: () = msg_send![self.view, setNavigationDelegate: std::ptr::null_mut::<Object>()];
            let _: () = msg_send![self.view, setUIDelegate: std::ptr::null_mut::<Object>()];
            self.release_keyboard();
            let _: () = msg_send![self.view, removeFromSuperview];
            let _: () = msg_send![self.view, release];
            let _: () = msg_send![self.delegate, release];
        }
    }
}

/// Deletes cookies, cache, local storage and history of the in-app browser.
pub fn clear_website_data() {
    unsafe {
        let store: Id = msg_send![class!(WKWebsiteDataStore), defaultDataStore];
        let types: Id = msg_send![class!(WKWebsiteDataStore), allWebsiteDataTypes];
        let since: Id = msg_send![class!(NSDate), distantPast];
        let done = block::ConcreteBlock::new(|| {}).copy();
        let _: () = msg_send![store, removeDataOfTypes: types modifiedSince: since completionHandler: &*done];
    }
}

/// Gives keyboard focus back to GPUI's view when a web view holds it, or when nothing does (the
/// window itself is first responder, as after a focused web view was hidden: keys then only beep).
/// (GPUI draws into a subview of the content view; making anything else first responder breaks
/// its hover, cursor and keyboard handling.)
pub fn focus_gpui_view(window: &gpui::Window) {
    let (Some(ns_window), Some(gpui_view)) = (crate::native::ns_window(window), crate::native::ns_view(window)) else { return };
    unsafe {
        let responder: Id = msg_send![ns_window, firstResponder];
        if responder == gpui_view {
            return;
        }
        let is_view: BOOL = if responder.is_null() { NO } else { msg_send![responder, isKindOfClass: class!(NSView)] };
        if is_view != YES {
            let _: BOOL = msg_send![ns_window, makeFirstResponder: gpui_view];
            return;
        }
        let mut current = responder;
        while !current.is_null() {
            let is_web: BOOL = msg_send![current, isKindOfClass: class!(WKWebView)];
            if is_web == YES {
                let _: BOOL = msg_send![ns_window, makeFirstResponder: gpui_view];
                return;
            }
            current = msg_send![current, superview];
        }
    }
}
