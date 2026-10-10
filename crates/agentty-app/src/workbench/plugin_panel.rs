//! Plugin UI inside the window: the panel docked right of the terminals (the area a plugin fills
//! with its own UI tree) and plugin buttons in the tab strip.

use super::Workbench;
use crate::i18n::{t, tf};
use crate::plugins::{self, RunState};
use crate::settings::settings;
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, icon_named, IconSize, Tooltip, TypeScale};
use agentty_bridge::plugins::manifest::{PanelMode, Surface};
use agentty_bridge::plugins::ui::{Node, UiEvent};
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, Entity, Focusable, SharedString, Subscription, Window};
use std::time::Duration;

/// One plugin's entry on a surface: which plugin, what to call it, and what to draw for it.
/// From this many sidebar plugins on, those not pinned share one activity-bar item.
const PLUGIN_GROUP_FROM: usize = 3;

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
    /// The value in the plugin's last tree. A tree that carries the same one again was drawn for
    /// some other reason (a status ticking) and says nothing about this field.
    from_plugin: String,
    generation: u64,
    /// Enter was pressed and the plugin has not answered yet.
    submitted: bool,
    /// What the plugin suggests for the word being typed.
    pub completions: Vec<agentty_bridge::plugins::ui::Completion>,
    /// The suggestions on screen under the field.
    pub suggest: Option<Suggest>,
    _subscription: Subscription,
}

/// Suggestions shown under a field: the range they replace, which of the plugin's completions fit
/// (best first), and the one ↑ ↓ are on.
pub struct Suggest {
    pub range: std::ops::Range<usize>,
    pub items: Vec<usize>,
    pub selected: usize,
}

/// Rows of suggestions shown at once.
pub(super) const MAX_SUGGESTIONS: usize = 8;

/// The suggestions for a field showing `text` with the caret at `cursor`.
fn suggest_for(completions: &[agentty_bridge::plugins::ui::Completion], text: &str, cursor: usize) -> Option<Suggest> {
    use agentty_bridge::plugins::ui::{completion_word, matching_completions};
    if completions.is_empty() {
        return None;
    }
    let (range, word) = completion_word(text, cursor)?;
    let braced = text[range.clone()].starts_with("{{");
    let found = matching_completions(completions, word, braced);
    let items: Vec<usize> =
        found.iter().take(MAX_SUGGESTIONS).filter_map(|c| completions.iter().position(|o| std::ptr::eq(o, *c))).collect();
    (!items.is_empty()).then_some(Suggest { range, items, selected: 0 })
}

/// A popover's card: wide enough for a step's settings, and scrolling past this height.
pub(super) const POPOVER_WIDTH: f32 = 380.;
const POPOVER_MAX_HEIGHT: f32 = 640.;
/// The window above a popover's body and a margin under it: title bar, panel header, the card's
/// own heading.
const POPOVER_TOP_ROOM: f32 = 200.;

/// A tree arrived with `value` for a field showing `typed`: whether that text is replaced by it.
/// `applied` is what the plugin has seen or set (typing waiting for the pause before it is sent
/// differs from it), `from_plugin` the value of the tree before, `submitted` that Enter or a
/// flushed control event went out since.
fn sync_field(applied: &mut String, from_plugin: &mut String, submitted: &mut bool, typed: &str, value: &str, focused: bool) -> bool {
    let changed = *from_plugin != value;
    *from_plugin = value.to_string();
    if typed == value {
        // What is on screen is what the plugin has: nothing waits to be sent.
        *applied = value.to_string();
        return false;
    }
    // Typing the plugin has not been sent yet: its answer is still to come, and only a value it
    // changed meanwhile is looked at.
    let pending = *applied != typed;
    if !changed && pending {
        return false;
    }
    let was_submitted = std::mem::take(submitted);
    let apply = should_apply(typed, value, focused, was_submitted, changed);
    if apply {
        *applied = value.to_string();
    }
    apply
}

/// Whether a value the plugin sent for an input should replace what is on screen.
///
/// A field the user is not in always shows the plugin's value. In the field they are typing in,
/// only a value the plugin `changed` since its last tree counts: a plugin redraws its panel for
/// many reasons (a status, a countdown) and each tree carries the value it had before it read the
/// latest keystrokes — applying that one put back a number the user had just deleted, or a name
/// they were halfway through retyping. A changed value is still held back when what is typed runs
/// ahead of it (an older echo catching up), unless it is empty right after a submit (Enter) or a
/// flushed control event (a button, list action, choice or toggle) — the plugin clearing the field
/// it just read.
fn should_apply(typed: &str, value: &str, focused: bool, submitted: bool, changed: bool) -> bool {
    if typed == value {
        return false;
    }
    if !focused || submitted && value.is_empty() {
        return true;
    }
    changed && !typed.starts_with(value)
}

