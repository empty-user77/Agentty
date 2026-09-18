//! Detects agent harnesses: projects that declare one with a marker file (`.harness`,
//! `HARNESS.md`, …), an `agentty.json` `"harness"` list or a user pattern, so work can be started
//! through the project's commands and skills from the UI. Commands, skills or hooks alone
//! (`.claude/`, `.codex/`) are too common to count as a harness.
//!
//! Detection only looks at the project (never user-level configuration) and only follows the
//! patterns' own path segments, so it is cheap enough to run whenever a terminal changes folder.

use crate::extensions::{self, ExtensionKind};
use crate::fsutil;
use crate::model::Agent;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Built-in patterns, relative to the project root. `*` and `?` match within a name, `**` any
/// number of folders.
pub const DEFAULT_PATTERNS: &[&str] = &[".harness", "harness.json", "harness.yaml", "harness.yml", "HARNESS.md"];

/// Folders never searched by `**`.
const SKIPPED_DIRS: &[&str] = &[".git", "node_modules", "target", "build", "dist", ".venv", "vendor", "Pods", ".next"];
const MAX_DEPTH: usize = 6;
/// Folders above the terminal's folder that are checked (up to the repository root).
const MAX_PARENTS: usize = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// Declared in the project's `agentty.json` (`"harness": [...]`).
    Configured,
    Command,
    Skill,
}

/// A way to start work through the harness.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub kind: EntryKind,
    /// The agent it belongs to; `None` works with either.
    pub agent: Option<Agent>,
    pub name: String,
    pub description: String,
    /// Typed before the user's input (`/implement`), or a template with `{input}`.
    pub invocation: String,
    /// What the input should be (`argument-hint`, or `input` in `agentty.json`).
    pub hint: Option<String>,
}

