//! The elements of a plugin's panel, drawn from the tree it sent (`ui/setPanel`).
//!
//! Plugins name what they want (a card, a table, a tone) and never a color or a size: every
//! measure here comes from the few constants below, so all plugins look like one app and like
//! Agentty itself.

use super::Workbench;
use crate::editor::highlight;
use crate::editor::language::Language;
use crate::i18n::t;
use crate::theme::{hex, hex_alpha, lighten, Chrome};
use crate::ui::{icon, icon_named, IconSize, Tooltip, TypeScale};
use agentty_bridge::plugins::ui::{Align, FlowState, Gap, GridWidth, Node, TextStyle, Tone, UiEvent, Variant};
use gpui::{div, prelude::*, px, relative, AnyElement, ClickEvent, Context, Div, HighlightStyle, SharedString, StyledText, Window};
use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Behind fields, code and the track of a bar: a step darker than the panel.
const SUNKEN: u32 = 0x1a1a1d;
/// Cards and stats: a step lighter than the panel.
const RAISED: u32 = 0x27272b;
/// Secondary buttons.
const CONTROL: u32 = 0x2e2e33;
/// Height of a button, a select and a one-line field, so a row of them lines up.
const CONTROL_HEIGHT: f32 = 28.;
const MONO: &str = "JetBrains Mono";

/// A `flow` step's icon box, and where the line joining the steps runs: under its middle.
const FLOW_ICON: f32 = 26.;
const FLOW_LINE_LEFT: f32 = 8. + FLOW_ICON / 2. - 1.;
const FLOW_SIDE_INDENT: f32 = 28.;
/// A list row's icon box.
const ROW_ICON: f32 = 24.;
/// Lines of a `code` block that are colored; the rest is shown plain.
const MAX_COLORED_LINES: usize = 400;
/// `code` blocks whose colors are kept.
const MAX_COLORED_BLOCKS: usize = 64;

/// Colored stretches of a `code` block (byte range, color), by a hash of its text and language.
pub type CodeColors = RefCell<HashMap<u64, Rc<Vec<(Range<usize>, u32)>>>>;