impl Workbench {
    /// Creates and syncs the panel's text fields; called from render before drawing.
    /// Whose fields the panel shows: an automation's are its own (the same id in another tab is
    /// another field), so they are made and looked up under the automation in front.
    pub(super) fn plugin_input_scope(&self, plugin: &str, cx: &gpui::App) -> String {
        match self.active_instance(plugin, cx) {
            Some(instance) => format!("{plugin}#{instance}"),
            None => plugin.to_string(),
        }
    }

    pub(super) fn prepare_plugin_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.plugin_window_height.set(f32::from(window.viewport_size().height));
        // Drawn again below: a slot gone from the panel takes no more drops.
        self.plugin_drop_zones.borrow_mut().clear();
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
        let mut shows_code = false;
        if let Some(tree) = self.plugin_tree(&plugin, cx) {
            tree.inputs(&mut fields);
            shows_code = super::plugin_ui::has_code(tree);
        }
        if shows_code {
            self.load_plugin_grammars(cx);
        }
        let scope = self.plugin_input_scope(&plugin, cx);
        self.plugin_inputs.retain(|(owner, id), _| *owner == scope && fields.iter().any(|field| field.id == *id));
        for agentty_bridge::plugins::ui::InputField { id, placeholder, value, rows, completions } in fields {
            let key = (scope.clone(), id.clone());
            if let Some(existing) = self.plugin_inputs.get_mut(&key) {
                existing.completions = completions;
                existing.input.update(cx, |i, cx| i.set_placeholder(placeholder.clone(), cx));
                let input = existing.input.clone();
                let typed = input.read(cx).text().to_string();
                let focused = input.focus_handle(cx).is_focused(window);
                if sync_field(&mut existing.applied, &mut existing.from_plugin, &mut existing.submitted, &typed, &value, focused) {
                    input.update(cx, |i, cx| i.set_text(value, cx));
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
                // Suggestions open take the keys that move through and pick them.
                let suggesting = this.plugin_inputs.get(&key).is_some_and(|f| f.suggest.is_some());
                match event {
                    TextInputEvent::Up | TextInputEvent::Down if suggesting => {
                        if let Some(suggest) = this.plugin_inputs.get_mut(&key).and_then(|f| f.suggest.as_mut()) {
                            let n = suggest.items.len();
                            suggest.selected = if matches!(event, TextInputEvent::Up) {
                                (suggest.selected + n - 1) % n
                            } else {
                                (suggest.selected + 1) % n
                            };
                        }
                        cx.notify();
                        return;
                    }
                    TextInputEvent::Confirmed | TextInputEvent::Next if suggesting => {
                        let selected = this.plugin_inputs.get(&key).and_then(|f| f.suggest.as_ref()).map_or(0, |s| s.selected);
                        this.apply_plugin_completion(&key, selected, cx);
                        return;
                    }
                    TextInputEvent::Cancelled if suggesting => {
                        if let Some(field) = this.plugin_inputs.get_mut(&key) {
                            field.suggest = None;
                        }
                        cx.notify();
                        return;
                    }
                    TextInputEvent::Blurred if suggesting => {
                        // Later, so a click on a suggestion (which takes the focus) still lands.
                        let key = key.clone();
                        cx.spawn(async move |this, cx| {
                            cx.background_executor().timer(Duration::from_millis(200)).await;
                            let _ = this.update(cx, |this, cx| {
                                if let Some(field) = this.plugin_inputs.get_mut(&key) {
                                    field.suggest = None;
                                }
                                cx.notify();
                            });
                        })
                        .detach();
                        return;
                    }
                    _ => {}
                }
                if matches!(event, TextInputEvent::Changed) {
                    let cursor = input.read(cx).cursor();
                    if let Some(field) = this.plugin_inputs.get_mut(&key) {
                        // The plugin putting its value in (another tab picked) is not typing.
                        field.suggest = if text == field.from_plugin { None } else { suggest_for(&field.completions, &text, cursor) };
                    }
                }
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
            self.plugin_inputs.insert(
                key,
                PluginInput {
                    input,
                    from_plugin: value.clone(),
                    applied: value,
                    generation: 0,
                    submitted: false,
                    completions,
                    suggest: None,
                    _subscription: subscription,
                },
            );
        }
    }

    /// Puts suggestion `index` (of those on screen) into the field, in place of the word typed.
    pub(super) fn apply_plugin_completion(&mut self, key: &(String, String), index: usize, cx: &mut Context<Self>) {
        let Some(field) = self.plugin_inputs.get_mut(key) else { return };
        let Some(suggest) = field.suggest.take() else { return };
        let Some(completion) = suggest.items.get(index).and_then(|i| field.completions.get(*i)) else { return };
        let text = completion.insert.clone().unwrap_or_else(|| completion.label.clone());
        let input = field.input.clone();
        input.update(cx, |i, cx| i.replace_range(suggest.range, &text, cx));
        cx.notify();
    }

    /// Sends the plugin any typing it has not seen yet, right before a button, list action,
    /// choice or toggle of the same panel reaches it — so it acts on what is on screen rather
    /// than on a debounced `change` that has not gone out yet (typing pauses 250ms before it
    /// sends). Every input of the panel is marked `submitted` regardless, so an empty value the
    /// plugin answers with right after is applied as a clear rather than held back as a stale
    /// echo of what the user is still typing.
    pub(super) fn flush_plugin_inputs(&mut self, plugin: &str, cx: &mut Context<Self>) {
        let scope = self.plugin_input_scope(plugin, cx);
        let keys: Vec<(String, String)> = self.plugin_inputs.keys().filter(|(owner, _)| *owner == scope).cloned().collect();
        let mut to_send = Vec::new();
        for key in &keys {
            let Some(field) = self.plugin_inputs.get_mut(key) else { continue };
            let text = field.input.read(cx).text().to_string();
            field.submitted = true;
            if field.applied != text {
                field.generation += 1;
                field.applied = text.clone();
                to_send.push((key.1.clone(), text));
            }
        }
        for (element, text) in to_send {
            let event = UiEvent { element, event: "change".into(), value: Some(text.into()), item: None, action: None };
            self.send_plugin_event(plugin, event, cx);
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
        let window_id = plugin_id.clone();
        // A plugin with a workspace of its own can have a window of its own too.
        let pop_out = self.wants_workspace(&plugin_id, cx).then(|| {
            crate::ui::icon_only(
                "plugin-panel-window",
                "external-link",
                cx.listener(move |_, _: &ClickEvent, _, cx| crate::open_plugin_window(window_id.clone(), cx)),
            )
            .tooltip(Tooltip::text(t(cx, "plugins.open_window"), None))
        });
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
            .child(crate::ui::plugin_mark(panel_logo.clone(), panel_icon, IconSize::BUTTON, hex(Chrome::BRIGHT)))
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
            .children(pop_out)
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
        let popover = match (&tree, &state) {
            (Some(tree), RunState::Running) if !no_automation => tree.popover().cloned(),
            _ => None,
        }
        .map(|popover| self.render_plugin_popover(&plugin_id, &popover, cx));

        // The empty state renders only when no earlier arm (consent, failure) claims the body.
        let shows_empty = no_automation && !matches!(state, RunState::NeedsConsent | RunState::Failed(_));
        // A tree that takes the panel's height scrolls inside itself, column by column; the empty
        // state fills it too, so it can sit centred. A failure's logs or a consent notice stay in
        // the scrolling branch, so they never overflow out of reach.
        let fills_panel = shows_empty || (matches!(state, RunState::Running) && tree.as_ref().is_some_and(|t| t.fills_height()));
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
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_4()
                    .p_6()
                    .child(
                        // The plugin's own mark, dimmed, in a soft tile — the same badge for every plugin.
                        div()
                            .size(px(60.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_xl()
                            .bg(hex_alpha(Chrome::BRIGHT, 0.05))
                            .border_1()
                            .border_color(hex(Chrome::BORDER))
                            .child(crate::ui::plugin_mark(panel_logo, panel_icon, IconSize::ACTIVITY, hex(Chrome::MUTED))),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .t_large()
                                    .font_weight(crate::theme::EMPHASIS)
                                    .text_color(hex(Chrome::BRIGHT))
                                    .child(t(cx, "plugins.no_automation")),
                            )
                            .child(
                                div()
                                    .max_w(px(260.))
                                    .text_center()
                                    .t_small()
                                    .text_color(hex(Chrome::MUTED))
                                    .child(t(cx, "plugins.no_automation.hint")),
                            ),
                    )
                    .child(
                        // Primary call to action: start a fresh one.
                        div()
                            .id("plugin-new-automation")
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .px_3()
                            .py_1p5()
                            .rounded_md()
                            .cursor_pointer()
                            .t_small()
                            .font_weight(crate::theme::EMPHASIS)
                            .bg(hex(Chrome::ACCENT))
                            .text_color(hex(Chrome::BRIGHT))
                            .hover(|s| s.bg(hex_alpha(Chrome::ACCENT, 0.82)))
                            .child(icon("plus", IconSize::INLINE, hex(Chrome::BRIGHT)))
                            .child(t(cx, "plugins.new_automation"))
                            .on_click(cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                                // A double click is one instance, not two.
                                if this.workspaces.get(this.active_workspace).is_some_and(|ws| ws.tabs.is_empty()) {
                                    this.new_plugin_instance(&owner, cx);
                                }
                            })),
                    )
                    .into_any_element()
            }
            (Some(tree), _) => {
                let mut path = Vec::new();
                let fills = tree.fills_height();
                div()
                    .p_3()
                    .when(fills, |d| d.flex_1().min_h_0().flex().flex_col())
                    .child(self.render_plugin_node(&plugin_id, &tree, &mut path, cx))
                    .into_any_element()
            }
            (None, _) => crate::ui::loading_row(t(cx, "plugins.starting")).into_any_element(),
        };

