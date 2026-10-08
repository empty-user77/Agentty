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

/// Dragging a plugin grid's column edge. Which grid is in the handler's own closure; the payload
/// only has to exist for GPUI to track the drag.
#[derive(Clone, Copy)]
pub struct PluginGridDrag;

impl gpui::Render for PluginGridDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// Dragging a plugin tab strip sideways: which strip, and where the pointer was last.
#[derive(Clone)]
pub struct PluginTabStripDrag {
    strip: String,
    last_x: Rc<std::cell::Cell<Option<f32>>>,
}

impl gpui::Render for PluginTabStripDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// A `flow` step being dragged to a new place: its flow (`plugin/element`), id and title.
#[derive(Clone)]
pub struct PluginFlowDrag {
    flow: String,
    step: String,
    title: String,
}

impl gpui::Render for PluginFlowDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(hex(Chrome::ACCENT))
            .bg(hex(RAISED))
            .t_small()
            .text_color(hex(Chrome::BRIGHT))
            .child(self.title.clone())
    }
}

/// The narrowest and widest a resizable column is dragged to.
const RESIZE_RANGE: (f32, f32) = (160., 720.);

/// Behind fields, code and the track of a bar: a step darker than the panel.
const SUNKEN: u32 = 0x1a1a1d;
/// Cards and stats: a step lighter than the panel.
const RAISED: u32 = 0x27272b;
/// Secondary buttons.
const CONTROL: u32 = 0x2e2e33;
/// Height of a tab strip, its rule included: two strips side by side draw it on one line.
const TAB_STRIP_HEIGHT: f32 = 36.;
/// Row buttons shown on hover; the rest go in a "more" menu.
const INLINE_ROW_ACTIONS: usize = 3;
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
    // A heading is as wide as its words: what follows it in the row sits beside it.
    if let Node::Text { style, .. } = node {
        return *style != TextStyle::Title;
    }
    matches!(
        node,
        Node::Input { .. }
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

/// How wide a drop-down in a row is: its longest label, between a short word and a long name.
fn select_width(options: &[agentty_bridge::plugins::ui::ChoiceOption], placeholder: Option<&str>) -> f32 {
    // Wide characters (Korean, Japanese, Chinese) take about two narrow ones.
    let width = |text: &str| text.chars().map(|c| if c.len_utf8() > 2 { 2.0 } else { 1.0 }).sum::<f32>();
    let longest = options.iter().map(|o| width(&o.label)).chain(placeholder.map(width)).fold(0.0, f32::max);
    (longest * 7.2 + 48.).clamp(96., 280.)
}

/// Whether the tree shows a `code` block (and so needs the grammars).
pub(super) fn has_code(node: &Node) -> bool {
    matches!(node, Node::Code { .. } | Node::Input { language: Some(_), .. }) || node.children().iter().any(has_code)
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

    /// A menu under the control it belongs to, while it is the open one (`key`): each entry sends
    /// its event when picked. Placed against the control's right edge when `right`.
    fn plugin_menu(
        &self,
        plugin: &str,
        key: &str,
        entries: Vec<(String, Option<String>, UiEvent)>,
        right: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.plugin_tab_menu.as_deref() != Some(key) {
            return None;
        }
        let mut menu = div()
            .id(SharedString::from(format!("plugin-menu-{key}")))
            .min_w(px(180.))
            .p_1()
            .flex()
            .flex_col()
            .gap_0p5()
            .rounded_md()
            .border_1()
            .border_color(hex(Chrome::OVERLAY_BORDER))
            .bg(hex(RAISED))
            .shadow_lg()
            .occlude()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                if let Some(closed) = this.plugin_tab_menu.take() {
                    this.plugin_menu_closed = Some((closed, Instant::now()));
                }
                cx.notify();
            }));
        for (index, (label, glyph, event)) in entries.into_iter().enumerate() {
            let owner = plugin.to_string();
            menu = menu.child(
                div()
                    .id(SharedString::from(format!("plugin-menu-{key}-{index}")))
                    .h(px(28.))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_sm()
                    .t_small()
                    .text_color(hex(Chrome::BRIGHT))
                    .cursor_pointer()
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        this.plugin_tab_menu = None;
                        this.flush_plugin_inputs(&owner, cx);
                        this.send_plugin_event(&owner, event.clone(), cx);
                        cx.notify();
                    }))
                    .when_some(glyph.as_deref(), |d, g| d.child(icon(icon_named(Some(g)), IconSize::INLINE, hex(Chrome::MUTED))))
                    .child(label),
            );
        }
        let corner = if right { gpui::Corner::TopRight } else { gpui::Corner::TopLeft };
        let anchored = gpui::anchored().anchor(corner).snap_to_window_with_margin(px(8.)).child(menu);
        let place = div().absolute().top(px(CONTROL_HEIGHT + 4.));
        Some(if right { place.right_0() } else { place.left_0() }.child(self.overlay(anchored)).into_any_element())
    }

    /// The gap between two `flow` steps: the line joining them, and on hover "+ block" in the
    /// middle of a dashed rule, which opens the flow's insert menu.
    #[allow(clippy::too_many_arguments)]
    fn flow_gap(
        &self,
        plugin: &str,
        id: &str,
        after: &str,
        index: usize,
        lit: bool,
        insert_menu: &[agentty_bridge::plugins::ui::ChoiceOption],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let line = div().w(px(2.)).h_full().bg(if lit { hex_alpha(Chrome::SUCCESS, 0.55) } else { hex(Chrome::BORDER) });
        if insert_menu.is_empty() {
            return div().ml(px(FLOW_LINE_LEFT)).h(px(12.)).child(line).into_any_element();
        }
        let key = format!("flowins:{plugin}/{id}/{after}");
        let open = self.plugin_tab_menu.as_deref() == Some(key.as_str());
        let group = SharedString::from(format!("plugin-flow-gap-{plugin}-{id}-{index}"));
        let entries = insert_menu
            .iter()
            .map(|o| {
                let event = UiEvent {
                    element: id.to_string(),
                    event: "insert".into(),
                    value: Some(o.value.clone().into()),
                    item: Some(after.to_string()),
                    action: None,
                };
                (o.label.clone(), None, event)
            })
            .collect();
        let rule = || div().flex_1().h(px(1.)).border_t_1().border_dashed().border_color(hex_alpha(Chrome::ACCENT, 0.6));
        let control = div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .gap_2()
            .when(!open, |d| d.invisible().group_hover(group.clone(), |s| s.visible()))
            .child(rule())
            .child(
                div()
                    .relative()
                    .child(
                        div()
                            .id(SharedString::from(format!("plugin-flow-insert-{plugin}-{id}-{index}")))
                            .h(px(20.))
                            .px_2()
                            .flex()
                            .items_center()
                            .gap_1()
                            .rounded_md()
                            .border_1()
                            .border_color(hex_alpha(Chrome::ACCENT, 0.6))
                            .bg(hex(Chrome::PANEL))
                            .t_caption()
                            .text_color(hex(Chrome::BRIGHT))
                            .cursor_pointer()
                            .hover(|s| s.bg(hex(Chrome::HOVER)))
                            .on_click(cx.listener(Self::toggle_plugin_menu(key.clone())))
                            .child(icon("plus", IconSize::INLINE, hex(Chrome::ACCENT)))
                            .child(t(cx, "plugins.ui.add_block").to_string()),
                    )
                    .children(self.plugin_menu(plugin, &key, entries, false, cx)),
            )
            .child(rule());
        div()
            .id(group.clone())
            .group(group)
            .relative()
            .h(px(22.))
            .child(div().ml(px(FLOW_LINE_LEFT)).h_full().child(line))
            .child(control)
            .into_any_element()
    }

    /// A menu or a list of suggestions over everything else: deferred, unless it is in a popover,
    /// which is deferred itself.
    fn overlay(&self, anchored: gpui::Anchored) -> AnyElement {
        if self.plugin_in_popover.get() {
            anchored.into_any_element()
        } else {
            gpui::deferred(anchored).with_priority(3).into_any_element()
        }
    }

    /// Opens or closes the menu `key`.
    fn toggle_plugin_menu(key: String) -> impl Fn(&mut Workbench, &ClickEvent, &mut Window, &mut Context<Workbench>) + 'static {
        move |this, _, _, cx| {
            cx.stop_propagation();
            // Pressing the button of an open menu closes it: the press outside the menu already
            // did, and this click is the same press.
            let just_closed = this.plugin_menu_closed.take().is_some_and(|(k, at)| k == key && at.elapsed() < Duration::from_millis(400));
            this.plugin_tab_menu =
                if just_closed || this.plugin_tab_menu.as_deref() == Some(key.as_str()) { None } else { Some(key.clone()) };
            cx.notify();
        }
    }

    /// The bar on a resizable grid column's right edge, in the gap beside it: dragging it moves the
    /// edge, and the plugin hears the width once the drag pauses.
    fn grid_resize_handle(&self, plugin: &str, grid: &str, current: f32, gap: gpui::Pixels, cx: &mut Context<Self>) -> impl IntoElement {
        let (owner, element) = (plugin.to_string(), grid.to_string());
        let half = f32::from(gap) / 2.;
        div()
            .id(SharedString::from(format!("plugin-grid-resize-{plugin}-{grid}")))
            .absolute()
            .top_0()
            .bottom_0()
            .right(px(-half - 3.))
            .w(px(6.))
            .flex()
            .justify_center()
            .cursor(gpui::CursorStyle::ResizeLeftRight)
            .group("grid-resize")
            .child(div().w(px(1.)).h_full().bg(hex(Chrome::BORDER)).group_hover("grid-resize", |s| s.w(px(2.)).bg(hex(Chrome::ACCENT))))
            .on_drag(PluginGridDrag, |_, _, _, cx| cx.new(|_| PluginGridDrag))
            .on_drag_move(cx.listener(move |this, event: &gpui::DragMoveEvent<PluginGridDrag>, _, cx| {
                // How far the pointer is from the bar drawn in the last frame.
                let delta = f32::from(event.event.position.x) - (f32::from(event.bounds.origin.x) + 3.);
                if delta.abs() < 0.5 {
                    return;
                }
                let key = (owner.clone(), element.clone());
                let (width, count) = this.plugin_grid_widths.get(&key).copied().unwrap_or((current, 0));
                let width = (width + delta).clamp(RESIZE_RANGE.0, RESIZE_RANGE.1).round();
                this.plugin_grid_widths.insert(key.clone(), (width, count + 1));
                cx.notify();
                let (owner, element) = (owner.clone(), element.clone());
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(Duration::from_millis(300)).await;
                    let _ = this.update(cx, |this, cx| {
                        if this.plugin_grid_widths.get(&key).is_some_and(|(_, c)| *c == count + 1) {
                            let event = UiEvent { element, event: "resize".into(), value: Some(width.into()), item: None, action: None };
                            this.send_plugin_event(&owner, event, cx);
                        }
                    });
                })
                .detach();
            }))
    }

    pub(super) fn render_plugin_node(&self, plugin: &str, node: &Node, path: &mut Vec<usize>, cx: &mut Context<Self>) -> AnyElement {
        let key = |id: &str, path: &[usize]| SharedString::from(format!("plugin-{plugin}-{id}-{path:?}"));
        match node {
            Node::Column { children, gap: g } => {
                let fills = node.fills_height();
                self.render_plugin_children(
                    plugin,
                    children,
                    path,
                    div().flex().flex_col().gap(gap(*g)).min_w_0().when(fills, |d| d.flex_1().min_h_0()),
                    cx,
                )
                .into_any_element()
            }
            Node::Section { title, children } => div()
                .pt_3()
                .flex()
                .flex_col()
                .gap_2()
                .min_w_0()
                // One line as wide as the section: measured alone it wrapped a character a line.
                .child(cut_line(div().t_caption().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::MUTED)), title.to_uppercase()))
                .child(self.render_plugin_children(plugin, children, path, div().flex().flex_col().gap_2().min_w_0(), cx))
                .into_any_element(),
            Node::Row { children, gap: g, wrap } => {
                let mut row = div().flex().items_center().gap(gap(*g)).min_w_0().when(*wrap, |d| d.flex_wrap());
                for (index, child) in children.iter().enumerate() {
                    path.push(index);
                    let rendered = self.render_plugin_node(plugin, child, path, cx);
                    row = row.child(match child {
                        // A drop-down beside a field is a control, not the field: it takes the width
                        // of its longest option (an HTTP method beside a URL), not half the row.
                        Node::Select { options, placeholder, .. } => {
                            div().flex_shrink_0().w(px(select_width(options, placeholder.as_deref()))).child(rendered).into_any_element()
                        }
                        _ if fills_row(child) => div().flex_1().min_w_0().child(rendered).into_any_element(),
                        // A checkbox keeps its label on one line: squeezed by a text beside it,
                        // the label wrapped a letter a line.
                        Node::Checkbox { .. } => div().flex_shrink_0().child(rendered).into_any_element(),
                        // A heading is as wide as its words, up to most of the row.
                        Node::Text { .. } => div().flex_shrink_0().max_w(relative(0.6)).min_w_0().child(rendered).into_any_element(),
                        _ => rendered,
                    });
                    path.pop();
                }
                row.into_any_element()
            }
            Node::Text { text, style } => {
                let base = div().min_w_0().whitespace_normal();
                match style {
                    // One line, cut with an ellipsis: a heading wrapped in a narrow place broke a
                    // letter a line.
                    TextStyle::Title => {
                        // Cut in a flex line of its own: a cut line measured straight in a column
                        // has no width and shows only its ellipsis.
                        return cut_line(div().t_large().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)), text.clone())
                            .into_any_element();
                    }
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
            Node::Button { id, label, icon: glyph, variant, disabled, menu } => {
                let (bg, hover, border, fg) = match variant {
                    Variant::Primary => (hex(Chrome::ACCENT), lighten(Chrome::ACCENT, 0.12), hex(Chrome::ACCENT), hex(Chrome::BRIGHT)),
                    Variant::Secondary => (hex(CONTROL), lighten(CONTROL, 0.06), hex(Chrome::OVERLAY_BORDER), hex(Chrome::BRIGHT)),
                    Variant::Ghost => (hex_alpha(0, 0.), hex(Chrome::HOVER), hex_alpha(0, 0.), hex(Chrome::FOREGROUND)),
                    Variant::Danger => {
                        (hex_alpha(Chrome::ERROR, 0.14), hex_alpha(Chrome::ERROR, 0.24), hex_alpha(Chrome::ERROR, 0.4), hex(Chrome::ERROR))
                    }
                };
                // An icon alone is a square: the padding of a labelled button spread icons apart.
                let icon_only = label.trim().is_empty() && glyph.is_some();
                let menu_key = format!("button:{plugin}/{id}");
                let has_menu = !menu.is_empty();
                let button = div()
                    .id(key(id, path))
                    .flex_shrink_0()
                    .h(px(CONTROL_HEIGHT))
                    .when(icon_only, |d| d.w(px(CONTROL_HEIGHT)))
                    .when(!icon_only, |d| d.px_3())
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
                    .when(!*disabled && !has_menu, |d| {
                        d.cursor_pointer().hover(move |s| s.bg(hover)).on_click(cx.listener(emit(
                            plugin.to_string(),
                            id.clone(),
                            "click",
                            None,
                            None,
                        )))
                    })
                    .when(!*disabled && has_menu, |d| {
                        d.cursor_pointer().hover(move |s| s.bg(hover)).on_click(cx.listener(Self::toggle_plugin_menu(menu_key.clone())))
                    })
                    .children(glyph.as_deref().map(|g| icon(icon_named(Some(g)), IconSize::INLINE, fg)))
                    .when(!icon_only, |d| d.child(label.clone()))
                    .when(has_menu && !icon_only, |d| d.child(icon("chevron-down", IconSize::INLINE, fg)));
                if !has_menu {
                    return button.into_any_element();
                }
                // A button with a menu: picking an entry sends `select` with its value.
                let entries = menu
                    .iter()
                    .map(|o| {
                        (
                            o.label.clone(),
                            None,
                            UiEvent {
                                element: id.clone(),
                                event: "select".into(),
                                value: Some(o.value.clone().into()),
                                item: None,
                                action: None,
                            },
                        )
                    })
                    .collect();
                div()
                    .relative()
                    .flex_shrink_0()
                    .child(button)
                    .children(self.plugin_menu(plugin, &menu_key, entries, false, cx))
                    .into_any_element()
            }
            Node::Input { id, rows, mono, language, .. } => {
                let scope = self.plugin_input_scope(plugin, cx);
                let Some(field) = self.plugin_inputs.get(&(scope.clone(), id.clone())) else { return div().into_any_element() };
                // A text area of code is colored as it is typed (the colors are kept per text, so
                // an unchanged one costs a lookup).
                if let Some(language) = language.as_deref().filter(|_| *rows > 1) {
                    let text = field.input.read(cx).text().to_string();
                    let colors = self.code_colors(&text, Some(language));
                    field.input.update(cx, |input, cx| input.set_colors(colors, cx));
                }
                let boxed = div()
                    .w_full()
                    .when(*mono, |d| d.font_family(MONO))
                    .px_2p5()
                    .when(*rows <= 1, |d| d.min_h(px(CONTROL_HEIGHT)).flex().items_center())
                    .when(*rows > 1, |d| d.py_1p5().flex().flex_col())
                    .overflow_hidden()
                    .rounded_md()
                    .border_1()
                    .border_color(hex(Chrome::OVERLAY_BORDER))
                    .bg(hex(SUNKEN))
                    .t_small()
                    .text_color(hex(Chrome::BRIGHT))
                    // A column, so the field is stretched to the box: measured on its own a text
                    // area had no width and wrapped a letter a line.
                    .child(div().w_full().min_w_0().flex().flex_col().child(field.input.clone()));
                let Some(suggest) = field.suggest.as_ref() else { return boxed.into_any_element() };
                let mut menu = div()
                    .id(SharedString::from(format!("plugin-suggest-{plugin}-{id}")))
                    .w(px(380.))
                    .p_1()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .rounded_md()
                    .border_1()
                    .border_color(hex(Chrome::OVERLAY_BORDER))
                    .bg(hex(RAISED))
                    .shadow_lg()
                    .occlude();
                for (row, index) in suggest.items.iter().enumerate() {
                    let Some(completion) = field.completions.get(*index) else { continue };
                    let active = row == suggest.selected;
                    let key = (scope.clone(), id.clone());
                    menu = menu.child(
                        div()
                            .id(SharedString::from(format!("plugin-suggest-{plugin}-{id}-{row}")))
                            .h(px(26.))
                            .px_2()
                            .flex()
                            .items_center()
                            .gap_3()
                            .rounded_sm()
                            .cursor_pointer()
                            .when(active, |d| d.bg(hex_alpha(Chrome::ACCENT, 0.28)))
                            .hover(|s| s.bg(hex(Chrome::HOVER)))
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(move |this, _, window, cx| {
                                    let input = this.plugin_inputs.get(&key).map(|f| f.input.clone());
                                    this.apply_plugin_completion(&key, row, cx);
                                    if let Some(input) = input {
                                        window.focus(&gpui::Focusable::focus_handle(&input, cx));
                                    }
                                    cx.stop_propagation();
                                }),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .max_w(px(200.))
                                    .truncate()
                                    .font_family(MONO)
                                    .t_small()
                                    .text_color(hex(Chrome::BRIGHT))
                                    .child(completion.label.clone()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_right()
                                    .t_caption()
                                    .text_color(hex(Chrome::MUTED))
                                    .child(completion.detail.clone().unwrap_or_default()),
                            ),
                    );
                }
                let anchored = gpui::anchored().anchor(gpui::Corner::TopLeft).snap_to_window_with_margin(px(8.)).child(menu);
                div()
                    .w_full()
                    .relative()
                    .child(boxed)
                    .child(div().absolute().left_0().top(px(CONTROL_HEIGHT + 4.)).child(self.overlay(anchored)))
                    .into_any_element()
            }
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
                    // Its "more" menu open, the row keeps its buttons while the pointer is on the menu.
                    let menu_open = self.plugin_tab_menu.as_deref() == Some(format!("row:{plugin}/{id}/{}", item.id).as_str());
                    // The first few on the row, the rest behind "more".
                    let inline = if item.actions.len() > INLINE_ROW_ACTIONS + 1 { INLINE_ROW_ACTIONS } else { item.actions.len() };
                    for (action_index, action) in item.actions.iter().enumerate().take(inline) {
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
                    if inline < item.actions.len() {
                        let menu_key = format!("row:{plugin}/{id}/{}", item.id);
                        let entries = item.actions[inline..]
                            .iter()
                            .map(|a| {
                                let label = a.label.clone().or_else(|| a.tooltip.clone()).unwrap_or_else(|| a.id.clone());
                                let event = UiEvent {
                                    element: id.clone(),
                                    event: "action".into(),
                                    value: None,
                                    item: Some(item.id.clone()),
                                    action: Some(a.id.clone()),
                                };
                                (label, a.icon.clone(), event)
                            })
                            .collect();
                        actions = actions.child(
                            div()
                                .relative()
                                .child(
                                    div()
                                        .id(SharedString::from(format!("plugin-row-more-{plugin}-{id}-{index}")))
                                        .px_1()
                                        .h(px(22.))
                                        .flex()
                                        .items_center()
                                        .rounded_sm()
                                        .cursor_pointer()
                                        .hover(|s| s.bg(hex(Chrome::SELECTED)))
                                        .on_click(cx.listener(Self::toggle_plugin_menu(menu_key.clone())))
                                        .child(icon("ellipsis", IconSize::INLINE, hex(Chrome::FOREGROUND))),
                                )
                                .children(self.plugin_menu(plugin, &menu_key, entries, true, cx)),
                        );
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
                            .when(item.selected, |d| d.bg(hex_alpha(Chrome::ACCENT, 0.22)))
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
                            .when(has_actions, |d| {
                                d.child(if menu_open { actions } else { actions.invisible().group_hover(group, |s| s.visible()) })
                            }),
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
            Node::Flow { id, steps, reorderable, insert_menu } => {
                // Cards top to bottom, each joined to the next by a short line under its icon: the
                // line is faint where the step it leads to is off.
                let flow_key = format!("{plugin}/{id}");
                let mut flow = div().flex().flex_col().min_w_0();
                for (index, step) in steps.iter().enumerate() {
                    if index > 0 {
                        let lit = step.state != FlowState::Off && steps[index - 1].state != FlowState::Off;
                        flow = flow.child(self.flow_gap(plugin, id, &steps[index - 1].id, index, lit, insert_menu, cx));
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
                            .when(*reorderable, |d| {
                                let drag = PluginFlowDrag { flow: flow_key.clone(), step: step.id.clone(), title: step.title.clone() };
                                let (owner, element, target, key) = (plugin.to_string(), id.clone(), step.id.clone(), flow_key.clone());
                                d.on_drag(drag, |drag, _, _, cx| cx.new(|_| drag.clone()))
                                    .drag_over::<PluginFlowDrag>(|s, _, _, _| {
                                        s.border_color(hex(Chrome::ACCENT)).bg(hex_alpha(Chrome::ACCENT, 0.12))
                                    })
                                    .on_drop(cx.listener(move |this, drag: &PluginFlowDrag, _, cx| {
                                        if drag.flow != key || drag.step == target {
                                            return;
                                        }
                                        let event = UiEvent {
                                            element: element.clone(),
                                            event: "move".into(),
                                            value: Some(target.clone().into()),
                                            item: Some(drag.step.clone()),
                                            action: None,
                                        };
                                        this.send_plugin_event(&owner, event, cx);
                                    }))
                            })
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
                // After the last step too, so a block can be added at the end.
                if let Some(last) = steps.last().filter(|_| !insert_menu.is_empty()) {
                    flow = flow.child(self.flow_gap(plugin, id, &last.id, steps.len(), false, insert_menu, cx));
                }
                flow.into_any_element()
            }
            Node::Card { title, subtitle, icon: glyph, tone, children, id: card_id, closable } => {
                let close = card_id.as_ref().filter(|_| *closable).map(|card| {
                    crate::ui::icon_only(
                        SharedString::from(format!("plugin-card-close-{plugin}-{card}")),
                        "x",
                        cx.listener(emit(plugin.to_string(), card.clone(), "close", None, None)),
                    )
                });
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
                        .children(close)
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
            Node::Grid { children, columns, gap: g, widths, id: grid_id, resizable, fill, align } => {
                let columns = (*columns).max(1);
                let mut widths: Vec<GridWidth> =
                    (0..columns).map(|i| widths.get(i).map_or(GridWidth::Share(1), |w| GridWidth::parse(w))).collect();
                // The column the user drags: the first fixed one, at the width they left it.
                let handle = grid_id.as_ref().filter(|_| *resizable).and_then(|grid| {
                    let column = widths.iter().position(|w| matches!(w, GridWidth::Px(_)))?;
                    if let Some((dragged, _)) = self.plugin_grid_widths.get(&(plugin.to_string(), grid.clone())) {
                        widths[column] = GridWidth::Px(*dragged);
                    }
                    Some((column, grid.clone()))
                });
                let shares: u32 = widths.iter().map(|w| if let GridWidth::Share(s) = w { *s } else { 0 }).sum::<u32>().max(1);
                // A fixed column keeps its width; the others start at their share of the row and
                // give back what the fixed columns and gaps take. A share is a definite width, not
                // a basis of nothing: text in a cell is measured against it, where a basis of zero
                // had it wrap a character a line.
                let cell = |column: usize| match widths[column] {
                    GridWidth::Px(px_width) => div().w(px(px_width)).flex_shrink_0().min_w_0(),
                    GridWidth::Share(share) => div().flex_basis(relative(share as f32 / shares as f32)).flex_shrink().min_w_0(),
                };
                // Heights flow down by flex (a percentage of a flexed height is not settled, and
                // the columns grew with their content instead of scrolling).
                let mut grid = div().flex().flex_col().gap(gap(*g)).min_w_0().when(*fill, |d| d.flex_1().min_h_0());
                for (row_index, chunk) in children.chunks(columns).enumerate() {
                    // Filling, the first row is the panel's height and each of its cells scrolls
                    // by itself.
                    let filling = *fill && row_index == 0;
                    let mut row = div()
                        .flex()
                        .gap(gap(*g))
                        .min_w_0()
                        .when(!filling && *align == Align::Center, |d| d.items_center())
                        .when(!filling && *align != Align::Center, |d| d.items_start())
                        .when(filling, |d| d.flex_1().min_h_0());
                    for (offset, child) in chunk.iter().enumerate() {
                        path.push(row_index * columns + offset);
                        let rendered = self.render_plugin_node(plugin, child, path, cx);
                        let mut column = if filling {
                            // Tabs that fill keep their strip and scroll their content themselves.
                            let inner = if matches!(child, Node::Tabs { fill: true, .. }) {
                                div().flex_1().min_h_0().flex().flex_col().child(rendered).into_any_element()
                            } else {
                                let scroll_id = SharedString::from(format!("plugin-grid-scroll-{plugin}-{path:?}"));
                                // A block, like a tab's scrolling content: as a flex column its text was
                                // measured at no width and wrapped a letter a line.
                                div()
                                    .id(scroll_id)
                                    .flex_1()
                                    .min_h_0()
                                    .w_full()
                                    .overflow_y_scroll()
                                    .flex()
                                    .flex_col()
                                    .child(div().w_full().flex_shrink_0().flex().flex_col().child(rendered))
                                    .into_any_element()
                            };
                            cell(offset).min_h_0().flex().flex_col().child(inner)
                        } else {
                            cell(offset).flex().flex_col().child(rendered)
                        };
                        if let Some((resized, grid)) = handle.as_ref().filter(|(c, _)| row_index == 0 && *c == offset) {
                            let current = match widths[*resized] {
                                GridWidth::Px(w) => w,
                                GridWidth::Share(_) => RESIZE_RANGE.0,
                            };
                            column = column.relative().child(self.grid_resize_handle(plugin, grid, current, gap(*g), cx));
                        }
                        row = row.child(column);
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
            Node::Tabs { id, tabs, value, children, fill, add_menu } => {
                // Tabs keep their size and the strip scrolls sideways when they do not fit: by
                // dragging it, by the wheel either way, and to a tab when it is picked.
                let strip_key = format!("{plugin}/{id}");
                let scroll = {
                    let mut strips = self.plugin_tab_strips.borrow_mut();
                    let entry = strips.entry(strip_key.clone()).or_insert_with(|| (gpui::ScrollHandle::new(), String::new()));
                    if entry.1 != *value {
                        entry.1 = value.clone();
                        if let Some(index) = tabs.iter().position(|t| t.id == *value) {
                            entry.0.scroll_to_item(index);
                        }
                    }
                    entry.0.clone()
                };
                let (wheel, dragged) = (scroll.clone(), scroll.clone());
                let mut strip = div()
                    .id(SharedString::from(format!("plugin-tab-strip-{plugin}-{id}")))
                    .flex()
                    .items_end()
                    .gap_1()
                    .flex_1()
                    .min_w_0()
                    .overflow_x_scroll()
                    .track_scroll(&scroll)
                    .on_scroll_wheel(move |event, window, _| {
                        // A mouse wheel only turns up and down: that moves the strip sideways.
                        let delta = event.delta.pixel_delta(window.line_height());
                        if delta.y.abs() > delta.x.abs() {
                            let offset = wheel.offset();
                            let max = wheel.max_offset().width;
                            wheel.set_offset(gpui::point((offset.x + delta.y).clamp(-max, px(0.)), offset.y));
                            window.refresh();
                        }
                    })
                    .on_drag(PluginTabStripDrag { strip: strip_key.clone(), last_x: Rc::default() }, |drag, _, _, cx| {
                        cx.new(|_| drag.clone())
                    })
                    .on_drag_move(cx.listener(move |_, event: &gpui::DragMoveEvent<PluginTabStripDrag>, window, cx| {
                        // Follows the pointer: the strip moves by how far it went since the last move.
                        let drag = event.drag(cx);
                        if drag.strip != strip_key {
                            return;
                        }
                        let x = f32::from(event.event.position.x);
                        if let Some(last) = drag.last_x.replace(Some(x)) {
                            let offset = dragged.offset();
                            let max = dragged.max_offset().width;
                            dragged.set_offset(gpui::point((offset.x + px(x - last)).clamp(-max, px(0.)), offset.y));
                            window.refresh();
                        }
                    }));
                for (index, tab) in tabs.iter().enumerate() {
                    let active = tab.id == *value;
                    let fg = if active { Chrome::BRIGHT } else { Chrome::MUTED };
                    strip = strip.child(
                        div()
                            .id(SharedString::from(format!("plugin-tab-{plugin}-{id}-{index}")))
                            .px_2p5()
                            .py_1p5()
                            .flex()
                            .flex_shrink_0()
                            .max_w(px(220.))
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
                let menu_key = format!("tab:{plugin}/{id}");
                let add = (!add_menu.is_empty()).then(|| {
                    let open = self.plugin_tab_menu.as_deref() == Some(menu_key.as_str());
                    let entries = add_menu
                        .iter()
                        .map(|o| {
                            (
                                o.label.clone(),
                                None,
                                UiEvent {
                                    element: id.clone(),
                                    event: "add".into(),
                                    value: Some(o.value.clone().into()),
                                    item: None,
                                    action: None,
                                },
                            )
                        })
                        .collect();
                    div()
                        .relative()
                        .flex_shrink_0()
                        .child(
                            div()
                                .id(SharedString::from(format!("plugin-tab-add-{plugin}-{id}")))
                                .size(px(CONTROL_HEIGHT))
                                .mb_0p5()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_md()
                                .cursor_pointer()
                                .when(open, |d| d.bg(hex(Chrome::HOVER)))
                                .hover(|s| s.bg(hex(Chrome::HOVER)))
                                .on_click(cx.listener(Self::toggle_plugin_menu(menu_key.clone())))
                                .child(icon("plus", IconSize::INLINE, hex(Chrome::FOREGROUND))),
                        )
                        .children(self.plugin_menu(plugin, &menu_key, entries, true, cx))
                });
                let strip = div()
                    .h(px(TAB_STRIP_HEIGHT))
                    .flex_shrink_0()
                    .flex()
                    .items_end()
                    .gap_1()
                    .min_w_0()
                    .border_b_1()
                    .border_color(hex(Chrome::BORDER))
                    .child(strip)
                    .children(add);
                let content_fills = children.iter().any(Node::fills_height);
                let content = self.render_plugin_children(
                    plugin,
                    children,
                    path,
                    div().flex().flex_col().gap_2().min_w_0().when(content_fills, |d| d.flex_1().min_h_0()),
                    cx,
                );
                if *fill && content_fills {
                    // A page that splits itself into columns scrolls them one by one.
                    return div()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .min_w_0()
                        .child(strip.flex_shrink_0())
                        .child(div().flex_1().min_h_0().flex().flex_col().child(content))
                        .into_any_element();
                }
                if *fill {
                    // The strip stays; what is under it scrolls.
                    let scroll_id = SharedString::from(format!("plugin-tabs-scroll-{plugin}-{id}"));
                    return div()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .min_w_0()
                        .child(strip.flex_shrink_0())
                        // A column holding a full-width wrapper: as a block, a short page (a sidebar
                        // list) was laid out at its own width and its search box shrank to nothing.
                        .child(
                            div()
                                .id(scroll_id)
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scroll()
                                .pr_1()
                                .flex()
                                .flex_col()
                                .child(div().w_full().flex_shrink_0().flex().flex_col().child(content)),
                        )
                        .into_any_element();
                }
                div().flex().flex_col().gap_3().min_w_0().child(strip).child(content).into_any_element()
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
                                div()
                                    .w(relative(0.38))
                                    .max_w(px(200.))
                                    .flex_shrink_0()
                                    .t_small()
                                    .text_color(hex(Chrome::MUTED))
                                    .child(item.label.clone()),
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
                                // Pressing an open drop-down closes it: the press outside its list
                                // already did, and this click is the same press.
                                let menu_key = format!("select:{}/{}", toggle_key.0, toggle_key.1);
                                let just_closed = this
                                    .plugin_menu_closed
                                    .take()
                                    .is_some_and(|(k, at)| k == menu_key && at.elapsed() < Duration::from_millis(400));
                                this.plugin_select_open = if just_closed || this.plugin_select_open.as_ref() == Some(&toggle_key) {
                                    None
                                } else {
                                    Some(toggle_key.clone())
                                };
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
                            if let Some((scope, id)) = this.plugin_select_open.take() {
                                this.plugin_menu_closed = Some((format!("select:{scope}/{id}"), Instant::now()));
                            }
                            cx.notify();
                        }))
                        .child(menu);
                    let anchored =
                        gpui::anchored().anchor(gpui::Corner::TopLeft).snap_to_window_with_margin(px(8.)).child(div().mt_1().child(card));
                    div().absolute().top(px(CONTROL_HEIGHT)).left_0().child(self.overlay(anchored))
                });
                // A column, so the trigger is stretched: as a block its width was not settled when
                // the label was laid out, and the arrow sat right after the words.
                div().relative().w_full().min_w_0().flex().flex_col().child(trigger).children(menu).into_any_element()
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
                    // Says so: a copy button that does nothing visible looks broken.
                    cx.listener(move |this, _: &ClickEvent, _, cx| {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(copied.clone()));
                        this.show_toast(t(cx, "plugins.ui.copied").to_string(), cx);
                    }),
                )
                .tooltip(Tooltip::text(t(cx, "plugins.ui.copy"), None));
                let caption = title.clone().or_else(|| language.clone());
                let group = SharedString::from(format!("plugin-code-{plugin}-{path:?}"));
                // Long lines scroll sideways: a trackpad, Shift and the wheel, or dragging the code.
                let scroll_key = format!("code:{plugin}/{path:?}");
                let scroll = {
                    let mut strips = self.plugin_tab_strips.borrow_mut();
                    let entry = strips.entry(scroll_key.clone()).or_insert_with(|| (gpui::ScrollHandle::new(), String::new()));
                    // Other code in the same place (another language picked) starts at its left edge.
                    let stamp = format!("{}:{}", text.len(), text.chars().take(64).collect::<String>());
                    if entry.1 != stamp {
                        entry.1 = stamp;
                        entry.0.set_offset(gpui::point(px(0.), px(0.)));
                    }
                    entry.0.clone()
                };
                let dragged = scroll.clone();
                let bar_handle = scroll.clone();
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
                        // A bar along the bottom while the pointer is over a block wider than its
                        // place: dragged with any mouse.
                        div()
                            .relative()
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .group(crate::ui::SCROLL_GROUP)
                            .child(
                                div()
                                    .id(SharedString::from(format!("plugin-code-body-{plugin}-{path:?}")))
                                    .px_3()
                                    .py_2p5()
                                    // As wide as the block, so a long line scrolls inside it (a trackpad
                                    // sideways, a wheel with Shift) instead of widening the block.
                                    .w_full()
                                    .min_w_0()
                                    .overflow_x_scroll()
                                    .track_scroll(&scroll)
                                    .cursor(gpui::CursorStyle::OpenHand)
                                    .on_drag(PluginTabStripDrag { strip: scroll_key.clone(), last_x: Rc::default() }, |drag, _, _, cx| {
                                        cx.new(|_| drag.clone())
                                    })
                                    .on_drag_move(cx.listener(move |_, event: &gpui::DragMoveEvent<PluginTabStripDrag>, window, cx| {
                                        let drag = event.drag(cx);
                                        if drag.strip != scroll_key {
                                            return;
                                        }
                                        let x = f32::from(event.event.position.x);
                                        if let Some(last) = drag.last_x.replace(Some(x)) {
                                            let offset = dragged.offset();
                                            let max = dragged.max_offset().width;
                                            dragged.set_offset(gpui::point((offset.x + px(x - last)).clamp(-max, px(0.)), offset.y));
                                            window.refresh();
                                        }
                                    }))
                                    .font_family(MONO)
                                    .t_small()
                                    .text_color(hex(highlight::FOREGROUND))
                                    // A row whose one child is as wide as its longest line: text
                                    // spilling out of a box does not widen it, and the block saw
                                    // nothing to scroll.
                                    .flex()
                                    .child(div().flex_shrink_0().whitespace_nowrap().child(styled)),
                            )
                            .child(crate::ui::scrollbar_h(bar_handle)),
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
            // Drawn beside the panel, not in it: in a column it would add a gap for nothing.
            if matches!(child, Node::Popover { .. }) {
                continue;
            }
            path.push(index);
            let rendered = self.render_plugin_node(plugin, child, path, cx);
            if child.fills_height() {
                container = container.child(div().flex_1().min_h_0().flex().flex_col().child(rendered));
                path.pop();
                continue;
            }
            // Text takes the column's width. Measured on its own in a narrow place (a grid cell, a
            // card in a sidebar) it shrinks to its narrowest wrap — a character a line for Korean,
            // Japanese and Chinese — and spills into the column beside it.
            container = container.child(if matches!(child, Node::Text { .. }) {
                div().w_full().min_w_0().child(rendered).into_any_element()
            } else {
                rendered
            });
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
