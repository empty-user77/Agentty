//! Widths of the panels docked right of the terminals (plugin panel, Docker panel): each can be
//! dragged wider or narrower by the handle at its left edge, like the browser and the files panel.
//! The width is kept in the settings, never goes below a readable minimum, and always leaves the
//! terminals room.

use super::Workbench;
use crate::theme::{hex, Chrome};
use agentty_bridge::plugins::manifest::PanelMode;
use gpui::{div, prelude::*, px, AnyElement, Context, SharedString};

/// Narrowest a side panel gets.
pub const MIN_WIDTH: f32 = 240.;
/// What the terminals keep at least while a side panel is dragged wider.
const MIN_TERMINALS: f32 = 420.;
pub const DEFAULT_PLUGIN_WIDTH: f32 = 360.;
pub const DEFAULT_DOCKER_WIDTH: f32 = 340.;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SidePanel {
    Plugin,
    Docker,
}

impl Workbench {
    /// Width the plugin panel is drawn at, wherever it is drawn.
    pub(super) fn plugin_panel_shown_width(&self, cx: &gpui::App) -> f32 {
        crate::settings::settings(cx).plugin_panel_width.max(MIN_WIDTH)
    }

    /// Width the plugin panel takes from the window's layout: nothing unless it is docked, and
    /// never more than leaves the terminals their room — everything else on the row (the activity
    /// bar, the workspace list, the other panels) is counted first.
    pub(super) fn plugin_panel_width(&self, cx: &gpui::App) -> f32 {
        let Some(plugin) = self.plugin_panel.as_ref() else { return 0. };
        if !self.plugin_panel_mode(plugin, cx).is_docked() {
            return 0.;
        }
        self.plugin_panel_shown_width(cx).min(self.dockable_width(cx).max(MIN_WIDTH))
    }

    /// The most the plugin panel may take while docked.
    pub(super) fn dockable_width(&self, cx: &gpui::App) -> f32 {
        let prefs = crate::settings::settings(cx);
        let sidebar = if self.sidebar_open { prefs.sidebar_width } else { 0. };
        let others = self.docker_panel_width(cx) + if self.docker.open { 5. } else { 0. };
        self.viewport_width - super::chrome::ACTIVITY_BAR_WIDTH - sidebar - others - 5. - MIN_TERMINALS
    }

    /// Width of the Docker panel as shown (0 while it is closed).
    pub(super) fn docker_panel_width(&self, cx: &gpui::App) -> f32 {
        if self.docker.open {
            crate::settings::settings(cx).docker_panel_width.max(MIN_WIDTH)
        } else {
            0.
        }
    }

    /// Everything the side panels take, their handles included. A panel that floats, fills the
    /// area or has a window of its own takes nothing.
    pub(super) fn side_panels_total(&self, cx: &gpui::App) -> f32 {
        let handle = |shown: bool| if shown { 5. } else { 0. };
        let plugin = self.plugin_panel_width(cx);
        plugin + handle(plugin > 0.) + self.docker_panel_width(cx) + handle(self.docker.open)
    }

    /// How this plugin's panel opens: what the user chose, else what the plugin asks for.
    pub(super) fn plugin_panel_mode(&self, plugin: &str, cx: &gpui::App) -> PanelMode {
        if let Some(mode) = crate::settings::settings(cx).plugin_panel_modes.get(plugin).and_then(|id| PanelMode::from_id(id)) {
            return mode;
        }
        crate::plugins::plugin(cx, plugin).and_then(|p| p.manifest.as_ref()).map(|m| m.panel_mode()).unwrap_or_default()
    }

    pub(super) fn set_plugin_panel_mode(&mut self, plugin: &str, mode: PanelMode, _window: &mut gpui::Window, cx: &mut Context<Self>) {
        let id = mode.id().to_string();
        let chosen = plugin.to_string();
        crate::settings::update_settings(cx, |settings| {
            settings.plugin_panel_modes.insert(chosen, id);
        });
        // A panel that moved into a window of its own, or out of one, is opened or closed by
        // `reconcile_plugin_windows` on the next render — the one place that does it.
        cx.notify();
    }

