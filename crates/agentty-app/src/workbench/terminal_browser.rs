//! The in-app browser, one per terminal.
//!
//! Every terminal (pane) has a browser of its own — its tabs, its pages, where each one is. The
//! panel shows the browser of the terminal in front; selecting another terminal shows that one's
//! (or none, when it never opened one), and the first comes back as it was when its terminal is
//! selected again. Pages of the terminals out of sight keep running: every page is a background
//! view, parked outside the window rather than hidden, so nothing that works in one stops.
//!
//! - The user's own pages (the browser button, the address bar, a link) belong to the terminal in
//!   front when they are opened.
//! - An agent in a terminal (`agentty browser …`, the browser MCP tools) works in a tab of its own
//!   in that terminal's browser: the first `open` makes it, every later command of that terminal
//!   goes to it, and no command of that terminal reaches any other page — another terminal's, the
//!   user's, or a plugin's. Links clicked in a terminal and servers it starts open in that tab too.
//! - An agent's tab is laid out at the same size on screen and off (fitted to the panel by zoom),
//!   so what an agent checks does not depend on what the user looks at.
//! - Closing the panel hides the terminal's browser (it comes back when opened again); closing a
//!   terminal closes its browser.
//! - Cookies and sign-ins are shared by every page, unless "Separate cookies and sign-ins for each
//!   terminal" gives each agent tab a store of its own.

use super::browser::{BrowserPanel, BrowserTab, MAX_TABS};
use super::Workbench;
use crate::settings::settings;
use crate::webview::WebView;
use gpui::{Context, Window};
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::time::Instant;

/// Who opens a page in a terminal's tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Opener {
    /// Its agent (`agentty browser`, the browser tools): the page is locked while it works.
    Agent,
    /// The user, with a click (a link in the terminal, a port chip): shown now.
    User,
    /// A server the terminal started, opened by itself.
    Server,
}

impl Workbench {
    /// The terminal in front: in the tab in front of the workspace in front, with no page, start
    /// screen or plugin workspace over it.
    fn front_pane_id(&self, cx: &gpui::App) -> Option<u64> {
        if self.page.is_some() || self.welcome || self.plugin_workspace_shown.is_some() {
            return None;
        }
        self.active_pane().map(|pane| pane.read(cx).pane_id)
    }

    fn pane_on_screen(&self, pane: u64, cx: &gpui::App) -> bool {
        self.front_pane_id(cx) == Some(pane)
    }

    /// `pane`'s browser, wherever it is: the panel on screen, the one set aside while a plugin's
    /// workspace is in front, or kept for when its terminal is selected again.
    fn terminal_browser_mut(&mut self, pane: u64) -> Option<&mut BrowserPanel> {
        if self.browser_owner == Some(pane) {
            if self.plugin_workspace_shown.is_some() {
                if self.stashed_browser.is_some() {
                    return self.stashed_browser.as_mut();
                }
            } else if self.browser.is_some() {
                return self.browser.as_mut();
            }
        }
        self.pane_browsers.get_mut(&pane)
    }

    fn terminal_browser(&self, pane: u64) -> Option<&BrowserPanel> {
        if self.browser_owner == Some(pane) {
            if self.plugin_workspace_shown.is_some() {
                if self.stashed_browser.is_some() {
                    return self.stashed_browser.as_ref();
                }
            } else if self.browser.is_some() {
                return self.browser.as_ref();
            }
        }
        self.pane_browsers.get(&pane)
    }

    /// `pane`'s agent tab, wherever it is.
    pub(super) fn terminal_tab_mut(&mut self, pane: u64) -> Option<&mut BrowserTab> {
        self.terminal_browser_mut(pane)?.tabs.iter_mut().find(|tab| tab.pane == Some(pane))
    }

    /// `pane`'s agent tab with its page loaded again if it had been unloaded (to stay within the
    /// in-app browser's limits): what an agent's command runs in.
    pub(super) fn live_terminal_tab(&mut self, pane: u64, window: &Window, cx: &gpui::App) -> Option<&mut BrowserTab> {
        let prefs = settings(cx).browser.clone();
        let browser = self.terminal_browser_mut(pane)?;
        let tab = browser.tabs.iter_mut().find(|tab| tab.pane == Some(pane))?;
        if tab.webview.borrow().is_none() {
            tab.revive(window, &prefs);
            browser.unloaded = false;
            let browser = self.terminal_browser_mut(pane)?;
            return browser.tabs.iter_mut().find(|tab| tab.pane == Some(pane));
        }
        self.terminal_browser_mut(pane)?.tabs.iter_mut().find(|tab| tab.pane == Some(pane))
    }

