//! Responsive mode of the in-app browser, like Chrome's device toolbar: the page is laid out at a
//! chosen size (a device or any width × height), centred over a dark backdrop and scaled down when
//! the panel is smaller. A bar under the address bar picks the device, types the size, rotates it;
//! the right and bottom edges drag. Agents set it with `agentty browser viewport …`.

use super::Workbench;
use crate::i18n::{t, tf};
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, icon_only, TypeScale};
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, Entity, Subscription, Window};
use std::cell::Cell;
use std::rc::Rc;

/// A page size in CSS pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Viewport {
    pub width: u32,
    pub height: u32,
}

const MIN: u32 = 120;
const MAX: u32 = 3840;
/// Space around the page on the backdrop, and the drag handles' thickness.
const PAD: f32 = 16.;
const HANDLE: f32 = 12.;
/// Height of the bar under the address bar.
const BAR: f32 = 32.;

/// (id for agents, name shown, width, height) — portrait sizes in CSS pixels.
pub const DEVICES: &[(&str, &str, u32, u32)] = &[
    ("iphone-se", "iPhone SE", 375, 667),
    ("iphone-15-pro", "iPhone 15 Pro", 393, 852),
    ("iphone-15-pro-max", "iPhone 15 Pro Max", 430, 932),
    ("pixel-8", "Pixel 8", 412, 915),
    ("galaxy-s24", "Galaxy S24", 360, 780),
    ("ipad-mini", "iPad mini", 768, 1024),
    ("ipad-air", "iPad Air", 820, 1180),
    ("ipad-pro", "iPad Pro 12.9\"", 1024, 1366),
    ("laptop", "Laptop", 1280, 800),
    ("desktop", "Desktop", 1920, 1080),
];

impl Viewport {
    pub fn new(width: u32, height: u32) -> Self {
        Self { width: width.clamp(MIN, MAX), height: height.clamp(MIN, MAX) }
    }

    /// `375x667`, `375×667` or a device id (`iphone-se`, also `iPhone SE`).
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        if let Some((w, h)) = text.split_once(['x', 'X', '×']) {
            return Some(Self::new(w.trim().parse().ok()?, h.trim().parse().ok()?));
        }
        let wanted = text.to_ascii_lowercase().replace([' ', '_'], "-").replace('"', "");
        DEVICES.iter().find(|(id, ..)| *id == wanted).map(|(_, _, w, h)| Self::new(*w, *h))
    }

    /// The device this size is, in either orientation.
    pub fn device(self) -> Option<&'static str> {
        DEVICES
            .iter()
            .find(|(_, _, w, h)| (*w, *h) == (self.width, self.height) || (*h, *w) == (self.width, self.height))
            .map(|(_, name, ..)| *name)
    }

    /// A phone of the list: sites are asked for their phone pages, as the phone's browser would.
    /// (A tablet's Safari asks for desktop pages, and a size typed in is only a size.)
    pub fn is_phone(self) -> bool {
        self.device().is_some() && self.width.min(self.height) < 600
    }

    fn rotated(self) -> Self {
        Self { width: self.height, height: self.width }
    }

    /// How far the page is scaled down to fit a backdrop of `width` × `height` (never up).
    pub fn fit(self, width: f32, height: f32) -> f32 {
        let across = (width - 2. * (PAD + HANDLE)) / self.width as f32;
        let down = (height - 2. * PAD - HANDLE) / self.height as f32;
        across.min(down).clamp(0.1, 1.)
    }
}

/// Which edge a drag holds.
#[derive(Clone, Copy)]
enum Edge {
    Right,
    Bottom,
}

/// What a drag carries; nothing is drawn under the pointer.
struct EdgeDrag(Edge);

impl Render for EdgeDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// The browser panel's responsive mode.
pub struct Responsive {
    /// The size in use; `None` while the mode is off.
    pub viewport: Option<Viewport>,
    /// What turning it on again brings back.
    last: Viewport,
    width: Entity<TextInput>,
    height: Entity<TextInput>,
    pub menu: bool,
    /// An edge is being dragged (the native view is hidden meanwhile: it would swallow the mouse).
    pub dragging: bool,
    /// Where the drag started: the size, the pointer along the edge's axis and the scale, which
    /// holds until the drag ends (a scale that shrinks as the page grows would run away).
    drag_start: Option<(Viewport, f32, f32)>,
    /// Size of the backdrop at the last paint, for the scale of the next layout.
    stage: Rc<Cell<(f32, f32)>>,
    _subscriptions: [Subscription; 2],
}

