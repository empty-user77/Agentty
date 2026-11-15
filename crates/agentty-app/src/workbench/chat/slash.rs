//! A chat composer's `/` list: the skills and commands the agent has. Picking one puts it in the
//! composer, with room for more words, and sends nothing. The list itself comes from
//! `agentty_bridge::extensions`, where the remote page's `/api/skills` gets it too.

use super::*;
use agentty_bridge::extensions::{slash_find, slash_items, SlashItem};

/// Read again when asked this long after the last time: a skill just added shows up.
const SLASH_REFRESH: Duration = Duration::from_secs(15);
/// Rows a list holds: more than this is narrowed down by typing.
const SLASH_MAX_ROWS: usize = 50;
const SLASH_MAX_HEIGHT: f32 = 264.;

#[derive(Default)]
pub struct SlashList {
    items: Option<Arc<Vec<SlashItem>>>,
    read_at: Option<Instant>,
    loading: bool,
    /// The row Enter takes.
    pick: usize,
    /// Esc closed the list for this text: it opens again when the text changes.
    closed_for: Option<String>,
    scroll: gpui::ScrollHandle,
}

impl SlashList {
    /// The row Enter takes.
    pub(super) fn picked(&self) -> usize {
        self.pick
    }

    /// What the list shows for `text`, in the composer of a chat view.
    fn rows(&self, text: &str) -> Vec<SlashItem> {
        if self.closed_for.as_deref() == Some(text) {
            return Vec::new();
        }
        let Some(items) = &self.items else { return Vec::new() };
        slash_find(items, text).into_iter().take(SLASH_MAX_ROWS).cloned().collect()
    }
}

impl Workbench {
    pub(super) fn slash_rows(&self, id: EntityId, cx: &App) -> Vec<SlashItem> {
        let Some(state) = self.chats.get(&id) else { return Vec::new() };
        let Some((input, _)) = &state.input else { return Vec::new() };
        state.slash.rows(input.read(cx).text())
    }

    /// The composer's text changed: the list follows it, and is read if it has not been lately.
    pub(super) fn slash_text_changed(&mut self, id: EntityId, cx: &mut Context<Self>) {
        let Some(state) = self.chats.get_mut(&id) else { return };
        let Some((input, _)) = &state.input else { return };
        let text = input.read(cx).text().to_string();
        state.slash.pick = 0;
        if state.slash.closed_for.as_deref().is_some_and(|closed| closed != text) {
            state.slash.closed_for = None;
        }
        let asked = agentty_bridge::extensions::slash_query(&text).is_some();
        let stale = state.slash.read_at.is_none_or(|at| at.elapsed() >= SLASH_REFRESH);
        if asked && stale && !state.slash.loading {
            state.slash.loading = true;
            let view = state.lead.read(cx);
            let agent = pane_chat_agent(view);
            let folder = view.display_cwd();
            cx.spawn(async move |this, cx| {
                let items =
                    cx.background_spawn(async move { agent.map(|agent| slash_items(agent, Some(&folder))).unwrap_or_default() }).await;
                let _ = this.update(cx, |this, cx| {
                    if let Some(state) = this.chats.get_mut(&id) {
                        state.slash.items = Some(Arc::new(items));
                        state.slash.read_at = Some(Instant::now());
                        state.slash.loading = false;
                    }
                    this.slash_arrows(id, cx);
                    cx.notify();
                });
            })
            .detach();
        }
        self.slash_arrows(id, cx);
        cx.notify();
    }

    /// The composer's arrow keys move through the list while it is open, and its caret otherwise.
    fn slash_arrows(&mut self, id: EntityId, cx: &mut Context<Self>) {
        let open = !self.slash_rows(id, cx).is_empty();
        if let Some((input, _)) = self.chats.get(&id).and_then(|state| state.input.as_ref()) {
            input.update(cx, |input, _| input.set_arrows_to_owner(open));
        }
    }

