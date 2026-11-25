//! The words of bots' conversations follow the chats' rule ([`super::chat::selectable`]): dragged
//! over to select (a double click takes a word), copied with ⌘C, all of them taken with ⌘A, a bar
//! where a drag ends to answer what was selected, a menu on a right click, addresses that open.

use super::chat::selectable;
use super::chat::selection_ui::quoted;
use super::Workbench;
use crate::i18n::t;
use crate::text_input::TextInput;
use crate::theme::{hex, Chrome};
use crate::ui::{icon, TypeScale as _};
use gpui::{
    div, prelude::*, px, AnyElement, ClickEvent, Context, Entity, FocusHandle, MouseButton, MouseDownEvent, MouseUpEvent, Pixels, Point,
    SharedString, Window,
};

/// Where bots' conversations stand among the chats' selectable text: far from any pane's id.
const CHATS: u64 = 0xB07C_0000_0000_0000;
const ROOMS: u64 = 0xB07D_0000_0000_0000;
const WITHIN: u64 = 0xFFFF_FFFF_FFFF;

/// A bot's chat, as the selection knows it.
pub(super) fn chat_area(handle: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    handle.hash(&mut hasher);
    CHATS | (hasher.finish() & WITHIN)
}

/// A room, as the selection knows it.
pub(super) fn room_area(id: u64) -> u64 {
    ROOMS | (id & WITHIN)
}

/// The words drawn from here on are entry `item` of conversation `area`.
pub(super) fn begin(area: u64, item: usize) {
    selectable::begin_item(area, item);
}

/// Plain words of the entry begun last, selectable as the rest.
pub(super) fn words(text: &str) -> AnyElement {
    selectable::selectable(if text.is_empty() { " ".to_string() } else { text.to_string() }, Vec::new())
}

/// The bar or the menu over a selection.
struct Over {
    area: u64,
    at: Point<Pixels>,
    text: String,
    menu: bool,
}

#[derive(Default)]
pub(super) struct Reading {
    /// Has the keyboard while words are selected, for ⌘C and ⌘A.
    focus: Option<FocusHandle>,
    over: Option<Over>,
    /// What the next message answers: the conversation it was selected in, and the words.
    quote: Option<(u64, String)>,
}

impl Reading {
    /// Another conversation comes in front: the bar goes, and so does a quote nobody would see
    /// before it went out with the next message.
    pub(super) fn forget(&mut self) {
        if self.over.take().is_some() {
            selectable::clear();
        }
        self.quote = None;
    }
}

