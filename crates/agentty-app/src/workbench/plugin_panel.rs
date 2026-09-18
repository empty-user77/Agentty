//! Plugin UI inside the window: the panel docked right of the terminals (the area a plugin fills
//! with its own UI tree), plugin buttons in the tab strip, and pane-bar buttons above terminals.

use super::{Pane, Workbench};
use crate::i18n::{t, tf};
use crate::launch::PaneKind;
use crate::plugins::{self, RunState};
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, icon_named, IconSize, Tooltip, TypeScale};
use agentty_bridge::plugins::manifest::When;
use agentty_bridge::plugins::ui::{Gap, Node, TextStyle, Tone, UiEvent, Variant};
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, Entity, Focusable, FontWeight, SharedString, Subscription, Window};
use std::time::Duration;

pub const PANEL_WIDTH: f32 = 360.;

/// A text field of a plugin panel, kept across renders.
pub struct PluginInput {
    pub input: Entity<TextInput>,
    /// Last value the plugin sent; a different one replaces what is typed.
    applied: String,
    generation: u64,
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
    pub(super) fn prepare_plugin_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(plugin) = self.plugin_panel.clone() else {
            self.plugin_inputs.clear();
            return;
        };
        // A plugin that was disabled or removed closes its panel.
        if plugins::plugin(cx, &plugin).is_none_or(|p| !p.active()) {
            self.plugin_panel = None;
            self.plugin_inputs.clear();
            return;
        }
        let mut fields = Vec::new();
        if let Some(tree) = plugins::runtime(cx, &plugin).and_then(|r| r.panel.as_ref()) {
            tree.inputs(&mut fields);
        }
        self.plugin_inputs.retain(|(owner, id), _| *owner == plugin && fields.iter().any(|(field, _, _)| field == id));
        for (id, placeholder, value) in fields {
            let key = (plugin.clone(), id.clone());
            if let Some(existing) = self.plugin_inputs.get_mut(&key) {
                if existing.applied != value {
                    existing.applied = value.clone();
                    let input = existing.input.clone();
                    let typed = input.read(cx).text().to_string();
                    // A plugin echoing an older value while the user keeps typing must not undo keystrokes.
                    let stale_echo = input.focus_handle(cx).is_focused(window) && typed.starts_with(&value);
                    if typed != value && !stale_echo {
                        input.update(cx, |i, cx| i.set_text(value, cx));
                    }
                }
                continue;
            }
            // Built empty and filled, so the caret sits at the end instead of selecting everything.
            let input = cx.new(|cx| {
                let mut input = TextInput::new("", placeholder, window, cx);
                input.set_text(value.clone(), cx);
                input
            });
            let (owner, element) = (plugin.clone(), id.clone());
            let subscription = cx.subscribe(&input, move |this, input, event: &TextInputEvent, cx| {
                let text = input.read(cx).text().to_string();
                let key = (owner.clone(), element.clone());
                match event {
                    TextInputEvent::Confirmed => {
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
                        if field.applied == text {
                            return;
                        }
                        field.generation += 1;
                        let generation = field.generation;
                        let (owner, element) = (owner.clone(), element.clone());
                        cx.spawn(async move |this, cx| {
                            cx.background_executor().timer(Duration::from_millis(250)).await;
                            let _ = this.update(cx, |this, cx| {
                                let current =
                                    this.plugin_inputs.get(&(owner.clone(), element.clone())).is_some_and(|f| f.generation == generation);
                                if current {
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
            self.plugin_inputs.insert(key, PluginInput { input, applied: value, generation: 0, _subscription: subscription });
        }
    }

    pub(super) fn render_plugin_panel(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let plugin_id = self.plugin_panel.clone()?;
        let plugin = plugins::plugin(cx, &plugin_id)?.clone();
        let manifest = plugin.manifest.clone()?;
        let close = icon_only_close(cx);
        let runtime = plugins::runtime(cx, &plugin_id);
        let state = runtime.map(|r| r.state.clone()).unwrap_or(RunState::Stopped);
        let tree = runtime.and_then(|r| r.panel.clone());
        let log_tail: Vec<String> = runtime.map(|r| r.logs.iter().rev().take(12).rev().cloned().collect()).unwrap_or_default();
        let panel_title = manifest.contributes.panel.as_ref().map_or(manifest.name.clone(), |p| p.title.clone());
        let panel_icon = icon_named(manifest.contributes.panel.as_ref().and_then(|p| p.icon.as_deref()).or(manifest.icon.as_deref()));

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
            .child(icon(panel_icon, IconSize::BUTTON, hex(Chrome::BRIGHT)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .t_body()
                    .font_weight(FontWeight::SEMIBOLD)
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

        let body: AnyElement = match (tree, state) {
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
            (Some(tree), _) => {
                let mut path = Vec::new();
                div().p_3().child(self.render_plugin_node(&plugin_id, &tree, &mut path, cx)).into_any_element()
            }
            (None, _) => crate::ui::loading_row(t(cx, "plugins.starting")).into_any_element(),
        };

        Some(
            div()
                .w(px(PANEL_WIDTH))
                .flex_shrink_0()
                .h_full()
                .flex()
                .flex_col()
                .border_l_1()
                .border_color(hex(Chrome::BORDER))
                .bg(hex(Chrome::PANEL))
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
                        .child(crate::ui::scrollbar(self.plugin_scroll.clone())),
                )
                .into_any_element(),
        )
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
                    .child(div().t_caption().font_weight(FontWeight::SEMIBOLD).text_color(hex(Chrome::MUTED)).child(title.to_uppercase()))
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
                    TextStyle::Title => base.t_title().font_weight(FontWeight::SEMIBOLD).text_color(hex(Chrome::BRIGHT)),
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
            Node::Input { id, .. } => match self.plugin_inputs.get(&(plugin.to_string(), id.clone())) {
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
                    let mut actions = div().flex().items_center().gap_0p5().flex_shrink_0();
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
                                    ),
                            )
                            .children(
                                item.detail.clone().map(|d| div().flex_shrink_0().t_caption().text_color(hex(Chrome::MUTED)).child(d)),
                            )
                            .child(actions.invisible().group_hover(group, |s| s.visible())),
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

    /// Tab-strip buttons of enabled plugins that have a panel.
    pub(super) fn render_plugin_header_buttons(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let entries: Vec<(String, String, &'static str, String)> = plugins::active(cx)
            .filter_map(|(plugin, manifest)| {
                let panel = manifest.contributes.panel.as_ref()?;
                let glyph = icon_named(panel.icon.as_deref().or(manifest.icon.as_deref()));
                let badge = plugins::runtime(cx, &plugin.id).map(|r| r.badge.clone()).unwrap_or_default();
                Some((plugin.id.clone(), panel.title.clone(), glyph, badge))
            })
            .collect();
        entries
            .into_iter()
            .map(|(id, title, glyph, badge)| {
                let open = self.plugin_panel.as_deref() == Some(id.as_str()) && self.page.is_none();
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
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.toggle_plugin_panel(&target, cx)))
                    .child(icon(glyph, IconSize::BUTTON, hex(if open { Chrome::BRIGHT } else { Chrome::FOREGROUND })))
                    .when(!badge.is_empty(), |d| d.child(div().t_caption().text_color(hex(Chrome::BRIGHT)).child(badge)))
                    .into_any_element()
            })
            .collect()
    }

    /// Pane-bar buttons (plugin commands with `paneBar`) for `pane`.
    pub(super) fn render_plugin_pane_buttons(&self, pane: &Pane, cx: &mut Context<Self>) -> Option<AnyElement> {
        let view = pane.read(cx);
        let agent = view.agent_kind().is_some_and(|k| k != PaneKind::Shell);
        let pane_id = view.pane_id;
        let commands: Vec<(String, String, String, &'static str)> = plugins::active(cx)
            .flat_map(|(plugin, manifest)| {
                manifest
                    .contributes
                    .commands
                    .iter()
                    .filter(|c| c.pane_bar)
                    .filter(|c| match c.when {
                        When::Always => true,
                        When::Agent => agent,
                        When::Shell => !agent,
                    })
                    .map(|c| (plugin.id.clone(), c.id.clone(), c.title.clone(), icon_named(c.icon.as_deref().or(manifest.icon.as_deref()))))
                    .collect::<Vec<_>>()
            })
            .collect();
        if commands.is_empty() {
            return None;
        }
        let mut row = div().flex().items_center().gap_0p5().flex_shrink_0();
        for (index, (plugin, command, title, glyph)) in commands.into_iter().enumerate() {
            let target = pane.clone();
            row = row.child(
                div()
                    .id(SharedString::from(format!("pane-plugin-{pane_id}-{index}")))
                    .size(px(22.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_sm()
                    .cursor_pointer()
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .tooltip(Tooltip::text(title, None))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        this.run_plugin_command(&plugin, &command, Some(target.clone()), cx);
                    }))
                    .child(icon(glyph, IconSize::INLINE, hex(Chrome::FOREGROUND))),
            );
        }
        Some(row.into_any_element())
    }
}

fn icon_only_close(cx: &mut Context<Workbench>) -> impl IntoElement {
    crate::ui::icon_only("plugin-panel-close", "x", cx.listener(|this, _: &ClickEvent, _, cx| this.close_plugin_panel(cx)))
}
