//! A workspace of a plugin's own (`contributes.panel.mode: "workspace"`).
//!
//! Some plugins do their work in agents and web pages the user should see: collecting posts in the
//! browser, having an agent write the drafts. Pressing such a plugin's icon switches to its
//! workspace — the agents it started as tabs, the browser with its pages beside them, its panel
//! (what it is doing) docked right of those. Each job it runs is a tab (`prompt/inject` with
//! `target: "own"`), so several run side by side. Leaving the workspace puts the window back as it
//! was; pressing the icon again goes back to where the user came from.

use super::Workbench;
use crate::launch::{LaunchSpec, PaneKind};
use agentty_bridge::plugins::manifest::PanelMode;
use gpui::{Context, Window};

impl Workbench {
    /// Whether `plugin` works in a workspace of its own (as the user or its manifest set it).
    pub(super) fn wants_workspace(&self, plugin: &str, cx: &gpui::App) -> bool {
        self.plugin_panel_mode(plugin, cx) == PanelMode::Workspace
    }

    /// Index of `plugin`'s workspace, if it has one.
    pub(super) fn plugin_workspace(&self, plugin: &str) -> Option<usize> {
        self.workspaces.iter().position(|ws| ws.plugin.as_deref() == Some(plugin))
    }

    /// The plugin that owns the workspace in front, if it still works that way.
    pub(super) fn front_plugin_workspace(&self, cx: &gpui::App) -> Option<String> {
        let plugin = self.workspaces.get(self.active_workspace)?.plugin.clone()?;
        let active = crate::plugins::plugin(cx, &plugin).is_some_and(|p| p.active());
        (active && self.wants_workspace(&plugin, cx)).then_some(plugin)
    }

    /// Whether `plugin`'s panel takes its place in the layout right now: docked, and — for a
    /// workspace plugin — only while its workspace is in front.
    pub(super) fn plugin_panel_docked_here(&self, plugin: &str, cx: &gpui::App) -> bool {
        match self.plugin_panel_mode(plugin, cx) {
            PanelMode::Push => true,
            PanelMode::Workspace => self.front_plugin_workspace(cx).as_deref() == Some(plugin),
            _ => false,
        }
    }

    /// Opens `plugin` the way it works: its workspace, or its panel.
    pub(super) fn show_plugin(&mut self, plugin: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.wants_workspace(plugin, cx) {
            if self.front_plugin_workspace(cx).as_deref() != Some(plugin) {
                self.open_plugin_workspace(plugin, window, cx);
            }
        } else {
            self.open_plugin_panel(plugin, cx);
        }
    }

    /// The plugin's icon: its workspace (and back), or its panel opened and closed.
    pub(super) fn toggle_plugin(&mut self, plugin: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.wants_workspace(plugin, cx) {
            self.open_plugin_workspace(plugin, window, cx);
        } else {
            self.toggle_plugin_panel(plugin, cx);
        }
    }

    /// The plugin's icon was pressed: go to its workspace (made the first time), or — already
    /// there — back to the workspace the user came from.
    pub(super) fn open_plugin_workspace(&mut self, plugin: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.front_plugin_workspace(cx).as_deref() == Some(plugin) && self.page.is_none() {
            let back = self.before_plugin_workspace.and_then(|id| self.workspaces.iter().position(|ws| ws.id == id));
            if let Some(index) = back {
                self.activate_workspace(index, window, cx);
            }
            return;
        }
        let current = self.workspaces.get(self.active_workspace).filter(|ws| ws.plugin.is_none()).map(|ws| ws.id);
        if current.is_some() {
            self.before_plugin_workspace = current;
        }
        match self.plugin_workspace(plugin) {
            Some(index) => self.activate_workspace(index, window, cx),
            None => self.create_plugin_workspace(plugin, window, cx),
        }
    }

    /// Makes `plugin`'s workspace — one terminal in the plugin's own data folder, named after it —
    /// and brings it to the front.
    pub(super) fn create_plugin_workspace(&mut self, plugin: &str, window: &mut Window, cx: &mut Context<Self>) {
        let cwd = agentty_bridge::plugins::store::plugin_data_dir(plugin);
        let _ = std::fs::create_dir_all(&cwd);
        let name = crate::plugins::plugin(cx, plugin).map(|p| p.name().to_string()).unwrap_or_else(|| plugin.to_string());
        let mut spec = LaunchSpec::new(PaneKind::Shell, cwd);
        spec.title = name.clone();
        self.create_workspace(spec, window, cx);
        if let Some(ws) = self.workspaces.last_mut() {
            ws.name = Some(name);
            ws.plugin = Some(plugin.to_string());
        }
        // Owned only now: show what that means.
        self.sync_plugin_workspace(cx);
        self.persist(cx);
    }

