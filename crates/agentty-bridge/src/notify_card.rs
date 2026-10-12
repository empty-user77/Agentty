//! How a notice looks in each chat service: a card with a colored edge, the workspace in bold, the
//! session, the agent's own words with their formatting carried over, and where it ran.
//!
//! The agent writes Markdown. Slack has its own markup ("mrkdwn"), Telegram takes a small set of
//! HTML tags and refuses the whole message over one unclosed tag, Discord reads Markdown as is. The
//! converters here only ever emit balanced tags and escape everything else, so a stray `**` or `<`
//! in an agent's reply can't break the message.

use serde_json::{json, Value};

/// Slack's limit for one text block is 3000 characters; stay below it.
const SLACK_SECTION: usize = 2900;

/// Whether the notice reports a result or waits for the user: it picks the edge color.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Tone {
    #[default]
    Done,
    Ask,
}

impl Tone {
    fn rgb(self) -> u32 {
        match self {
            Tone::Done => 0x2eb67d,
            Tone::Ask => 0xe8912d,
        }
    }
}

/// What a notice says, before it is laid out for a service. Empty fields are left out.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Card {
    pub tone: Tone,
    /// "✅ Finished", with its emoji.
    pub headline: String,
    pub workspace: String,
    pub agent: String,
    /// The session's title.
    pub session: String,
    /// What the agent said or asks, in Markdown.
    pub body: String,
    pub branch: String,
    /// The folder, home shortened to `~`.
    pub folder: String,
    /// Where to see it now (the remote page, opened at this terminal), and what to call that.
    pub link: String,
    pub link_label: String,
}

