//! Messages to chat services when an agent needs the user (and, if they want, when one finishes),
//! so the question reaches their phone.
//!
//! Slack and Discord each take either a **webhook URL** or a **bot token plus a channel**; Telegram
//! is always a bot token plus a chat. The two ways are kept apart end to end — their own secret in
//! the credential store, their own accepted shape, their own request — so switching one does not
//! quietly send with the other's credential.
//!
//! - Webhook URLs and bot tokens are secrets: they live in the credential store (service
//!   `run.agentty.notify`, scoped to the data folder like connectors) and never appear in settings,
//!   logs or error messages. Only the channel or chat to write to is kept in settings.
//! - A secret is accepted only in the shape of its service and transport, so a message can only
//!   ever go to Slack, Discord or Telegram.
//! - What is sent is decided by the caller; this module adds nothing (no paths, no prompts).
//!   Discord and Slack mentions are switched off, so a message can't ping `@everyone`.

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const KEYCHAIN_SERVICE: &str = "run.agentty.notify";
/// Longest message sent (chat services cut or refuse longer ones; a notification needs less).
const MAX_TEXT: usize = 1500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Channel {
    Slack,
    Discord,
    Telegram,
}

impl Channel {
    pub const ALL: [Channel; 3] = [Channel::Slack, Channel::Discord, Channel::Telegram];

    pub fn id(self) -> &'static str {
        match self {
            Channel::Slack => "slack",
            Channel::Discord => "discord",
            Channel::Telegram => "telegram",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Channel::Slack => "Slack",
            Channel::Discord => "Discord",
            Channel::Telegram => "Telegram",
        }
    }
}

/// How a message reaches a service. A channel's two transports have separate credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Transport {
    /// A URL that already says where to post; nothing else is needed.
    Webhook,
    /// A bot token, plus the channel or chat to write to.
    Bot,
}

impl Transport {
    /// The credential store account for this channel and transport. The webhook keeps the plain
    /// channel id, so credentials saved before bots existed are still found.
    fn account(self, channel: Channel) -> String {
        match self {
            Transport::Webhook => channel.id().to_string(),
            Transport::Bot => format!("{}.bot", channel.id()),
        }
    }
}

impl Channel {
    /// Whether this channel can be reached this way at all (Telegram is bot-only).
    pub fn supports(self, transport: Transport) -> bool {
        match self {
            Channel::Slack | Channel::Discord => true,
            Channel::Telegram => transport == Transport::Bot,
        }
    }

    /// Whether a target (channel name or chat id) is needed alongside the secret.
    pub fn needs_target(self, transport: Transport) -> bool {
        transport == Transport::Bot || self == Channel::Telegram
    }
}

fn keychain_service() -> String {
    crate::connectors::scoped_service(KEYCHAIN_SERVICE, std::env::var_os("AGENTTY_DATA_DIR").as_deref())
}

