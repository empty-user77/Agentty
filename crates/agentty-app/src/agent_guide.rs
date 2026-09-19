//! What agents started in Agentty are told about it: a short guide appended to their instructions
//! (Claude Code `--append-system-prompt-file`, Codex `developer_instructions`) and, for Claude Code,
//! a plugin with Agentty's skills (`--plugin-dir`). Everything lives in Agentty's data folder and is
//! passed on the command line: nothing is written into the user's projects or agent settings.
//!
//! The guide only describes commands that are safe to hand an agent: the in-app browser, pane
//! notifications and `agentty tasks` (which always waits for the user). It holds no paths, names or
//! secrets of the user.

use anyhow::Result;
use std::path::{Path, PathBuf};

/// Appended to the agent's instructions. English only (it is a prompt, not UI text).
pub const GUIDE: &str = "\
You are running inside Agentty, a terminal app for AI coding agents. Besides your usual tools you \
can use these Agentty commands from the shell of this pane:

- `agentty tasks` splits work into parallel sessions. When the user asks for several independent \
pieces of work at once (\"do A, B and C in parallel\", \"split this up\"), or when a request clearly \
falls into independent parts that would each take a while, hand the parts to new sessions instead \
of doing them one after another: write one self-contained prompt per part and run \
`agentty tasks --plan <plan.json>` (a JSON array of {\"title\", \"prompt\", \"agent\"}; up to 6) or \
`agentty tasks --title <title> --prompt-file <file>`. Agentty asks the user once; each started task \
runs in a split pane of this tab, in its own git worktree on a new branch from the project's default \
branch, and gets its prompt as the first message. The command prints each task's branch and folder. \
A task's prompt must stand on its own: goal, relevant files, constraints, how to verify, and whether \
to commit, push and open a pull request when done. Do not edit the other tasks' worktrees yourself.
- `agentty browser ...` drives Agentty's in-app browser (open a local dev server, read text, click, \
type, screenshots, console). `agentty browser --help` lists the commands.
- `agentty notify <message>` shows a notification for this pane when you finish something the user \
is waiting for.

Work that goes on its own branch belongs in its own git worktree from the project's default branch, \
unless the user says otherwise. Never stop Agentty processes by name (`pkill agentty`): other \
sessions depend on them. Talk to the user in the language they use.";

const SKILL_TASKS: &str = "\
---
name: agentty-parallel-tasks
description: Split a request into parallel Agentty sessions, each in its own git worktree and split pane. Use when the user asks for several independent pieces of work at once, asks to work in parallel or split work up, or a request clearly falls into independent parts.
---

# Parallel tasks in Agentty

1. Decide the parts. Each must be independent: no part waits for another, and two parts don't edit
   the same files where avoidable (shared registries such as translation tables are fine; say where
   to add entries so merges stay simple).
2. Write one prompt per part. It is the new session's first message and its only context, so include:
   the goal and why, the files and modules involved, constraints and project rules to follow, how to
   verify (tests, checks, running the app), and what to deliver (commit, push and open a pull request
   against the default branch, or just report back) - as the user asked.
3. Put them in a plan file and start them:

   ```sh
   cat > /tmp/agentty-plan.json <<'EOF'
   [
     {\"title\": \"Docker panel\", \"prompt\": \"...\", \"agent\": \"claude\"},
     {\"title\": \"Code editor\", \"prompt\": \"...\"}
   ]
   EOF
   agentty tasks --plan /tmp/agentty-plan.json
   ```

   `agent` is `claude` (default) or `codex`; at most 6 tasks. For a single task:
   `agentty tasks --title \"...\" --prompt-file prompt.md`.
4. Agentty asks the user once. On yes, every task starts in a split pane of the current tab, in a git
   worktree on a new branch from the project's default branch. The command prints each task's title,
   branch and folder; exit status 1 means the user declined or nothing could start.
5. Tell the user what started where. Keep working on your own part, if any; do not edit the other
   tasks' worktrees.
";

const PLUGIN_JSON: &str = r#"{
  "name": "agentty",
  "description": "Agentty: parallel tasks in split panes and worktrees, the in-app browser, notifications.",
  "version": "1.0.0"
}
"#;

/// `<data dir>/agent-guide`.
pub fn dir() -> PathBuf {
    agentty_bridge::fsutil::data_dir().join("agent-guide")
}

pub fn guide_file() -> PathBuf {
    dir().join("guide.md")
}