impl Card {
    /// One line for the phone's notification preview: what happened, and where.
    pub fn preview(&self) -> String {
        let place = [self.workspace.as_str(), self.agent.as_str()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
        if place.is_empty() {
            self.headline.clone()
        } else {
            format!("{} — {place}", self.headline)
        }
    }

    /// The whole notice as plain text, for clients that show no formatting.
    pub fn plain(&self) -> String {
        let mut out = self.preview();
        for extra in [&self.session, &self.body] {
            if !extra.is_empty() {
                out.push('\n');
                out.push_str(extra);
            }
        }
        if let Some(link) = self.link() {
            out.push_str(&format!("\n{}: {link}", self.link_label));
        }
        out
    }

    /// The link, when it is one: an `https://` address with nothing in it that could break out of
    /// the markup it goes into.
    fn link(&self) -> Option<&str> {
        let ok = self.link.starts_with("https://")
            && self.link.len() <= 300
            && !self.link.chars().any(|c| c.is_whitespace() || c.is_control() || "<>|\"'`".contains(c));
        ok.then_some(self.link.as_str())
    }

    fn footer(&self) -> Vec<String> {
        let mut parts = Vec::new();
        if !self.branch.is_empty() {
            parts.push(self.branch.clone());
        }
        if !self.folder.is_empty() {
            parts.push(self.folder.clone());
        }
        parts
    }

    /// Slack (webhook or bot): the headline as the message, the rest as an attachment whose edge
    /// takes the tone's color, built from blocks.
    pub fn slack(&self, body: &mut Value) {
        let mut blocks = Vec::new();
        let mut who = Vec::new();
        if !self.workspace.is_empty() {
            who.push(format!("*{}*", slack_escape(&self.workspace)));
        }
        if !self.agent.is_empty() {
            who.push(slack_escape(&self.agent));
        }
        let mut lead = who.join("  ·  ");
        if !self.session.is_empty() {
            if !lead.is_empty() {
                lead.push('\n');
            }
            lead.push_str(&format!("_{}_", slack_escape(&self.session)));
        }
        if !lead.is_empty() {
            blocks.push(json!({ "type": "section", "text": { "type": "mrkdwn", "text": lead } }));
        }
        if !self.body.is_empty() {
            blocks.push(json!({ "type": "section", "text": { "type": "mrkdwn", "text": clip_chars(&markdown_to_slack(&self.body), SLACK_SECTION) } }));
        }
        let footer = self.footer();
        if !footer.is_empty() {
            let elements: Vec<Value> =
                footer.iter().map(|part| json!({ "type": "mrkdwn", "text": format!("`{}`", part.replace('`', "'")) })).collect();
            blocks.push(json!({ "type": "context", "elements": elements }));
        }
        if let Some(link) = self.link() {
            let text = format!("*{}:* <{}>", slack_escape(&self.link_label), link.replace('&', "&amp;"));
            blocks.push(json!({ "type": "section", "text": { "type": "mrkdwn", "text": text } }));
        }
        body["text"] = json!(slack_escape(&self.preview()));
        body["unfurl_links"] = json!(false);
        if !blocks.is_empty() {
            body["attachments"] = json!([{ "color": format!("#{:06x}", self.tone.rgb()), "fallback": self.plain(), "blocks": blocks }]);
        }
    }

    /// Discord: an embed with the tone's color; Discord reads the agent's Markdown itself.
    pub fn discord(&self, body: &mut Value) {
        let mut lines = Vec::new();
        let who = [
            (!self.workspace.is_empty()).then(|| format!("**{}**", discord_escape(&self.workspace))),
            (!self.agent.is_empty()).then(|| discord_escape(&self.agent)),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
        if !who.is_empty() {
            lines.push(who);
        }
        if !self.session.is_empty() {
            lines.push(format!("*{}*", discord_escape(&self.session)));
        }
        if !self.body.is_empty() {
            lines.push(String::new());
            lines.push(self.body.clone());
        }
        if let Some(link) = self.link() {
            lines.push(String::new());
            lines.push(format!("**{}:** {link}", discord_escape(&self.link_label)));
        }
        let mut embed = json!({ "title": self.headline, "description": lines.join("\n"), "color": self.tone.rgb() });
        let footer = self.footer();
        if !footer.is_empty() {
            embed["footer"] = json!({ "text": footer.join(" · ") });
        }
        body["content"] = json!("");
        body["embeds"] = json!([embed]);
    }

    /// Telegram: HTML, its tags always balanced.
    pub fn telegram(&self, body: &mut Value) {
        let mut out = format!("<b>{}</b>", html_escape(&self.headline));
        let who = [
            (!self.workspace.is_empty()).then(|| format!("<b>{}</b>", html_escape(&self.workspace))),
            (!self.agent.is_empty()).then(|| html_escape(&self.agent)),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
        if !who.is_empty() {
            out.push('\n');
            out.push_str(&who);
        }
        if !self.session.is_empty() {
            out.push_str(&format!("\n<i>{}</i>", html_escape(&self.session)));
        }
        if !self.body.is_empty() {
            out.push_str("\n\n");
            out.push_str(&telegram_quoted(&markdown_to_telegram(&self.body)));
        }
        let footer = self.footer();
        if !footer.is_empty() {
            out.push_str("\n\n");
            out.push_str(&footer.iter().map(|part| format!("<code>{}</code>", html_escape(part))).collect::<Vec<_>>().join(" · "));
        }
        if let Some(link) = self.link() {
            let link = html_escape(link);
            out.push_str(&format!("\n\n<b>{}:</b> <a href=\"{link}\">{link}</a>", html_escape(&self.link_label)));
        }
        body["text"] = json!(out);
        body["parse_mode"] = json!("HTML");
    }
}

fn clip_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(max - 1).collect();
    cut.push('…');
    cut
}

fn slack_escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn html_escape(text: &str) -> String {
    slack_escape(text).replace('"', "&quot;")
}

fn discord_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if "*_~`|\\".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// A Markdown line's block shape: a heading, a list item, or plain text.
fn block_line(line: &str) -> (Option<&'static str>, &str) {
    let trimmed = line.trim_start();
    if let Some(rest) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
        return (Some("bullet"), rest);
    }
    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ') {
        return (Some("heading"), trimmed[hashes..].trim());
    }
    (None, line)
}

/// Agent Markdown to Slack mrkdwn: `**bold**` → `*bold*`, `[text](url)` → `<url|text>`, headings
/// bold, `-` lists as bullets, code kept, `&`, `<`, `>` escaped.
pub fn markdown_to_slack(markdown: &str) -> String {
    let mut out = Vec::new();
    let mut in_fence = false;
    for line in markdown.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            out.push("```".to_string());
            continue;
        }
        if in_fence {
            out.push(slack_escape(line));
            continue;
        }
        let (shape, text) = block_line(line);
        let inline = slack_inline(text);
        out.push(match shape {
            Some("bullet") => format!("• {inline}"),
            Some("heading") => format!("*{}*", inline.trim_matches('*')),
            _ => inline,
        });
    }
    if in_fence {
        out.push("```".to_string());
    }
    out.join("\n")
}

fn slack_inline(text: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    let mut in_code = false;
    while i < chars.len() {
        let c = chars[i];
        if c == '`' {
            in_code = !in_code;
            out.push(c);
            i += 1;
            continue;
        }
        if !in_code {
            if c == '*' && chars.get(i + 1) == Some(&'*') {
                out.push('*');
                i += 2;
                continue;
            }
            if c == '[' {
                if let Some((label, url, used)) = markdown_link(&chars[i..]) {
                    out.push_str(&format!("<{}|{}>", slack_escape(&url), slack_escape(&label).replace('|', "¦")));
                    i += used;
                    continue;
                }
            }
        }
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
        i += 1;
    }
    out
}

