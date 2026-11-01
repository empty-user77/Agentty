//! What a selection offers, as Claude Desktop does: a bar where a drag ends (side chat, reply), a
//! menu on a right click in a plain chat (copy, the whole message, as Markdown, reply, side chat),
//! and the side chat's question box: the quote and a line to ask it in. An agent's terminal gets
//! the bar too: what was selected there (the agent's own selection, copied) is replied to in its
//! input, or asked about in a side chat.

use super::*;
use gpui::{MouseButton, MouseDownEvent, Pixels, Point};

/// The selection's bar, menu or question box, in one pane (a chat, or an agent's terminal).
pub(in crate::workbench) struct SelectUi {
    pub(super) chat: EntityId,
    pub(super) at: Point<Pixels>,
    /// What was selected, as it was when the bar or menu came up.
    pub(super) text: String,
    /// In the pane's terminal: a reply goes into the agent's own input.
    pub(super) terminal: bool,
    pub(super) kind: SelectUiKind,
}

pub(in crate::workbench) enum SelectUiKind {
    Bar,
    Menu,
    /// The side chat's question: what was selected, and the question about it.
    Ask {
        quote: String,
        input: Entity<TextInput>,
        _confirm: Subscription,
    },
}

/// `quote` as a Markdown quote, before what is asked about it.
pub(super) fn quoted(quote: &str, text: &str) -> String {
    let quote: Vec<String> = quote.lines().map(|line| format!("> {line}")).collect();
    if text.trim().is_empty() {
        quote.join("\n")
    } else {
        format!("{}\n\n{text}", quote.join("\n"))
    }
}

impl Workbench {
    /// A drag over text in chat `id` ended: its bar comes up where the button was let go.
    pub(super) fn selection_ended(&mut self, id: EntityId, at: Point<Pixels>, cx: &mut Context<Self>) {
        if selectable::take_just_selected() {
            let Some(text) = selectable::selected_text() else { return };
            self.chat_select = Some(SelectUi { chat: id, at, text, terminal: false, kind: SelectUiKind::Bar });
            cx.notify();
        }
    }

    /// Text was selected in `pane`'s terminal (the agent copied its own selection, or Agentty's
    /// selection ended): an agent's pane gets the bar where the button was let go.
    pub(in crate::workbench) fn terminal_selected(&mut self, pane: &Pane, text: &str, at: Point<Pixels>, cx: &mut Context<Self>) {
        if pane_chat_agent(pane.read(cx)).is_none() || text.trim().is_empty() {
            return;
        }
        let text = text.to_string();
        self.chat_select = Some(SelectUi { chat: pane.entity_id(), at, text, terminal: true, kind: SelectUiKind::Bar });
        cx.notify();
    }

    /// A right click in chat `id`: the menu, while something is selected.
    pub(super) fn selection_menu(&mut self, id: EntityId, at: Point<Pixels>, cx: &mut Context<Self>) {
        if let Some(text) = selectable::selected_text() {
            self.chat_select = Some(SelectUi { chat: id, at, text, terminal: false, kind: SelectUiKind::Menu });
            cx.notify();
        }
    }

    fn close_select_ui(&mut self, cx: &mut Context<Self>) {
        if self.chat_select.take().is_some() {
            cx.notify();
        }
    }

    fn copy_with_toast(&mut self, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
        self.show_toast(t(cx, "chat.copied"), cx);
    }

    /// The pane `id` (a chat, or a terminal), wherever it is.
    fn pane_for_entity(&self, id: EntityId) -> Option<Pane> {
        self.all_panes().into_iter().find(|pane| pane.entity_id() == id)
    }

