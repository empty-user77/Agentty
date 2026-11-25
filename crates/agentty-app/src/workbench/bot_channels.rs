//! Bots on Telegram, Slack and Discord. While a bot takes messages from a channel, one thread
//! holds that channel's connection open (a long poll, a socket) and sleeps in the network until
//! something arrives; the window is only woken for a message or a change of the connection's
//! state. No bot on a channel, no connection.
//!
//! What arrives is checked against the bots' allowed senders before anything else, noted in the
//! bot's audit log, and put in its mailbox in a frame that says where it is from. The answer goes
//! back to the chat the message came from and nowhere else.
//!
//! An allowed sender can also run the bots from there: ask what they are doing, switch one on or
//! off, stop its turn, and pick an option when its agent asks for an approval. What a bot did by
//! itself (a schedule, a watch) is reported to the chat that sender last wrote to it from alone.

use super::bots::BotState;
use super::bots::Origin;
use super::Pane;
use super::Workbench;
use crate::i18n::t;
use crate::i18n::tf;
use crate::text_input::TextInput;
use crate::theme::{hex, Chrome};
use crate::ui::TypeScale as _;
use agentty_bridge::bots::channels::{self, Ask, Incoming, Routed, Secret};
use agentty_bridge::bots::{Bot, BotChannel, ChannelKind};
use futures::StreamExt as _;
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, Entity};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub(super) const KINDS: [ChannelKind; 3] = [ChannelKind::Telegram, ChannelKind::Slack, ChannelKind::Discord];
/// An agent's question is on its screen a moment after its signal: the options are read then.
const ASK_SETTLE: Duration = Duration::from_millis(1200);
/// What a bot last said, as a status asked from a channel quotes it.
const STATUS_QUOTE: usize = 600;
/// A connection that held this long was a working one: the next is opened without a growing wait.
const STEADY: Duration = Duration::from_secs(60);
/// A sender on nobody's list is noted in the logs this often at most.
const REFUSED_NOTE_EVERY_MS: u64 = 60_000;
const REFUSED_NOTED_MAX: usize = 64;

/// What a bot asked on a channel: the options as they were on its screen, and the chats told.
struct Asked {
    choices: Vec<String>,
    chats: Vec<(ChannelKind, String)>,
}

/// An option that goes on holding after this once ("don't ask again", "allow all edits during
/// this session"): given in the app only, where what it covers is on the screen.
fn lasting(option: &str) -> bool {
    let option = option.to_lowercase().replace('\u{2019}', "'");
    ["don't ask again", "dont ask again", "always", "allow all", "this session", "this project", "this conversation"]
        .iter()
        .any(|mark| option.contains(mark))
}

/// Whether two addresses are the same chat (Slack's carries the thread after a `/`).
fn same_chat(one: &str, other: &str) -> bool {
    let room = |chat: &str| chat.split('/').next().unwrap_or_default().to_string();
    room(one) == room(other)
}

/// The secrets a channel needs, with the key of each one's name.
pub(super) fn secrets_of(kind: ChannelKind) -> &'static [(Secret, &'static str)] {
    match kind {
        ChannelKind::Telegram => &[(Secret::TelegramBot, "bots.token_telegram")],
        ChannelKind::Slack => &[(Secret::SlackApp, "bots.token_slack_app"), (Secret::SlackBot, "bots.token_slack_bot")],
        ChannelKind::Discord => &[(Secret::DiscordBot, "bots.token_telegram")],
    }
}

enum Event {
    Message(Incoming),
    /// Why the channel is not connected, or `None` once it is.
    State(Option<String>),
}