/// `[label](https://…)` at the start of `chars`: the label, the URL and how many chars it took.
fn markdown_link(chars: &[char]) -> Option<(String, String, usize)> {
    let close = chars.iter().position(|c| *c == ']')?;
    if chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let end = close + 2 + chars[close + 2..].iter().position(|c| *c == ')')?;
    let url: String = chars[close + 2..end].iter().collect();
    if !(url.starts_with("https://") || url.starts_with("http://")) || url.contains(char::is_whitespace) {
        return None;
    }
    Some((chars[1..close].iter().collect(), url, end + 1))
}

/// Agent Markdown to Telegram HTML: bold, inline code, code blocks, links, headings and bullets.
/// Every tag opened on a line is closed on it, and everything else is escaped.
pub fn markdown_to_telegram(markdown: &str) -> String {
    let mut out = Vec::new();
    let mut fence: Option<Vec<String>> = None;
    for line in markdown.lines() {
        if line.trim_start().starts_with("```") {
            match fence.take() {
                Some(lines) => out.push(format!("<pre>{}</pre>", lines.join("\n"))),
                None => fence = Some(Vec::new()),
            }
            continue;
        }
        if let Some(lines) = fence.as_mut() {
            lines.push(html_escape(line));
            continue;
        }
        let (shape, text) = block_line(line);
        let inline = telegram_inline(text);
        out.push(match shape {
            Some("bullet") => format!("• {inline}"),
            Some("heading") => format!("<b>{inline}</b>"),
            _ => inline,
        });
    }
    if let Some(lines) = fence {
        out.push(format!("<pre>{}</pre>", lines.join("\n")));
    }
    out.join("\n")
}

/// The agent's words in a collapsible quote box: a long reply folds to a few lines and opens with
/// a tap. Code blocks stay between the boxes, since Telegram does not take `<pre>` inside a quote.
fn telegram_quoted(html: &str) -> String {
    let mut out = String::new();
    let quote = |out: &mut String, text: &str| {
        let text = text.trim_matches('\n');
        if !text.is_empty() {
            out.push_str(&format!("<blockquote expandable>{text}</blockquote>"));
        }
    };
    let mut rest = html;
    while let Some(start) = rest.find("<pre>") {
        quote(&mut out, &rest[..start]);
        let end = rest[start..].find("</pre>").map_or(rest.len(), |i| start + i + "</pre>".len());
        out.push_str(&rest[start..end]);
        rest = &rest[end..];
    }
    quote(&mut out, rest);
    out
}