    /// The drag handle at the left edge of `panel` (a hairline at rest, lit while hovered or dragged).
    pub(super) fn render_side_splitter(&self, panel: SidePanel, cx: &mut Context<Self>) -> AnyElement {
        let id = SharedString::from(format!("side-resize-{panel:?}"));
        let group = id.clone();
        let active = self.side_resizing == Some(panel);
        div()
            .id(id)
            .group(group.clone())
            .w(px(5.))
            .flex_shrink_0()
            .h_full()
            .flex()
            .justify_center()
            .cursor(gpui::CursorStyle::ResizeLeftRight)
            .child(
                div()
                    .w(px(1.))
                    .h_full()
                    .bg(hex(Chrome::BORDER))
                    .group_hover(group, |s| s.w(px(5.)).bg(hex(Chrome::ACCENT)))
                    .when(active, |d| d.w(px(5.)).bg(hex(Chrome::ACCENT))),
            )
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.side_resizing = Some(panel);
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    /// While a side panel's handle is dragged: its new width from the pointer's x position (its right
    /// edge is left of whatever is docked further right: the Docker panel, the files panel).
    pub(super) fn drag_side_panel(
        &mut self,
        panel: SidePanel,
        pointer_x: f32,
        viewport: f32,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        let files = self.files_panel.as_ref().map_or(0., |_| self.docked_widths(cx).1 + 5.);
        let right = match panel {
            SidePanel::Docker => files,
            SidePanel::Plugin => files + self.docker_panel_width(cx) + if self.docker.open { 5. } else { 0. },
        };
        let edge = viewport - right;
        // A plugin panel dragged wider than the window can dock stops taking room from it and
        // floats above it instead — the rest of the window keeps its shape.
        let floating = match (panel, self.plugin_panel.clone()) {
            (SidePanel::Plugin, Some(plugin)) => {
                let mode = self.plugin_panel_mode(&plugin, cx);
                let dockable = self.dockable_width(cx);
                let wanted = edge - pointer_x - 2.5;
                if mode == PanelMode::Push && wanted > dockable {
                    self.set_plugin_panel_mode(&plugin, PanelMode::Overlay, window, cx);
                }
                !self.plugin_panel_mode(&plugin, cx).is_docked()
            }
            _ => false,
        };
        // A floating panel may be as wide as the window bar its left edge; a docked one leaves the
        // terminals their room.
        let width = if floating { (edge - pointer_x - 2.5).clamp(MIN_WIDTH, edge) } else { side_width(edge, pointer_x) };
        gpui::BorrowAppContext::update_global::<crate::settings::SettingsStore, _>(cx, |store, _| match panel {
            SidePanel::Plugin => store.settings.plugin_panel_width = width,
            SidePanel::Docker => store.settings.docker_panel_width = width,
        });
        cx.notify();
    }
}

/// A side panel whose right edge is at `edge`, dragged to `pointer_x`: at least [`MIN_WIDTH`], and
/// never so wide that the terminals get less than [`MIN_TERMINALS`].
fn side_width(edge: f32, pointer_x: f32) -> f32 {
    (edge - pointer_x - 2.5).clamp(MIN_WIDTH, (edge - MIN_TERMINALS).max(MIN_WIDTH))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn side_panels_stay_readable_and_leave_the_terminals_room() {
        // Right edge at 1400: dragging to x=1000 gives about 400.
        assert_eq!(side_width(1400., 1000.), 397.5);
        // Too narrow: the minimum.
        assert_eq!(side_width(1400., 1390.), MIN_WIDTH);
        // Too wide: the terminals keep their room.
        assert_eq!(side_width(1400., 10.), 1400. - MIN_TERMINALS);
        // A tiny window: never below the minimum.
        assert_eq!(side_width(500., 10.), MIN_WIDTH);
    }
}