    /// The selection goes over the composer, quoted in front of the next message; in a terminal,
    /// into the agent's input (pasted, not sent), with room after it for the question.
    fn reply_to_selection(&mut self, id: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(SelectUi { text, terminal, .. }) = self.chat_select.take() else { return };
        selectable::clear();
        if terminal {
            let Some(pane) = self.pane_for_entity(id) else { return };
            pane.update(cx, |view, _| view.insert_text(&format!("{}\n\n", quoted(&text, ""))));
            self.focus_pane(&pane, window, cx);
            return cx.notify();
        }
        let Some(state) = self.chats.get_mut(&id) else { return };
        state.quote = Some(text);
        if let Some((input, _)) = state.input.as_ref() {
            window.focus(&gpui::Focusable::focus_handle(input, cx));
        }
        cx.notify();
    }

    /// The side chat's question box, over the selection.
    fn ask_in_side_chat(&mut self, id: EntityId, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(SelectUi { text: quote, terminal, .. }) = self.chat_select.take() else { return };
        let input = cx.new(|cx| TextInput::localized("", "chat.side_ask_placeholder", window, cx));
        let confirm = cx.subscribe_in(&input, window, move |this, _, event: &TextInputEvent, window, cx| {
            if let TextInputEvent::Confirmed = event {
                this.send_side_question(id, window, cx);
            }
        });
        window.focus(&gpui::Focusable::focus_handle(&input, cx));
        let text = quote.clone();
        self.chat_select = Some(SelectUi { chat: id, at, text, terminal, kind: SelectUiKind::Ask { quote, input, _confirm: confirm } });
        cx.notify();
    }

    /// Opens the side chat (a copy of this conversation beside it) and asks it the question.
    fn send_side_question(&mut self, id: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(SelectUi { kind: SelectUiKind::Ask { quote, input, .. }, .. }) = self.chat_select.take() else { return };
        let question = input.read(cx).text().trim().to_string();
        let Some(pane) = self.pane_for_entity(id) else { return };
        selectable::clear();
        self.open_side_chat(&pane, window, cx);
        let Some(side) = self.active_pane().filter(|side| *side != pane) else { return };
        // The side chat is a chat too, and its first message is the question about the quote.
        self.chat_modes.insert(side.entity_id(), true);
        self.side_questions.insert(side.entity_id(), quoted(&quote, &question));
        cx.notify();
    }

    fn copy_message(&mut self, id: EntityId, markdown: bool, cx: &mut Context<Self>) {
        let Some(item) = selectable::selected_item(id.as_u64()) else { return };
        let text = if markdown {
            self.chats.get(&id).and_then(|state| state.shown.get(item)).map(|item| match item {
                ChatItem::User(text) => text.clone(),
                ChatItem::Reply(blocks) => reply_text(blocks),
                ChatItem::Peer { text, .. } => text.clone(),
                _ => String::new(),
            })
        } else {
            Some(selectable::item_text(id.as_u64(), item))
        };
        self.chat_select = None;
        if let Some(text) = text.filter(|t| !t.trim().is_empty()) {
            self.copy_with_toast(text, cx);
        }
        cx.notify();
    }

