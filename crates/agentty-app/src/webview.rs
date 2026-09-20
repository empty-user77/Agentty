//! In-app browser: a WebKit `WKWebView` placed over a GPUI element. GPUI has no web view of its
//! own, so the native view is added to the window's content view and its frame is kept in sync
//! with the placeholder element's bounds every frame.

#![allow(unexpected_cfgs)] // objc 0.2 macros check a `cargo-clippy` cfg

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

/// `WKNavigationDelegate` + `WKUIDelegate`: records failed loads and requests for new windows.
fn delegate_class() -> &'static Class {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| {
        let mut decl = ClassDecl::new("AgenttyWebNavigation", class!(NSObject)).expect("AgenttyWebNavigation registered twice");
        unsafe {
            decl.add_method(sel!(webView:didStartProvisionalNavigation:), did_start as extern "C" fn(&Object, Sel, Id, Id));
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

/// Whether the keyboard is inside this web view (the first responder is it or one of its subviews).
unsafe fn holds_keyboard(view: &Object) -> bool {
    let window: Id = msg_send![view, window];
    if window.is_null() {
        return false;
    }
    let responder: Id = msg_send![window, firstResponder];
    if responder.is_null() {
        return false;
    }
    let is_view: BOOL = msg_send![responder, isKindOfClass: class!(NSView)];
    if is_view != YES {
        return false;
    }
    let target = view as *const Object as Id;
    let mut current = responder;
    while !current.is_null() {
        if current == target {
            return true;
        }
        current = msg_send![current, superview];
    }
    false
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
        if flags & COMMAND != 0 && flags & (CONTROL | ALTERNATE) == 0 && holds_keyboard(this) {
            let characters: Id = msg_send![event, charactersIgnoringModifiers];
            let key = rust_string(characters).unwrap_or_default().to_lowercase();
            let shift = flags & SHIFT != 0;
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

pub struct WebView {
    view: Id,
    parent: Id,
    /// The navigation and UI delegate (WebKit holds it weakly, so it is owned here).
    delegate: Id,
    visible: bool,
}

impl WebView {
    /// Creates the web view inside `window`'s content view (hidden until `set_frame`).
    pub fn new(window: &gpui::Window, prefs: &crate::settings::BrowserSettings) -> Option<Self> {
        let ns_window = crate::native::ns_window(window)?;
        unsafe {
            let parent: Id = msg_send![ns_window, contentView];
            if parent.is_null() {
                return None;
            }
            let config: Id = msg_send![class!(WKWebViewConfiguration), new];
            let bool_of = |on: bool| if on { YES } else { NO };
            let preferences: Id = msg_send![config, preferences];
            let _: () = msg_send![preferences, setJavaScriptCanOpenWindowsAutomatically: bool_of(prefs.popups)];
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
            Some(Self { view, parent, delegate, visible: false })
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
        use block::ConcreteBlock;
        unsafe {
            let dictionary: Id = msg_send![class!(NSMutableDictionary), dictionary];
            for (key, value) in args {
                let _: () = msg_send![dictionary, setObject: ns_string(value) forKey: ns_string(key)];
            }
            let world: Id = msg_send![class!(WKContentWorld), pageWorld];
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
            if !self.visible {
                let _: () = msg_send![self.view, setHidden: NO];
                self.visible = true;
            }
        }
    }

    pub fn hide(&mut self) {
        if self.visible {
            unsafe {
                let _: () = msg_send![self.view, setHidden: YES];
            }
            self.visible = false;
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
        unsafe {
            let _: () = msg_send![self.view, setNavigationDelegate: std::ptr::null_mut::<Object>()];
            let _: () = msg_send![self.view, setUIDelegate: std::ptr::null_mut::<Object>()];
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

/// Gives keyboard focus back to GPUI's view, but only when the web view currently holds it.
/// (GPUI draws into a subview of the content view; making anything else first responder breaks
/// its hover, cursor and keyboard handling.)
pub fn focus_gpui_view(window: &gpui::Window) {
    let (Some(ns_window), Some(gpui_view)) = (crate::native::ns_window(window), crate::native::ns_view(window)) else { return };
    unsafe {
        let responder: Id = msg_send![ns_window, firstResponder];
        if responder.is_null() || responder == gpui_view {
            return;
        }
        let is_view: BOOL = msg_send![responder, isKindOfClass: class!(NSView)];
        if is_view != YES {
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
