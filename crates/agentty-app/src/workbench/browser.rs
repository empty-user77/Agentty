//! In-app browser panel docked to the right of the terminals (like cmux's browser split).
//! Links open here or in the default browser, per Settings → General.
//!
//! The panel holds several tabs, each with its own native web view: pages that ask for a new
//! window (`window.open`, `target="_blank"`) open as a tab rather than being dropped, and the
//! browser shortcuts (⌘R, ⌘⇧R, ⌘T, ⌘W, ⌘L, ⌘[, ⌘]) work while the page has the keyboard.
//! Under the page sits a small network panel: how much the page pulled over the network, and
//! every API call it made with its request and response.

use super::Workbench;
use crate::i18n::{t, tf};
use crate::settings::{settings, LinkOpener};
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::{hex, Chrome};
use crate::ui::{icon, icon_only, icon_only_sized, Tooltip, TypeScale};
use crate::webview::{normalize_url_with, BrowserKey, LoadError, WebView, NET_DETAIL, NET_ENTRIES, NET_TOTALS};
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, Entity, Focusable, FontWeight, SharedString, Subscription, Window};
use serde::Deserialize;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Height of the tab strip, the toolbar and the network status bar.
const TAB_STRIP: f32 = 28.;
const TOOLBAR: f32 = 36.;
const STATUS_BAR: f32 = 24.;
/// Most pages the panel keeps open at once (each one is a native web view of its own).
const MAX_TABS: usize = 16;
/// How often the page is asked for its network summary (and, while open, its calls).
const NET_INTERVAL: Duration = Duration::from_millis(900);
const NET_INTERVAL_OPEN: Duration = Duration::from_millis(400);

/// What the user typed as a URL, or a search with their chosen engine.
pub(super) fn browser_url(input: &str, cx: &gpui::App) -> String {
    normalize_url_with(input, settings(cx).browser.search_engine.query_prefix())
}

/// One recorded API call, without its bodies (those are fetched when a call is opened).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct NetCall {
    id: u64,
    kind: String,
    method: String,
    url: String,
    status: u16,
    duration: u64,
    size: u64,
    #[serde(rename = "type")]
    mime: String,
    error: String,
}

/// Everything about one call: its headers and its (truncated) request and response bodies.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct NetDetail {
    id: u64,
    method: String,
    url: String,
    status: u16,
    status_text: String,
    duration: u64,
    size: u64,
    #[serde(rename = "type")]
    mime: String,
    error: String,
    req_body: String,
    body: String,
    req_headers: Vec<(String, String)>,
    res_headers: Vec<(String, String)>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(default)]
struct NetTotals {
    /// Bytes that actually came over the network (compressed, cache hits excluded).
    transferred: u64,
    /// Bytes the page ended up with once decompressed.
    decoded: u64,
    /// Resources fetched (documents, scripts, styles, images, …).
    count: u32,
    /// `fetch` / `XMLHttpRequest` calls recorded.
    calls: u32,
}

/// Answers from the page, written by the JavaScript callbacks and read on the next poll tick.
#[derive(Default)]
struct NetInbox {
    totals: Option<String>,
    calls: Option<String>,
    detail: Option<Result<String, String>>,
    totals_pending: bool,
    calls_pending: bool,
    detail_pending: bool,
}

/// What the network panel knows about one tab.
#[derive(Default)]
struct Network {
    totals: NetTotals,
    calls: Vec<NetCall>,
    /// The call the user opened, and its details once the page answered.
    selected: Option<u64>,
    detail: Option<Result<NetDetail, String>>,
    polled_at: Option<Instant>,
    inbox: Rc<RefCell<NetInbox>>,
}

pub struct BrowserTab {
    pub(super) webview: Rc<RefCell<Option<WebView>>>,
    /// URL to load once the native view exists (it needs the window).
    pub(super) pending: Option<String>,
    title: String,
    loading: bool,
    /// Progress of the current load (0.0–1.0), shown as the bar under the toolbar.
    progress: f32,
    /// When the last load finished: the full bar stays up briefly so even a quick reload shows.
    finished_at: Option<Instant>,
    /// The page could not be opened: shown instead of the web view, with a retry.
    error: Option<LoadError>,
    /// Address of the page this tab shows.
    url: String,
    net: Network,
    /// The plugin page this tab shows (`browser/*`): the page is the plugin's, the tab only lends
    /// it a place on screen, and closing the tab sends it back out of sight.
    pub(super) owner: Option<u64>,
    /// Name of the plugin driving it, shown on the tab.
    owner_name: Option<String>,
}

impl BrowserTab {
    fn new(url: String) -> Self {
        Self {
            webview: Rc::new(RefCell::new(None)),
            pending: Some(url.clone()),
            title: String::new(),
            loading: true,
            progress: 0.,
            finished_at: None,
            error: None,
            url,
            net: Network::default(),
            owner: None,
            owner_name: None,
        }
    }

    /// A tab showing a plugin's page (already loaded, owned by the plugin).
    pub(super) fn for_plugin(webview: Rc<RefCell<Option<WebView>>>, url: String, owner: u64, plugin: String) -> Self {
        Self { webview, pending: None, loading: false, owner: Some(owner), owner_name: Some(plugin), ..Self::new(url) }
    }

    /// The load just finished and the full bar is still shown.
    fn finishing(&self) -> bool {
        self.finished_at.is_some_and(|at| at.elapsed() < Duration::from_millis(350))
    }

    fn view_id(&self) -> Option<usize> {
        self.webview.borrow().as_ref().map(|view| view.id())
    }

    /// What the tab is called in the strip: the page title, else its host, else "New tab".
    fn label(&self, cx: &gpui::App) -> String {
        let base = self.page_label(cx);
        match &self.owner_name {
            Some(plugin) => format!("{plugin} · {base}"),
            None => base,
        }
    }

    fn page_label(&self, cx: &gpui::App) -> String {
        if !self.title.is_empty() {
            return self.title.clone();
        }
        let address = self.pending.clone().unwrap_or_else(|| self.url.clone());
        let host = address
            .split_once("://")
            .map(|(_, rest)| rest)
            .unwrap_or(&address)
            .split(['/', '?', '#'])
            .next()
            .unwrap_or_default()
            .trim_start_matches("www.");
        if host.is_empty() {
            t(cx, "browser.new_tab").to_string()
        } else {
            host.to_string()
        }
    }
}

pub struct BrowserPanel {
    pub(super) tabs: Vec<BrowserTab>,
    pub(super) active: usize,
    address: Entity<TextInput>,
    /// Last URL written into the address bar by navigation; anything else there is the user typing.
    synced_url: String,
    /// The network panel under the page is open.
    network_open: bool,
    /// Shortcuts pressed inside a web view, acted on at the next render (they need the window).
    keys: Vec<(usize, BrowserKey)>,
    calls_scroll: gpui::ScrollHandle,
    detail_scroll: gpui::ScrollHandle,
    /// Responsive mode: the page at a device's size.
    pub(super) responsive: super::responsive::Responsive,
    _subscription: Subscription,
}

