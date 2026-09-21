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

/// What [`Workbench::reconcile_plugin_windows`] has decided: which windows go, and which one to
/// open. Pure on purpose — the rule is what three bugs were in, and it is worth a test that needs
/// no screen.
#[derive(Debug, Default, PartialEq, Eq)]
struct WindowPlan {
    close: Vec<String>,
    open: Option<String>,
}

/// Exactly one window may exist: the open panel's, and only while its mode says so. A window that
/// is already open, or already on its way, is not opened twice.
fn window_plan<'a>(
    panel: Option<&str>,
    mode: Option<PanelMode>,
    open: impl Iterator<Item = &'a str>,
    opening: impl Iterator<Item = &'a str>,
) -> WindowPlan {
    let keep = panel.filter(|_| mode == Some(PanelMode::Window));
    let open: Vec<&str> = open.collect();
    let mut plan = WindowPlan { close: open.iter().filter(|id| keep != Some(**id)).map(|id| id.to_string()).collect(), open: None };
    // Sorted, so a plan over a map with no order of its own is the same plan every time.
    plan.close.sort();
    let Some(plugin) = keep else { return plan };
    if open.contains(&plugin) || opening.into_iter().any(|id| id == plugin) {
        return plan;
    }
    plan.open = Some(plugin.to_string());
    plan
}

impl Workbench {
    /// Brings the windows in line with the panel that is open. Every way a panel can be opened —
    /// an icon on any surface, the Plugins page, the command palette, an idea, the plugin asking
    /// for it with `ui/showPanel` — ends here, in the one place that has a `Window` to open one
    /// with. A call site that has to remember to do this itself is a call site that forgets, and
    /// a `window`-mode panel that opens no window shows the user nothing at all.
    pub(super) fn reconcile_plugin_windows(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mode = self.plugin_panel.clone().map(|plugin| self.plugin_panel_mode(&plugin, cx));
        let plan = window_plan(
            self.plugin_panel.as_deref(),
            mode,
            self.plugin_windows.keys().map(String::as_str),
            self.plugin_windows_opening.iter().map(String::as_str),
        );
        for id in plan.close {
            self.close_plugin_window(&id, cx);
        }
        if let Some(plugin) = plan.open {
            self.open_plugin_window(&plugin, window, cx);
        }
    }

    /// Brings a panel's own window to the front, for when the user asks for a panel that is
    /// already open behind something.
    pub(super) fn activate_plugin_window(&mut self, plugin: &str, cx: &mut Context<Self>) {
        if let Some(handle) = self.plugin_windows.get(plugin).copied() {
            let _ = handle.update(cx, |_, window, _| window.activate_window());
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
        self.plugin_windows_opening.insert(owner.clone());
        // A mark left over from a close whose observer never ran would swallow the next close the
        // user makes; this window has not been closed by anyone yet.
        self.plugin_windows_closing.remove(&owner);
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
                        this.plugin_windows_opening.remove(&owner);
                        this.plugin_windows.insert(owner.clone(), handle);
                        // Closing the window from its own button closes the panel: the entity goes
                        // with the window, and the workbench hears about it here.
                        if let Some(entity) = entity {
                            cx.observe_release(&entity, move |this: &mut Workbench, _, cx| {
                                this.plugin_windows.remove(&owner);
                                // Agentty closed it — to dock the panel somewhere else, or because
                                // the panel is going anyway. Only the user closing the window means
                                // "I am done with this panel".
                                if this.plugin_windows_closing.remove(&owner) {
                                    return;
                                }
                                if this.plugin_panel.as_deref() == Some(owner.as_str()) {
                                    this.close_plugin_panel(cx);
                                }
                            })
                            .detach();
                        }
                    });
                }
                Err(err) => workbench.update(cx, |this, cx| {
                    this.plugin_windows_opening.remove(&owner);
                    this.set_status(format!("{err}"), cx);
                }),
            }
        });
    }

    /// Closes the plugin's own window, if it has one.
    pub(super) fn close_plugin_window(&mut self, plugin: &str, cx: &mut Context<Self>) {
        self.plugin_windows_opening.remove(plugin);
        if let Some(handle) = self.plugin_windows.remove(plugin) {
            // Read by the release observer above, so this close is not taken for the user's.
            self.plugin_windows_closing.insert(plugin.to_string());
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{window_plan, WindowPlan};
    use agentty_bridge::plugins::manifest::PanelMode;

    fn plan(panel: Option<&str>, mode: Option<PanelMode>, open: &[&str], opening: &[&str]) -> WindowPlan {
        window_plan(panel, mode, open.iter().copied(), opening.iter().copied())
    }

    #[test]
    fn a_window_mode_panel_gets_a_window() {
        assert_eq!(plan(Some("hello"), Some(PanelMode::Window), &[], &[]), WindowPlan { close: vec![], open: Some("hello".into()) });
    }

    #[test]
    fn a_panel_docked_or_floating_or_filling_the_area_gets_none() {
        for mode in [PanelMode::Push, PanelMode::Overlay, PanelMode::Full] {
            assert_eq!(plan(Some("hello"), Some(mode), &[], &[]), WindowPlan::default(), "{mode:?}");
        }
    }

    #[test]
    fn the_window_goes_when_the_panel_leaves_it() {
        // The mode was changed from `window` to `push`: the window closes and the panel stays.
        assert_eq!(plan(Some("hello"), Some(PanelMode::Push), &["hello"], &[]), WindowPlan { close: vec!["hello".into()], open: None });
    }

    #[test]
    fn the_window_goes_when_its_plugin_does() {
        // Disabled, uninstalled, or the panel simply closed: nothing is left behind.
        assert_eq!(plan(None, None, &["hello"], &[]), WindowPlan { close: vec!["hello".into()], open: None });
    }

    #[test]
    fn another_plugin_s_window_is_not_left_open_behind_this_one() {
        assert_eq!(
            plan(Some("hello"), Some(PanelMode::Window), &["other"], &[]),
            WindowPlan { close: vec!["other".into()], open: Some("hello".into()) }
        );
    }

    #[test]
    fn a_window_that_exists_is_not_opened_again() {
        assert_eq!(plan(Some("hello"), Some(PanelMode::Window), &["hello"], &[]), WindowPlan::default());
    }

    #[test]
    fn a_window_on_its_way_is_not_opened_again() {
        // Opening is deferred to after the update; without this the next frame asks for a second.
        assert_eq!(plan(Some("hello"), Some(PanelMode::Window), &[], &["hello"]), WindowPlan::default());
    }

    #[test]
    fn every_stale_window_goes_at_once() {
        assert_eq!(
            plan(Some("hello"), Some(PanelMode::Window), &["one", "two"], &[]),
            WindowPlan { close: vec!["one".into(), "two".into()], open: Some("hello".into()) }
        );
    }
}
