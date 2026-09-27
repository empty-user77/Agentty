//! The in-app browser, one tab per terminal.
//!
//! An agent in a terminal (`agentty browser …`, the browser MCP tools) works in a tab of its own:
//! the first `open` makes it, every later command of that terminal goes to it, and no command of
//! that terminal reaches any other tab — another terminal's, the user's own, or a plugin's. Agents
//! in several terminals test side by side.
//!
//! - A terminal's tab never stops for being out of sight: its view is a background one, parked
//!   outside the window when another tab is in front, laid out at the same size on screen and off
//!   (fitted to the panel by zoom), so what an agent checks does not depend on what the user looks at.
//! - Selecting a terminal brings its tab to the front. An agent in a terminal that is not on screen
//!   never changes what the user looks at: its tab is added behind the others, or — with no panel
//!   open — kept backstage until its terminal is selected.
//! - Closing the terminal closes its tab.
//! - Cookies and sign-ins are shared with the rest of the browser, unless "Separate sessions per
//!   terminal" gives each tab a store of its own.

use super::browser::{BrowserTab, MAX_TABS};
use super::Workbench;
use crate::settings::settings;
use crate::webview::WebView;
use gpui::{Context, Window};
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::time::Instant;

/// Where a terminal's tab is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Place {
    /// The browser panel of this window.
    Front,
    /// The user's browser, set aside while a plugin's workspace is in front.
    Stashed,
    /// No panel for it yet: waiting, running, for its terminal to be selected.
    Backstage,
}

impl Workbench {
    fn terminal_tab_place(&self, pane: u64) -> Option<(Place, usize)> {
        let find = |tabs: &[BrowserTab]| tabs.iter().position(|tab| tab.pane == Some(pane));
        if let Some(index) = self.browser.as_ref().and_then(|b| find(&b.tabs)) {
            return Some((Place::Front, index));
        }
        if let Some(index) = self.stashed_browser.as_ref().and_then(|b| find(&b.tabs)) {
            return Some((Place::Stashed, index));
        }
        find(&self.backstage_tabs).map(|index| (Place::Backstage, index))
    }

    fn tabs_at(&mut self, place: Place) -> Option<&mut Vec<BrowserTab>> {
        match place {
            Place::Front => self.browser.as_mut().map(|b| &mut b.tabs),
            Place::Stashed => self.stashed_browser.as_mut().map(|b| &mut b.tabs),
            Place::Backstage => Some(&mut self.backstage_tabs),
        }
    }

    /// `pane`'s tab, wherever it is.
    pub(super) fn terminal_tab_mut(&mut self, pane: u64) -> Option<&mut BrowserTab> {
        let (place, index) = self.terminal_tab_place(pane)?;
        self.tabs_at(place)?.get_mut(index)
    }

    /// Whether `pane` is the terminal the user looks at: in the tab in front of the workspace in
    /// front, with no page, start screen or plugin workspace over it.
    fn pane_on_screen(&self, pane: u64, cx: &gpui::App) -> bool {
        self.front_pane_id(cx) == Some(pane)
    }