impl BrowserPanel {
    /// Every tab's address (the one it is loading, if any), for the saved layout.
    pub(super) fn tab_urls(&self) -> Vec<String> {
        // A plugin's page is the plugin's to open again, not the panel's.
        self.tabs.iter().filter(|tab| tab.owner.is_none()).map(|tab| tab.pending.clone().unwrap_or_else(|| tab.url.clone())).collect()
    }

    fn tab(&self) -> &BrowserTab {
        &self.tabs[self.active.min(self.tabs.len() - 1)]
    }

    fn tab_mut(&mut self) -> &mut BrowserTab {
        let index = self.active.min(self.tabs.len() - 1);
        &mut self.tabs[index]
    }

    /// The native view of the tab in front, for the agent commands and the toolbar buttons.
    pub(super) fn webview(&self) -> Rc<RefCell<Option<WebView>>> {
        self.tab().webview.clone()
    }

    /// Whether the tab in front is ready for a command (its view exists, nothing queued).
    pub(super) fn ready(&self) -> bool {
        let tab = self.tab();
        tab.webview.borrow().is_some() && tab.pending.is_none()
    }

    /// What the debug driver reports about the panel: its tabs and the network it recorded.
    pub(super) fn debug_state(&self) -> serde_json::Value {
        let tabs: Vec<serde_json::Value> = self
            .tabs
            .iter()
            .map(|tab| serde_json::json!({ "url": tab.url, "title": tab.title, "loading": tab.loading, "error": tab.error.as_ref().map(|e| e.message.clone()) }))
            .collect();
        let net = &self.tab().net;
        serde_json::json!({
            "active": self.active,
            "focused": self.tab().webview.borrow().as_ref().is_some_and(|view| view.has_keyboard()),
            "tabs": tabs,
            "networkOpen": self.network_open,
            "transferred": net.totals.transferred,
            "decoded": net.totals.decoded,
            "resources": net.totals.count,
            "calls": net.calls.iter().map(|c| format!("{} {} {} {}B", c.method, c.status, c.url, c.size)).collect::<Vec<_>>(),
            "selected": net.selected,
            "detail": net.detail.as_ref().map(|d| match d {
                // The body is only counted, never quoted: it may carry a token or personal data.
                Ok(detail) => format!("{} {} body={} chars", detail.status, detail.url, detail.body.chars().count()),
                Err(error) => format!("error: {error}"),
            }),
        })
    }

    /// Address of the page shown (or about to be).
    pub(super) fn current_url(&self) -> Option<String> {
        let tab = self.tab();
        tab.pending.clone().or_else(|| tab.webview.borrow().as_ref().and_then(|view| view.current_url()))
    }
}

impl Workbench {
    /// Opens `url` according to the user's choice (in-app panel or the default browser).
    /// Opens a link the app produced (a pull request, a port, a ⌘-click in a terminal).
    ///
    /// Anything that is not on this machine goes to the browser the user is already signed in to:
    /// a pull request, a ticket or a cloud console asks for a login, and signing in inside the
    /// embedded browser is a fight nobody should have. The in-app browser keeps what it is good
    /// at — a dev server, a local page, a file — and its own address bar still loads anything the
    /// user types there on purpose.
    pub(super) fn open_link(&mut self, url: String, cx: &mut Context<Self>) {
        let in_app = settings(cx).link_opener == LinkOpener::InApp && is_local_url(&url);
        if in_app {
            self.open_browser(Some(url), cx);
        } else {
            cx.open_url(&url);
        }
    }

