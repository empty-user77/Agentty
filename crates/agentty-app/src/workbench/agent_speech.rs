//! Subagents' work in words a person reads: each tool call as a short sentence ("Reading
//! src/app.ts"), what an agent is doing right now (its own last remark, or its current call), and the
//! Markdown agents write (reports, remarks, the task they were given) drawn as text instead of raw marks.

use crate::i18n::{t, tf};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::TypeScale;
use agentty_bridge::subagent_trace::{Step, Trace};
use gpui::{div, prelude::*, px, FontStyle, FontWeight, HighlightStyle, SharedString, StyledText};
use std::ops::Range;

/// The last part of a path: `src/app/page.tsx` → `page.tsx`.
fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).find(|p| !p.is_empty()).unwrap_or(path)
}

/// At most `max` characters, with `…` when cut.
fn clip(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// A tool call as a sentence.
pub fn narrate(step: &Step, cx: &gpui::App) -> String {
    let Some(tool) = step.tool.as_deref() else { return plain(&step.target, 140) };
    let target = step.target.as_str();
    let say = |key: &'static str, args: &[(&str, &str)]| tf(cx, key, args);
    match tool {
        "Read" => say("narrate.read", &[("file", file_name(target))]),
        "Edit" | "MultiEdit" | "NotebookEdit" => say("narrate.edit", &[("file", file_name(target))]),
        "Write" => say("narrate.write", &[("file", file_name(target))]),
        "Grep" => match target.split_once("  in ") {
            Some((what, dir)) => say("narrate.search_in", &[("what", &clip(what, 40)), ("dir", file_name(dir))]),
            None => say("narrate.search", &[("what", &clip(target, 40))]),
        },
        "Glob" => say("narrate.find_files", &[("what", &clip(target, 40))]),
        // The agent says why it runs a command; that reads better than the command.
        "Bash" | "PowerShell" => match &step.detail {
            Some(why) => clip(why, 120),
            None => say("narrate.run", &[("command", &clip(target, 48))]),
        },
        "WebFetch" => say("narrate.fetch", &[("site", &site(target))]),
        "WebSearch" => say("narrate.web_search", &[("what", &clip(target, 48))]),
        "Agent" | "Task" => say("narrate.delegate", &[("what", &clip(target, 60))]),
        "TodoWrite" | "TaskCreate" | "TaskUpdate" => t(cx, "narrate.plan").to_string(),
        "Skill" => say("narrate.skill", &[("what", &clip(target, 40))]),
        _ => {
            // `mcp__server__tool` → `server tool`.
            let name = tool.strip_prefix("mcp__").map(|rest| rest.replace("__", " ")).unwrap_or_else(|| tool.to_string());
            say("narrate.tool", &[("tool", &name)])
        }
    }
}

/// `https://docs.rs/gpui/latest` → `docs.rs`.
fn site(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.split(['/', '?', '#']).next().unwrap_or(rest).trim_start_matches("www.").to_string()
}

/// What an agent is busy with, as it would say it: what it last said, while that is newer than its
/// last tool call, otherwise that call (the one it waits on, or the one it just made). Once `done`,
/// the first line of its report, or the last thing it said.
pub fn speech(trace: &Trace, done: bool, cx: &gpui::App) -> String {
    let last_tool = trace.steps.iter().rposition(|s| s.tool.is_some());
    let last_note = trace.steps.iter().rposition(|s| s.tool.is_none() && !s.target.trim().is_empty());
    if done || trace.finished {
        return match last_note {
            Some(note) if last_tool.is_none_or(|tool| note > tool) => plain(&trace.steps[note].target, 140),
            _ => t(cx, "narrate.done").to_string(),
        };
    }
    match (last_note, last_tool) {
        // A remark right before the call says why it makes it.
        (Some(note), Some(tool)) if note + 1 == tool => plain(&trace.steps[note].target, 140),
        (Some(note), Some(tool)) if note > tool => plain(&trace.steps[note].target, 140),
        (_, Some(tool)) => narrate(&trace.steps[tool], cx),
        (Some(note), None) => plain(&trace.steps[note].target, 140),
        (None, None) => t(cx, "collab.thinking").to_string(),
    }
}

/// The first sentence of some Markdown, without its marks, at most `max` characters.
pub fn plain(text: &str, max: usize) -> String {
    let line = strip_wrappers(text).lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with("```")).unwrap_or("").to_string();
    // Heading, quote and list marks go; a `**` that opens bold stays.
    let line = line.trim_start_matches(['#', '>', ' ']);
    let line = ["- ", "* ", "+ "].iter().find_map(|m| line.strip_prefix(m)).unwrap_or(line).to_string();
    let (shown, _) = inline(&line);
    // One sentence is enough for a bubble.
    // The first ". " that ends a sentence, not a number ("1. Done.").
    let end = shown.match_indices(". ").map(|(at, _)| at).find(|&at| {
        let word = shown[..at].rsplit(' ').next().unwrap_or("");
        !word.chars().all(|c| c.is_ascii_digit())
    });
    let sentence = end.map_or(shown.as_str(), |end| &shown[..=end]);
    clip(sentence, max)
}

