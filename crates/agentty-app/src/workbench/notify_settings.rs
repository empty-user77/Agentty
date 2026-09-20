//! Settings → Notifications, and the messages to chat services (Slack, Discord, Telegram) when an
//! agent needs an answer — so the question reaches the user's phone when they are away.
//!
//! Webhook URLs and the bot token go straight from a masked field to the credential store
//! (`agentty_bridge::notify`); the page only ever shows whether one is saved. A message names the
//! agent and the workspace; what the agent asks is added only when the user turned that on.

use super::settings_page::{row_with_hint, section, toggle};
use super::Workbench;
use crate::i18n::{t, tf};
use crate::settings::{settings, update_settings};
use crate::terminal::NoticeKind;
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::{hex, Chrome};
use crate::ui::{action_button, icon, TypeScale};
use agentty_bridge::notify::{self, Channel, Limiter};
use gpui::{div, prelude::*, px, ClickEvent, Context, Div, Entity, FontWeight, SharedString, Subscription, Window};
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub(super) struct ChatNotifyState {
    /// The masked field for each service's webhook URL / bot token.
    inputs: HashMap<Channel, Entity<TextInput>>,
    /// The Telegram chat id (saved as you type).
    chat: Option<(Entity<TextInput>, Subscription)>,
    /// Whether a secret is saved, per service (looked up off the main thread; `None`: not yet).
    configured: HashMap<Channel, bool>,
    looked_up: bool,
    /// The last result per service: text and whether it is an error.
    status: HashMap<Channel, (String, bool)>,
    busy: HashSet<Channel>,
    limiter: Limiter,
}

impl ChatNotifyState {
    pub(super) fn debug_state(&self) -> serde_json::Value {
        let per = |f: &dyn Fn(Channel) -> serde_json::Value| -> serde_json::Value {
            Channel::ALL.iter().map(|c| (c.id().to_string(), f(*c))).collect::<serde_json::Map<_, _>>().into()
        };
        serde_json::json!({
            "configured": per(&|c| self.configured.get(&c).copied().into()),
            "status": per(&|c| self.status.get(&c).map(|(text, error)| serde_json::json!([text, error])).unwrap_or_default()),
        })
    }
}

fn kind_code(kind: NoticeKind) -> u8 {
    match kind {
        NoticeKind::Finished => 0,
        NoticeKind::Permission => 1,
        NoticeKind::Question => 2,
        NoticeKind::Message => 3,
        NoticeKind::Bell => 4,
    }
}

fn secret_placeholder(channel: Channel) -> &'static str {
    match channel {
        Channel::Slack => "https://hooks.slack.com/services/…",
        Channel::Discord => "https://discord.com/api/webhooks/…",
        Channel::Telegram => "123456789:AA…",
    }
}

impl Workbench {
    /// Sends a notice to the chat services the user turned on: always when an agent needs an
    /// answer, when one finishes only if asked for. Skipped while the user looks at that pane.
    pub(super) fn send_chat_notice(
        &mut self,
        pane_id: u64,
        kind: NoticeKind,
        source: &str,
        workspace: &str,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        let in_view = self.pane_in_view(pane_id, cx);
        let prefs = settings(cx).chat_notify.clone();
        let asks = matches!(kind, NoticeKind::Permission | NoticeKind::Question);
        let finished = matches!(kind, NoticeKind::Finished | NoticeKind::Message) && prefs.on_finish;
        if in_view || !(asks || finished) {
            return;
        }
        let channels: Vec<Channel> = Channel::ALL.into_iter().filter(|c| prefs.enabled(*c)).collect();
        if channels.is_empty() || !self.chat_notify.limiter.allow(pane_id, kind_code(kind), std::time::Instant::now()) {
            return;
        }
        let headline = match kind {
            NoticeKind::Permission => t(cx, "chat.permission"),
            NoticeKind::Question => t(cx, "chat.question"),
            _ => t(cx, "chat.finished"),
        };
        let place = if workspace.is_empty() { source.to_string() } else { format!("{source} · {workspace}") };
        let mut message = format!("{headline} — {place}");
        if prefs.details && !text.trim().is_empty() {
            message.push('\n');
            message.push_str(text.trim());
        }
        for channel in channels {
            self.send_chat(channel, message.clone(), false, cx);
        }
    }