impl Entry {
    /// The first message for the agent.
    pub fn prompt(&self, input: &str) -> String {
        let input = input.trim();
        if self.invocation.contains("{input}") {
            return self.invocation.replace("{input}", input).trim().to_string();
        }
        match (self.invocation.trim(), input) {
            ("", input) => input.to_string(),
            (invocation, "") => invocation.to_string(),
            (invocation, input) => format!("{invocation} {input}"),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Harness {
    /// Folder the harness belongs to (where the agent should start).
    pub root: PathBuf,
    /// Patterns that matched, for "why is this a harness".
    pub matched: Vec<String>,
    pub claude: bool,
    pub codex: bool,
    pub commands: usize,
    pub skills: usize,
    pub agents: usize,
    pub mcp: usize,
    pub hooks: usize,
    pub entries: Vec<Entry>,
}

impl Harness {
    /// The agent to start by default.
    pub fn preferred_agent(&self) -> Agent {
        if self.codex && !self.claude {
            Agent::Codex
        } else {
            Agent::Claude
        }
    }
}

/// The harness for a terminal in `dir`: `dir` itself or a parent up to the repository root.
pub fn detect(dir: &Path, extra_patterns: &[String]) -> Option<Harness> {
    let home = fsutil::home();
    let mut current = Some(dir);
    for _ in 0..=MAX_PARENTS {
        let candidate = current?;
        if candidate == home || candidate.parent().is_none() {
            return None;
        }
        if let Some(harness) = detect_at(candidate, extra_patterns) {
            return Some(harness);
        }
        if candidate.join(".git").exists() {
            return None;
        }
        current = candidate.parent();
    }
    None
}

/// Whether `root` itself holds a harness.
pub fn detect_at(root: &Path, extra_patterns: &[String]) -> Option<Harness> {
    let mut harness = Harness { root: root.to_path_buf(), ..Default::default() };
    for pattern in DEFAULT_PATTERNS.iter().map(|p| p.to_string()).chain(extra_patterns.iter().cloned()) {
        if matches(root, &pattern) && !harness.matched.contains(&pattern) {
            harness.matched.push(pattern);
        }
    }
    let configured = configured_entries(root);
    if !configured.is_empty() {
        harness.matched.push("agentty.json (harness)".into());
    }
    if harness.matched.is_empty() {
        return None;
    }

    let settings: Value =
        std::fs::read(root.join(".claude").join("settings.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    harness.hooks = settings["hooks"].as_object().map_or(0, |h| h.len());
    let mcp: Value = std::fs::read(root.join(".mcp.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    harness.mcp = mcp["mcpServers"].as_object().map_or(0, |m| m.len());
    let mut commands = Vec::new();
    let mut skills = Vec::new();
    for agent in [Agent::Claude, Agent::Codex] {
        for item in extensions::project_only(agent, root) {
            match agent {
                Agent::Codex => harness.codex = true,
                _ => harness.claude = true,
            }
            let entry = |kind| Entry {
                kind,
                agent: Some(agent),
                name: item.name.clone(),
                description: item.description.clone(),
                invocation: item.invocation.clone().unwrap_or_default(),
                hint: item.detail.clone().filter(|h| !h.trim().is_empty()),
            };
            match item.kind {
                ExtensionKind::Command => {
                    harness.commands += 1;
                    commands.push(entry(EntryKind::Command));
                }
                ExtensionKind::Skill => {
                    harness.skills += 1;
                    skills.push(entry(EntryKind::Skill));
                }
                ExtensionKind::Agent => harness.agents += 1,
                _ => {}
            }
        }
    }
    // Commands that take an argument are the usual entry points ("/implement <ticket>").
    commands.sort_by_key(|e| e.hint.is_none());
    harness.claude |= harness.hooks > 0 || harness.mcp > 0;
    harness.entries = configured;
    harness.entries.extend(commands);
    harness.entries.extend(skills);
    Some(harness)
}

/// `"harness": [{ "label", "command" | "prompt", "input", "agent", "description" }]` in `agentty.json`.
fn configured_entries(root: &Path) -> Vec<Entry> {
    let config: Value = std::fs::read(root.join("agentty.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let Some(items) = config["harness"].as_array() else { return Vec::new() };
    items
        .iter()
        .filter_map(|item| {
            let invocation = item["command"].as_str().or_else(|| item["prompt"].as_str())?.trim().to_string();
            let name = item["label"].as_str().map(str::to_string).unwrap_or_else(|| invocation.clone());
            let agent = match item["agent"].as_str() {
                Some("claude") => Some(Agent::Claude),
                Some("codex") => Some(Agent::Codex),
                _ => None,
            };
            Some(Entry {
                kind: EntryKind::Configured,
                agent,
                name,
                description: item["description"].as_str().unwrap_or_default().to_string(),
                invocation,
                hint: item["input"].as_str().map(str::to_string),
            })
        })
        .collect()
}

/// A pattern users can add: relative, without `..`.
pub fn is_valid_pattern(pattern: &str) -> bool {
    let pattern = normalize(pattern);
    !pattern.is_empty() && !pattern.starts_with('/') && !pattern.starts_with('~') && !pattern.split('/').any(|s| s == "..")
}

fn normalize(pattern: &str) -> &str {
    let pattern = pattern.trim();
    let pattern = pattern.strip_prefix("./").unwrap_or(pattern);
    pattern.trim_end_matches('/')
}

/// Whether anything under `root` matches `pattern`.
pub fn matches(root: &Path, pattern: &str) -> bool {
    if !is_valid_pattern(pattern) {
        return false;
    }
    let segments: Vec<&str> = normalize(pattern).split('/').filter(|s| !s.is_empty() && *s != ".").collect();
    exists(root, &segments, 0)
}

fn exists(dir: &Path, segments: &[&str], depth: usize) -> bool {
    let Some((segment, rest)) = segments.split_first() else { return true };
    if *segment == "**" {
        if exists(dir, rest, depth) {
            return true;
        }
        if depth >= MAX_DEPTH {
            return false;
        }
        return subdirs(dir).iter().any(|sub| exists(sub, segments, depth + 1));
    }
    if !segment.contains(['*', '?']) {
        let path = dir.join(segment);
        return if rest.is_empty() { path.exists() } else { path.is_dir() && exists(&path, rest, depth + 1) };
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return false };
    entries.flatten().any(|entry| {
        let name = entry.file_name().to_string_lossy().to_string();
        wildcard(segment, &name) && (rest.is_empty() || (entry.path().is_dir() && exists(&entry.path(), rest, depth + 1)))
    })
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|e| !SKIPPED_DIRS.contains(&e.file_name().to_string_lossy().as_ref()))
        .map(|e| e.path())
        .collect()
}

/// `*` and `?` within one name.
fn wildcard(pattern: &str, name: &str) -> bool {
    let (p, n): (Vec<char>, Vec<char>) = (pattern.chars().collect(), name.chars().collect());
    let (mut pi, mut ni, mut star, mut mark) = (0, 0, None, 0);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ni;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("agentty-harness-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        dir
    }

    fn write(path: PathBuf, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn wildcards_match_names() {
        assert!(wildcard("*.md", "implement.md"));
        assert!(wildcard("SKILL.md", "SKILL.md"));
        assert!(wildcard("a?c*", "abcdef"));
        assert!(!wildcard("*.md", "notes.txt"));
    }

    #[test]
    fn plain_project_is_not_a_harness() {
        let dir = project("plain");
        write(dir.join("CLAUDE.md"), "# Rules");
        write(dir.join("AGENTS.md"), "# Rules");
        assert_eq!(detect_at(&dir, &[]), None);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn agent_tooling_alone_is_not_a_harness() {
        let dir = project("tooling");
        write(dir.join(".claude/commands/review.md"), "Review");
        write(dir.join(".claude/skills/deploy/SKILL.md"), "---\nname: deploy\n---\n");
        write(dir.join(".claude/settings.json"), r#"{"hooks":{"PreToolUse":[]}}"#);
        write(dir.join(".codex/config.toml"), "");
        write(dir.join(".mcp.json"), r#"{"mcpServers":{}}"#);
        assert_eq!(detect_at(&dir, &[]), None);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn claude_commands_become_entry_points() {
        let dir = project("claude");
        write(dir.join("HARNESS.md"), "# Flow");
        write(dir.join(".claude/commands/review.md"), "---\ndescription: Review the diff\n---\nReview");
        write(
            dir.join(".claude/commands/jira/implement.md"),
            "---\ndescription: Implement a ticket\nargument-hint: <JIRA-KEY>\n---\nDo $ARGUMENTS",
        );
        write(dir.join(".claude/skills/deploy/SKILL.md"), "---\nname: deploy\ndescription: Ship it\n---\n");
        write(dir.join(".claude/agents/tester.md"), "---\nname: tester\n---\n");
        let harness = detect_at(&dir, &[]).unwrap();
        assert_eq!(harness.matched, vec!["HARNESS.md".to_string()]);
        assert!(harness.claude && !harness.codex);
        assert_eq!((harness.commands, harness.skills, harness.agents), (2, 1, 1));
        let first = &harness.entries[0];
        assert_eq!((first.invocation.as_str(), first.hint.as_deref()), ("/jira:implement", Some("<JIRA-KEY>")));
        assert_eq!(first.prompt(" PROJ-12 "), "/jira:implement PROJ-12");
        assert_eq!(harness.entries.last().unwrap().kind, EntryKind::Skill);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn custom_patterns_and_configured_entries() {
        let dir = project("custom");
        write(dir.join("ops/ai/flow.yaml"), "steps: []");
        assert_eq!(detect_at(&dir, &[]), None);
        let harness = detect_at(&dir, &["ops/**/*.yaml".to_string()]).unwrap();
        assert_eq!(harness.matched, vec!["ops/**/*.yaml".to_string()]);
        assert!(harness.entries.is_empty());

        write(
            dir.join("agentty.json"),
            r#"{"harness":[{"label":"Start a ticket","prompt":"Work on {input} following docs/flow.md","input":"Ticket key","agent":"codex"}]}"#,
        );
        let harness = detect_at(&dir, &[]).unwrap();
        let entry = &harness.entries[0];
        assert_eq!((entry.kind, entry.agent, entry.name.as_str()), (EntryKind::Configured, Some(Agent::Codex), "Start a ticket"));
        assert_eq!(entry.prompt("ABC-1"), "Work on ABC-1 following docs/flow.md");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn finds_the_harness_from_a_subfolder_but_not_past_the_repository() {
        let dir = project("nested");
        write(dir.join(".harness"), "");
        std::fs::create_dir_all(dir.join("src/deep")).unwrap();
        assert_eq!(detect(&dir.join("src/deep"), &[]).map(|h| h.root), Some(dir.clone()));
        let inner = dir.join("vendor-repo");
        std::fs::create_dir_all(inner.join(".git")).unwrap();
        assert_eq!(detect(&inner, &[]), None);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn rejects_patterns_outside_the_project() {
        assert!(is_valid_pattern(".harness"));
        assert!(is_valid_pattern("./tools/agent/"));
        assert!(!is_valid_pattern("../other"));
        assert!(!is_valid_pattern("/etc/passwd"));
        assert!(!is_valid_pattern("~/.claude"));
        assert!(!is_valid_pattern("  "));
    }

    #[test]
    fn claude_hooks_count() {
        let dir = project("hooks");
        write(dir.join("harness.yaml"), "steps: []");
        write(dir.join(".claude/settings.json"), r#"{"hooks":{"PreToolUse":[]}}"#);
        let harness = detect_at(&dir, &[]).unwrap();
        assert_eq!((harness.hooks, harness.claude), (1, true));
        std::fs::remove_dir_all(dir).ok();
    }
}
