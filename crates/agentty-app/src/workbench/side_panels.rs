//! Widths of the panels docked right of the terminals (plugin panel, Docker panel): each can be
//! dragged wider or narrower by the handle at its left edge, like the browser and the files panel.
//! The width is kept in the settings, never goes below a readable minimum, and always leaves the
//! terminals room.

use super::Workbench;
use crate::theme::{hex, Chrome};
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
    /// Width of the plugin panel as shown (0 while it is closed).
    pub(super) fn plugin_panel_width(&self, cx: &gpui::App) -> f32 {
        self.plugin_panel.as_ref().map_or(0., |_| crate::settings::settings(cx).plugin_panel_width.max(MIN_WIDTH))
    }

    /// Width of the Docker panel as shown (0 while it is closed).
    pub(super) fn docker_panel_width(&self, cx: &gpui::App) -> f32 {
        if self.docker.open {
            crate::settings::settings(cx).docker_panel_width.max(MIN_WIDTH)
        } else {
            0.
        }
    }

    /// Everything the side panels take, their handles included.
    pub(super) fn side_panels_total(&self, cx: &gpui::App) -> f32 {
        let handle = |shown: bool| if shown { 5. } else { 0. };
        self.plugin_panel_width(cx) + handle(self.plugin_panel.is_some()) + self.docker_panel_width(cx) + handle(self.docker.open)
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
    pub(super) fn drag_side_panel(&mut self, panel: SidePanel, pointer_x: f32, viewport: f32, cx: &mut Context<Self>) {
        let files = self.files_panel.as_ref().map_or(0., |_| self.docked_widths(cx).1 + 5.);
        let right = match panel {
            SidePanel::Docker => files,
            SidePanel::Plugin => files + self.docker_panel_width(cx) + if self.docker.open { 5. } else { 0. },
        };
        let edge = viewport - right;
        let width = side_width(edge, pointer_x);
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
