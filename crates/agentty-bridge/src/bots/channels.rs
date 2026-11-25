//! Outside channels a bot takes messages from. The computer opens the connection and waits
//! (Telegram long polling, Slack Socket Mode, the Discord gateway), so nothing listens on a port
//! and no relay is needed. A message only reaches a bot when its sender is on that bot's list;
//! everything else is dropped and noted in the bot's audit log.
//!
//! A sender on a bot's list can also ask Agentty itself a few things from there ([`ask`]): which
//! bots there are and what they do, to switch one on or off, to stop its turn, and to pick an
//! option of what its agent asks.
//!
//! Tokens live in the system keychain under their own service, never in a bot's files, and no
//! error or log line here repeats one.

use super::{Bot, BotStore, ChannelKind};
use anyhow::{anyhow, bail, Result};
use serde_json::Value;
use std::time::Duration;

const KEYCHAIN_SERVICE: &str = "run.agentty.bots";
/// Telegram holds a `getUpdates` call open this long when nothing arrives.
pub const TELEGRAM_POLL_SECONDS: u64 = 50;
/// Telegram refuses a longer message.
const TELEGRAM_TEXT_LIMIT: usize = 4_000;
/// A message older than this when it arrives is [`Incoming::stale`].
const STALE_AFTER_SECONDS: i64 = 120;
/// The most a channel's socket hands over in one message: far above any event read here.
const SOCKET_MESSAGE_LIMIT: usize = 8 * 1024 * 1024;

/// A socket that refuses a message larger than [`SOCKET_MESSAGE_LIMIT`].
fn socket_config() -> tungstenite::protocol::WebSocketConfig {
    tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(SOCKET_MESSAGE_LIMIT))
        .max_frame_size(Some(SOCKET_MESSAGE_LIMIT))
}
/// Parts of one answer sent to a channel; the rest is read in the app.
const ANSWER_PARTS: usize = 3;
/// The audit log is cut back to its newer half when it passes this.
const AUDIT_LIMIT: u64 = 256 * 1024;
const AUDIT_FILE: &str = "audit.log";

impl ChannelKind {
    pub fn id(self) -> &'static str {
        match self {
            ChannelKind::Telegram => "telegram",
            ChannelKind::Slack => "slack",
            ChannelKind::Discord => "discord",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ChannelKind::Telegram => "Telegram",
            ChannelKind::Slack => "Slack",
            ChannelKind::Discord => "Discord",
        }
    }
}

/// Which of a channel's secrets: Telegram has one token, Slack an app token (the socket) and a
/// bot token (answers), Discord one bot token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Secret {
    TelegramBot,
    SlackApp,
    SlackBot,
    DiscordBot,
}

/// Every secret a channel can have.
pub const SECRETS: [Secret; 4] = [Secret::TelegramBot, Secret::SlackApp, Secret::SlackBot, Secret::DiscordBot];

impl Secret {
    fn account(self) -> &'static str {
        match self {
            Secret::TelegramBot => "telegram.bot",
            Secret::SlackApp => "slack.app",
            Secret::SlackBot => "slack.bot",
            Secret::DiscordBot => "discord.bot",
        }
    }

    /// Whether `value` has the shape of this secret. Checked before it is stored or put in a URL.
    pub fn looks_right(self, value: &str) -> bool {
        let plain = |text: &str| !text.is_empty() && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        match self {
            Secret::TelegramBot => {
                value.split_once(':').is_some_and(|(id, key)| !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()) && plain(key))
            }
            Secret::SlackApp => value.strip_prefix("xapp-").is_some_and(plain),
            Secret::SlackBot => value.strip_prefix("xoxb-").is_some_and(plain),
            // Three runs joined by dots: the bot's id, a time and a signature.
            Secret::DiscordBot => value.len() >= 50 && value.split('.').count() == 3 && value.split('.').all(plain),
        }
    }
}

fn keychain_service() -> String {
    crate::connectors::scoped_service(KEYCHAIN_SERVICE, std::env::var_os("AGENTTY_DATA_DIR").as_deref())
}

pub fn save_secret(which: Secret, value: &str) -> Result<()> {
    let value = value.trim();
    if !which.looks_right(value) {
        bail!("that does not look like the right token");
    }
    crate::secret_store::store(&keychain_service(), which.account(), value)
}

pub fn load_secret(which: Secret) -> Option<String> {
    crate::secret_store::load(&keychain_service(), which.account()).ok().filter(|value| which.looks_right(value))
}

pub fn has_secret(which: Secret) -> bool {
    load_secret(which).is_some()
}

pub fn remove_secret(which: Secret) {
    let _ = crate::secret_store::delete(&keychain_service(), which.account());
}

/// A message that arrived on a channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Incoming {
    pub kind: ChannelKind,
    /// Where the answer goes: the chat it was written in (Slack: `channel` or `channel/thread`).
    pub chat: String,
    /// Who wrote it, as the channel names them: what a bot's allowed list holds.
    pub sender: String,
    pub text: String,
    /// Written to the bot alone, not in a group: only such a chat is kept for a bot's reports.
    pub direct: bool,
    /// Sent a while before it arrived (Telegram keeps what was written while nobody listened):
    /// what was about that moment, a switch or an option picked, is not acted on any more.
    pub stale: bool,
}

/// What became of an incoming message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Routed {
    /// For this bot, with the text left after its name.
    To(String, String),
    /// Nobody this sender may write to: dropped, with nothing sent back.
    Refused,
    /// An allowed sender who named no bot while several take their messages.
    Unclear(Vec<String>),
}

/// Picks the bot a message is for. The sender must be on that bot's list for this channel; a bot
/// is named by `/handle` or `@handle` at the start, and need not be when only one would listen.
pub fn route(incoming: &Incoming, bots: &[Bot]) -> Routed {
    let listening: Vec<&Bot> = bots
        .iter()
        .filter(|bot| bot.enabled)
        .filter(|bot| bot.channels.iter().any(|c| c.kind == incoming.kind && c.allowed_senders.iter().any(|s| s.trim() == incoming.sender)))
        .collect();
    if listening.is_empty() {
        return Routed::Refused;
    }
    let text = incoming.text.trim();
    let named = text.strip_prefix('/').or_else(|| text.strip_prefix('@')).and_then(|rest| {
        let end = rest.find(|c: char| c.is_whitespace()).unwrap_or(rest.len());
        // Telegram writes a command in a group as `/handle@its_bot_name`.
        let handle = rest[..end].split('@').next().unwrap_or_default().to_ascii_lowercase();
        listening.iter().find(|bot| bot.handle == handle).map(|bot| (bot.handle.clone(), rest[end..].trim().to_string()))
    });
    match named {
        Some((handle, body)) => Routed::To(handle, body),
        None if listening.len() == 1 => Routed::To(listening[0].handle.clone(), text.to_string()),
        None => Routed::Unclear(listening.iter().map(|bot| bot.handle.clone()).collect()),
    }
}