    pub(super) fn toggle_browser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.browser.take().is_some() {
            crate::webview::focus_gpui_view(window);
            self.focus_active(window, cx);
            return cx.notify();
        }
        self.open_browser(None, cx);
    }

    /// Shows the panel (loading `url` in the tab in front). The panel is built on the next render.
    pub(super) fn open_browser(&mut self, url: Option<String>, cx: &mut Context<Self>) {
        if !crate::platform::HAS_WEBVIEW {
            // No embedded browser on this platform: the default browser opens the page instead.
            return cx.open_url(&url.unwrap_or_else(|| browser_url(&settings(cx).browser.home, cx)));
        }
        self.page = None;
        match self.browser.as_mut() {
            Some(browser) => {
                if let Some(url) = url {
                    browser.tab_mut().pending = Some(url);
                }
            }
            None => self.browser_request = Some(url.unwrap_or_else(|| browser_url(&settings(cx).browser.home, cx))),
        }
        cx.notify();
    }

    /// Whether the keyboard is in the browser's address bar (⌘W then means "close this page").
    pub(super) fn browser_address_focused(&self, window: &Window, cx: &Context<Self>) -> bool {
        self.browser.as_ref().is_some_and(|browser| browser.address.read(cx).focus_handle(cx).is_focused(window))
    }

    /// ⌘T and "+" leave the keyboard in the address bar, ready for an address, as a browser does.
    fn focus_browser_address(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(browser) = self.browser.as_ref() else { return };
        let address = browser.address.clone();
        crate::webview::focus_gpui_view(window);
        address.update(cx, |input, cx| input.focus_and_select_all(window, cx));
    }

    /// Opens `url` in a new tab of the panel (⌘T, a page asking for a new window, the "+" button).
    /// Pages ask for new windows too, so the count is capped: a page in a loop must not fill the
    /// panel (and the memory of one web view per tab) without end.
    pub(super) fn open_browser_tab(&mut self, url: Option<String>, cx: &mut Context<Self>) {
        // In a plugin's workspace the browser's "+" (and ⌘T) starts a new automation of the plugin.
        if url.is_none() {
            if let Some(plugin) = self.front_plugin_workspace(cx).filter(|_| self.page.is_none()) {
                return self.new_plugin_instance(&plugin, cx);
            }
        }
        let Some(browser) = self.browser.as_mut() else { return self.open_browser(url, cx) };
        if browser.tabs.len() >= MAX_TABS {
            return;
        }
        let url = url.unwrap_or_else(|| browser_url(&settings(cx).browser.home, cx));
        browser.tabs.push(BrowserTab::new(url));
        browser.active = browser.tabs.len() - 1;
        self.show_address(cx);
        cx.notify();
    }

    /// Closes one tab; the last one closes the panel.
    pub(super) fn close_browser_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(browser) = self.browser.as_mut() else { return };
        if index >= browser.tabs.len() {
            return;
        }
        if browser.tabs.len() == 1 {
            return self.toggle_browser(window, cx);
        }
        browser.tabs.remove(index);
        if browser.active > index {
            browser.active -= 1;
        }
        browser.active = browser.active.min(browser.tabs.len() - 1);
        self.show_address(cx);
        cx.notify();
    }

    pub(super) fn select_browser_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(browser) = self.browser.as_mut() else { return };
        if index >= browser.tabs.len() || browser.active == index {
            return;
        }
        browser.active = index;
        self.show_address(cx);
        cx.notify();
    }

    /// Puts the tab in front's address into the address bar.
    fn show_address(&mut self, cx: &mut Context<Self>) {
        let Some(browser) = self.browser.as_mut() else { return };
        let url = browser.tab().pending.clone().unwrap_or_else(|| browser.tab().url.clone());
        browser.synced_url = url.clone();
        let address = browser.address.clone();
        address.update(cx, |input, cx| input.set_text(url, cx));
    }

    /// Materializes a requested panel and keeps the native views in sync; called from render.
    pub(super) fn prepare_browser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(url) = self.browser_request.take() {
            // After a restart: every tab that was open, not only the one in front.
            let (tabs, active) = match self.browser_restore.take() {
                Some((urls, active)) if !urls.is_empty() => {
                    let active = active.min(urls.len() - 1);
                    (urls.into_iter().take(MAX_TABS).map(BrowserTab::new).collect::<Vec<_>>(), active.min(MAX_TABS - 1))
                }
                _ => (vec![BrowserTab::new(url.clone())], 0),
            };
            self.build_browser_panel(tabs, active, url, window, cx);
        }
        self.sync_plugin_instances(cx);
        if let Some(tabs) = self.sync_plugin_browsers(cx) {
            match self.browser.as_mut() {
                Some(browser) => {
                    browser.tabs.extend(tabs);
                    browser.active = browser.tabs.len() - 1;
                    self.show_address(cx);
                }
                None => {
                    let url = tabs[0].url.clone();
                    let active = tabs.len() - 1;
                    self.build_browser_panel(tabs, active, url, window, cx);
                }
            }
        }
        self.place_browser_views(window, cx);
    }

    fn build_browser_panel(&mut self, tabs: Vec<BrowserTab>, active: usize, url: String, window: &mut Window, cx: &mut Context<Self>) {
        {
            let address = cx.new(|cx| TextInput::localized(url.clone(), "browser.address", window, cx));
            let subscription = cx.subscribe(&address, |this, input, event: &TextInputEvent, cx| {
                if matches!(event, TextInputEvent::Confirmed) {
                    let url = browser_url(input.read(cx).text(), cx);
                    if let Some(browser) = this.browser.as_mut() {
                        browser.synced_url = input.read(cx).text().to_string();
                        browser.tab_mut().pending = Some(url);
                    }
                    cx.notify();
                }
            });
            self.browser = Some(BrowserPanel {
                tabs,
                active,
                address,
                synced_url: url,
                network_open: false,
                keys: Vec::new(),
                calls_scroll: gpui::ScrollHandle::new(),
                detail_scroll: gpui::ScrollHandle::new(),
                responsive: super::responsive::Responsive::new(window, cx),
                _subscription: subscription,
            });
            self.watch_browser(cx);
        }
    }

    fn place_browser_views(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Pages (settings, Git, …) take the whole area: close the browser rather than hide it.
        if self.page.is_some() && self.browser.take().is_some() {
            crate::webview::focus_gpui_view(window);
        }
        self.apply_browser_keys(window, cx);
        // Menus and popups sit on top of it: hide the native views meanwhile.
        let hidden = self.overlay_open();
        let prefs = settings(cx).browser.clone();
        let Some(browser) = self.browser.as_mut() else { return };
        let active = browser.active.min(browser.tabs.len() - 1);
        let mut load_active = None;
        for (index, tab) in browser.tabs.iter_mut().enumerate() {
            if tab.webview.borrow().is_none() {
                *tab.webview.borrow_mut() = WebView::new(window, &prefs);
            }
            // Only the tab in front is on screen; the others keep loading behind it.
            if index != active || hidden {
                if let Some(view) = tab.webview.borrow_mut().as_mut() {
                    view.hide();
                }
            }
            let Some(url) = tab.pending.clone() else { continue };
            let borrowed = tab.webview.borrow();
            let Some(view) = borrowed.as_ref() else { continue };
            view.load(&url);
            drop(borrowed);
            tab.pending = None;
            tab.loading = true;
            tab.progress = 0.;
            tab.finished_at = None;
            tab.error = None;
            tab.url = url.clone();
            tab.net = Network::default();
            if index == active {
                load_active = Some(url);
            }
        }
        // The address bar shows where the tab in front is going, as a browser does.
        if let Some(url) = load_active {
            browser.synced_url = url.clone();
            let address = browser.address.clone();
            if address.read(cx).text() != url {
                address.update(cx, |input, cx| input.set_text(url, cx));
            }
        }
    }

    /// Acts on the shortcuts the web views recorded (they need the window, so not in the poll loop).
    fn apply_browser_keys(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(browser) = self.browser.as_mut() else { return };
        let keys = std::mem::take(&mut browser.keys);
        for (view, key) in keys {
            let Some(index) = self.browser.as_ref().and_then(|b| b.tabs.iter().position(|t| t.view_id() == Some(view))) else { continue };
            self.select_browser_tab(index, cx);
            match key {
                BrowserKey::Reload => self.reload_browser(false, cx),
                BrowserKey::HardReload => self.reload_browser(true, cx),
                BrowserKey::NewTab => {
                    self.open_browser_tab(None, cx);
                    self.focus_browser_address(window, cx);
                }
                BrowserKey::CloseTab => self.close_browser_tab(index, window, cx),
                BrowserKey::Back | BrowserKey::Forward => {
                    let Some(browser) = self.browser.as_ref() else { continue };
                    if let Some(view) = browser.webview().borrow().as_ref() {
                        if key == BrowserKey::Back {
                            view.back();
                        } else {
                            view.forward();
                        }
                    }
                }
                BrowserKey::FocusAddress => {
                    let Some(browser) = self.browser.as_ref() else { continue };
                    let address = browser.address.clone();
                    crate::webview::focus_gpui_view(window);
                    address.update(cx, |input, cx| input.focus_and_select_all(window, cx));
                }
            }
            cx.notify();
        }
    }

    /// Follows what the pages do: address bar, title, progress, failures, pages asking for a new
    /// window, shortcuts pressed inside them, and the network summary. Polled quickly while a page
    /// loads so the bar moves smoothly, slowly otherwise.
    fn watch_browser(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let mut interval = Duration::from_millis(150);
            loop {
                cx.background_executor().timer(interval).await;
                let alive = this.update(cx, |this, cx| {
                    let Some(browser) = this.browser.as_mut() else { return false };
                    let active = browser.active.min(browser.tabs.len() - 1);
                    let mut changed = false;
                    let mut busy = false;
                    let mut popups = Vec::new();
                    let mut views = Vec::new();
                    for (index, tab) in browser.tabs.iter_mut().enumerate() {
                        let borrowed = tab.webview.borrow();
                        let Some(view) = borrowed.as_ref() else { continue };
                        views.push(view.id());
                        popups.extend(view.take_popups());
                        let (url, title, loading, progress, error) = (
                            view.current_url(),
                            view.title().unwrap_or_default(),
                            view.is_loading(),
                            view.estimated_progress(),
                            view.load_error(),
                        );
                        drop(borrowed);
                        changed |=
                            title != tab.title || loading != tab.loading || (loading && progress != tab.progress) || error != tab.error;
                        // The failed address stays in view (the web view still has the previous page's URL).
                        if let Some(error) = error.as_ref().filter(|e| !e.url.is_empty() && tab.error.as_ref() != Some(e)) {
                            tab.url = error.url.clone();
                        }
                        let failed = error.is_some();
                        tab.error = error;
                        if tab.loading && !loading {
                            tab.finished_at = Some(Instant::now());
                        }
                        let finishing = tab.finishing();
                        if !loading && !finishing && tab.finished_at.take().is_some() {
                            changed = true; // the full bar goes away
                        }
                        busy |= loading || finishing;
                        tab.title = title;
                        tab.loading = loading;
                        tab.progress = if loading { progress } else { 1. };
                        if let Some(url) = url.filter(|u| *u != tab.url && !failed) {
                            tab.url = url;
                            changed |= index == active;
                        }
                    }
                    changed |= this.poll_browser_network(cx);
                    // The address bar follows the tab in front, unless the user is typing in it.
                    if let Some(browser) = this.browser.as_mut() {
                        let url = browser.tab().url.clone();
                        let typing = browser.address.read(cx).text() != browser.synced_url;
                        if !typing && url != browser.synced_url && browser.tab().pending.is_none() {
                            browser.synced_url = url.clone();
                            let address = browser.address.clone();
                            address.update(cx, |input, cx| input.set_text(url, cx));
                            changed = true;
                        }
                        let keys = crate::webview::take_key_commands(&views);
                        if !keys.is_empty() {
                            browser.keys.extend(keys);
                            changed = true;
                        }
                    }
                    for url in popups {
                        this.open_browser_tab(Some(url), cx);
                        changed = true;
                    }
                    interval = Duration::from_millis(if busy { 50 } else { 150 });
                    if changed {
                        cx.notify();
                    }
                    true
                });
                if !matches!(alive, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// Reads what the page answered about its network use, and asks it again when due.
    fn poll_browser_network(&mut self, cx: &mut Context<Self>) -> bool {
        let _ = cx;
        let Some(browser) = self.browser.as_mut() else { return false };
        let open = browser.network_open;
        let tab = browser.tab_mut();
        let borrowed = tab.webview.borrow();
        let Some(view) = borrowed.as_ref() else { return false };
        let mut changed = false;
        let inbox = tab.net.inbox.clone();
        {
            let mut inbox = inbox.borrow_mut();
            if let Some(json) = inbox.totals.take() {
                let totals: NetTotals = serde_json::from_str(&json).unwrap_or_default();
                changed |= totals.transferred != tab.net.totals.transferred || totals.calls != tab.net.totals.calls;
                tab.net.totals = totals;
            }
            if let Some(json) = inbox.calls.take() {
                let calls: Vec<NetCall> = serde_json::from_str(&json).unwrap_or_default();
                changed |= calls.len() != tab.net.calls.len() || calls.last().map(|c| c.status) != tab.net.calls.last().map(|c| c.status);
                tab.net.calls = calls;
            }
            if let Some(result) = inbox.detail.take() {
                tab.net.detail = Some(match result {
                    Ok(json) => serde_json::from_str::<NetDetail>(&json).map_err(|e| e.to_string()),
                    Err(error) => Err(error),
                });
                changed = true;
            }
        }
        let due = tab.net.polled_at.is_none_or(|at| at.elapsed() >= if open { NET_INTERVAL_OPEN } else { NET_INTERVAL });
        if due {
            tab.net.polled_at = Some(Instant::now());
            let mut inbox_mut = inbox.borrow_mut();
            if !inbox_mut.totals_pending {
                inbox_mut.totals_pending = true;
                let inbox = inbox.clone();
                view.call_async(
                    NET_TOTALS,
                    &[],
                    Box::new(move |result| {
                        let mut inbox = inbox.borrow_mut();
                        inbox.totals_pending = false;
                        inbox.totals = result.ok();
                    }),
                );
            }
            if open && !inbox_mut.calls_pending {
                inbox_mut.calls_pending = true;
                let inbox = inbox.clone();
                view.call_async(
                    NET_ENTRIES,
                    &[],
                    Box::new(move |result| {
                        let mut inbox = inbox.borrow_mut();
                        inbox.calls_pending = false;
                        inbox.calls = result.ok();
                    }),
                );
            }
        }
        // A call the user opened: fetched once, then kept until another one is opened.
        let wanted = tab.net.selected.filter(|_| tab.net.detail.is_none());
        if let Some(id) = wanted {
            let mut inbox_mut = inbox.borrow_mut();
            if !inbox_mut.detail_pending {
                inbox_mut.detail_pending = true;
                let inbox = inbox.clone();
                view.call_async(
                    NET_DETAIL,
                    &[("id", &id.to_string())],
                    Box::new(move |result| {
                        let mut inbox = inbox.borrow_mut();
                        inbox.detail_pending = false;
                        inbox.detail = Some(result);
                    }),
                );
            }
        }
        changed
    }

    /// The reload button, "Try again" and ⌘R / ⌘⇧R. A page that failed to load (a dev server not
    /// up yet) is not the web view's URL — WebKit's reload would load the previous page, or nothing
    /// when there was none — so the failed address is opened again instead.
    pub(super) fn reload_browser(&mut self, hard: bool, cx: &mut Context<Self>) {
        let Some(browser) = self.browser.as_mut() else { return };
        let fallback = browser.synced_url.clone();
        let tab = browser.tab_mut();
        if let Some(view) = tab.webview.borrow().as_ref() {
            match (tab.error.take(), view.current_url()) {
                (Some(error), _) if !error.url.is_empty() => view.load(&error.url),
                (_, Some(_)) if hard => view.hard_reload(),
                (_, Some(_)) => view.reload(),
                (_, None) => view.load(&browser_url(&fallback, cx)),
            }
        }
        tab.loading = true;
        tab.progress = 0.;
        tab.finished_at = None;
        tab.net = Network::default();
        cx.notify();
    }

    /// Whether something drawn by GPUI would sit on top of the native web view.
    fn overlay_open(&self) -> bool {
        self.page.is_some()
            || self.launcher_open
            || self.notices_open
            || self.picker.is_some()
            || self.palette.is_some()
            || self.updates.popup
            || self.about_open
            || self.tab_menu.is_some()
            || self.status_menu.is_some()
            || self.branch_menu.is_some()
            || self.resume_menu.is_some()
            || self.close_confirm.is_some()
            || self.agent_panel.is_some()
            || self.prompt_dialog.is_some()
            || self.harness_dialog.is_some()
            || self.onboarding.as_ref().is_some_and(|o| o.is_modal())
            // Native views swallow mouse events; hide it so the resize drags keep reaching GPUI.
            || self.browser_resizing
            || self.browser_net_drag.is_some()
            // The device list opens over the page; an edge drag must keep reaching GPUI.
            || self.browser.as_ref().is_some_and(|b| b.responsive.menu || b.responsive.dragging)
    }

    /// Splitter between the terminals and the browser: a real layout column (not overlapping the
    /// native view, which would swallow the mouse).
    pub(super) fn render_browser_splitter(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.browser.as_ref()?;
        Some(
            div()
                .id("browser-resize")
                .group("browser-resize")
                .w(px(5.))
                .flex_shrink_0()
                .h_full()
                .flex()
                .justify_center()
                .cursor(gpui::CursorStyle::ResizeLeftRight)
                // A hairline at rest; the whole grab area lights up on hover or while dragging.
                .child(
                    div()
                        .w(px(1.))
                        .h_full()
                        .bg(hex(Chrome::BORDER))
                        .group_hover("browser-resize", |s| s.w(px(5.)).bg(hex(Chrome::ACCENT)))
                        .when(self.browser_resizing, |d| d.w(px(5.)).bg(hex(Chrome::ACCENT))),
                )
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.browser_resizing = true;
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
                .into_any_element(),
        )
    }

    pub(super) fn render_browser(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let browser = self.browser.as_ref()?;
        let tab = browser.tab();
        let webview = tab.webview.clone();
        let hidden = self.overlay_open();
        // A failed load shows its message in place of the page (the native view would cover it).
        let covered = hidden || tab.error.is_some();
        if covered {
            if let Some(view) = webview.borrow_mut().as_mut() {
                view.hide();
            }
        }
        // The native view is positioned over this element after layout. In responsive mode the
        // element is the device-sized frame and the page zoom lays it out at the device's width.
        let placeholder = webview.clone();
        let zoom = match browser.responsive.viewport {
            Some(_) => browser.responsive.scale() as f64,
            None => settings(cx).browser.zoom as f64,
        };
        let content = gpui::canvas(
            |_, _, _| {},
            move |bounds, _, _, _| {
                if let Some(view) = placeholder.borrow_mut().as_mut() {
                    if !covered {
                        view.set_zoom(zoom);
                        view.set_frame(bounds);
                    }
                }
            },
        )
        .flex_1()
        .size_full()
        .into_any_element();
        let content = self.render_responsive_stage(&browser.responsive, content, cx);
        Some(
            div()
                .relative()
                // In a plugin's workspace the browser fills its part of the layout.
                .map(|d| {
                    if self.front_plugin_workspace(cx).is_some() && self.page.is_none() {
                        d.flex_1().min_w_0().w_full()
                    } else {
                        d.w(px(self.docked_widths(cx).0)).flex_shrink_0()
                    }
                })
                .h_full()
                .flex()
                .flex_col()
                .bg(hex(0xffffff))
                .child(self.render_browser_tabs(browser, cx))
                .child(self.render_browser_toolbar(browser, cx))
                .children(self.render_sign_in_banner(tab.owner, cx))
                .children(self.render_responsive_bar(&browser.responsive, cx))
                .child(progress_bar(tab.loading || tab.finishing(), tab.progress))
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .relative()
                        .child(content)
                        .when_some(tab.error.clone().filter(|_| !hidden), |d, error| d.child(self.render_load_error(error, cx)))
                        .when(hidden, |d| {
                            d.child(
                                div()
                                    .absolute()
                                    .inset_0()
                                    .bg(hex(Chrome::EDITOR))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .t_small()
                                    .text_color(hex(Chrome::MUTED))
                                    .child(tab.title.clone()),
                            )
                        })
                        // Over everything else in the page area, the native view included (it is
                        // hidden while the list is open).
                        .children(self.render_device_menu(cx)),
                )
                .when(browser.network_open, |d| d.child(self.render_network_panel(browser, cx)))
                .child(self.render_network_bar(browser, cx))
                .into_any_element(),
        )
    }

    /// The tab strip: one row per open page, and "+" for a new one.
    /// The tab strip: one rounded chip per open page, then "+" for a new one. Deliberately unlike
    /// the workbench's square terminal tabs right above it — these are the browser's own pages.
    fn render_browser_tabs(&self, browser: &BrowserPanel, cx: &mut Context<Self>) -> AnyElement {
        let active = browser.active.min(browser.tabs.len() - 1);
        let mut strip = div().id("browser-tabs").flex().flex_1().min_w_0().h_full().items_center().gap_1().overflow_x_scroll();
        for (index, tab) in browser.tabs.iter().enumerate() {
            let selected = index == active;
            let label = tab.label(cx);
            strip = strip.child(
                div()
                    .id(SharedString::from(format!("browser-tab-{index}")))
                    .h(px(20.))
                    .flex()
                    .flex_shrink()
                    .min_w(px(58.))
                    .max_w(px(170.))
                    .items_center()
                    .gap_1()
                    .pl_2()
                    .pr_0p5()
                    .rounded_full()
                    .t_caption()
                    .cursor_pointer()
                    .when(selected, |d| d.bg(hex(Chrome::SELECTED)).text_color(hex(Chrome::BRIGHT)).font_weight(FontWeight::MEDIUM))
                    .when(!selected, |d| {
                        d.text_color(hex(Chrome::MUTED)).hover(|s| s.bg(hex(Chrome::HOVER)).text_color(hex(Chrome::BRIGHT)))
                    })
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.select_browser_tab(index, cx)))
                    .when(tab.loading, |d| d.child(icon("loader-circle", 10., hex(Chrome::ACCENT))))
                    .child(div().flex_1().min_w_0().truncate().child(label))
                    .child(
                        div()
                            .id(SharedString::from(format!("browser-tab-close-{index}")))
                            .flex_shrink_0()
                            .size(px(15.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .hover(|s| s.bg(hex(Chrome::OVERLAY_BORDER)))
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                cx.stop_propagation();
                                this.close_browser_tab(index, window, cx)
                            }))
                            .child(icon("x", 9., hex(Chrome::MUTED))),
                    ),
            );
        }
        // "+" sits right after the last tab, where a browser puts it.
        strip = strip.child(
            div()
                .id("browser-tab-new")
                .flex_shrink_0()
                .size(px(20.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .hover(|s| s.bg(hex(Chrome::HOVER)))
                .tooltip(Tooltip::text(t(cx, "browser.new_tab"), Some("cmd-t")))
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.open_browser_tab(None, cx);
                    this.focus_browser_address(window, cx);
                }))
                .child(icon("plus", 12., hex(Chrome::MUTED))),
        );
        div()
            .h(px(TAB_STRIP))
            .flex_shrink_0()
            .px_1p5()
            .flex()
            .items_center()
            .gap_1p5()
            .border_b_1()
            .border_color(hex(Chrome::BORDER))
            .bg(hex(Chrome::PANEL))
            // Marks the row as the browser's pages, not the workbench's terminal tabs.
            .child(icon("globe", 12., hex(Chrome::MUTED)))
            .child(strip)
            .into_any_element()
    }

    fn render_browser_toolbar(&self, browser: &BrowserPanel, cx: &mut Context<Self>) -> AnyElement {
        let tab = browser.tab();
        let nav = |id: &'static str, glyph: &'static str, action: fn(&WebView), cx: &mut Context<Self>| {
            let webview = browser.webview();
            icon_only(
                id,
                glyph,
                cx.listener(move |_, _: &ClickEvent, _, _| {
                    if let Some(view) = webview.borrow().as_ref() {
                        action(view);
                    }
                }),
            )
        };
        let external = browser.address.clone();
        div()
            .h(px(TOOLBAR))
            .flex_shrink_0()
            .px_1()
            .flex()
            .items_center()
            .gap_0p5()
            .border_b_1()
            .border_color(hex(Chrome::BORDER))
            .bg(hex(Chrome::SIDE_BAR))
            .child(nav("browser-back", "arrow-left", WebView::back, cx))
            .child(nav("browser-forward", "arrow-right", WebView::forward, cx))
            .child(
                icon_only(
                    "browser-reload",
                    "rotate-cw",
                    cx.listener(|this, event: &ClickEvent, _, cx| {
                        // ⇧-click reloads the hard way, like a browser's reload button.
                        this.reload_browser(event.modifiers().shift, cx)
                    }),
                )
                .tooltip(Tooltip::text(t(cx, "browser.hard_reload"), Some("cmd-shift-r"))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .mx_1()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(hex(if tab.loading { Chrome::ACCENT } else { Chrome::BORDER }))
                    .bg(hex(0x1a1a1a))
                    .t_small()
                    .text_color(hex(Chrome::BRIGHT))
                    .child(browser.address.clone()),
            )
            .child(self.render_responsive_button(browser.responsive.viewport.is_some(), cx))
            .child(icon_only(
                "browser-external",
                "arrow-up-right",
                cx.listener(move |_, _: &ClickEvent, _, cx| {
                    let url = external.read(cx).text().to_string();
                    cx.open_url(&browser_url(&url, cx));
                }),
            ))
            .child(icon_only(
                "browser-settings",
                "settings",
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.settings_section = super::settings_page::SettingsSection::Browser;
                    this.page = Some(super::Page::Settings);
                    cx.notify();
                }),
            ))
            .child(icon_only("browser-close", "x", cx.listener(|this, _: &ClickEvent, window, cx| this.toggle_browser(window, cx))))
            .into_any_element()
    }
}

