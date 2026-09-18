//! The plugin guides, rendered for the in-app browser: the Markdown Agentty ships is turned into a
//! small dark-themed HTML page, so the guide opens inside Agentty instead of another app.

use std::path::{Path, PathBuf};

/// Where the guides and the SDK are written for reading and for plugin authors to copy.
pub fn sdk_dir() -> PathBuf {
    agentty_bridge::plugins::store::plugins_dir().join(".sdk")
}

/// Writes the SDK, the Markdown sources and their HTML, and returns the page to open.
pub fn unpack() -> std::io::Result<PathBuf> {
    use agentty_bridge::plugins::store::{AI_PROMPT, GUIDE, NODE_SDK, NODE_SDK_TYPES, PROTOCOL, USAGE};
    let dir = sdk_dir();
    std::fs::create_dir_all(&dir)?;
    for (name, contents) in [
        ("README.md", GUIDE),
        ("usage.md", USAGE),
        ("protocol.md", PROTOCOL),
        ("AI_PROMPT.md", AI_PROMPT),
        ("agentty-plugin.mjs", NODE_SDK),
        ("agentty-plugin.d.ts", NODE_SDK_TYPES),
    ] {
        std::fs::write(dir.join(name), contents)?;
    }
    for (page, title, markdown) in [
        ("README.html", "Agentty plugin guide", GUIDE),
        ("usage.html", "Using plugins", USAGE),
        ("protocol.html", "Agentty plugin protocol", PROTOCOL),
        ("AI_PROMPT.html", "Building a plugin with AI", AI_PROMPT),
    ] {
        std::fs::write(dir.join(page), render(title, markdown))?;
    }
    Ok(dir.join("README.html"))
}

pub fn file_url(path: &Path) -> String {
    let encoded: String = path
        .to_string_lossy()
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => (b as char).to_string(),
            other => format!("%{other:02X}"),
        })
        .collect();
    format!("file://{encoded}")
}

const STYLE: &str = "\
:root { color-scheme: dark; }
body { margin: 0 auto; padding: 28px 30px 80px; max-width: 820px; background: #1f1f1f; color: #cccccc;
  font: 14px/1.65 -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif; }