/// The bots `incoming`'s sender is on the list of for its channel, switched on or not.
pub fn reachable<'a>(incoming: &Incoming, bots: &'a [Bot]) -> Vec<&'a Bot> {
    bots.iter()
        .filter(|bot| bot.channels.iter().any(|c| c.kind == incoming.kind && c.allowed_senders.iter().any(|s| s.trim() == incoming.sender)))
        .collect()
}

/// What an allowed sender asks of Agentty itself, rather than of a bot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// The bots this sender can reach, and what each is doing.
    Bots,
    /// What one bot is doing and what it last said.
    Status(String),
    Switch {
        handle: String,
        on: bool,
    },
    /// The bot's running turn ended.
    Stop(String),
    Help,
    /// A command that names a bot this sender cannot reach.
    NoSuchBot(String),
}

/// Reads a message as a command to Agentty: `bots`, `status [handle]`, `on <handle>`,
/// `off <handle>`, `stop <handle>`, `help`, each with or without a `/` or `!` in front (Slack and
/// Discord keep `/` for their own commands). `handles`: the bots the sender can reach; with one
/// only, it need not be named. Only a message that is nothing but the command counts, so "status
/// of the build?" still goes to a bot; and a bot whose handle is a command's word is written to
/// with `/handle`, as ever.
pub fn ask(text: &str, handles: &[String]) -> Option<Ask> {
    let text = text.trim();
    let marked = text.starts_with('/') || text.starts_with('!');
    let mut words = text.trim_start_matches(['/', '!']).split_whitespace();
    // Telegram writes a command in a group as `/bots@its_bot_name`.
    let word = words.next()?.split('@').next().unwrap_or_default().to_ascii_lowercase();
    let named = words.next().map(|name| name.trim_start_matches(['@', '/']).to_ascii_lowercase());
    if words.next().is_some() || (marked && handles.contains(&word)) {
        return None;
    }
    let bot = |named: Option<String>| match named {
        Some(name) if handles.contains(&name) => Some(Ok(name)),
        // An unknown name after a marked command is said; after a bare word it is just a message.
        Some(name) => marked.then_some(Err(name)),
        None if handles.len() == 1 => Some(Ok(handles[0].clone())),
        None => None,
    };
    let of = |named: Option<String>, make: &dyn Fn(String) -> Ask| match bot(named) {
        Some(Ok(handle)) => Some(make(handle)),
        Some(Err(name)) => Some(Ask::NoSuchBot(name)),
        None => marked.then_some(Ask::Help),
    };
    match word.as_str() {
        "bots" | "list" if named.is_none() => Some(Ask::Bots),
        "help" if named.is_none() => Some(Ask::Help),
        // What Telegram sends when a chat with the bot is opened.
        "start" if marked && named.is_none() => Some(Ask::Help),
        "status" if named.is_none() => Some(Ask::Bots),
        "status" => of(named, &Ask::Status),
        "on" => of(named, &|handle| Ask::Switch { handle, on: true }),
        "off" => of(named, &|handle| Ask::Switch { handle, on: false }),
        "stop" => of(named, &Ask::Stop),
        _ => None,
    }
}

/// The bot a message names at its start (`/handle …`, `@handle …`), of the ones the sender can
/// reach: switched off, it is said so, and the message goes to no other bot.
pub fn named(text: &str, handles: &[String]) -> Option<String> {
    let rest = text.trim().strip_prefix(['/', '@'])?;
    let word = rest.split_whitespace().next()?.split('@').next().unwrap_or_default().to_ascii_lowercase();
    handles.contains(&word).then_some(word)
}

/// A message that is only the number of an option (`2`, `/2`): the option, counted from 0.
pub fn option_number(text: &str) -> Option<usize> {
    let text = text.trim().trim_start_matches(['/', '!', '#']).trim_end_matches('.');
    match text.parse::<usize>() {
        Ok(number) if (1..=9).contains(&number) && text.len() == 1 => Some(number - 1),
        _ => None,
    }
}

/// An answer cut into parts a channel takes, at line ends where it can. Past [`ANSWER_PARTS`] the
/// last part says the rest is in the app.
pub fn answer_parts(text: &str, limit: usize) -> Vec<String> {
    let mut parts = Vec::new();
    let mut rest = text.trim();
    while !rest.is_empty() {
        if parts.len() == ANSWER_PARTS {
            if let Some(last) = parts.last_mut() {
                *last = format!("{last}\n[…] The rest of this answer is in Agentty.");
            }
            break;
        }
        // Counted as the channels count: an emoji is two.
        let mut units = 0;
        let end = rest.char_indices().find(|(_, c)| {
            units += c.len_utf16();
            units > limit
        });
        let Some((end, _)) = end else {
            parts.push(rest.to_string());
            break;
        };
        let cut = rest[..end].rfind('\n').filter(|at| *at > end / 2).unwrap_or(end);
        parts.push(rest[..cut].trim_end().to_string());
        rest = rest[cut..].trim_start();
    }
    parts
}

/// How long to wait before connecting again after the `attempt`th failure in a row: 1 s doubling
/// up to 5 minutes.
pub fn backoff(attempt: u32) -> Duration {
    Duration::from_secs((1u64 << attempt.min(9)).min(300))
}

/// A word that is a credential by its looks: a known prefix, or a channel's own token.
fn known_secret(core: &str) -> bool {
    let prefixed =
        ["xoxb-", "xapp-", "xoxp-", "ghp_", "gho_", "sk-", "figd_", "glpat-"].iter().any(|p| core.starts_with(p)) && core.len() >= 12;
    prefixed || Secret::TelegramBot.looks_right(core) && core.len() >= 20 || Secret::DiscordBot.looks_right(core)
}

/// `text` with each word `secretive` says is one masked, its spacing and line ends kept.
fn masked(text: &str, secretive: impl Fn(&str) -> bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        if !word.is_empty() {
            let core = word.trim_matches(|c: char| !c.is_ascii_alphanumeric());
            out.push_str(if secretive(core) { "[redacted]" } else { word.as_str() });
            word.clear();
        }
    };
    for c in text.chars() {
        if c.is_whitespace() {
            flush(&mut word, &mut out);
            out.push(c);
        } else {
            word.push(c);
        }
    }
    flush(&mut word, &mut out);
    out
}

/// Token-shaped runs in `text`, masked: what goes into a log is never a credential.
pub fn scrub(text: &str) -> String {
    masked(text, |core| {
        let long = core.len() >= 24
            && core.chars().all(|c| c.is_ascii_alphanumeric() || "_-:.".contains(c))
            && core.chars().any(|c| c.is_ascii_digit());
        long || known_secret(core)
    })
}

