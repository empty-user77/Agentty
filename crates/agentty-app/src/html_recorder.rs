//! Records an HTML animation a plugin's agent wrote into video frames (`media/htmlToVideo`).
//!
//! The page loads in a web view of its own, out of sight, with no network, no cookies and a
//! virtual clock put in before its first script: `requestAnimationFrame`, timers, `Date`,
//! `performance.now` and `Math.random` answer to the clock, and `__seek(ms)` moves it, running
//! what falls due and setting every CSS / Web Animation and SVG animation to that moment. A frame
//! is then a `__seek` and a snapshot, so the video is exact at any speed the machine draws.

#![allow(unexpected_cfgs)] // objc 0.2 macros check a `cargo-clippy` cfg

/// Put in before the page's own scripts: the virtual clock and the error list.
#[cfg(target_os = "macos")]
pub const CLOCK_JS: &str = r#"(() => {
  const errors = [];
  window.__agenttyErrors = errors;
  addEventListener('error', (e) => errors.push(String(e.message || e)));
  addEventListener('unhandledrejection', (e) => errors.push(String(e.reason)));
  let now = 0;
  const origin = 1767225600000;
  let nextId = 1;
  let frames = [];
  let timers = [];
  window.requestAnimationFrame = (cb) => { const id = nextId++; frames.push({ id, cb }); return id; };
  window.cancelAnimationFrame = (id) => { frames = frames.filter((f) => f.id !== id); };
  window.setTimeout = (cb, ms, ...args) => { const id = nextId++; timers.push({ id, at: now + (+ms || 0), cb, args }); return id; };
  window.setInterval = (cb, ms, ...args) => { const every = Math.max(1, +ms || 0); const id = nextId++; timers.push({ id, at: now + every, every, cb, args }); return id; };
  window.clearTimeout = window.clearInterval = (id) => { timers = timers.filter((t) => t.id !== id); };
  performance.now = () => now;
  const RealDate = Date;
  class ClockDate extends RealDate {
    constructor(...args) { if (args.length) super(...args); else super(origin + now); }
    static now() { return origin + now; }
  }
  window.Date = ClockDate;
  let seed = 20260926;
  Math.random = () => { seed = (seed * 16807) % 2147483647; return (seed - 1) / 2147483646; };
  const run = (cb, args) => { try { typeof cb === 'function' ? cb(...(args || [])) : (0, eval)(String(cb)); } catch (e) { errors.push(String(e && e.message || e)); } };
  window.__seek = (ms) => {
    for (let guard = 0; guard < 100000; guard++) {
      let due = null;
      for (const t of timers) if (t.at <= ms && (!due || t.at < due.at)) due = t;
      if (!due) break;
      now = due.at;
      if (due.every) due.at += due.every; else timers = timers.filter((t) => t !== due);
      run(due.cb, due.args);
    }
    now = ms;
    const due = frames; frames = [];
    for (const f of due) run(() => f.cb(ms));
    for (const a of document.getAnimations()) { try { a.pause(); a.currentTime = ms; } catch (e) {} }
    for (const svg of document.querySelectorAll('svg')) { try { svg.pauseAnimations(); svg.setCurrentTime(ms / 1000); } catch (e) {} }
    return errors.length;
  };
})();"#;

/// Put first in the page: nothing is fetched from anywhere, whatever the page asks for.
#[cfg(target_os = "macos")]
pub const NO_NETWORK: &str = r#"<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline' data:; script-src 'unsafe-inline'; img-src data: blob:; font-src data:; media-src data: blob:">"#;

#[cfg(target_os = "macos")]
mod imp {
    use super::{CLOCK_JS, NO_NETWORK};
    use block::ConcreteBlock;
    use cocoa::foundation::{NSPoint, NSRect, NSSize};
    use objc::runtime::{Object, BOOL, YES};
    use objc::{class, msg_send, sel, sel_impl};
    use std::ffi::{c_void, CString};

    type Id = *mut Object;

