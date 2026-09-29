//! "Resolve with AI": a merge or pull that stopped on conflicts (or a repository left mid-merge)
//! opens a terminal tab in the repository with an agent told what happened and what to do. The
//! agent is the first one installed of Claude Code, Codex, then the others in the launcher's order.

use super::Workbench;
use crate::i18n::{t, tf};
use crate::launch::LaunchSpec;
use agentty_bridge::model::Agent;
use gpui::{AppContext, Context, Window};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// What went wrong in the repository, as the agent is told about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitTrouble {
    /// Merging `branch` into the current branch stopped on conflicts (and was undone).
    Merge { branch: String },
    /// `git pull --ff-only` refused: the branch and its upstream have diverged.
    Pull,
    /// The repository is in the middle of a merge, rebase, … with unresolved conflicts.
    InProgress,
}

/// The first message for the agent. Prompts are English whatever the app's language; the last
/// line says which language to talk in. Reads the repository (git runs): call it off the main thread.
pub fn prompt(repo: &Path, trouble: &GitTrouble, language: &str) -> String {
    let current = agentty_bridge::git::head(repo).and_then(|(branch, _)| branch).unwrap_or_else(|| "HEAD".into());
    let careful = "Keep what both sides meant; when a conflict needs a decision only I can make, ask me instead of guessing. \
                   Afterwards make sure the project still builds and its tests pass.";
    let body = match trouble {
        GitTrouble::Merge { branch } => format!(
            "Merging `{branch}` into `{current}` in this repository stops on conflicts (Agentty undid that attempt). \
             Run `git merge {branch}`, resolve every conflict, then commit the merge. {careful} Don't push."
        ),
        GitTrouble::Pull => format!(
            "`git pull` can't fast-forward `{current}`: it and its upstream have diverged. \
             Bring the upstream changes in with `git pull --no-rebase`, resolve any conflicts, then commit the merge. {careful} \
             Don't push without asking me."
        ),
        GitTrouble::InProgress => {
            let operation = agentty_bridge::git::operation_in_progress(repo).unwrap_or("merge");
            let files = agentty_bridge::git::conflicted_files(repo);
            let list = if files.is_empty() { "(git lists none — check `git status`)".to_string() } else { files.join(", ") };
            format!(
                "This repository is in the middle of a {operation} on `{current}` with unresolved conflicts in: {list}. \
                 Resolve them, stage the files and finish with `git {operation} --continue`. {careful} Don't push."
            )
        }
    };
    format!("{body}\n\nTalk to me in {language}.")
}

/// Which agent resolves: the first installed of Claude Code, Codex, then the other agents in the
/// launcher's order.
#[derive(Debug, PartialEq, Eq)]
enum Resolver {
    Claude,
    Codex,
    Other(&'static str),
    None,
}

/// `installed` is `None` while it is still being detected (just after start): Claude Code, the likeliest.
fn pick_resolver(installed: Option<&crate::agents::Installed>) -> Resolver {
    let Some(installed) = installed else { return Resolver::Claude };
    if installed.has("claude") {
        Resolver::Claude
    } else if installed.has("codex") {
        Resolver::Codex
    } else {
        installed.other_agents().next().map_or(Resolver::None, |agent| Resolver::Other(agent.binary))
    }
}

impl Workbench {
    /// Opens a terminal tab in `repo` with the preferred agent, told how to resolve `trouble`.
    pub(super) fn resolve_with_ai(&mut self, repo: PathBuf, trouble: GitTrouble, window: &mut Window, cx: &mut Context<Self>) {
        let language = agentty_bridge::idea::language_name(crate::settings::settings(cx).language.code());
        let source = repo.clone();
        let task = cx.background_spawn(async move { prompt(&source, &trouble, language) });
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let prompt = task.await;
            let _ = cx.update_window(handle, |_, window, cx| {
                let _ = this.update(cx, |this, cx| this.start_resolver(repo, prompt, window, cx));
            });
        })
        .detach();
    }

    /// Claude Code, else Codex (both take the prompt as their first message), else the first other
    /// agent installed (started, then the prompt is typed into it), else a terminal and a note.
    fn start_resolver(&mut self, repo: PathBuf, prompt: String, window: &mut Window, cx: &mut Context<Self>) {
        let title = t(cx, "git.ai_resolve_title").to_string();
        self.page = None;
        // Straight into a tab of the repository, never the "a new worktree?" question: the
        // conflicts are in this folder, and only here can they be resolved.
        match pick_resolver(self.installed.as_ref()) {
            Resolver::Claude => self.open_tab(LaunchSpec::with_prompt(Agent::Claude, prompt, title, repo), window, cx),
            Resolver::Codex => self.open_tab(LaunchSpec::with_prompt(Agent::Codex, prompt, title, repo), window, cx),
            Resolver::Other(binary) => {
                self.open_tab(LaunchSpec::shell_command(binary.to_string(), title, repo), window, cx);
                let Some(pane) = self.active_pane() else { return };
                cx.spawn(async move |_, cx| {
                    // Give the agent's TUI time to start before typing, as for other typed prompts.
                    cx.background_executor().timer(Duration::from_secs(4)).await;
                    let _ = pane.update(cx, |view, cx| view.submit_prompt(prompt, cx));
                })
                .detach();
            }
            Resolver::None => {
                self.open_tab(LaunchSpec::new(crate::launch::PaneKind::Shell, repo), window, cx);
                self.show_toast(t(cx, "git.ai_none").to_string(), cx);
            }
        }
    }
}

/// Button label and the banner text that goes with a trouble.
pub fn trouble_text(trouble: &GitTrouble, cx: &gpui::App) -> String {
    match trouble {
        GitTrouble::Merge { branch } => tf(cx, "git.merge_conflict", &[("branch", branch)]),
        GitTrouble::Pull => t(cx, "git.pull_diverged").to_string(),
        GitTrouble::InProgress => t(cx, "git.conflicts_open").to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_then_codex_then_the_others() {
        let with = |binaries: &[&str]| crate::agents::Installed {
            binaries: binaries.iter().map(|b| b.to_string()).collect(),
            ..Default::default()
        };
        assert_eq!(pick_resolver(Some(&with(&["codex", "claude", "gemini"]))), Resolver::Claude);
        assert_eq!(pick_resolver(Some(&with(&["codex", "gemini"]))), Resolver::Codex);
        assert_eq!(pick_resolver(Some(&with(&["amp", "gemini"]))), Resolver::Other("gemini"));
        assert_eq!(pick_resolver(Some(&with(&["amp"]))), Resolver::Other("amp"));
        assert_eq!(pick_resolver(Some(&with(&[]))), Resolver::None);
        assert_eq!(pick_resolver(None), Resolver::Claude);
    }

    #[test]
    fn prompts_say_what_to_do_and_in_which_language() {
        let dir = std::env::temp_dir().join(format!("agentty-ai-resolve-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let merge = prompt(&dir, &GitTrouble::Merge { branch: "feature/x".into() }, "Korean");
        assert!(merge.contains("git merge feature/x") && merge.contains("Don't push"), "{merge}");
        assert!(merge.ends_with("Talk to me in Korean."));
        let pull = prompt(&dir, &GitTrouble::Pull, "English");
        assert!(pull.contains("git pull --no-rebase"), "{pull}");
        let open = prompt(&dir, &GitTrouble::InProgress, "English");
        assert!(open.contains("--continue"), "{open}");
        std::fs::remove_dir_all(dir).ok();
    }
}