    fn terminal_name(&self, pane: u64, cx: &gpui::App) -> String {
        // The terminal's folder (its title is whatever runs in it at the moment), and its number to
        // tell two terminals in the same folder apart.
        let folder = self
            .all_panes()
            .into_iter()
            .find(|p| p.read(cx).pane_id == pane)
            .and_then(|p| p.read(cx).current_dir().file_name().map(|name| name.to_string_lossy().to_string()))
            .unwrap_or_default();
        let title = agentty_bridge::fsutil::one_line(&folder, 18);
        if title.is_empty() {
            format!("#{pane}")
        } else {
            format!("{title} #{pane}")
        }
    }

    /// Opens `url` in `pane`'s agent tab — the one it has, else a new one in the terminal's
    /// browser. Without `url` a new tab opens the home page and an existing one stays where it is.
    /// Returns the tab's address.
    pub(super) fn open_terminal_tab(
        &mut self,
        pane: u64,
        url: Option<String>,
        opener: Opener,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<String, String> {
        if opener == Opener::User {
            // A click is the user asking to see it: a page over the window steps aside.
            self.page = None;
        }
        let driven = (opener == Opener::Agent).then(Instant::now);
        let existing = self.terminal_browser(pane).map(|b| (b.tabs.iter().position(|tab| tab.pane == Some(pane)), b.tabs.len()));
        let address = if let Some((found, count)) = existing {
            match found {
                Some(index) => {
                    let prefs = settings(cx).browser.clone();
                    let browser = self.terminal_browser_mut(pane).expect("the terminal's browser is there");
                    browser.unloaded = false;
                    let tab = &mut browser.tabs[index];
                    if driven.is_some() {
                        tab.driven_at = driven;
                    }
                    let unloaded = tab.webview.borrow().is_none();
                    let address = match url {
                        // An unloaded page comes back where it is sent.
                        Some(url) if unloaded => {
                            tab.pending = Some(url.clone());
                            tab.revive(window, &prefs);
                            url
                        }
                        Some(url) => {
                            tab.load_now(url.clone());
                            url
                        }
                        None => {
                            tab.revive(window, &prefs);
                            tab.address()
                        }
                    };
                    if opener != Opener::Server {
                        browser.active = index;
                    }
                    address
                }
                None => {
                    if count >= MAX_TABS {
                        return Err(format!("the in-app browser already has {MAX_TABS} tabs open; close some first"));
                    }
                    let (tab, url) = self.new_terminal_tab(pane, url, driven, window, cx)?;
                    let browser = self.terminal_browser_mut(pane).expect("the terminal's browser is still there");
                    browser.tabs.push(tab);
                    browser.active = browser.tabs.len() - 1;
                    url
                }
            }
        } else {
            // The terminal's first page: a browser of its own.
            let (tab, url) = self.new_terminal_tab(pane, url, driven, window, cx)?;
            let shown = self.browser.take();
            self.build_browser_panel(vec![tab], 0, url.clone(), window, cx);
            let made = self.browser.take();
            self.browser = shown;
            if let Some(made) = made {
                self.pane_browsers.insert(pane, made);
            }
            url
        };
        if opener == Opener::User && !self.pane_on_screen(pane, cx) {
            // The user asked for another terminal's page (a port chip on its workspace's card):
            // that terminal is selected, and its browser comes with it.
            self.select_terminal(pane, window, cx);
        }
        if self.pane_on_screen(pane, cx) || opener == Opener::User {
            self.show_terminal_browser(pane, cx);
        }
        if self.browser_owner == Some(pane) {
            self.show_address(cx);
        }
        cx.notify();
        Ok(address)
    }

    fn new_terminal_tab(
        &self,
        pane: u64,
        url: Option<String>,
        driven: Option<Instant>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(BrowserTab, String), String> {
        let url = url.unwrap_or_else(|| super::browser::browser_url(&settings(cx).browser.home, cx));
        let prefs = settings(cx).browser.clone();
        // Private mode already gives every view a store of its own that is never kept.
        let apart = prefs.separate_sessions && !prefs.private_mode && crate::webview::profiles_supported();
        let profile = apart.then(|| *uuid::Uuid::new_v4().as_bytes());
        let mut view = WebView::new_background_in(window, &prefs, profile).ok_or("the in-app browser is not available here")?;
        view.park_as(None);
        view.load(&url);
        let mut tab = BrowserTab::for_terminal(Rc::new(RefCell::new(Some(view))), url.clone(), pane, self.terminal_name(pane, cx), profile);
        tab.driven_at = driven;
        Ok((tab, url))
    }

    /// Brings terminal `pane` to the front: its workspace, its tab, and the keyboard.
    fn select_terminal(&mut self, pane: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pane) = self.all_panes().into_iter().find(|p| p.read(cx).pane_id == pane) else { return };
        let Some((workspace, _)) = self.locate(&pane) else { return };
        if workspace != self.active_workspace || self.welcome || self.page.is_some() {
            self.activate_workspace(workspace, window, cx);
        }
        self.mark_active(&pane, cx);
        self.focus_pane(&pane, window, cx);
    }

    /// Puts `pane`'s browser on screen when its terminal is the one in front (or the user asked
    /// for it): a browser kept for it, or one the user had closed, comes back.
    fn show_terminal_browser(&mut self, pane: u64, cx: &mut Context<Self>) {
        if self.plugin_workspace_shown.is_some() || self.page.is_some() {
            return;
        }
        if self.browser_owner != Some(pane) {
            // The user asked for another terminal's page (a port chip on its workspace's card):
            // that terminal's browser comes forward in place of the one on screen.
            self.put_browser_away();
            self.browser_owner = Some(pane);
        }
        if self.browser.is_none() {
            if let Some(mut browser) = self.pane_browsers.remove(&pane) {
                browser.closed = false;
                browser.unloaded = false;
                self.browser = Some(browser);
            }
        }
        cx.notify();
    }

    /// Keeps the browser on screen for its terminal (out of sight, still running). A plugin's pages
    /// among its tabs go back to the plugin, which keeps them running too.
    fn put_browser_away(&mut self) {
        let Some(mut browser) = self.browser.take() else { return };
        let Some(owner) = self.browser_owner else { return };
        browser.tabs.retain(|tab| tab.owner.is_none());
        if browser.tabs.is_empty() {
            return;
        }
        for tab in &browser.tabs {
            if let Some(view) = tab.webview.borrow_mut().as_mut() {
                view.hide();
            }
        }
        browser.active = browser.active.min(browser.tabs.len() - 1);
        browser.last_shown = Instant::now();
        self.pane_browsers.insert(owner, browser);
    }

    /// The user closed the panel: the terminal's browser is kept, hidden, until it is opened again.
    /// Returns false when there is no terminal to keep it for (the caller drops it then).
    pub(super) fn close_terminal_browser(&mut self, window: &mut Window) -> bool {
        if self.plugin_workspace_shown.is_some() || self.browser_owner.is_none() || self.browser.is_none() {
            return false;
        }
        self.put_browser_away();
        if let Some(owner) = self.browser_owner {
            if let Some(browser) = self.pane_browsers.get_mut(&owner) {
                browser.closed = true;
            }
        }
        crate::webview::focus_gpui_view(window);
        true
    }

    /// The browser the user closed for the terminal in front, back on screen.
    pub(super) fn reopen_terminal_browser(&mut self) -> bool {
        if self.browser.is_some() || self.plugin_workspace_shown.is_some() {
            return false;
        }
        let Some(owner) = self.browser_owner else { return false };
        match self.pane_browsers.remove(&owner) {
            Some(mut browser) => {
                browser.closed = false;
                browser.unloaded = false;
                self.browser = Some(browser);
                true
            }
            None => false,
        }
    }

    /// Closes `pane`'s agent tab. Returns whether it had one.
    pub(super) fn close_terminal_tab(&mut self, pane: u64, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(browser) = self.terminal_browser_mut(pane) else { return false };
        let Some(index) = browser.tabs.iter().position(|tab| tab.pane == Some(pane)) else { return false };
        let tab = browser.tabs.remove(index);
        let now_empty = browser.tabs.is_empty();
        if !now_empty && (index < browser.active || browser.active >= browser.tabs.len()) {
            browser.active = browser.active.saturating_sub(1);
        }
        drop_terminal_tab(tab);
        if now_empty {
            self.drop_terminal_browser(pane, window);
        } else if self.browser_owner == Some(pane) {
            self.show_address(cx);
        }
        cx.notify();
        true
    }

    /// `pane`'s browser goes away (its terminal closed, or its last page did).
    fn drop_terminal_browser(&mut self, pane: u64, window: &mut Window) {
        let browser = if self.browser_owner == Some(pane) && self.plugin_workspace_shown.is_none() && self.browser.is_some() {
            crate::webview::focus_gpui_view(window);
            self.browser.take()
        } else if self.browser_owner == Some(pane) && self.plugin_workspace_shown.is_some() && self.stashed_browser.is_some() {
            self.stashed_browser.take()
        } else {
            self.pane_browsers.remove(&pane)
        };
        if let Some(browser) = browser {
            for tab in browser.tabs {
                drop_terminal_tab(tab);
            }
        }
    }

    /// Opens `url` for terminal `pane` — in its own tab of the in-app browser when links open there
    /// and it is a local address, else in the default browser (as any link). Done before the next
    /// frame (a web view needs the window).
    pub(super) fn open_link_for_terminal(&mut self, pane: u64, url: String, opener: Opener, cx: &mut Context<Self>) {
        let in_app = settings(cx).link_opener == crate::settings::LinkOpener::InApp
            && super::browser::is_local_url(&url)
            && crate::platform::HAS_WEBVIEW;
        if !in_app {
            return self.open_link(url, cx);
        }
        self.terminal_links.push((pane, url, opener));
        cx.notify();
    }

    /// The terminal whose server listens on `port`, among `panes`.
    pub(super) fn terminal_of_port(&self, port: u16, panes: impl Iterator<Item = u64>) -> Option<u64> {
        panes.into_iter().find(|pane| self.servers.listeners.get(pane).is_some_and(|found| found.iter().any(|l| l.port == port)))
    }

    /// Where `pane`'s agent tab is or is going, if it has one.
    pub(super) fn terminal_tab_address(&self, pane: u64) -> Option<String> {
        let tab = self.terminal_browser(pane)?.tabs.iter().find(|tab| tab.pane == Some(pane))?;
        Some(tab.webview.borrow().as_ref().and_then(|view| view.current_url()).unwrap_or_else(|| tab.address()))
    }

    /// Kept in step with the window before each frame: pages asked for where no window was at
    /// hand open, the panel shows the browser of the terminal in front, the browsers of closed
    /// terminals close, and each tab keeps its own responsive size.
    pub(super) fn sync_terminal_tabs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for (pane, url, opener) in std::mem::take(&mut self.terminal_links) {
            if let Err(err) = self.open_terminal_tab(pane, Some(url), opener, window, cx) {
                eprintln!("agentty: could not open a page for terminal {pane}: {err}");
            }
        }
        if let Some(front) = self.front_pane_id(cx) {
            if self.browser_owner != Some(front) {
                if self.browser_owner.is_some() {
                    self.put_browser_away();
                    if let Some(mut browser) = self.pane_browsers.remove(&front) {
                        if browser.closed {
                            self.pane_browsers.insert(front, browser);
                        } else {
                            browser.unloaded = false;
                            self.browser = Some(browser);
                        }
                    }
                    if self.browser.is_none() {
                        crate::webview::focus_gpui_view(window);
                    }
                }
                // With no terminal before (the browser opened on the start screen) it becomes
                // this terminal's.
                self.browser_owner = Some(front);
                cx.notify();
            }
        }
        self.close_orphaned_terminal_browsers(window, cx);
        self.sync_tab_viewports(cx);
    }