    fn front_pane_id(&self, cx: &gpui::App) -> Option<u64> {
        if self.page.is_some() || self.welcome || self.plugin_workspace_shown.is_some() {
            return None;
        }
        self.active_pane().map(|pane| pane.read(cx).pane_id)
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

    /// Opens `url` in `pane`'s tab — the one it has, else a new one. Without `url` a new tab
    /// opens the home page and an existing one stays where it is. Returns the tab's address.
    pub(super) fn open_terminal_tab(
        &mut self,
        pane: u64,
        url: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<String, String> {
        let on_screen = self.pane_on_screen(pane, cx);
        if let Some(tab) = self.terminal_tab_mut(pane) {
            tab.driven_at = Some(Instant::now());
            let address = match url {
                Some(url) => {
                    tab.load_now(url.clone());
                    url
                }
                None => tab.address(),
            };
            if on_screen {
                self.bring_terminal_tab(pane, window, cx);
            }
            cx.notify();
            return Ok(address);
        }
        let url = url.unwrap_or_else(|| super::browser::browser_url(&settings(cx).browser.home, cx));
        let place = if self.plugin_workspace_shown.is_some() {
            if self.stashed_browser.is_some() {
                Place::Stashed
            } else {
                Place::Backstage
            }
        } else if self.browser.is_some() || on_screen {
            Place::Front
        } else {
            Place::Backstage
        };
        let open = self.tabs_at(place).map_or(0, |tabs| tabs.len());
        if open >= MAX_TABS {
            return Err(format!("the in-app browser already has {MAX_TABS} tabs open; close some first"));
        }
        let prefs = settings(cx).browser.clone();
        // Private mode already gives every view a store of its own that is never kept.
        let apart = prefs.separate_sessions && !prefs.private_mode && crate::webview::profiles_supported();
        let profile = apart.then(|| *uuid::Uuid::new_v4().as_bytes());
        let mut view = WebView::new_background_in(window, &prefs, profile).ok_or("the in-app browser is not available here")?;
        view.park_as(None);
        view.load(&url);
        let mut tab = BrowserTab::for_terminal(Rc::new(RefCell::new(Some(view))), url.clone(), pane, self.terminal_name(pane, cx), profile);
        tab.driven_at = Some(Instant::now());
        match place {
            Place::Front => match self.browser.as_mut() {
                Some(browser) => {
                    browser.tabs.push(tab);
                    if on_screen {
                        browser.active = browser.tabs.len() - 1;
                        self.show_address(cx);
                    }
                }
                None => {
                    self.page = None;
                    self.build_browser_panel(vec![tab], 0, url.clone(), window, cx);
                }
            },
            Place::Stashed => {
                if let Some(browser) = self.stashed_browser.as_mut() {
                    browser.tabs.push(tab);
                }
            }
            Place::Backstage => self.backstage_tabs.push(tab),
        }
        cx.notify();
        Ok(url)
    }

    /// Puts `pane`'s tab in front of the panel (opening the panel for one kept backstage).
    fn bring_terminal_tab(&mut self, pane: u64, window: &mut Window, cx: &mut Context<Self>) {
        if self.plugin_workspace_shown.is_some() || self.page.is_some() {
            return;
        }
        match self.terminal_tab_place(pane) {
            Some((Place::Front, index)) => self.select_browser_tab(index, cx),
            Some((Place::Backstage, index)) => {
                let tab = self.backstage_tabs.remove(index);
                match self.browser.as_mut() {
                    Some(browser) => {
                        browser.tabs.push(tab);
                        browser.active = browser.tabs.len() - 1;
                        self.show_address(cx);
                    }
                    None => {
                        let url = tab.address();
                        self.build_browser_panel(vec![tab], 0, url, window, cx);
                    }
                }
                cx.notify();
            }
            _ => {}
        }
    }

    /// Closes `pane`'s tab. Returns whether it had one.
    pub(super) fn close_terminal_tab(&mut self, pane: u64, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some((place, index)) = self.terminal_tab_place(pane) else { return false };
        let Some(tabs) = self.tabs_at(place) else { return false };
        let tab = tabs.remove(index);
        let now_empty = tabs.is_empty();
        drop_terminal_tab(tab);
        match place {
            Place::Front => {
                if now_empty {
                    self.browser = None;
                    crate::webview::focus_gpui_view(window);
                } else if let Some(browser) = self.browser.as_mut() {
                    if index < browser.active || browser.active >= browser.tabs.len() {
                        browser.active = browser.active.saturating_sub(1);
                    }
                    self.show_address(cx);
                }
            }
            Place::Stashed if now_empty => self.stashed_browser = None,
            _ => {}
        }
        cx.notify();
        true
    }

    /// Keeps the terminals' tabs among `tabs` (a panel going away): out of sight, still running,
    /// until a panel opens again.
    pub(super) fn keep_terminal_tabs(&mut self, tabs: Vec<BrowserTab>) {
        for tab in tabs.into_iter().filter(|tab| tab.pane.is_some()) {
            if let Some(view) = tab.webview.borrow_mut().as_mut() {
                view.hide();
            }
            self.backstage_tabs.push(tab);
        }
    }

    /// Kept in step with the window before each frame: tabs waiting backstage join a panel that
    /// opened, the selected terminal's tab comes to the front, the tabs of closed terminals close,
    /// and each tab keeps its own responsive size.
    pub(super) fn sync_terminal_tabs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.backstage_tabs.is_empty() {
            let waiting = std::mem::take(&mut self.backstage_tabs);
            let front = self.front_pane_id(cx);
            let in_plugin_workspace = self.plugin_workspace_shown.is_some();
            let panel = if in_plugin_workspace { self.stashed_browser.as_mut() } else { self.browser.as_mut() };
            match panel {
                Some(browser) => {
                    // The panel just opened again: the selected terminal's tab is the one in front.
                    let selected = front.and_then(|pane| waiting.iter().position(|tab| tab.pane == Some(pane)));
                    let base = browser.tabs.len();
                    browser.tabs.extend(waiting);
                    if let Some(index) = selected.filter(|_| !in_plugin_workspace) {
                        browser.active = base + index;
                        self.show_address(cx);
                    }
                }
                None => self.backstage_tabs = waiting,
            }
        }
        // Selecting a terminal shows its tab (only when the terminal in front changes: a tab the
        // user picked stays while they type).
        let front = self.front_pane_id(cx);
        if front != self.browser_front_pane {
            self.browser_front_pane = front;
            if let Some(pane) = front.filter(|pane| self.terminal_tab_place(*pane).is_some()) {
                self.bring_terminal_tab(pane, window, cx);
            }
        }
        self.close_orphaned_terminal_tabs(window, cx);
        self.sync_tab_viewports(cx);
    }

    fn close_orphaned_terminal_tabs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let claimed = |tabs: &[BrowserTab]| tabs.iter().filter_map(|tab| tab.pane).collect::<Vec<_>>();
        let mut panes = claimed(&self.backstage_tabs);
        panes.extend(self.browser.as_ref().map(|b| claimed(&b.tabs)).unwrap_or_default());
        panes.extend(self.stashed_browser.as_ref().map(|b| claimed(&b.tabs)).unwrap_or_default());
        if panes.is_empty() {
            return;
        }
        let live: HashSet<u64> = self.all_panes().iter().map(|p| p.read(cx).pane_id).collect();
        for pane in panes {
            if !live.contains(&pane) {
                self.close_terminal_tab(pane, window, cx);
            }
        }
    }

