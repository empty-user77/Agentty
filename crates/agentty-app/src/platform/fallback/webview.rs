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

/// A browser shortcut pressed inside the web view. The panel matches on these, but without a web
/// view nothing ever makes one, so this build only reads the type.
#[allow(dead_code)]
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

/// A standard editing command; without a web view there is never a page to run one in.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditCommand {
    Copy,
    Paste,
    SelectAll,
}

pub fn perform_in_page(_command: EditCommand) -> bool {
    false
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

    pub fn new_background(_window: &gpui::Window, _prefs: &crate::settings::BrowserSettings) -> Option<Self> {
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

    pub fn call_in_world(&self, _body: &str, _args: &[(&str, &str)], _world: Option<&str>, reply: Reply) {
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

    pub fn set_zoom(&mut self, _zoom: f64) {}

    pub fn hide(&mut self) {}

    pub fn park(&mut self) {}
}

/// One cookie of the in-app browser (there is none on this platform).
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub expires: Option<f64>,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: Option<String>,
}

pub fn cookies(_keep: impl Fn(&str) -> bool + 'static, reply: impl FnOnce(Vec<Cookie>) + 'static) {
    reply(Vec::new());
}

pub fn set_cookies(_cookies: &[Cookie]) {}

pub fn any_view() -> bool {
    false
}

pub fn take_key_commands(_views: &[usize]) -> Vec<(usize, BrowserKey)> {
    Vec::new()
}

pub fn clear_website_data() {}

pub fn focus_gpui_view(_window: &gpui::Window) {}