fn gap(gap: Gap) -> gpui::Pixels {
    px(match gap {
        Gap::None => 0.,
        Gap::Small => 4.,
        Gap::Medium => 8.,
        Gap::Large => 16.,
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

/// The icon a callout shows when the plugin names none.
fn tone_icon(tone: Tone) -> &'static str {
    match tone {
        Tone::Neutral => "lightbulb",
        Tone::Info => "info",
        Tone::Success => "circle-check",
        Tone::Warning => "shield-alert",
        Tone::Error => "circle-x",
    }
}

/// An icon in a small tinted square: list rows, cards, flow steps.
fn icon_tile(name: Option<&str>, size: f32, color: u32) -> Div {
    div().size(px(size)).flex_shrink_0().flex().items_center().justify_center().rounded_md().bg(hex_alpha(color, 0.16)).child(icon(
        icon_named(name),
        IconSize::INLINE,
        hex(color),
    ))
}

/// Elements that read as a block: in a `row` they share the width instead of shrinking to their
/// narrowest. Text left to its own size wraps one character a line for Korean, Japanese and Chinese.
fn fills_row(node: &Node) -> bool {
    matches!(
        node,
        Node::Text { .. }
            | Node::Input { .. }
            | Node::Select { .. }
            | Node::Card { .. }
            | Node::Stat { .. }
            | Node::Progress { .. }
            | Node::Callout { .. }
            | Node::KeyValue { .. }
            | Node::Table { .. }
            | Node::Code { .. }
            | Node::Grid { .. }
    )
}

/// Whether the tree shows a `code` block (and so needs the grammars).
pub(super) fn has_code(node: &Node) -> bool {
    matches!(node, Node::Code { .. }) || node.children().iter().any(has_code)
}

/// The extension a `code` block's language reads as, for finding its grammar.
fn code_extension(language: &str) -> String {
    let language = language.trim().trim_start_matches('.').to_ascii_lowercase();
    match language.as_str() {
        "rust" => "rs",
        "typescript" => "ts",
        "javascript" | "node" => "js",
        "python" => "py",
        "shell" | "bash" | "zsh" | "console" => "sh",
        "yml" => "yaml",
        "markdown" => "md",
        "golang" => "go",
        "kotlin" => "kt",
        "dockerfile" => "Dockerfile",
        other => other,
    }
    .to_string()
}

/// Sends a plugin an event from one of its controls, after any typing it has not seen yet.
fn emit(
    owner: String,
    element: String,
    event: &'static str,
    value: Option<serde_json::Value>,
    item: Option<String>,
) -> impl Fn(&mut Workbench, &ClickEvent, &mut Window, &mut Context<Workbench>) + 'static {
    move |this, _, _, cx| {
        this.flush_plugin_inputs(&owner, cx);
        let event = UiEvent { element: element.clone(), event: event.into(), value: value.clone(), item: item.clone(), action: None };
        this.send_plugin_event(&owner, event, cx);
    }
}

impl Workbench {
    /// Loads the grammars `code` blocks are colored with, once a panel shows one.
    pub(super) fn load_plugin_grammars(&mut self, cx: &mut Context<Self>) {
        if self.plugin_grammars_loading || highlight::syntaxes().is_some() {
            return;
        }
        self.plugin_grammars_loading = true;
        let load = cx.background_spawn(async { highlight::load() });
        cx.spawn(async move |this, cx| {
            load.await;
            let _ = this.update(cx, |this, cx| {
                this.plugin_grammars_loading = false;
                this.plugin_code_colors.borrow_mut().clear();
                cx.notify();
            });
        })
        .detach();
    }

    /// The colored stretches of a `code` block; empty while the grammars load or for a language
    /// no grammar knows.
    fn code_colors(&self, text: &str, language: Option<&str>) -> Rc<Vec<(Range<usize>, u32)>> {
        let (Some(set), Some(language)) = (highlight::syntaxes(), language) else { return Rc::default() };
        let key = {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            (text, language).hash(&mut hasher);
            hasher.finish()
        };
        if let Some(colors) = self.plugin_code_colors.borrow().get(&key) {
            return colors.clone();
        }
        let path = PathBuf::from(format!("code.{}", code_extension(language)));
        let lines: Vec<&str> = text.split('\n').collect();
        let grammar = highlight::grammar_for(set, Language::detect(&path), &path, lines[0]);
        let mut colors = Vec::new();
        if let Some(grammar) = grammar {
            let mut highlights = highlight::Highlights::new(Some(grammar));
            let upto = lines.len().min(MAX_COLORED_LINES);
            // A block too slow to color in time is shown with the lines it got through.
            highlights.advance(upto, Instant::now() + Duration::from_millis(40), |i| lines[i]);
            let mut offset = 0;
            for (index, line) in lines.iter().enumerate().take(upto) {
                let mut at = offset;
                for span in highlights.spans(index).unwrap_or_default() {
                    if span.color != highlight::FOREGROUND {
                        colors.push((at..at + span.len, span.color));
                    }
                    at += span.len;
                }
                offset += line.len() + 1;
            }
        }
        let colors = Rc::new(colors);
        let mut cache = self.plugin_code_colors.borrow_mut();
        if cache.len() >= MAX_COLORED_BLOCKS {
            cache.clear();
        }
        cache.insert(key, colors.clone());
        colors
    }

    pub(super) fn render_plugin_node(&self, plugin: &str, node: &Node, path: &mut Vec<usize>, cx: &mut Context<Self>) -> AnyElement {
        let key = |id: &str, path: &[usize]| SharedString::from(format!("plugin-{plugin}-{id}-{path:?}"));
        match node {
            Node::Column { children, gap: g } => {
                self.render_plugin_children(plugin, children, path, div().flex().flex_col().gap(gap(*g)).min_w_0(), cx).into_any_element()
            }
            Node::Section { title, children } => div()
                .pt_3()
                .flex()
                .flex_col()
                .gap_2()
                .min_w_0()
                .child(div().t_caption().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::MUTED)).child(title.to_uppercase()))
                .child(self.render_plugin_children(plugin, children, path, div().flex().flex_col().gap_2().min_w_0(), cx))
                .into_any_element(),
            Node::Row { children, gap: g, wrap } => {
                let mut row = div().flex().items_center().gap(gap(*g)).min_w_0().when(*wrap, |d| d.flex_wrap());
                for (index, child) in children.iter().enumerate() {
                    path.push(index);
                    let rendered = self.render_plugin_node(plugin, child, path, cx);
                    row = row.child(if fills_row(child) { div().flex_1().min_w_0().child(rendered).into_any_element() } else { rendered });
                    path.pop();
                }
                row.into_any_element()
            }
            Node::Text { text, style } => {
                let base = div().min_w_0().whitespace_normal();
                match style {
                    TextStyle::Title => base.t_large().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)),
                    TextStyle::Muted => base.t_small().text_color(hex(Chrome::MUTED)),
                    TextStyle::Small => base.t_caption().text_color(hex(Chrome::MUTED)),
                    TextStyle::Code => base
                        .t_small()
                        .font_family(MONO)
                        .px_2p5()
                        .py_2()
                        .rounded_md()
                        .border_1()
                        .border_color(hex(Chrome::BORDER))
                        .bg(hex(SUNKEN))
                        .text_color(hex(Chrome::FOREGROUND)),
                    TextStyle::Error => base.t_small().text_color(hex(Chrome::ERROR)),
                    TextStyle::Success => base.t_small().text_color(hex(Chrome::SUCCESS)),
                    TextStyle::Body => base.t_body().text_color(hex(Chrome::FOREGROUND)),
                }
                .child(text.clone())
                .into_any_element()
            }
            Node::Button { id, label, icon: glyph, variant, disabled } => {
                let (bg, hover, border, fg) = match variant {
                    Variant::Primary => (hex(Chrome::ACCENT), lighten(Chrome::ACCENT, 0.12), hex(Chrome::ACCENT), hex(Chrome::BRIGHT)),
                    Variant::Secondary => (hex(CONTROL), lighten(CONTROL, 0.06), hex(Chrome::OVERLAY_BORDER), hex(Chrome::BRIGHT)),
                    Variant::Ghost => (hex_alpha(0, 0.), hex(Chrome::HOVER), hex_alpha(0, 0.), hex(Chrome::FOREGROUND)),
                    Variant::Danger => {
                        (hex_alpha(Chrome::ERROR, 0.14), hex_alpha(Chrome::ERROR, 0.24), hex_alpha(Chrome::ERROR, 0.4), hex(Chrome::ERROR))
                    }
                };
                div()
                    .id(key(id, path))
                    .flex_shrink_0()
                    .h(px(CONTROL_HEIGHT))
                    .px_3()
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap_1p5()
                    .rounded_md()
                    .border_1()
                    .border_color(border)
                    .t_small()
                    .font_weight(crate::theme::EMPHASIS)
                    .bg(bg)
                    .text_color(fg)
                    .when(*disabled, |d| d.opacity(0.4))
                    .when(!*disabled, |d| {
                        d.cursor_pointer().hover(move |s| s.bg(hover)).on_click(cx.listener(emit(
                            plugin.to_string(),
                            id.clone(),
                            "click",
                            None,
                            None,
                        )))
                    })
                    .children(glyph.as_deref().map(|g| icon(icon_named(Some(g)), IconSize::INLINE, fg)))
                    .child(label.clone())
                    .into_any_element()
            }
            Node::Input { id, rows, mono, .. } => match self.plugin_inputs.get(&(self.plugin_input_scope(plugin, cx), id.clone())) {
                Some(field) => div()
                    .w_full()
                    .when(*mono, |d| d.font_family(MONO))
                    .px_2p5()
                    .when(*rows <= 1, |d| d.min_h(px(CONTROL_HEIGHT)).flex().items_center())
                    .when(*rows > 1, |d| d.py_1p5())
                    .rounded_md()
                    .border_1()
                    .border_color(hex(Chrome::OVERLAY_BORDER))
                    .bg(hex(SUNKEN))
                    .t_small()
                    .text_color(hex(Chrome::BRIGHT))
                    .child(div().w_full().child(field.input.clone()))
                    .into_any_element(),
                None => div().into_any_element(),
            },
            Node::List { id, items, empty } => {
                if items.is_empty() {
                    return empty_state(empty.clone().unwrap_or_else(|| t(cx, "plugins.ui.no_rows").to_string())).into_any_element();
                }
                let mut list = div().flex().flex_col().gap_0p5();
                for (index, item) in items.iter().enumerate() {
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
                                this.flush_plugin_inputs(&owner, cx);
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
                            .gap_2p5()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|s| s.bg(hex(Chrome::HOVER)))
                            .on_click(cx.listener(emit(plugin.to_string(), id.clone(), "select", None, Some(item.id.clone()))))
                            // A tree: each level a step in.
                            .when(item.depth > 0, |d| d.pl(px(8. + f32::from(item.depth) * 14.)))
                            .when_some(item.icon.as_deref(), |d, g| {
                                // A neutral icon keeps the text's color; a tone tints it.
                                let color = if item.tone == Tone::Neutral { Chrome::FOREGROUND } else { tone_color(item.tone) };
                                d.child(icon_tile(Some(g), ROW_ICON, color))
                            })
                            .when_some(item.tag.clone().filter(|t| !t.is_empty()), |d, tag| {
                                // An HTTP method or a status, in a column of its own so titles line up.
                                let color = if item.tag_tone == Tone::Neutral { Chrome::MUTED } else { tone_color(item.tag_tone) };
                                d.child(
                                    div()
                                        .flex_shrink_0()
                                        .min_w(px(38.))
                                        .t_caption()
                                        .font_family(MONO)
                                        .font_weight(crate::theme::EMPHASIS)
                                        .text_color(hex(color))
                                        .child(tag),
                                )
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap_0p5()
                                    .child(div().t_body().text_color(hex(Chrome::BRIGHT)).truncate().child(item.title.clone()))
                                    .children(
                                        item.subtitle.clone().map(|s| div().t_small().text_color(hex(Chrome::MUTED)).truncate().child(s)),
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
                    row = row.child(crate::ui::chip(
                        SharedString::from(format!("plugin-choice-{plugin}-{id}-{index}")),
                        option.label.clone(),
                        *value == option.value,
                        cx.listener(emit(plugin.to_string(), id.clone(), "change", Some(option.value.clone().into()), None)),
                    ));
                }
                row.into_any_element()
            }
            Node::Toggle { id, label, value } => div()
                .id(key(id, path))
                .flex()
                .items_center()
                .gap_2()
                .cursor_pointer()
                .t_small()
                .text_color(hex(Chrome::FOREGROUND))
                .hover(|s| s.text_color(hex(Chrome::BRIGHT)))
                .on_click(cx.listener(emit(plugin.to_string(), id.clone(), "change", Some((!*value).into()), None)))
                .child(
                    div()
                        .flex_shrink_0()
                        .w(px(30.))
                        .h(px(17.))
                        .rounded_full()
                        .p(px(2.))
                        .flex()
                        .when(*value, |d| d.justify_end())
                        .bg(if *value { hex(Chrome::ACCENT) } else { hex(0x3a3a40) })
                        .child(div().size(px(13.)).rounded_full().bg(hex(Chrome::BRIGHT))),
                )
                .child(label.clone())
                .into_any_element(),
            Node::Badge { text, tone } => div()
                .flex_shrink_0()
                .px_2()
                .py_0p5()
                .rounded_full()
                .bg(hex_alpha(tone_color(*tone), 0.16))
                .t_caption()
                .font_weight(crate::theme::EMPHASIS)
                .text_color(hex(tone_color(*tone)))
                .child(text.clone())
                .into_any_element(),
            Node::Spinner { text } => crate::ui::loading_row(text.clone()).into_any_element(),
            Node::Divider => div().my_1().h(px(1.)).w_full().bg(hex(Chrome::BORDER)).into_any_element(),
            // Drawn beside the panel (`render_plugin_popover`), not in its flow.
            Node::Popover { .. } => div().into_any_element(),
            Node::Flow { id, steps } => {
                // Cards top to bottom, each joined to the next by a short line under its icon: the
                // line is faint where the step it leads to is off.
                let mut flow = div().flex().flex_col().min_w_0();
                for (index, step) in steps.iter().enumerate() {
                    if index > 0 {
                        let lit = step.state != FlowState::Off && steps[index - 1].state != FlowState::Off;
                        flow = flow.child(div().ml(px(FLOW_LINE_LEFT)).w(px(2.)).h(px(12.)).bg(if lit {
                            hex_alpha(Chrome::SUCCESS, 0.55)
                        } else {
                            hex(Chrome::BORDER)
                        }));
                    }
                    let color = match step.state {
                        FlowState::Off => Chrome::MUTED,
                        FlowState::On => Chrome::BLUE,
                        FlowState::Active => Chrome::ORANGE,
                        FlowState::Done => Chrome::SUCCESS,
                        FlowState::Error => Chrome::ERROR,
                    };
                    let border = if step.selected {
                        hex(Chrome::ACCENT)
                    } else if step.state == FlowState::Active {
                        hex_alpha(Chrome::ORANGE, 0.6)
                    } else {
                        hex(Chrome::BORDER)
                    };
                    flow = flow.child(
                        div()
                            .id(SharedString::from(format!("plugin-flow-{plugin}-{id}-{index}")))
                            // An optional step branches off the main line.
                            .when(step.side, |d| d.ml(px(FLOW_SIDE_INDENT)))
                            .px_2()
                            .py_1p5()
                            .flex()
                            .items_center()
                            .gap_2()
                            .rounded_lg()
                            .border_1()
                            .border_color(border)
                            .bg(hex(if step.selected { Chrome::SELECTED } else { RAISED }))
                            .cursor_pointer()
                            .hover(|s| s.bg(hex(Chrome::HOVER)))
                            .when(step.state == FlowState::Off, |d| d.opacity(0.55))
                            .on_click(cx.listener(emit(plugin.to_string(), id.clone(), "select", None, Some(step.id.clone()))))
                            .child(icon_tile(step.icon.as_deref(), FLOW_ICON, color))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(div().t_small().text_color(hex(Chrome::BRIGHT)).truncate().child(step.title.clone()))
                                    .children(
                                        step.subtitle.clone().map(|s| div().t_caption().text_color(hex(Chrome::MUTED)).truncate().child(s)),
                                    ),
                            )
                            .map(|d| match step.state {
                                FlowState::Active => d.child(crate::ui::dot_spinner(
                                    SharedString::from(format!("plugin-flow-spin-{plugin}-{id}-{index}")),
                                    12.,
                                    hex(Chrome::ORANGE),
                                )),
                                FlowState::Error => d.child(icon("circle-x", IconSize::INLINE, hex(Chrome::ERROR))),
                                _ => d.child(icon("chevron-right", IconSize::INLINE, hex(Chrome::MUTED))),
                            }),
                    );
                }
                flow.into_any_element()
            }
            Node::Card { title, subtitle, icon: glyph, tone, children } => {
                let heading = (title.is_some() || subtitle.is_some() || glyph.is_some()).then(|| {
                    div()
                        .flex()
                        .items_center()
                        .gap_2p5()
                        .min_w_0()
                        .when_some(glyph.as_deref(), |d, g| {
                            let color = if *tone == Tone::Neutral { Chrome::FOREGROUND } else { tone_color(*tone) };
                            d.child(icon_tile(Some(g), FLOW_ICON, color))
                        })
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .children(title.clone().map(|title| {
                                    div().t_body().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(title)
                                }))
                                .children(subtitle.clone().map(|s| div().t_small().text_color(hex(Chrome::MUTED)).child(s))),
                        )
                });
                div()
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_2p5()
                    .min_w_0()
                    .rounded_lg()
                    .border_1()
                    .border_color(if *tone == Tone::Neutral { hex(Chrome::BORDER) } else { hex_alpha(tone_color(*tone), 0.45) })
                    .bg(hex(RAISED))
                    .children(heading)
                    .when(!children.is_empty(), |d| {
                        d.child(self.render_plugin_children(plugin, children, path, div().flex().flex_col().gap_2().min_w_0(), cx))
                    })
                    .into_any_element()
            }
            Node::Grid { children, columns, gap: g, widths } => {
                let columns = (*columns).max(1);
                let widths: Vec<GridWidth> =
                    (0..columns).map(|i| widths.get(i).map_or(GridWidth::Share(1), |w| GridWidth::parse(w))).collect();
                // A fixed column keeps its width; the others share what is left, by their weight.
                let cell = |column: usize| match widths[column] {
                    GridWidth::Px(px_width) => div().w(px(px_width)).flex_shrink_0().min_w_0(),
                    GridWidth::Share(share) => div().flex_grow().flex_basis(px(0.)).min_w_0().map(|d| {
                        let mut d = d;
                        d.style().flex_grow = Some(share as f32);
                        d
                    }),
                };
                let mut grid = div().flex().flex_col().gap(gap(*g)).min_w_0();
                for (row_index, chunk) in children.chunks(columns).enumerate() {
                    let mut row = div().flex().items_start().gap(gap(*g)).min_w_0();
                    for (offset, child) in chunk.iter().enumerate() {
                        path.push(row_index * columns + offset);
                        row = row.child(cell(offset).flex().flex_col().child(self.render_plugin_node(plugin, child, path, cx)));
                        path.pop();
                    }
                    // The last row keeps the others' column widths.
                    for column in chunk.len()..columns {
                        row = row.child(cell(column));
                    }
                    grid = grid.child(row);
                }
                grid.into_any_element()
            }
            Node::Tabs { id, tabs, value, children } => {
                let mut strip = div().flex().items_end().gap_1().min_w_0().border_b_1().border_color(hex(Chrome::BORDER));
                for (index, tab) in tabs.iter().enumerate() {
                    let active = tab.id == *value;
                    let fg = if active { Chrome::BRIGHT } else { Chrome::MUTED };
                    strip = strip.child(
                        div()
                            .id(SharedString::from(format!("plugin-tab-{plugin}-{id}-{index}")))
                            .px_2p5()
                            .py_1p5()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .min_w_0()
                            .border_b_2()
                            .border_color(if active { hex(Chrome::ACCENT) } else { hex_alpha(0, 0.) })
                            .t_small()
                            .when(active, |d| d.font_weight(crate::theme::EMPHASIS))
                            .text_color(hex(fg))
                            .cursor_pointer()
                            .hover(|s| s.text_color(hex(Chrome::BRIGHT)))
                            .on_click(cx.listener(emit(plugin.to_string(), id.clone(), "change", Some(tab.id.clone().into()), None)))
                            .when_some(tab.icon.as_deref(), |d, g| d.child(icon(icon_named(Some(g)), IconSize::INLINE, hex(fg))))
                            .child(div().truncate().child(tab.label.clone()))
                            .when_some(tab.badge.clone(), |d, badge| {
                                d.child(
                                    div()
                                        .px_1p5()
                                        .rounded_full()
                                        .bg(hex(if active { Chrome::SELECTED } else { Chrome::HOVER }))
                                        .t_caption()
                                        .text_color(hex(fg))
                                        .child(badge),
                                )
                            })
                            .when(tab.closable, |d| {
                                let (owner, element, tab_id) = (plugin.to_string(), id.clone(), tab.id.clone());
                                d.child(
                                    div()
                                        .id(SharedString::from(format!("plugin-tab-close-{plugin}-{id}-{index}")))
                                        .ml_0p5()
                                        .rounded_sm()
                                        .flex()
                                        .items_center()
                                        .hover(|s| s.bg(hex(Chrome::HOVER)))
                                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                            // Closing is not picking.
                                            cx.stop_propagation();
                                            this.flush_plugin_inputs(&owner, cx);
                                            let event = UiEvent {
                                                element: element.clone(),
                                                event: "close".into(),
                                                value: Some(tab_id.clone().into()),
                                                item: None,
                                                action: None,
                                            };
                                            this.send_plugin_event(&owner, event, cx);
                                        }))
                                        .child(icon("x", IconSize::INLINE, hex(Chrome::MUTED))),
                                )
                            }),
                    );
                }
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .min_w_0()
                    .child(strip)
                    .child(self.render_plugin_children(plugin, children, path, div().flex().flex_col().gap_2().min_w_0(), cx))
                    .into_any_element()
            }
            Node::Table { id, columns, rows, empty, selected } => {
                let total: u32 = columns.iter().map(|c| c.grow).sum::<u32>().max(1);
                let cell = |grow: u32, align: Align| {
                    div().w(relative(grow as f32 / total as f32)).min_w_0().px_2().flex().items_center().gap_1p5().map(|d| match align {
                        Align::Start => d,
                        Align::Center => d.justify_center(),
                        Align::End => d.justify_end(),
                    })
                };
                let mut header = div().flex().py_1p5().border_b_1().border_color(hex(Chrome::BORDER));
                for column in columns {
                    header = header.child(
                        cell(column.grow, column.align)
                            .t_caption()
                            .font_weight(crate::theme::EMPHASIS)
                            .text_color(hex(Chrome::MUTED))
                            .child(div().truncate().child(column.label.to_uppercase())),
                    );
                }
                let mut table = div().flex().flex_col().min_w_0().rounded_lg().border_1().border_color(hex(Chrome::BORDER)).bg(hex(SUNKEN));
                table = table.child(header);
                if rows.is_empty() {
                    table = table.child(empty_state(empty.clone().unwrap_or_else(|| t(cx, "plugins.ui.no_rows").to_string())));
                }
                for (index, row) in rows.iter().enumerate() {
                    let picked = selected.as_deref() == Some(row.id.as_str());
                    let mut line = div()
                        .id(SharedString::from(format!("plugin-table-{plugin}-{id}-{index}")))
                        .flex()
                        .py_1p5()
                        .when(index + 1 < rows.len(), |d| d.border_b_1().border_color(hex_alpha(Chrome::BORDER, 0.6)))
                        .when(picked, |d| d.bg(hex(Chrome::SELECTED)))
                        .cursor_pointer()
                        .hover(|s| s.bg(hex(Chrome::HOVER)))
                        .on_click(cx.listener(emit(plugin.to_string(), id.clone(), "select", None, Some(row.id.clone()))));
                    for (column_index, column) in columns.iter().enumerate() {
                        let text = row.cells.get(column_index).cloned().unwrap_or_default();
                        let first = column_index == 0;
                        line = line.child(
                            cell(column.grow, column.align)
                                .t_small()
                                .text_color(hex(if first { Chrome::BRIGHT } else { Chrome::FOREGROUND }))
                                .when(first && row.tone != Tone::Neutral, |d| {
                                    d.child(div().flex_shrink_0().size(px(7.)).rounded_full().bg(hex(tone_color(row.tone))))
                                })
                                .child(div().truncate().child(text)),
                        );
                    }
                    table = table.child(line);
                }
                table.into_any_element()
            }
            Node::KeyValue { items } => {
                let mut list = div().flex().flex_col().min_w_0();
                for (index, item) in items.iter().enumerate() {
                    list = list.child(
                        div()
                            .flex()
                            .items_start()
                            .gap_3()
                            .py_1p5()
                            .when(index + 1 < items.len(), |d| d.border_b_1().border_color(hex_alpha(Chrome::BORDER, 0.6)))
                            .child(
                                div().w(relative(0.38)).flex_shrink_0().t_small().text_color(hex(Chrome::MUTED)).child(item.label.clone()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .whitespace_normal()
                                    .t_small()
                                    .when(item.mono, |d| d.font_family(MONO))
                                    .text_color(hex(if item.tone == Tone::Neutral { Chrome::BRIGHT } else { tone_color(item.tone) }))
                                    .child(item.value.clone()),
                            ),
                    );
                }
                list.into_any_element()
            }
            Node::Stat { label, value, detail, icon: glyph, tone } => {
                let accent = if *tone == Tone::Neutral { Chrome::MUTED } else { tone_color(*tone) };
                div()
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .min_w_0()
                    .rounded_lg()
                    .border_1()
                    .border_color(hex(Chrome::BORDER))
                    .bg(hex(RAISED))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .min_w_0()
                            .when_some(glyph.as_deref(), |d, g| d.child(icon(icon_named(Some(g)), IconSize::INLINE, hex(accent))))
                            .child(
                                div()
                                    .truncate()
                                    .t_caption()
                                    .font_weight(crate::theme::EMPHASIS)
                                    .text_color(hex(Chrome::MUTED))
                                    .child(label.to_uppercase()),
                            ),
                    )
                    .child(cut_line(div().t_display().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)), value.clone()))
                    .children(detail.clone().map(|detail| cut_line(div().t_caption().text_color(hex(accent)), detail)))
                    .into_any_element()
            }
            Node::Progress { value, label, detail, tone } => {
                let fill = if *tone == Tone::Neutral { Chrome::ACCENT } else { tone_color(*tone) };
                let percent = format!("{}%", (value * 100.).round() as u32);
                div()
                    .flex()
                    .flex_col()
                    .gap_1p5()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .t_small()
                            .child(div().flex_1().min_w_0().truncate().text_color(hex(Chrome::FOREGROUND)).children(label.clone()))
                            .child(div().flex_shrink_0().text_color(hex(Chrome::MUTED)).child(detail.clone().unwrap_or(percent))),
                    )
                    .child(
                        div()
                            .h(px(6.))
                            .w_full()
                            .rounded_full()
                            .bg(hex(Chrome::BORDER))
                            .child(div().h_full().w(relative(*value as f32)).rounded_full().bg(hex(fill))),
                    )
                    .into_any_element()
            }
            Node::Callout { text, title, icon: glyph, tone } => {
                let color = tone_color(*tone);
                div()
                    .px_3()
                    .py_2p5()
                    .flex()
                    .items_start()
                    .gap_2p5()
                    .min_w_0()
                    .rounded_lg()
                    .border_1()
                    .border_color(hex_alpha(color, 0.35))
                    .bg(hex_alpha(color, 0.1))
                    .child(div().pt(px(1.)).child(icon(
                        glyph.as_deref().map_or(tone_icon(*tone), |g| icon_named(Some(g))),
                        IconSize::INLINE,
                        hex(color),
                    )))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .children(title.clone().map(|title| {
                                div().t_small().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(title)
                            }))
                            .child(div().whitespace_normal().t_small().text_color(hex(Chrome::FOREGROUND)).child(text.clone())),
                    )
                    .into_any_element()
            }
            Node::Select { id, options, value, placeholder, disabled } => {
                let scope = self.plugin_input_scope(plugin, cx);
                let open = !*disabled && self.plugin_select_open.as_ref() == Some(&(scope.clone(), id.clone()));
                let current = options.iter().find(|o| o.value == *value).map(|o| o.label.clone());
                let shown = current.is_some();
                let label = current.or_else(|| placeholder.clone()).unwrap_or_else(|| t(cx, "plugins.ui.choose").to_string());
                let toggle_key = (scope.clone(), id.clone());
                let trigger = div()
                    .id(key(id, path))
                    .w_full()
                    .h(px(CONTROL_HEIGHT))
                    .px_2p5()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_md()
                    .border_1()
                    .border_color(hex(if open { Chrome::ACCENT } else { Chrome::OVERLAY_BORDER }))
                    .bg(hex(SUNKEN))
                    .t_small()
                    .when(*disabled, |d| d.opacity(0.4))
                    .when(!*disabled, |d| {
                        d.cursor_pointer().hover(|s| s.border_color(hex(lighten_u32(Chrome::OVERLAY_BORDER)))).on_click(cx.listener(
                            move |this, _: &ClickEvent, _, cx| {
                                this.plugin_select_open =
                                    if this.plugin_select_open.as_ref() == Some(&toggle_key) { None } else { Some(toggle_key.clone()) };
                                cx.notify();
                            },
                        ))
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(hex(if shown { Chrome::BRIGHT } else { Chrome::MUTED }))
                            .child(label),
                    )
                    .child(icon("chevron-down", IconSize::INLINE, hex(Chrome::MUTED)));
                let menu = open.then(|| {
                    let mut menu = div()
                        .id(SharedString::from(format!("plugin-select-menu-{plugin}-{id}")))
                        .min_w(px(200.))
                        .max_h(px(280.))
                        .overflow_y_scroll()
                        .flex()
                        .flex_col();
                    for (index, option) in options.iter().enumerate() {
                        let picked = option.value == *value;
                        let (owner, element, chosen) = (plugin.to_string(), id.clone(), option.value.clone());
                        menu = menu.child(
                            div()
                                .id(SharedString::from(format!("plugin-select-{plugin}-{id}-{index}")))
                                .px_2()
                                .py_1()
                                .flex()
                                .items_center()
                                .gap_2()
                                .rounded_md()
                                .t_small()
                                .cursor_pointer()
                                .text_color(hex(if picked { Chrome::BRIGHT } else { Chrome::FOREGROUND }))
                                .hover(|s| s.bg(hex(Chrome::HOVER)))
                                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    this.plugin_select_open = None;
                                    this.flush_plugin_inputs(&owner, cx);
                                    let event = UiEvent {
                                        element: element.clone(),
                                        event: "change".into(),
                                        value: Some(chosen.clone().into()),
                                        item: None,
                                        action: None,
                                    };
                                    this.send_plugin_event(&owner, event, cx);
                                }))
                                .child(div().flex_1().min_w_0().truncate().child(option.label.clone()))
                                .when(picked, |d| d.child(icon("check", IconSize::INLINE, hex(Chrome::ACCENT)))),
                        );
                    }
                    let card = crate::ui::popover()
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.plugin_select_open = None;
                            cx.notify();
                        }))
                        .child(menu);
                    let anchored =
                        gpui::anchored().anchor(gpui::Corner::TopLeft).snap_to_window_with_margin(px(8.)).child(div().mt_1().child(card));
                    div().absolute().top(px(CONTROL_HEIGHT)).left_0().child(gpui::deferred(anchored).with_priority(3))
                });
                div().relative().w_full().min_w_0().child(trigger).children(menu).into_any_element()
            }
            Node::Checkbox { id, label, value, description, disabled } => div()
                .id(key(id, path))
                .flex()
                .items_start()
                .gap_2()
                .min_w_0()
                .when(*disabled, |d| d.opacity(0.4))
                .when(!*disabled, |d| {
                    d.cursor_pointer().on_click(cx.listener(emit(plugin.to_string(), id.clone(), "change", Some((!*value).into()), None)))
                })
                .child(
                    div()
                        .mt(px(1.))
                        .size(px(16.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_sm()
                        .border_1()
                        .border_color(hex(if *value { Chrome::ACCENT } else { Chrome::OVERLAY_BORDER }))
                        .bg(hex(if *value { Chrome::ACCENT } else { SUNKEN }))
                        .when(*value, |d| d.child(icon("check", 12., hex(Chrome::BRIGHT)))),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .child(div().t_small().text_color(hex(Chrome::BRIGHT)).child(label.clone()))
                        .children(
                            description.clone().map(|d| div().whitespace_normal().t_caption().text_color(hex(Chrome::MUTED)).child(d)),
                        ),
                )
                .into_any_element(),
            Node::Code { text, language, title } => {
                let colors = self.code_colors(text, language.as_deref());
                let styled = StyledText::new(text.clone()).with_highlights(
                    colors.iter().map(|(range, color)| (range.clone(), HighlightStyle { color: Some(hex(*color)), ..Default::default() })),
                );
                let copied = text.clone();
                let copy = crate::ui::icon_only(
                    SharedString::from(format!("plugin-code-copy-{plugin}-{path:?}")),
                    "copy",
                    move |_: &ClickEvent, _: &mut Window, cx: &mut gpui::App| {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(copied.clone()))
                    },
                )
                .tooltip(Tooltip::text(t(cx, "plugins.ui.copy"), None));
                let caption = title.clone().or_else(|| language.clone());
                let group = SharedString::from(format!("plugin-code-{plugin}-{path:?}"));
                div()
                    .group(group.clone())
                    .relative()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .rounded_lg()
                    .border_1()
                    .border_color(hex(Chrome::BORDER))
                    .bg(hex(SUNKEN))
                    .map(|d| match caption {
                        Some(caption) => d.child(
                            div()
                                .h(px(30.))
                                .pl_3()
                                .pr_1()
                                .flex()
                                .items_center()
                                .border_b_1()
                                .border_color(hex(Chrome::BORDER))
                                .child(div().flex_1().min_w_0().truncate().t_caption().text_color(hex(Chrome::MUTED)).child(caption))
                                .child(copy),
                        ),
                        // No heading: the button sits in the corner while the block is hovered.
                        None => d.child(div().absolute().top_1().right_1().invisible().group_hover(group, |s| s.visible()).child(copy)),
                    })
                    .child(
                        div()
                            .id(SharedString::from(format!("plugin-code-body-{plugin}-{path:?}")))
                            .px_3()
                            .py_2p5()
                            .overflow_x_scroll()
                            .font_family(MONO)
                            .t_small()
                            .text_color(hex(highlight::FOREGROUND))
                            .child(div().whitespace_nowrap().child(styled)),
                    )
                    .into_any_element()
            }
        }
    }

    /// `children` drawn into `container`, each under its own index in `path`.
    fn render_plugin_children(
        &self,
        plugin: &str,
        children: &[Node],
        path: &mut Vec<usize>,
        container: Div,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut container = container;
        for (index, child) in children.iter().enumerate() {
            path.push(index);
            container = container.child(self.render_plugin_node(plugin, child, path, cx));
            path.pop();
        }
        container
    }
}

