//! Full-text search over session transcripts, for the workspace and session search boxes.
//!
//! A search runs over every local session while someone is still typing, so it has to be quick:
//! files are searched in parallel, as bytes, without turning every line into a lower-case string
//! first. A raw hit only counts when a text value of that line holds the words — a match inside a
//! JSON key, an id or a folder path recorded on every line is not what anyone searched for — and
//! each hit comes back with the sentence around it, so the result can show where it was found.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// How much of a transcript a search reads. Without a limit, one file left without a line break (a
/// corrupted transcript, or one huge tool result) would be read into memory whole — once per
/// worker searching at the same time; anything real is far below this.
const MAX_SCAN: u64 = 16 * 1024 * 1024;

/// Characters kept before and after the words in a snippet.
const BEFORE: usize = 32;
const AFTER: usize = 80;

/// Who said the words a search found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitRole {
    User,
    Assistant,
    /// A tool call, its result, or anything else recorded in the transcript.
    Other,
}

/// Where a search found its words: the text around them, cut into what came before, the words as
/// written there, and what follows (each on one line).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptHit {
    pub role: HitRole,
    pub before: String,
    pub matched: String,
    pub after: String,
}

/// A prepared search: the words in lower case, and how to find them in the raw file.
pub struct Needle {
    lower: String,
    /// The words as they are written inside a JSON string (quotes and backslashes escaped), lower
    /// case, as bytes.
    raw: Vec<u8>,
    /// Some letter has a case outside ASCII (`É`, `Ω`): a byte search that only folds ASCII would
    /// miss the other case, so lines are compared the slow way.
    unicode_case: bool,
}

impl Needle {
    /// `None` for blank words.
    pub fn new(words: &str) -> Option<Self> {
        let lower = words.trim().to_lowercase();
        if lower.is_empty() {
            return None;
        }
        let escaped = serde_json::to_string(&lower).unwrap_or_default();
        let raw = escaped.get(1..escaped.len().saturating_sub(1)).unwrap_or(&lower).as_bytes().to_vec();
        let unicode_case = lower.chars().any(|c| !c.is_ascii() && c.to_uppercase().ne(c.to_lowercase()));
        Some(Self { lower, raw, unicode_case })
    }

    pub fn words(&self) -> &str {
        &self.lower
    }
}

/// Searches `paths` in parallel; the result is in the same order, `None` where nothing matched.
/// `cancelled` is checked between files, so a search the user typed past stops early (what it had
/// not reached yet comes back as `None`).
pub fn search_transcripts(paths: &[PathBuf], needle: &Needle, cancelled: &AtomicBool) -> Vec<Option<TranscriptHit>> {
    let mut results: Vec<Option<TranscriptHit>> = vec![None; paths.len()];
    if paths.is_empty() {
        return results;
    }
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(2, 6).min(paths.len());
    let next = AtomicUsize::new(0);
    let found: Vec<Vec<(usize, TranscriptHit)>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut mine = Vec::new();
                    loop {
                        if cancelled.load(Ordering::Relaxed) {
                            break;
                        }
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(path) = paths.get(index) else { break };
                        if let Some(hit) = find_in_file(path, needle) {
                            mine.push((index, hit));
                        }
                    }
                    mine
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap_or_default()).collect()
    });
    for (index, hit) in found.into_iter().flatten() {
        results[index] = Some(hit);
    }
    results
}

/// The first place in the transcript at `path` where a text value holds the words.
pub fn find_in_file(path: &Path, needle: &Needle) -> Option<TranscriptHit> {
    use std::io::Read;
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_SCAN).read_to_end(&mut bytes).ok()?;
    find_in_bytes(&bytes, needle)
}

/// Lines read whole after a tool's output held the words, looking for someone saying them.
const MORE_LINES: usize = 12;