    /// The selection's bar, menu or question box, wherever it is (drawn over everything).
    pub(in crate::workbench) fn render_select_ui(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let ui = self.chat_select.as_ref()?;
        let id = ui.chat;
        // A side chat copies a Claude Code session (Codex has no such copy).
        let side_chat = self.pane_for_entity(id).is_some_and(|pane| {
            pane.read(cx).agent_kind().and_then(crate::launch::PaneKind::agent) == Some(agentty_bridge::model::Agent::Claude)
        });
        let at = ui.at;
        let row = |key: &'static str, label: String, keys: Option<&'static str>| {
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
                .children(keys.map(|keys| div().t_small().text_color(hex(Chrome::MUTED)).child(keys)))
        };
        let separator = || div().my_1().h(px(1.)).bg(hex(Chrome::BORDER));
        let panel = || {
            div()
                // Nothing under it takes the pointer (the text below would start a new selection).
                .occlude()
                .p_1()
                .rounded_lg()
                .border_1()
                .border_color(hex(Chrome::BORDER))
                .bg(hex(Chrome::PANEL))
                .shadow_lg()
                // A press here must not reach the chat (which would close this first).
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        };
        let (corner, body): (gpui::Corner, AnyElement) = match &ui.kind {
            SelectUiKind::Bar => {
                let button = |key: &'static str, label: String| {
                    div()
                        .id(key)
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .cursor_pointer()
                        .t_small()
                        .text_color(hex(Chrome::BRIGHT))
                        .hover(|s| s.bg(hex(Chrome::HOVER)))
                        .child(label)
                };
                let bar = panel()
                    .flex()
                    .items_center()
                    .gap_1()
                    .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| this.close_select_ui(cx)))
                    .when(side_chat, |d| {
                        d.child(
                            button("chat-sel-side", t(cx, "chat.side_chat").to_string())
                                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.ask_in_side_chat(id, at, window, cx))),
                        )
                        .child(div().w(px(1.)).h(px(14.)).bg(hex(Chrome::BORDER)))
                    })
                    .child(
                        button("chat-sel-reply", t(cx, "chat.reply").to_string())
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.reply_to_selection(id, window, cx))),
                    );
                (gpui::Corner::BottomLeft, bar.into_any_element())
            }
            SelectUiKind::Menu => {
                let menu = panel()
                    .w(px(260.))
                    .flex()
                    .flex_col()
                    .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| this.close_select_ui(cx)))
                    .child(row("chat-menu-copy", t(cx, "chat.menu_copy").to_string(), Some("⌘C")).on_click(cx.listener(
                        |this, _: &ClickEvent, _, cx| {
                            if let Some(ui) = this.chat_select.take() {
                                this.copy_with_toast(ui.text, cx);
                            }
                        },
                    )))
                    .child(separator())
                    .child(
                        row("chat-menu-copy-message", t(cx, "chat.menu_copy_message").to_string(), None)
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.copy_message(id, false, cx))),
                    )
                    .child(
                        row("chat-menu-copy-markdown", t(cx, "chat.menu_copy_markdown").to_string(), None)
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.copy_message(id, true, cx))),
                    )
                    .child(separator())
                    .child(
                        row("chat-menu-reply", t(cx, "chat.reply").to_string(), None)
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.reply_to_selection(id, window, cx))),
                    )
                    .when(side_chat, |d| {
                        d.child(
                            row("chat-menu-side", t(cx, "chat.side_chat").to_string(), None)
                                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.ask_in_side_chat(id, at, window, cx))),
                        )
                    });
                (gpui::Corner::TopLeft, menu.into_any_element())
            }
            SelectUiKind::Ask { quote, input, .. } => {
                let first = quote.lines().find(|line| !line.trim().is_empty()).unwrap_or_default().to_string();
                let ask = panel()
                    .w(px(440.))
                    .p_2()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| this.close_select_ui(cx)))
                    .child(
                        div()
                            .pl_2()
                            .border_l_2()
                            .border_color(hex(Chrome::BORDER))
                            .t_small()
                            .text_color(hex(Chrome::MUTED))
                            .truncate()
                            .child(first),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().flex_1().min_w_0().t_body().text_color(hex(Chrome::BRIGHT)).child(input.clone()))
                            .child(
                                div()
                                    .id("chat-side-send")
                                    .size(px(26.))
                                    .rounded_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                                    .child(icon("arrow-up", 14., hex(Chrome::BRIGHT)))
                                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.send_side_question(id, window, cx))),
                            ),
                    );
                (gpui::Corner::BottomLeft, ask.into_any_element())
            }
        };
        let gap = if corner == gpui::Corner::BottomLeft { div().mb_2() } else { div().mt_1() };
        Some(
            gpui::deferred(gpui::anchored().position(at).anchor(corner).snap_to_window_with_margin(px(8.)).child(gap.child(body)))
                .with_priority(4)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::quoted;

    #[test]
    fn a_quote_goes_before_the_question() {
        assert_eq!(quoted("first\nsecond", "why?"), "> first\n> second\n\nwhy?");
        assert_eq!(quoted("only", ""), "> only");
    }
}