    /// Sends `message` to `channel` in the background and records the result for the settings page.
    fn send_chat(&mut self, channel: Channel, message: String, test: bool, cx: &mut Context<Self>) {
        let chat = settings(cx).chat_notify.telegram_chat.clone();
        if channel == Channel::Telegram && !notify::valid_chat_id(&chat) {
            self.chat_notify.status.insert(channel, (t(cx, "chat.telegram_no_chat").to_string(), true));
            return cx.notify();
        }
        if test {
            self.chat_notify.busy.insert(channel);
            cx.notify();
        }
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { notify::send(channel, Some(&chat), &message) }).await;
            let _ = this.update(cx, |this, cx| {
                this.chat_notify.busy.remove(&channel);
                let status = match result {
                    Ok(()) if test => (t(cx, "chat.test_sent").to_string(), false),
                    Ok(()) => (t(cx, "chat.last_sent").to_string(), false),
                    Err(err) => (format!("{err:#}"), true),
                };
                this.chat_notify.status.insert(channel, status);
                cx.notify();
            });
        })
        .detach();
    }

    /// Looks up which services have a secret saved (the credential store may be slow or ask).
    fn look_up_chat_secrets(&mut self, cx: &mut Context<Self>) {
        if self.chat_notify.looked_up {
            return;
        }
        self.chat_notify.looked_up = true;
        cx.spawn(async move |this, cx| {
            let found: Vec<(Channel, bool)> =
                cx.background_spawn(async move { Channel::ALL.into_iter().map(|c| (c, notify::configured(c))).collect() }).await;
            let _ = this.update(cx, |this, cx| {
                this.chat_notify.configured.extend(found);
                cx.notify();
            });
        })
        .detach();
    }

    fn save_chat_secret(&mut self, channel: Channel, cx: &mut Context<Self>) {
        let Some(input) = self.chat_notify.inputs.get(&channel).cloned() else { return };
        let value = input.read(cx).text().trim().to_string();
        if let Err(reason) = notify::validate(channel, &value) {
            self.chat_notify.status.insert(channel, (reason.to_string(), true));
            return cx.notify();
        }
        self.chat_notify.busy.insert(channel);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { notify::save_secret(channel, &value) }).await;
            let _ = this.update(cx, |this, cx| {
                this.chat_notify.busy.remove(&channel);
                match result {
                    Ok(()) => {
                        input.update(cx, |input, cx| input.set_text("", cx));
                        this.chat_notify.configured.insert(channel, true);
                        this.chat_notify.status.insert(channel, (t(cx, "chat.saved").to_string(), false));
                        update_settings(cx, move |s| s.chat_notify.set_enabled(channel, true));
                    }
                    Err(err) => {
                        this.chat_notify.status.insert(channel, (format!("{err:#}"), true));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn remove_chat_secret(&mut self, channel: Channel, cx: &mut Context<Self>) {
        cx.background_spawn(async move { notify::remove_secret(channel) }).detach();
        self.chat_notify.configured.insert(channel, false);
        self.chat_notify.status.remove(&channel);
        update_settings(cx, move |s| s.chat_notify.set_enabled(channel, false));
        cx.notify();
    }

    /// Fills the Telegram chat id from the last message the user sent the bot.
    fn find_telegram_chat(&mut self, cx: &mut Context<Self>) {
        self.chat_notify.busy.insert(Channel::Telegram);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { notify::telegram_latest_chat() }).await;
            let _ = this.update(cx, |this, cx| {
                this.chat_notify.busy.remove(&Channel::Telegram);
                match result {
                    Ok((id, name)) => {
                        if let Some((input, _)) = &this.chat_notify.chat {
                            let id = id.clone();
                            input.update(cx, |input, cx| input.set_text(id, cx));
                        }
                        let saved = id.clone();
                        update_settings(cx, move |s| s.chat_notify.telegram_chat = saved);
                        this.chat_notify
                            .status
                            .insert(Channel::Telegram, (tf(cx, "chat.telegram_found", &[("name", &name), ("id", &id)]), false));
                    }
                    Err(err) => {
                        this.chat_notify.status.insert(Channel::Telegram, (format!("{err:#}"), true));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn chat_input(&mut self, channel: Channel, window: &mut Window, cx: &mut Context<Self>) -> Entity<TextInput> {
        if let Some(input) = self.chat_notify.inputs.get(&channel) {
            return input.clone();
        }
        let input = cx.new(|cx| TextInput::new("", secret_placeholder(channel), window, cx).masked());
        self.chat_notify.inputs.insert(channel, input.clone());
        input
    }

    fn telegram_chat_input(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<TextInput> {
        if let Some((input, _)) = &self.chat_notify.chat {
            return input.clone();
        }
        let chat = settings(cx).chat_notify.telegram_chat.clone();
        let input = cx.new(|cx| TextInput::localized(chat, "chat.telegram_chat_placeholder", window, cx));
        let subscription = cx.subscribe(&input, |_, input, event: &TextInputEvent, cx| {
            if matches!(event, TextInputEvent::Changed | TextInputEvent::Confirmed) {
                let chat = input.read(cx).text().trim().to_string();
                update_settings(cx, move |s| s.chat_notify.telegram_chat = chat);
            }
        });
        self.chat_notify.chat = Some((input.clone(), subscription));
        input
    }

    pub(super) fn render_notification_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        self.look_up_chat_secrets(cx);
        let prefs = settings(cx).clone();
        let desktop = section(t(cx, "settings.notifications_desktop"))
            .child(row_with_hint(
                t(cx, "settings.system_notifications"),
                t(cx, "settings.system_notifications_hint"),
                toggle("system-notifications", prefs.system_notifications, |s| s.system_notifications = !s.system_notifications, cx),
            ))
            .child(row_with_hint(
                t(cx, "settings.notify_answer_requests"),
                t(cx, "settings.notify_answer_requests_hint"),
                toggle("notify-answers", prefs.notify_answer_requests, |s| s.notify_answer_requests = !s.notify_answer_requests, cx),
            ))
            .child(row_with_hint(
                t(cx, "settings.notify_when_focused"),
                t(cx, "settings.notify_when_focused_hint"),
                toggle("notify-focused", prefs.notify_when_focused, |s| s.notify_when_focused = !s.notify_when_focused, cx),
            ));

        let mut chats =
            section(t(cx, "settings.notifications_chat")).child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "chat.intro")));
        for channel in Channel::ALL {
            chats = chats.child(self.render_chat_card(channel, window, cx));
        }
        chats = chats
            .child(row_with_hint(
                t(cx, "chat.on_finish"),
                t(cx, "chat.on_finish_hint"),
                toggle("chat-on-finish", prefs.chat_notify.on_finish, |s| s.chat_notify.on_finish = !s.chat_notify.on_finish, cx),
            ))
            .child(row_with_hint(
                t(cx, "chat.details"),
                t(cx, "chat.details_hint"),
                toggle("chat-details", prefs.chat_notify.details, |s| s.chat_notify.details = !s.chat_notify.details, cx),
            ));
        div().flex().flex_col().child(desktop).child(chats)
    }

    fn render_chat_card(&mut self, channel: Channel, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let prefs = settings(cx).chat_notify.clone();
        let configured = self.chat_notify.configured.get(&channel).copied();
        let busy = self.chat_notify.busy.contains(&channel);
        let input = self.chat_input(channel, window, cx);
        let id = channel.id();
        let field = |input: Entity<TextInput>| {
            div()
                .flex_1()
                .min_w_0()
                .px_2()
                .py_1()
                .rounded_md()
                .border_1()
                .border_color(hex(Chrome::BORDER))
                .bg(hex(0x1a1a1a))
                .t_body()
                .child(input)
        };
        let state = match configured {
            Some(true) => (t(cx, "chat.connected"), Chrome::SUCCESS),
            Some(false) => (t(cx, "chat.not_set_up"), Chrome::MUTED),
            None => ("…", Chrome::MUTED),
        };
        let enabled_toggle = match channel {
            Channel::Slack => toggle("chat-slack", prefs.slack, |s| s.chat_notify.slack = !s.chat_notify.slack, cx).into_any_element(),
            Channel::Discord => {
                toggle("chat-discord", prefs.discord, |s| s.chat_notify.discord = !s.chat_notify.discord, cx).into_any_element()
            }
            Channel::Telegram => {
                toggle("chat-telegram", prefs.telegram, |s| s.chat_notify.telegram = !s.chat_notify.telegram, cx).into_any_element()
            }
        };
        let mut card = div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(hex(Chrome::BORDER))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(icon(if channel == Channel::Telegram { "send" } else { "message-square" }, 14., hex(Chrome::FOREGROUND)))
                    .child(div().t_body().font_weight(FontWeight::SEMIBOLD).text_color(hex(Chrome::BRIGHT)).child(channel.label()))
                    .child(div().t_small().text_color(hex(state.1)).child(state.0.to_string()))
                    .child(div().flex_1())
                    .when(configured == Some(true), |d| d.child(enabled_toggle)),
            )
            .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(
                cx,
                match channel {
                    Channel::Slack => "chat.slack_how",
                    Channel::Discord => "chat.discord_how",
                    Channel::Telegram => "chat.telegram_how",
                },
            )))
            .child(div().flex().items_center().gap_2().child(field(input)).child(action_button(
                SharedString::from(format!("chat-save-{id}")),
                t(cx, "chat.save"),
                cx.listener(move |this, _: &ClickEvent, _, cx| this.save_chat_secret(channel, cx)),
            )));
        if channel == Channel::Telegram {
            let chat = self.telegram_chat_input(window, cx);
            card = card.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().w(px(80.)).flex_shrink_0().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "chat.telegram_chat")))
                    .child(field(chat))
                    .when(configured == Some(true), |d| {
                        d.child(action_button(
                            "chat-telegram-find",
                            t(cx, "chat.telegram_find"),
                            cx.listener(|this, _: &ClickEvent, _, cx| this.find_telegram_chat(cx)),
                        ))
                    }),
            );
        }
        if configured == Some(true) {
            card = card.child(
                div()
                    .flex()
                    .gap_2()
                    .child(action_button(
                        SharedString::from(format!("chat-test-{id}")),
                        t(cx, "chat.test"),
                        cx.listener(move |this, _: &ClickEvent, _, cx| {
                            let message = t(cx, "chat.test_message").to_string();
                            this.send_chat(channel, message, true, cx)
                        }),
                    ))
                    .child(action_button(
                        SharedString::from(format!("chat-remove-{id}")),
                        t(cx, "chat.remove"),
                        cx.listener(move |this, _: &ClickEvent, _, cx| this.remove_chat_secret(channel, cx)),
                    )),
            );
        }
        if busy {
            card = card.child(crate::ui::loading_row(t(cx, "chat.working")));
        } else if let Some((text, error)) = self.chat_notify.status.get(&channel) {
            card = card.child(div().t_small().text_color(hex(if *error { Chrome::ERROR } else { Chrome::SUCCESS })).child(text.clone()));
        }
        card
    }

    /// Debug driver: `chat-notify save <channel> <secret>`, `test <channel>`, `remove <channel>`,
    /// `notice <pane> <kind>` (as if that pane asked, with Agentty in the background).
    pub(super) fn debug_chat_notify(&mut self, argument: &str, cx: &mut Context<Self>) {
        let mut parts = argument.splitn(3, ' ');
        let verb = parts.next().unwrap_or("");
        let channel = parts.next().and_then(|name| Channel::ALL.into_iter().find(|c| c.id() == name));
        match (verb, channel) {
            ("save", Some(channel)) => {
                let value = parts.next().unwrap_or("").to_string();
                if let Some(input) = self.chat_notify.inputs.get(&channel).cloned() {
                    input.update(cx, |input, cx| input.set_text(value, cx));
                    self.save_chat_secret(channel, cx);
                }
            }
            ("test", Some(channel)) => {
                let message = t(cx, "chat.test_message").to_string();
                self.send_chat(channel, message, true, cx);
            }
            ("remove", Some(channel)) => self.remove_chat_secret(channel, cx),
            ("notice", _) => {
                let kind = match argument.rsplit(' ').next() {
                    Some("finished") => NoticeKind::Finished,
                    Some("permission") => NoticeKind::Permission,
                    _ => NoticeKind::Question,
                };
                self.send_chat_notice(0, kind, "Claude Code", "debug", "Allow Bash: ls", cx);
            }
            _ => {}
        }
        cx.notify();
    }
}
