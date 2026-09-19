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

/// Result of an asynchronous web view call, delivered on the main thread.
pub type Reply = Box<dyn FnOnce(Result<String, String>)>;

/// A page that could not be opened (no server answering, unknown host, TLS error, …).
#[derive(Clone, Debug, PartialEq)]
pub struct LoadError {
    pub url: String,
    pub message: String,
}

/// Failed loads by web view (pointer), written by the navigation delegate on the main thread.
static LOAD_ERRORS: Mutex<Option<HashMap<usize, LoadError>>> = Mutex::new(None);

fn set_load_error(view: Id, error: Option<LoadError>) {
    let mut errors = LOAD_ERRORS.lock().unwrap_or_else(|e| e.into_inner());
    let errors = errors.get_or_insert_with(HashMap::new);
    match error {
        Some(error) => errors.insert(view as usize, error),
        None => errors.remove(&(view as usize)),
    };
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

/// `WKNavigationDelegate` that records failed loads in `LOAD_ERRORS`.
fn delegate_class() -> &'static Class {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| {
        let mut decl = ClassDecl::new("AgenttyWebNavigation", class!(NSObject)).expect("AgenttyWebNavigation registered twice");
        unsafe {
            decl.add_method(sel!(webView:didStartProvisionalNavigation:), did_start as extern "C" fn(&Object, Sel, Id, Id));
            decl.add_method(sel!(webView:didFailProvisionalNavigation:withError:), did_fail as extern "C" fn(&Object, Sel, Id, Id, Id));
            decl.add_method(sel!(webView:didFailNavigation:withError:), did_fail as extern "C" fn(&Object, Sel, Id, Id, Id));
        }
        decl.register();
    });
    Class::get("AgenttyWebNavigation").expect("AgenttyWebNavigation class")
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
    /// The navigation delegate (WebKit holds it weakly, so it is owned here).
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
            // Console output is kept in the page so `agentty browser console` can read it.
            let controller: Id = msg_send![config, userContentController];
            if !controller.is_null() {
                let script: Id = msg_send![class!(WKUserScript), alloc];
                let script: Id = msg_send![script, initWithSource: ns_string(CONSOLE_CAPTURE) injectionTime: 0isize forMainFrameOnly: NO];
                let _: () = msg_send![controller, addUserScript: script];
                let _: () = msg_send![script, release];
            }
            if prefs.private_mode {
                let store: Id = msg_send![class!(WKWebsiteDataStore), nonPersistentDataStore];
                let _: () = msg_send![config, setWebsiteDataStore: store];
            }
            let view: Id = msg_send![class!(WKWebView), alloc];
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
            let _: () = msg_send![view, setHidden: YES];
            let _: () = msg_send![parent, addSubview: view];
            Some(Self { view, parent, delegate, visible: false })
        }
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

    pub fn current_url(&self) -> Option<String> {
        unsafe { current_url_of(self.view) }
    }

    /// Why the last page could not be opened, until the next navigation starts.
    pub fn load_error(&self) -> Option<LoadError> {
        LOAD_ERRORS.lock().unwrap_or_else(|e| e.into_inner()).as_ref()?.get(&(self.view as usize)).cloned()
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
        unsafe {
            let _: () = msg_send![self.view, setNavigationDelegate: std::ptr::null_mut::<Object>()];
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