/// Checks that `secret` has the shape `channel` accepts for `transport`. A webhook URL is only
/// ever accepted as a webhook and a bot token only as a bot, so the two cannot be mixed up.
pub fn validate(channel: Channel, transport: Transport, secret: &str) -> Result<(), &'static str> {
    let secret = secret.trim();
    if secret.is_empty() || secret.len() > 400 || secret.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("not a valid value");
    }
    if !channel.supports(transport) {
        return Err("not a valid value");
    }
    if transport == Transport::Bot {
        return match channel {
            // `xoxb-` is a bot token, `xoxp-` a user token; both post with chat.postMessage.
            Channel::Slack => match secret.split_once('-') {
                Some((prefix, rest))
                    if matches!(prefix, "xoxb" | "xoxp")
                        && rest.len() >= 20
                        && rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') =>
                {
                    Ok(())
                }
                _ => Err("a Slack bot token starts with xoxb-"),
            },
            // Discord bot tokens are dot-separated base64url; no fixed length is documented.
            Channel::Discord => {
                let shape = secret.len() >= 50
                    && secret.matches('.').count() >= 2
                    && secret.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c));
                if shape {
                    Ok(())
                } else {
                    Err("a Discord bot token is the long value from the Bot page")
                }
            }
            Channel::Telegram => {
                let (id, token) = secret.split_once(':').ok_or("a Telegram bot token looks like 123456789:AA…")?;
                let id_ok = id.len() >= 5 && id.chars().all(|c| c.is_ascii_digit());
                let token_ok = token.len() >= 30 && token.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
                if id_ok && token_ok {
                    Ok(())
                } else {
                    Err("a Telegram bot token looks like 123456789:AA…")
                }
            }
        };
    }
    let path_ok = |rest: &str| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric() || "/_-".contains(c));
    match channel {
        Channel::Slack => match secret.strip_prefix("https://hooks.slack.com/") {
            Some(rest)
                if (rest.starts_with("services/") || rest.starts_with("triggers/") || rest.starts_with("workflows/")) && path_ok(rest) =>
            {
                Ok(())
            }
            _ => Err("a Slack webhook URL starts with https://hooks.slack.com/services/"),
        },
        Channel::Discord => {
            let rest = [
                "https://discord.com/api/webhooks/",
                "https://discordapp.com/api/webhooks/",
                "https://ptb.discord.com/api/webhooks/",
                "https://canary.discord.com/api/webhooks/",
            ]
            .iter()
            .find_map(|prefix| secret.strip_prefix(prefix));
            match rest {
                Some(rest) if path_ok(rest) && rest.contains('/') => Ok(()),
                _ => Err("a Discord webhook URL starts with https://discord.com/api/webhooks/"),
            }
        }
        // Unreachable: Telegram does not support webhooks here (`supports` refused above).
        Channel::Telegram => Err("a Telegram bot token looks like 123456789:AA…"),
    }
}