    fn close_orphaned_terminal_browsers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut panes: Vec<u64> = self.pane_browsers.keys().copied().collect();
        panes.extend(self.browser_owner);
        if panes.is_empty() {
            return;
        }
        let live: HashSet<u64> = self.all_panes().iter().map(|p| p.read(cx).pane_id).collect();
        for pane in panes {
            if !live.contains(&pane) {
                self.drop_terminal_browser(pane, window);
                if self.browser_owner == Some(pane) {
                    self.browser_owner = None;
                }
                cx.notify();
            }
        }
    }

    /// The panel shows the responsive size of the tab in front; each tab keeps its own, and an
    /// agent's tab is laid out at it out of sight too.
    fn sync_tab_viewports(&mut self, cx: &mut Context<Self>) {
        let switched = {
            let Some(browser) = self.browser.as_mut() else { return };
            if browser.tabs.is_empty() {
                return;
            }
            let active = browser.active.min(browser.tabs.len() - 1);
            let shown = browser.tabs[active].webview.borrow().as_ref().map(|view| view.id());
            if shown != browser.viewport_tab {
                browser.viewport_tab = shown;
                let wanted = browser.tabs[active].viewport;
                Some(wanted).filter(|wanted| *wanted != browser.responsive.viewport)
            } else {
                None
            }
        };
        if let Some(wanted) = switched {
            self.set_viewport(wanted, cx);
            return;
        }
        let Some(browser) = self.browser.as_mut() else { return };
        let active = browser.active.min(browser.tabs.len() - 1);
        let current = browser.responsive.viewport;
        let tab = &mut browser.tabs[active];
        if tab.viewport != current {
            tab.viewport = current;
            if tab.pane.is_some() {
                if let Some(view) = tab.webview.borrow_mut().as_mut() {
                    view.park_as(current.map(|v| (v.width as f64, v.height as f64)));
                }
            }
        }
    }

    /// Sets the responsive size of `pane`'s agent tab (`None`: off). The panel follows when the tab
    /// is in front; out of sight the page is laid out at it and asks for phone pages at a phone's size.
    pub(super) fn set_terminal_viewport(&mut self, pane: u64, viewport: Option<super::responsive::Viewport>, cx: &mut Context<Self>) {
        let in_front = self.browser_owner == Some(pane)
            && self.plugin_workspace_shown.is_none()
            && self
                .browser
                .as_ref()
                .is_some_and(|b| b.tabs.get(b.active.min(b.tabs.len().saturating_sub(1))).is_some_and(|t| t.pane == Some(pane)));
        if in_front {
            self.set_viewport(viewport, cx);
        }
        let Some(tab) = self.terminal_tab_mut(pane) else { return };
        tab.viewport = viewport;
        if let Some(view) = tab.webview.borrow_mut().as_mut() {
            view.park_as(viewport.map(|v| (v.width as f64, v.height as f64)));
            if !in_front {
                view.set_mobile(viewport.is_some_and(|v| v.is_phone()) || settings(cx).browser.mobile);
            }
        }
        cx.notify();
    }
}