impl Workbench {
    /// "This page couldn't be opened": the address, WebKit's reason and a retry.
    fn render_load_error(&self, error: LoadError, cx: &mut Context<Self>) -> AnyElement {
        div()
            .absolute()
            .inset_0()
            .bg(hex(Chrome::EDITOR))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .px_6()
            .child(div().t_body().text_color(hex(Chrome::BRIGHT)).child(t(cx, "browser.load_failed")))
            .child(div().t_small().text_color(hex(Chrome::MUTED)).text_center().child(error.url))
            .when(!error.message.is_empty(), |d| d.child(div().t_small().text_color(hex(Chrome::MUTED)).text_center().child(error.message)))
            .child(div().pt_2().child(crate::ui::action_button(
                "browser-retry",
                t(cx, "browser.retry"),
                cx.listener(|this, _: &ClickEvent, _, cx| this.reload_browser(false, cx)),
            )))
            .into_any_element()
    }

    /// The bar under the page: what the page pulled over the network, and the way into the panel.
    fn render_network_bar(&self, browser: &BrowserPanel, cx: &mut Context<Self>) -> AnyElement {
        let net = &browser.tab().net;
        let open = browser.network_open;
        let summary = tf(
            cx,
            "browser.net_summary",
            &[
                ("transferred", &bytes(net.totals.transferred)),
                ("resources", &net.totals.count.to_string()),
                ("calls", &net.totals.calls.to_string()),
            ],
        );
        div()
            .id("browser-net-bar")
            .h(px(STATUS_BAR))
            .flex_shrink_0()
            .px_2()
            .flex()
            .items_center()
            .gap_1p5()
            .border_t_1()
            .border_color(hex(Chrome::BORDER))
            .bg(hex(Chrome::STATUS_BAR))
            .t_caption()
            .text_color(hex(Chrome::MUTED))
            .cursor_pointer()
            .hover(|s| s.text_color(hex(Chrome::BRIGHT)))
            .tooltip(Tooltip::text(t(cx, "browser.net_hint"), None))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                if let Some(browser) = this.browser.as_mut() {
                    browser.network_open = !browser.network_open;
                    cx.notify();
                }
            }))
            .child(icon(if open { "chevron-down" } else { "chevron-up" }, 11., hex(Chrome::MUTED)))
            .child(icon("arrow-down", 11., hex(Chrome::BLUE)))
            .child(div().flex_1().min_w_0().truncate().child(summary))
            .child(div().text_color(hex(Chrome::MUTED)).child(bytes(net.totals.decoded)))
            .into_any_element()
    }

    /// A simple developer tools panel: every API call the page made, and what came back.
    fn render_network_panel(&self, browser: &BrowserPanel, cx: &mut Context<Self>) -> AnyElement {
        let net = &browser.tab().net;
        let header = div()
            .h(px(26.))
            .flex_shrink_0()
            .px_2()
            .flex()
            .items_center()
            .gap_1()
            .border_b_1()
            .border_color(hex(Chrome::BORDER))
            .bg(hex(Chrome::SIDE_BAR))
            .t_caption()
            .font_weight(crate::theme::EMPHASIS)
            .text_color(hex(Chrome::MUTED))
            .child(t(cx, "browser.network").to_uppercase())
            .child(div().flex_1())
            .child(icon_only_sized(
                "browser-net-clear",
                "trash-2",
                20.,
                12.,
                cx.listener(|this, _: &ClickEvent, _, cx| this.clear_browser_network(cx)),
            ))
            .child(icon_only_sized(
                "browser-net-close",
                "x",
                20.,
                12.,
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    if let Some(browser) = this.browser.as_mut() {
                        browser.network_open = false;
                        cx.notify();
                    }
                }),
            ));
        let body = match net.selected {
            Some(_) => self.render_network_detail(browser, cx),
            None => self.render_network_list(browser, cx),
        };
        div()
            .h(px(settings(cx).browser.network_height))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(hex(Chrome::EDITOR))
            .child(self.render_network_splitter(cx))
            .child(header)
            .child(body)
            .into_any_element()
    }

    /// The top edge of the network panel: drag it to make the panel taller or shorter.
    fn render_network_splitter(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let resizing = self.browser_net_drag.is_some();
        let height = settings(cx).browser.network_height;
        div()
            .id("browser-net-resize")
            .group("browser-net-resize")
            .h(px(5.))
            .w_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .cursor(gpui::CursorStyle::ResizeUpDown)
            .child(
                div()
                    .h(px(1.))
                    .w_full()
                    .bg(hex(Chrome::BORDER))
                    .group_hover("browser-net-resize", |s| s.h(px(5.)).bg(hex(Chrome::ACCENT)))
                    .when(resizing, |d| d.h(px(5.)).bg(hex(Chrome::ACCENT))),
            )
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    this.browser_net_drag = Some((f32::from(event.position.y), height));
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
    }

    fn render_network_list(&self, browser: &BrowserPanel, cx: &mut Context<Self>) -> AnyElement {
        let net = &browser.tab().net;
        if net.calls.is_empty() {
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .px_4()
                .t_small()
                .text_color(hex(Chrome::MUTED))
                .text_center()
                .child(t(cx, "browser.net_empty"))
                .into_any_element();
        }
        let mut rows = div().flex().flex_col();
        for call in net.calls.iter().rev() {
            let id = call.id;
            let (status, color) = status_label(call, cx);
            rows = rows.child(
                div()
                    .id(SharedString::from(format!("browser-call-{id}")))
                    .px_2()
                    .py_1()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(hex(0x262626))
                    .t_caption()
                    .cursor_pointer()
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.open_browser_call(id, cx)))
                    .child(div().w(px(46.)).flex_shrink_0().text_color(hex(Chrome::MUTED)).child(call.method.clone()))
                    .child(div().w(px(30.)).flex_shrink_0().text_color(hex(color)).child(status))
                    .child(div().flex_1().min_w_0().truncate().text_color(hex(Chrome::FOREGROUND)).child(short_url(&call.url)))
                    .child(div().flex_shrink_0().text_color(hex(Chrome::MUTED)).child(bytes(call.size)))
                    .child(
                        div().w(px(44.)).flex_shrink_0().text_right().text_color(hex(Chrome::MUTED)).child(format!("{} ms", call.duration)),
                    ),
            );
        }
        div()
            .id("browser-net-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&browser.calls_scroll)
            .child(rows)
            .into_any_element()
    }

    fn render_network_detail(&self, browser: &BrowserPanel, cx: &mut Context<Self>) -> AnyElement {
        let net = &browser.tab().net;
        let back = div()
            .id("browser-call-back")
            .px_2()
            .py_1()
            .flex()
            .items_center()
            .gap_1()
            .t_caption()
            .text_color(hex(Chrome::MUTED))
            .cursor_pointer()
            .hover(|s| s.text_color(hex(Chrome::BRIGHT)))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.close_browser_call(cx)))
            .child(icon("arrow-left", 11., hex(Chrome::MUTED)))
            .child(t(cx, "browser.net_back"));
        let detail = match &net.detail {
            None => div().px_2().py_2().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "browser.net_loading")).into_any_element(),
            Some(Err(error)) => div().px_2().py_2().t_small().text_color(hex(Chrome::ERROR)).child(error.clone()).into_any_element(),
            Some(Ok(detail)) => {
                let mono = |text: String| {
                    div()
                        .p_1p5()
                        .rounded_sm()
                        .bg(hex(0x141414))
                        .t_caption()
                        .font_family("JetBrains Mono")
                        .text_color(hex(Chrome::FOREGROUND))
                        .child(text)
                };
                let section = |title: &str| {
                    div().pt_1().t_caption().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::MUTED)).child(title.to_uppercase())
                };
                let headers = |list: &[(String, String)]| list.iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>().join("\n");
                let status = match (detail.status, detail.error.is_empty()) {
                    (0, true) => t(cx, "browser.net_failed").to_string(),
                    (0, false) => detail.error.clone(),
                    _ => format!("{} {}", detail.status, detail.status_text).trim_end().to_string(),
                };
                let facts = [
                    (t(cx, "browser.net_status"), status),
                    (t(cx, "browser.net_method"), detail.method.clone()),
                    (t(cx, "browser.net_type"), if detail.mime.is_empty() { "—".into() } else { detail.mime.clone() }),
                    (t(cx, "browser.net_size"), bytes(detail.size)),
                    (t(cx, "browser.net_time"), format!("{} ms", detail.duration)),
                ];
                let mut facts_row = div().flex().flex_wrap().gap_x_4().gap_y_1();
                for (label, value) in facts {
                    facts_row = facts_row.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .t_caption()
                            .child(div().text_color(hex(Chrome::MUTED)).child(label))
                            .child(div().text_color(hex(Chrome::BRIGHT)).child(value)),
                    );
                }
                div()
                    .px_2()
                    .pb_2()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().t_small().text_color(hex(Chrome::BRIGHT)).child(detail.url.clone()))
                    .child(facts_row)
                    .when(!detail.req_headers.is_empty(), |d| {
                        d.child(section(t(cx, "browser.net_req_headers"))).child(mono(headers(&detail.req_headers)))
                    })
                    .when(!detail.req_body.is_empty(), |d| {
                        d.child(section(t(cx, "browser.net_req_body"))).child(mono(detail.req_body.clone()))
                    })
                    .when(!detail.res_headers.is_empty(), |d| {
                        d.child(section(t(cx, "browser.net_res_headers"))).child(mono(headers(&detail.res_headers)))
                    })
                    .child(section(t(cx, "browser.net_response")))
                    .child(mono(if detail.body.is_empty() { t(cx, "browser.net_no_body").to_string() } else { detail.body.clone() }))
                    .into_any_element()
            }
        };
        div()
            .id("browser-net-detail")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&browser.detail_scroll)
            .child(div().flex().flex_col().child(back).child(detail))
            .into_any_element()
    }

    /// `debug browser-net [id]`: opens or closes the panel, or opens one recorded call.
    pub(super) fn debug_browser_network(&mut self, argument: &str, cx: &mut Context<Self>) {
        match argument.parse::<u64>() {
            Ok(id) => self.open_browser_call(id, cx),
            Err(_) if argument == "clear" => self.clear_browser_network(cx),
            Err(_) if argument == "back" => self.close_browser_call(cx),
            Err(_) => {
                if let Some(browser) = self.browser.as_mut() {
                    browser.network_open = !browser.network_open;
                    cx.notify();
                }
            }
        }
    }

    fn open_browser_call(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(browser) = self.browser.as_mut() else { return };
        let tab = browser.tab_mut();
        tab.net.selected = Some(id);
        tab.net.detail = None;
        tab.net.polled_at = None;
        cx.notify();
    }

    fn close_browser_call(&mut self, cx: &mut Context<Self>) {
        let Some(browser) = self.browser.as_mut() else { return };
        let tab = browser.tab_mut();
        tab.net.selected = None;
        tab.net.detail = None;
        cx.notify();
    }

    /// Forgets the recorded calls, in the page as well as here.
    fn clear_browser_network(&mut self, cx: &mut Context<Self>) {
        let Some(browser) = self.browser.as_mut() else { return };
        let tab = browser.tab_mut();
        if let Some(view) = tab.webview.borrow().as_ref() {
            view.call_async(
                "const state = window.__agenttyNet; if (state) state.entries.length = 0;
                 try { performance.clearResourceTimings(); } catch (e) {}
                 return JSON.stringify(true);",
                &[],
                Box::new(|_| {}),
            );
        }
        tab.net.calls.clear();
        tab.net.selected = None;
        tab.net.detail = None;
        tab.net.polled_at = None;
        cx.notify();
    }
}