/// A Slack channel to post to: `#name`, `name`, or a channel/user id like `C01ABCDEF`.
pub fn valid_slack_channel(channel: &str) -> bool {
    let name = channel.trim().strip_prefix('#').unwrap_or(channel.trim());
    (1..=80).contains(&name.len()) && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// A Discord channel id. The API posts to an id, not a name — a name cannot be resolved without
/// listing the server, which needs more access than sending a message does.
pub fn valid_discord_channel(channel: &str) -> bool {
    let id = channel.trim();
    (15..=25).contains(&id.len()) && id.chars().all(|c| c.is_ascii_digit())
}

/// Whether `target` is what `channel` needs alongside a bot token.
pub fn valid_target(channel: Channel, target: &str) -> bool {
    match channel {
        Channel::Slack => valid_slack_channel(target),
        Channel::Discord => valid_discord_channel(target),
        Channel::Telegram => valid_chat_id(target),
    }
}

/// A Telegram chat id: a number (negative for groups) or `@channelname`.
pub fn valid_chat_id(chat: &str) -> bool {
    let chat = chat.trim();
    match chat.strip_prefix('@') {
        Some(name) => (5..=32).contains(&name.len()) && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
        None => {
            let digits = chat.strip_prefix('-').unwrap_or(chat);
            !digits.is_empty() && digits.len() <= 20 && digits.chars().all(|c| c.is_ascii_digit())
        }
    }
}

/// Credentials already read this run, keyed by the channel and the way it sends.
type SecretCache = Mutex<HashMap<(Channel, Transport), Option<String>>>;

/// Secrets read once per run (the credential store may ask the user; don't ask per message).
fn cache() -> &'static SecretCache {
    static CACHE: OnceLock<SecretCache> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn secret(channel: Channel, transport: Transport) -> Option<String> {
    let account = transport.account(channel);
    let mut cache = cache().lock().unwrap_or_else(|e| e.into_inner());
    cache
        .entry((channel, transport))
        .or_insert_with(|| crate::secret_store::load(&keychain_service(), &account).ok().filter(|s| !s.is_empty()))
        .clone()
}

/// Whether a credential is saved for this channel and transport.
pub fn configured(channel: Channel, transport: Transport) -> bool {
    channel.supports(transport) && secret(channel, transport).is_some()
}

/// Saves the webhook URL or bot token for this channel and transport (checked first).
pub fn save_secret(channel: Channel, transport: Transport, value: &str) -> Result<()> {
    let value = value.trim();
    validate(channel, transport, value).map_err(|e| anyhow!(e))?;
    crate::secret_store::store(&keychain_service(), &transport.account(channel), value)
        .map_err(|_| anyhow!("could not save it in the credential store"))?;
    cache().lock().unwrap_or_else(|e| e.into_inner()).insert((channel, transport), Some(value.to_string()));
    Ok(())
}

pub fn remove_secret(channel: Channel, transport: Transport) {
    let _ = crate::secret_store::delete(&keychain_service(), &transport.account(channel));
    cache().lock().unwrap_or_else(|e| e.into_inner()).insert((channel, transport), None);
}

fn clip(text: &str) -> String {
    let text: String = text.chars().filter(|c| !c.is_control() || *c == '\n').collect();
    if text.chars().count() <= MAX_TEXT {
        return text;
    }
    let mut cut: String = text.chars().take(MAX_TEXT - 1).collect();
    cut.push('…');
    cut
}

/// One outgoing message, fully decided, with no network touched yet.
struct Request {
    url: String,
    body: Value,
    /// `Authorization` header, for the transports that authenticate with one.
    auth: Option<String>,
    /// Slack's bot API answers 200 with `{"ok":false,…}`, so its body has to be read.
    check_ok_field: bool,
}

/// Where one message goes and what it carries (no network).
fn request(channel: Channel, transport: Transport, secret: &str, target: Option<&str>, text: &str) -> Result<Request> {
    validate(channel, transport, secret).map_err(|e| anyhow!(e))?;
    let text = clip(text);
    let plain = |url: String, body: Value| Request { url, body, auth: None, check_ok_field: false };
    if transport == Transport::Webhook {
        return Ok(match channel {
            Channel::Slack => plain(secret.to_string(), json!({ "text": text })),
            Channel::Discord => plain(secret.to_string(), json!({ "content": text, "allowed_mentions": { "parse": [] } })),
            Channel::Telegram => bail!("Telegram needs a bot token"),
        });
    }
    let target =
        target.map(str::trim).filter(|t| valid_target(channel, t)).ok_or_else(|| anyhow!("no {} channel to write to", channel.label()))?;
    Ok(match channel {
        Channel::Slack => Request {
            url: "https://slack.com/api/chat.postMessage".to_string(),
            // `link_names: false` keeps `@here` in the text from turning into a ping.
            body: json!({ "channel": target, "text": text, "link_names": false }),
            auth: Some(format!("Bearer {secret}")),
            check_ok_field: true,
        },
        Channel::Discord => Request {
            url: format!("https://discord.com/api/v10/channels/{target}/messages"),
            body: json!({ "content": text, "allowed_mentions": { "parse": [] } }),
            auth: Some(format!("Bot {secret}")),
            check_ok_field: false,
        },
        Channel::Telegram => plain(
            format!("https://api.telegram.org/bot{secret}/sendMessage"),
            json!({ "chat_id": target, "text": text, "disable_web_page_preview": true }),
        ),
    })
}

/// Sends `text` to `channel` the way `transport` says. Errors name the service and the HTTP status
/// only — never the URL or the header, which hold the secret.
pub fn send(channel: Channel, transport: Transport, target: Option<&str>, text: &str) -> Result<()> {
    let Some(secret) = secret(channel, transport) else { bail!("{} is not set up", channel.label()) };
    let Request { url, body, auth, check_ok_field } = request(channel, transport, &secret, target, text)?;
    let agent = crate::http::agent_builder().redirects(0).timeout(Duration::from_secs(15)).build();
    let mut post = agent.post(&url).set("User-Agent", "Agentty");
    if let Some(auth) = &auth {
        post = post.set("Authorization", auth);
    }
    match post.send_json(body) {
        // Redirects are off, so a 3xx arrives as success: Slack answers an unknown webhook with a
        // 302, and calling that "sent" would leave the user believing notifications work.
        Ok(response) if !(200..300).contains(&response.status()) => bail!("{} answered {}", channel.label(), response.status()),
        // Slack's bot API reports refusals in a 200 body, so a 200 alone is not "sent".
        Ok(response) if check_ok_field => {
            let value: Value = response.into_json().map_err(|_| anyhow!("{} sent something unexpected", channel.label()))?;
            if value["ok"].as_bool() == Some(true) {
                return Ok(());
            }
            let detail: String = value["error"].as_str().unwrap_or("refused").chars().take(120).collect();
            bail!("{} refused it: {detail}", channel.label())
        }
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(status, response)) => {
            // Telegram explains what's wrong ("chat not found"); its text never holds the token.
            let detail = (channel == Channel::Telegram)
                .then(|| response.into_json::<Value>().ok())
                .flatten()
                .and_then(|v| v["description"].as_str().map(|d| d.chars().take(120).collect::<String>()));
            match detail {
                Some(detail) => bail!("{} answered {status}: {detail}", channel.label()),
                None => bail!("{} answered {status}", channel.label()),
            }
        }
        Err(_) => bail!("could not reach {}", channel.label()),
    }
}