/// What is sent out to a channel, with what is a credential by its looks masked. Narrower than
/// [`scrub`]: an answer may well hold a commit's hash or an id, which a log can do without.
pub fn scrub_sent(text: &str) -> String {
    masked(text, known_secret)
}

/// The newer half of a log, from the start of a line: cut as bytes, since its middle may fall
/// inside a character.
fn newer_half(log: &[u8]) -> &[u8] {
    let half = log.len() / 2;
    let from = log[half..].iter().position(|byte| *byte == b'\n').map_or(log.len(), |at| half + at + 1);
    &log[from..]
}

/// Adds a line to the bot's audit log: when, from where, and the start of what was said.
pub fn audit(store: &BotStore, handle: &str, now_ms: u64, source: &str, what: &str) {
    use std::io::Write;
    let Ok(dir) = store.dir(handle) else { return };
    if !dir.join("bot.json").is_file() {
        return;
    }
    let path = dir.join(AUDIT_FILE);
    if std::fs::metadata(&path).is_ok_and(|meta| meta.len() > AUDIT_LIMIT) {
        if let Ok(bytes) = std::fs::read(&path) {
            let _ = crate::fsutil::write_private(&path, newer_half(&bytes));
        }
    }
    let line = format!("{now_ms}\t{}\t{}\n", crate::fsutil::one_line(source, 80), scrub(&crate::fsutil::one_line(what, 300)));
    let file = if path.exists() { std::fs::OpenOptions::new().append(true).open(&path) } else { crate::fsutil::create_private(&path) };
    if let Ok(mut file) = file {
        let _ = file.write_all(line.as_bytes());
    }
}

pub mod telegram {
    use super::*;

    /// The messages of a `getUpdates` answer, and the offset to ask from next.
    pub fn parse_updates(value: &Value, now_seconds: i64) -> (Vec<Incoming>, Option<i64>) {
        let mut out = Vec::new();
        let mut next = None;
        for update in value["result"].as_array().into_iter().flatten() {
            if let Some(id) = update["update_id"].as_i64() {
                next = Some(next.map_or(id + 1, |n: i64| n.max(id + 1)));
            }
            let message = &update["message"];
            // A person's text only: nothing a bot wrote, no edits, no channel posts.
            if message["from"]["is_bot"].as_bool() == Some(true) {
                continue;
            }
            let (Some(chat), Some(sender), Some(text)) =
                (message["chat"]["id"].as_i64(), message["from"]["id"].as_i64(), message["text"].as_str())
            else {
                continue;
            };
            if !text.trim().is_empty() {
                out.push(Incoming {
                    kind: ChannelKind::Telegram,
                    chat: chat.to_string(),
                    sender: sender.to_string(),
                    text: text.to_string(),
                    direct: message["chat"]["type"].as_str() == Some("private"),
                    stale: message["date"].as_i64().is_some_and(|sent| now_seconds - sent > STALE_AFTER_SECONDS),
                });
            }
        }
        (out, next)
    }

    /// Waits up to [`TELEGRAM_POLL_SECONDS`] for messages to the bot.
    pub fn get_updates(token: &str, offset: Option<i64>) -> Result<(Vec<Incoming>, Option<i64>)> {
        if !Secret::TelegramBot.looks_right(token) {
            bail!("Telegram is not set up");
        }
        let url = format!("https://api.telegram.org/bot{token}/getUpdates");
        let agent = crate::http::agent_builder().redirects(0).timeout(Duration::from_secs(TELEGRAM_POLL_SECONDS + 15)).build();
        let mut request = agent
            .get(&url)
            .set("User-Agent", "Agentty")
            .query("timeout", &TELEGRAM_POLL_SECONDS.to_string())
            .query("allowed_updates", "[\"message\"]");
        if let Some(offset) = offset {
            request = request.query("offset", &offset.to_string());
        }
        let value: Value = match request.call() {
            Ok(response) => response.into_json().map_err(|_| anyhow!("Telegram sent something unexpected"))?,
            Err(ureq::Error::Status(status, _)) => bail!("Telegram answered {status}"),
            Err(_) => bail!("could not reach Telegram"),
        };
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |now| now.as_secs() as i64);
        Ok(parse_updates(&value, now))
    }

    pub fn send(token: &str, chat: &str, text: &str) -> Result<()> {
        if !Secret::TelegramBot.looks_right(token) {
            bail!("Telegram is not set up");
        }
        let url = format!("https://api.telegram.org/bot{token}/sendMessage");
        let agent = crate::http::agent_builder().redirects(0).timeout(Duration::from_secs(15)).build();
        for part in answer_parts(text, TELEGRAM_TEXT_LIMIT) {
            match agent.post(&url).set("User-Agent", "Agentty").send_json(serde_json::json!({ "chat_id": chat, "text": part })) {
                Ok(response) if (200..300).contains(&response.status()) => {}
                Ok(response) => bail!("Telegram answered {}", response.status()),
                Err(ureq::Error::Status(status, _)) => bail!("Telegram answered {status}"),
                Err(_) => bail!("could not reach Telegram"),
            }
        }
        Ok(())
    }
}

pub mod slack {
    use super::*;

    /// Slack takes far longer messages; this keeps an answer readable in a thread.
    const TEXT_LIMIT: usize = 3_500;