    /// Enter, Tab, Esc and the arrows, for the list while it is open. False: not the list's.
    pub(super) fn slash_key(&mut self, id: EntityId, event: &TextInputEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let rows = self.slash_rows(id, cx);
        if rows.is_empty() {
            return false;
        }
        let Some(state) = self.chats.get_mut(&id) else { return false };
        match event {
            TextInputEvent::Confirmed | TextInputEvent::Next => {
                if let Some(item) = rows.get(state.slash.pick.min(rows.len() - 1)) {
                    self.slash_pick(id, item.invocation.clone(), window, cx);
                }
            }
            TextInputEvent::Up | TextInputEvent::Previous => {
                state.slash.pick = state.slash.pick.checked_sub(1).unwrap_or(rows.len() - 1);
                state.slash.scroll.scroll_to_item(state.slash.pick);
            }
            TextInputEvent::Down => {
                state.slash.pick = (state.slash.pick + 1) % rows.len();
                state.slash.scroll.scroll_to_item(state.slash.pick);
            }
            TextInputEvent::Cancelled => {
                state.slash.closed_for = state.input.as_ref().map(|(input, _)| input.read(cx).text().to_string());
                self.slash_arrows(id, cx);
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    /// Into the composer with a space after it: the rest of the prompt goes there. Nothing is sent.
    fn slash_pick(&mut self, id: EntityId, invocation: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some((input, _)) = self.chats.get(&id).and_then(|state| state.input.as_ref()) else { return };
        let input = input.clone();
        input.update(cx, |input, cx| input.set_text(format!("{invocation} "), cx));
        window.focus(&gpui::Focusable::focus_handle(&input, cx), cx);
    }

    /// The list, over the composer it belongs to.
    pub(super) fn render_slash_list(&self, state: &ChatState, cx: &mut Context<Self>) -> Option<AnyElement> {
        let id = state.lead.entity_id();
        let rows = self.slash_rows(id, cx);
        if rows.is_empty() {
            return None;
        }
        let pick = state.slash.pick.min(rows.len() - 1);
        let rows = rows.into_iter().enumerate().map(|(index, item)| {
            let invocation = item.invocation.clone();
            div()
                .id(("chat-slash", index))
                .flex()
                .items_baseline()
                .gap_2()
                .px_2p5()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .when(index == pick, |d| d.bg(hex(Chrome::SELECTED)))
                .when(index != pick, |d| d.hover(|s| s.bg(hex(Chrome::HOVER))))
                .child(div().flex_shrink_0().t_body().text_color(hex(Chrome::BRIGHT)).child(item.invocation))
                .child(div().flex_1().min_w_0().truncate().t_small().text_color(hex(Chrome::MUTED)).child(item.description))
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.slash_pick(id, invocation.clone(), window, cx)))
        });
        let list = div()
            .id("chat-slash-list")
            .occlude()
            .w_full()
            .max_h(px(SLASH_MAX_HEIGHT))
            .overflow_y_scroll()
            .track_scroll(&state.slash.scroll)
            .p_1()
            .rounded_lg()
            .border_1()
            .border_color(hex(Chrome::BORDER))
            .bg(hex(Chrome::PANEL))
            .shadow_lg()
            .flex()
            .flex_col()
            .children(rows);
        Some(div().absolute().left_0().right_0().bottom_full().mb_1().child(gpui::deferred(list).with_priority(3)).into_any_element())
    }

    /// What the agent still has running in the background, over the composer: between turns the chat
    /// looks finished while it waits on these.
    pub(super) fn render_chat_background(&self, state: &ChatState, cx: &App) -> Option<gpui::Div> {
        let first = state.background.first()?;
        let more = state.background.len() - 1;
        let text = tf(cx, "chat.background_running", &[("name", &first.label)]);
        Some(
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .px_2()
                .min_w_0()
                .t_small()
                .text_color(hex(Chrome::MUTED))
                .child(div().flex_shrink_0().size(px(6.)).rounded_full().bg(hex(Chrome::ACCENT)))
                .child(div().min_w_0().truncate().child(text))
                .when(more > 0, |d| d.child(div().flex_shrink_0().child(tf(cx, "chat.background_more", &[("n", &more.to_string())])))),
        )
    }
}