/// [`find_in_file`] over a transcript already read. Words someone said (the person or the agent)
/// are preferred: a tool's output that happens to hold them — a path in a listing — only counts
/// when nobody said them.
pub fn find_in_bytes(bytes: &[u8], needle: &Needle) -> Option<TranscriptHit> {
    let mut tool: Option<TranscriptHit> = None;
    let mut budget = MORE_LINES;
    if needle.unicode_case {
        // The slow way, one line at a time: lower-casing changes byte lengths.
        for line in bytes.split(|b| *b == b'\n') {
            let line = String::from_utf8_lossy(line);
            if !line.to_lowercase().contains(&needle.lower) {
                continue;
            }
            if let Some(hit) = hit_in_line(&line, needle).and_then(|hit| said(&mut tool, hit)) {
                return Some(hit);
            }
        }
        return tool;
    }
    let mut from = 0;
    while let Some(at) = find_folded(&bytes[from..], &needle.raw).map(|i| i + from) {
        let start = bytes[..at].iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
        let end = bytes[at..].iter().position(|b| *b == b'\n').map_or(bytes.len(), |i| at + i);
        // In a key, or in a folder, id or version: every line of a session carries those, and
        // reading each line whole to find that out is what made a folder's name slow to search.
        if let Some(close) = bookkeeping(&bytes[start..end], at - start) {
            from = start + close + 1;
            continue;
        }
        if let Some(hit) = hit_in_line(&String::from_utf8_lossy(&bytes[start..end]), needle).and_then(|hit| said(&mut tool, hit)) {
            return Some(hit);
        }
        if tool.is_some() {
            if budget == 0 {
                break;
            }
            budget -= 1;
        }
        // Only in a key, an id or a path on this line (or a tool said it): carry on after it.
        from = end + 1;
        if from >= bytes.len() {
            break;
        }
    }
    tool
}

/// A hit said by someone, which ends the search; a tool's output is kept in `tool` for when
/// nobody says the words.
fn said(tool: &mut Option<TranscriptHit>, hit: TranscriptHit) -> Option<TranscriptHit> {
    if hit.role != HitRole::Other {
        return Some(hit);
    }
    tool.get_or_insert(hit);
    None
}

/// When the match at `at` of a JSON `line` is in a key, or in the value of one of the
/// [`SKIPPED_KEYS`], the offset of the quote that ends that string (to carry on after it).
fn bookkeeping(line: &[u8], at: usize) -> Option<usize> {
    let open = quote_before(line, at)?;
    let close = quote_from(line, at)?;
    let before = line[..open].iter().rposition(|b| !b.is_ascii_whitespace())?;
    match line[before] {
        b':' => {
            let key_close = line[..before].iter().rposition(|b| !b.is_ascii_whitespace())?;
            if line[key_close] != b'"' {
                return None;
            }
            let key_open = quote_before(line, key_close)?;
            let key = &line[key_open + 1..key_close];
            SKIPPED_KEYS.iter().any(|k| k.as_bytes() == key).then_some(close)
        }
        // A string after `{` or `,` is a key when a `:` follows it (else an item of a list).
        b'{' | b',' => {
            let next = line[close + 1..].iter().position(|b| !b.is_ascii_whitespace())? + close + 1;
            (line[next] == b':').then_some(close)
        }
        _ => None,
    }
}

/// The last unescaped `"` before `at`.
fn quote_before(line: &[u8], at: usize) -> Option<usize> {
    (0..at).rev().find(|&i| line[i] == b'"' && !escaped(line, i))
}

/// The first unescaped `"` at or after `at`.
fn quote_from(line: &[u8], at: usize) -> Option<usize> {
    (at..line.len()).find(|&i| line[i] == b'"' && !escaped(line, i))
}

/// Whether the byte at `i` follows an odd number of backslashes.
fn escaped(line: &[u8], i: usize) -> bool {
    line[..i].iter().rev().take_while(|b| **b == b'\\').count() % 2 == 1
}

/// Where `needle` (lower case) first appears in `haystack`, ignoring the case of ASCII letters.
fn find_folded(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    let first = *needle.first()?;
    if !first.is_ascii_alphabetic() && !needle.iter().any(u8::is_ascii_alphabetic) {
        // Nothing to fold (Korean, Japanese, digits): an exact search is fastest.
        return memchr::memmem::find(haystack, needle);
    }
    let (lower, upper) = (first.to_ascii_lowercase(), first.to_ascii_uppercase());
    let mut from = 0;
    while let Some(i) = memchr::memchr2(lower, upper, &haystack[from..]).map(|i| i + from) {
        if haystack.len() - i < needle.len() {
            return None;
        }
        if haystack[i..i + needle.len()].eq_ignore_ascii_case(needle) {
            return Some(i);
        }
        from = i + 1;
    }
    None
}

/// Keys whose values are bookkeeping, not what was said: ids, folders, versions, signatures and
/// pictures. A search for a folder's name would otherwise match every line of every session in it.
const SKIPPED_KEYS: &[&str] = &[
    "uuid",
    "parentUuid",
    "leafUuid",
    "sessionId",
    "session_id",
    "requestId",
    "messageId",
    "id",
    "tool_use_id",
    "call_id",
    "cwd",
    "gitBranch",
    "version",
    "timestamp",
    "type",
    "model",
    "signature",
    "encrypted_content",
    "data",
    "media_type",
    "userType",
    "entrypoint",
    "role",
    "stop_reason",
];