/// A channel's connection. Dropping it tells its thread to end at its next look.
struct Listener {
    stop: Arc<AtomicBool>,
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[derive(Default)]
pub struct ChannelsState {
    listeners: HashMap<ChannelKind, Listener>,
    /// Which secrets are in the keychain, as last looked up (off the main thread, never in `render`).
    saved: HashMap<Secret, bool>,
    looked_up: bool,
    trouble: HashMap<ChannelKind, String>,
    /// What was last sent out, newest last, a handful kept: the debug driver reads it in place of
    /// a real channel.
    pub(super) sent: Vec<(ChannelKind, String, String)>,
    /// What each bot last asked on a channel. A number picks an option only from a chat that was
    /// told, and only while the bot's screen still shows the options told.
    asked: HashMap<String, Asked>,
    /// When a sender on nobody's list was last noted in the logs, so a flood of them does not
    /// push the logs' older lines out. A handful kept.
    refused: HashMap<(ChannelKind, String), u64>,
}

/// Sleeps `total` in short steps, so a stopped listener ends soon.
fn wait(total: Duration, stop: &AtomicBool) {
    let mut left = total;
    while !left.is_zero() && !stop.load(Ordering::Relaxed) {
        let step = left.min(Duration::from_secs(1));
        std::thread::sleep(step);
        left -= step;
    }
}

/// The body of a channel's thread: wait for messages, reconnect with growing pauses when the
/// connection fails, end when `stop` is set or nobody listens any more.
fn listen(kind: ChannelKind, stop: Arc<AtomicBool>, events: futures::channel::mpsc::UnboundedSender<Event>) {
    // `reported`: the trouble the window was last told of (`None`: connected, as far as it knows).
    let (mut attempt, mut offset, mut reported) = (0u32, None, None::<String>);
    while !stop.load(Ordering::Relaxed) {
        let started = std::time::Instant::now();
        let result = match kind {
            ChannelKind::Telegram => match channels::load_secret(Secret::TelegramBot) {
                None => Err(anyhow::anyhow!("Telegram is not set up")),
                // The token is read once per connection, not once per poll.
                Some(token) => loop {
                    if stop.load(Ordering::Relaxed) {
                        break Ok(());
                    }
                    match channels::telegram::get_updates(&token, offset) {
                        Ok((messages, next)) => {
                            offset = next.or(offset);
                            attempt = 0;
                            if reported.take().is_some() {
                                let _ = events.unbounded_send(Event::State(None));
                            }
                            for message in messages {
                                let _ = events.unbounded_send(Event::Message(message));
                            }
                        }
                        Err(err) => break Err(err),
                    }
                },
            },
            ChannelKind::Slack => match channels::load_secret(Secret::SlackApp) {
                None => Err(anyhow::anyhow!("Slack is not set up")),
                Some(token) => {
                    let sender = events.clone();
                    channels::slack::listen(&token, &stop, move |message| {
                        let _ = sender.unbounded_send(Event::Message(message));
                    })
                }
            },
            ChannelKind::Discord => match channels::load_secret(Secret::DiscordBot) {
                None => Err(anyhow::anyhow!("Discord is not set up")),
                Some(token) => {
                    let (sender, state) = (events.clone(), events.clone());
                    let mut connected = false;
                    channels::discord::listen(&token, &stop, move |message| {
                        // A message arrived: whatever was wrong with the connection is over.
                        if !std::mem::replace(&mut connected, true) {
                            let _ = state.unbounded_send(Event::State(None));
                        }
                        let _ = sender.unbounded_send(Event::Message(message));
                    })
                }
            },
        };
        if events.is_closed() {
            break;
        }
        match result {
            Ok(()) => {
                if reported.take().is_some() {
                    let _ = events.unbounded_send(Event::State(None));
                }
                // The other side ended it. Each connection signs in anew and the channels count
                // those: one that ended right away is followed by a growing wait, like a failure.
                if started.elapsed() < STEADY {
                    wait(channels::backoff(attempt), &stop);
                    attempt = attempt.saturating_add(1);
                } else {
                    attempt = 0;
                    wait(Duration::from_secs(1), &stop);
                }
            }
            Err(err) => {
                let text = err.to_string();
                if reported.as_deref() != Some(text.as_str()) {
                    let _ = events.unbounded_send(Event::State(Some(text.clone())));
                    reported = Some(text);
                }
                wait(channels::backoff(attempt), &stop);
                attempt = attempt.saturating_add(1);
            }
        }
    }
}

fn senders_from(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for sender in text.split(|c: char| c == ',' || c.is_whitespace()).map(str::trim).filter(|s| !s.is_empty()) {
        if !out.iter().any(|known| known == sender) {
            out.push(sender.to_string());
        }
    }
    out
}

impl Workbench {
    /// Opens the connection of every channel a bot uses, and closes the others. A bot switched
    /// off keeps its channel's connection: that is where its allowed sender switches it on again.
    pub(super) fn bot_channels_changed(&mut self, cx: &mut Context<Self>) {
        if !self.hosts_bots() {
            return;
        }
        let on = self.prefs(cx).bots_enabled;
        for kind in KINDS {
            let wanted = on && self.bots.list.iter().any(|bot| bot.channels.iter().any(|c| c.kind == kind));
            if !wanted {
                self.bots.channels.listeners.remove(&kind);
                self.bots.channels.trouble.remove(&kind);
            } else if !self.bots.channels.listeners.contains_key(&kind) {
                self.start_bot_channel(kind, cx);
            }
        }
    }