    /// What one Socket Mode frame asks of us.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Frame {
        /// An event to acknowledge with this envelope id, and the message in it when it is one.
        Event {
            envelope: String,
            message: Option<Incoming>,
        },
        /// Slack is closing this connection: connect again.
        Disconnect,
        Other,
    }

    /// Reads one frame. A message counts when a person mentions the app (`app_mention`) or writes
    /// to it directly (`message` in an `im` channel); the leading mention is taken off the text.
    pub fn parse_frame(text: &str) -> Frame {
        let Ok(value) = serde_json::from_str::<Value>(text) else { return Frame::Other };
        match value["type"].as_str() {
            Some("disconnect") => Frame::Disconnect,
            Some("events_api") => {
                let Some(envelope) = value["envelope_id"].as_str() else { return Frame::Other };
                let event = &value["payload"]["event"];
                let kind = event["type"].as_str().unwrap_or_default();
                let direct = kind == "message" && event["channel_type"].as_str() == Some("im") && event["subtype"].is_null();
                let person = event["bot_id"].is_null();
                let message = ((kind == "app_mention" || direct) && person)
                    .then(|| {
                        let (channel, sender, text) = (event["channel"].as_str()?, event["user"].as_str()?, event["text"].as_str()?);
                        // An answer goes into the thread the message is in, or starts one under it.
                        let thread = event["thread_ts"].as_str().or_else(|| event["ts"].as_str())?;
                        let text = strip_mention(text);
                        (!text.is_empty()).then(|| Incoming {
                            kind: ChannelKind::Slack,
                            chat: format!("{channel}/{thread}"),
                            sender: sender.to_string(),
                            text,
                            direct,
                            stale: false,
                        })
                    })
                    .flatten();
                Frame::Event { envelope: envelope.to_string(), message }
            }
            _ => Frame::Other,
        }
    }

    /// `<@U123> reviewer check the PR` → `reviewer check the PR`.
    fn strip_mention(text: &str) -> String {
        let text = text.trim();
        match text.strip_prefix("<@").and_then(|rest| rest.split_once('>')) {
            Some((_, rest)) => rest.trim().to_string(),
            None => text.to_string(),
        }
    }

    /// The reply that tells Slack an event arrived.
    pub fn ack(envelope: &str) -> String {
        serde_json::json!({ "envelope_id": envelope }).to_string()
    }

    /// Asks Slack for a Socket Mode address (`apps.connections.open`). The address is single-use
    /// and carries a ticket: it is connected to at once and never logged.
    pub fn open_connection(app_token: &str) -> Result<String> {
        if !Secret::SlackApp.looks_right(app_token) {
            bail!("Slack is not set up");
        }
        let agent = crate::http::agent_builder().redirects(0).timeout(Duration::from_secs(15)).build();
        let request = agent
            .post("https://slack.com/api/apps.connections.open")
            .set("User-Agent", "Agentty")
            .set("Authorization", &format!("Bearer {app_token}"));
        let value: Value = match request.call() {
            Ok(response) => response.into_json().map_err(|_| anyhow!("Slack sent something unexpected"))?,
            Err(ureq::Error::Status(status, _)) => bail!("Slack answered {status}"),
            Err(_) => bail!("could not reach Slack"),
        };
        connection_url(&value)
    }

    pub fn connection_url(value: &Value) -> Result<String> {
        if value["ok"].as_bool() != Some(true) {
            let detail: String = value["error"].as_str().unwrap_or("refused").chars().take(80).collect();
            bail!("Slack refused the connection: {detail}");
        }
        let url = value["url"].as_str().unwrap_or_default();
        // Only Slack's own socket hosts, over TLS.
        let host_ok = url::Url::parse(url)
            .ok()
            .is_some_and(|u| u.scheme() == "wss" && u.host_str().is_some_and(|h| h == "slack.com" || h.ends_with(".slack.com")));
        if !host_ok {
            bail!("Slack sent an address that is not its own");
        }
        Ok(url.to_string())
    }

    /// Holds one Socket Mode connection open and hands every message to `on_message`, until Slack
    /// closes it (`Ok`: connect again at once), it is lost (`Err`: wait first), or `stop` is set.
    pub fn listen(app_token: &str, stop: &std::sync::atomic::AtomicBool, mut on_message: impl FnMut(Incoming)) -> Result<()> {
        use std::sync::atomic::Ordering;
        use tungstenite::Message;
        let address = open_connection(app_token)?;
        let host = url::Url::parse(&address)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string))
            .ok_or_else(|| anyhow!("Slack sent no address"))?;
        let tcp = std::net::TcpStream::connect((host.as_str(), 443)).map_err(|_| anyhow!("could not reach Slack"))?;
        // A read gives up after a while so `stop` is looked at; Slack pings well inside that.
        tcp.set_read_timeout(Some(Duration::from_secs(20))).map_err(|_| anyhow!("could not reach Slack"))?;
        let connector = tungstenite::Connector::Rustls(crate::http::tls_config());
        let (mut socket, _) = tungstenite::client_tls_with_config(address.as_str(), tcp, Some(socket_config()), Some(connector))
            .map_err(|_| anyhow!("could not connect to Slack"))?;
        loop {
            if stop.load(Ordering::Relaxed) {
                let _ = socket.close(None);
                return Ok(());
            }
            match socket.read() {
                Ok(Message::Text(text)) => match parse_frame(text.as_str()) {
                    Frame::Event { envelope, message } => {
                        socket.send(Message::text(ack(&envelope))).map_err(|_| anyhow!("the connection to Slack was lost"))?;
                        if let Some(message) = message {
                            on_message(message);
                        }
                    }
                    Frame::Disconnect => return Ok(()),
                    Frame::Other => {}
                },
                Ok(Message::Close(_)) => return Ok(()),
                // A ping is answered by the library; the answer leaves with the next write or flush.
                Ok(_) => {
                    let _ = socket.flush();
                }
                Err(tungstenite::Error::Io(err)) if matches!(err.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                }
                Err(_) => bail!("the connection to Slack was lost"),
            }
        }
    }

    /// Posts an answer into the thread a message came from (`chat` is `channel/thread`).
    pub fn send(bot_token: &str, chat: &str, text: &str) -> Result<()> {
        if !Secret::SlackBot.looks_right(bot_token) {
            bail!("Slack is not set up");
        }
        let (channel, thread) = chat.split_once('/').unwrap_or((chat, ""));
        let agent = crate::http::agent_builder().redirects(0).timeout(Duration::from_secs(15)).build();
        for part in answer_parts(text, TEXT_LIMIT) {
            // Slack reads `<!channel>`, `<@U…>` and `<address|words>` as markup: an answer is
            // text, and pings nobody.
            let part = part.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
            let mut body = serde_json::json!({ "channel": channel, "text": part, "unfurl_links": false, "unfurl_media": false });
            if !thread.is_empty() {
                body["thread_ts"] = Value::String(thread.to_string());
            }
            let request = agent
                .post("https://slack.com/api/chat.postMessage")
                .set("User-Agent", "Agentty")
                .set("Authorization", &format!("Bearer {bot_token}"));
            let value: Value = match request.send_json(body) {
                Ok(response) => response.into_json().map_err(|_| anyhow!("Slack sent something unexpected"))?,
                Err(ureq::Error::Status(status, _)) => bail!("Slack answered {status}"),
                Err(_) => bail!("could not reach Slack"),
            };
            if value["ok"].as_bool() != Some(true) {
                let detail: String = value["error"].as_str().unwrap_or("refused").chars().take(80).collect();
                bail!("Slack refused it: {detail}");
            }
        }
        Ok(())
    }
}

pub mod discord {
    use super::*;
    use std::time::Instant;

    /// Discord refuses a longer message.
    const TEXT_LIMIT: usize = 1_900;
    const GATEWAY: &str = "wss://gateway.discord.gg/?v=10&encoding=json";
    const GATEWAY_HOST: &str = "gateway.discord.gg";
    /// Messages in servers and direct messages. Without the message-content intent Discord still
    /// gives the text of a direct message and of one that mentions the bot: all a bot reads.
    const INTENTS: u64 = (1 << 9) | (1 << 12);
    /// How Discord asks an app to name itself.
    const AGENT: &str = "DiscordBot (https://agentty.run, 1)";