/// Drops the lines that only wrap a message in a tag (`<teammate-message …>` … `</teammate-message>`).
pub fn strip_wrappers(text: &str) -> String {
    let is_tag = |line: &str| {
        let line = line.trim();
        line.starts_with('<')
            && line.ends_with('>')
            && line[1..].chars().next().is_some_and(|c| c == '/' || c.is_ascii_alphabetic())
            && !line[1..line.len() - 1].contains('<')
    };
    text.lines().filter(|l| !is_tag(l)).collect::<Vec<_>>().join("\n")
}

/// Inline marks (`**bold**`, `*italic*`, `` `code` ``, `[text](link)`) taken out of `line`, and
/// where each styled run is in what is left.
fn inline(line: &str) -> (String, Vec<(Range<usize>, Mark)>) {
    let mut out = String::with_capacity(line.len());
    let mut marks = Vec::new();
    let mut rest = line;
    while !rest.is_empty() {
        let run = |marker: &str, rest: &str| -> Option<(usize, usize)> {
            let body = rest.strip_prefix(marker)?;
            let end = body.find(marker)?;
            (end > 0 && !body[..end].starts_with(' ')).then_some((marker.len(), end))
        };
        if let Some((open, len)) = rest.starts_with('`').then(|| run("`", rest)).flatten() {
            let start = out.len();
            out.push_str(&rest[open..open + len]);
            marks.push((start..out.len(), Mark::Code));
            rest = &rest[open + len + open..];
            continue;
        }
        if let Some((open, len)) = run("**", rest).or_else(|| run("__", rest)) {
            let start = out.len();
            out.push_str(&rest[open..open + len]);
            marks.push((start..out.len(), Mark::Bold));
            rest = &rest[open + len + open..];
            continue;
        }
        if let Some((open, len)) = run("*", rest) {
            let start = out.len();
            out.push_str(&rest[open..open + len]);
            marks.push((start..out.len(), Mark::Italic));
            rest = &rest[open + len + open..];
            continue;
        }
        // `[text](link)` → `text`.
        if let Some(body) = rest.strip_prefix('[') {
            if let Some((text, after)) = body.split_once("](") {
                if let Some(close) = after.find(')') {
                    if !text.contains(']') {
                        let start = out.len();
                        out.push_str(text);
                        marks.push((start..out.len(), Mark::Link));
                        rest = &after[close + 1..];
                        continue;
                    }
                }
            }
        }
        let ch = rest.chars().next().unwrap_or(' ');
        out.push(ch);
        rest = &rest[ch.len_utf8()..];
    }
    (out, marks)
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mark {
    Bold,
    Italic,
    Code,
    Link,
}

fn styled(line: &str) -> StyledText {
    let (text, marks) = inline(line);
    let highlights = marks.into_iter().map(|(range, mark)| {
        let style = match mark {
            Mark::Bold => HighlightStyle { font_weight: Some(FontWeight::BOLD), color: Some(hex(Chrome::BRIGHT)), ..Default::default() },
            Mark::Italic => HighlightStyle { font_style: Some(FontStyle::Italic), ..Default::default() },
            Mark::Code => HighlightStyle {
                color: Some(hex(Chrome::BRIGHT)),
                background_color: Some(hex_alpha(Chrome::MUTED, 0.22)),
                ..Default::default()
            },
            Mark::Link => HighlightStyle { color: Some(hex(Chrome::BLUE)), ..Default::default() },
        };
        (range, style)
    });
    StyledText::new(SharedString::from(if text.is_empty() { " ".to_string() } else { text })).with_highlights(highlights)
}

/// Markdown as text: headings, lists, quotes, code blocks and tables, with inline marks styled.
/// `keep` limits the blocks drawn (a closed remark shows one), with `…` when some were left out.
pub fn markdown(text: &str, keep: usize) -> gpui::Div {
    let text = strip_wrappers(text);
    let mut blocks: Vec<gpui::AnyElement> = Vec::new();
    let mut lines = text.lines().peekable();
    let mut more = false;
    while let Some(line) = lines.next() {
        if blocks.len() >= keep {
            more = lines.peek().is_some() || !line.trim().is_empty();
            break;
        }
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("```") {
            let mut code = Vec::new();
            for next in lines.by_ref() {
                if next.trim_start().starts_with("```") {
                    break;
                }
                code.push(next.to_string());
            }
            blocks.push(
                div()
                    .p_1p5()
                    .rounded_md()
                    .bg(hex_alpha(Chrome::MUTED, 0.12))
                    .t_caption()
                    .font_family("JetBrains Mono")
                    .flex()
                    .flex_col()
                    .children(code.into_iter().map(|l| div().child(if l.is_empty() { " ".to_string() } else { l })))
                    .into_any_element(),
            );
            continue;
        }
        if let Some(heading) = ["### ", "## ", "# "].iter().find_map(|h| trimmed.strip_prefix(h)) {
            blocks.push(
                div().pt_1().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(styled(heading)).into_any_element(),
            );
            continue;
        }
        if let Some(quote) = trimmed.strip_prefix("> ").or_else(|| trimmed.strip_prefix('>')) {
            blocks.push(
                div()
                    .pl_2()
                    .border_l_2()
                    .border_color(hex_alpha(Chrome::MUTED, 0.5))
                    .text_color(hex(Chrome::MUTED))
                    .child(styled(quote))
                    .into_any_element(),
            );
            continue;
        }
        if trimmed.starts_with('|') {
            // A table row as a line of cells; the `|---|` rule under the header is left out.
            if trimmed.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ')) {
                continue;
            }
            let cells: Vec<&str> = trimmed.trim_matches('|').split('|').map(str::trim).collect();
            blocks.push(
                div()
                    .flex()
                    .gap_3()
                    .t_caption()
                    .children(cells.into_iter().map(|cell| div().flex_1().min_w_0().child(styled(cell))))
                    .into_any_element(),
            );
            continue;
        }
        let indent = (line.len() - trimmed.len()) as f32 * 4.;
        let bullet = ["- ", "* ", "+ "].iter().find_map(|b| trimmed.strip_prefix(b)).map(|item| ("•".to_string(), item));
        let numbered = trimmed
            .split_once(". ")
            .filter(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
            .map(|(n, item)| (format!("{n}."), item));
        if let Some((marker, item)) = bullet.or(numbered) {
            blocks.push(
                div()
                    .pl(px(indent))
                    .flex()
                    .items_start()
                    .gap_1p5()
                    .child(div().flex_shrink_0().text_color(hex(Chrome::MUTED)).child(marker))
                    .child(div().flex_1().min_w_0().child(styled(item)))
                    .into_any_element(),
            );
            continue;
        }
        blocks.push(div().child(styled(trimmed)).into_any_element());
    }
    div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .gap_1()
        .children(blocks)
        .when(more, |d| d.child(div().text_color(hex(Chrome::MUTED)).child("…")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_come_out_of_the_text() {
        let (text, marks) = inline("Run **cargo test** in `crates/app` and see [the docs](https://x.dev).");
        assert_eq!(text, "Run cargo test in crates/app and see the docs.");
        assert_eq!(marks.iter().map(|(_, m)| *m).collect::<Vec<_>>(), [Mark::Bold, Mark::Code, Mark::Link]);
        assert_eq!(&text[marks[0].0.clone()], "cargo test");
        // A lone star or backtick stays as it is.
        assert_eq!(inline("2 * 3 = 6, a ` b").0, "2 * 3 = 6, a ` b");
    }

    #[test]
    fn a_bubble_gets_one_plain_sentence() {
        assert_eq!(plain("**1. Done.** All 449 entries are in. More follows.", 140), "1. Done.");
        assert_eq!(plain("- All 449 entries are in. More follows.", 140), "All 449 entries are in.");
        assert_eq!(plain("<teammate-message id=\"x\">\nRun the build.\n</teammate-message>", 140), "Run the build.");
        assert_eq!(plain("## Summary\nok", 140), "Summary");
        assert_eq!(site("https://www.docs.rs/gpui?x=1"), "docs.rs");
        assert_eq!(file_name("src/app/page.tsx"), "page.tsx");
    }
}
