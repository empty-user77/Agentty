//! A short question with a couple of answers: hand a session to another agent, delete one, compact
//! a nearly full context before continuing it. What each answer does is an [`AskAction`], so the
//! dialog never carries a callback that could outlive the window it was opened from.

use super::Workbench;
use crate::i18n::t;
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::TypeScale;
use agentty_bridge::model::SessionInfo;
use gpui::{div, prelude::*, px, ClickEvent, Context, SharedString, Window};

#[derive(Clone)]
pub enum AskAction {
    /// Continue this session in the other agent through a handoff document.
    Migrate(SessionInfo),
    /// Delete the session's transcript from disk.
    Delete(SessionInfo),
    /// Continue the session; `compact` asks the agent to compact its context first.
    Resume { session: SessionInfo, compact: bool },
    /// Start a new agent session in a working tree of its own.
    NewTree(super::worktrees::TreeRequest),
    /// Start it in the working tree another agent already works in.
    SameTree(super::worktrees::TreeRequest),
}

#[derive(Clone)]
pub struct AskChoice {
    pub label: SharedString,
    pub action: AskAction,
    /// The answer the button styling leads to.
    pub primary: bool,
    /// Destructive: the button is red.
    pub danger: bool,
}

pub struct Ask {
    pub title: SharedString,
    pub body: Option<SharedString>,
    pub choices: Vec<AskChoice>,
    /// Whether a Cancel button is shown. Off when closing the dialog does what one of the answers
    /// does (a shell waiting on it starts its agent where it was typed): two buttons, one outcome.
    pub cancel: bool,
}

impl Workbench {
    pub(super) fn ask(&mut self, ask: Ask, cx: &mut Context<Self>) {
        if let Some(previous) = self.ask.take() {
            self.release(&previous);
        }
        self.ask = Some(ask);
        self.launcher_open = false;
        self.notices_open = false;
        cx.notify();
    }

    pub(super) fn dismiss_ask(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(ask) = self.ask.take() else { return false };
        self.release(&ask);
        cx.notify();
        true
    }

    /// A question closed without an answer: a shell waiting on it starts its agent where it was
    /// typed, instead of waiting on a dialog that is gone — and, having just been asked, is not
    /// warned about sharing the tree on top of it.
    fn release(&mut self, ask: &Ask) {
        for choice in &ask.choices {
            if let AskAction::SameTree(request) = &choice.action {
                if let Some(pane) = request.shell_pane() {
                    self.shared_tree_warned.insert(pane);
                }
                request.stay();
            }
        }
    }

    fn perform_ask(&mut self, action: AskAction, window: &mut Window, cx: &mut Context<Self>) {
        self.ask = None;
        match action {
            AskAction::Migrate(session) => self.migrate_session(session, window, cx),
            AskAction::Delete(session) => self.delete_session(session, cx),
            AskAction::Resume { session, compact } => self.resume_session_compacting(&session, compact, window, cx),
            AskAction::NewTree(request) => self.start_in_new_tree(request, window, cx),
            AskAction::SameTree(request) => self.start_in_same_tree(request, window, cx),
        }
        cx.notify();
    }

    pub(super) fn render_ask(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let ask = self.ask.as_ref()?;
        // Long labels (and longer translations) wrap onto a second row instead of spilling out of the dialog.
        let mut buttons = div().flex().flex_wrap().justify_end().gap_2();
        if ask.cancel {
            buttons = buttons.child(
                div()
                    .id("ask-cancel")
                    .flex_none()
                    .px_3()
                    .py_1p5()
                    .rounded_md()
                    .t_body()
                    .cursor_pointer()
                    .bg(hex(0x2d2d30))
                    .text_color(hex(Chrome::BRIGHT))
                    .hover(|s| s.opacity(0.85))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.dismiss_ask(cx);
                    }))
                    .child(t(cx, "confirm.cancel")),
            );
        }
        for (index, choice) in ask.choices.iter().enumerate() {
            let action = choice.action.clone();
            let background = match (choice.danger, choice.primary) {
                (true, _) => hex(Chrome::ERROR),
                (false, true) => hex(Chrome::ACCENT),
                (false, false) => hex(0x2d2d30),
            };
            buttons = buttons.child(
                div()
                    .id(("ask-choice", index))
                    .flex_none()
                    .px_3()
                    .py_1p5()
                    .rounded_md()
                    .t_body()
                    .cursor_pointer()
                    .bg(background)
                    .text_color(hex(Chrome::BRIGHT))
                    .hover(|s| s.opacity(0.85))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.perform_ask(action.clone(), window, cx)))
                    .child(choice.label.clone()),
            );
        }
        Some(
            div()
                .id("ask-overlay")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(hex_alpha(0x000000, 0.45))
                .occlude()
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.dismiss_ask(cx);
                }))
                .child(
                    div()
                        .id("ask-dialog")
                        .w(px(420.))
                        .p_5()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .rounded_xl()
                        .bg(hex(Chrome::OVERLAY))
                        .border_1()
                        .border_color(hex(Chrome::OVERLAY_BORDER))
                        .shadow_lg()
                        // Clicking the dialog itself must not count as clicking the backdrop.
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .child(div().t_title().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(ask.title.clone()))
                        .children(ask.body.clone().map(|body| div().t_body().text_color(hex(Chrome::FOREGROUND)).child(body)))
                        .child(buttons),
                ),
        )
    }
}
