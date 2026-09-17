//! Converts a transcript from one agent into a markdown handoff another agent can continue from.

use crate::fsutil;
use crate::model::{Agent, Role, Turn};
use anyhow::Result;
use serde::Serialize;
use std::fmt::Write as _;
use std::path::PathBuf;

/// Upper bound for the conversation section so the receiving agent can read it in one pass.
const CONVERSATION_BUDGET: usize = 120_000;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Handoff {
    pub path: PathBuf,
    pub cwd: Option<String>,
    pub prompt: String,
    pub args: Vec<String>,
    pub turn_count: usize,
}

pub fn create(from: Agent, id: &str, to: Agent, cwd: Option<String>, turns: &[Turn]) -> Result<Handoff> {
    let path = write_document(from, id, to, cwd.as_deref(), turns)?;

    let prompt = format!(
        "This task continues a previous {} session. Read the handoff file {} to recover the context, \
         briefly summarize where things stand, then continue the work.",
        from.display_name(),
        path.display()
    );
    Ok(Handoff { args: to.prompt_args(&prompt), path, cwd, prompt, turn_count: turns.len() })
}

/// Shares a session's context with an already running agent session (flow connections).
/// The returned prompt is meant to be submitted into the target session.
pub fn create_share(from: Agent, id: &str, title: &str, to: Agent, cwd: Option<String>, turns: &[Turn]) -> Result<Handoff> {
    let path = write_document(from, id, to, cwd.as_deref(), turns)?;
    let prompt = format!(
        "Context shared from another {} session (\"{title}\"). Read {} and use it as background for the \
         current work. Reply with a short summary of what you learned, then wait for my next instruction.",
        from.display_name(),
        path.display()
    );
    Ok(Handoff { args: Vec::new(), path, cwd, prompt, turn_count: turns.len() })
}

/// Live link update: only the turns added since the last share (`turns` is already that slice).
pub fn create_update(from: Agent, id: &str, title: &str, to: Agent, cwd: Option<String>, turns: &[Turn]) -> Result<Handoff> {
    anyhow::ensure!(!turns.is_empty(), "no new turns to share");
    let dir = fsutil::data_dir().join("handoffs");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}-{id}-to-{}-update.md", agent_slug(from), agent_slug(to)));
    std::fs::write(&path, render(from, id, to, cwd.as_deref(), turns))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    let prompt = update_prompt(from, title, turns.len(), &path);
    Ok(Handoff { args: Vec::new(), path, cwd, prompt, turn_count: turns.len() })
}

fn update_prompt(from: Agent, title: &str, count: usize, path: &std::path::Path) -> String {
    format!(
        "Update from the linked {} session (\"{title}\"): {count} new turn(s) in {}. Read it and take it into account. \
         Reply in one or two sentences with anything that changes your plan, then wait for my next instruction.",
        from.display_name(),
        path.display()
    )
}

fn write_document(from: Agent, id: &str, to: Agent, cwd: Option<&str>, turns: &[Turn]) -> Result<PathBuf> {
    let dir = fsutil::data_dir().join("handoffs");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}-{id}-to-{}.md", agent_slug(from), agent_slug(to)));
    std::fs::write(&path, render(from, id, to, cwd, turns))?;
    // Handoffs contain conversation text; keep them private to the user.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(path)
}

fn agent_slug(agent: Agent) -> &'static str {
    match agent {
        Agent::Claude => "claude",
        other => other.id(),
    }
}

fn render(from: Agent, id: &str, to: Agent, cwd: Option<&str>, turns: &[Turn]) -> String {
    let mut doc = String::new();
    let _ = writeln!(doc, "# Agentty session handoff\n");
    let _ = writeln!(doc, "- Source: {} session `{id}`", from.display_name());
    let _ = writeln!(doc, "- Target: {}", to.display_name());
    if let Some(cwd) = cwd {
        let _ = writeln!(doc, "- Working directory: `{cwd}`");
    }
    let _ = writeln!(
        doc,
        "\n## Instructions\n\nThis is the conversation from the source session. `[tool: ...]` lines are \
         abbreviated tool calls; their results are not included, so re-check files before relying on them. \
         The most recent turns are at the bottom.\n\n## Conversation\n"
    );

    let sections: Vec<String> = turns
        .iter()
        .map(|t| {
            let who = match t.role {
                Role::User => "User",
                Role::Assistant => from.display_name(),
            };
            format!("### {who}\n\n{}\n\n", t.text)
        })
        .collect();

    // Keep the opening request plus as many recent turns as fit the budget.
    let mut tail = Vec::new();
    let mut used = sections.first().map_or(0, String::len);
    for section in sections.iter().skip(1).rev() {
        if used + section.len() > CONVERSATION_BUDGET {
            break;
        }
        used += section.len();
        tail.push(section.as_str());
    }
    if let Some(first) = sections.first() {
        doc.push_str(first);
    }
    let omitted = sections.len().saturating_sub(1 + tail.len());
    if omitted > 0 {
        let _ = writeln!(doc, "_… {omitted} earlier turns omitted …_\n");
    }
    tail.iter().rev().for_each(|s| doc.push_str(s));
    doc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_prompt_names_source_and_count() {
        let prompt = update_prompt(Agent::Codex, "Fix login", 3, std::path::Path::new("/tmp/x.md"));
        assert!(
            prompt.contains("Codex")
                && prompt.contains("\"Fix login\"")
                && prompt.contains("3 new turn(s)")
                && prompt.contains("/tmp/x.md")
        );
    }
}
