//! A plugin panel in a window of its own: the same panel, drawn by the workbench that opened it,
//! in a window the user can move and resize like any other.

use super::Workbench;
use crate::theme::{hex, Chrome};
use agentty_bridge::plugins::manifest::PanelMode;
use gpui::{div, prelude::*, px, size, Bounds, Context, Window, WindowBounds, WindowOptions};

/// Where a panel's own window opens, before the user moves it.
const WIDTH: f32 = 520.;
const HEIGHT: f32 = 640.;

pub struct PluginWindow {
    /// Weak on purpose: the window the panel came from must still be able to close.
    workbench: gpui::WeakEntity<Workbench>,
    plugin: String,
}

impl gpui::Render for PluginWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let plugin = self.plugin.clone();
        // The panel belongs to the workbench: its inputs, its scroll, its listeners. This window
        // only gives it a place to be drawn — and goes when that workbench does.
        // The workbench went with its window: this one has nothing left to draw, and closes.
        let Some(workbench) = self.workbench.upgrade() else {
            window.remove_window();
            return div();
        };
        let contents = workbench.update(cx, |workbench, cx| workbench.render_plugin_panel_contents(&plugin, cx));
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(hex(Chrome::PANEL))
            .text_color(hex(Chrome::FOREGROUND))
            .font_family(".SystemUIFont")
            .children(contents)
    }
}

impl Workbench {
    /// Opens or closes the plugin's own window to match `mode`.
    pub(super) fn sync_plugin_window(&mut self, plugin: &str, mode: PanelMode, window: &mut Window, cx: &mut Context<Self>) {
        let open = self.plugin_windows.contains_key(plugin);
        match (mode, open) {
            (PanelMode::Window, false) => self.open_plugin_window(plugin, window, cx),
            (PanelMode::Window, true) => {
                if let Some(handle) = self.plugin_windows.get(plugin).copied() {
                    let _ = handle.update(cx, |_, window, _| window.activate_window());
                }
            }
            (_, true) => self.close_plugin_window(plugin, cx),
            (_, false) => {}
        }
    }

    fn open_plugin_window(&mut self, plugin: &str, window: &mut Window, cx: &mut Context<Self>) {
        let title = crate::plugins::plugin(cx, plugin)
            .and_then(|p| p.manifest.as_ref())
            .map(|manifest| manifest.contributes.panel.as_ref().map_or(manifest.name.clone(), |panel| panel.title.clone()))
            .unwrap_or_else(|| plugin.to_string());
        // Beside the window it came from, so it does not land on top of what it was docked to.
        let origin = window.bounds().origin + gpui::point(px(60.), px(60.));
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(origin, size(px(WIDTH), px(HEIGHT))))),
            titlebar: Some(gpui::TitlebarOptions { title: Some(title.into()), ..Default::default() }),
            focus: true,
            show: true,
            ..Default::default()
        };
        let weak = cx.entity().downgrade();
        let owner = plugin.to_string();
        // Opened after this update finishes: the new window draws itself at once, and drawing it
        // asks the workbench for the panel — which it cannot give while it is still being changed.
        cx.defer(move |cx| {
            let workbench = weak.clone();
            let plugin = owner.clone();
            let handle = cx.open_window(options, |_, cx| cx.new(|_| PluginWindow { workbench, plugin }));
            let Some(workbench) = weak.upgrade() else { return };
            match handle {
                Ok(handle) => {
                    let entity = handle.entity(cx).ok();
                    workbench.update(cx, |this, cx| {
                        this.plugin_windows.insert(owner.clone(), handle);
                        // Closing the window from its own button closes the panel: the entity goes
                        // with the window, and the workbench hears about it here.
                        if let Some(entity) = entity {
                            cx.observe_release(&entity, move |this: &mut Workbench, _, cx| {
                                this.plugin_windows.remove(&owner);
                                if this.plugin_panel.as_deref() == Some(owner.as_str()) {
                                    this.close_plugin_panel(cx);
                                }
                            })
                            .detach();
                        }
                    });
                }
                Err(err) => workbench.update(cx, |this, cx| this.set_status(format!("{err}"), cx)),
            }
        });
    }

    /// Closes the plugin's own window, if it has one.
    pub(super) fn close_plugin_window(&mut self, plugin: &str, cx: &mut Context<Self>) {
        if let Some(handle) = self.plugin_windows.remove(plugin) {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
    }
}
