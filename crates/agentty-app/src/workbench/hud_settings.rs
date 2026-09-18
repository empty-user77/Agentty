//! Settings → Appearance → Status bar: which items the AI CLI status bar shows, and in which order.
//! Rows are dragged (or moved with the arrows); required items have a lock instead of a switch.

use super::settings_page::section;
use super::Workbench;
use crate::hud::{self, HudEntry, HudItem};
use crate::i18n::t;
use crate::settings::{settings, update_settings};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{action_button, icon, icon_only_sized, IconSize, Tooltip, TypeScale};
use gpui::{div, prelude::*, px, ClickEvent, Context, Div, SharedString};

/// A status bar item being dragged to another place in the list.
#[derive(Clone)]
struct DraggedHudItem {
    index: usize,
    title: SharedString,
}

fn save(layout: Vec<HudEntry>, cx: &mut gpui::App) {
    update_settings(cx, move |s| s.hud = layout);
}

impl Workbench {
    pub(super) fn render_hud_settings(&self, cx: &mut Context<Self>) -> Div {
        let layout = hud::normalized(&settings(cx).hud);

        // What the bar looks like with this layout: names in order, the spacer as the gap it is.
        let mut preview = div()
            .h(px(30.))
            .px_2()
            .flex()
            .items_center()
            .gap_1p5()
            .overflow_hidden()
            .rounded_md()
            .border_1()
            .border_color(hex(Chrome::BORDER))
            .bg(hex(Chrome::TAB_INACTIVE))
            .t_caption();
        for entry in layout.iter().filter(|e| e.visible) {
            preview = preview.child(match entry.item {
                HudItem::Spacer => div().flex_1().min_w(px(12.)).h(px(1.)).bg(hex_alpha(0xffffff, 0.12)),
                item => div()
                    .flex_shrink_0()
                    .px_1p5()
                    .rounded_sm()
                    .bg(hex_alpha(if item.required() { Chrome::ACCENT } else { 0xffffff }, if item.required() { 0.3 } else { 0.1 }))
                    .text_color(hex(Chrome::BRIGHT))
                    .child(t(cx, item.label_key())),
            });
        }

        let mut list = div().flex().flex_col().gap_0p5();
        let count = layout.len();
        for (index, entry) in layout.iter().enumerate() {
            let item = entry.item;
            let title: SharedString = t(cx, item.label_key()).into();
            let (for_up, for_down, for_toggle, for_drop) = (layout.clone(), layout.clone(), layout.clone(), layout.clone());
            list = list.child(
                div()
                    .id(("hud-row", index))
                    .h(px(32.))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_md()
                    .border_1()
                    .border_color(hex(Chrome::BORDER))
                    .bg(hex(Chrome::PANEL))
                    .cursor(gpui::CursorStyle::OpenHand)
                    .on_drag(DraggedHudItem { index, title: title.clone() }, |dragged, _, _, cx| {
                        cx.new(|_| super::chrome::DragPreview { title: dragged.title.clone() })
                    })
                    .drag_over::<DraggedHudItem>(move |style, dragged, _, _| {
                        if dragged.index == index {
                            style
                        } else if dragged.index > index {
                            style.border_t_2().border_color(hex(Chrome::ACCENT))
                        } else {
                            style.border_b_2().border_color(hex(Chrome::ACCENT))
                        }
                    })
                    .on_drop(cx.listener(move |_, dragged: &DraggedHudItem, _, cx| {
                        let mut layout = for_drop.clone();
                        if dragged.index < layout.len() && dragged.index != index {
                            let entry = layout.remove(dragged.index);
                            layout.insert(index.min(layout.len()), entry);
                            save(layout, cx);
                        }
                    }))
                    .child(icon("grip-vertical", IconSize::INLINE, hex(Chrome::MUTED)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .t_body()
                            .text_color(hex(if entry.visible { Chrome::FOREGROUND } else { Chrome::MUTED }))
                            .child(title),
                    )
                    .when(item.required(), |d| {
                        d.child(
                            div()
                                .id(("hud-required", index))
                                .flex()
                                .items_center()
                                .gap_1()
                                .t_caption()
                                .text_color(hex(Chrome::MUTED))
                                .tooltip(Tooltip::text(t(cx, "hud.required_hint"), None))
                                .child(icon("lock", 11., hex(Chrome::MUTED)))
                                .child(t(cx, "hud.required")),
                        )
                    })
                    .when(!item.required(), |d| {
                        let on = entry.visible;
                        d.child(
                            div()
                                .id(("hud-toggle", index))
                                .w(px(36.))
                                .h(px(20.))
                                .rounded_full()
                                .cursor_pointer()
                                .bg(if on { hex(Chrome::ACCENT) } else { hex(0x3c3c3c) })
                                .flex()
                                .items_center()
                                .when(on, |d| d.justify_end())
                                .px_0p5()
                                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| save(hud::toggled(&for_toggle, item), cx)))
                                .child(div().size(px(16.)).rounded_full().bg(hex(Chrome::BRIGHT))),
                        )
                    })
                    .child(
                        icon_only_sized(("hud-up", index), "arrow-up", 22., IconSize::INLINE, {
                            cx.listener(move |_, _: &ClickEvent, _, cx| save(hud::moved(&for_up, index, -1), cx))
                        })
                        .when(index == 0, |d| d.opacity(0.3))
                        .tooltip(Tooltip::text(t(cx, "hud.move_up"), None)),
                    )
                    .child(
                        icon_only_sized(("hud-down", index), "arrow-down", 22., IconSize::INLINE, {
                            cx.listener(move |_, _: &ClickEvent, _, cx| save(hud::moved(&for_down, index, 1), cx))
                        })
                        .when(index + 1 == count, |d| d.opacity(0.3))
                        .tooltip(Tooltip::text(t(cx, "hud.move_down"), None)),
                    ),
            );
        }

        section(t(cx, "hud.title"))
            .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "hud.hint")))
            .child(preview)
            .child(list)
            .child(
                div().flex().child(action_button("hud-reset", t(cx, "hud.reset"), |_: &ClickEvent, _, cx| save(hud::default_layout(), cx))),
            )
    }
}
