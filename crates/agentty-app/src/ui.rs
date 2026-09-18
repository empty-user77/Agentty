//! Small shared UI building blocks and formatting helpers.

use crate::launch::PaneKind;
use crate::theme::{hex, hex_alpha, Chrome};
use gpui::{div, prelude::*, px, svg, AnimationExt, App, ClickEvent, Div, ElementId, Hsla, SharedString, Stateful, Styled, Svg, Window};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Type scale (px). Every UI string uses one of these.
pub struct Type;

impl Type {
    /// Section headers, badges.
    pub const CAPTION: f32 = 11.0;
    /// Secondary lines, metadata, buttons.
    pub const SMALL: f32 = 12.0;
    /// Default UI text (VS Code's base size).
    pub const BODY: f32 = 13.0;
    /// Card and dialog titles.
    pub const TITLE: f32 = 14.0;
    pub const LARGE: f32 = 16.0;
    pub const HEADING: f32 = 18.0;
    pub const DISPLAY: f32 = 22.0;
}

/// Icon sizes (px).
pub struct IconSize;

impl IconSize {
    pub const INLINE: f32 = 14.0;
    pub const BUTTON: f32 = 16.0;
    pub const ACTIVITY: f32 = 20.0;
}

/// Hit area of icon-only buttons.
pub const ICON_BUTTON: f32 = 26.0;

/// Every icon referenced by name (checked by a test against the embedded assets).
pub const ICONS: &[&str] = &[
    "arrow-up-right",
    "bell",
    "bell-dot",
    "blocks",
    "brain",
    "chart-column",
    "chevron-down",
    "chevron-right",
    "columns-2",
    "ellipsis",
    "folder",
    "folder-plus",
    "git-branch",
    "history",
    "layout-panel-left",
    "pencil",
    "plus",
    "refresh-cw",
    "rows-2",
    "settings",
    "star",
    "terminal",
    "workflow",
    "x",
    "chevron-up",
    "git-commit-horizontal",
    "check",
    "arrow-down",
    "arrow-up",
    "undo-2",
    "picture-in-picture-2",
    "search",
    "command",
    "sparkles",
    "bot",
    "globe",
    "arrow-left",
    "arrow-right",
    "rotate-cw",
    "package",
    "loader-circle",
    "maximize-2",
    "minimize-2",
    "grip-vertical",
    "panel-left-close",
    "panel-left-open",
    "message-circle-question",
    "shield-alert",
    "circle-pause",
    "link",
    "unlink",
    "users",
    "file-text",
    "square-plus",
    "folder-open",
    "copy",
    "info",
    "external-link",
    "square-terminal",
    "network",
    "list-tree",
    "circle-check",
    "circle-x",
    "clock",
    "app-window",
    "puzzle",
    "notebook-pen",
    "send",
    "save",
    "file-input",
    "trash-2",
    "download",
    "upload",
    "power",
    "scroll-text",
    "zap",
    "wand-sparkles",
    "code",
    "file-plus",
    "list",
    "message-square",
    "notebook",
    "sticky-note",
    "bookmark",
    "calendar",
    "tag",
    "clipboard",
    "clipboard-paste",
    "git-pull-request",
    "bug",
    "rocket",
    "book-open",
    "hammer",
    "wrench",
    "database",
    "cloud",
    "eye",
    "play",
    "square",
    "lightbulb",
    "plug",
    "hash",
    "house",
    "at-sign",
    "mail",
    "image",
];

/// An icon named at runtime (plugins): the matching embedded icon, or a generic one.
pub fn icon_named(name: Option<&str>) -> &'static str {
    name.and_then(|name| ICONS.iter().find(|known| **known == name).copied()).unwrap_or("puzzle")
}

pub trait TypeScale: Styled + Sized {
    fn t_caption(self) -> Self {
        self.text_size(px(Type::CAPTION))
    }
    fn t_small(self) -> Self {
        self.text_size(px(Type::SMALL))
    }
    fn t_body(self) -> Self {
        self.text_size(px(Type::BODY))
    }
    fn t_title(self) -> Self {
        self.text_size(px(Type::TITLE))
    }
    fn t_large(self) -> Self {
        self.text_size(px(Type::LARGE))
    }
    fn t_heading(self) -> Self {
        self.text_size(px(Type::HEADING))
    }
    fn t_display(self) -> Self {
        self.text_size(px(Type::DISPLAY))
    }
}

impl<T: Styled + Sized> TypeScale for T {}