        let mut body_slot = Some(body);
        Some(
            div()
                .size_full()
                .flex()
                .flex_col()
                .bg(hex(Chrome::PANEL))
                .relative()
                .child(header)
                .when(fills_panel, |d| {
                    d.child(div().flex_1().min_h_0().flex().flex_col().child(body_slot.take().unwrap_or_else(|| div().into_any_element())))
                })
                .when(!fills_panel, |d| {
                    d.child(
                        div()
                            .id("plugin-panel-scroll")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .track_scroll(&self.plugin_scroll)
                            .relative()
                            .child(body_slot.take().unwrap_or_else(|| div().into_any_element()))
                            .group(crate::ui::SCROLL_GROUP)
                            .child(crate::ui::scrollbar(self.plugin_scroll.clone())),
                    )
                })
                // Last, so it paints over the panel's body instead of under it.
                .children(mode_menu)
                .children(popover)
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
                .w(px(self.plugin_overlay_width(cx)))
                .flex()
                .shadow_lg()
                // Floating over the page: a click on the panel is the panel's, never the page's.
                .occlude()
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

    /// A popover of the panel: a card at the panel's right edge, over the page beside it (which is
    /// hidden meanwhile, see `overlay_open`).
    fn render_plugin_popover(&self, plugin: &str, node: &Node, cx: &mut Context<Self>) -> AnyElement {
        let Node::Popover { id, title, children } = node else { return div().into_any_element() };
        let (owner, element) = (plugin.to_string(), id.clone());
        // Full width: a text field inside takes the card's width, not its own (none).
        let mut body = div().w_full().flex().flex_col().gap_2().min_w_0();
        let mut path = vec![usize::MAX];
        for (index, child) in children.iter().enumerate() {
            path.push(index);
            body = body.child(self.render_plugin_node(plugin, child, &mut path, cx));
            path.pop();
        }
        let card = div()
            .ml_2()
            .w(px(POPOVER_WIDTH))
            .flex()
            .flex_col()
            .rounded_lg()
            .border_1()
            .border_color(hex(Chrome::OVERLAY_BORDER))
            .bg(hex(Chrome::PANEL))
            .shadow_lg()
            .occlude()
            .child(
                div()
                    .h(px(36.))
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(hex(Chrome::BORDER))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .t_body()
                            .font_weight(crate::theme::EMPHASIS)
                            .text_color(hex(Chrome::BRIGHT))
                            .child(title.clone()),
                    )
                    .child(crate::ui::icon_only(
                        SharedString::from(format!("plugin-popover-close-{plugin}-{id}")),
                        "x",
                        cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.flush_plugin_inputs(&owner, cx);
                            let event = UiEvent { element: element.clone(), event: "close".into(), value: None, item: None, action: None };
                            this.send_plugin_event(&owner, event, cx);
                        }),
                    )),
            )
            .child(
                div()
                    .id(SharedString::from(format!("plugin-popover-body-{plugin}-{id}")))
                    .w_full()
                    // What fits under the panel's header in this window, and scrolls past that.
                    .max_h(px((self.plugin_window_height.get() - POPOVER_TOP_ROOM).clamp(160., POPOVER_MAX_HEIGHT)))
                    .overflow_y_scroll()
                    .p_3()
                    // A column, so the body is the card's width: a long line in it (a code
                    // snippet) scrolls inside its block instead of being cut at the card's edge.
                    .flex()
                    .flex_col()
                    .child(body.flex_shrink_0()),
            );
        let anchored = gpui::anchored().anchor(gpui::Corner::TopLeft).snap_to_window_with_margin(px(8.)).child(card);
        // Not deferred: it is the panel's last child, so it is drawn over the page already, and a
        // drop-down inside it can be deferred (over the card) without nesting deferred drawing.
        div().absolute().top(px(44.)).right_0().child(anchored).into_any_element()
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
    /// Agentty's own items, and the bar scrolls once there are more than fit. From
    /// [`PLUGIN_GROUP_FROM`] plugins on, those not pinned fold into one group item whose list opens
    /// to the right; pinned ones keep an item of their own however many there are.
    pub(super) fn render_plugin_activity_items(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let entries = self.plugin_surface_entries(Surface::Sidebar, cx);
        if entries.len() < PLUGIN_GROUP_FROM {
            return entries.into_iter().map(|entry| self.render_plugin_activity_item(entry, cx)).collect();
        }
        let pinned = settings(cx).pinned_plugins.clone();
        let (shown, grouped): (Vec<_>, Vec<_>) = entries.into_iter().partition(|entry| pinned.contains(&entry.id));
        let group_open = grouped.iter().any(|entry| self.plugin_panel_open(&entry.id));
        let group_badge = grouped.iter().any(|entry| !entry.badge.is_empty());
        let mut items: Vec<AnyElement> = shown.into_iter().map(|entry| self.render_plugin_activity_item(entry, cx)).collect();
        items.push(self.render_plugin_group_item(group_open, group_badge, cx));
        items
    }

    fn render_plugin_activity_item(
        &self,
        SurfaceEntry { id, title, glyph, logo, badge }: SurfaceEntry,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.plugin_panel_open(&id);
        let target = id.clone();
        let menu_id = id.clone();
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
            .on_mouse_down(
                gpui::MouseButton::Right,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    this.plugin_pin_menu = Some((menu_id.clone(), event.position));
                    cx.notify();
                }),
            )
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
            .children(self.render_plugin_pin_menu(&id, cx))
            .into_any_element()
    }

    /// The right-click menu of a plugin's activity-bar item: pin it there, or let it go back into
    /// the group.
    fn render_plugin_pin_menu(&self, id: &str, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (_, position) = self.plugin_pin_menu.as_ref().filter(|(menu, _)| menu == id)?;
        let pinned = settings(cx).pinned_plugins.iter().any(|p| p == id);
        let target = id.to_string();
        let menu = crate::ui::popover()
            .min_w(px(200.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.plugin_pin_menu = None;
                cx.notify();
            }))
            .child(crate::ui::menu_item(
                SharedString::from(format!("activity-plugin-pin-{id}")),
                t(cx, if pinned { "plugins.unpin" } else { "plugins.pin" }),
                cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.plugin_pin_menu = None;
                    toggle_plugin_pin(&target, cx);
                    cx.notify();
                }),
            ));
        let anchored = gpui::anchored().position(*position).snap_to_window_with_margin(px(8.)).child(menu);
        Some(gpui::deferred(anchored).with_priority(3).into_any_element())
    }

    /// The item standing for the sidebar plugins not pinned: lit while one of their panels is open,
    /// a dot while one of them has a badge, and the list of all of them beside it once clicked.
    fn render_plugin_group_item(&self, open: bool, badge: bool, cx: &mut Context<Self>) -> AnyElement {
        const ID: &str = "activity-plugin-group";
        let lit = open || self.plugin_group_menu;
        div()
            .id(ID)
            .group(ID)
            .tooltip(Tooltip::text(t(cx, "plugins.group"), None))
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
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                // The click that closed the list from outside must not open it again.
                if !this.just_dismissed(ID) {
                    this.plugin_group_menu = !this.plugin_group_menu;
                }
                cx.notify();
            }))
            .child(
                icon("blocks", IconSize::ACTIVITY, if lit { hex(Chrome::BRIGHT) } else { hex(0x858585) })
                    .group_hover(ID, |s| s.text_color(hex(Chrome::BRIGHT))),
            )
            .when(badge, |d| d.child(div().absolute().bottom(px(10.)).right(px(10.)).size(px(7.)).rounded_full().bg(hex(Chrome::ACCENT))))
            .when(self.plugin_group_menu, |d| d.child(self.render_plugin_group_menu(cx)))
            .into_any_element()
    }

    /// Every sidebar plugin, pinned ones first, each with its pin: a click opens its panel.
    fn render_plugin_group_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let pinned = settings(cx).pinned_plugins.clone();
        let mut entries = self.plugin_surface_entries(Surface::Sidebar, cx);
        // Stable: within pinned and not pinned the order stays the activity bar's.
        entries.sort_by_key(|entry| !pinned.contains(&entry.id));
        let rows = entries.into_iter().map(|SurfaceEntry { id, title, glyph, logo, badge }| {
            let open = self.plugin_panel_open(&id);
            let is_pinned = pinned.contains(&id);
            let target = id.clone();
            let pin_id = id.clone();
            let pin = div()
                .id(SharedString::from(format!("plugin-group-pin-{id}")))
                .tooltip(Tooltip::text(t(cx, if is_pinned { "plugins.unpin" } else { "plugins.pin" }), None))
                .flex_shrink_0()
                .size(px(22.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_md()
                .cursor_pointer()
                .hover(|s| s.bg(hex(Chrome::HOVER)))
                .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                    // The pin only pins: the row's own click (open the panel) stays out of it.
                    cx.stop_propagation();
                    toggle_plugin_pin(&pin_id, cx);
                }))
                // Unpinned rows show their pin on hover only, so the list reads as plugins first.
                .child(
                    icon("pin", IconSize::INLINE, hex(if is_pinned { Chrome::BRIGHT } else { Chrome::MUTED }))
                        .when(!is_pinned, |i| i.invisible().group_hover(format!("plugin-group-row-{id}"), |s| s.visible())),
                );
            div()
                .id(SharedString::from(format!("plugin-group-row-{id}")))
                .group(format!("plugin-group-row-{id}"))
                .h(px(30.))
                .px_2()
                .flex()
                .items_center()
                .gap_2()
                .rounded_md()
                .cursor_pointer()
                .t_small()
                .when(open, |d| d.bg(hex(Chrome::SELECTED)).text_color(hex(Chrome::BRIGHT)))
                .hover(|s| s.bg(hex(Chrome::HOVER)))
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.plugin_group_menu = false;
                    this.toggle_plugin_surface(&target, window, cx);
                    cx.notify();
                }))
                .child(crate::ui::plugin_mark(logo, glyph, IconSize::BUTTON, hex(if open { Chrome::BRIGHT } else { Chrome::FOREGROUND })))
                .child(div().flex_1().min_w_0().truncate().child(title))
                .when(!badge.is_empty(), |d| {
                    d.child(
                        div()
                            .flex_shrink_0()
                            .px_1()
                            .rounded_sm()
                            .bg(hex(Chrome::ACCENT))
                            .t_caption()
                            .text_color(hex(Chrome::BRIGHT))
                            .child(badge),
                    )
                })
                .child(pin)
        });
        let menu = crate::ui::popover()
            .w(px(240.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.plugin_group_menu = false;
                this.note_dismissed("activity-plugin-group");
                cx.notify();
            }))
            .child(div().px_2().pt_1().pb_1p5().t_caption().text_color(hex(Chrome::MUTED)).child(t(cx, "plugins.group")))
            .children(rows);
        // Right of the bar, level with the item: deferred, so the scrolling bar does not clip it.
        let anchored = gpui::anchored().anchor(gpui::Corner::TopLeft).snap_to_window_with_margin(px(8.)).child(menu);
        div()
            .absolute()
            .top_0()
            .left(px(super::chrome::ACTIVITY_BAR_WIDTH + 4.))
            .child(gpui::deferred(crate::ui::fade_in("plugin-group-fade", anchored)).with_priority(3))
            .into_any_element()
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