/// The chat that last wrote to the Telegram bot (the user sends it any message first): its id and
/// a name to show.
pub fn telegram_latest_chat() -> Result<(String, String)> {
    let Some(secret) = secret(Channel::Telegram, Transport::Bot) else { bail!("Telegram is not set up") };
    validate(Channel::Telegram, Transport::Bot, &secret).map_err(|e| anyhow!(e))?;
    let url = format!("https://api.telegram.org/bot{secret}/getUpdates");
    let agent = crate::http::agent_builder().redirects(0).timeout(Duration::from_secs(15)).build();
    let value: Value = match agent.get(&url).set("User-Agent", "Agentty").call() {
        Ok(response) => response.into_json().map_err(|_| anyhow!("Telegram sent something unexpected"))?,
        Err(ureq::Error::Status(status, _)) => bail!("Telegram answered {status}"),
        Err(_) => bail!("could not reach Telegram"),
    };
    latest_chat(&value).ok_or_else(|| anyhow!("no message to the bot yet: send it any message in Telegram, then try again"))
}

fn latest_chat(updates: &Value) -> Option<(String, String)> {
    updates["result"].as_array()?.iter().rev().find_map(|update| {
        let message = ["message", "channel_post", "my_chat_member"].iter().find_map(|k| update.get(*k))?;
        let chat = &message["chat"];
        let id = chat["id"].as_i64()?.to_string();
        let name =
            chat["title"].as_str().or(chat["username"].as_str()).or(chat["first_name"].as_str()).unwrap_or("").chars().take(60).collect();
        Some((id, name))
    })
}

/// Keeps a chatty agent from flooding a channel: one message per pane and kind per `gap`, and at
/// most `burst` messages per `window` overall.
pub struct Limiter {
    last: HashMap<(u64, u8), Instant>,
    sent: Vec<Instant>,
    gap: Duration,
    window: Duration,
    burst: usize,
}

impl Default for Limiter {
    fn default() -> Self {
        Limiter { last: HashMap::new(), sent: Vec::new(), gap: Duration::from_secs(30), window: Duration::from_secs(600), burst: 30 }
    }
}