/// The hit on one transcript line, when one of its text values holds the words.
fn hit_in_line(line: &str, needle: &Needle) -> Option<TranscriptHit> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        // Not JSON (a plain-text log): the line itself is the text.
        return snippet(line, needle).map(|(before, matched, after)| TranscriptHit { role: HitRole::Other, before, matched, after });
    };
    if injected(&value) {
        return None;
    }
    let text = first_text_with(&value, needle)?;
    let (before, matched, after) = snippet(text, needle)?;
    let mut role = role_of(&value);
    // An agent's line holds its tool calls too: words only in a call's input were not said.
    if role == HitRole::Assistant && !said_in_text(&value, needle) {
        role = HitRole::Other;
    }
    Some(TranscriptHit { role, before, matched, after })
}

/// Whether a text block of the line's message (not a tool call or a thought) holds the words.
fn said_in_text(value: &serde_json::Value, needle: &Needle) -> bool {
    let content = value.get("message").or_else(|| value.get("payload")).and_then(|m| m.get("content"));
    match content {
        Some(serde_json::Value::String(text)) => contains_folded(text, needle),
        Some(serde_json::Value::Array(items)) => items.iter().any(|item| {
            let kind = item.get("type").and_then(|t| t.as_str()).unwrap_or_default();
            matches!(kind, "text" | "output_text" | "input_text")
                && item.get("text").and_then(|t| t.as_str()).is_some_and(|t| contains_folded(t, needle))
        }),
        _ => false,
    }
}

/// Lines the agent CLI adds to every session on its own — the skill list, project instructions,
/// reminders, the system prompt — rather than anything said in the conversation. A word from them
/// would match nearly every session.
fn injected(value: &serde_json::Value) -> bool {
    let kind = value.get("type").and_then(|t| t.as_str()).unwrap_or_default();
    matches!(kind, "attachment" | "system" | "file-history-snapshot" | "queue-operation" | "session_meta" | "turn_context")
        || value.get("isMeta").and_then(|m| m.as_bool()) == Some(true)
}

fn first_text_with<'a>(value: &'a serde_json::Value, needle: &Needle) -> Option<&'a str> {
    match value {
        serde_json::Value::String(s) => contains_folded(s, needle).then_some(s.as_str()),
        serde_json::Value::Array(items) => items.iter().find_map(|v| first_text_with(v, needle)),
        serde_json::Value::Object(map) => {
            map.iter().filter(|(k, _)| !SKIPPED_KEYS.contains(&k.as_str())).find_map(|(_, v)| first_text_with(v, needle))
        }
        _ => None,
    }
}

fn contains_folded(text: &str, needle: &Needle) -> bool {
    if needle.unicode_case {
        text.to_lowercase().contains(&needle.lower)
    } else {
        find_folded(text.as_bytes(), needle.lower.as_bytes()).is_some()
    }
}

/// Who a transcript line is from: Claude Code's `type`, Codex's `payload.role`, or a `role` field.
fn role_of(value: &serde_json::Value) -> HitRole {
    let role = value
        .get("message")
        .and_then(|m| m.get("role"))
        .or_else(|| value.get("payload").and_then(|p| p.get("role")))
        .or_else(|| value.get("role"))
        .or_else(|| value.get("type"))
        .and_then(|r| r.as_str())
        .unwrap_or_default();
    // A user line that carries a tool's result is the tool talking, not the person.
    let tool_result = value
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
        .is_some_and(|items| items.iter().any(|i| i.get("type").and_then(|t| t.as_str()) == Some("tool_result")));
    match role {
        "user" if !tool_result => HitRole::User,
        "assistant" | "model" | "gemini" => HitRole::Assistant,
        _ => HitRole::Other,
    }
}

/// `text` cut around the first place the words appear: some characters before, the words as
/// written, some after, each on one line with runs of white space made one space.
fn snippet(text: &str, needle: &Needle) -> Option<(String, String, String)> {
    let (start, end) = match_range(text, needle)?;
    let before: String = {
        let chars: Vec<char> = text[..start].chars().collect();
        let from = chars.len().saturating_sub(BEFORE);
        let cut: String = chars[from..].iter().collect();
        let cut = one_line(&cut);
        if from > 0 {
            format!("…{}", cut.trim_start())
        } else {
            cut.trim_start().to_string()
        }
    };
    let after: String = {
        let rest = &text[end..];
        let cut: String = rest.chars().take(AFTER).collect();
        let more = rest.chars().nth(AFTER).is_some();
        let cut = one_line(&cut);
        if more {
            format!("{}…", cut.trim_end())
        } else {
            cut.trim_end().to_string()
        }
    };
    Some((before, one_line(&text[start..end]), after))
}