    #[link(name = "WebKit", kind = "framework")]
    extern "C" {}
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGColorSpaceCreateDeviceRGB() -> *mut c_void;
        fn CGColorSpaceRelease(space: *mut c_void);
        fn CGBitmapContextCreate(
            data: *mut c_void,
            width: usize,
            height: usize,
            bits: usize,
            row: usize,
            space: *mut c_void,
            info: u32,
        ) -> *mut c_void;
        fn CGContextDrawImage(context: *mut c_void, rect: NSRect, image: *mut c_void);
        fn CGContextRelease(context: *mut c_void);
    }

    /// `kCGImageAlphaPremultipliedFirst | kCGBitmapByteOrder32Little`: BGRA in memory.
    const BGRA_INFO: u32 = 2 | (2 << 12);

    fn ns_string(text: &str) -> Id {
        let c = CString::new(text.replace('\0', "")).unwrap_or_default();
        unsafe { msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()] }
    }

    fn rust_string(value: Id) -> Option<String> {
        if value.is_null() {
            return None;
        }
        unsafe {
            let is_string: BOOL = msg_send![value, isKindOfClass: class!(NSString)];
            let text: Id = if is_string == YES { value } else { msg_send![value, description] };
            let utf8: *const std::os::raw::c_char = msg_send![text, UTF8String];
            (!utf8.is_null()).then(|| std::ffi::CStr::from_ptr(utf8).to_string_lossy().into_owned())
        }
    }

    pub struct Recorder {
        view: Id,
        width: usize,
        height: usize,
        scale: f64,
    }

    impl Recorder {
        /// Loads `html` out of sight in `window`, `width`×`height` CSS pixels, with the clock in.
        pub fn start(window: &gpui::Window, width: u32, height: u32, html: &str) -> Option<Self> {
            let ns_window = crate::native::ns_window(window)?;
            unsafe {
                let parent: Id = msg_send![ns_window, contentView];
                if parent.is_null() {
                    return None;
                }
                let scale: f64 = msg_send![ns_window, backingScaleFactor];
                let config: Id = msg_send![class!(WKWebViewConfiguration), new];
                // No cookies, no stored data: the page is the agent's, not the user's.
                let store: Id = msg_send![class!(WKWebsiteDataStore), nonPersistentDataStore];
                let _: () = msg_send![config, setWebsiteDataStore: store];
                let controller: Id = msg_send![config, userContentController];
                let script: Id = msg_send![class!(WKUserScript), alloc];
                let script: Id = msg_send![script, initWithSource: ns_string(CLOCK_JS) injectionTime: 0isize forMainFrameOnly: YES];
                let _: () = msg_send![controller, addUserScript: script];
                let _: () = msg_send![script, release];
                let frame =
                    NSRect::new(NSPoint::new(-(width as f64) - 6000., -(height as f64) - 6000.), NSSize::new(width as f64, height as f64));
                let view: Id = msg_send![class!(WKWebView), alloc];
                let view: Id = msg_send![view, initWithFrame: frame configuration: config];
                let _: () = msg_send![config, release];
                if view.is_null() {
                    return None;
                }
                let _: () = msg_send![parent, addSubview: view];
                let page = format!("{NO_NETWORK}\n{html}");
                let _: Id = msg_send![view, loadHTMLString: ns_string(&page) baseURL: std::ptr::null_mut::<Object>()];
                Some(Self { view, width: width as usize, height: height as usize, scale: scale.max(1.) })
            }
        }

        pub fn loading(&self) -> bool {
            let loading: BOOL = unsafe { msg_send![self.view, isLoading] };
            loading == YES
        }

        /// Where the page is: it must stay the page it was given (`about:blank`).
        pub fn address(&self) -> String {
            unsafe {
                let url: Id = msg_send![self.view, URL];
                if url.is_null() {
                    return String::new();
                }
                let text: Id = msg_send![url, absoluteString];
                rust_string(text).unwrap_or_default()
            }
        }

        /// Runs `script` and hands back what it returns, as text.
        pub fn eval(&self, script: &str, reply: impl FnOnce(Result<String, String>) + 'static) {
            unsafe {
                let reply = std::cell::RefCell::new(Some(reply));
                let completion = ConcreteBlock::new(move |result: Id, error: Id| {
                    let Some(reply) = reply.borrow_mut().take() else { return };
                    if !error.is_null() {
                        let description: Id = msg_send![error, localizedDescription];
                        return reply(Err(rust_string(description).unwrap_or_else(|| "JavaScript error".into())));
                    }
                    reply(Ok(rust_string(result).unwrap_or_default()))
                })
                .copy();
                let _: () = msg_send![self.view, evaluateJavaScript: ns_string(script) completionHandler: &*completion];
            }
        }

        /// What the page shows now, as `width`×`height` BGRA (rows packed, opaque).
        pub fn frame(&self, reply: impl FnOnce(Option<Vec<u8>>) + 'static) {
            let (width, height) = (self.width, self.height);
            unsafe {
                // At the video's own size: the snapshot is drawn at the screen's scale.
                let config: Id = msg_send![class!(WKSnapshotConfiguration), new];
                let points: Id = msg_send![class!(NSNumber), numberWithDouble: width as f64 / self.scale];
                let _: () = msg_send![config, setSnapshotWidth: points];
                let reply = std::cell::RefCell::new(Some(reply));
                let completion = ConcreteBlock::new(move |image: Id, _error: Id| {
                    let Some(reply) = reply.borrow_mut().take() else { return };
                    if image.is_null() {
                        return reply(None);
                    }
                    let cg: *mut c_void = msg_send![image, CGImageForProposedRect: std::ptr::null_mut::<NSRect>() context: std::ptr::null_mut::<Object>() hints: std::ptr::null_mut::<Object>()];
                    if cg.is_null() {
                        return reply(None);
                    }
                    let mut pixels = vec![0u8; width * height * 4];
                    let space = CGColorSpaceCreateDeviceRGB();
                    let context = CGBitmapContextCreate(pixels.as_mut_ptr().cast(), width, height, 8, width * 4, space, BGRA_INFO);
                    if !context.is_null() {
                        CGContextDrawImage(context, NSRect::new(NSPoint::new(0., 0.), NSSize::new(width as f64, height as f64)), cg);
                        CGContextRelease(context);
                    }
                    CGColorSpaceRelease(space);
                    reply((!context.is_null()).then_some(pixels))
                })
                .copy();
                let _: () = msg_send![self.view, takeSnapshotWithConfiguration: config completionHandler: &*completion];
                let _: () = msg_send![config, release];
            }
        }
    }

    impl Drop for Recorder {
        fn drop(&mut self) {
            unsafe {
                let _: () = msg_send![self.view, stopLoading];
                let _: () = msg_send![self.view, removeFromSuperview];
                let _: () = msg_send![self.view, release];
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    pub struct Recorder;

    impl Recorder {
        pub fn start(_: &gpui::Window, _: u32, _: u32, _: &str) -> Option<Self> {
            None
        }
        pub fn loading(&self) -> bool {
            false
        }
        pub fn address(&self) -> String {
            String::new()
        }
        pub fn eval(&self, _: &str, reply: impl FnOnce(Result<String, String>) + 'static) {
            reply(Err("not available on this system".into()))
        }
        pub fn frame(&self, reply: impl FnOnce(Option<Vec<u8>>) + 'static) {
            reply(None)
        }
    }
}

pub use imp::Recorder;