pub fn plugin_dir() -> PathBuf {
    dir().join("claude-plugin")
}

/// Writes the guide and the Claude Code plugin (at startup; cheap when unchanged).
pub fn write_files() -> Result<()> {
    write_in(&dir())
}

fn write_in(dir: &Path) -> Result<()> {
    let plugin = dir.join("claude-plugin");
    std::fs::create_dir_all(plugin.join(".claude-plugin"))?;
    std::fs::create_dir_all(plugin.join("skills").join("agentty-parallel-tasks"))?;
    write_if_changed(&dir.join("guide.md"), GUIDE)?;
    write_if_changed(&plugin.join(".claude-plugin").join("plugin.json"), PLUGIN_JSON)?;
    write_if_changed(&plugin.join("skills").join("agentty-parallel-tasks").join("SKILL.md"), SKILL_TASKS)?;
    Ok(())
}

fn write_if_changed(path: &Path, content: &str) -> Result<()> {
    if std::fs::read_to_string(path).ok().as_deref() != Some(content) {
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, content)?;
        std::fs::rename(tmp, path)?;
    }
    Ok(())
}

/// Whether agents get the guide (Settings → General), read from the settings file like the other
/// launch-time switches.
pub fn enabled() -> bool {
    crate::settings::agent_guide_enabled()
}

/// Claude Code options that hand it the guide and Agentty's skills, when they are on and in place.
pub fn claude_args() -> Vec<String> {
    let (guide, plugin) = (guide_file(), plugin_dir());
    if !enabled() || !guide.is_file() || !plugin.is_dir() {
        return Vec::new();
    }
    vec!["--append-system-prompt-file".into(), guide.display().to_string(), "--plugin-dir".into(), plugin.display().to_string()]
}

/// Codex `-c` override that adds the guide as developer instructions — unless the user set their own
/// `developer_instructions` (an override would replace them; theirs win).
pub fn codex_override() -> Option<String> {
    if !enabled() || user_codex_instructions() {
        return None;
    }
    Some(codex_value(GUIDE))
}

/// `developer_instructions="…"`: a TOML basic string (JSON string escapes are valid TOML ones).
fn codex_value(text: &str) -> String {
    format!("developer_instructions={}", serde_json::to_string(text).unwrap_or_default())
}

fn user_codex_instructions() -> bool {
    let home = std::env::var_os("CODEX_HOME").map(PathBuf::from).or_else(|| Some(crate::launch::home_dir().join(".codex")));
    home.and_then(|h| std::fs::read_to_string(h.join("config.toml")).ok()).is_some_and(|text| sets_instructions(&text))
}

/// A top-level `developer_instructions` key (before the first table).
fn sets_instructions(config: &str) -> bool {
    for line in config.lines().map(str::trim) {
        if line.starts_with('[') {
            return false;
        }
        if line.split('=').next().is_some_and(|key| key.trim() == "developer_instructions") {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_the_guide_and_a_plugin_with_the_skill() {
        let dir = std::env::temp_dir().join(format!("agentty-guide-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write_in(&dir).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("guide.md")).unwrap(), GUIDE);
        let manifest: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("claude-plugin/.claude-plugin/plugin.json")).unwrap()).unwrap();
        assert_eq!(manifest["name"], "agentty");
        let skill = std::fs::read_to_string(dir.join("claude-plugin/skills/agentty-parallel-tasks/SKILL.md")).unwrap();
        assert!(skill.starts_with("---\nname: agentty-parallel-tasks\ndescription: "));
        write_in(&dir).unwrap();
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn codex_gets_a_valid_toml_string_unless_the_user_has_instructions() {
        let value = codex_value("say \"hi\"\nand \\ bye");
        assert_eq!(value, r#"developer_instructions="say \"hi\"\nand \\ bye""#);
        assert!(sets_instructions("model = \"o3\"\ndeveloper_instructions = \"mine\"\n"));
        assert!(!sets_instructions("model = \"o3\"\n[profiles.x]\ndeveloper_instructions = \"only in a profile\"\n"));
        assert!(!sets_instructions("# developer_instructions = \"commented\"\n"));
    }

    /// The guide is a prompt shipped in code: English only, and nothing personal in it.
    #[test]
    fn the_guide_is_plain_english() {
        for text in [GUIDE, SKILL_TASKS, PLUGIN_JSON] {
            assert!(text.is_ascii(), "prompts are English");
            assert!(!text.contains("/Users/") && !text.contains("/home/"));
        }
    }
}