fn telegram_inline(text: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = text.chars().collect();
    let (mut bold, mut code) = (false, false);
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '`' {
            out.push_str(if code { "</code>" } else { "<code>" });
            code = !code;
            i += 1;
            continue;
        }
        if !code {
            if c == '*' && chars.get(i + 1) == Some(&'*') {
                out.push_str(if bold { "</b>" } else { "<b>" });
                bold = !bold;
                i += 2;
                continue;
            }
            if c == '[' {
                if let Some((label, url, used)) = markdown_link(&chars[i..]) {
                    out.push_str(&format!("<a href=\"{}\">{}</a>", html_escape(&url), html_escape(&label)));
                    i += used;
                    continue;
                }
            }
        }
        out.push_str(&html_escape(&c.to_string()));
        i += 1;
    }
    if code {
        out.push_str("</code>");
    }
    if bold {
        out.push_str("</b>");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card() -> Card {
        Card {
            tone: Tone::Done,
            headline: "✅ Finished".into(),
            workspace: "CosmicaDesktop".into(),
            agent: "Claude Code".into(),
            session: "Fix the timer".into(),
            body: "Done. See **Slack card** and `notify.rs`.".into(),
            branch: "main".into(),
            folder: "~/cosmica-desktop".into(),
            ..Default::default()
        }
    }

    #[test]
    fn slack_gets_a_colored_card() {
        let mut body = json!({});
        card().slack(&mut body);
        assert_eq!(body["text"], "✅ Finished — CosmicaDesktop · Claude Code");
        let attachment = &body["attachments"][0];
        assert_eq!(attachment["color"], "#2eb67d");
        assert_eq!(attachment["blocks"][0]["text"]["text"], "*CosmicaDesktop*  ·  Claude Code\n_Fix the timer_");
        assert_eq!(attachment["blocks"][1]["text"]["text"], "Done. See *Slack card* and `notify.rs`.");
        assert_eq!(attachment["blocks"][2]["elements"][0]["text"], "`main`");
    }

    #[test]
    fn markdown_becomes_slack_markup() {
        assert_eq!(
            markdown_to_slack("## Result\n- one **two**\n[docs](https://example.com/a?b=1&c=2) <x>"),
            "*Result*\n• one *two*\n<https://example.com/a?b=1&amp;c=2|docs> &lt;x&gt;"
        );
        assert_eq!(markdown_to_slack("```\na < b\n```"), "```\na &lt; b\n```");
        assert_eq!(markdown_to_slack("[x](javascript:alert(1))"), "[x](javascript:alert(1))", "only web links become links");
    }

    #[test]
    fn telegram_html_is_always_balanced() {
        assert_eq!(markdown_to_telegram("**bold** and `code` <b>"), "<b>bold</b> and <code>code</code> &lt;b&gt;");
        assert_eq!(markdown_to_telegram("**never closed `either"), "<b>never closed <code>either</code></b>");
        assert_eq!(markdown_to_telegram("```\nx < y"), "<pre>x &lt; y</pre>");
        let mut body = json!({});
        card().telegram(&mut body);
        assert_eq!(body["parse_mode"], "HTML");
        assert!(body["text"]
            .as_str()
            .unwrap()
            .starts_with("<b>✅ Finished</b>\n<b>CosmicaDesktop</b> · Claude Code\n<i>Fix the timer</i>"));
    }

    #[test]
    fn telegram_quotes_the_reply_around_code_blocks() {
        let html = markdown_to_telegram("Done.\n```\nlet a = 1;\n```\nAll **good** <pre>");
        assert_eq!(
            telegram_quoted(&html),
            "<blockquote expandable>Done.</blockquote><pre>let a = 1;</pre><blockquote expandable>All <b>good</b> &lt;pre&gt;</blockquote>"
        );
        let mut body = json!({});
        card().telegram(&mut body);
        assert!(body["text"]
            .as_str()
            .unwrap()
            .contains("<blockquote expandable>Done. See <b>Slack card</b> and <code>notify.rs</code>.</blockquote>"));
    }

    #[test]
    fn the_link_goes_last_and_only_when_it_is_safe() {
        let linked = Card { link: "https://mac.tail1.ts.net:8743/#12".into(), link_label: "바로 확인하기".into(), ..card() };
        let mut slack = json!({});
        linked.slack(&mut slack);
        let blocks = slack["attachments"][0]["blocks"].as_array().unwrap();
        assert_eq!(blocks.last().unwrap()["text"]["text"], "*바로 확인하기:* <https://mac.tail1.ts.net:8743/#12>");
        let mut telegram = json!({});
        linked.telegram(&mut telegram);
        assert!(telegram["text"]
            .as_str()
            .unwrap()
            .ends_with("<b>바로 확인하기:</b> <a href=\"https://mac.tail1.ts.net:8743/#12\">https://mac.tail1.ts.net:8743/#12</a>"));
        let mut discord = json!({});
        linked.discord(&mut discord);
        assert!(discord["embeds"][0]["description"].as_str().unwrap().ends_with("**바로 확인하기:** https://mac.tail1.ts.net:8743/#12"));
        assert!(linked.plain().ends_with("바로 확인하기: https://mac.tail1.ts.net:8743/#12"));
        for bad in ["javascript:alert(1)", "http://plain.example/", "https://x.example/a b", "https://x.example/\"><b>", "https://x|y"] {
            let card = Card { link: bad.into(), link_label: "L".into(), ..card() };
            let mut slack = json!({});
            card.slack(&mut slack);
            assert!(!slack.to_string().contains("L:"), "{bad}");
            assert!(!card.plain().contains("L:"), "{bad}");
        }
    }

    #[test]
    fn discord_gets_an_embed() {
        let mut body = json!({});
        Card { tone: Tone::Ask, workspace: "a_b".into(), ..card() }.discord(&mut body);
        let embed = &body["embeds"][0];
        assert_eq!(embed["color"], 0xe8912d);
        assert!(embed["description"].as_str().unwrap().starts_with("**a\\_b** · Claude Code"));
        assert_eq!(embed["footer"]["text"], "main · ~/cosmica-desktop");
    }

    #[test]
    fn empty_fields_are_left_out() {
        let bare = Card { headline: "🔔 Agentty".into(), body: "It works.".into(), ..Default::default() };
        assert_eq!(bare.preview(), "🔔 Agentty");
        let mut body = json!({});
        bare.slack(&mut body);
        assert_eq!(body["attachments"][0]["blocks"].as_array().unwrap().len(), 1);
    }
}