impl Workbench {
    /// `body`, a conversation of bots, read as a chat is: a press on its words starts a selection
    /// and takes the keyboard there, a press elsewhere drops it and gives the keyboard back to
    /// `composer`.
    pub(super) fn bot_reading(
        &mut self,
        area: u64,
        composer: Option<Entity<TextInput>>,
        body: gpui::Div,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let focus = self.bots.reading.focus.get_or_insert_with(|| cx.focus_handle()).clone();
        // The bar of another conversation's selection does not follow here.
        if self.bots.reading.over.as_ref().is_some_and(|over| over.area != area) {
            self.bots.reading.over = None;
        }
        let (typed, typed_focus) = (composer.clone(), focus.clone());
        let (paste_into, back_to) = (composer.clone(), composer);
        body.id(SharedString::from(format!("bot-reading-{area}")))
            .key_context("ChatReading")
            .track_focus(&focus)
            .on_action(cx.listener(|this, _: &super::CopyChatSelection, _, cx| {
                if let Some(text) = selectable::selected_text() {
                    this.bot_copy(text, cx);
                }
            }))
            .on_action(cx.listener(move |this, _: &super::SelectAllChat, _, cx| {
                if selectable::select_all(area) {
                    this.bots.reading.over = None;
                    cx.notify();
                }
            }))
            .on_action(cx.listener(move |_, _: &super::PasteIntoChat, window, cx| {
                let Some(input) = &paste_into else { return };
                let focus = gpui::Focusable::focus_handle(input, cx);
                // The box itself has the keyboard: it took this paste already.
                if focus.is_focused(window) {
                    return;
                }
                window.focus(&focus, cx);
                input.update(cx, |input, cx| input.paste_clipboard(window, cx));
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseUpEvent, window, cx| {
                    if selectable::take_just_selected() {
                        if let Some(text) = selectable::selected_text() {
                            // Linux: what the pointer selected is what a middle click pastes.
                            crate::platform::primary::set(text.clone(), cx);
                            this.bots.reading.over = Some(Over { area, at: selectable::bar_point(event.position), text, menu: false });
                            cx.notify();
                        }
                    }
                    // A click on words selects nothing: the keyboard goes back to the composer.
                    if selectable::selected_text().is_none() {
                        selectable::clear();
                        if this.bots.reading.focus.as_ref().is_some_and(|focus| focus.is_focused(window)) {
                            if let Some(input) = &back_to {
                                window.focus(&gpui::Focusable::focus_handle(input, cx), cx);
                            }
                        }
                        cx.notify();
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    if let Some(text) = selectable::selected_text() {
                        this.bots.reading.over = Some(Over { area, at: event.position, text, menu: true });
                        cx.notify();
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    this.bots.reading.over = None;
                    if selectable::take_press() {
                        if let Some(focus) = &this.bots.reading.focus {
                            window.focus(focus, cx);
                        }
                    }
                    cx.notify();
                }),
            )
            // Typing while words are selected goes to the composer, an input method's too.
            .child(
                gpui::canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| {
                        if let Some(input) = typed {
                            window.handle_input(&typed_focus, gpui::ElementInputHandler::new(bounds, input), cx);
                        }
                    },
                )
                .absolute()
                .size_0(),
            )
            .children(self.render_bot_over(area, cx))
    }

    /// Typing with words selected went to the composer: the keyboard follows it there, so the
    /// keys after it (Return, the arrows, ⌫) do too.
    pub(super) fn bot_typed_over_selection(&mut self, input: &Entity<TextInput>, window: &mut Window, cx: &mut Context<Self>) {
        if self.bots.reading.focus.as_ref().is_some_and(|focus| focus.is_focused(window)) {
            window.focus(&gpui::Focusable::focus_handle(input, cx), cx);
            selectable::clear();
            self.bots.reading.over = None;
            cx.notify();
        }
    }

    /// The bots' pages went out of sight: a selection made there and its bar go with them.
    pub(super) fn drop_hidden_bot_selection(&mut self) {
        if self.bots.reading.over.take().is_some() {
            selectable::clear();
        }
    }

    /// For the debug driver: what is selected, and what ⌘C, ⌘A and the bar's button do with it.
    pub(super) fn debug_bot_reading(&mut self, what: &str, window: &mut Window, cx: &mut Context<Self>) {
        let area = self.bots.reading.over.as_ref().map(|over| over.area);
        match what {
            "copy" => {
                if let Some(text) = selectable::selected_text() {
                    self.bot_copy(text, cx);
                }
            }
            "all" => {
                let front = match (self.bots.profile.as_deref(), self.bots.rooms.open_room()) {
                    (Some(handle), _) => Some(chat_area(handle)),
                    (None, room) => room.map(room_area),
                };
                if front.is_some_and(selectable::select_all) {
                    cx.notify();
                }
            }
            "reply" => {
                if let Some(area) = area {
                    self.bot_reply_to_selection(area, window, cx);
                }
            }
            _ => {}
        }
        let reading = &self.bots.reading;
        eprintln!(
            "bots-reading: selection={:?} over={} keyboard={} quote={:?}",
            selectable::selected_text(),
            reading.over.as_ref().map_or("none", |over| if over.menu { "menu" } else { "bar" }),
            reading.focus.as_ref().is_some_and(|focus| focus.is_focused(window)),
            reading.quote.as_ref().map(|(_, quote)| quote),
        );
    }

    fn bot_copy(&mut self, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
        self.show_toast(t(cx, "chat.copied"), cx);
    }

    /// The box a conversation is written in.
    fn bot_composer(&self, area: u64) -> Option<Entity<TextInput>> {
        if area & !WITHIN == ROOMS {
            self.bots.rooms.composer()
        } else {
            self.bots.ask_input()
        }
    }

    /// What entry `item` of a conversation says, as it was written (its Markdown).
    fn bot_said(&self, area: u64, item: usize) -> Option<String> {
        if area & !WITHIN == ROOMS {
            let room = self.bots.rooms.rooms.iter().find(|room| room_area(room.id) == area)?;
            return room.log.get(item).map(|entry| entry.text.clone());
        }
        let handle = self.bots.profile.as_deref().filter(|handle| chat_area(handle) == area)?;
        self.bots.chats.get(handle)?.get(item).map(|line| line.text.clone())
    }

    fn bot_copy_message(&mut self, area: u64, markdown: bool, cx: &mut Context<Self>) {
        let Some(item) = selectable::selected_item(area) else { return };
        let text = if markdown { self.bot_said(area, item) } else { Some(selectable::item_text(area, item)) };
        self.bots.reading.over = None;
        if let Some(text) = text.filter(|text| !text.trim().is_empty()) {
            self.bot_copy(text, cx);
        }
        cx.notify();
    }

    /// The selection goes over the composer, quoted in front of the next message.
    fn bot_reply_to_selection(&mut self, area: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(over) = self.bots.reading.over.take() else { return };
        selectable::clear();
        self.bots.reading.quote = Some((area, over.text));
        if let Some(input) = self.bot_composer(area) {
            window.focus(&gpui::Focusable::focus_handle(&input, cx), cx);
        }
        cx.notify();
    }

    /// `text` as it goes out of conversation `area`'s composer: after what it answers, when
    /// something was quoted there.
    pub(super) fn bot_with_quote(&mut self, area: u64, text: String) -> String {
        match self.bots.reading.quote.take() {
            Some((quoted_in, quote)) if quoted_in == area => quoted(&quote, &text),
            other => {
                self.bots.reading.quote = other;
                text
            }
        }
    }

    /// What the next message of conversation `area` answers, over its composer, with a button
    /// that drops it.
    pub(super) fn render_bot_quote(&self, area: u64, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let (_, quote) = self.bots.reading.quote.as_ref().filter(|(quoted_in, _)| *quoted_in == area)?;
        let first = quote.lines().find(|line| !line.trim().is_empty()).unwrap_or_default().to_string();
        Some(
            div()
                .pl_2()
                .border_l_2()
                .border_color(hex(Chrome::BORDER))
                .flex()
                .items_center()
                .gap_2()
                .child(icon("undo-2", 13., hex(Chrome::MUTED)))
                .child(div().flex_1().min_w_0().truncate().t_small().text_color(hex(Chrome::MUTED)).child(first))
                .child(
                    div()
                        .id("bot-quote-remove")
                        .size(px(20.))
                        .rounded_md()
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|s| s.bg(hex(Chrome::HOVER)))
                        .tooltip(crate::ui::Tooltip::text(t(cx, "chat.remove_attachment"), None))
                        .child(icon("x", 12., hex(Chrome::MUTED)))
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.bots.reading.quote = None;
                            cx.notify();
                        })),
                ),
        )
    }

    /// The selection's bar or menu, drawn over everything.
    fn render_bot_over(&self, area: u64, cx: &mut Context<Self>) -> Option<AnyElement> {
        let over = self.bots.reading.over.as_ref().filter(|over| over.area == area)?;
        let at = over.at;
        let panel = || {
            div()
                // Nothing under it takes the pointer (the words below would start a new selection).
                .occlude()
                .p_1()
                .rounded_lg()
                .border_1()
                .border_color(hex(Chrome::BORDER))
                .bg(hex(Chrome::PANEL))
                .shadow_lg()
                // A press here must not reach the conversation (which would close this first).
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    if this.bots.reading.over.take().is_some() {
                        cx.notify();
                    }
                }))
        };
        let (corner, body): (gpui::Anchor, AnyElement) = if over.menu {
            let row = |key: &'static str, label: &'static str, keys: Option<&'static str>| {
                div()
                    .id(key)
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_6()
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .cursor_pointer()
                    .t_body()
                    .text_color(hex(Chrome::BRIGHT))
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .child(label)
                    .children(keys.map(|keys| {
                        div().t_small().text_color(hex(Chrome::MUTED)).child(crate::keymap::display_in_text(keys).into_owned())
                    }))
            };
            let separator = || div().my_1().h(px(1.)).bg(hex(Chrome::BORDER));
            let menu = panel()
                .w(px(260.))
                .flex()
                .flex_col()
                .child(row("bot-menu-copy", t(cx, "chat.menu_copy"), Some("⌘C")).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    if let Some(over) = this.bots.reading.over.take() {
                        this.bot_copy(over.text, cx);
                    }
                })))
                .child(separator())
                .child(
                    row("bot-menu-copy-message", t(cx, "chat.menu_copy_message"), None)
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.bot_copy_message(area, false, cx))),
                )
                .child(
                    row("bot-menu-copy-markdown", t(cx, "chat.menu_copy_markdown"), None)
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.bot_copy_message(area, true, cx))),
                )
                .child(separator())
                .child(
                    row("bot-menu-reply", t(cx, "chat.reply"), None)
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.bot_reply_to_selection(area, window, cx))),
                );
            (gpui::Anchor::TopLeft, menu.into_any_element())
        } else {
            let bar = panel().flex().items_center().gap_1().child(
                div()
                    .id("bot-sel-reply")
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .t_small()
                    .text_color(hex(Chrome::BRIGHT))
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .child(t(cx, "chat.reply"))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.bot_reply_to_selection(area, window, cx))),
            );
            (gpui::Anchor::BottomLeft, bar.into_any_element())
        };
        let gap = if corner == gpui::Anchor::BottomLeft { div().mb_2() } else { div().mt_1() };
        Some(
            gpui::deferred(gpui::anchored().position(at).anchor(corner).snap_to_window_with_margin(px(8.)).child(gap.child(body)))
                .with_priority(4)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversations_of_bots_stand_apart_from_each_other_and_from_panes() {
        assert_eq!(chat_area("scout"), chat_area("scout"));
        assert_ne!(chat_area("scout"), chat_area("buddy"));
        assert_ne!(chat_area("scout") & !WITHIN, room_area(7) & !WITHIN);
        assert_eq!(room_area(7) & WITHIN, 7);
        // A pane's id is its slot and that slot's count of uses: nowhere near these.
        assert!(chat_area("scout") >> 48 == 0xB07C && room_area(1) >> 48 == 0xB07D);
    }
}
