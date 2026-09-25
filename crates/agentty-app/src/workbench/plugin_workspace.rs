//! A workspace of a plugin's own (`contributes.panel.mode: "workspace"`).
//!
//! Some plugins do their work in agents and web pages the user should see: collecting posts in the
//! browser, having an agent write the drafts. Pressing such a plugin's icon switches to its
//! workspace — the agents it started as tabs, the browser with its pages beside them, its panel
//! (what it is doing) docked right of those. Each job it runs is a tab (`prompt/inject` with
//! `target: "own"`), so several run side by side. Leaving the workspace puts the window back as it
//! was; pressing the icon again goes back to where the user came from.

use super::persist::TabInstance;
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
        let first = self.new_instance();
        if let Some(ws) = self.workspaces.last_mut() {
            ws.name = Some(name);
            ws.plugin = Some(plugin.to_string());
            // Its first tab is its first automation.
            if let Some(tab) = ws.tabs.first_mut() {
                tab.instance = Some(first);
            }
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
            // Its pages go back out of sight with it (and keep running there); the browser the user
            // had before comes back as it was.
            self.browser = None;
            self.browser = self.stashed_browser.take();
            self.instance_shown = None;
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
        // The browser in here shows the automation's pages only: the user's own tabs wait, out of
        // sight, for the user to come back.
        if let Some(browser) = self.browser.take() {
            for tab in &browser.tabs {
                if tab.owner.is_none() {
                    if let Some(view) = tab.webview.borrow_mut().as_mut() {
                        view.hide();
                    }
                }
            }
            if self.stashed_browser.is_none() {
                self.stashed_browser = Some(browser);
            }
        }
        self.instance_shown = None;
    }

    /// A fresh automation id (unique in this window, kept with the tab across restarts).
    /// A new automation, with an id no other tab has — open, asleep or recently closed. The id counter
    /// starts over at every launch while saved tabs keep theirs, so a number alone may be taken;
    /// two tabs sharing an id would share their panel, pages and settings.
    pub(super) fn new_instance(&mut self) -> TabInstance {
        let taken: std::collections::HashSet<String> = self
            .workspaces
            .iter()
            .flat_map(|ws| {
                let open = ws.tabs.iter().filter_map(|t| t.instance.as_ref());
                let asleep = ws.dormant.iter().flat_map(|s| s.tabs.iter().chain(&s.closed_tabs)).filter_map(|t| t.instance.as_ref());
                let closed = ws.closed_tabs.iter().filter_map(|t| t.instance.as_ref());
                open.chain(asleep).chain(closed).map(|i| i.id.clone())
            })
            .collect();
        TabInstance { id: unused_instance_id(&taken, || self.next_id()), title: None }
    }

    /// The automation in front, when `plugin`'s workspace is.
    pub(super) fn active_instance(&self, plugin: &str, cx: &gpui::App) -> Option<String> {
        if self.front_plugin_workspace(cx).as_deref() != Some(plugin) {
            return None;
        }
        let ws = self.workspaces.get(self.active_workspace)?;
        ws.tabs.get(ws.active_tab)?.instance.as_ref().map(|i| i.id.clone())
    }

    /// What `plugin`'s panel shows now: the automation in front's own panel, else its only one.
    pub(super) fn plugin_tree<'a>(&self, plugin: &str, cx: &'a gpui::App) -> Option<&'a agentty_bridge::plugins::ui::Node> {
        let instance = self.active_instance(plugin, cx);
        let runtime = crate::plugins::runtime(cx, plugin)?;
        instance.and_then(|i| runtime.panels.get(&i)).or(runtime.panel.as_ref())
    }

    /// Every automation of `plugin`'s workspace, awake or not: `(id, title, in front)`.
    fn instances_of(&self, plugin: &str) -> Vec<(String, Option<String>, bool)> {
        let Some(index) = self.plugin_workspace(plugin) else { return Vec::new() };
        let ws = &self.workspaces[index];
        if let Some(dormant) = &ws.dormant {
            return dormant
                .tabs
                .iter()
                .enumerate()
                .filter_map(|(i, t)| t.instance.as_ref().map(|inst| (inst.id.clone(), inst.title.clone(), i == dormant.active_tab)))
                .collect();
        }
        let front = index == self.active_workspace;
        ws.tabs
            .iter()
            .enumerate()
            .filter_map(|(i, t)| t.instance.as_ref().map(|inst| (inst.id.clone(), inst.title.clone(), front && i == ws.active_tab)))
            .collect()
    }

    /// `workspace/instances`: `[{ instance, title, active }]`.
    pub(super) fn plugin_instances(&self, plugin: &str, _cx: &gpui::App) -> Vec<serde_json::Value> {
        self.instances_of(plugin)
            .into_iter()
            .map(|(id, title, active)| serde_json::json!({ "instance": id, "title": title, "active": active }))
            .collect()
    }

    /// `workspace/setInstanceTitle`: what the automation's tab is called.
    pub(super) fn set_instance_title(
        &mut self,
        plugin: &str,
        instance: &str,
        title: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let index = self.plugin_workspace(plugin).ok_or("the plugin has no workspace")?;
        let title = title.map(|t| t.trim().chars().take(60).collect::<String>()).filter(|t| !t.is_empty());
        let ws = &mut self.workspaces[index];
        let live = ws.tabs.iter_mut().filter_map(|t| t.instance.as_mut());
        let sleeping = ws.dormant.iter_mut().flat_map(|d| d.tabs.iter_mut()).filter_map(|t| t.instance.as_mut());
        let found = live.chain(sleeping).find(|i| i.id == instance).ok_or("no such automation")?;
        found.title = title;
        self.persist(cx);
        cx.notify();
        Ok(())
    }

    /// Closes the tab of one of `plugin`'s own automations (the plugin deleted it). Only a tab of
    /// that plugin's workspace, and only one that is open, can be closed this way.
    pub(super) fn close_instance(
        &mut self,
        plugin: &str,
        instance: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let index = self.plugin_workspace(plugin).ok_or("the plugin has no workspace")?;
        let ws = &self.workspaces[index];
        let tab = ws.tabs.iter().find(|t| t.instance.as_ref().is_some_and(|i| i.id == instance)).ok_or("no such open automation")?;
        let panes = tab.root.leaves();
        self.perform_close(super::confirm::CloseTarget::Tabs(panes), window, cx);
        // Deleted, not closed: it is not offered among the recently closed tabs.
        for ws in &mut self.workspaces {
            ws.closed_tabs.retain(|t| t.instance.as_ref().is_none_or(|i| i.id != instance));
        }
        self.persist(cx);
        Ok(())
    }

    /// A new automation: a tab of the plugin's workspace (in front), which the plugin hears about.
    pub(super) fn new_plugin_instance(&mut self, plugin: &str, cx: &mut Context<Self>) {
        let Some(index) = self.plugin_workspace(plugin) else { return };
        let cwd = self.workspaces[index].cwd.clone();
        let pane = self.spawn_pane(LaunchSpec::new(PaneKind::Shell, cwd), cx);
        let instance = self.new_instance();
        let ws = &mut self.workspaces[index];
        ws.tabs.push(super::Tab { root: super::panes::PaneNode::Leaf(pane.clone()), active: pane, instance: Some(instance) });
        ws.active_tab = ws.tabs.len() - 1;
        self.page = None;
        self.persist(cx);
        cx.notify();
    }

    /// Opens a job's terminal beside the terminals of `instance` (waking the workspace if it
    /// sleeps), without bringing that automation to the front.
    pub(super) fn open_in_instance(
        &mut self,
        plugin: &str,
        instance: &str,
        spec: LaunchSpec,
        cx: &mut Context<Self>,
    ) -> Result<super::Pane, String> {
        let index = self.plugin_workspace(plugin).ok_or("the plugin has no workspace")?;
        if let Some(snapshot) = self.workspaces[index].dormant.take() {
            self.revive(index, snapshot, cx);
        }
        let tab = self.workspaces[index]
            .tabs
            .iter()
            .position(|t| t.instance.as_ref().is_some_and(|i| i.id == instance))
            .ok_or("no such automation")?;
        let pane = self.spawn_pane(spec, cx);
        let tab = &mut self.workspaces[index].tabs[tab];
        let beside = tab.active.clone();
        tab.root.split(&beside, pane.clone(), super::Axis::Horizontal);
        tab.active = pane.clone();
        self.persist(cx);
        Ok(pane)
    }

    /// Called every render. The plugin hears which automations exist (again after it restarts) and
    /// which went; an automation that went takes its pages with it; the automation in front shows
    /// its pages in the browser, the others keep theirs out of sight (and running).
    pub(super) fn sync_plugin_instances(&mut self, cx: &mut Context<Self>) {
        if let Some(plugin) = self.front_plugin_workspace(cx).filter(|_| self.page.is_none()) {
            // Its workspace in front means it works: a plugin stopped meanwhile (restarted, turned
            // off and on) is started again — unless it failed, which the panel says.
            let failed = crate::plugins::runtime(cx, &plugin).is_some_and(|r| matches!(r.state, crate::plugins::RunState::Failed(_)));
            if crate::plugins::generation(&plugin, cx).is_none() && !failed {
                let context = self.plugin_context(&plugin, None, cx);
                crate::plugins::notify_plugin(&plugin, "panel/open", serde_json::json!({ "context": context }), cx);
            }
            // The user's own tabs (brought back after a restart, opened while here) wait out of sight.
            if self.browser.as_ref().is_some_and(|b| b.tabs.iter().any(|tab| tab.owner.is_none())) {
                if let Some(mut browser) = self.browser.take() {
                    let (own, plugin_tabs): (Vec<_>, Vec<_>) = browser.tabs.drain(..).partition(|tab| tab.owner.is_none());
                    for tab in &own {
                        if let Some(view) = tab.webview.borrow_mut().as_mut() {
                            view.hide();
                        }
                    }
                    if self.stashed_browser.is_none() && !own.is_empty() {
                        browser.tabs = own;
                        browser.active = 0;
                        self.stashed_browser = Some(browser);
                    }
                    // Its pages come back as tabs of a browser of their own.
                    for tab in plugin_tabs {
                        if let Some(id) = tab.owner {
                            self.plugin_pages_to_show.push(id);
                        }
                    }
                    self.instance_shown = None;
                }
            }
        }
        let plugins: Vec<String> = self.workspaces.iter().filter_map(|ws| ws.plugin.clone()).collect();
        for plugin in plugins {
            let Some(index) = self.plugin_workspace(&plugin) else { continue };
            // A tab that came back without being an automation (reopened, dragged in) becomes one.
            let missing: Vec<usize> =
                self.workspaces[index].tabs.iter().enumerate().filter(|(_, t)| t.instance.is_none()).map(|(i, _)| i).collect();
            for tab in missing {
                let instance = self.new_instance();
                self.workspaces[index].tabs[tab].instance = Some(instance);
            }
            let generation = crate::plugins::generation(&plugin, cx);
            let now: Vec<(String, Option<String>)> = self.instances_of(&plugin).into_iter().map(|(id, title, _)| (id, title)).collect();
            let (known_generation, known) = self.known_instances.get(&plugin).cloned().unwrap_or_default();
            let restarted = generation.is_some() && generation != known_generation;
            for (id, title) in &now {
                if restarted || !known.contains(id) {
                    crate::plugins::send_if_running(&plugin, "instance/open", serde_json::json!({ "instance": id, "title": title }), cx);
                }
            }
            for id in known.iter().filter(|id| !now.iter().any(|(n, _)| n == *id)) {
                crate::plugins::send_if_running(&plugin, "instance/close", serde_json::json!({ "instance": id }), cx);
                let pages: Vec<u64> = self
                    .plugin_browsers
                    .iter()
                    .filter(|p| p.plugin == plugin && p.instance.as_deref() == Some(id.as_str()))
                    .map(|p| p.id)
                    .collect();
                for page in pages {
                    self.close_plugin_page(page, cx);
                }
            }
            if generation.is_some() {
                self.known_instances.insert(plugin.clone(), (generation, now.into_iter().map(|(id, _)| id).collect()));
            }
        }
        let front = self.front_plugin_workspace(cx).and_then(|plugin| self.active_instance(&plugin, cx).map(|i| (plugin, i)));
        if front == self.instance_shown {
            return;
        }
        let Some((plugin, instance)) = front.clone() else {
            self.instance_shown = None;
            return;
        };
        let pages: Vec<(u64, bool, bool)> = self
            .plugin_browsers
            .iter()
            .filter(|p| p.plugin == plugin)
            .map(|p| (p.id, p.shown, p.instance.as_deref().is_none_or(|i| i == instance)))
            .collect();
        for (id, shown, mine) in pages {
            if mine && !shown {
                self.show_plugin_page(id, None, cx);
            } else if !mine && shown {
                self.hide_plugin_page(id, cx);
            }
        }
        self.instance_shown = front;
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
        // One tab is one automation: switching it switches the panel, the browser and the
        // terminals below together.
        let body = div()
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
            );
        div().flex_1().min_h_0().flex().flex_col().child(self.render_tab_strip(cx)).child(body).into_any_element()
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

/// The first `a<n>` from `next` that is not in `taken`.
fn unused_instance_id(taken: &std::collections::HashSet<String>, mut next: impl FnMut() -> u64) -> String {
    loop {
        let id = format!("a{}", next());
        if !taken.contains(&id) {
            return id;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::unused_instance_id;
    use std::collections::HashSet;

    #[test]
    fn a_new_automation_never_takes_the_id_of_a_saved_one() {
        // After a restart the counter starts low again while tabs a3..a5 came back from the layout.
        let taken: HashSet<String> = ["a3", "a4", "a5"].map(String::from).into_iter().collect();
        let mut counter = 3;
        let id = unused_instance_id(&taken, || {
            counter += 1;
            counter
        });
        assert_eq!(id, "a6");
        assert_eq!(unused_instance_id(&HashSet::new(), || 7), "a7");
    }
}