/// After the upgrade that brought the plugin group: pins the sidebar plugins installed then, so
/// each keeps its own activity-bar item. Once, right after the plugins are loaded.
pub fn pin_installed_sidebar_plugins(cx: &mut gpui::App) {
    if !settings(cx).pin_sidebar_plugins {
        return;
    }
    let sidebar: Vec<String> = plugins::active(cx)
        .filter(|(_, manifest)| manifest.contributes.panel.is_some() && manifest.surface() == Surface::Sidebar)
        .map(|(plugin, _)| plugin.id.clone())
        .collect();
    crate::settings::update_settings(cx, move |s| {
        s.pin_sidebar_plugins = false;
        if s.pinned_plugins.is_empty() {
            s.pinned_plugins = sidebar;
        }
    });
}

/// Pins a sidebar plugin to the activity bar, or unpins it.
fn toggle_plugin_pin(id: &str, cx: &mut gpui::App) {
    let id = id.to_string();
    crate::settings::update_settings(cx, move |s| {
        if let Some(at) = s.pinned_plugins.iter().position(|p| *p == id) {
            s.pinned_plugins.remove(at);
        } else {
            s.pinned_plugins.push(id);
        }
    });
}

fn icon_only_close(cx: &mut Context<Workbench>) -> impl IntoElement {
    crate::ui::icon_only(
        "plugin-panel-close",
        "x",
        cx.listener(|this, _: &ClickEvent, _, cx| {
            // In a plugin's workspace the panel is the workspace's: closing it leaves the workspace
            // (the plugin runs on), or it would come straight back.
            if this.front_plugin_workspace(cx).is_some() {
                this.leave_plugin_workspace(cx);
            } else {
                this.close_plugin_panel(cx);
            }
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::should_apply;

    #[test]
    fn a_field_not_focused_always_takes_the_plugins_value() {
        assert!(should_apply("kim", "", false, false, false));
        assert!(should_apply("kim", "lee", false, false, false));
        // Left with a number the plugin would not take: it shows the one the plugin kept.
        assert!(should_apply("", "10", false, false, false));
    }

    #[test]
    fn typing_ahead_of_an_empty_echo_is_not_undone() {
        // The user typed "kim" and the plugin has not read it yet (still empty from before).
        assert!(!should_apply("kim", "", true, false, false));
        assert!(!should_apply("kim", "", true, false, true));
    }

    #[test]
    fn an_empty_value_right_after_a_submit_or_flush_is_the_plugins_clear() {
        // Enter, or a button click that flushed the field first: the plugin read what was typed
        // and answered with an empty value — that is the field being cleared, not a stale echo.
        assert!(should_apply("kim", "", true, true, true));
    }

    #[test]
    fn a_non_empty_echo_while_focused_is_still_stale() {
        // Even marked submitted, a non-empty value that is a prefix match of what's typed now is
        // an old echo catching up, not something the plugin means to set.
        assert!(!should_apply("kim", "ki", true, true, true));
    }

    #[test]
    fn a_value_the_plugin_changed_to_something_else_is_applied() {
        // The plugin changed the field to something the user did not type (not a prefix at all):
        // an update of its own, not an echo.
        assert!(should_apply("kim", "lee", true, false, true));
    }

    /// One field, the way the panel keeps it: what is on screen, what the plugin has, the last tree.
    struct Field {
        typed: String,
        applied: String,
        from_plugin: String,
        submitted: bool,
        focused: bool,
    }

    impl Field {
        fn new(value: &str) -> Self {
            Field { typed: value.into(), applied: value.into(), from_plugin: value.into(), submitted: false, focused: true }
        }
        /// A keystroke: on screen, not sent yet.
        fn typing(&mut self, text: &str) {
            self.typed = text.into();
        }
        /// The pause after typing: the plugin gets a `change` with what is on screen.
        fn sent(&mut self) -> String {
            self.applied = self.typed.clone();
            self.typed.clone()
        }
        /// A tree from the plugin with this field's value.
        fn tree(&mut self, value: &str) {
            if super::sync_field(&mut self.applied, &mut self.from_plugin, &mut self.submitted, &self.typed, value, self.focused) {
                self.typed = value.into();
            }
        }
    }

    /// A count the plugin keeps between 1 and 100: empty or not a number keeps the one set.
    fn count(text: &str, kept: &str) -> String {
        match text.trim().parse::<i64>() {
            Ok(n) if n >= 1 => n.min(100).to_string(),
            _ => kept.to_string(),
        }
    }

    /// The recording that found the bug: a count of 10 in a plugin that redraws every second.
    /// Deleting the "0", clearing the field, typing a new number — each came back as "10".
    #[test]
    fn a_count_can_be_retyped_while_the_plugin_keeps_redrawing() {
        let mut kept = "10".to_string();
        let mut field = Field::new("10");
        // Backspace: "1". A redraw from before the plugin read it still says "10".
        field.typing("1");
        field.tree(&kept);
        assert_eq!(field.typed, "1");
        let change = field.sent();
        field.tree(&kept); // drawn before the change arrived
        assert_eq!(field.typed, "1");
        kept = count(&change, &kept);
        field.tree(&kept);
        assert_eq!((field.typed.as_str(), kept.as_str()), ("1", "1"));
        // "15", with redraws of "1" in between.
        field.typing("15");
        field.tree(&kept);
        assert_eq!(field.typed, "15");
        kept = count(&field.sent(), &kept);
        field.tree(&kept);
        assert_eq!((field.typed.as_str(), kept.as_str()), ("15", "15"));

        // Cleared to type another: the plugin keeps 15 meanwhile, and the field stays empty.
        field.typing("");
        field.tree(&kept);
        kept = count(&field.sent(), &kept);
        for _ in 0..5 {
            field.tree(&kept);
        }
        assert_eq!((field.typed.as_str(), kept.as_str()), ("", "15"));
        field.typing("7");
        field.tree(&kept);
        kept = count(&field.sent(), &kept);
        field.tree(&kept);
        assert_eq!((field.typed.as_str(), kept.as_str()), ("7", "7"));

        // Over the most: the plugin makes it 100 and the field shows what was kept.
        field.typing("106");
        field.tree(&kept);
        assert_eq!(field.typed, "106");
        kept = count(&field.sent(), &kept);
        field.tree(&kept);
        assert_eq!((field.typed.as_str(), kept.as_str()), ("100", "100"));

        // Left empty: once the user is out of the field, it shows the number that counts.
        field.typing("");
        kept = count(&field.sent(), &kept);
        field.tree(&kept);
        assert_eq!(field.typed, "");
        field.focused = false;
        field.tree(&kept);
        assert_eq!(field.typed, "100");
    }

    /// The automation's name (its tab's name), retyped from scratch while the panel redraws.
    #[test]
    fn a_name_can_be_retyped_while_the_plugin_keeps_redrawing() {
        let mut kept = "자동화 2".to_string();
        let mut field = Field::new(&kept);
        for typed in ["", "M", "My", "My ", "My b", "My bo", "My bot"] {
            field.typing(typed);
            field.tree(&kept);
            assert_eq!(field.typed, typed, "undone while typing {typed:?}");
            // Every other keystroke, a pause: the plugin takes it (an empty name keeps the old one).
            if typed.len() % 2 == 0 {
                let sent = field.sent();
                field.tree(&kept);
                if !sent.trim().is_empty() {
                    kept = sent;
                }
                field.tree(&kept);
                assert_eq!(field.typed, typed);
            }
        }
        kept = field.sent();
        field.tree(&kept);
        field.focused = false;
        field.tree(&kept);
        assert_eq!((field.typed.as_str(), kept.as_str()), ("My bot", "My bot"));
    }

    /// What the plugin does on its own still reaches the field: a value it changes, and the clear
    /// after Enter.
    #[test]
    fn the_plugins_own_changes_still_reach_the_field() {
        let mut field = Field::new("kim");
        field.tree("lee");
        assert_eq!(field.typed, "lee");
        field.typing("note");
        field.sent();
        field.submitted = true; // Enter
        field.tree("");
        assert_eq!(field.typed, "");
        // A field the user is not in follows the plugin whatever it says.
        field.focused = false;
        field.tree("from the plugin");
        assert_eq!(field.typed, "from the plugin");
    }

    /// The count field of a plugin that redraws every second: "10", the user deletes the "0" or
    /// the whole number to type another, and trees keep coming with "10" until the plugin reads
    /// the change. None of them may put "10" back while the field is in use.
    #[test]
    fn a_redraw_with_the_old_value_does_not_undo_typing() {
        assert!(!should_apply("1", "10", true, false, false));
        assert!(!should_apply("", "10", true, false, false));
        assert!(!should_apply("106", "10", true, false, false));
        // A name retyped from scratch, same thing.
        assert!(!should_apply("My au", "자동화 2", true, false, false));
        // Then the plugin takes it: its value is what is typed, nothing to do.
        assert!(!should_apply("5", "5", true, false, true));
    }
}
