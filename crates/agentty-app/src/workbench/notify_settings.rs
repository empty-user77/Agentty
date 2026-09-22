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
use crate::ui::{action_button, chip, icon, TypeScale};
use agentty_bridge::notify::{self, Channel, Limiter, Transport};
use gpui::{div, prelude::*, px, ClickEvent, Context, Div, Entity, SharedString, Subscription, Window};
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub(super) struct ChatNotifyState {
    /// The masked field for each service's webhook URL / bot token, per transport.
    inputs: HashMap<(Channel, Transport), Entity<TextInput>>,
    /// Where a bot writes to, per service (saved as you type): a Slack channel, a Discord channel
    /// id, a Telegram chat.
    targets: HashMap<Channel, (Entity<TextInput>, Subscription)>,
    /// Whether a credential is saved, per channel and transport — a webhook and a bot are set up
    /// separately, so switching between them must not look configured when it is not.
    configured: HashMap<(Channel, Transport), bool>,
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
            "configured": per(&|c| serde_json::json!({
                "webhook": self.configured.get(&(c, Transport::Webhook)).copied(),
                "bot": self.configured.get(&(c, Transport::Bot)).copied(),
            })),
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

/// What the credential field should look like — it is a different credential per transport, so
/// the hint has to say which one is wanted.
fn secret_placeholder(channel: Channel, transport: Transport) -> &'static str {
    match (channel, transport) {
        (Channel::Slack, Transport::Webhook) => "https://hooks.slack.com/services/…",
        (Channel::Slack, Transport::Bot) => "xoxb-…",
        (Channel::Discord, Transport::Webhook) => "https://discord.com/api/webhooks/…",
        (Channel::Discord, Transport::Bot) => "MTIz….….…",
        (Channel::Telegram, _) => "123456789:AA…",
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
        let prefs = settings(cx).chat_notify.clone();
        let transport = prefs.transport(channel);
        let target = prefs.target(channel);
        // A bot with nowhere to write would fail on every message: say so once, here.
        if channel.needs_target(transport) && !notify::valid_target(channel, &target) {
            let key = if channel == Channel::Telegram { "chat.telegram_no_chat" } else { "chat.no_channel" };
            self.chat_notify.status.insert(channel, (t(cx, key).to_string(), true));
            return cx.notify();
        }
        if test {
            self.chat_notify.busy.insert(channel);
            cx.notify();
        }
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { notify::send(channel, transport, Some(&target), &message) }).await;
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
            // Both transports are looked up: the page shows whether the one in use is set up, and
            // switching to the other must not claim it is when it is not.
            let found: Vec<((Channel, Transport), bool)> = cx
                .background_spawn(async move {
                    Channel::ALL
                        .into_iter()
                        .flat_map(|c| [Transport::Webhook, Transport::Bot].map(move |t| (c, t)))
                        .filter(|(c, t)| c.supports(*t))
                        .map(|(c, t)| ((c, t), notify::configured(c, t)))
                        .collect()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.chat_notify.configured.extend(found);
                cx.notify();
            });
        })
        .detach();
    }

    fn save_chat_secret(&mut self, channel: Channel, cx: &mut Context<Self>) {
        let transport = settings(cx).chat_notify.transport(channel);
        let Some(input) = self.chat_notify.inputs.get(&(channel, transport)).cloned() else { return };
        let value = input.read(cx).text().trim().to_string();
        if let Err(reason) = notify::validate(channel, transport, &value) {
            self.chat_notify.status.insert(channel, (reason.to_string(), true));
            return cx.notify();
        }
        self.chat_notify.busy.insert(channel);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { notify::save_secret(channel, transport, &value) }).await;
            let _ = this.update(cx, |this, cx| {
                this.chat_notify.busy.remove(&channel);
                match result {
                    Ok(()) => {
                        input.update(cx, |input, cx| input.set_text("", cx));
                        this.chat_notify.configured.insert((channel, transport), true);
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
        let transport = settings(cx).chat_notify.transport(channel);
        cx.background_spawn(async move { notify::remove_secret(channel, transport) }).detach();
        self.chat_notify.configured.insert((channel, transport), false);
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
                        if let Some((input, _)) = this.chat_notify.targets.get(&Channel::Telegram) {
                            let id = id.clone();
                            input.update(cx, |input, cx| input.set_text(id, cx));
                        }
                        let saved = id.clone();
                        update_settings(cx, move |s| s.chat_notify.set_target(Channel::Telegram, saved.clone()));
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

    /// The masked credential field. Keyed by channel and transport, so switching between a webhook
    /// and a bot gives a field that asks for the right thing instead of keeping the other's hint.
    fn chat_input(&mut self, channel: Channel, transport: Transport, window: &mut Window, cx: &mut Context<Self>) -> Entity<TextInput> {
        if let Some(input) = self.chat_notify.inputs.get(&(channel, transport)) {
            return input.clone();
        }
        let input = cx.new(|cx| TextInput::new("", secret_placeholder(channel, transport), window, cx).masked());
        self.chat_notify.inputs.insert((channel, transport), input.clone());
        input
    }

    /// The field for where this service's bot writes to, kept across renders and saved as typed.
    fn target_input(&mut self, channel: Channel, window: &mut Window, cx: &mut Context<Self>) -> Entity<TextInput> {
        if let Some((input, _)) = self.chat_notify.targets.get(&channel) {
            return input.clone();
        }
        let current = settings(cx).chat_notify.target(channel);
        let placeholder = match channel {
            Channel::Slack => "chat.slack_channel_placeholder",
            Channel::Discord => "chat.discord_channel_placeholder",
            Channel::Telegram => "chat.telegram_chat_placeholder",
        };
        let input = cx.new(|cx| TextInput::localized(current, placeholder, window, cx));
        let subscription = cx.subscribe(&input, move |_, input, event: &TextInputEvent, cx| {
            if matches!(event, TextInputEvent::Changed | TextInputEvent::Confirmed) {
                let value = input.read(cx).text().trim().to_string();
                update_settings(cx, move |s| s.chat_notify.set_target(channel, value.clone()));
            }
        });
        self.chat_notify.targets.insert(channel, (input.clone(), subscription));
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
        let transport = prefs.transport(channel);
        let configured = self.chat_notify.configured.get(&(channel, transport)).copied();
        let busy = self.chat_notify.busy.contains(&channel);
        let input = self.chat_input(channel, transport, window, cx);
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
                    .child(div().t_body().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(channel.label()))
                    .child(div().t_small().text_color(hex(state.1)).child(state.0.to_string()))
                    .child(div().flex_1())
                    .when(configured == Some(true), |d| d.child(enabled_toggle)),
            )
            .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(
                cx,
                match (channel, transport) {
                    (Channel::Slack, Transport::Bot) => "chat.slack_bot_how",
                    (Channel::Slack, _) => "chat.slack_how",
                    (Channel::Discord, Transport::Bot) => "chat.discord_bot_how",
                    (Channel::Discord, _) => "chat.discord_how",
                    (Channel::Telegram, _) => "chat.telegram_how",
                },
            )))
            // Slack and Discord can be reached either way; the pick decides which credential the
            // field below is for, so it sits above it.
            .when(channel.supports(Transport::Webhook) && channel.supports(Transport::Bot), |d| {
                let mut modes = div().flex().gap_1();
                for (mode, key) in [(Transport::Webhook, "chat.by_webhook"), (Transport::Bot, "chat.by_bot")] {
                    modes = modes.child(chip(
                        SharedString::from(format!("chat-mode-{id}-{}", if mode == Transport::Bot { "bot" } else { "hook" })),
                        t(cx, key).to_string(),
                        transport == mode,
                        cx.listener(move |this, _: &ClickEvent, _, cx| {
                            update_settings(cx, move |s| s.chat_notify.set_transport(channel, mode));
                            // The field belongs to the other credential now: start it empty.
                            if let Some(input) = this.chat_notify.inputs.get(&(channel, mode)).cloned() {
                                input.update(cx, |input, cx| input.set_text("", cx));
                            }
                            this.chat_notify.status.remove(&channel);
                            cx.notify();
                        }),
                    ));
                }
                d.child(modes)
            })
            .child(div().flex().items_center().gap_2().child(field(input)).child(action_button(
                SharedString::from(format!("chat-save-{id}")),
                t(cx, "chat.save"),
                cx.listener(move |this, _: &ClickEvent, _, cx| this.save_chat_secret(channel, cx)),
            )));
        if channel.needs_target(transport) {
            let target_field = self.target_input(channel, window, cx);
            let label = match channel {
                Channel::Slack | Channel::Discord => "chat.channel",
                Channel::Telegram => "chat.telegram_chat",
            };
            card = card.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().w(px(80.)).flex_shrink_0().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, label)))
                    .child(field(target_field))
                    .when(channel == Channel::Telegram && configured == Some(true), |d| {
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
                let transport = settings(cx).chat_notify.transport(channel);
                if let Some(input) = self.chat_notify.inputs.get(&(channel, transport)).cloned() {
                    input.update(cx, |input, cx| input.set_text(value, cx));
                    self.save_chat_secret(channel, cx);
                }
            }
            // `mode <service> bot|webhook`, so the driver can set up either way.
            ("mode", Some(channel)) => {
                let mode = if parts.next() == Some("bot") { Transport::Bot } else { Transport::Webhook };
                update_settings(cx, move |s| s.chat_notify.set_transport(channel, mode));
                cx.notify();
            }
            // `target <service> <channel or chat>`.
            ("target", Some(channel)) => {
                let value = parts.next().unwrap_or("").trim().to_string();
                if let Some(input) = self.chat_notify.targets.get(&channel).map(|(input, _)| input.clone()) {
                    input.update(cx, |input, cx| input.set_text(value.clone(), cx));
                }
                update_settings(cx, move |s| s.chat_notify.set_target(channel, value.clone()));
                cx.notify();
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
