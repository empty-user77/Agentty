//! Messages to chat services — Slack and Discord webhooks, a Telegram bot — when an agent needs the
//! user (and, if they want, when one finishes), so the question reaches their phone.
//!
//! - Webhook URLs and the bot token are secrets: they live in the credential store (service
//!   `run.agentty.notify`, scoped to the data folder like connectors) and never appear in settings,
//!   logs or error messages. Only the Telegram chat id is kept in settings.
//! - A secret is accepted only in its service's own shape (`https://hooks.slack.com/…`,
//!   `https://discord.com/api/webhooks/…`, `<digits>:<token>`), so a message can only ever go to
//!   Slack, Discord or Telegram.
//! - What is sent is decided by the caller; this module adds nothing (no paths, no prompts).
//!   Discord mentions are switched off, so a message can't ping `@everyone`.

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

fn keychain_service() -> String {
    crate::connectors::scoped_service(KEYCHAIN_SERVICE, std::env::var_os("AGENTTY_DATA_DIR").as_deref())
}

/// Checks that `secret` has the shape of `channel`'s webhook URL or bot token.
pub fn validate(channel: Channel, secret: &str) -> Result<(), &'static str> {
    let secret = secret.trim();
    if secret.is_empty() || secret.len() > 400 || secret.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("not a valid value");
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

/// Secrets read once per run (the credential store may ask the user; don't ask per message).
fn cache() -> &'static Mutex<HashMap<Channel, Option<String>>> {
    static CACHE: OnceLock<Mutex<HashMap<Channel, Option<String>>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn secret(channel: Channel) -> Option<String> {
    let mut cache = cache().lock().unwrap_or_else(|e| e.into_inner());
    cache
        .entry(channel)
        .or_insert_with(|| crate::secret_store::load(&keychain_service(), channel.id()).ok().filter(|s| !s.is_empty()))
        .clone()
}

/// Whether a webhook URL / bot token is saved for `channel`.
pub fn configured(channel: Channel) -> bool {
    secret(channel).is_some()
}

/// Saves `channel`'s webhook URL or bot token (checked first).
pub fn save_secret(channel: Channel, value: &str) -> Result<()> {
    let value = value.trim();
    validate(channel, value).map_err(|e| anyhow!(e))?;
    crate::secret_store::store(&keychain_service(), channel.id(), value)
        .map_err(|_| anyhow!("could not save it in the credential store"))?;
    cache().lock().unwrap_or_else(|e| e.into_inner()).insert(channel, Some(value.to_string()));
    Ok(())
}

pub fn remove_secret(channel: Channel) {
    let _ = crate::secret_store::delete(&keychain_service(), channel.id());
    cache().lock().unwrap_or_else(|e| e.into_inner()).insert(channel, None);
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

/// The URL and JSON body for one message (no network).
fn request(channel: Channel, secret: &str, chat: Option<&str>, text: &str) -> Result<(String, Value)> {
    validate(channel, secret).map_err(|e| anyhow!(e))?;
    let text = clip(text);
    Ok(match channel {
        Channel::Slack => (secret.to_string(), json!({ "text": text })),
        Channel::Discord => (secret.to_string(), json!({ "content": text, "allowed_mentions": { "parse": [] } })),
        Channel::Telegram => {
            let chat = chat.map(str::trim).filter(|c| valid_chat_id(c)).ok_or_else(|| anyhow!("no Telegram chat id"))?;
            (
                format!("https://api.telegram.org/bot{secret}/sendMessage"),
                json!({ "chat_id": chat, "text": text, "disable_web_page_preview": true }),
            )
        }
    })
}

/// Sends `text` to `channel`. Errors name the service and the HTTP status only — never the URL,
/// which holds the secret.
pub fn send(channel: Channel, chat: Option<&str>, text: &str) -> Result<()> {
    let Some(secret) = secret(channel) else { bail!("{} is not set up", channel.label()) };
    let (url, body) = request(channel, &secret, chat, text)?;
    let agent = crate::http::agent_builder().redirects(0).timeout(Duration::from_secs(15)).build();
    match agent.post(&url).set("User-Agent", "Agentty").send_json(body) {
        // Redirects are off, so a 3xx arrives as success: Slack answers an unknown webhook with a
        // 302, and calling that "sent" would leave the user believing notifications work.
        Ok(response) if !(200..300).contains(&response.status()) => bail!("{} answered {}", channel.label(), response.status()),
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
    let Some(secret) = secret(Channel::Telegram) else { bail!("Telegram is not set up") };
    validate(Channel::Telegram, &secret).map_err(|e| anyhow!(e))?;
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

    #[test]
    fn secrets_only_in_their_services_shape() {
        assert!(validate(Channel::Slack, &slack_url()).is_ok());
        assert!(validate(Channel::Slack, "https://hooks.slack.com.evil.example/services/x").is_err());
        assert!(validate(Channel::Slack, "http://hooks.slack.com/services/x").is_err());
        assert!(validate(Channel::Slack, "https://hooks.slack.com/services/a?b=c").is_err());
        assert!(validate(Channel::Discord, "https://discord.com/api/webhooks/123/example-not-real").is_ok());
        assert!(validate(Channel::Discord, "https://discord.com/api/webhooks/123").is_err());
        assert!(validate(Channel::Discord, "https://example.com/api/webhooks/1/x").is_err());
        assert!(validate(Channel::Telegram, &telegram_token()).is_ok());
        assert!(validate(Channel::Telegram, "123:short").is_err());
        assert!(validate(Channel::Telegram, "123456789:AAexample not a token with spaces xxxxxx").is_err());
        assert!(valid_chat_id("-1001234567") && valid_chat_id("42") && valid_chat_id("@agentty_example"));
        assert!(!valid_chat_id("") && !valid_chat_id("12a") && !valid_chat_id("@x"));
    }

    #[test]
    fn requests_per_service() {
        let (url, body) = request(Channel::Slack, &slack_url(), None, "Claude needs your answer").unwrap();
        assert_eq!(url, slack_url());
        assert_eq!(body, json!({ "text": "Claude needs your answer" }));
        let discord = "https://discord.com/api/webhooks/123/example-not-real";
        let (_, body) = request(Channel::Discord, discord, None, "@everyone hi").unwrap();
        assert_eq!(body["allowed_mentions"]["parse"], json!([]), "no pings");
        let (url, body) = request(Channel::Telegram, &telegram_token(), Some("42"), "hi").unwrap();
        assert!(url.starts_with("https://api.telegram.org/bot123456789:") && url.ends_with("/sendMessage"));
        assert_eq!(body["chat_id"], "42");
        assert!(request(Channel::Telegram, &telegram_token(), None, "hi").is_err(), "needs a chat");
        let long = "x".repeat(5000);
        let (_, body) = request(Channel::Slack, &slack_url(), None, &long).unwrap();
        assert_eq!(body["text"].as_str().unwrap().chars().count(), MAX_TEXT);
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