impl Responsive {
    pub fn new(window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        let last = Viewport::new(DEVICES[1].2, DEVICES[1].3);
        // Nothing selected: a new field would otherwise start with all of it selected.
        let field = |value: u32, window: &mut Window, cx: &mut Context<Workbench>| {
            cx.new(|cx| {
                let mut input = TextInput::new("", "", window, cx);
                input.set_text(value.to_string(), cx);
                input
            })
        };
        let width = field(last.width, window, cx);
        let height = field(last.height, window, cx);
        // A typed size applies on Enter or when the field is left.
        let on_input = |this: &mut Workbench, _: Entity<TextInput>, event: &TextInputEvent, cx: &mut Context<Workbench>| {
            if matches!(event, TextInputEvent::Confirmed | TextInputEvent::Blurred) {
                this.apply_typed_viewport(cx);
            }
        };
        let subscriptions = [cx.subscribe(&width, on_input), cx.subscribe(&height, on_input)];
        Self {
            viewport: None,
            last,
            width,
            height,
            menu: false,
            dragging: false,
            drag_start: None,
            stage: Rc::default(),
            _subscriptions: subscriptions,
        }
    }

    /// The scale the page is drawn at (1 while it fits).
    pub fn scale(&self) -> f32 {
        if let Some((_, _, scale)) = self.drag_start {
            return scale;
        }
        let (w, h) = self.stage.get();
        match self.viewport {
            Some(v) if w > 0. && h > 0. => v.fit(w, h),
            _ => 1.,
        }
    }
}

impl Workbench {
    /// Turns responsive mode on (at the last size) or off.
    pub(super) fn toggle_responsive(&mut self, cx: &mut Context<Self>) {
        let Some(browser) = self.browser.as_ref() else { return };
        let next = match browser.responsive.viewport {
            Some(_) => None,
            None => Some(browser.responsive.last),
        };
        self.set_viewport(next, cx);
    }

    /// Sets the responsive size (`None` turns the mode off). Returns false without a browser.
    pub(super) fn set_viewport(&mut self, viewport: Option<Viewport>, cx: &mut Context<Self>) -> bool {
        let Some(browser) = self.browser.as_mut() else { return false };
        let state = &mut browser.responsive;
        state.viewport = viewport;
        state.menu = false;
        if let Some(v) = viewport {
            state.last = v;
            let (width, height) = (state.width.clone(), state.height.clone());
            if width.read(cx).text() != v.width.to_string() {
                width.update(cx, |input, cx| input.set_text(v.width.to_string(), cx));
            }
            if height.read(cx).text() != v.height.to_string() {
                height.update(cx, |input, cx| input.set_text(v.height.to_string(), cx));
            }
        }
        cx.notify();
        true
    }

    fn end_viewport_drag(&mut self, cx: &mut Context<Self>) {
        if let Some(browser) = self.browser.as_mut() {
            let state = &mut browser.responsive;
            if state.dragging || state.drag_start.is_some() {
                state.dragging = false;
                state.drag_start = None;
                cx.notify();
            }
        }
    }

    fn apply_typed_viewport(&mut self, cx: &mut Context<Self>) {
        let Some(browser) = self.browser.as_ref() else { return };
        let state = &browser.responsive;
        let Some(current) = state.viewport else { return };
        let number = |input: &Entity<TextInput>, fallback: u32| input.read(cx).text().trim().parse().unwrap_or(fallback);
        let typed = Viewport::new(number(&state.width, current.width), number(&state.height, current.height));
        // Also puts a cleaned-up value back into a field that held something else.
        self.set_viewport(Some(typed), cx);
    }

    /// The toolbar button that turns the mode on and off.
    pub(super) fn render_responsive_button(&self, on: bool, cx: &mut Context<Self>) -> AnyElement {
        icon_only("browser-responsive", "smartphone", cx.listener(|this, _: &ClickEvent, _, cx| this.toggle_responsive(cx)))
            .when(on, |d| d.bg(hex_alpha(Chrome::ACCENT, 0.3)))
            .tooltip(crate::ui::Tooltip::text(t(cx, "browser.responsive"), None))
            .into_any_element()
    }