/// Bytes as a browser shows them ("312 kB", "1.4 MB").
fn bytes(value: u64) -> String {
    match value {
        0 => "0 B".into(),
        v if v < 1_000 => format!("{v} B"),
        v if v < 1_000_000 => format!("{:.0} kB", v as f64 / 1_000.),
        v if v < 1_000_000_000 => format!("{:.1} MB", v as f64 / 1_000_000.),
        v => format!("{:.2} GB", v as f64 / 1_000_000_000.),
    }
}

/// The last part of a URL, which is what tells calls apart in the list.
fn short_url(url: &str) -> String {
    let path = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let tail = path.rsplit('/').next().unwrap_or(path);
    if tail.is_empty() {
        path.to_string()
    } else {
        tail.to_string()
    }
}

/// The status code and the color it is shown in (green ok, orange redirect, red error).
fn status_label(call: &NetCall, cx: &gpui::App) -> (String, u32) {
    if !call.error.is_empty() || call.status == 0 {
        return (t(cx, "browser.net_error_short").to_string(), Chrome::ERROR);
    }
    let color = match call.status {
        200..=299 => Chrome::SUCCESS,
        300..=399 => Chrome::BLUE,
        _ => Chrome::ERROR,
    };
    (call.status.to_string(), color)
}

/// A thin bar under the toolbar that fills while a page loads, like a browser's. Its own row (not
/// drawn over the page): the native web view would cover it.
fn progress_bar(shown: bool, progress: f32) -> gpui::Div {
    // WebKit starts at 0.1; a sliver shows right away even before its first estimate.
    let fill = progress.clamp(0.08, 1.);
    div()
        .h(px(3.))
        .w_full()
        .flex_shrink_0()
        .bg(hex(Chrome::SIDE_BAR))
        .when(shown, |d| d.child(div().h_full().w(gpui::relative(fill)).rounded_r_sm().bg(hex(Chrome::BLUE))))
}