    fn start_bot_channel(&mut self, kind: ChannelKind, cx: &mut Context<Self>) {
        let stop = Arc::new(AtomicBool::new(false));
        let (events, mut inbox) = futures::channel::mpsc::unbounded();
        let flag = stop.clone();
        if std::thread::Builder::new().name(format!("agentty-bot-{}", kind.id())).spawn(move || listen(kind, flag, events)).is_err() {
            return;
        }
        self.bots.channels.listeners.insert(kind, Listener { stop });
        cx.spawn(async move |this, cx| {
            while let Some(event) = inbox.next().await {
                if this.update(cx, |this, cx| this.bot_channel_event(kind, event, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn bot_channel_event(&mut self, kind: ChannelKind, event: Event, cx: &mut Context<Self>) {
        match event {
            Event::State(trouble) => {
                let changed = match trouble {
                    Some(text) => self.bots.channels.trouble.insert(kind, text.clone()) != Some(text),
                    None => self.bots.channels.trouble.remove(&kind).is_some(),
                };
                if changed {
                    cx.notify();
                }
            }
            Event::Message(incoming) => self.bot_channel_message(incoming, cx),
        }
    }

    pub(super) fn bot_channel_message(&mut self, incoming: Incoming, cx: &mut Context<Self>) {
        let source = format!("{}:{}", incoming.kind.id(), incoming.sender);
        let now = crate::ui::now_ms();
        let audit = |handles: Vec<String>, what: String, cx: &mut Context<Self>| {
            let source = source.clone();
            cx.background_spawn(async move {
                let store = crate::bots::store();
                for handle in handles {
                    channels::audit(&store, &handle, now, &source, &what);
                }
            })
            .detach();
        };
        let reach: Vec<String> = channels::reachable(&incoming, &self.bots.list).iter().map(|bot| bot.handle.clone()).collect();
        // Nobody this sender may write to: noted with every bot on the channel, answered by none.
        if reach.is_empty() {
            let noted = &mut self.bots.channels.refused;
            let key = (incoming.kind, incoming.sender.clone());
            if noted.get(&key).is_some_and(|at| now.saturating_sub(*at) < REFUSED_NOTE_EVERY_MS) {
                return;
            }
            if noted.len() >= REFUSED_NOTED_MAX {
                noted.clear();
            }
            noted.insert(key, now);
            let listening: Vec<String> = self
                .bots
                .list
                .iter()
                .filter(|bot| bot.channels.iter().any(|c| c.kind == incoming.kind))
                .map(|bot| bot.handle.clone())
                .collect();
            return audit(listening, "refused: the sender is not on this bot's list".into(), cx);
        }
        // Written while nobody listened and only handed over now: a switch, a stop or an option
        // picked was about that moment, and is not acted on.
        if incoming.stale {
            let of_its_moment = channels::option_number(&incoming.text).is_some()
                || matches!(channels::ask(&incoming.text, &reach), Some(Ask::Switch { .. } | Ask::Stop(_)));
            if of_its_moment {
                return audit(reach, "ignored: it was sent before Agentty was listening".into(), cx);
            }
        }
        // Something asked of Agentty itself, about the bots this sender can reach.
        if let Some(ask) = channels::ask(&incoming.text, &reach) {
            audit(reach.clone(), format!("asked Agentty: {}", incoming.text.trim()), cx);
            let answer = self.bot_channel_ask(ask, &reach, cx);
            return self.send_to_channel(incoming.kind, incoming.chat, answer, cx);
        }
        // Written to a bot that is switched off: said, and handed to no other bot.
        let off = channels::named(&incoming.text, &reach).and_then(|handle| self.bots.get(&handle).filter(|bot| !bot.enabled));
        if let Some(bot) = off {
            let text = tf(cx, "bots.ch_is_off", &[("name", &bot.name), ("bot", &bot.handle)]);
            return self.send_to_channel(incoming.kind, incoming.chat, text, cx);
        }
        match channels::route(&incoming, &self.bots.list) {
            // On a list, and every bot of theirs is switched off.
            Routed::Refused => {
                let text = format!("{}\n{}", t(cx, "bots.ch_all_off"), self.bot_channel_overview(&reach, cx));
                self.send_to_channel(incoming.kind, incoming.chat, text, cx);
            }
            Routed::Unclear(handles) => {
                // A number alone answers the one bot that asks something.
                let asking: Vec<&String> =
                    handles.iter().filter(|handle| self.bot_asked_in(handle, incoming.kind, &incoming.chat)).collect();
                if let (Some(option), [handle]) = (channels::option_number(&incoming.text), asking.as_slice()) {
                    let handle = (*handle).clone();
                    audit(vec![handle.clone()], format!("picked option {}", option + 1), cx);
                    let answer = self.bot_channel_pick(&handle, option, cx);
                    return self.send_to_channel(incoming.kind, incoming.chat, answer, cx);
                }
                let names: Vec<String> = handles.iter().map(|handle| format!("/{handle}")).collect();
                self.send_to_channel(incoming.kind, incoming.chat, tf(cx, "bots.ch_which", &[("bots", &names.join(", "))]), cx);
            }
            Routed::To(handle, body) => {
                if let Some(option) = channels::option_number(&body).filter(|_| self.bot_asked_in(&handle, incoming.kind, &incoming.chat)) {
                    audit(vec![handle.clone()], format!("picked option {}", option + 1), cx);
                    let answer = self.bot_channel_pick(&handle, option, cx);
                    return self.send_to_channel(incoming.kind, incoming.chat, answer, cx);
                }
                audit(vec![handle.clone()], body.clone(), cx);
                self.remember_report_chat(&handle, &incoming, cx);
                let from = Origin::Channel { kind: incoming.kind, chat: incoming.chat.clone(), sender: incoming.sender };
                if let Err(why) = self.bot_post(&handle, from, body, 0, cx) {
                    self.send_to_channel(incoming.kind, incoming.chat, why, cx);
                }
            }
        }
    }

    /// The chat an allowed sender wrote to the bot from alone becomes where its reports go.
    fn remember_report_chat(&mut self, handle: &str, incoming: &Incoming, cx: &mut Context<Self>) {
        if !incoming.direct {
            return;
        }
        // Slack's address carries the thread; a report starts its own.
        let chat = incoming.chat.split('/').next().unwrap_or_default().to_string();
        let kind = incoming.kind;
        let known = self.bots.get(handle).and_then(|bot| bot.channels.iter().find(|c| c.kind == kind)).map(|c| c.report_chat.clone());
        if BotChannel::chat_looks_right(&chat) && known.is_some_and(|known| known != chat) {
            self.change_bot(
                handle,
                |bot| {
                    if let Some(channel) = bot.channels.iter_mut().find(|c| c.kind == kind) {
                        channel.report_chat = chat;
                    }
                },
                cx,
            );
        }
    }

    /// The bots `handles` name, one line each: its name, how to write to it, and what it is doing.
    fn bot_channel_overview(&self, handles: &[String], cx: &gpui::App) -> String {
        let rows: Vec<String> = self
            .bots
            .list
            .iter()
            .filter(|bot| handles.contains(&bot.handle))
            .map(|bot| {
                let state = self.bot_state(bot, cx);
                let (mark, word) = match state {
                    BotState::Off => ("○", t(cx, "bots.state_off")),
                    BotState::Idle => ("●", t(cx, "bots.ch_idle")),
                    BotState::Working => ("▶", t(cx, "bots.state_working")),
                    BotState::NeedsYou => ("!", t(cx, "bots.needs_you")),
                };
                let line =
                    self.bot_line(bot, state, cx).filter(|_| state != BotState::Idle).map(|line| format!(" · {line}")).unwrap_or_default();
                format!("{mark} {} (/{}) · {word}{line}", bot.name, bot.handle)
            })
            .collect();
        rows.join("\n")
    }

    /// Does what an allowed sender asked of Agentty, and words the answer.
    fn bot_channel_ask(&mut self, ask: Ask, reach: &[String], cx: &mut Context<Self>) -> String {
        let name = |this: &Self, handle: &str| this.bots.get(handle).map_or_else(|| handle.to_string(), |bot| bot.name.clone());
        match ask {
            Ask::Bots => self.bot_channel_overview(reach, cx),
            Ask::Help => t(cx, "bots.ch_help").to_string(),
            Ask::NoSuchBot(named) => format!("{}\n{}", tf(cx, "bots.ch_no_bot", &[("name", &named)]), self.bot_channel_overview(reach, cx)),
            Ask::Status(handle) => {
                let mut text = self.bot_channel_overview(std::slice::from_ref(&handle), cx);
                let said = super::bots::Party::Bot(handle.clone());
                let last = self.bots.chats.get(&handle).and_then(|lines| lines.iter().rev().find(|line| line.from == said));
                if let Some(last) = last {
                    text = format!("{text}\n\n{}", agentty_bridge::fsutil::truncate_chars(last.text.trim(), STATUS_QUOTE));
                }
                text
            }
            Ask::Switch { handle, on } => {
                let was = self.bots.get(&handle).is_some_and(|bot| bot.enabled);
                if was != on && !self.change_bot(&handle, |bot| bot.enabled = on, cx) {
                    return t(cx, "bots.error_save").to_string();
                }
                tf(cx, if on { "bots.ch_on" } else { "bots.ch_off" }, &[("name", &name(self, &handle))])
            }
            Ask::Stop(handle) => {
                let working = self.bot_pane(&handle, cx).filter(|pane| pane.read(cx).status.in_turn());
                match working {
                    Some(pane) => {
                        self.remote_stop_turn(&pane, cx);
                        tf(cx, "bots.ch_stopped", &[("name", &name(self, &handle))])
                    }
                    None => tf(cx, "bots.ch_not_working", &[("name", &name(self, &handle))]),
                }
            }
        }
    }

    /// The pane of `handle` whose agent stopped at a question or an approval.
    fn bot_asking_pane(&self, handle: &str, cx: &gpui::App) -> Option<Pane> {
        self.all_panes().into_iter().find(|pane| {
            let view = pane.read(cx);
            view.spec.bot.as_deref() == Some(handle) && view.status.needs_user()
        })
    }

    /// Whether the bot's question was told to this chat: only there does a number answer it.
    fn bot_asked_in(&self, handle: &str, kind: ChannelKind, chat: &str) -> bool {
        self.bots.channels.asked.get(handle).is_some_and(|asked| asked.chats.iter().any(|(told, to)| *told == kind && same_chat(to, chat)))
    }

    /// Picks an option of what the bot's agent asks, as the chat's buttons do, and says which.
    /// Only of the question as it was told: one that changed since, or was never told, is
    /// answered where it can be read. An option that goes on holding is given in the app only.
    fn bot_channel_pick(&mut self, handle: &str, option: usize, cx: &mut Context<Self>) -> String {
        let Some(pane) = self.bot_asking_pane(handle, cx) else {
            self.bots.channels.asked.remove(handle);
            return t(cx, "bots.ch_no_question").to_string();
        };
        let choices = super::chat::screen_choices(&pane.read(cx).screen_lines(40)).map(|(choices, _)| choices).unwrap_or_default();
        if self.bots.channels.asked.get(handle).is_none_or(|asked| asked.choices != choices) {
            self.bots.channels.asked.remove(handle);
            return t(cx, "bots.ch_answer_in_app").to_string();
        }
        match choices.get(option) {
            Some(choice) if lasting(choice) => t(cx, "bots.ch_answer_in_app").to_string(),
            Some(choice) => {
                let picked = format!("✓ {}", channels::scrub(choice));
                self.bots.channels.asked.remove(handle);
                self.remote_answer_choice(&pane, option, cx);
                picked
            }
            None => t(cx, "bots.ch_no_option").to_string(),
        }
    }

    /// A bot's agent stopped at a question or an approval: said where the user reads their
    /// messages, with the options to pick from there.
    pub(super) fn bot_asks_user(&mut self, pane: &Pane, cx: &mut Context<Self>) {
        let Some(handle) = pane.read(cx).spec.bot.clone() else { return };
        if !self.hosts_bots() || self.bot_report_chats(&handle, true).is_empty() {
            return;
        }
        let pane = pane.clone();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(ASK_SETTLE).await;
            let _ = this.update(cx, |this, cx| this.bot_tell_ask(&handle, &pane, cx));
        })
        .detach();
    }

    fn bot_tell_ask(&mut self, handle: &str, pane: &Pane, cx: &mut Context<Self>) {
        let (what, choices) = {
            let view = pane.read(cx);
            let what = match &view.status {
                crate::terminal::AgentStatus::Permission(what) | crate::terminal::AgentStatus::Question(what) => {
                    what.clone().unwrap_or_default()
                }
                // Answered meanwhile, in the app.
                _ => return,
            };
            (what, super::chat::screen_choices(&view.screen_lines(40)).map(|(choices, _)| choices).unwrap_or_default())
        };
        let name = self.bots.get(handle).map_or_else(|| handle.to_string(), |bot| bot.name.clone());
        let mut text = tf(cx, "bots.ch_asks", &[("name", &name)]);
        if !what.trim().is_empty() {
            text = format!("{text}\n{}", what.trim());
        }
        // Each keeps the number it has on the screen; one that goes on holding is not offered.
        let offered: Vec<(usize, &String)> = choices.iter().enumerate().take(9).filter(|(_, choice)| !lasting(choice)).collect();
        for (at, choice) in &offered {
            text = format!("{text}\n{}. {choice}", at + 1);
        }
        let how = if offered.is_empty() { t(cx, "bots.ch_answer_in_app").to_string() } else { tf(cx, "bots.ch_pick", &[("bot", handle)]) };
        let pickable = !offered.is_empty();
        let text = channels::scrub(&format!("{text}\n{how}"));
        let chats = self.bot_report_chats(handle, true);
        if pickable {
            self.bots.channels.asked.insert(handle.to_string(), Asked { choices, chats: chats.clone() });
        } else {
            self.bots.channels.asked.remove(handle);
        }
        for (kind, chat) in chats {
            self.send_to_channel(kind, chat, text.clone(), cx);
        }
    }

    /// Where a bot's news goes. What it asks while working on a channel's message goes to that
    /// chat; anything else to each channel's report chat, where reports are on.
    fn bot_report_chats(&self, handle: &str, asking: bool) -> Vec<(ChannelKind, String)> {
        if let (true, Some(Origin::Channel { kind, chat, .. })) = (asking, self.bots.mailbox.working_on(handle).map(|mail| &mail.from)) {
            return vec![(*kind, chat.clone())];
        }
        let Some(bot) = self.bots.get(handle) else { return Vec::new() };
        bot.channels.iter().filter(|c| c.reports && !c.report_chat.is_empty()).map(|c| (c.kind, c.report_chat.clone())).collect()
    }

    /// What a bot did by itself (a schedule, a watch, another bot's answer), reported outside.
    pub(super) fn bot_channel_report(&mut self, handle: &str, report: &str, cx: &mut Context<Self>) {
        if !self.hosts_bots() {
            return;
        }
        let name = self.bots.get(handle).map_or_else(|| handle.to_string(), |bot| bot.name.clone());
        let (owner, now) = (handle.to_string(), crate::ui::now_ms());
        for (kind, chat) in self.bot_report_chats(handle, false) {
            let (owner, note) = (owner.clone(), format!("reported to {}:{chat}", kind.id()));
            cx.background_spawn(async move { channels::audit(&crate::bots::store(), &owner, now, "agentty", &note) }).detach();
            self.send_to_channel(kind, chat, format!("[{name}] {report}"), cx);
        }
    }

    /// A bot's answer to a message from a channel, sent to the chat it came from.
    pub(super) fn bot_channel_reply(&mut self, handle: &str, kind: ChannelKind, chat: String, reply: String, cx: &mut Context<Self>) {
        let (handle, now) = (handle.to_string(), crate::ui::now_ms());
        let note = format!("answered {}:{chat}", kind.id());
        cx.background_spawn(async move { channels::audit(&crate::bots::store(), &handle, now, "agentty", &note) }).detach();
        self.send_to_channel(kind, chat, reply, cx);
    }

    fn send_to_channel(&mut self, kind: ChannelKind, chat: String, text: String, cx: &mut Context<Self>) {
        // Whatever an agent was talked into saying, no credential leaves through a channel.
        let text = channels::scrub_sent(&text);
        self.bots.channels.sent.push((kind, chat.clone(), text.clone()));
        let over = self.bots.channels.sent.len().saturating_sub(16);
        self.bots.channels.sent.drain(..over);
        cx.background_spawn(async move {
            let sent = match kind {
                ChannelKind::Telegram => match channels::load_secret(Secret::TelegramBot) {
                    Some(token) => channels::telegram::send(&token, &chat, &text),
                    None => Err(anyhow::anyhow!("Telegram is not set up")),
                },
                ChannelKind::Slack => match channels::load_secret(Secret::SlackBot) {
                    Some(token) => channels::slack::send(&token, &chat, &text),
                    None => Err(anyhow::anyhow!("Slack is not set up")),
                },
                ChannelKind::Discord => match channels::load_secret(Secret::DiscordBot) {
                    Some(token) => channels::discord::send(&token, &chat, &text),
                    None => Err(anyhow::anyhow!("Discord is not set up")),
                },
            };
            if let Err(err) = sent {
                eprintln!("agentty: a bot's answer was not sent: {err}");
            }
        })
        .detach();
    }

    fn look_up_bot_secrets(&mut self, cx: &mut Context<Self>) {
        self.bots.channels.looked_up = true;
        let read = cx.background_spawn(async move { channels::SECRETS.map(|secret| (secret, channels::has_secret(secret))) });
        cx.spawn(async move |this, cx| {
            let saved = read.await;
            let _ = this.update(cx, |this, cx| {
                this.bots.channels.saved = saved.into_iter().collect();
                cx.notify();
            });
        })
        .detach();
    }

    fn save_bot_secret(&mut self, kind: ChannelKind, secret: Secret, input: Entity<TextInput>, cx: &mut Context<Self>) {
        let value = input.read(cx).text().trim().to_string();
        if !secret.looks_right(&value) {
            return self.show_toast(t(cx, "bots.error_token"), cx);
        }
        let save = cx.background_spawn(async move { channels::save_secret(secret, &value).is_ok() });
        cx.spawn(async move |this, cx| {
            let saved = save.await;
            let _ = this.update(cx, |this, cx| {
                if !saved {
                    return this.show_toast(t(cx, "bots.error_save"), cx);
                }
                input.update(cx, |input, cx| input.set_text("", cx));
                this.bots.channels.saved.insert(secret, true);
                // Connect again with the new token.
                this.bots.channels.listeners.remove(&kind);
                this.bot_channels_changed(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn save_bot_senders(&mut self, handle: &str, kind: ChannelKind, input: &Entity<TextInput>, cx: &mut Context<Self>) {
        let allowed = senders_from(input.read(cx).text());
        self.change_bot(
            handle,
            |bot| {
                let before = bot.channels.iter().position(|c| c.kind == kind).map(|at| bot.channels.remove(at));
                // No sender allowed is no channel: one open to anybody cannot be saved.
                if !allowed.is_empty() {
                    let mut channel = BotChannel::new(kind, allowed);
                    if let Some(before) = before {
                        channel.reports = before.reports;
                        // With a sender taken off the list, where reports went is forgotten too.
                        if before.allowed_senders.iter().all(|sender| channel.allowed_senders.contains(sender)) {
                            channel.report_chat = before.report_chat;
                        }
                    }
                    bot.channels.push(channel);
                }
            },
            cx,
        );
    }

    pub(super) fn render_bot_channels(&mut self, bot: &Bot, cx: &mut Context<Self>) -> AnyElement {
        use super::settings_page::{row, section};
        if !self.bots.channels.looked_up {
            self.look_up_bot_secrets(cx);
        }
        let Some(inputs) = self.bot_form_inputs() else { return div().into_any_element() };
        let field = |input: &Entity<TextInput>| {
            div()
                .w(px(260.))
                .max_w_full()
                .px_2()
                .py_1()
                .rounded_md()
                .bg(hex(Chrome::PANEL))
                .border_1()
                .border_color(hex(Chrome::BORDER))
                .t_body()
                .text_color(hex(Chrome::BRIGHT))
                .child(input.clone())
        };
        let mut out = div().flex().flex_col().gap_4();
        for kind in KINDS {
            let Some(senders) = inputs.senders.get(&kind).cloned() else { continue };
            let on = bot.channels.iter().any(|c| c.kind == kind);
            let title = format!("{} · {}", kind.label(), t(cx, if on { "bots.channel_on" } else { "bots.channel_off" }));
            let handle = bot.handle.clone();
            let mut card = section(&title).child(row(
                t(cx, "bots.channel_senders"),
                div().flex().flex_wrap().items_center().justify_end().gap_2().child(field(&senders)).child(crate::ui::action_button(
                    gpui::SharedString::from(format!("bot-senders-{}", kind.id())),
                    t(cx, "save"),
                    cx.listener(move |this, _: &ClickEvent, _, cx| this.save_bot_senders(&handle, kind, &senders, cx)),
                )),
            ));
            for (secret, key) in secrets_of(kind) {
                let Some(input) = inputs.tokens.get(secret).cloned() else { continue };
                let saved = self.bots.channels.saved.get(secret).copied().unwrap_or(false);
                let label = if saved { format!("{} · {}", t(cx, key), t(cx, "bots.token_saved")) } else { t(cx, key).to_string() };
                let secret = *secret;
                card = card.child(row(
                    &label,
                    div().flex().flex_wrap().items_center().justify_end().gap_2().child(field(&input)).child(crate::ui::action_button(
                        gpui::SharedString::from(format!("bot-token-{key}")),
                        t(cx, "save"),
                        cx.listener(move |this, _: &ClickEvent, _, cx| this.save_bot_secret(kind, secret, input.clone(), cx)),
                    )),
                ));
            }
            if let Some(channel) = bot.channels.iter().find(|c| c.kind == kind) {
                let handle = bot.handle.clone();
                card = card.child(row(
                    t(cx, "bots.channel_reports"),
                    super::settings_page::switch(gpui::SharedString::from(format!("bot-reports-{}", kind.id())), channel.reports, {
                        cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.change_bot(
                                &handle,
                                |bot| {
                                    if let Some(channel) = bot.channels.iter_mut().find(|c| c.kind == kind) {
                                        channel.reports = !channel.reports;
                                    }
                                },
                                cx,
                            );
                        })
                    }),
                ));
                if channel.reports && channel.report_chat.is_empty() {
                    card = card.child(div().pt_2().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "bots.channel_reports_hint")));
                }
            }
            // Looked up and not there: said in the user's words, where the connection's own
            // error would only repeat it.
            let no_token = secrets_of(kind).iter().any(|(secret, _)| self.bots.channels.saved.get(secret) == Some(&false));
            if on && no_token {
                card = card.child(div().pt_2().t_small().text_color(hex(Chrome::WARNING)).child(t(cx, "bots.channel_needs_token")));
            } else if let Some(trouble) = self.bots.channels.trouble.get(&kind).filter(|_| on) {
                card = card.child(div().pt_2().t_small().text_color(hex(Chrome::ERROR)).child(trouble.clone()));
            }
            out = out.child(card);
        }
        out.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowed_senders_are_read_from_a_list_without_repeats() {
        assert_eq!(senders_from(" 1000, 2000\n1000  U1ABC "), ["1000", "2000", "U1ABC"]);
        assert!(senders_from(" , ").is_empty());
    }

    #[test]
    fn an_option_that_goes_on_holding_is_not_given_from_a_channel() {
        for option in ["Yes, and don't ask again for git status", "Yes, allow all edits during this session (shift+tab)", "Always allow"] {
            assert!(lasting(option), "{option}");
        }
        assert!(lasting("Yes, and don\u{2019}t ask again"));
        for option in ["Yes", "No, and tell Claude what to do differently (esc)", "Coffee", "Walk"] {
            assert!(!lasting(option), "{option}");
        }
    }

    #[test]
    fn a_thread_is_part_of_its_chat() {
        assert!(same_chat("D1/5.0", "D1/9.0") && same_chat("D1", "D1/9.0") && same_chat("1000", "1000"));
        assert!(!same_chat("D1/5.0", "D2/5.0") && !same_chat("1000", "10000"));
    }

    #[test]
    fn a_stopped_listener_does_not_sit_out_its_whole_pause() {
        let stop = AtomicBool::new(true);
        let started = std::time::Instant::now();
        wait(Duration::from_secs(300), &stop);
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