    /// Device · width × height · rotate · scale, under the address bar.
    pub(super) fn render_responsive_bar(&self, state: &Responsive, cx: &mut Context<Self>) -> Option<AnyElement> {
        let viewport = state.viewport?;
        let field = |input: &Entity<TextInput>| {
            div()
                .w(px(52.))
                .flex_shrink_0()
                .px_1p5()
                .py_0p5()
                .rounded_sm()
                .border_1()
                .border_color(hex(Chrome::BORDER))
                .bg(hex(0x1a1a1a))
                .t_small()
                .text_color(hex(Chrome::BRIGHT))
                .child(input.clone())
        };
        let device = viewport.device().map(str::to_string).unwrap_or_else(|| t(cx, "browser.responsive_custom").to_string());
        let scale = (state.scale() * 100.).round() as u32;
        Some(
            div()
                .h(px(BAR))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .gap_2()
                .px_2()
                .overflow_hidden()
                .border_b_1()
                .border_color(hex(Chrome::BORDER))
                .bg(hex(Chrome::SIDE_BAR))
                .t_small()
                .text_color(hex(Chrome::FOREGROUND))
                .child(
                    div()
                        .id("browser-device-menu")
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_1p5()
                        .py_0p5()
                        .rounded_sm()
                        .cursor_pointer()
                        .hover(|s| s.bg(hex(Chrome::HOVER)))
                        // In a narrow browser panel the device name and the scale give way, not the fields.
                        .child(div().min_w_0().truncate().child(device))
                        .child(icon("chevron-down", crate::ui::IconSize::INLINE, hex(Chrome::MUTED)))
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            if let Some(browser) = this.browser.as_mut() {
                                browser.responsive.menu = !browser.responsive.menu;
                                cx.notify();
                            }
                        })),
                )
                .child(field(&state.width))
                .child(div().text_color(hex(Chrome::MUTED)).child("×"))
                .child(field(&state.height))
                .child(
                    icon_only(
                        "browser-rotate",
                        "rotate-cw",
                        cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.set_viewport(Some(viewport.rotated()), cx);
                        }),
                    )
                    .tooltip(crate::ui::Tooltip::text(t(cx, "browser.responsive_rotate"), None)),
                )
                .child(div().min_w_0().truncate().text_color(hex(Chrome::MUTED)).child(tf(
                    cx,
                    "browser.responsive_scale",
                    &[("n", &scale.to_string())],
                )))
                .into_any_element(),
        )
    }

    /// The device list under the bar's device button.
    pub(super) fn render_device_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let browser = self.browser.as_ref()?;
        let current = browser.responsive.viewport?;
        if !browser.responsive.menu {
            return None;
        }
        let mut list = crate::ui::popover().w(px(220.)).on_mouse_down_out(cx.listener(|this, _, _, cx| {
            if let Some(browser) = this.browser.as_mut() {
                browser.responsive.menu = false;
                cx.notify();
            }
        }));
        for (id, name, w, h) in DEVICES {
            let viewport = Viewport::new(*w, *h);
            let active = current.device() == Some(*name);
            list = list.child(
                crate::ui::menu_item(gpui::SharedString::from(format!("browser-device-{id}")), format!("{name}   {w}×{h}"), {
                    cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.set_viewport(Some(viewport), cx);
                    })
                })
                .when(active, |d| d.text_color(hex(Chrome::BRIGHT)).font_weight(gpui::FontWeight::SEMIBOLD)),
            );
        }
        Some(
            div()
                .absolute()
                .top(px(0.))
                .left(px(0.))
                .size_full()
                .flex()
                .items_start()
                .justify_center()
                .pt(px(4.))
                .child(list)
                .into_any_element(),
        )
    }

    /// The page at its responsive size: centred on a dark backdrop, scaled to fit, with the right
    /// and bottom edges to drag. `page` is the element the native view is placed over.
    pub(super) fn render_responsive_stage(&self, state: &Responsive, page: AnyElement, cx: &mut Context<Self>) -> AnyElement {
        let Some(viewport) = state.viewport else { return page };
        let scale = state.scale();
        let (w, h) = (viewport.width as f32 * scale, viewport.height as f32 * scale);
        let stage = state.stage.clone();
        let handle = |edge: Edge, id: &'static str| {
            div()
                .id(id)
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded_sm()
                .bg(hex_alpha(0xffffff, 0.06))
                .hover(|s| s.bg(hex_alpha(0xffffff, 0.14)))
                .child(div().rounded_full().bg(hex_alpha(0xffffff, 0.45)).map(|d| match edge {
                    Edge::Right => d.w(px(2.)).h(px(24.)),
                    Edge::Bottom => d.w(px(24.)).h(px(2.)),
                }))
                .map(|d| match edge {
                    Edge::Right => d.w(px(HANDLE)).h(px(h)).cursor_ew_resize(),
                    Edge::Bottom => d.w(px(w)).h(px(HANDLE)).cursor_ns_resize(),
                })
                .on_drag(EdgeDrag(edge), |drag, _, _, cx| cx.new(|_| EdgeDrag(drag.0)))
                .on_drag_move(cx.listener(move |this, event: &gpui::DragMoveEvent<EdgeDrag>, _, cx| {
                    let Some(browser) = this.browser.as_mut() else { return };
                    let state = &mut browser.responsive;
                    let edge = event.drag(cx).0;
                    let pointer = match edge {
                        Edge::Right => f32::from(event.event.position.x),
                        Edge::Bottom => f32::from(event.event.position.y),
                    };
                    state.dragging = true;
                    let (start, from, scale) = *state.drag_start.get_or_insert((viewport, pointer, scale));
                    let moved = (pointer - from) / scale;
                    // The page stays centred across and top-aligned down: the right edge grows it
                    // on both sides (twice the pointer's move), the bottom edge only downwards.
                    let next = match edge {
                        Edge::Right => Viewport::new((start.width as f32 + 2. * moved).round().max(0.) as u32, start.height),
                        Edge::Bottom => Viewport::new(start.width, (start.height as f32 + moved).round().max(0.) as u32),
                    };
                    this.set_viewport(Some(next), cx);
                }))
        };
        div()
            .relative()
            .size_full()
            .bg(hex(0x2b2b2e))
            .child(
                gpui::canvas(|_, _, _| {}, move |bounds, _, _, _| stage.set((f32::from(bounds.size.width), f32::from(bounds.size.height))))
                    .absolute()
                    .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .size_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .pt(px(PAD))
                    .child(
                        div()
                            .flex()
                            // A spacer as wide as the handle keeps the page itself in the middle.
                            .child(div().w(px(HANDLE)).flex_shrink_0())
                            .child(div().w(px(w)).h(px(h)).flex_shrink_0().shadow_lg().child(page))
                            .child(handle(Edge::Right, "browser-viewport-right")),
                    )
                    .child(handle(Edge::Bottom, "browser-viewport-bottom")),
            )
            // The drag ends wherever the button comes up.
            .on_mouse_up(gpui::MouseButton::Left, cx.listener(|this, _, _, cx| this.end_viewport_drag(cx)))
            .on_mouse_up_out(gpui::MouseButton::Left, cx.listener(|this, _, _, cx| this.end_viewport_drag(cx)))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_devices() {
        assert_eq!(Viewport::parse("375x667"), Some(Viewport { width: 375, height: 667 }));
        assert_eq!(Viewport::parse(" 1024 × 768 "), Some(Viewport { width: 1024, height: 768 }));
        assert_eq!(Viewport::parse("iphone-se"), Some(Viewport { width: 375, height: 667 }));
        assert_eq!(Viewport::parse("iPad Air"), Some(Viewport { width: 820, height: 1180 }));
        assert_eq!(Viewport::parse("10x99999"), Some(Viewport { width: MIN, height: MAX }));
        assert_eq!(Viewport::parse("tv"), None);
        assert_eq!(Viewport::parse("x"), None);
        // Named in either orientation.
        assert_eq!(Viewport::new(667, 375).device(), Some("iPhone SE"));
        assert_eq!(Viewport::new(500, 500).device(), None);
    }

    #[test]
    fn scaled_down_to_fit_never_up() {
        let phone = Viewport::new(375, 667);
        assert_eq!(phone.fit(2000., 2000.), 1.);
        let desktop = Viewport::new(1920, 1080);
        let scale = desktop.fit(800., 900.);
        assert!(scale < 0.4 && 1920. * scale <= 800. - 2. * (PAD + HANDLE) + 0.01, "{scale}");
    }
}