/// Whether a URL points at this machine: a dev server, a local file, a page served from here.
pub fn is_local_url(url: &str) -> bool {
    let rest = match url.split_once("://") {
        Some(("file", _)) => return true,
        Some((_, rest)) => rest,
        None => url,
    };
    // Strip credentials and take the host part of `host:port/path?query`.
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default().rsplit('@').next().unwrap_or_default();
    let host = match authority.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or_default(),
        None => authority
            .rsplit_once(':')
            .map(|(host, port)| if port.chars().all(|c| c.is_ascii_digit()) { host } else { authority })
            .unwrap_or(authority),
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    // An address is judged as an address, not by how it is spelled: "10.evil.com" and
    // "192.168.example.com" are ordinary public names that a prefix test would call local.
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return match ip {
            std::net::IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_unspecified(),
            std::net::IpAddr::V6(v6) => v6.is_loopback() || v6.is_unspecified(),
        };
    }
    // `.localhost`, `.local` and `.test` are reserved for exactly this (RFC 6761, RFC 6762).
    host.is_empty() || host == "localhost" || host.ends_with(".localhost") || host.ends_with(".local") || host.ends_with(".test")
}

#[cfg(test)]
mod link_tests {
    use super::is_local_url;

    #[test]
    fn only_this_machine_is_local() {
        for url in [
            "http://localhost:3000/app",
            "http://127.0.0.1:8080",
            "https://app.localhost/x",
            "file:///Users/me/notes.md",
            "http://192.168.1.14:5173",
            "http://mybox.local:9000",
        ] {
            assert!(is_local_url(url), "{url} should be local");
        }
        for url in [
            "https://github.com/empty-user77/Agentty/pull/28",
            "https://team.atlassian.net/browse/BR-1516",
            "https://console.cloud.google.com/",
            "https://vercel.com/dashboard",
        ] {
            assert!(!is_local_url(url), "{url} needs the signed-in browser");
        }
    }

    /// A name is not an address: a public host that merely starts or ends like a local one goes to
    /// the signed-in browser like any other.
    #[test]
    fn a_public_name_that_looks_local_is_not() {
        for url in [
            "http://10.evil.example/steal",
            "http://192.168.example.com/",
            "http://127.0.0.1.example.com/",
            "http://localhost.example.com/",
            "http://10.0.0.1.example.com/",
            "http://127.0.0.1@example.com/",
        ] {
            assert!(!is_local_url(url), "{url} is a public name");
        }
        // Real private addresses still are local, including the Docker range a prefix test missed.
        for url in ["http://172.17.0.2:8080/", "http://10.0.0.7:3000/", "http://169.254.1.1/"] {
            assert!(is_local_url(url), "{url} is on this network");
        }
    }

    #[test]
    fn credentials_and_ports_do_not_confuse_it() {
        assert!(is_local_url("http://user:pw@localhost:3000/"));
        assert!(!is_local_url("https://user@github.com/o/r/pull/1"));
        assert!(is_local_url("http://[::1]:7000/"));
    }
}