    /// After the workspace in front changed: a plugin's workspace brings up its panel, its pages
    /// and the browser; leaving it puts the window back the way it was.
    pub(super) fn sync_plugin_workspace(&mut self, cx: &mut Context<Self>) {
        let front = self.front_plugin_workspace(cx);
        if front == self.plugin_workspace_shown {
            return;
        }
        if let Some(previous) = self.plugin_workspace_shown.take() {
            if self.plugin_panel.as_deref() == Some(previous.as_str()) {
                self.close_plugin_panel(cx);
            }
            if std::mem::take(&mut self.plugin_workspace_browser) {
                // Its pages go back out of sight with it (and keep running there).
                self.browser = None;
            }
            if let Some(open) = self.plugin_workspace_sidebar.take() {
                self.sidebar_open = open;
            }
        }
        let Some(plugin) = front else { return };
        self.plugin_workspace_shown = Some(plugin.clone());
        // The workspace list folds away while the plugin's terminals, browser and panel share the
        // window; it comes back as it was when the user leaves.
        if self.plugin_workspace_sidebar.is_none() {
            self.plugin_workspace_sidebar = Some(self.sidebar_open);
        }
        self.sidebar_open = false;
        self.open_plugin_panel(&plugin, cx);
        let pages: Vec<u64> = self.plugin_browsers.iter().filter(|page| page.plugin == plugin).map(|page| page.id).collect();
        if self.browser.is_none() && crate::platform::HAS_WEBVIEW {
            self.plugin_workspace_browser = true;
            if pages.is_empty() {
                // Nothing of its own open yet: the site it works on, as the user is signed in to it.
                let home = crate::plugins::plugin(cx, &plugin)
                    .and_then(|p| p.manifest.as_ref())
                    .and_then(|m| m.browser.as_ref())
                    .and_then(|b| b.sites.first())
                    .map(|site| site.home_url());
                self.open_browser(home, cx);
            }
        }
        for id in pages {
            self.show_plugin_page(id, None, cx);
        }
    }
}

/// Which edge of a plugin's workspace is being dragged, from where, and what it measured then.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PluginWorkspaceDrag {
    /// The panel's right edge: (pointer x, panel width) when the drag started.
    Panel(f32, f32),
    /// The top edge of the terminals: (pointer y, their height) when the drag started.
    Terminals(f32, f32),
}

/// Narrowest the panel gets, and the least the browser and terminals keep.
const MIN_PANEL: f32 = 260.;
const MIN_TERMINALS: f32 = 120.;
const MIN_BROWSER: f32 = 240.;

impl Workbench {
    /// A plugin's workspace: its panel on the left, the browser (its pages) filling the right, and
    /// its terminals — one tab per job — under the browser. Every edge can be dragged.
    pub(super) fn render_plugin_workspace(&self, plugin: &str, main: Option<gpui::AnyElement>, cx: &mut Context<Self>) -> gpui::AnyElement {
        use crate::theme::{hex, Chrome};
        use gpui::{div, prelude::*, px};
        let prefs = crate::settings::settings(cx);
        let (panel_width, terminal_height) = (prefs.plugin_workspace_panel_width, prefs.plugin_workspace_terminal_height);
        let panel = self.render_plugin_panel_contents(plugin, cx);
        let browser = self.render_browser(cx);
        let has_browser = browser.is_some();
        let terminals = div()
            .flex()
            .flex_col()
            .min_h_0()
            .map(|d| if has_browser { d.h(px(terminal_height)).flex_shrink_0() } else { d.flex_1() })
            .child(self.render_tab_strip(cx))
            .child(div().flex_1().min_h_0().children(main));
        let handle = |id: &'static str, vertical: bool| {
            div()
                .id(id)
                .group(id)
                .flex_shrink_0()
                .flex()
                .justify_center()
                .items_center()
                .map(|d| {
                    if vertical {
                        d.w(px(5.)).h_full().cursor(gpui::CursorStyle::ResizeLeftRight)
                    } else {
                        d.h(px(5.)).w_full().cursor(gpui::CursorStyle::ResizeUpDown)
                    }
                })
                .child(
                    div()
                        .bg(hex(Chrome::BORDER))
                        .map(|d| if vertical { d.w(px(1.)).h_full() } else { d.h(px(1.)).w_full() })
                        .group_hover(id, |d| d.bg(hex(Chrome::ACCENT))),
                )
        };
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .children(panel.map(|panel| div().w(px(panel_width)).flex_shrink_0().h_full().child(panel)))
            .child(handle("plugin-ws-panel-edge", true).on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    this.plugin_ws_drag = Some(PluginWorkspaceDrag::Panel(f32::from(event.position.x), panel_width));
                    cx.stop_propagation();
                }),
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .when_some(browser, |d, browser| {
                        d.child(div().flex_1().min_h_0().flex().child(browser)).child(
                            handle("plugin-ws-terminal-edge", false).on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                                    this.plugin_ws_drag =
                                        Some(PluginWorkspaceDrag::Terminals(f32::from(event.position.y), terminal_height));
                                    cx.stop_propagation();
                                }),
                            ),
                        )
                    })
                    .child(terminals),
            )
            .into_any_element()
    }

    /// A drag of one of the workspace's edges moved to `(x, y)` in a window `viewport` large.
    pub(super) fn drag_plugin_workspace(
        &mut self,
        drag: PluginWorkspaceDrag,
        x: f32,
        y: f32,
        viewport: gpui::Size<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        let (width, height) = (f32::from(viewport.width), f32::from(viewport.height));
        gpui::BorrowAppContext::update_global::<crate::settings::SettingsStore, _>(cx, |store, _| match drag {
            PluginWorkspaceDrag::Panel(start_x, start_width) => {
                let most = (width - super::chrome::ACTIVITY_BAR_WIDTH - 420.).max(MIN_PANEL);
                store.settings.plugin_workspace_panel_width = (start_width + x - start_x).clamp(MIN_PANEL, most);
            }
            PluginWorkspaceDrag::Terminals(start_y, start_height) => {
                let most = (height - 120. - MIN_BROWSER).max(MIN_TERMINALS);
                store.settings.plugin_workspace_terminal_height = (start_height + start_y - y).clamp(MIN_TERMINALS, most);
            }
        });
        cx.notify();
    }
}
