//! Plugin UI inside the window: the panel docked right of the terminals (the area a plugin fills
//! with its own UI tree) and plugin buttons in the tab strip.

use super::Workbench;
use crate::i18n::{t, tf};
use crate::plugins::{self, RunState};
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, icon_named, IconSize, Tooltip, TypeScale};
use agentty_bridge::plugins::manifest::{PanelMode, Surface};
use agentty_bridge::plugins::ui::{Gap, Node, TextStyle, Tone, UiEvent, Variant};
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, Entity, Focusable, SharedString, Subscription, Window};
use std::time::Duration;

/// One plugin's entry on a surface: which plugin, what to call it, and what to draw for it.
struct SurfaceEntry {
    id: String,
    title: String,
    glyph: &'static str,
    /// The plugin's own logo on disk, preferred over `glyph`.
    logo: Option<std::path::PathBuf>,
    badge: String,
}

/// A text field of a plugin panel, kept across renders.
pub struct PluginInput {
    pub input: Entity<TextInput>,
    /// Last value the plugin sent; a different one replaces what is typed.
    applied: String,
    generation: u64,
    /// Enter was pressed and the plugin has not answered yet.
    submitted: bool,
    _subscription: Subscription,
}

fn gap(gap: Gap) -> gpui::Pixels {
    px(match gap {
        Gap::None => 0.,
        Gap::Small => 4.,
        Gap::Medium => 8.,
        Gap::Large => 14.,
    })
}

fn tone_color(tone: Tone) -> u32 {
    match tone {
        Tone::Neutral => Chrome::MUTED,
        Tone::Info => Chrome::BLUE,
        Tone::Success => Chrome::SUCCESS,
        Tone::Warning => Chrome::WARNING,
        Tone::Error => Chrome::ERROR,
    }
}

impl Workbench {
    /// Creates and syncs the panel's text fields; called from render before drawing.
    /// Whose fields the panel shows: an automation's are its own (the same id in another tab is
    /// another field), so they are made and looked up under the automation in front.
    fn plugin_input_scope(&self, plugin: &str, cx: &gpui::App) -> String {
        match self.active_instance(plugin, cx) {
            Some(instance) => format!("{plugin}#{instance}"),
            None => plugin.to_string(),
        }
    }