    /// What one gateway frame asks of us.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Frame {
        /// The first frame: beat this often, and say who we are.
        Hello {
            beat_ms: u64,
        },
        /// Signed in, as the user with this id.
        Ready {
            me: String,
        },
        /// Something was written; the message when it is a person's, to the bot.
        Message(Option<Incoming>),
        /// Discord wants a heartbeat now.
        Beat,
        /// Our heartbeat was heard.
        Acked,
        /// This connection is over: connect again.
        Reconnect,
        Other,
    }

    /// Reads one frame, and the sequence number it carries (what a heartbeat repeats). A message
    /// counts when a person writes to the bot directly or mentions it (`me`, its id); the leading
    /// mention is taken off the text.
    pub fn parse_frame(text: &str, me: &str) -> (Frame, Option<i64>) {
        let Ok(value) = serde_json::from_str::<Value>(text) else { return (Frame::Other, None) };
        let seq = value["s"].as_i64();
        let data = &value["d"];
        let frame = match (value["op"].as_i64(), value["t"].as_str()) {
            (Some(10), _) => match data["heartbeat_interval"].as_u64() {
                Some(beat_ms) => Frame::Hello { beat_ms },
                None => Frame::Other,
            },
            (Some(0), Some("READY")) => match data["user"]["id"].as_str() {
                Some(id) => Frame::Ready { me: id.to_string() },
                None => Frame::Other,
            },
            (Some(0), Some("MESSAGE_CREATE")) => Frame::Message(message(data, me)),
            (Some(1), _) => Frame::Beat,
            (Some(11), _) => Frame::Acked,
            (Some(7 | 9), _) => Frame::Reconnect,
            _ => Frame::Other,
        };
        (frame, seq)
    }

    fn message(data: &Value, me: &str) -> Option<Incoming> {
        let author = &data["author"];
        if author["bot"].as_bool() == Some(true) || me.is_empty() {
            return None;
        }
        let (chat, sender, text) = (data["channel_id"].as_str()?, author["id"].as_str()?, data["content"].as_str()?);
        let direct = data["guild_id"].is_null();
        let mentioned = data["mentions"].as_array().is_some_and(|all| all.iter().any(|user| user["id"].as_str() == Some(me)));
        if !(direct || mentioned) || !digits(chat) || !digits(sender) {
            return None;
        }
        let text = strip_mention(text, me);
        (!text.is_empty()).then(|| Incoming {
            kind: ChannelKind::Discord,
            chat: chat.to_string(),
            sender: sender.to_string(),
            text,
            direct,
            stale: false,
        })
    }

    fn digits(id: &str) -> bool {
        !id.is_empty() && id.len() <= 24 && id.chars().all(|c| c.is_ascii_digit())
    }

    /// `<@123> reviewer check the PR` → `reviewer check the PR` (`<@!123>` is the older spelling).
    fn strip_mention(text: &str, me: &str) -> String {
        let text = text.trim();
        for mention in [format!("<@{me}>"), format!("<@!{me}>")] {
            if let Some(rest) = text.strip_prefix(&mention) {
                return rest.trim().to_string();
            }
        }
        text.to_string()
    }

    pub fn identify(token: &str) -> String {
        serde_json::json!({
            "op": 2,
            "d": {
                "token": token,
                "intents": INTENTS,
                "properties": { "os": std::env::consts::OS, "browser": "agentty", "device": "agentty" },
            },
        })
        .to_string()
    }

    pub fn heartbeat(seq: Option<i64>) -> String {
        serde_json::json!({ "op": 1, "d": seq }).to_string()
    }

    /// Why Discord closed the connection, when the code says the set-up is wrong and trying again
    /// as it is would not help.
    pub fn close_reason(code: u16) -> Option<&'static str> {
        match code {
            4004 => Some("Discord refused the bot token"),
            4013 | 4014 => Some("Discord refused what the bot asks to read"),
            _ => None,
        }
    }

    /// Holds one gateway connection open and hands every message to `on_message`, until Discord
    /// ends it (`Ok`: connect again at once), it is lost (`Err`: wait first), or `stop` is set.
    pub fn listen(token: &str, stop: &std::sync::atomic::AtomicBool, mut on_message: impl FnMut(Incoming)) -> Result<()> {
        use std::sync::atomic::Ordering;
        use tungstenite::Message;
        if !Secret::DiscordBot.looks_right(token) {
            bail!("Discord is not set up");
        }
        let lost = || anyhow!("the connection to Discord was lost");
        let tcp = std::net::TcpStream::connect((GATEWAY_HOST, 443)).map_err(|_| anyhow!("could not reach Discord"))?;
        // A read gives up after a while so `stop` and the heartbeat are looked at.
        tcp.set_read_timeout(Some(Duration::from_secs(5))).map_err(|_| anyhow!("could not reach Discord"))?;
        let connector = tungstenite::Connector::Rustls(crate::http::tls_config());
        let (mut socket, _) = tungstenite::client_tls_with_config(GATEWAY, tcp, Some(socket_config()), Some(connector))
            .map_err(|_| anyhow!("could not connect to Discord"))?;
        let (mut me, mut seq) = (String::new(), None::<i64>);
        // When the next heartbeat is due and how far apart they are; whether the last was heard.
        let (mut beat, mut heard) = (None::<(Instant, Duration)>, true);
        loop {
            if stop.load(Ordering::Relaxed) {
                let _ = socket.close(None);
                return Ok(());
            }
            if let Some((due, every)) = beat.filter(|(due, _)| Instant::now() >= *due) {
                // A heartbeat Discord never answered: the connection is dead.
                if !heard {
                    bail!("the connection to Discord was lost");
                }
                socket.send(Message::text(heartbeat(seq))).map_err(|_| lost())?;
                (beat, heard) = (Some((due + every, every)), false);
            }
            match socket.read() {
                Ok(Message::Text(text)) => {
                    let (frame, at) = parse_frame(text.as_str(), &me);
                    seq = at.or(seq);
                    match frame {
                        Frame::Hello { beat_ms } => {
                            let every = Duration::from_millis(beat_ms.clamp(1_000, 120_000));
                            // The first one early, as Discord asks, so many clients do not beat together.
                            (beat, heard) = (Some((Instant::now() + every / 2, every)), true);
                            socket.send(Message::text(identify(token))).map_err(|_| lost())?;
                        }
                        Frame::Ready { me: id } => me = id,
                        Frame::Message(Some(message)) => on_message(message),
                        Frame::Beat => socket.send(Message::text(heartbeat(seq))).map_err(|_| lost())?,
                        Frame::Acked => heard = true,
                        Frame::Reconnect => {
                            let _ = socket.close(None);
                            // Discord asks for a short wait before a new session.
                            bail!("Discord ended the session");
                        }
                        Frame::Message(None) | Frame::Other => {}
                    }
                }
                Ok(Message::Close(frame)) => {
                    if let Some(why) = frame.and_then(|frame| close_reason(frame.code.into())) {
                        bail!("{why}");
                    }
                    return Ok(());
                }
                Ok(_) => {
                    let _ = socket.flush();
                }
                Err(tungstenite::Error::Io(err)) if matches!(err.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                }
                Err(_) => bail!("the connection to Discord was lost"),
            }
        }
    }

    /// Posts an answer into the chat a message came from (`chat` is the channel's id).
    pub fn send(token: &str, chat: &str, text: &str) -> Result<()> {
        if !Secret::DiscordBot.looks_right(token) {
            bail!("Discord is not set up");
        }
        if !digits(chat) {
            bail!("that is not a Discord chat");
        }
        let url = format!("https://discord.com/api/v10/channels/{chat}/messages");
        let agent = crate::http::agent_builder().redirects(0).timeout(Duration::from_secs(15)).build();
        for part in answer_parts(text, TEXT_LIMIT) {
            // Nobody is pinged by what an answer happens to contain.
            let body = serde_json::json!({ "content": part, "allowed_mentions": { "parse": [] } });
            match agent.post(&url).set("User-Agent", AGENT).set("Authorization", &format!("Bot {token}")).send_json(body) {
                Ok(response) if (200..300).contains(&response.status()) => {}
                Ok(response) => bail!("Discord answered {}", response.status()),
                Err(ureq::Error::Status(status, _)) => bail!("Discord answered {status}"),
                Err(_) => bail!("could not reach Discord"),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::{BotAgent, BotChannel};
    use super::*;

    fn bot(handle: &str, senders: &[&str]) -> Bot {
        let mut bot = Bot::new(handle, handle, "", BotAgent::Claude, std::env::temp_dir());
        bot.channels = vec![BotChannel::new(ChannelKind::Telegram, senders.iter().map(|s| s.to_string()).collect())];
        bot
    }

    fn incoming(sender: &str, text: &str) -> Incoming {
        Incoming { kind: ChannelKind::Telegram, chat: "500".into(), sender: sender.into(), text: text.into(), direct: false, stale: false }
    }

    #[test]
    fn a_sender_who_is_not_on_a_bots_list_reaches_no_bot() {
        let bots = [bot("reviewer", &["1000"]), bot("builder", &["2000"])];
        assert_eq!(route(&incoming("9999", "/reviewer delete everything"), &bots), Routed::Refused);
        // On one bot's list is not on the other's, even when naming it.
        assert_eq!(route(&incoming("1000", "/builder push"), &bots), Routed::To("reviewer".into(), "/builder push".into()));
        assert_eq!(route(&incoming("2000", "hi"), &bots), Routed::To("builder".into(), "hi".into()));
        // A bot switched off listens to nobody.
        let mut off = bot("reviewer", &["1000"]);
        off.enabled = false;
        assert_eq!(route(&incoming("1000", "hi"), &[off]), Routed::Refused);
        // Another channel's list does not count.
        let slack = Incoming { kind: ChannelKind::Slack, ..incoming("1000", "hi") };
        assert_eq!(route(&slack, &bots), Routed::Refused);
    }

    #[test]
    fn a_bot_is_named_by_its_handle_when_several_listen() {
        let bots = [bot("reviewer", &["1000"]), bot("builder", &["1000"])];
        assert_eq!(route(&incoming("1000", "/builder  ship it"), &bots), Routed::To("builder".into(), "ship it".into()));
        assert_eq!(route(&incoming("1000", "@Reviewer look"), &bots), Routed::To("reviewer".into(), "look".into()));
        assert_eq!(route(&incoming("1000", "/reviewer@my_agentty_bot look"), &bots), Routed::To("reviewer".into(), "look".into()));
        assert_eq!(route(&incoming("1000", "status?"), &bots), Routed::Unclear(vec!["reviewer".into(), "builder".into()]));
    }

    #[test]
    fn telegram_updates_give_peoples_texts_and_the_next_offset() {
        let value = serde_json::json!({ "ok": true, "result": [
            { "update_id": 7, "message": { "from": { "id": 1000, "is_bot": false }, "chat": { "id": 1000, "type": "private" }, "text": "status?" } },
            { "update_id": 8, "message": { "from": { "id": 55, "is_bot": true }, "chat": { "id": 1000 }, "text": "from a bot" } },
            { "update_id": 9, "message": { "from": { "id": 1000, "is_bot": false }, "chat": { "id": -42 }, "photo": [] } },
            { "update_id": 10, "edited_message": { "from": { "id": 1000 }, "chat": { "id": 1000 }, "text": "edited" } }
        ]});
        let (messages, next) = telegram::parse_updates(&value, 0);
        assert_eq!(
            messages,
            vec![incoming("1000", "status?")].into_iter().map(|m| Incoming { chat: "1000".into(), direct: true, ..m }).collect::<Vec<_>>()
        );
        assert_eq!(next, Some(11));
        assert_eq!(telegram::parse_updates(&serde_json::json!({ "ok": true, "result": [] }), 0), (Vec::new(), None));
        // What was written while nobody listened arrives marked as old.
        let kept = serde_json::json!({ "ok": true, "result": [
            { "update_id": 1, "message": { "date": 1000, "from": { "id": 1000 }, "chat": { "id": 1000 }, "text": "1" } },
            { "update_id": 2, "message": { "date": 1990, "from": { "id": 1000 }, "chat": { "id": 1000 }, "text": "1" } }
        ]});
        let stale: Vec<bool> = telegram::parse_updates(&kept, 2000).0.iter().map(|message| message.stale).collect();
        assert_eq!(stale, [true, false]);
    }

    #[test]
    fn slack_frames_give_mentions_and_direct_messages() {
        let mention = serde_json::json!({ "type": "events_api", "envelope_id": "env-1", "payload": { "event": {
            "type": "app_mention", "user": "U1", "channel": "C1", "ts": "111.2", "text": "<@UBOT> reviewer check the PR" } } });
        assert_eq!(
            slack::parse_frame(&mention.to_string()),
            slack::Frame::Event {
                envelope: "env-1".into(),
                message: Some(Incoming {
                    kind: ChannelKind::Slack,
                    chat: "C1/111.2".into(),
                    sender: "U1".into(),
                    text: "reviewer check the PR".into(),
                    direct: false,
                    stale: false
                })
            }
        );
        let threaded = serde_json::json!({ "type": "events_api", "envelope_id": "env-2", "payload": { "event": {
            "type": "message", "channel_type": "im", "user": "U1", "channel": "D1", "ts": "5.0", "thread_ts": "3.0", "text": "more" } } });
        let slack::Frame::Event { message: Some(message), .. } = slack::parse_frame(&threaded.to_string()) else {
            panic!("a direct message")
        };
        assert_eq!(message.chat, "D1/3.0");
        assert!(message.direct);
        // The app's own posts and channel chatter are acknowledged and not passed on.
        let own = serde_json::json!({ "type": "events_api", "envelope_id": "env-3", "payload": { "event": {
            "type": "message", "channel_type": "im", "bot_id": "B1", "user": "U1", "channel": "D1", "ts": "6.0", "text": "answer" } } });
        assert_eq!(slack::parse_frame(&own.to_string()), slack::Frame::Event { envelope: "env-3".into(), message: None });
        assert_eq!(slack::parse_frame("{\"type\":\"disconnect\"}"), slack::Frame::Disconnect);
        assert_eq!(slack::parse_frame("{\"type\":\"hello\"}"), slack::Frame::Other);
        assert_eq!(slack::parse_frame("not json"), slack::Frame::Other);
        assert_eq!(slack::ack("env-1"), "{\"envelope_id\":\"env-1\"}");
    }

    #[test]
    fn a_socket_address_must_be_slacks_own() {
        let ok = serde_json::json!({ "ok": true, "url": "wss://wss-primary.slack.com/link/?ticket=example" });
        assert!(slack::connection_url(&ok).is_ok());
        for bad in
            ["ws://wss-primary.slack.com/link", "wss://slack.com.example.org/link", "wss://example.org/slack.com", "https://slack.com/x"]
        {
            assert!(slack::connection_url(&serde_json::json!({ "ok": true, "url": bad })).is_err(), "{bad}");
        }
        assert!(slack::connection_url(&serde_json::json!({ "ok": false, "error": "invalid_auth" })).is_err());
    }

    #[test]
    fn tokens_are_checked_by_shape() {
        let telegram = format!("{}:{}", "123456789", "A".repeat(35));
        assert!(Secret::TelegramBot.looks_right(&telegram));
        assert!(!Secret::TelegramBot.looks_right("not a token"));
        assert!(!Secret::TelegramBot.looks_right("123/../x:abc"));
        assert!(Secret::SlackApp.looks_right("xapp-example-not-a-real-token"));
        assert!(!Secret::SlackApp.looks_right("xoxb-example-not-a-real-token"));
        assert!(Secret::SlackBot.looks_right("xoxb-example-not-a-real-token"));
        let discord = format!("{}.{}.{}", "example_not_a_real_id_xx", "fake00", "example_not_a_real_signature_xxxxxxxx");
        assert!(Secret::DiscordBot.looks_right(&discord));
        assert!(!Secret::DiscordBot.looks_right("example.not.real"));
        assert!(!Secret::DiscordBot.looks_right(&discord.replace('.', "/")));
        assert_eq!(scrub(&format!("token {discord} here")), "token [redacted] here");
        // On a line of its own or after a tab it is still found, and the text keeps its shape.
        assert_eq!(scrub(&format!("first\n{discord}\tlast")), "first\n[redacted]\tlast");
        // What goes out to a channel loses credentials, not a commit's hash.
        let hash = "0ef1bc386a5d4c1e9f7b2a6d8c3e4f5a6b7c8d9e";
        assert_eq!(scrub_sent(&format!("commit {hash}\nxoxb-example-not-a-real-token")), format!("commit {hash}\n[redacted]"));
        assert_eq!(scrub_sent(&format!("({discord})")), "[redacted]");
    }

    #[test]
    fn the_audit_log_is_cut_at_a_line_whatever_its_middle_holds() {
        let log = "1\tone\n2\t가나다라마바사아자차카타파하\n3\tthree\n";
        for cut in 0..log.len() {
            let newer = newer_half(&log.as_bytes()[..cut]);
            assert!(newer.is_empty() || log.as_bytes()[..cut].ends_with(newer));
        }
        assert_eq!(newer_half(log.as_bytes()), b"3\tthree\n");
    }

    #[test]
    fn discord_frames_give_direct_messages_and_mentions() {
        use discord::Frame;
        let frame = |value: serde_json::Value| discord::parse_frame(&value.to_string(), "900");
        assert_eq!(frame(serde_json::json!({ "op": 10, "d": { "heartbeat_interval": 41250 } })), (Frame::Hello { beat_ms: 41250 }, None));
        assert_eq!(
            frame(serde_json::json!({ "op": 0, "t": "READY", "s": 1, "d": { "user": { "id": "900" } } })).0,
            Frame::Ready { me: "900".into() }
        );
        let direct = serde_json::json!({ "op": 0, "t": "MESSAGE_CREATE", "s": 5, "d": {
            "channel_id": "700", "author": { "id": "1000" }, "content": "check the PR", "mentions": [] } });
        let message = Incoming {
            kind: ChannelKind::Discord,
            chat: "700".into(),
            sender: "1000".into(),
            text: "check the PR".into(),
            direct: true,
            stale: false,
        };
        assert_eq!(frame(direct), (Frame::Message(Some(message.clone())), Some(5)));
        // In a server only a message that mentions the bot is read, without the mention.
        let mention = serde_json::json!({ "op": 0, "t": "MESSAGE_CREATE", "s": 6, "d": { "guild_id": "1", "channel_id": "700",
            "author": { "id": "1000" }, "content": "<@900> check the PR", "mentions": [{ "id": "900" }] } });
        assert_eq!(frame(mention).0, Frame::Message(Some(Incoming { direct: false, ..message })));
        let chatter = serde_json::json!({ "op": 0, "t": "MESSAGE_CREATE", "s": 7, "d": { "guild_id": "1", "channel_id": "700",
            "author": { "id": "1000" }, "content": "", "mentions": [] } });
        assert_eq!(frame(chatter).0, Frame::Message(None));
        let own = serde_json::json!({ "op": 0, "t": "MESSAGE_CREATE", "s": 8, "d": {
            "channel_id": "700", "author": { "id": "900", "bot": true }, "content": "answer", "mentions": [] } });
        assert_eq!(frame(own).0, Frame::Message(None));
        // An id that is not one never becomes part of an address.
        let odd = serde_json::json!({ "op": 0, "t": "MESSAGE_CREATE", "d": {
            "channel_id": "700/../1", "author": { "id": "1000" }, "content": "hi", "mentions": [] } });
        assert_eq!(frame(odd).0, Frame::Message(None));
        assert_eq!(frame(serde_json::json!({ "op": 11 })).0, Frame::Acked);
        assert_eq!(frame(serde_json::json!({ "op": 1 })).0, Frame::Beat);
        assert_eq!(frame(serde_json::json!({ "op": 7 })).0, Frame::Reconnect);
        assert_eq!(frame(serde_json::json!({ "op": 9, "d": false })).0, Frame::Reconnect);
        assert_eq!(discord::parse_frame("not json", "900"), (Frame::Other, None));
        assert_eq!(discord::heartbeat(Some(5)), "{\"op\":1,\"d\":5}");
        assert_eq!(discord::heartbeat(None), "{\"op\":1,\"d\":null}");
        assert!(discord::close_reason(4004).is_some() && discord::close_reason(1000).is_none());
        assert!(discord::send("not a token", "700", "hi").is_err());
    }

    #[test]
    fn an_allowed_sender_asks_agentty_about_the_bots() {
        let one = ["reviewer".to_string()];
        let two = ["reviewer".to_string(), "builder".to_string()];
        assert_eq!(ask("/bots", &two), Some(Ask::Bots));
        assert_eq!(ask("bots", &two), Some(Ask::Bots));
        assert_eq!(ask("/status@my_agentty_bot", &two), Some(Ask::Bots));
        assert_eq!(ask("status reviewer", &two), Some(Ask::Status("reviewer".into())));
        assert_eq!(ask("/off @Builder", &two), Some(Ask::Switch { handle: "builder".into(), on: false }));
        assert_eq!(ask("!on builder", &two), Some(Ask::Switch { handle: "builder".into(), on: true }));
        assert_eq!(ask("stop reviewer", &two), Some(Ask::Stop("reviewer".into())));
        assert_eq!(ask("/start", &two), Some(Ask::Help));
        // With one bot it need not be named; with several, a marked command says how.
        assert_eq!(ask("off", &one), Some(Ask::Switch { handle: "reviewer".into(), on: false }));
        assert_eq!(ask("/off", &two), Some(Ask::Help));
        assert_eq!(ask("off", &two), None);
        // A bot the sender cannot reach is not there for them.
        assert_eq!(ask("/on ghost", &two), Some(Ask::NoSuchBot("ghost".into())));
        assert_eq!(ask("on ghost", &two), None);
        // Anything longer is a message for a bot, and so is `/handle`.
        assert_eq!(ask("status of the build?", &one), None);
        assert_eq!(ask("start", &one), None);
        assert_eq!(ask("turn off the lights", &one), None);
        assert_eq!(ask("/status", &["status".to_string()]), None);
        assert_eq!(ask("  ", &one), None);
    }

    #[test]
    fn a_message_names_the_bot_it_is_for() {
        let two = ["reviewer".to_string(), "builder".to_string()];
        assert_eq!(named("/builder ship it", &two), Some("builder".into()));
        assert_eq!(named(" @Reviewer look", &two), Some("reviewer".into()));
        assert_eq!(named("/reviewer@my_agentty_bot look", &two), Some("reviewer".into()));
        assert_eq!(named("/ghost look", &two), None);
        assert_eq!(named("builder ship it", &two), None);
        assert_eq!(named("/", &two), None);
    }

    #[test]
    fn an_option_is_picked_by_its_number_alone() {
        assert_eq!(option_number("1"), Some(0));
        assert_eq!(option_number(" /3 "), Some(2));
        assert_eq!(option_number("2."), Some(1));
        for not in ["0", "10", "1 2", "yes", "", "2nd"] {
            assert_eq!(option_number(not), None, "{not}");
        }
    }

    #[test]
    fn a_sender_reaches_a_switched_off_bot_to_switch_it_on() {
        let mut off = bot("reviewer", &["1000"]);
        off.enabled = false;
        let bots = [off, bot("builder", &["2000"])];
        let handles: Vec<&str> = reachable(&incoming("1000", "on"), &bots).iter().map(|bot| bot.handle.as_str()).collect();
        assert_eq!(handles, ["reviewer"]);
        assert!(reachable(&incoming("9999", "on"), &bots).is_empty());
        assert!(reachable(&Incoming { kind: ChannelKind::Discord, ..incoming("1000", "on") }, &bots).is_empty());
    }

    #[test]
    fn long_answers_are_sent_in_a_few_parts() {
        assert_eq!(answer_parts("  hi  ", 10), ["hi"]);
        assert!(answer_parts("", 10).is_empty());
        let text = (0..40).map(|n| format!("line {n}")).collect::<Vec<_>>().join("\n");
        let parts = answer_parts(&text, 60);
        assert_eq!(parts.len(), ANSWER_PARTS);
        assert!(parts[0].starts_with("line 0") && parts[0].chars().count() <= 60);
        assert!(parts.last().unwrap().ends_with("The rest of this answer is in Agentty."));
        let korean = "가".repeat(25);
        assert_eq!(answer_parts(&korean, 10).iter().map(|p| p.chars().count()).take(2).collect::<Vec<_>>(), [10, 10]);
        // Counted as the channels count them: an emoji is two.
        let faces = "\u{1F600}".repeat(25);
        assert_eq!(answer_parts(&faces, 10).iter().map(|p| p.chars().count()).take(2).collect::<Vec<_>>(), [5, 5]);
    }

    #[test]
    fn the_wait_before_reconnecting_grows_to_five_minutes() {
        assert_eq!(backoff(0), Duration::from_secs(1));
        assert_eq!(backoff(3), Duration::from_secs(8));
        assert_eq!(backoff(9), Duration::from_secs(300));
        assert_eq!(backoff(40), Duration::from_secs(300));
    }

    #[test]
    fn the_audit_log_keeps_no_tokens_and_does_not_grow_forever() {
        let root = std::env::temp_dir().join(format!("agentty-bots-audit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let store = BotStore::at(root.join("bots"));
        store.create(&bot("reviewer", &["1000"])).unwrap();
        let fake = format!("{}:{}", "123456789", "example_not_a_real_token_".repeat(2));
        audit(&store, "reviewer", 1, "telegram:1000", &format!("my token is {fake} and xoxb-example-not-a-real-token\nsecond line"));
        let path = store.dir("reviewer").unwrap().join("audit.log");
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text, "1\ttelegram:1000\tmy token is [redacted] and [redacted] second line\n");
        for n in 0..3000u64 {
            audit(&store, "reviewer", n, "telegram:1000", &"x".repeat(200));
        }
        assert!(std::fs::metadata(&path).unwrap().len() <= AUDIT_LIMIT + 1024);
        // Nothing is written for a bot that is not there, or outside the store.
        audit(&store, "ghost", 1, "telegram:1", "x");
        audit(&store, "../x", 1, "telegram:1", "x");
        assert!(!store.root().join("ghost").exists());
        let _ = std::fs::remove_dir_all(root);
    }
}