impl Limiter {
    /// Whether a message for (`pane`, `kind`) may go out now; records it when it may.
    pub fn allow(&mut self, pane: u64, kind: u8, now: Instant) -> bool {
        if self.last.get(&(pane, kind)).is_some_and(|at| now.duration_since(*at) < self.gap) {
            return false;
        }
        self.sent.retain(|at| now.duration_since(*at) < self.window);
        if self.sent.len() >= self.burst {
            return false;
        }
        self.last.insert((pane, kind), now);
        self.sent.push(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slack_url() -> String {
        format!("https://hooks.slack.com/services/{}/{}/{}", "TEXAMPLE", "BEXAMPLE", "notarealsecretexample")
    }

    fn telegram_token() -> String {
        format!("{}:{}", "123456789", "AAexample_not_a_real_token_padding_xx")
    }

    fn slack_bot_token() -> String {
        format!("xoxb-{}-{}", "000000000000", "exampleNotARealSlackToken")
    }

    fn discord_bot_token() -> String {
        format!("{}.{}.{}", "MTIzNDU2Nzg5MDEyMzQ1Njc4", "Gexampl", "notarealdiscordbottokenexamplepadding")
    }

    #[test]
    fn secrets_only_in_their_services_shape() {
        use Transport::Webhook as W;
        assert!(validate(Channel::Slack, W, &slack_url()).is_ok());
        assert!(validate(Channel::Slack, W, "https://hooks.slack.com.evil.example/services/x").is_err());
        assert!(validate(Channel::Slack, W, "http://hooks.slack.com/services/x").is_err());
        assert!(validate(Channel::Slack, W, "https://hooks.slack.com/services/a?b=c").is_err());
        assert!(validate(Channel::Discord, W, "https://discord.com/api/webhooks/123/example-not-real").is_ok());
        assert!(validate(Channel::Discord, W, "https://discord.com/api/webhooks/123").is_err());
        assert!(validate(Channel::Discord, W, "https://example.com/api/webhooks/1/x").is_err());
        assert!(validate(Channel::Telegram, Transport::Bot, &telegram_token()).is_ok());
        assert!(validate(Channel::Telegram, Transport::Bot, "123:short").is_err());
        assert!(validate(Channel::Telegram, Transport::Bot, "123456789:AAexample not a token with spaces xxxxxx").is_err());
        assert!(valid_chat_id("-1001234567") && valid_chat_id("42") && valid_chat_id("@agentty_example"));
        assert!(!valid_chat_id("") && !valid_chat_id("12a") && !valid_chat_id("@x"));
    }

    /// The two ways of reaching a service never accept each other's credential, so a token pasted
    /// into the webhook field (or the reverse) is refused instead of silently never working.
    #[test]
    fn a_transport_refuses_the_other_ones_credential() {
        use Transport::{Bot, Webhook};
        assert!(validate(Channel::Slack, Bot, &slack_bot_token()).is_ok());
        assert!(validate(Channel::Slack, Bot, &slack_url()).is_err(), "a webhook URL is not a bot token");
        assert!(validate(Channel::Slack, Webhook, &slack_bot_token()).is_err(), "a bot token is not a webhook URL");
        assert!(validate(Channel::Slack, Bot, "xoxb-short").is_err());
        assert!(validate(Channel::Discord, Bot, &discord_bot_token()).is_ok());
        assert!(validate(Channel::Discord, Bot, "https://discord.com/api/webhooks/123/example-not-real").is_err());
        assert!(validate(Channel::Discord, Webhook, &discord_bot_token()).is_err());
        // Telegram has no webhook form at all.
        assert!(!Channel::Telegram.supports(Webhook));
        assert!(validate(Channel::Telegram, Webhook, &telegram_token()).is_err());
    }

    #[test]
    fn targets_per_service() {
        assert!(valid_target(Channel::Slack, "#general") && valid_target(Channel::Slack, "general"));
        assert!(valid_target(Channel::Slack, "C01ABCDEF"));
        assert!(!valid_target(Channel::Slack, "") && !valid_target(Channel::Slack, "has space"));
        // Discord posts to an id, not a name.
        assert!(valid_target(Channel::Discord, "123456789012345678")); // gitleaks:allow — a made-up id in a test
        assert!(!valid_target(Channel::Discord, "#general") && !valid_target(Channel::Discord, "1234"));
        assert!(valid_target(Channel::Telegram, "-1001234567"));
    }

    #[test]
    fn bot_requests_carry_their_own_auth() {
        let slack = request(Channel::Slack, Transport::Bot, &slack_bot_token(), Some("#general"), "needs you").unwrap();
        assert_eq!(slack.url, "https://slack.com/api/chat.postMessage");
        assert_eq!(slack.body["channel"], "#general");
        assert_eq!(slack.body["link_names"], json!(false), "no pings");
        assert_eq!(slack.auth.as_deref(), Some(format!("Bearer {}", slack_bot_token()).as_str()));
        assert!(slack.check_ok_field, "Slack reports refusals in a 200 body");

        let discord = request(Channel::Discord, Transport::Bot, &discord_bot_token(), Some("123456789012345678"), "@everyone hi").unwrap();
        assert_eq!(discord.url, "https://discord.com/api/v10/channels/123456789012345678/messages");
        assert_eq!(discord.body["allowed_mentions"]["parse"], json!([]), "no pings");
        assert_eq!(discord.auth.as_deref(), Some(format!("Bot {}", discord_bot_token()).as_str()));
        assert!(!discord.check_ok_field);

        // A bot with nowhere to write is refused rather than sent anywhere.
        assert!(request(Channel::Slack, Transport::Bot, &slack_bot_token(), None, "hi").is_err());
        assert!(request(Channel::Discord, Transport::Bot, &discord_bot_token(), Some("#general"), "hi").is_err(), "id, not a name");
    }

    /// A webhook is the URL itself and carries no header — the secret must never become one.
    #[test]
    fn webhook_requests_send_no_authorization_header() {
        let slack = request(Channel::Slack, Transport::Webhook, &slack_url(), None, "hi").unwrap();
        assert!(slack.auth.is_none() && slack.url == slack_url());
        let discord = "https://discord.com/api/webhooks/123/example-not-real";
        let out = request(Channel::Discord, Transport::Webhook, discord, None, "hi").unwrap();
        assert!(out.auth.is_none() && out.url == discord);
    }

    #[test]
    fn requests_per_service() {
        let slack = request(Channel::Slack, Transport::Webhook, &slack_url(), None, "Claude needs your answer").unwrap();
        assert_eq!(slack.url, slack_url());
        assert_eq!(slack.body, json!({ "text": "Claude needs your answer" }));
        let discord = "https://discord.com/api/webhooks/123/example-not-real";
        let out = request(Channel::Discord, Transport::Webhook, discord, None, "@everyone hi").unwrap();
        assert_eq!(out.body["allowed_mentions"]["parse"], json!([]), "no pings");
        let telegram = request(Channel::Telegram, Transport::Bot, &telegram_token(), Some("42"), "hi").unwrap();
        assert!(telegram.url.starts_with("https://api.telegram.org/bot123456789:") && telegram.url.ends_with("/sendMessage"));
        assert_eq!(telegram.body["chat_id"], "42");
        assert!(request(Channel::Telegram, Transport::Bot, &telegram_token(), None, "hi").is_err(), "needs a chat");
        let long = "x".repeat(5000);
        let cut = request(Channel::Slack, Transport::Webhook, &slack_url(), None, &long).unwrap();
        assert_eq!(cut.body["text"].as_str().unwrap().chars().count(), MAX_TEXT);
    }

    #[test]
    fn finds_the_latest_chat() {
        let updates = json!({ "ok": true, "result": [
            { "update_id": 1, "message": { "chat": { "id": 11, "first_name": "Old" } } },
            { "update_id": 2, "message": { "chat": { "id": -100200, "title": "Team" } } },
        ]});
        assert_eq!(latest_chat(&updates), Some(("-100200".into(), "Team".into())));
        assert_eq!(latest_chat(&json!({ "ok": true, "result": [] })), None);
    }

    #[test]
    fn limiter_spaces_messages() {
        let mut limiter = Limiter { burst: 3, ..Default::default() };
        let start = Instant::now();
        assert!(limiter.allow(1, 0, start));
        assert!(!limiter.allow(1, 0, start + Duration::from_secs(5)), "same pane and kind: wait");
        assert!(limiter.allow(1, 1, start + Duration::from_secs(5)), "another kind");
        assert!(limiter.allow(2, 0, start + Duration::from_secs(6)));
        assert!(!limiter.allow(3, 0, start + Duration::from_secs(7)), "burst used up");
        assert!(limiter.allow(1, 0, start + Duration::from_secs(700)), "window passed");
    }
}
