//! Find in terminal (⌘F): a small bar over the active pane that highlights matches in the whole
//! scrollback; Enter / ↓ next, ↑ previous, Esc closes.

use super::{Pane, Workbench};
use crate::i18n::t;
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::{hex, Chrome};
use crate::ui::{icon_only, TypeScale};
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, Entity, Focusable, Subscription, Window};

pub struct FindBar {
    pub pane: Pane,
    input: Entity<TextInput>,
    _subscription: Subscription,
}

impl Workbench {
    pub(super) fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pane) = self.active_pane() else { return };
        if let Some(bar) = self.find_bar.as_ref().filter(|b| b.pane == pane) {
            window.focus(&bar.input.focus_handle(cx));
            return;
        }
        self.close_find(window, cx);
        let input = cx.new(|cx| TextInput::new("", t(cx, "find.placeholder"), window, cx));
        let subscription = cx.subscribe_in(&input, window, |this, input, event: &TextInputEvent, window, cx| {
            let Some(pane) = this.find_bar.as_ref().map(|b| b.pane.clone()) else { return };
            match event {
                TextInputEvent::Changed => {
                    let query = input.read(cx).text().to_string();
                    pane.update(cx, |view, cx| view.set_search(&query, cx));
                }
                // Matches start at the newest; Enter and ↑ walk back through older output.
                TextInputEvent::Confirmed | TextInputEvent::Up => pane.update(cx, |view, cx| view.step_search(false, cx)),
                TextInputEvent::Down => pane.update(cx, |view, cx| view.step_search(true, cx)),
                TextInputEvent::Cancelled => this.close_find(window, cx),
                _ => {}
            }
            cx.notify();
        });
        window.focus(&input.focus_handle(cx));
        self.find_bar = Some(FindBar { pane, input, _subscription: subscription });
        cx.notify();
    }

    pub(super) fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(bar) = self.find_bar.take() {
            bar.pane.update(cx, |view, cx| view.set_search("", cx));
            self.focus_pane(&bar.pane, window, cx);
        }
        cx.notify();
    }

    pub(super) fn render_find_bar(&self, pane: &Pane, cx: &mut Context<Self>) -> Option<AnyElement> {
        let bar = self.find_bar.as_ref().filter(|b| b.pane == *pane)?;
        let position = pane.read(cx).search_position();
        let count = match position {
            Some((_, 0)) => t(cx, "find.none").to_string(),
            Some((current, total)) => format!("{current}/{total}"),
            None => String::new(),
        };
        let (prev, next) = (pane.clone(), pane.clone());
        Some(
            div()
                .absolute()
                .top(px(6.))
                .right(px(14.))
                .w(px(340.))
                .px_2()
                .py_1()
                .flex()
                .items_center()
                .gap_1()
                .rounded_md()
                .bg(hex(Chrome::OVERLAY))
                .border_1()
                .border_color(hex(Chrome::OVERLAY_BORDER))
                .shadow_lg()
                .occlude()
                .child(div().flex_1().min_w_0().t_small().text_color(hex(Chrome::BRIGHT)).child(bar.input.clone()))
                .child(div().flex_shrink_0().t_caption().text_color(hex(Chrome::MUTED)).child(count))
                .child(
                    icon_only(
                        "find-prev",
                        "chevron-up",
                        cx.listener(move |_, _: &ClickEvent, _, cx| prev.update(cx, |v, cx| v.step_search(false, cx))),
                    )
                    .tooltip(crate::ui::Tooltip::text(crate::i18n::t(cx, "tooltip.find_prev"), Some("⇧↩"))),
                )
                .child(
                    icon_only(
                        "find-next",
                        "chevron-down",
                        cx.listener(move |_, _: &ClickEvent, _, cx| next.update(cx, |v, cx| v.step_search(true, cx))),
                    )
                    .tooltip(crate::ui::Tooltip::text(crate::i18n::t(cx, "tooltip.find_next"), Some("↩"))),
                )
                .child(
                    icon_only("find-close", "x", cx.listener(|this, _: &ClickEvent, window, cx| this.close_find(window, cx)))
                        .tooltip(crate::ui::Tooltip::text(crate::i18n::t(cx, "confirm.close"), Some("esc"))),
                )
                .into_any_element(),
        )
    }
}