    pub(super) fn prepare_plugin_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A plugin's workspace in front whose plugin just came back (turned off and on, updated)
        // gets its panel again: nothing else switches workspaces meanwhile to bring it up.
        self.sync_plugin_workspace(cx);
        // In a plugin's workspace the panel is part of the layout: dropped while the workspace
        // stayed in front (its plugin turned off and on), it comes back. Another plugin's panel
        // opened from there is left alone.
        if let Some(front) = self.front_plugin_workspace(cx).filter(|_| self.page.is_none()) {
            if self.plugin_panel.is_none() {
                self.open_plugin_panel(&front, cx);
            }
        }
        let Some(plugin) = self.plugin_panel.clone() else {
            self.reconcile_plugin_windows(window, cx);
            self.plugin_inputs.clear();
            return;
        };
        // A plugin that was disabled or removed closes its panel, and its own window with it.
        if plugins::plugin(cx, &plugin).is_none_or(|p| !p.active()) {
            self.drop_plugin_panel(&plugin, cx);
            self.reconcile_plugin_windows(window, cx);
            self.plugin_inputs.clear();
            return;
        }
        self.reconcile_plugin_windows(window, cx);
        let mut fields = Vec::new();
        if let Some(tree) = self.plugin_tree(&plugin, cx) {
            tree.inputs(&mut fields);
        }
        let scope = self.plugin_input_scope(&plugin, cx);
        self.plugin_inputs.retain(|(owner, id), _| *owner == scope && fields.iter().any(|field| field.id == *id));
        for agentty_bridge::plugins::ui::InputField { id, placeholder, value, rows } in fields {
            let key = (scope.clone(), id.clone());
            if let Some(existing) = self.plugin_inputs.get_mut(&key) {
                existing.input.update(cx, |i, cx| i.set_placeholder(placeholder.clone(), cx));
                if existing.applied != value {
                    existing.applied = value.clone();
                    let input = existing.input.clone();
                    let typed = input.read(cx).text().to_string();
                    // A plugin echoing an older value while the user keeps typing must not undo
                    // keystrokes. Right after Enter, though, an empty value is the plugin clearing
                    // the field it just took the text from.
                    let clears_after_submit = std::mem::take(&mut existing.submitted) && value.is_empty();
                    let stale_echo = input.focus_handle(cx).is_focused(window) && typed.starts_with(&value) && !clears_after_submit;
                    if typed != value && !stale_echo {
                        input.update(cx, |i, cx| i.set_text(value, cx));
                    }
                }
                continue;
            }
            // Built empty and filled, so the caret sits at the end instead of selecting everything.
            let input = cx.new(|cx| {
                // More than one row is a text area: Enter adds a line and a paste keeps its own.
                let mut input = TextInput::new("", placeholder, window, cx).multiline(rows);
                input.set_text(value.clone(), cx);
                input
            });
            let (owner, element, scope) = (plugin.clone(), id.clone(), scope.clone());
            let subscription = cx.subscribe(&input, move |this, input, event: &TextInputEvent, cx| {
                let text = input.read(cx).text().to_string();
                let key = (scope.clone(), element.clone());
                match event {
                    TextInputEvent::Confirmed => {
                        // The plugin now has this text: a value it sends back that differs (an
                        // empty one after it took what was typed) replaces it.
                        if let Some(field) = this.plugin_inputs.get_mut(&key) {
                            field.applied = text.clone();
                            field.submitted = true;
                        }
                        let event = UiEvent {
                            element: element.clone(),
                            event: "submit".into(),
                            value: Some(text.into()),
                            item: None,
                            action: None,
                        };
                        this.send_plugin_event(&owner, event, cx);
                    }
                    TextInputEvent::Changed => {
                        // Typing sends `change` once it pauses; the plugin's own updates don't echo back.
                        let Some(field) = this.plugin_inputs.get_mut(&key) else { return };
                        // Typing again: an empty value from the plugin is an old one, not a clear.
                        field.submitted = false;
                        if field.applied == text {
                            return;
                        }
                        field.generation += 1;
                        let generation = field.generation;
                        let (owner, element, scope) = (owner.clone(), element.clone(), key.0.clone());
                        cx.spawn(async move |this, cx| {
                            cx.background_executor().timer(Duration::from_millis(250)).await;
                            let _ = this.update(cx, |this, cx| {
                                let current =
                                    this.plugin_inputs.get(&(scope.clone(), element.clone())).is_some_and(|f| f.generation == generation);
                                if current {
                                    if let Some(field) = this.plugin_inputs.get_mut(&(scope.clone(), element.clone())) {
                                        field.applied = text.clone();
                                    }
                                    let event = UiEvent {
                                        element: element.clone(),
                                        event: "change".into(),
                                        value: Some(text.into()),
                                        item: None,
                                        action: None,
                                    };
                                    this.send_plugin_event(&owner, event, cx);
                                }
                            });
                        })
                        .detach();
                    }
                    _ => {}
                }
            });
            self.plugin_inputs
                .insert(key, PluginInput { input, applied: value, generation: 0, submitted: false, _subscription: subscription });
        }
    }

    /// The panel itself — its header and what the plugin drew — without saying where it sits.
    pub(super) fn render_plugin_panel_contents(&self, plugin_id: &str, cx: &mut Context<Self>) -> Option<AnyElement> {
        let plugin_id = plugin_id.to_string();
        let plugin = plugins::plugin(cx, &plugin_id)?.clone();
        let manifest = plugin.manifest.clone()?;
        let close = icon_only_close(cx);
        let runtime = plugins::runtime(cx, &plugin_id);
        let state = runtime.map(|r| r.state.clone()).unwrap_or(RunState::Stopped);
        let tree = self.plugin_tree(&plugin_id, cx).cloned();
        // Its workspace in front with every tab closed: no automation to show a panel for.
        let no_automation = self.front_plugin_workspace(cx).as_deref() == Some(plugin_id.as_str())
            && self.workspaces.get(self.active_workspace).is_some_and(|ws| ws.tabs.is_empty());
        let log_tail: Vec<String> = runtime.map(|r| r.logs.iter().rev().take(12).rev().cloned().collect()).unwrap_or_default();
        let panel_title = manifest.contributes.panel.as_ref().map_or(manifest.name.clone(), |p| p.title.clone());
        let panel_icon = icon_named(manifest.contributes.panel.as_ref().and_then(|p| p.icon.as_deref()).or(manifest.icon.as_deref()));
        let panel_logo = agentty_bridge::plugins::store::logo_file(&plugin);

        let restart_id = plugin_id.clone();
        let header = div()
            .h(px(36.))
            .flex_shrink_0()
            .px_2()
            .flex()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(hex(Chrome::BORDER))
            .bg(hex(Chrome::SIDE_BAR))
            .child(crate::ui::plugin_mark(panel_logo, panel_icon, IconSize::BUTTON, hex(Chrome::BRIGHT)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .t_body()
                    .font_weight(crate::theme::EMPHASIS)
                    .text_color(hex(Chrome::BRIGHT))
                    .child(panel_title),
            )
            .child(
                crate::ui::icon_only(
                    "plugin-panel-restart",
                    "rotate-cw",
                    cx.listener(move |_, _: &ClickEvent, _, cx| {
                        plugins::restart(&restart_id, cx);
                    }),
                )
                .tooltip(Tooltip::text(t(cx, "plugins.restart"), None)),
            )
            .child(
                crate::ui::icon_only(
                    "plugin-panel-layout",
                    "layout-panel-left",
                    cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.plugin_mode_menu = !this.plugin_mode_menu;
                        cx.notify();
                    }),
                )
                .tooltip(Tooltip::text(t(cx, "plugins.mode"), None)),
            )
            .child(
                crate::ui::icon_only(
                    "plugin-panel-manage",
                    "settings",
                    cx.listener(move |this, _: &ClickEvent, _, cx| {
                        let focus = this.plugin_panel.clone();
                        this.open_plugins_page(focus, cx);
                    }),
                )
                .tooltip(Tooltip::text(t(cx, "plugins.manage"), None)),
            )
            .child(close);

        let mode_menu = self.plugin_mode_menu.then(|| self.render_panel_mode_menu(&plugin_id, cx));

        let body: AnyElement = match (tree, state) {
            (_, RunState::NeedsConsent) => {
                let owner = plugin_id.clone();
                div()
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().t_body().text_color(hex(Chrome::FOREGROUND)).child(tf(
                        cx,
                        "plugins.consent.panel",
                        &[("name", &manifest.name)],
                    )))
                    .child(crate::ui::action_button(
                        "plugin-consent-again",
                        t(cx, "plugins.consent.review"),
                        cx.listener(move |_, _: &gpui::ClickEvent, _, cx| plugins::ask_consent_again(&owner, cx)),
                    ))
                    .into_any_element()
            }
            (_, RunState::Failed(error)) => div()
                .p_3()
                .flex()
                .flex_col()
                .gap_2()
                .child(div().t_body().text_color(hex(Chrome::ERROR)).child(tf(cx, "plugins.failed", &[("name", &manifest.name)])))
                .child(div().t_small().text_color(hex(Chrome::FOREGROUND)).child(error))
                .child(
                    div()
                        .p_2()
                        .rounded_md()
                        .bg(hex(0x1a1a1a))
                        .t_caption()
                        .font_family("JetBrains Mono")
                        .text_color(hex(Chrome::MUTED))
                        .child(log_tail.join("\n")),
                )
                .into_any_element(),
            // What the plugin drew for no automation (if anything) would wait for one forever:
            // say what happened and offer the way out.
            _ if no_automation => {
                let owner = plugin_id.clone();
                div()
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().t_body().text_color(hex(Chrome::BRIGHT)).child(t(cx, "plugins.no_automation")))
                    .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "plugins.no_automation.hint")))
                    .child(crate::ui::action_button(
                        "plugin-new-automation",
                        t(cx, "plugins.new_automation"),
                        cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                            // A double click is one automation, not two.
                            if this.workspaces.get(this.active_workspace).is_some_and(|ws| ws.tabs.is_empty()) {
                                this.new_plugin_instance(&owner, cx);
                            }
                        }),
                    ))
                    .into_any_element()
            }
            (Some(tree), _) => {
                let mut path = Vec::new();
                div().p_3().child(self.render_plugin_node(&plugin_id, &tree, &mut path, cx)).into_any_element()
            }
            (None, _) => crate::ui::loading_row(t(cx, "plugins.starting")).into_any_element(),
        };

        Some(
            div()
                .size_full()
                .flex()
                .flex_col()
                .bg(hex(Chrome::PANEL))
                .relative()
                .child(header)
                .child(
                    div()
                        .id("plugin-panel-scroll")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .track_scroll(&self.plugin_scroll)
                        .relative()
                        .child(body)
                        .group(crate::ui::SCROLL_GROUP)
                        .child(crate::ui::scrollbar(self.plugin_scroll.clone())),
                )
                // Last, so it paints over the panel's body instead of under it.
                .children(mode_menu)
                .into_any_element(),
        )
    }

    /// The panel docked beside the terminals, which move over to make room. `None` unless that is
    /// how this panel opens.
    pub(super) fn render_plugin_panel(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let plugin_id = self.plugin_panel.clone()?;
        if !self.plugin_panel_docked_here(&plugin_id, cx) {
            return None;
        }
        let width = self.plugin_panel_width(cx);
        let contents = self.render_plugin_panel_contents(&plugin_id, cx)?;
        Some(div().w(px(width)).flex_shrink_0().h_full().child(contents).into_any_element())
    }

    /// The panel drawn over the terminals: floating at the right edge, or filling the area. A
    /// panel in a window of its own is drawn there, not here.
    pub(super) fn render_plugin_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let plugin_id = self.plugin_panel.clone()?;
        let mode = self.plugin_panel_mode(&plugin_id, cx);
        if matches!(mode, PanelMode::Push | PanelMode::Window | PanelMode::Workspace) {
            return None;
        }
        let contents = self.render_plugin_panel_contents(&plugin_id, cx)?;
        let panel = match mode {
            // Floating: above the row at its right edge, with its own edge to drag.
            PanelMode::Overlay => div()
                .absolute()
                .top_0()
                .bottom_0()
                .right_0()
                .w(px(self.plugin_panel_shown_width(cx)))
                .flex()
                .shadow_lg()
                .child(self.render_side_splitter(super::side_panels::SidePanel::Plugin, cx))
                .child(div().flex_1().min_w_0().h_full().border_l_1().border_color(hex(Chrome::BORDER)).child(contents)),
            // The whole area the terminals and pages use.
            _ => div().absolute().inset_0().flex().child(div().size_full().child(contents)),
        };
        Some(panel.into_any_element())
    }

    /// The menu behind the panel's layout button: where this panel opens.
    fn render_panel_mode_menu(&self, plugin_id: &str, cx: &mut Context<Self>) -> AnyElement {
        let current = self.plugin_panel_mode(plugin_id, cx);
        let mut menu = div()
            .occlude()
            .absolute()
            .top(px(30.))
            .right(px(4.))
            .w(px(220.))
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(hex(Chrome::OVERLAY_BORDER))
            .bg(hex(Chrome::OVERLAY))
            .shadow_lg()
            .flex()
            .flex_col();
        for mode in PanelMode::ALL.iter().copied() {
            let (key, glyph) = match mode {
                PanelMode::Push => ("plugins.mode.push", "columns-2"),
                PanelMode::Overlay => ("plugins.mode.overlay", "layout-panel-left"),
                PanelMode::Window => ("plugins.mode.window", "app-window"),
                PanelMode::Full => ("plugins.mode.full", "maximize-2"),
                PanelMode::Workspace => ("plugins.mode.workspace", "square-terminal"),
            };
            let plugin = plugin_id.to_string();
            menu = menu.child(
                div()
                    .id(SharedString::from(format!("plugin-panel-mode-{}", mode.id())))
                    .px_2()
                    .py_1p5()
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .when(mode == current, |d| d.bg(hex(Chrome::SELECTED)))
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.plugin_mode_menu = false;
                        this.set_plugin_panel_mode(&plugin, mode, window, cx);
                    }))
                    .child(icon(glyph, IconSize::INLINE, hex(if mode == current { Chrome::BRIGHT } else { Chrome::MUTED })))
                    .child(div().flex_1().t_small().text_color(hex(Chrome::FOREGROUND)).child(t(cx, key))),
            );
        }
        menu.into_any_element()
    }

    fn render_plugin_node(&self, plugin: &str, node: &Node, path: &mut Vec<usize>, cx: &mut Context<Self>) -> AnyElement {
        let key = |id: &str, path: &[usize]| SharedString::from(format!("plugin-{plugin}-{id}-{path:?}"));
        match node {
            Node::Column { children, gap: g } => {
                let mut column = div().flex().flex_col().gap(gap(*g)).min_w_0();
                for (index, child) in children.iter().enumerate() {
                    path.push(index);
                    column = column.child(self.render_plugin_node(plugin, child, path, cx));
                    path.pop();
                }
                column.into_any_element()
            }
            Node::Section { title, children } => {
                let mut column = div().flex().flex_col().gap_2().min_w_0();
                for (index, child) in children.iter().enumerate() {
                    path.push(index);
                    column = column.child(self.render_plugin_node(plugin, child, path, cx));
                    path.pop();
                }
                div()
                    .pt_2()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().t_caption().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::MUTED)).child(title.to_uppercase()))
                    .child(column)
                    .into_any_element()
            }
            Node::Row { children, gap: g, wrap } => {
                let mut row = div().flex().items_center().gap(gap(*g)).min_w_0().when(*wrap, |d| d.flex_wrap());
                for (index, child) in children.iter().enumerate() {
                    path.push(index);
                    let rendered = self.render_plugin_node(plugin, child, path, cx);
                    // Text takes the room the buttons leave. Left to its own size it shrinks to its
                    // narrowest wrap — one character a line for Korean, Japanese and Chinese.
                    row = row.child(if matches!(child, Node::Text { .. }) {
                        div().flex_1().min_w_0().child(rendered).into_any_element()
                    } else {
                        rendered
                    });
                    path.pop();
                }
                row.into_any_element()
            }
            Node::Text { text, style } => {
                let base = div().min_w_0().whitespace_normal();
                match style {
                    TextStyle::Title => base.t_title().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)),
                    TextStyle::Muted => base.t_small().text_color(hex(Chrome::MUTED)),
                    TextStyle::Small => base.t_caption().text_color(hex(Chrome::MUTED)),
                    TextStyle::Code => base
                        .t_small()
                        .font_family("JetBrains Mono")
                        .p_2()
                        .rounded_md()
                        .bg(hex(0x1a1a1a))
                        .text_color(hex(Chrome::FOREGROUND)),
                    TextStyle::Error => base.t_small().text_color(hex(Chrome::ERROR)),
                    TextStyle::Success => base.t_small().text_color(hex(Chrome::SUCCESS)),
                    TextStyle::Body => base.t_body().text_color(hex(Chrome::FOREGROUND)),
                }
                .child(text.clone())
                .into_any_element()
            }
            Node::Button { id, label, icon: glyph, variant, disabled } => {
                let (bg, fg) = match variant {
                    Variant::Primary => (hex(Chrome::ACCENT), hex(Chrome::BRIGHT)),
                    Variant::Secondary => (hex(0x2d2d30), hex(Chrome::FOREGROUND)),
                    Variant::Ghost => (hex_alpha(0, 0.), hex(Chrome::FOREGROUND)),
                    Variant::Danger => (hex_alpha(Chrome::ERROR, 0.2), hex(Chrome::ERROR)),
                };
                let (owner, element) = (plugin.to_string(), id.clone());
                div()
                    .id(key(id, path))
                    .flex_shrink_0()
                    .px_2()
                    .py_1()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .rounded_md()
                    .t_small()
                    .bg(bg)
                    .text_color(fg)
                    .when(*disabled, |d| d.opacity(0.45))
                    .when(!*disabled, |d| {
                        d.cursor_pointer().hover(|s| s.opacity(0.85)).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            let event = UiEvent { element: element.clone(), event: "click".into(), value: None, item: None, action: None };
                            this.send_plugin_event(&owner, event, cx);
                        }))
                    })
                    .children(glyph.as_deref().map(|g| icon(icon_named(Some(g)), IconSize::INLINE, fg)))
                    .child(label.clone())
                    .into_any_element()
            }
            Node::Input { id, .. } => match self.plugin_inputs.get(&(self.plugin_input_scope(plugin, cx), id.clone())) {
                Some(field) => div()
                    .w_full()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(hex(Chrome::BORDER))
                    .bg(hex(0x1a1a1a))
                    .t_small()
                    .text_color(hex(Chrome::BRIGHT))
                    .child(field.input.clone())
                    .into_any_element(),
                None => div().into_any_element(),
            },
            Node::List { id, items, empty } => {
                if items.is_empty() {
                    return div().t_small().text_color(hex(Chrome::MUTED)).children(empty.clone()).into_any_element();
                }
                let mut list = div().flex().flex_col().gap_0p5();
                for (index, item) in items.iter().enumerate() {
                    let (owner, element, item_id) = (plugin.to_string(), id.clone(), item.id.clone());
                    let group = SharedString::from(format!("plugin-row-{plugin}-{id}-{index}"));
                    // Shown over the right end of the row while it is hovered, so they take no room
                    // (and leave no gap) the rest of the time.
                    let mut actions =
                        div().absolute().top_0().bottom_0().right(px(4.)).pl_2().flex().items_center().gap_0p5().bg(hex(Chrome::HOVER));
                    let has_actions = !item.actions.is_empty();
                    for (action_index, action) in item.actions.iter().enumerate() {
                        let (owner, element, item_id, action_id) = (plugin.to_string(), id.clone(), item.id.clone(), action.id.clone());
                        let mut button = div()
                            .id(SharedString::from(format!("plugin-row-action-{plugin}-{id}-{index}-{action_index}")))
                            .px_1()
                            .h(px(22.))
                            .flex()
                            .items_center()
                            .gap_1()
                            .rounded_sm()
                            .cursor_pointer()
                            .t_caption()
                            .text_color(hex(Chrome::FOREGROUND))
                            .hover(|s| s.bg(hex(Chrome::SELECTED)))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                cx.stop_propagation();
                                let event = UiEvent {
                                    element: element.clone(),
                                    event: "action".into(),
                                    value: None,
                                    item: Some(item_id.clone()),
                                    action: Some(action_id.clone()),
                                };
                                this.send_plugin_event(&owner, event, cx);
                            }))
                            .when_some(action.icon.as_deref(), |d, g| {
                                d.child(icon(icon_named(Some(g)), IconSize::INLINE, hex(Chrome::FOREGROUND)))
                            })
                            .when_some(action.label.clone(), |d, label| d.child(label));
                        if let Some(tooltip) = action.tooltip.clone() {
                            button = button.tooltip(Tooltip::text(tooltip, None));
                        }
                        actions = actions.child(button);
                    }
                    list = list.child(
                        div()
                            .id(SharedString::from(format!("plugin-row-{plugin}-{id}-{index}")))
                            .group(group.clone())
                            .relative()
                            .px_2()
                            .py_1p5()
                            .flex()
                            .items_center()
                            .gap_2()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|s| s.bg(hex(Chrome::HOVER)))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                let event = UiEvent {
                                    element: element.clone(),
                                    event: "select".into(),
                                    value: None,
                                    item: Some(item_id.clone()),
                                    action: None,
                                };
                                this.send_plugin_event(&owner, event, cx);
                            }))
                            .when_some(item.icon.as_deref(), |d, g| {
                                d.child(icon(icon_named(Some(g)), IconSize::INLINE, hex(tone_color(item.tone))))
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(div().t_small().text_color(hex(Chrome::BRIGHT)).truncate().child(item.title.clone()))
                                    .children(
                                        item.subtitle.clone().map(|s| div().t_caption().text_color(hex(Chrome::MUTED)).truncate().child(s)),
                                    )
                                    // The detail is a line of its own under them: beside them it took
                                    // the width the title needs as soon as the panel is narrow.
                                    .children(
                                        item.detail
                                            .clone()
                                            .map(|d| div().t_caption().text_color(hex_alpha(Chrome::MUTED, 0.8)).truncate().child(d)),
                                    ),
                            )
                            .when(has_actions, |d| d.child(actions.invisible().group_hover(group, |s| s.visible()))),
                    );
                }
                list.into_any_element()
            }
            Node::Choice { id, options, value } => {
                let mut row = div().flex().flex_wrap().gap_1();
                for (index, option) in options.iter().enumerate() {
                    let (owner, element, picked) = (plugin.to_string(), id.clone(), option.value.clone());
                    row = row.child(crate::ui::chip(
                        SharedString::from(format!("plugin-choice-{plugin}-{id}-{index}")),
                        option.label.clone(),
                        *value == option.value,
                        cx.listener(move |this, _: &ClickEvent, _, cx| {
                            let event = UiEvent {
                                element: element.clone(),
                                event: "change".into(),
                                value: Some(picked.clone().into()),
                                item: None,
                                action: None,
                            };
                            this.send_plugin_event(&owner, event, cx);
                        }),
                    ));
                }
                row.into_any_element()
            }
            Node::Toggle { id, label, value } => {
                let (owner, element, next) = (plugin.to_string(), id.clone(), !*value);
                div()
                    .id(key(id, path))
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .t_small()
                    .text_color(hex(Chrome::FOREGROUND))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        let event = UiEvent {
                            element: element.clone(),
                            event: "change".into(),
                            value: Some(next.into()),
                            item: None,
                            action: None,
                        };
                        this.send_plugin_event(&owner, event, cx);
                    }))
                    .child(
                        div()
                            .w(px(26.))
                            .h(px(14.))
                            .rounded_full()
                            .p(px(2.))
                            .flex()
                            .when(*value, |d| d.justify_end())
                            .bg(if *value { hex(Chrome::ACCENT) } else { hex(0x3a3a3a) })
                            .child(div().size(px(10.)).rounded_full().bg(hex(Chrome::BRIGHT))),
                    )
                    .child(label.clone())
                    .into_any_element()
            }
            Node::Badge { text, tone } => div()
                .flex_shrink_0()
                .px_1p5()
                .py_0p5()
                .rounded_sm()
                .bg(hex_alpha(tone_color(*tone), 0.18))
                .t_caption()
                .text_color(hex(tone_color(*tone)))
                .child(text.clone())
                .into_any_element(),
            Node::Spinner { text } => crate::ui::loading_row(text.clone()).into_any_element(),
            Node::Divider => div().h(px(1.)).w_full().bg(hex(Chrome::BORDER)).into_any_element(),
        }
    }

    /// Enabled plugins whose panel sits on `surface`: (id, title, icon, badge).
    fn plugin_surface_entries(&self, surface: Surface, cx: &Context<Self>) -> Vec<SurfaceEntry> {
        plugins::active(cx)
            .filter_map(|(plugin, manifest)| {
                let panel = manifest.contributes.panel.as_ref().filter(|_| manifest.surface() == surface)?;
                let glyph = icon_named(panel.icon.as_deref().or(manifest.icon.as_deref()));
                let badge = plugins::runtime(cx, &plugin.id).map(|r| r.badge.clone()).unwrap_or_default();
                Some(SurfaceEntry {
                    id: plugin.id.clone(),
                    title: panel.title.clone(),
                    glyph,
                    // The plugin's own artwork wins over the icon name when it is there.
                    logo: agentty_bridge::plugins::store::logo_file(plugin),
                    badge,
                })
            })
            .collect()
    }

    /// Whether this plugin's panel is the one on screen.
    fn plugin_panel_open(&self, id: &str) -> bool {
        self.plugin_panel.as_deref() == Some(id) && self.page.is_none()
    }

    /// An icon on any of the three surfaces: opens or closes the panel, wherever that panel goes.
    pub(super) fn toggle_plugin_surface(&mut self, plugin: &str, window: &mut Window, cx: &mut Context<Self>) {
        // The window the panel may need is opened by `reconcile_plugin_windows` on the next
        // render, the same as for every other way a panel opens.
        self.toggle_plugin(plugin, window, cx);
    }

    /// Tab-strip buttons of plugins that put their panel there (the default surface).
    pub(super) fn render_plugin_header_buttons(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        self.plugin_surface_entries(Surface::Pane, cx)
            .into_iter()
            .map(|SurfaceEntry { id, title, glyph, logo, badge }| {
                let open = self.plugin_panel_open(&id);
                let target = id.clone();
                div()
                    .id(SharedString::from(format!("header-plugin-{id}")))
                    .tooltip(Tooltip::text(title, None))
                    .flex_shrink_0()
                    .my_auto()
                    .h(px(crate::ui::ICON_BUTTON))
                    .min_w(px(crate::ui::ICON_BUTTON))
                    .px_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap_1()
                    .rounded_md()
                    .cursor_pointer()
                    .when(open, |d| d.bg(hex(Chrome::SELECTED)))
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.toggle_plugin_surface(&target, window, cx)))
                    .child(crate::ui::plugin_mark(
                        logo,
                        glyph,
                        IconSize::BUTTON,
                        hex(if open { Chrome::BRIGHT } else { Chrome::FOREGROUND }),
                    ))
                    .when(!badge.is_empty(), |d| d.child(div().t_caption().text_color(hex(Chrome::BRIGHT)).child(badge)))
                    .into_any_element()
            })
            .collect()
    }

    /// Activity-bar items of plugins that ask for the sidebar. They look and behave like
    /// Agentty's own items, and the bar scrolls once there are more than fit.
    pub(super) fn render_plugin_activity_items(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        self.plugin_surface_entries(Surface::Sidebar, cx)
            .into_iter()
            .map(|SurfaceEntry { id, title, glyph, logo, badge }| {
                let open = self.plugin_panel_open(&id);
                let target = id.clone();
                let element_id = SharedString::from(format!("activity-plugin-{id}"));
                div()
                    .id(element_id.clone())
                    .group(element_id.clone())
                    .tooltip(Tooltip::text(title, None))
                    .w_full()
                    .h(px(48.))
                    .flex_shrink_0()
                    .relative()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .border_l_2()
                    .border_color(if open { hex(Chrome::BRIGHT) } else { hex_alpha(0, 0.) })
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.toggle_plugin_surface(&target, window, cx)))
                    .child(crate::ui::plugin_mark(logo, glyph, IconSize::ACTIVITY, if open { hex(Chrome::BRIGHT) } else { hex(0x858585) }))
                    .when(!badge.is_empty(), |d| {
                        // The bar is only so wide: enough of the badge to read at a glance.
                        let badge: String = badge.chars().take(3).collect();
                        d.child(
                            div()
                                .absolute()
                                .bottom(px(6.))
                                .right(px(4.))
                                .px_1()
                                .rounded_sm()
                                .bg(hex(Chrome::ACCENT))
                                .t_caption()
                                .text_color(hex(Chrome::BRIGHT))
                                .child(badge),
                        )
                    })
                    .into_any_element()
            })
            .collect()
    }

    /// Status-bar items of plugins that ask for the bottom bar, at the left end of it.
    pub(super) fn render_plugin_status_items(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        self.plugin_surface_entries(Surface::Status, cx)
            .into_iter()
            .map(|SurfaceEntry { id, title, glyph, logo, badge }| {
                let open = self.plugin_panel_open(&id);
                let target = id.clone();
                div()
                    .id(SharedString::from(format!("status-plugin-{id}")))
                    .tooltip(Tooltip::text(title, None))
                    .h_full()
                    .px_1p5()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap_1()
                    .cursor_pointer()
                    .when(open, |d| d.bg(hex(Chrome::SELECTED)))
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.toggle_plugin_surface(&target, window, cx)))
                    .child(crate::ui::plugin_mark(logo, glyph, 13., hex(if open { Chrome::BRIGHT } else { Chrome::MUTED })))
                    .when(!badge.is_empty(), |d| d.child(div().t_caption().text_color(hex(Chrome::BRIGHT)).child(badge)))
                    .into_any_element()
            })
            .collect()
    }
}

fn icon_only_close(cx: &mut Context<Workbench>) -> impl IntoElement {
    crate::ui::icon_only("plugin-panel-close", "x", cx.listener(|this, _: &ClickEvent, _, cx| this.close_plugin_panel(cx)))
}