impl Workbench {
    /// `debug browser-tabs`: every browser, whose it is, and each tab's page.
    pub(super) fn debug_browser_tabs(&self) {
        let mut lines = Vec::new();
        let mut list = |place: String, browser: &BrowserPanel, active: bool| {
            for (index, tab) in browser.tabs.iter().enumerate() {
                let url = tab.webview.borrow().as_ref().and_then(|view| view.current_url());
                lines.push(format!(
                    "browser-tabs: {place}[{index}]{} agent={:?} plugin={:?} url={:?}",
                    if active && index == browser.active { "*" } else { "" },
                    tab.pane,
                    tab.owner,
                    url.unwrap_or_else(|| tab.address()),
                ));
            }
        };
        if let Some(browser) = &self.browser {
            list(format!("shown(terminal {:?})", self.browser_owner), browser, true);
        }
        if let Some(browser) = &self.stashed_browser {
            list(format!("set-aside(terminal {:?})", self.browser_owner), browser, false);
        }
        let mut kept: Vec<_> = self.pane_browsers.iter().collect();
        kept.sort_by_key(|(pane, _)| **pane);
        for (pane, browser) in kept {
            list(
                format!(
                    "kept(terminal {pane}{}{})",
                    if browser.closed { ", closed" } else { "" },
                    if browser.unloaded { ", unloaded" } else { "" }
                ),
                browser,
                false,
            );
        }
        lines.push(format!("browser-tabs: panel of terminal {:?}", self.browser_owner));
        eprintln!("{}", lines.join("\n"));
    }
}