    /// The panel shows the responsive size of the tab in front; each tab keeps its own, and a
    /// terminal's tab is laid out at it out of sight too.
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

    /// Sets the responsive size of `pane`'s tab (`None`: off). The panel follows when the tab is in
    /// front; out of sight the page is laid out at it and asks for phone pages at a phone's size.
    pub(super) fn set_terminal_viewport(&mut self, pane: u64, viewport: Option<super::responsive::Viewport>, cx: &mut Context<Self>) {
        let in_front = match (self.terminal_tab_place(pane), self.browser.as_ref()) {
            (Some((Place::Front, index)), Some(browser)) => browser.active.min(browser.tabs.len().saturating_sub(1)) == index,
            _ => false,
        };
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
    /// `debug browser-tabs`: every tab, where it is, whose it is and what its page shows.
    pub(super) fn debug_browser_tabs(&self) {
        let mut lines = Vec::new();
        let mut list = |place: &str, tabs: &[BrowserTab], active: Option<usize>| {
            for (index, tab) in tabs.iter().enumerate() {
                let url = tab.webview.borrow().as_ref().and_then(|view| view.current_url());
                lines.push(format!(
                    "browser-tabs: {place}[{index}]{} pane={:?} plugin={:?} url={:?}",
                    if Some(index) == active { "*" } else { "" },
                    tab.pane,
                    tab.owner,
                    url.unwrap_or_else(|| tab.address()),
                ));
            }
        };
        if let Some(browser) = &self.browser {
            list("front", &browser.tabs, Some(browser.active));
        }
        if let Some(browser) = &self.stashed_browser {
            list("stashed", &browser.tabs, None);
        }
        list("backstage", &self.backstage_tabs, None);
        lines.push(format!("browser-tabs: front pane {:?}", self.browser_front_pane));
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