/// A Lucide icon tinted with `color`.
pub fn icon(name: &'static str, size: f32, color: Hsla) -> Svg {
    debug_assert!(ICONS.contains(&name), "icon {name} is not in ICONS");
    svg().path(SharedString::from(format!("icons/{name}.svg"))).flex_shrink_0().size(px(size)).text_color(color)
}

/// Square icon-only button.
pub fn icon_only(
    id: impl Into<ElementId>,
    name: &'static str,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex_shrink_0()
        .size(px(ICON_BUTTON))
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .cursor_pointer()
        .hover(|s| s.bg(hex(Chrome::HOVER)))
        .on_click(on_click)
        .child(icon(name, IconSize::BUTTON, hex(Chrome::FOREGROUND)))
}

/// Hover tooltip for icon-only buttons: a short name and its shortcut, shown after ~1.2 s.
pub struct Tooltip {
    text: SharedString,
    shortcut: Option<SharedString>,
    visible: bool,
}

impl Tooltip {
    /// Builder for `.tooltip(...)`.
    pub fn text(
        text: impl Into<SharedString>,
        shortcut: Option<&'static str>,
    ) -> impl Fn(&mut Window, &mut App) -> gpui::AnyView + 'static {
        let text = text.into();
        move |_, cx| {
            let (text, shortcut) = (text.clone(), shortcut.map(SharedString::from));
            cx.new(|cx| {
                // GPUI shows tooltips after 0.5 s; wait a little longer so passing over icons stays quiet.
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(std::time::Duration::from_millis(700)).await;
                    let _ = this.update(cx, |this: &mut Tooltip, cx| {
                        this.visible = true;
                        cx.notify();
                    });
                })
                .detach();
                Tooltip { text, shortcut, visible: false }
            })
            .into()
        }
    }
}

impl Render for Tooltip {
    fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        div().when(self.visible, |d| {
            d.child(
                div()
                    .mt_1()
                    .px_2()
                    .py_1()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_md()
                    .bg(hex(Chrome::OVERLAY))
                    .border_1()
                    .border_color(hex(Chrome::OVERLAY_BORDER))
                    .shadow_md()
                    .t_small()
                    .text_color(hex(Chrome::BRIGHT))
                    .child(self.text.clone())
                    .children(self.shortcut.clone().map(|keys| div().text_color(hex(Chrome::MUTED)).child(keys))),
            )
        })
    }
}

/// Fades a popover in (short, so menus still feel instant).
pub fn fade_in(id: impl Into<ElementId>, element: impl IntoElement + 'static) -> impl IntoElement {
    div().child(element).with_animation(
        id,
        gpui::Animation::new(std::time::Duration::from_millis(120)).with_easing(gpui::ease_out_quint()),
        |d, delta| d.opacity(delta).mt(px((1. - delta) * -4.)),
    )
}

pub fn kind_color(kind: PaneKind) -> u32 {
    match kind {
        PaneKind::Shell => Chrome::SHELL,
        PaneKind::Claude => Chrome::CLAUDE,
        PaneKind::Codex => Chrome::CODEX,
    }
}

#[allow(dead_code)]
pub fn icon_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex_shrink_0()
        .px_1p5()
        .py_0p5()
        .rounded_sm()
        .cursor_pointer()
        .t_small()
        .text_color(hex(Chrome::FOREGROUND))
        .hover(|s| s.bg(hex(Chrome::HOVER)))
        .on_click(on_click)
        .child(label.into())
}

pub fn action_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex_shrink_0()
        .px_2()
        .py_0p5()
        .rounded_sm()
        .t_small()
        .cursor_pointer()
        .bg(hex(0x2d2d30))
        .text_color(hex(Chrome::FOREGROUND))
        .hover(|s| s.bg(hex(Chrome::ACCENT)).text_color(hex(Chrome::BRIGHT)))
        .on_click(on_click)
        .child(label.into())
}

/// Segmented-control style option.
pub fn chip(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    active: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id(id)
        .px_2p5()
        .py_1()
        .rounded_md()
        .t_small()
        .cursor_pointer()
        .border_1()
        .border_color(if active { hex(Chrome::ACCENT) } else { hex(Chrome::BORDER) })
        .bg(if active { hex_alpha(Chrome::ACCENT, 0.25) } else { hex_alpha(0, 0.) })
        .text_color(if active { hex(Chrome::BRIGHT) } else { hex(Chrome::MUTED) })
        .hover(|s| s.text_color(hex(Chrome::BRIGHT)))
        .on_click(on_click)
        .child(label.into())
}

pub fn menu_item(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id(id)
        .px_3()
        .py_1()
        .rounded_md()
        .t_body()
        .cursor_pointer()
        .text_color(hex(Chrome::FOREGROUND))
        .hover(|s| s.bg(hex(Chrome::ACCENT)).text_color(hex(Chrome::BRIGHT)))
        .on_click(on_click)
        .child(label.into())
}

