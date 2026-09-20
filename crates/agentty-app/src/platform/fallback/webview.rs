//! Windows / Linux: no embedded browser yet (GPUI has no web view and WebKit is macOS-only), so
//! `WebView::new` returns `None`; the browser pane then offers to open pages in the default
//! browser, and agent browser tools answer with an error instead of hanging.

pub use crate::platform::url::normalize_url_with;

pub type Reply = Box<dyn FnOnce(Result<String, String>)>;

#[derive(Clone, Debug, PartialEq)]
pub struct LoadError {
    pub url: String,
    pub message: String,
}

/// A browser shortcut pressed inside the web view (never happens without one).
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

pub const NET_TOTALS: &str = "";
pub const NET_ENTRIES: &str = "";
pub const NET_DETAIL: &str = "";

const UNSUPPORTED: &str = "the in-app browser is only available on macOS";

pub struct WebView {
    _private: (),
}

impl WebView {
    pub fn new(_window: &gpui::Window, _prefs: &crate::settings::BrowserSettings) -> Option<Self> {
        None
    }

    pub fn id(&self) -> usize {
        0
    }

    pub fn has_keyboard(&self) -> bool {
        false
    }

    pub fn load(&self, _url: &str) {}

    pub fn back(&self) {}

    pub fn forward(&self) {}

    pub fn reload(&self) {}

    pub fn hard_reload(&self) {}

    pub fn current_url(&self) -> Option<String> {
        None
    }

    pub fn title(&self) -> Option<String> {
        None
    }

    pub fn load_error(&self) -> Option<LoadError> {
        None
    }

    pub fn take_popups(&self) -> Vec<String> {
        Vec::new()
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

    pub fn estimated_progress(&self) -> f32 {
        0.
    }

    pub fn set_frame(&mut self, _bounds: gpui::Bounds<gpui::Pixels>) {}

    pub fn hide(&mut self) {}
}

pub fn take_key_commands(_views: &[usize]) -> Vec<(usize, BrowserKey)> {
    Vec::new()
}

pub fn clear_website_data() {}

pub fn focus_gpui_view(_window: &gpui::Window) {}