/// One line of `text` in `style`, cut with an ellipsis when it does not fit. Inside a row: a cut
/// line placed straight in a column is measured at no width and shows only its ellipsis.
fn cut_line(style: Div, text: String) -> Div {
    div().flex().min_w_0().child(style.flex_1().min_w_0().truncate().child(text))
}

/// What an empty list or table says, centered and quiet.
fn empty_state(text: String) -> Div {
    div()
        .py_4()
        .px_3()
        .flex()
        .flex_col()
        .items_center()
        .gap_1p5()
        .child(icon("list", IconSize::BUTTON, hex_alpha(Chrome::MUTED, 0.7)))
        .child(div().whitespace_normal().t_small().text_color(hex(Chrome::MUTED)).child(text))
}

fn lighten_u32(value: u32) -> u32 {
    crate::theme::lighten_rgb(value, 0.12)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_read_as_the_extensions_their_grammars_know() {
        assert_eq!(code_extension("Rust"), "rs");
        assert_eq!(code_extension(".ts"), "ts");
        assert_eq!(code_extension("bash"), "sh");
        assert_eq!(code_extension("json"), "json");
    }

    #[test]
    fn a_code_block_anywhere_in_the_tree_is_found() {
        let tree = Node::from_value(serde_json::json!({ "type": "column", "children": [
            { "type": "card", "children": [{ "type": "code", "text": "x" }] }
        ] }))
        .unwrap();
        assert!(has_code(&tree));
        assert!(!has_code(&Node::Divider));
    }
}
