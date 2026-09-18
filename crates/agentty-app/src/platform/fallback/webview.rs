//! Windows / Linux: no embedded browser yet (GPUI has no web view and WebKit is macOS-only), so
//! `WebView::new` returns `None`; the browser pane then offers to open pages in the default
//! browser, and agent browser tools answer with an error instead of hanging.

pub use crate::platform::url::normalize_url_with;

pub type Reply = Box<dyn FnOnce(Result<String, String>)>;

const UNSUPPORTED: &str = "the in-app browser is only available on macOS";

pub struct WebView {
    _private: (),
}

impl WebView {
    pub fn new(_window: &gpui::Window, _prefs: &crate::settings::BrowserSettings) -> Option<Self> {
        None
    }

    pub fn load(&self, _url: &str) {}

    pub fn back(&self) {}

    pub fn forward(&self) {}

    pub fn reload(&self) {}

    pub fn current_url(&self) -> Option<String> {
        None
    }

    pub fn title(&self) -> Option<String> {
        None
    }

    pub fn call_async(&self, _body: &str, _args: &[(&str, &str)], reply: Reply) {
        reply(Err(UNSUPPORTED.into()));
    }

    pub fn snapshot_png(&self, _path: std::path::PathBuf, reply: Reply) {
        reply(Err(UNSUPPORTED.into()));
    }

    pub fn is_loading(&self) -> bool {
        false
    }

    pub fn set_frame(&mut self, _bounds: gpui::Bounds<gpui::Pixels>) {}

    pub fn hide(&mut self) {}
}

pub fn clear_website_data() {}

pub fn focus_gpui_view(_window: &gpui::Window) {}