/// A terminal's tab going away: its page, and its own cookies when it had some.
fn drop_terminal_tab(tab: BrowserTab) {
    let profile = tab.profile;
    drop(tab);
    if let Some(bytes) = profile {
        crate::webview::remove_profile(bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workbench::plugin_browser::PageLayout;

    fn no_view() -> Rc<RefCell<Option<WebView>>> {
        Rc::new(RefCell::new(None))
    }

    #[test]
    fn only_the_users_own_tabs_are_theirs_to_send_elsewhere() {
        let own = BrowserTab::new("http://localhost:3000".into());
        let terminal = BrowserTab::for_terminal(no_view(), "http://localhost:5173".into(), 7, "app #7".into(), None);
        let plugin = BrowserTab::for_plugin(no_view(), "https://example.com/home".into(), 3, "Example plugin".into());
        assert!(!own.claimed());
        assert!(terminal.claimed());
        assert!(plugin.claimed());
        assert_eq!(terminal.pane, Some(7));
        assert_eq!(terminal.address(), "http://localhost:5173");
    }

    #[test]
    fn a_terminals_page_keeps_its_width_in_any_panel() {
        // Laid out at 1280 off screen; on screen the panel's width only changes the zoom.
        let layout = PageLayout { width: Some(1280.), ..Default::default() };
        for panel in [400., 560., 1280., 1600.] {
            let zoom = layout.zoom(1.0, panel);
            assert!((panel as f64 / zoom - 1280.).abs() < 1e-6, "panel {panel}: zoom {zoom}");
        }
    }
}
