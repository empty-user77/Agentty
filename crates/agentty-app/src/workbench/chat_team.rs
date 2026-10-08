//! Who does what in a chat: which agent its workers and reviewers run as, what a reviewer is asked
//! to do, and what the lead is told about the branch it works on. Plain data and text, kept apart
//! from the chat's view so it can be tested without a window.

use agentty_bridge::model::Agent;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Title a reviewer of worker "X" runs under: "Review: X".
pub const REVIEW_PREFIX: &str = "Review: ";

/// The agent a role runs as.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RoleAgent {
    #[default]
    Claude,
    Codex,
}

impl RoleAgent {
    pub fn agent(self) -> Agent {
        match self {
            RoleAgent::Claude => Agent::Claude,
            RoleAgent::Codex => Agent::Codex,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            RoleAgent::Claude => "Claude",
            RoleAgent::Codex => "Codex",
        }
    }

    /// `claude` / `codex`, as `agentty tasks` spells it.
    pub fn label(self) -> &'static str {
        match self {
            RoleAgent::Claude => "claude",
            RoleAgent::Codex => "codex",
        }
    }

    pub fn from_label(label: &str) -> Option<RoleAgent> {
        match label {
            "claude" => Some(RoleAgent::Claude),
            "codex" => Some(RoleAgent::Codex),
            _ => None,
        }
    }
}

/// A chat's choices, saved with its tab: the agent new workers run as when the lead names none, and
/// the one reviewers run as (`None`: no reviews).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatRoles {
    #[serde(default)]
    pub worker: RoleAgent,
    #[serde(default)]
    pub reviewer: Option<RoleAgent>,
}

impl ChatRoles {
    /// The next worker choice (Claude ↔ Codex; Claude only while Codex is not installed).
    pub fn next_worker(self, codex: bool) -> ChatRoles {
        let worker = match self.worker {
            RoleAgent::Claude if codex => RoleAgent::Codex,
            _ => RoleAgent::Claude,
        };
        ChatRoles { worker, ..self }
    }

    /// The next reviewer choice: off → Claude → Codex → off (Codex left out while not installed).
    pub fn next_reviewer(self, codex: bool) -> ChatRoles {
        let reviewer = match self.reviewer {
            None => Some(RoleAgent::Claude),
            Some(RoleAgent::Claude) if codex => Some(RoleAgent::Codex),
            Some(_) => None,
        };
        ChatRoles { reviewer, ..self }
    }

    /// What every worker report reminds the lead of while reviews are on.
    pub fn review_hint(self, worker: &str) -> Option<String> {
        let reviewer = self.reviewer?;
        Some(format!(
            "Reviews are on in this chat (reviewer: {}). Before merging this worker's branch, start one with \
             `agentty tasks review --worker \"{worker}\"` (add `--prompt` to say what to look at), unless the change is \
             trivial; its findings come back as another report.",
            reviewer.name()
        ))
    }
}

/// The first message of a reviewer of `worker`, which works on `branch` (when known) in the folder
/// the reviewer starts in, branched from `base`.
pub fn review_prompt(worker: &str, branch: Option<&str>, base: &str, focus: &str) -> String {
    let what = match branch {
        Some(branch) => format!("the work of \"{worker}\" on branch `{branch}`"),
        None => format!("the work of \"{worker}\""),
    };
    let mut prompt = format!(
        "Review {what} in this folder. Compare it with `{base}` (`git diff {base}...HEAD`, `git log {base}..HEAD`) and look \
         at uncommitted changes too (`git status`, `git diff`).\n\n\
         Do not edit, create or delete files, and do not commit: you are the reviewer. Run the project's tests or build \
         when that helps you judge the work; your sandbox may refuse commands that write files — then say which checks \
         you could not run.\n\n\
         Report, most important first:\n\
         1. Blocking problems: bugs, missing or failing tests, security issues, broken behavior — each with `file:line` \
         and why.\n\
         2. Smaller issues worth fixing.\n\
         3. A verdict on its own last line: `Verdict: approve` or `Verdict: changes needed`."
    );
    if !focus.trim().is_empty() {
        prompt.push_str(&format!("\n\nThe lead asks you to look at this in particular:\n{}", focus.trim()));
    }
    prompt
}

/// What the lead is told about where it works: on the chat's integration branch in a worktree of
/// its own (`integration`, with the project's own folder), or in the project folder itself.
pub fn folder_brief(integration: Option<(&str, &Path)>) -> String {
    match integration {
        Some((branch, project)) => format!(
            "Your folder is a git worktree on branch `{branch}`: this chat's integration branch. Workers start from its \
             latest commit, so merge each worker's branch into it here once you have reviewed the work, before starting \
             workers that build on it. That merge needs no permission from the user. The project's own folder is `{}`: \
             bring the integration branch there (a pull request when the repository has a remote, or a merge in that \
             folder when it has no uncommitted changes) only when the user asks for it.",
            project.display()
        ),
        None => "Your folder is the project folder itself. Workers start from the project's default branch. Merge a \
                 worker's branch only when the user asks for it or approved a plan that includes merging."
            .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_cycle_and_skip_codex_when_it_is_missing() {
        let roles = ChatRoles::default();
        assert_eq!(roles.worker, RoleAgent::Claude);
        assert_eq!(roles.reviewer, None);
        assert_eq!(roles.next_worker(true).worker, RoleAgent::Codex);
        assert_eq!(roles.next_worker(true).next_worker(true).worker, RoleAgent::Claude);
        assert_eq!(roles.next_worker(false).worker, RoleAgent::Claude);
        let on = roles.next_reviewer(true);
        assert_eq!(on.reviewer, Some(RoleAgent::Claude));
        assert_eq!(on.next_reviewer(true).reviewer, Some(RoleAgent::Codex));
        assert_eq!(on.next_reviewer(true).next_reviewer(true).reviewer, None);
        assert_eq!(on.next_reviewer(false).reviewer, None);
    }

    #[test]
    fn roles_survive_the_layout_file() {
        let roles = ChatRoles { worker: RoleAgent::Codex, reviewer: Some(RoleAgent::Claude) };
        let back: ChatRoles = serde_json::from_str(&serde_json::to_string(&roles).unwrap()).unwrap();
        assert_eq!(back, roles);
        assert_eq!(serde_json::from_str::<ChatRoles>("{}").unwrap(), ChatRoles::default());
    }

    #[test]
    fn reports_remind_of_reviews_only_while_they_are_on() {
        assert_eq!(ChatRoles::default().review_hint("API"), None);
        let hint = ChatRoles { reviewer: Some(RoleAgent::Codex), ..Default::default() }.review_hint("API").unwrap();
        assert!(hint.contains("agentty tasks review --worker \"API\"") && hint.contains("Codex"));
    }

    #[test]
    fn a_reviewer_is_told_what_to_compare_and_not_to_edit() {
        let prompt = review_prompt("API", Some("agentty/api-1"), "agentty/chat-1", "the auth checks");
        assert!(prompt.contains("`agentty/api-1`") && prompt.contains("git diff agentty/chat-1...HEAD"));
        assert!(prompt.contains("Do not edit") && prompt.contains("Verdict: approve"));
        assert!(prompt.ends_with("the auth checks"));
        assert!(!review_prompt("API", None, "main", " ").contains("in particular"));
    }

    #[test]
    fn the_lead_hears_where_it_works() {
        let integration = folder_brief(Some(("agentty/chat-1", Path::new("/work/app"))));
        assert!(integration.contains("`agentty/chat-1`") && integration.contains("`/work/app`"));
        assert!(folder_brief(None).contains("project folder itself"));
    }
}