/// Floating panel for menus and dropdowns. It blocks the mouse for what's underneath, so hovering
/// and scrolling it never reach the terminal or list behind it.
pub fn popover() -> Div {
    div()
        .occlude()
        .p_1()
        .flex()
        .flex_col()
        .bg(hex(Chrome::OVERLAY))
        .border_1()
        .border_color(hex(Chrome::OVERLAY_BORDER))
        .rounded_lg()
        .shadow_lg()
        .text_color(hex(Chrome::FOREGROUND))
}

/// Spinner + text, for anything loading.
pub fn loading_row(text: impl Into<SharedString>) -> Div {
    div()
        .px_3()
        .py_2()
        .flex()
        .items_center()
        .gap_2()
        .t_small()
        .text_color(hex(Chrome::MUTED))
        .child(spinner(IconSize::INLINE, hex(Chrome::MUTED)))
        .child(text.into())
}

/// Rotating loader icon.
pub fn spinner(size: f32, color: Hsla) -> impl IntoElement {
    icon("loader-circle", size, color).with_animation(
        "spinner",
        gpui::Animation::new(std::time::Duration::from_millis(900)).repeat(),
        |svg, delta| svg.with_transformation(gpui::Transformation::rotate(gpui::percentage(delta))),
    )
}

pub fn hint(text: impl Into<SharedString>) -> Div {
    div().px_3().py_2().t_small().text_color(hex(Chrome::MUTED)).child(text.into())
}

pub fn tilde(path: &Path) -> String {
    let home = crate::launch::home_dir();
    match path.strip_prefix(&home) {
        // The home folder itself reads better in full than as a lone "~".
        Ok(rest) if rest.as_os_str().is_empty() => home.display().to_string(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

pub fn relative_time(now: u64, then: u64) -> String {
    let minutes = now.saturating_sub(then) / 60_000;
    match minutes {
        0 => "now".into(),
        1..=59 => format!("{minutes}m"),
        60..=1439 => format!("{}h", minutes / 60),
        _ => format!("{}d", minutes / 1440),
    }
}

pub fn compact_number(value: u64) -> String {
    match value {
        0..=999 => value.to_string(),
        1_000..=999_999 => format!("{:.1}K", value as f64 / 1_000.0),
        1_000_000..=999_999_999 => format!("{:.1}M", value as f64 / 1_000_000.0),
        _ => format!("{:.2}B", value as f64 / 1_000_000_000.0),
    }
}

pub fn money(value: f64) -> String {
    if value >= 100.0 {
        format!("${value:.0}")
    } else if value >= 0.01 || value == 0.0 {
        format!("${value:.2}")
    } else {
        format!("${value:.4}")
    }
}

/// Thin overlay scrollbar for a scroll container (place as the last child of a `relative()`
/// container that tracks `handle`). Drawn at paint time from the live scroll offset, so it
/// never lags a frame behind and costs nothing when content fits.
pub fn scrollbar(handle: gpui::ScrollHandle) -> impl IntoElement {
    gpui::canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let viewport = handle.bounds();
            let max = handle.max_offset();
            let (height, max_y) = (f32::from(viewport.size.height), f32::from(max.height));
            if height <= 0. || max_y <= 0.5 {
                return;
            }
            let content = height + max_y;
            let thumb = (height * height / content).max(24.);
            let progress = (-f32::from(handle.offset().y) / max_y).clamp(0., 1.);
            let top = f32::from(viewport.origin.y) + (height - thumb) * progress;
            let rect = gpui::Bounds::new(gpui::point(bounds.right() - px(8.), px(top + 2.)), gpui::size(px(5.), px(thumb - 4.)));
            window.paint_quad(gpui::fill(rect, hex_alpha(0xffffff, 0.18)).corner_radii(px(3.)));
        },
    )
    .absolute()
    .top_0()
    .right_0()
    .h_full()
    .w(px(10.))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_numbers() {
        assert_eq!(compact_number(512_300), "512.3K");
        assert_eq!(compact_number(66_200_000), "66.2M");
        assert_eq!(money(59.0), "$59.00");
        assert_eq!(money(0.0067), "$0.0067");
    }

    #[test]
    fn relative_time_buckets() {
        let hour = 3_600_000;
        assert_eq!(relative_time(hour * 50, hour * 50), "now");
        assert_eq!(relative_time(hour * 50, hour * 49), "1h");
        assert_eq!(relative_time(hour * 50, hour * 2), "2d");
    }
}