/// The byte range of the first match of the words in `text`, on character boundaries.
fn match_range(text: &str, needle: &Needle) -> Option<(usize, usize)> {
    if !needle.unicode_case {
        let at = find_folded(text.as_bytes(), needle.lower.as_bytes())?;
        return Some((at, at + needle.lower.len()));
    }
    // Lower-casing may change lengths: walk the characters and compare what each start gives.
    let starts: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
    for &start in &starts {
        let mut lowered = String::new();
        for (offset, c) in text[start..].char_indices() {
            lowered.extend(c.to_lowercase());
            if lowered.len() >= needle.lower.len() {
                if lowered.starts_with(&needle.lower) {
                    return Some((start, start + offset + c.len_utf8()));
                }
                break;
            }
            if !needle.lower.starts_with(&lowered) {
                break;
            }
        }
    }
    None
}

fn one_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            space = true;
            continue;
        }
        if space {
            out.push(' ');
        }
        space = false;
        out.push(c);
    }
    if space {
        out.push(' ');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(lines: &[&str], words: &str) -> Option<TranscriptHit> {
        find_in_bytes(lines.join("\n").as_bytes(), &Needle::new(words).unwrap())
    }

    #[test]
    fn a_word_said_in_the_conversation_is_found_with_the_sentence_around_it() {
        let lines = [
            r#"{"type":"user","message":{"role":"user","content":"Please fix the Login timeout in the auth module"},"cwd":"/work/app"}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Done."}]}}"#,
        ];
        let hit = find(&lines, "login timeout").unwrap();
        assert_eq!(hit.role, HitRole::User);
        assert_eq!(hit.matched, "Login timeout");
        assert_eq!(hit.before, "Please fix the ");
        assert_eq!(hit.after, " in the auth module");
    }

    #[test]
    fn ids_folders_and_keys_are_not_what_was_searched_for() {
        let lines = [
            r#"{"type":"user","message":{"role":"user","content":"hello"},"cwd":"/work/payments","sessionId":"abc"}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"hi"}]}}"#,
        ];
        assert_eq!(find(&lines, "payments"), None);
        assert_eq!(find(&lines, "sessionid"), None);
        assert_eq!(find(&lines, "assistant"), None);
    }

    #[test]
    fn a_hit_only_in_a_key_on_one_line_keeps_looking_further_down() {
        let lines = [
            r#"{"type":"user","message":{"role":"user","content":"x"},"cwd":"/work/deploy"}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Run the deploy script"}]}}"#,
        ];
        let hit = find(&lines, "deploy").unwrap();
        assert_eq!(hit.role, HitRole::Assistant);
        assert_eq!(hit.after, " script");
    }

    #[test]
    fn korean_and_other_scripts_match_as_typed() {
        let lines = [r#"{"type":"user","message":{"role":"user","content":"작업공간 목록이 사라지는 문제를 고쳐줘"}}"#];
        let hit = find(&lines, "목록이 사라지는").unwrap();
        assert_eq!(hit.before, "작업공간 ");
        assert_eq!(hit.matched, "목록이 사라지는");
        assert_eq!(hit.after, " 문제를 고쳐줘");
    }

    #[test]
    fn letters_with_a_case_outside_ascii_still_ignore_case() {
        let lines = [r#"{"type":"user","message":{"role":"user","content":"Über das ÉTÉ"}}"#];
        assert_eq!(find(&lines, "été").unwrap().matched, "ÉTÉ");
        assert_eq!(find(&lines, "über").unwrap().matched, "Über");
    }

    #[test]
    fn quotes_and_line_breaks_inside_the_text_are_matched() {
        let lines = [r#"{"type":"user","message":{"role":"user","content":"say \"hi there\"\nthen   stop"}}"#];
        assert_eq!(find(&lines, "\"hi there\"").unwrap().matched, "\"hi there\"");
        let hit = find(&lines, "then").unwrap();
        assert_eq!(hit.before, "say \"hi there\" ");
    }

    #[test]
    fn a_folder_on_every_line_is_passed_over_and_the_same_line_still_counts() {
        let lines = [
            r#"{"cwd":"/work/worktrees/a","type":"user","message":{"role":"user","content":"x"}}"#,
            r#"{"cwd":"/work/worktrees/a","type":"user","message":{"role":"user","content":"list the worktrees"}}"#,
        ];
        let hit = find(&lines, "worktrees").unwrap();
        assert_eq!((hit.before.as_str(), hit.after.as_str()), ("list the ", ""));
        let line = lines[0].as_bytes();
        let at = lines[0].find("worktrees").unwrap();
        assert_eq!(bookkeeping(line, at), Some(lines[0].find("\",\"type").unwrap()));
        // A text value, and a key: only the key is bookkeeping.
        let text = r#"{"content":"say \"cwd\" here","cwd":"/x"}"#;
        assert_eq!(bookkeeping(text.as_bytes(), text.find("cwd").unwrap()), None);
        assert!(bookkeeping(text.as_bytes(), text.rfind("cwd").unwrap()).is_some());
    }

    #[test]
    fn words_someone_said_come_before_a_tool_output_holding_them() {
        let lines = [
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"ls: /work/deploy"}]}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Now deploy it"}]}}"#,
        ];
        assert_eq!(find(&lines, "deploy").unwrap().role, HitRole::Assistant);
        // Nobody said it: the tool's output is still a hit.
        assert_eq!(find(&lines[..1], "deploy").unwrap().role, HitRole::Other);
    }

    #[test]
    fn what_the_cli_adds_to_every_session_is_not_the_conversation() {
        let lines = [
            r#"{"type":"attachment","attachment":{"type":"skill_listing","content":"release: signed/notarized DMG"}}"#,
            r#"{"type":"user","isMeta":true,"message":{"role":"user","content":"<system-reminder>notarized</system-reminder>"}}"#,
            r#"{"type":"session_meta","payload":{"base_instructions":{"text":"notarized builds"}}}"#,
        ];
        assert_eq!(find(&lines, "notarized"), None);
        let said = r#"{"type":"user","message":{"role":"user","content":"is the build notarized?"}}"#;
        assert_eq!(find(&[lines[0], said], "notarized").unwrap().role, HitRole::User);
    }

    #[test]
    fn an_agent_calling_a_tool_with_the_words_did_not_say_them() {
        let call = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","name":"ToolSearch","input":{"query":"select:WebFetch"}}]}}"#;
        assert_eq!(find(&[call], "webfetch").unwrap().role, HitRole::Other);
    }

    #[test]
    fn a_tool_result_is_not_the_user_talking() {
        let lines = [
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"error: disk full"}]}}"#,
        ];
        assert_eq!(find(&lines, "disk full").unwrap().role, HitRole::Other);
    }

    #[test]
    fn codex_lines_say_who_spoke_in_their_payload() {
        let lines = [
            r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Migrated the schema"}]}}"#,
        ];
        assert_eq!(find(&lines, "schema").unwrap().role, HitRole::Assistant);
    }

    #[test]
    fn long_text_is_cut_around_the_words() {
        let long = format!("{}needle{}", "a ".repeat(100), "b ".repeat(100));
        let line = serde_json::json!({"type":"user","message":{"role":"user","content":long}}).to_string();
        let hit = find(&[&line], "needle").unwrap();
        assert!(hit.before.starts_with('…'));
        assert!(hit.after.ends_with('…'));
        assert!(hit.before.chars().count() <= BEFORE + 1);
    }

    #[test]
    fn plain_text_lines_are_searched_as_they_are() {
        let hit = find(&["not json at all", "the Needle is here"], "needle").unwrap();
        assert_eq!((hit.role, hit.matched.as_str()), (HitRole::Other, "Needle"));
    }

    #[test]
    fn files_are_searched_in_parallel_and_answer_in_order() {
        let dir = std::env::temp_dir().join(format!("agentty-search-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut paths = Vec::new();
        for i in 0..20 {
            let path = dir.join(format!("{i}.jsonl"));
            let text = if i % 3 == 0 { "find the marker here" } else { "nothing to see" };
            std::fs::write(&path, serde_json::json!({"type":"user","message":{"role":"user","content":text}}).to_string()).unwrap();
            paths.push(path);
        }
        paths.push(dir.join("missing.jsonl"));
        let results = search_transcripts(&paths, &Needle::new("MARKER").unwrap(), &AtomicBool::new(false));
        let found: Vec<usize> = results.iter().enumerate().filter(|(_, r)| r.is_some()).map(|(i, _)| i).collect();
        assert_eq!(found, vec![0, 3, 6, 9, 12, 15, 18]);
        assert!(search_transcripts(&paths, &Needle::new("marker").unwrap(), &AtomicBool::new(true)).iter().all(Option::is_none));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn blank_words_are_no_search() {
        assert!(Needle::new("   ").is_none());
    }
}
