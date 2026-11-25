//! One bot's own page: a chat with it, as a messenger shows one. What the user wrote is on the
//! right, what the bot said on the left, and what made it start without being asked (a time, a
//! watch, another bot) between them. Its settings are behind the button at the top.

use super::bots::{BotState, ChatLine, Party};
use super::Workbench;
use crate::i18n::{t, tf};
use crate::theme::{hex, Chrome};
use crate::ui::{icon, TypeScale as _};
use agentty_bridge::bots::Bot;
use gpui::{div, prelude::*, px, relative, AnyElement, ClickEvent, Context, SharedString, Window};

/// Lines of the bot's memory shown with its settings; the file itself opens from the rows below.
const MEMORY_LINES: usize = 8;
/// The newest lines of the chat that are drawn.
const CHAT_SHOWN: usize = 80;
/// Where the chat's markdown blocks are numbered from, apart from the rooms' and the chats'.
const CHAT_TEXT: usize = 700_000;
/// The chat's column: as wide as reads well, in the middle of a wide window.
const COLUMN: f32 = 780.;

impl Workbench {
    pub(super) fn render_bot_profile(&mut self, bot: &Bot, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let handle = bot.handle.clone();
        let state = self.bot_state(bot, cx);
        let settings = self.bots.profile_settings;
        let under = match state {
            BotState::Working => t(cx, "bots.state_working").to_string(),
            BotState::NeedsYou => t(cx, "bots.needs_you").to_string(),
            BotState::Off => t(cx, "bots.state_off").to_string(),
            BotState::Idle if bot.purpose.is_empty() => format!("@{handle}"),
            BotState::Idle => bot.purpose.clone(),
        };
        let pane_handle = handle.clone();
        let header = div()
            .flex_none()
            .h(px(56.))
            .px_4()
            .flex()
            .items_center()
            .gap_3()
            .border_b_1()
            .border_color(hex(Chrome::BORDER))
            .child(super::bot_face::avatar(bot, state, 34., Chrome::EDITOR))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .t_body()
                            .font_weight(crate::theme::EMPHASIS)
                            .text_color(hex(Chrome::BRIGHT))
                            .truncate()
                            .child(bot.name.clone()),
                    )
                    .child(
                        div()
                            .t_caption()
                            .truncate()
                            .text_color(hex(if state == BotState::NeedsYou { Chrome::ATTENTION } else { Chrome::MUTED }))
                            .child(under),
                    ),
            )
            .child(
                crate::ui::icon_only(
                    "bot-chat-pane",
                    "square-terminal",
                    cx.listener(move |this, _: &ClickEvent, window, cx| this.open_bot(&pane_handle, window, cx)),
                )
                .tooltip(crate::ui::Tooltip::text(t(cx, "bots.chat"), None)),
            )
            .child(
                crate::ui::icon_only(
                    "bot-chat-settings",
                    if settings { "message-square" } else { "settings" },
                    cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.bots.profile_settings = !this.bots.profile_settings;
                        if !this.bots.profile_settings {
                            this.bots.chat_scroll.scroll_to_bottom();
                        }
                        cx.notify();
                    }),
                )
                .tooltip(crate::ui::Tooltip::text(t(cx, if settings { "bots.back_to_chat" } else { "bots.settings" }), None)),
            );

        let body = if settings { self.render_bot_about(bot, cx) } else { self.render_bot_chat(bot, state, window, cx) };
        div().size_full().flex().flex_col().bg(hex(Chrome::EDITOR)).child(header).child(body).into_any_element()
    }

    /// The chat: its lines, what the bot is doing now, and the box to write to it.
    fn render_bot_chat(&mut self, bot: &Bot, state: BotState, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let handle = bot.handle.clone();
        let now = crate::ui::now_ms();
        let me = Party::Bot(handle.clone());
        // Each line keeps its place in the whole chat, so what is selected stays put as lines come.
        let area = super::bot_reading::chat_area(&handle);
        let first = self.bots.chats.get(&handle).map_or(0, |lines| lines.len().saturating_sub(CHAT_SHOWN));
        let lines: Vec<ChatLine> = self.bots.chats.get(&handle).map(|lines| lines[first..].to_vec()).unwrap_or_default();
        let mut thread = div().w_full().max_w(px(COLUMN)).mx_auto().px_5().py_5().flex().flex_col().gap_3();
        if lines.is_empty() {
            let hint = if self.bot_starts_alone(bot) { "bots.chat_empty" } else { "bots.starts_none" };
            thread = thread.child(
                div()
                    .pt_12()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_2()
                    .child(super::bot_face::tile(bot, 84.))
                    .child(
                        div()
                            .pt_2()
                            .t_heading()
                            .font_weight(crate::theme::EMPHASIS)
                            .text_color(hex(Chrome::BRIGHT))
                            .child(bot.name.clone()),
                    )
                    .child(div().t_body().text_color(hex(Chrome::MUTED)).child(bot.purpose.clone()))
                    .child(div().pt_2().max_w(px(460.)).text_center().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, hint))),
            );
        }
        for (index, line) in lines.iter().enumerate() {
            let at = div().flex_none().t_caption().text_color(hex(Chrome::MUTED)).child(crate::ui::relative_time(now, line.at_ms));
            super::bot_reading::begin(area, first + index);
            if line.from == Party::You {
                thread = thread.child(
                    div().flex().justify_end().items_end().gap_2().child(at).child(
                        div()
                            .max_w(relative(0.78))
                            .px_3p5()
                            .py_2()
                            .rounded_xl()
                            .bg(hex(Chrome::SELECTED))
                            .t_body()
                            .text_color(hex(Chrome::BRIGHT))
                            .child(super::bot_reading::words(&line.text)),
                    ),
                );
            } else if line.from == me {
                // Said to somebody else than the user: the bubble names who.
                let to = (line.to != Party::You).then(|| format!("→ {}", self.party_name(&line.to, cx)));
                thread = thread.child(
                    div()
                        .flex()
                        .items_end()
                        .gap_2()
                        .child(div().flex_none().pb_0p5().child(super::bot_face::tile(bot, 28.)))
                        .child(
                            div()
                                .max_w(relative(0.82))
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .px_3p5()
                                        .py_2()
                                        .rounded_xl()
                                        .bg(hex(Chrome::PANEL))
                                        .border_1()
                                        .border_color(hex(Chrome::BORDER))
                                        .child(super::chat::markdown_text(&line.text, CHAT_TEXT + index)),
                                )
                                .children(to.map(|to| div().pl_1().t_caption().text_color(hex(Chrome::MUTED)).child(to))),
                        )
                        .child(at),
                );
            } else {
                // What started the bot without the user: a time, a watch, a channel, another bot.
                thread = thread.child(
                    div().flex().justify_center().child(
                        div()
                            .max_w(relative(0.9))
                            .px_3()
                            .py_1()
                            .flex()
                            .items_center()
                            .gap_2()
                            .rounded_full()
                            .border_1()
                            .border_color(hex(Chrome::BORDER))
                            .child(self.party_face(&line.from, 18.))
                            .child(
                                div()
                                    .flex_none()
                                    .t_caption()
                                    .font_weight(crate::theme::EMPHASIS)
                                    .text_color(hex(Chrome::FOREGROUND))
                                    .child(self.party_name(&line.from, cx)),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .t_caption()
                                    .text_color(hex(Chrome::MUTED))
                                    .child(super::bot_reading::words(&super::bots::plain_line(&line.text, 200))),
                            )
                            .child(at),
                    ),
                );
            }
        }
        match state {
            BotState::Working => {
                thread =
                    thread.child(div().flex().items_end().gap_2().child(super::bot_face::tile(bot, 28.)).child(
                        div().px_3p5().py_2p5().rounded_xl().bg(hex(Chrome::PANEL)).border_1().border_color(hex(Chrome::BORDER)).child(
                            crate::ui::dot_spinner(SharedString::from(format!("bot-chat-working-{handle}")), 13., hex(Chrome::BRIGHT)),
                        ),
                    ));
            }
            BotState::NeedsYou => {
                let what = self.bot_line(bot, state, cx).unwrap_or_default();
                let answer = handle.clone();
                thread = thread.child(
                    div()
                        .px_3p5()
                        .py_2p5()
                        .flex()
                        .items_center()
                        .gap_3()
                        .rounded_xl()
                        .border_1()
                        .border_color(hex(Chrome::ATTENTION))
                        .child(icon("message-circle-question", 16., hex(Chrome::ATTENTION)))
                        .child(div().flex_1().min_w_0().t_small().text_color(hex(Chrome::BRIGHT)).child(what))
                        .child(crate::ui::action_button(
                            "bot-chat-answer",
                            t(cx, "bots.answer"),
                            cx.listener(move |this, _: &ClickEvent, window, cx| this.open_bot(&answer, window, cx)),
                        )),
                );
            }
            BotState::Idle | BotState::Off => {}
        }

        let scroll = self.bots.chat_scroll.clone();
        let input = self.bot_ask_input(window, cx);
        let quote = self.render_bot_quote(area, cx);
        let typed = input.clone();
        let composer =
            div().flex_none().w_full().max_w(px(COLUMN)).mx_auto().px_5().pb_4().flex().flex_col().gap_1p5().children(quote).child(
                div()
                    .pl_4()
                    .pr_1p5()
                    .py_1p5()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_xl()
                    .bg(hex(Chrome::PANEL))
                    .border_1()
                    .border_color(hex(Chrome::BORDER))
                    .child(div().flex_1().min_w_0().t_body().text_color(hex(Chrome::BRIGHT)).child(input))
                    .child(crate::ui::icon_only(
                        "bot-chat-send",
                        "arrow-up",
                        cx.listener(|this, _: &ClickEvent, _, cx| this.bot_ask_send(cx)),
                    )),
            );
        let chat = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .child(div().id("bot-chat").size_full().overflow_y_scroll().track_scroll(&scroll).child(thread))
                    .group(crate::ui::SCROLL_GROUP)
                    .child(crate::ui::scrollbar(scroll)),
            )
            .child(composer);
        self.bot_reading(area, Some(typed), chat, cx).into_any_element()
    }

    /// A schedule, a watch or a channel makes the bot start without the user.
    fn bot_starts_alone(&self, bot: &Bot) -> bool {
        bot.enabled && (bot.schedules.iter().any(|schedule| schedule.enabled) || !bot.watches.is_empty() || !bot.channels.is_empty())
    }

    /// Behind the chat: what the bot is made of, when it starts by itself, and its settings.
    fn render_bot_about(&mut self, bot: &Bot, cx: &mut Context<Self>) -> AnyElement {
        use super::settings_page::section;
        let handle = bot.handle.clone();
        let mut starts = section(t(cx, "bots.starts"));
        let start_row = |glyph: &'static str, what: String, when: Option<String>| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .py_1p5()
                .child(icon(glyph, 14., hex(Chrome::FOREGROUND)))
                .child(div().flex_1().min_w_0().truncate().t_small().text_color(hex(Chrome::BRIGHT)).child(what))
                .children(when.map(|when| div().flex_none().t_caption().text_color(hex(Chrome::MUTED)).child(when)))
        };
        if self.bot_starts_alone(bot) {
            let next = self.bots.schedule.next_run(&handle).map(|at| tf(cx, "bots.next_run", &[("when", &at)]));
            for schedule in bot.schedules.iter().filter(|schedule| schedule.enabled) {
                let what = format!("{} · {}", schedule.cron, agentty_bridge::fsutil::one_line(&schedule.prompt, 120));
                starts = starts.child(start_row("clock", what, next.clone()));
            }
            for kind in &bot.watches {
                let trouble = self.bots.watch.trouble.get(&(handle.clone(), *kind)).cloned();
                starts = starts.child(start_row("eye", t(cx, super::bot_watch::label(*kind)).to_string(), trouble));
            }
            for channel in &bot.channels {
                starts = starts.child(start_row("send", channel.kind.label().to_string(), None));
            }
        } else {
            starts = starts.child(div().py_1p5().t_small().text_color(hex(Chrome::WARNING)).child(t(cx, "bots.starts_none")));
        }

        let mut memory = section(t(cx, "bots.memory"));
        let remembered: Vec<&str> = self.bots.profile_memory.lines().filter(|line| !line.trim().is_empty()).take(MEMORY_LINES).collect();
        if remembered.is_empty() {
            memory = memory.child(div().py_1p5().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "bots.memory_none")));
        }
        for line in remembered {
            memory = memory.child(div().py_0p5().t_small().truncate().text_color(hex(Chrome::FOREGROUND)).child(line.to_string()));
        }

        let scroll = self.bots.scroll.clone();
        let parts = self.render_bot_parts(bot, cx);
        let body = div().px_6().py_5().flex().flex_col().gap_4().child(parts).child(starts).child(self.render_bot_settings(bot, cx));
        div()
            .flex_1()
            .min_h_0()
            .relative()
            .child(div().id("bot-profile").size_full().overflow_y_scroll().track_scroll(&scroll).child(body))
            .group(crate::ui::SCROLL_GROUP)
            .child(crate::ui::scrollbar(scroll))
            .into_any_element()
    }
}