h1, h2, h3, h4 { color: #ffffff; line-height: 1.3; margin: 1.6em 0 .6em; }
h1 { font-size: 1.7em; margin-top: .2em; } h2 { font-size: 1.3em; border-bottom: 1px solid #2b2b2b; padding-bottom: .3em; }
h3 { font-size: 1.1em; }
a { color: #4daafc; text-decoration: none; } a:hover { text-decoration: underline; }
code { font-family: 'JetBrains Mono', ui-monospace, monospace; font-size: .92em; background: #141414; padding: .15em .35em; border-radius: 4px; }
pre { background: #141414; border: 1px solid #2b2b2b; border-radius: 8px; padding: 12px 14px; overflow-x: auto; }
pre code { background: none; padding: 0; }
table { border-collapse: collapse; width: 100%; margin: 1em 0; font-size: .95em; }
th, td { border: 1px solid #2b2b2b; padding: 7px 10px; text-align: left; vertical-align: top; }
th { background: #252526; color: #ffffff; }
blockquote { margin: 1em 0; padding: .3em 0 .3em 14px; border-left: 3px solid #0078d4; color: #9d9d9d; }
ul, ol { padding-left: 22px; } li { margin: .25em 0; }
hr { border: none; border-top: 1px solid #2b2b2b; margin: 2em 0; }
";

/// Minimal Markdown → HTML for the guides Agentty ships (headings, lists, tables, code, links).
pub fn render(title: &str, markdown: &str) -> String {
    let mut out = String::new();
    let mut lines = markdown.lines().peekable();
    let mut list: Option<&'static str> = None;
    // Markdown wraps paragraphs over several lines; they belong to one <p>.
    let mut paragraph: Vec<String> = Vec::new();
    let close_list = |out: &mut String, list: &mut Option<&'static str>| {
        if let Some(tag) = list.take() {
            out.push_str(&format!("</{tag}>\n"));
        }
    };
    macro_rules! flush_paragraph {
        () => {
            if !paragraph.is_empty() {
                out.push_str(&format!("<p>{}</p>\n", paragraph.join(" ")));
                paragraph.clear();
            }
        };
    }
    while let Some(line) = lines.next() {
        let trimmed = line.trim_end();
        // Fenced code blocks are copied verbatim.
        if let Some(rest) = trimmed.trim_start().strip_prefix("```") {
            flush_paragraph!();
            close_list(&mut out, &mut list);
            let _language = rest;
            out.push_str("<pre><code>");
            for code in lines.by_ref() {
                if code.trim_start().starts_with("```") {
                    break;
                }
                out.push_str(&escape(code));
                out.push('\n');
            }
            out.push_str("</code></pre>\n");
            continue;
        }
        // Tables: a header row followed by the `---` separator.
        if trimmed.starts_with('|') && lines.peek().is_some_and(|next| is_table_rule(next)) {
            flush_paragraph!();
            close_list(&mut out, &mut list);
            lines.next();
            out.push_str("<table>\n<tr>");
            for cell in split_row(trimmed) {
                out.push_str(&format!("<th>{}</th>", inline(&cell)));
            }
            out.push_str("</tr>\n");
            while lines.peek().is_some_and(|next| next.trim_start().starts_with('|')) {
                let row = lines.next().unwrap_or_default();
                out.push_str("<tr>");
                for cell in split_row(row.trim_end()) {
                    out.push_str(&format!("<td>{}</td>", inline(&cell)));
                }
                out.push_str("</tr>\n");
            }
            out.push_str("</table>\n");
            continue;
        }
        if trimmed.trim().is_empty() {
            flush_paragraph!();
            close_list(&mut out, &mut list);
            continue;
        }
        if trimmed.starts_with("---") && trimmed.chars().all(|c| c == '-') {
            flush_paragraph!();
            close_list(&mut out, &mut list);
            out.push_str("<hr>\n");
            continue;
        }
        let heading = trimmed.chars().take_while(|c| *c == '#').count();
        if (1..=4).contains(&heading) && trimmed.chars().nth(heading) == Some(' ') {
            flush_paragraph!();
            close_list(&mut out, &mut list);
            // Slugged like GitHub, so the tables of contents in the guides jump to the section.
            let text = trimmed[heading + 1..].trim();
            out.push_str(&format!("<h{heading} id=\"{}\">{}</h{heading}>\n", slug(text), inline(text)));
            continue;
        }
        if let Some(rest) = trimmed.trim_start().strip_prefix("> ") {
            flush_paragraph!();
            close_list(&mut out, &mut list);
            out.push_str(&format!("<blockquote>{}</blockquote>\n", inline(rest)));
            continue;
        }
        let bullet = trimmed.trim_start().strip_prefix("- ").or_else(|| trimmed.trim_start().strip_prefix("* "));
        let numbered = trimmed
            .trim_start()
            .split_once(". ")
            .filter(|(number, _)| !number.is_empty() && number.chars().all(|c| c.is_ascii_digit()))
            .map(|(_, rest)| rest);
        if let Some(item) = bullet.or(numbered) {
            flush_paragraph!();
            let tag = if bullet.is_some() { "ul" } else { "ol" };
            if list != Some(tag) {
                close_list(&mut out, &mut list);
                out.push_str(&format!("<{tag}>\n"));
                list = Some(tag);
            }
            out.push_str(&format!("<li>{}</li>\n", inline(item)));
            continue;
        }
        // A continuation line of the item above, or a paragraph.
        match list {
            Some(_) if trimmed.starts_with("  ") => {
                out.pop();
                out.truncate(out.trim_end_matches("</li>\n").len());
                out.push_str(&format!(" {}</li>\n", inline(trimmed.trim())));
            }
            _ => {
                close_list(&mut out, &mut list);
                paragraph.push(inline(trimmed));
            }
        }
    }
    flush_paragraph!();
    close_list(&mut out, &mut list);
    format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>{}</title><style>{STYLE}</style></head><body>\n{out}</body></html>\n",
        escape(title)
    )
}

/// `## Sending prompts` → `sending-prompts`, the anchor Markdown tables of contents link to.
fn slug(text: &str) -> String {
    let plain: String = text.chars().filter(|c| !matches!(c, '`' | '*' | '(' | ')' | '.' | ',' | ':' | '\'' | '"')).collect();
    plain
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

fn is_table_rule(line: &str) -> bool {
    let line = line.trim();
    line.starts_with('|') && line.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

fn split_row(line: &str) -> Vec<String> {
    line.trim().trim_matches('|').split('|').map(|cell| cell.trim().to_string()).collect()
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Inline Markdown: `code`, **bold**, [text](url). Everything else is escaped text.
fn inline(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while !rest.is_empty() {
        if let Some(start) = rest.find('`') {
            if let Some(end) = rest[start + 1..].find('`') {
                out.push_str(&escape(&rest[..start]));
                out.push_str(&format!("<code>{}</code>", escape(&rest[start + 1..start + 1 + end])));
                rest = &rest[start + end + 2..];
                continue;
            }
        }
        if let Some(start) = rest.find("**") {
            if let Some(end) = rest[start + 2..].find("**") {
                out.push_str(&escape(&rest[..start]));
                out.push_str(&format!("<strong>{}</strong>", escape(&rest[start + 2..start + 2 + end])));
                rest = &rest[start + end + 4..];
                continue;
            }
        }
        if let Some(start) = rest.find('[') {
            let after = &rest[start..];
            if let Some((label, tail)) = after[1..].split_once("](") {
                if let Some((url, tail)) = tail.split_once(')') {
                    out.push_str(&escape(&rest[..start]));
                    // Links between the guides open the matching page; anything else opens on the web.
                    let target = url.strip_suffix(".md").map(|name| format!("{name}.html")).unwrap_or_else(|| url.to_string());
                    out.push_str(&format!("<a href=\"{}\">{}</a>", escape(&target), escape(label)));
                    rest = tail;
                    continue;
                }
            }
        }
        out.push_str(&escape(rest));
        break;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_the_markdown_the_guides_use() {
        let html = render(
            "T",
            "# Title\n\nText with `code`, **bold** and a [link](protocol.md).\n\n- one\n- two\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\n```js\nconst x = 1 < 2;\n```\n",
        );
        assert!(html.contains("<h1 id=\"title\">Title</h1>"), "{html}");
        assert!(html.contains("<p>Text with <code>code</code>"), "wrapped lines join into one paragraph");
        assert!(html.contains("<code>code</code>") && html.contains("<strong>bold</strong>"));
        assert!(html.contains("<a href=\"protocol.html\">link</a>"), "guide links stay inside the app");
        assert!(html.contains("<ul>\n<li>one</li>"));
        assert!(html.contains("<th>A</th>") && html.contains("<td>2</td>"));
        assert!(html.contains("const x = 1 &lt; 2;"), "code is escaped, not executed");
        assert!(!html.contains("<script"));
    }

    #[test]
    fn wrapped_paragraphs_and_anchors() {
        let html = render("T", "## Sending prompts\n\nOne line\nand its continuation.\n\nNext paragraph.\n");
        assert!(html.contains("<h2 id=\"sending-prompts\">"), "{html}");
        assert!(html.contains("<p>One line and its continuation.</p>"), "{html}");
        assert!(html.contains("<p>Next paragraph.</p>"));
        assert_eq!(slug("Manifest (`agentty-plugin.json`)"), "manifest-agentty-pluginjson");
    }

    #[test]
    fn every_shipped_guide_renders() {
        use agentty_bridge::plugins::store::{AI_PROMPT, GUIDE, PROTOCOL, USAGE};
        for markdown in [GUIDE, USAGE, PROTOCOL, AI_PROMPT] {
            let html = render("Guide", markdown);
            assert!(html.contains("<h1 id="), "a guide with no heading?");
            assert!(!html.contains("<script"));
        }
        assert!(file_url(Path::new("/tmp/a b/x.html")).ends_with("/tmp/a%20b/x.html"));
    }
}
