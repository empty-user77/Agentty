//! `agentty worktree-guard`: a Claude Code hook (PreToolUse, on file edits) that keeps an agent
//! working inside the git worktree it edits.
//!
//! An agent started in one checkout that edits another worktree of the same repository by its
//! path works on a branch its session is not on: Agentty shows the first checkout and its branch
//! for that pane, git commands run in the wrong tree, and the edits look like they happen
//! somewhere else. The hook refuses such an edit (exit code 2, which Claude Code shows to the agent)
//! and says how to switch the session into that worktree first. Edits anywhere else — outside git,
//! in another repository, in the session's own tree — pass untouched.

use std::io::Read;
use std::path::{Path, PathBuf};

/// The checkout `path` is in: the nearest folder above it holding `.git` (a folder for the main
/// checkout, a file for a linked worktree).
pub fn tree_root(path: &Path) -> Option<PathBuf> {
    path.ancestors().find(|dir| dir.join(".git").exists()).map(Path::to_path_buf)
}

/// The repository a checkout belongs to: its shared `.git` folder, the same for every worktree.
pub fn common_dir(root: &Path) -> Option<PathBuf> {
    let git = root.join(".git");
    if git.is_dir() {
        return git.canonicalize().ok();
    }
    // A linked worktree: `.git` is a file naming `<repo>/.git/worktrees/<name>`, whose
    // `commondir` points back at the shared folder (relative to it).
    let text = std::fs::read_to_string(&git).ok()?;
    let gitdir = PathBuf::from(text.strip_prefix("gitdir:")?.trim());
    let gitdir = if gitdir.is_absolute() { gitdir } else { root.join(gitdir) };
    let common = std::fs::read_to_string(gitdir.join("commondir")).ok().map(|c| PathBuf::from(c.trim()));
    let common = match common {
        Some(c) if c.is_absolute() => c,
        Some(c) => gitdir.join(c),
        None => gitdir.parent()?.parent()?.to_path_buf(),
    };
    common.canonicalize().ok()
}

/// The branch checked out in `root`, for the message.
fn branch(root: &Path) -> Option<String> {
    crate::procinfo::git_branch(root)
}

/// Why an edit of `file` from a session in `cwd` must wait for the session to move, if it must.
pub fn check(cwd: &Path, file: &Path) -> Option<String> {
    let file = if file.is_absolute() { file.to_path_buf() } else { cwd.join(file) };
    let here = tree_root(cwd)?;
    // The file may not exist yet: its folder (or the nearest one that does) says where it goes.
    let existing = file.ancestors().find(|p| p.exists())?;
    let there = tree_root(existing)?;
    let same = |a: &Path, b: &Path| a.canonicalize().ok() == b.canonicalize().ok();
    if same(&here, &there) {
        return None;
    }
    // Another repository altogether (a nested one, a dependency): not this rule's business.
    if common_dir(&here)? != common_dir(&there)? {
        return None;
    }
    let branch = branch(&there).map(|b| format!(" (branch {b})")).unwrap_or_default();
    Some(format!(
        "{} is in the git worktree {}{branch}, but this session works in {}. Switch the session into that \
         worktree before editing it — Claude Code: EnterWorktree with path {}; otherwise `cd` there and keep \
         working from it — then make this edit again. Agentty shows the pane's folder and branch from where the \
         session is, so work done in another worktree by its path looks like it happens on the wrong branch.",
        file.display(),
        there.display(),
        here.display(),
        there.display(),
    ))
}

/// Reads the hook's JSON from stdin: `cwd` and the tool input's `file_path` / `notebook_path`.
pub fn run() -> i32 {
    let mut input = String::new();
    if std::io::stdin().take(4 * 1024 * 1024).read_to_string(&mut input).is_err() {
        return 0;
    }
    let Ok(event) = serde_json::from_str::<serde_json::Value>(&input) else { return 0 };
    let Some(cwd) = event["cwd"].as_str().map(PathBuf::from) else { return 0 };
    let tool = &event["tool_input"];
    let Some(file) = tool["file_path"].as_str().or_else(|| tool["notebook_path"].as_str()) else { return 0 };
    match check(&cwd, Path::new(file)) {
        Some(reason) => {
            // Exit code 2: Claude Code blocks the tool call and hands this to the agent.
            eprintln!("{reason}");
            2
        }
        None => 0,
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    #[test]
    fn an_edit_in_another_worktree_of_the_same_repository_waits_for_the_session_to_move() {
        let base = std::env::temp_dir().join(format!("agentty-guard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let main = base.join("main");
        let other = base.join("other");
        let elsewhere = base.join("elsewhere");
        std::fs::create_dir_all(&main).unwrap();
        std::fs::create_dir_all(&elsewhere).unwrap();
        git(&main, &["init", "-q", "-b", "main"]);
        std::fs::write(main.join("a.txt"), "a").unwrap();
        git(&main, &["add", "."]);
        git(&main, &["commit", "-q", "-m", "a"]);
        git(&main, &["worktree", "add", "-q", "-b", "feature", other.to_str().unwrap()]);
        git(&elsewhere, &["init", "-q"]);

        // Its own tree, a new file in it, another repository, no repository at all: allowed.
        assert_eq!(check(&main, &main.join("a.txt")), None);
        assert_eq!(check(&main, &main.join("src/new.rs")), None);
        assert_eq!(check(&main, &elsewhere.join("x.txt")), None);
        assert_eq!(check(&main, Path::new("/tmp/agentty-guard-free.txt")), None);
        // The same repository's other worktree, from either side: refused, and it says where to go.
        let refusal = check(&main, &other.join("a.txt")).expect("refused");
        assert!(refusal.contains("EnterWorktree"));
        assert!(refusal.contains(other.to_str().unwrap()));
        assert!(check(&other, &main.join("a.txt")).is_some());
        // Once the session is in that worktree, the edit goes through.
        assert_eq!(check(&other, &other.join("a.txt")), None);
        let _ = std::fs::remove_dir_all(&base);
    }
}
