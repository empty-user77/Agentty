//! Git worktrees, so agent sessions working on the same project don't edit the same files.
//!
//! When a second agent session starts in a working tree another session already uses, Agentty gives
//! it a working tree of its own: `git worktree add` on a new branch `agentty/<name>`, from the
//! project's default branch (see [`base_ref`]), not from whatever branch the project folder has
//! checked out, so a session never builds on another session's unmerged work. The trees live in Agentty's data folder (not inside the project, where
//! every search, watcher and build would walk into a second copy of the code):
//!
//! ```text
//! <data dir>/worktrees/<project>-<hash of its path>/<name>/
//! ```
//!
//! Only trees under that folder are ever removed by Agentty; worktrees the user made are listed
//! and left alone.

use anyhow::{bail, ensure, Context, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Stdio;

/// Branches Agentty creates start with this.
pub const BRANCH_PREFIX: &str = "agentty/";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    /// `None` while detached.
    pub branch: Option<String>,
    pub head: String,
    /// The repository's main working tree (where `.git` is a folder).
    pub main: bool,
    /// Created by Agentty (it lives in Agentty's worktree folder).
    pub managed: bool,
    /// Its folder is gone; `git worktree prune` would forget it.
    pub prunable: bool,
}

impl Worktree {
    /// Short name for lists: the folder name.
    pub fn name(&self) -> String {
        self.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
    }
}

pub(crate) fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let output = crate::process::command("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .output()
        .context("git is not installed")?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!("{}", if stderr.is_empty() { format!("git {} failed", args.first().unwrap_or(&"")) } else { stderr })
    }
}

/// The working tree `path` is in: the closest folder with a `.git` entry (a folder in the main
/// tree, a file in a linked one). No git process: cheap enough to ask on every render.
pub fn tree_root(path: &Path) -> Option<PathBuf> {
    path.ancestors().find(|dir| dir.join(".git").exists()).map(Path::to_path_buf)
}

/// Where Agentty keeps the worktrees it creates.
pub fn managed_dir() -> PathBuf {
    crate::fsutil::data_dir().join("worktrees")
}

/// Whether `path` really is inside Agentty's worktree folder. Both sides are resolved, so neither a
/// symlink nor `..` can make a folder elsewhere pass; a folder that is gone is nobody's.
fn is_managed(path: &Path, managed: &Path) -> bool {
    match (path.canonicalize(), managed.canonicalize()) {
        (Ok(path), Ok(managed)) => path.starts_with(managed),
        _ => false,
    }
}

/// Folder (under `managed`) for the worktrees of the repository whose main tree is `main_root`.
fn home_for(main_root: &Path, managed: &Path) -> PathBuf {
    let canonical = main_root.canonicalize().unwrap_or_else(|_| main_root.to_path_buf());
    let digest = Sha256::digest(canonical.to_string_lossy().as_bytes());
    let hash: String = digest.iter().take(4).map(|b| format!("{b:02x}")).collect();
    let name: String = canonical
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "project".into())
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '-' })
        .collect();
    managed.join(format!("{}-{hash}", name.trim_matches('.')))
}

/// Parses `git worktree list --porcelain`. The first entry is the main working tree.
pub fn parse_list(raw: &str) -> Vec<Worktree> {
    let mut trees: Vec<Worktree> = Vec::new();
    for line in raw.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            let main = trees.is_empty();
            trees.push(Worktree { path: PathBuf::from(path), branch: None, head: String::new(), main, managed: false, prunable: false });
        } else if let Some(tree) = trees.last_mut() {
            if let Some(head) = line.strip_prefix("HEAD ") {
                tree.head = head.to_string();
            } else if let Some(branch) = line.strip_prefix("branch ") {
                tree.branch = Some(branch.strip_prefix("refs/heads/").unwrap_or(branch).to_string());
            } else if line == "prunable" || line.starts_with("prunable ") {
                tree.prunable = true;
            }
        }
    }
    // A bare repository lists itself first; it is not a tree anyone works in.
    trees.retain(|tree| !tree.head.is_empty());
    trees
}

/// Every working tree of the repository `path` is in, the main one first.
pub fn list(path: &Path) -> Result<Vec<Worktree>> {
    list_in(path, &managed_dir())
}

fn list_in(path: &Path, managed: &Path) -> Result<Vec<Worktree>> {
    let mut trees = parse_list(&git(path, &["worktree", "list", "--porcelain"])?);
    for tree in &mut trees {
        tree.managed = !tree.main && is_managed(&tree.path, managed);
    }
    Ok(trees)
}

/// `claude` → `claude-0918-1432`; letters, digits and dashes only, whatever the label was.
fn tree_name(label: &str, stamp: &str, taken: impl Fn(&str) -> bool) -> String {
    let label: String = label.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(24).collect::<String>().to_lowercase();
    let base = format!("{}-{stamp}", if label.is_empty() { "session" } else { &label });
    (1..).map(|n| if n == 1 { base.clone() } else { format!("{base}-{n}") }).find(|name| !taken(name)).unwrap_or(base)
}

/// Where a new session tree starts: the repository's default branch — the local branch when there
/// is one (it has the user's own commits), else the remote's — and the checked-out commit only in a
/// repository without either. The default branch is what `origin/HEAD` names, else `main` or
/// `master`. Nothing is fetched.
pub fn base_ref(path: &Path) -> String {
    let exists = |reference: &str| git(path, &["rev-parse", "--verify", "--quiet", &format!("{reference}^{{commit}}")]).is_ok();
    let remote_default = git(path, &["symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"]).ok();
    pick_base(remote_default.as_deref().map(str::trim), exists)
}

/// [`base_ref`] without git: `remote_default` is `origin/HEAD`'s short name (`origin/main`).
fn pick_base(remote_default: Option<&str>, exists: impl Fn(&str) -> bool) -> String {
    let named = remote_default.and_then(|r| r.strip_prefix("origin/")).filter(|name| !name.is_empty());
    let candidates: Vec<&str> = match named {
        Some(name) => vec![name],
        None => vec!["main", "master"],
    };
    for name in candidates {
        let (local, remote) = (format!("refs/heads/{name}"), format!("refs/remotes/origin/{name}"));
        if exists(&local) {
            return name.to_string();
        }
        if exists(&remote) {
            return format!("origin/{name}");
        }
    }
    "HEAD".into()
}

/// Creates a working tree for a new session of the project at `path`, on a new branch from the
/// project's default branch ([`base_ref`]). Uncommitted changes stay where they are, in the tree
/// they were made.
pub fn create(path: &Path, label: &str) -> Result<Worktree> {
    create_in(path, label, &managed_dir())
}

fn create_in(path: &Path, label: &str, managed: &Path) -> Result<Worktree> {
    let trees = list_in(path, managed)?;
    let main = trees.iter().find(|t| t.main).context("the repository has no working tree")?;
    let home = home_for(&main.path, managed);
    std::fs::create_dir_all(&home)?;
    // Sessions, prompts and code end up in there: keep it to the user, like the rest of the data folder.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(managed, std::fs::Permissions::from_mode(0o700));
    }
    let base = base_ref(path);
    let stamp = chrono::Local::now().format("%m%d-%H%M").to_string();
    // The name is free when it is picked, but another session may take it a moment later (two tabs
    // opened at once): pick again, a few times, instead of failing the launch.
    let mut attempt = 0;
    let (target, branch) = loop {
        attempt += 1;
        let branches = git(path, &["for-each-ref", "--format=%(refname:short)", "refs/heads/agentty"]).unwrap_or_default();
        let name =
            tree_name(label, &stamp, |name| home.join(name).exists() || branches.lines().any(|b| b == format!("{BRANCH_PREFIX}{name}")));
        let (target, branch) = (home.join(&name), format!("{BRANCH_PREFIX}{name}"));
        let target_text = target.to_string_lossy().to_string();
        match git(path, &["worktree", "add", "--no-track", "-b", &branch, &target_text, &base]) {
            Ok(_) => break (target, branch),
            Err(err) if attempt < 4 && format!("{err:#}").contains("already exists") => continue,
            Err(err) => {
                // Nothing was made (a repository without commits, say): don't leave an empty folder behind.
                let _ = std::fs::remove_dir(&home);
                return Err(err);
            }
        }
    };
    // A session tree of a project the user trusts in Claude Code is trusted too: the session starts
    // at once instead of asking about a folder that holds the same repository.
    // (Tests never touch the real `~/.claude.json`.)
    if !cfg!(test) {
        let _ = crate::claude_trust::inherit_trust(&main.path, &target);
    }
    let head = git(&target, &["rev-parse", "HEAD"]).map(|s| s.trim().to_string()).unwrap_or_default();
    Ok(Worktree { path: target, branch: Some(branch), head, main: false, managed: true, prunable: false })
}

/// Removes a working tree Agentty created, and its branch when nothing on it would be lost
/// (`git branch -d` refuses a branch that is not merged). Git refuses a tree with changes unless
/// `force`; trees Agentty did not create are never touched.
pub fn remove(tree: &Path, force: bool) -> Result<()> {
    remove_in(tree, force, &managed_dir())
}

fn remove_in(tree: &Path, force: bool, managed: &Path) -> Result<()> {
    ensure!(is_managed(tree, managed), "only worktrees created by Agentty are removed here");
    let trees = list_in(tree, managed)?;
    let main = trees.iter().find(|t| t.main).context("the repository has no working tree")?.path.clone();
    let canonical = tree.canonicalize().unwrap_or_else(|_| tree.to_path_buf());
    let branch =
        trees.iter().find(|t| t.path.canonicalize().unwrap_or_else(|_| t.path.clone()) == canonical).and_then(|t| t.branch.clone());
    let tree_text = tree.to_string_lossy().to_string();
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(&tree_text);
    git(&main, &args)?;
    if let Some(branch) = branch.filter(|b| b.starts_with(BRANCH_PREFIX)) {
        let _ = git(&main, &["branch", "-d", &branch]);
    }
    Ok(())
}

/// What a removal did, so the panel can say it in one line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Removal {
    /// The branch that was asked to go but git kept: it has commits nothing else has.
    pub branch_kept: Option<String>,
    /// The branch was deleted on the remote too (`origin/…`, as the user asked).
    pub remote_deleted: Option<String>,
    /// Git's reason for not deleting the remote branch (offline, no permission, already gone).
    pub remote_error: Option<String>,
}

/// The branch's counterpart on a remote (`origin/feature`), when the repository already knows one:
/// what it tracks, else a remote branch of the same name. Nothing is fetched — this reads the refs
/// git has, so it is safe to ask while a dialog is open.
pub fn remote_branch(repo: &Path, branch: &str) -> Option<String> {
    if !branch_ok(repo, branch) {
        return None;
    }
    let upstream = git(repo, &["for-each-ref", "--format=%(upstream:short)", &format!("refs/heads/{branch}")])
        .ok()
        .map(|out| out.trim().to_string())
        .filter(|name| !name.is_empty());
    if upstream.is_some() {
        return upstream;
    }
    let remote = format!("origin/{branch}");
    git(repo, &["rev-parse", "--verify", "--quiet", &format!("refs/remotes/{remote}")]).ok().map(|_| remote)
}

/// A name git itself accepts as a branch, never an option (`-D`).
fn branch_ok(repo: &Path, name: &str) -> bool {
    !name.starts_with('-') && git(repo, &["check-ref-format", "--branch", name]).is_ok()
}

/// Git's words for a failed push, with any credential a remote URL carries masked: an https remote
/// can hold `user:token@host`, and this text is shown in the panel.
fn mask_credentials(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("://") {
        let (head, tail) = rest.split_at(start + 3);
        out.push_str(head);
        // The authority ends at the path, the query or the end of the word.
        let end = tail.find(|c: char| c.is_whitespace() || c == '/' || c == '?').unwrap_or(tail.len());
        match tail[..end].rfind('@') {
            Some(at) => {
                out.push_str("***@");
                rest = &tail[at + 1..];
            }
            None => {
                out.push_str(&tail[..end]);
                rest = &tail[end..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Removes a linked working tree of the repository `repo` is in, when the user asks for it (the files
/// panel's menu) — one Agentty made or one they made themselves. Never the project's own tree and
/// never with `--force`: git refuses a tree with uncommitted or untracked files. With
/// `delete_branch`, its branch goes too through `git branch -d`, which git refuses when the branch
/// has commits nothing else has; a branch Agentty made (`agentty/…`) goes the same way when it is
/// merged. With `delete_remote` the branch goes on `origin` as well — but only once the local branch
/// really went: a branch git kept holds commits that only the remote still has.
pub fn remove_linked(repo: &Path, tree: &Path, delete_branch: bool, delete_remote: bool) -> Result<Removal> {
    let trees = list(repo)?;
    let main = trees.iter().find(|t| t.main).context("the repository has no working tree")?.path.clone();
    let same =
        |a: &Path| a.canonicalize().unwrap_or_else(|_| a.to_path_buf()) == tree.canonicalize().unwrap_or_else(|_| tree.to_path_buf());
    let entry = trees.iter().find(|t| same(&t.path)).context("not a working tree of this repository")?;
    ensure!(!entry.main, "the project's own working tree is never removed");
    let branch = entry.branch.clone();
    // Asked while the tree is still there: the remote it tracks is unreachable once it is gone.
    let remote = branch.as_deref().filter(|_| delete_branch && delete_remote).and_then(|b| remote_branch(&main, b));
    git(&main, &["worktree", "remove", &tree.to_string_lossy()])?;
    let Some(branch) = branch.filter(|b| delete_branch || b.starts_with(BRANCH_PREFIX)) else { return Ok(Removal::default()) };
    if git(&main, &["branch", "-d", &branch]).is_err() {
        return Ok(Removal { branch_kept: delete_branch.then_some(branch), ..Removal::default() });
    }
    let Some(remote) = remote else { return Ok(Removal::default()) };
    let Some((remote_name, remote_branch)) = remote.split_once('/') else { return Ok(Removal::default()) };
    // Both halves reach git as arguments of their own: neither may read as an option.
    if remote_name.starts_with('-') || !branch_ok(&main, remote_branch) {
        return Ok(Removal::default());
    }
    match git(&main, &["push", remote_name, "--delete", remote_branch]) {
        Ok(_) => Ok(Removal { remote_deleted: Some(remote.clone()), ..Removal::default() }),
        Err(err) => Ok(Removal { remote_error: Some(mask_credentials(&format!("{err:#}"))), ..Removal::default() }),
    }
}

/// Forgets working trees whose folder is gone (`git worktree prune`).
pub fn prune(repo: &Path) -> Result<()> {
    git(repo, &["worktree", "prune"]).map(|_| ())
}

/// Commits on `tree`'s branch that the main working tree does not have yet.
pub fn commits_ahead(tree: &Path, main_head: &str) -> u32 {
    if main_head.is_empty() || !main_head.chars().all(|c| c.is_ascii_hexdigit()) {
        return 0;
    }
    git(tree, &["rev-list", "--count", &format!("{main_head}..HEAD")]).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_worktree_list() {
        let raw = "worktree /repo\nHEAD 1111111111111111111111111111111111111111\nbranch refs/heads/main\n\n\
                   worktree /data/worktrees/repo-abcd/claude-0918-1432\nHEAD 2222222222222222222222222222222222222222\nbranch refs/heads/agentty/claude-0918-1432\n\n\
                   worktree /elsewhere/detached\nHEAD 3333333333333333333333333333333333333333\ndetached\n\n\
                   worktree /gone\nHEAD 4444444444444444444444444444444444444444\nbranch refs/heads/old\nprunable gitdir file points to non-existent location\n";
        let trees = parse_list(raw);
        assert_eq!(trees.len(), 4);
        assert!(trees[0].main && !trees[1].main);
        assert_eq!(trees[0].branch.as_deref(), Some("main"));
        assert_eq!(trees[1].branch.as_deref(), Some("agentty/claude-0918-1432"));
        assert_eq!(trees[1].name(), "claude-0918-1432");
        assert_eq!(trees[2].branch, None);
        assert!(trees[3].prunable);
    }

    #[test]
    fn session_trees_start_from_the_default_branch() {
        let refs = |known: &'static [&'static str]| move |r: &str| known.contains(&r);
        // origin/HEAD names the default branch; the local branch wins over the remote one.
        assert_eq!(pick_base(Some("origin/develop"), refs(&["refs/heads/develop", "refs/remotes/origin/develop"])), "develop");
        assert_eq!(pick_base(Some("origin/develop"), refs(&["refs/remotes/origin/develop"])), "origin/develop");
        // No origin/HEAD: main, then master.
        assert_eq!(pick_base(None, refs(&["refs/heads/master"])), "master");
        assert_eq!(pick_base(None, refs(&["refs/heads/main", "refs/heads/master"])), "main");
        assert_eq!(pick_base(None, refs(&["refs/remotes/origin/main"])), "origin/main");
        // Nothing to go by: the checked-out commit.
        assert_eq!(pick_base(None, refs(&[])), "HEAD");
        assert_eq!(pick_base(Some("origin/trunk"), refs(&["refs/heads/main"])), "HEAD");
    }

    /// Trees the user made are removed on request too, safely: never the project's own tree, never
    /// one with changes, and a branch with unmerged commits stays.
    #[test]
    fn removes_linked_trees_on_request() {
        if crate::process::command("git").arg("--version").output().is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("agentty-worktree-linked-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let repo = dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]).unwrap();
        for (key, value) in [("user.email", "t@example.com"), ("user.name", "Tester"), ("commit.gpgsign", "false")] {
            git(&repo, &["config", key, value]).unwrap();
        }
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
        git(&repo, &["add", "a.txt"]).unwrap();
        git(&repo, &["commit", "-q", "-m", "first"]).unwrap();
        let add = |name: &str| {
            let path = dir.join(name);
            git(&repo, &["worktree", "add", "-q", "-b", name, &path.to_string_lossy()]).unwrap();
            path
        };

        // The project's own tree: never.
        assert!(remove_linked(&repo, &repo, false, false).is_err());

        // A tree with changes stays; a clean one goes, and its branch only when asked.
        let kept = add("kept-branch");
        std::fs::write(kept.join("new.txt"), "x\n").unwrap();
        assert!(remove_linked(&repo, &kept, false, false).is_err(), "untracked files keep the tree");
        std::fs::remove_file(kept.join("new.txt")).unwrap();
        assert_eq!(remove_linked(&repo, &kept, false, false).unwrap(), Removal::default());
        assert!(!kept.exists());
        assert!(git(&repo, &["rev-parse", "--verify", "--quiet", "refs/heads/kept-branch"]).is_ok());

        // Asked to delete a merged branch: gone. An unmerged one: kept, and said so.
        let merged = add("merged");
        assert_eq!(remove_linked(&repo, &merged, true, false).unwrap(), Removal::default());
        assert!(git(&repo, &["rev-parse", "--verify", "--quiet", "refs/heads/merged"]).is_err());
        let ahead = add("ahead");
        std::fs::write(ahead.join("b.txt"), "two\n").unwrap();
        git(&ahead, &["add", "b.txt"]).unwrap();
        git(&ahead, &["commit", "-q", "-m", "work"]).unwrap();
        assert_eq!(remove_linked(&repo, &ahead, true, false).unwrap().branch_kept, Some("ahead".to_string()));
        assert!(!ahead.exists());
        assert!(git(&repo, &["rev-parse", "--verify", "--quiet", "refs/heads/ahead"]).is_ok());

        // A branch that was pushed: asked for, the remote one goes too — and the local branch going
        // is the condition, so an unmerged branch keeps both.
        let origin = dir.join("origin.git");
        git(&repo, &["init", "-q", "--bare", &origin.to_string_lossy()]).unwrap();
        git(&repo, &["remote", "add", "origin", &origin.to_string_lossy()]).unwrap();
        let pushed = add("pushed");
        git(&pushed, &["push", "-q", "-u", "origin", "pushed"]).unwrap();
        assert_eq!(remote_branch(&repo, "pushed").as_deref(), Some("origin/pushed"));
        assert_eq!(remote_branch(&repo, "kept-branch"), None, "never pushed: no remote branch");
        assert_eq!(remote_branch(&repo, "--delete"), None, "an option is not a branch name");
        let removal = remove_linked(&repo, &pushed, true, true).unwrap();
        assert_eq!(removal.remote_deleted.as_deref(), Some("origin/pushed"));
        assert!(removal.remote_error.is_none());
        assert!(git(&repo, &["rev-parse", "--verify", "--quiet", "refs/remotes/origin/pushed"]).is_err());

        let unmerged = add("unmerged");
        git(&unmerged, &["push", "-q", "-u", "origin", "unmerged"]).unwrap();
        // A commit made after the push: neither the project's branch nor the remote has it, so git
        // refuses `branch -d` and the remote branch stays with it.
        std::fs::write(unmerged.join("c.txt"), "three\n").unwrap();
        git(&unmerged, &["add", "c.txt"]).unwrap();
        git(&unmerged, &["commit", "-q", "-m", "work"]).unwrap();
        let removal = remove_linked(&repo, &unmerged, true, true).unwrap();
        assert_eq!(removal.branch_kept, Some("unmerged".to_string()));
        assert!(removal.remote_deleted.is_none(), "the remote keeps commits the local branch still has");
        assert!(git(&origin, &["rev-parse", "--verify", "--quiet", "refs/heads/unmerged"]).is_ok());

        // A tree whose folder was deleted by hand: prune forgets it.
        let gone = add("gone");
        std::fs::remove_dir_all(&gone).unwrap();
        assert!(list(&repo).unwrap().iter().any(|t| t.prunable));
        prune(&repo).unwrap();
        assert_eq!(list(&repo).unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_push_never_shows_the_credential_in_its_remote() {
        assert_eq!(
            mask_credentials("fatal: could not read from 'https://someone:not_a_real_token@example.com/x.git'"),
            "fatal: could not read from 'https://***@example.com/x.git'"
        );
        // Nothing to mask, nothing changed.
        assert_eq!(
            mask_credentials("error: failed to push some refs to 'https://example.com/x.git'"),
            "error: failed to push some refs to 'https://example.com/x.git'"
        );
        assert_eq!(mask_credentials("remote: permission denied"), "remote: permission denied");
        // An `@` further along the line is not part of the authority.
        assert_eq!(mask_credentials("https://example.com/a@b and mail@example.com"), "https://example.com/a@b and mail@example.com");
    }

    #[test]
    fn names_are_safe_and_unique() {
        assert_eq!(tree_name("claude", "0918-1432", |_| false), "claude-0918-1432");
        assert_eq!(tree_name("claude", "0918-1432", |name| name == "claude-0918-1432"), "claude-0918-1432-2");
        // Whatever the label was, the name stays a plain folder and branch name.
        assert_eq!(tree_name("../x; rm -rf ~", "0918-1432", |_| false), "xrm-rf-0918-1432");
        assert_eq!(tree_name("", "0918-1432", |_| false), "session-0918-1432");
    }

    #[test]
    fn finds_the_working_tree_of_a_path() {
        let dir = std::env::temp_dir().join(format!("agentty-tree-root-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("main/.git")).unwrap();
        std::fs::create_dir_all(dir.join("main/src/deep")).unwrap();
        std::fs::create_dir_all(dir.join("linked/src")).unwrap();
        std::fs::write(dir.join("linked/.git"), "gitdir: /somewhere/.git/worktrees/linked\n").unwrap();
        assert_eq!(tree_root(&dir.join("main/src/deep")), Some(dir.join("main")));
        assert_eq!(tree_root(&dir.join("linked/src")), Some(dir.join("linked")));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn creates_lists_and_removes_a_session_tree() {
        if crate::process::command("git").arg("--version").output().is_err() {
            return;
        }
        let data = std::env::temp_dir().join(format!("agentty-worktree-data-{}", std::process::id()));
        let repo = std::env::temp_dir().join(format!("agentty-worktree-repo-{}", std::process::id()));
        for dir in [&data, &repo] {
            let _ = std::fs::remove_dir_all(dir);
            std::fs::create_dir_all(dir).unwrap();
        }
        git(&repo, &["init", "-q", "-b", "main"]).unwrap();
        for (key, value) in [("user.email", "t@example.com"), ("user.name", "Tester"), ("commit.gpgsign", "false")] {
            git(&repo, &["config", key, value]).unwrap();
        }
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
        git(&repo, &["add", "a.txt"]).unwrap();
        git(&repo, &["commit", "-q", "-m", "first"]).unwrap();

        // A repository without a commit has nothing to branch from: an error, and no folder left behind.
        let unborn = std::env::temp_dir().join(format!("agentty-worktree-unborn-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&unborn);
        std::fs::create_dir_all(&unborn).unwrap();
        git(&unborn, &["init", "-q", "-b", "main"]).unwrap();
        assert!(create_in(&unborn, "claude", &data).is_err());
        assert_eq!(std::fs::read_dir(&data).unwrap().count(), 0, "nothing is left in the worktree folder");
        let _ = std::fs::remove_dir_all(&unborn);

        let tree = create_in(&repo, "claude", &data).unwrap();
        assert!(tree.managed && tree.path.join("a.txt").exists());
        assert!(tree.branch.as_deref().is_some_and(|b| b.starts_with("agentty/claude-")));
        let listed = list_in(&repo, &data).unwrap();
        assert_eq!(listed.len(), 2);
        assert!(listed[0].main && listed[1].managed);
        assert_eq!(commits_ahead(&tree.path, &listed[0].head), 0);

        // Work in the session's tree does not show up in the project's own tree.
        std::fs::write(tree.path.join("a.txt"), "two\n").unwrap();
        assert_eq!(std::fs::read_to_string(repo.join("a.txt")).unwrap(), "one\n");
        assert!(remove_in(&tree.path, false, &data).is_err(), "a tree with changes is kept");
        assert!(remove_in(&repo, true, &data).is_err(), "the project's own tree is never removed");
        // Neither `..` nor a symlink makes a folder elsewhere Agentty's.
        assert!(!is_managed(&data.join("..").join(repo.file_name().unwrap()), &data));
        #[cfg(unix)]
        {
            let link = data.join("link-to-the-project");
            std::os::unix::fs::symlink(&repo, &link).unwrap();
            assert!(!is_managed(&link, &data));
            assert!(remove_in(&link, true, &data).is_err());
            std::fs::remove_file(&link).unwrap();
        }
        assert!(!is_managed(&data.join("gone"), &data));
        remove_in(&tree.path, true, &data).unwrap();
        assert_eq!(list_in(&repo, &data).unwrap().len(), 1);

        // The project folder on a feature branch: a new session still starts from the default branch.
        git(&repo, &["checkout", "-q", "-b", "feature"]).unwrap();
        std::fs::write(repo.join("feature.txt"), "wip\n").unwrap();
        git(&repo, &["add", "feature.txt"]).unwrap();
        git(&repo, &["commit", "-q", "-m", "wip"]).unwrap();
        let tree = create_in(&repo, "codex", &data).unwrap();
        assert!(tree.path.join("a.txt").exists() && !tree.path.join("feature.txt").exists());
        remove_in(&tree.path, true, &data).unwrap();

        for dir in [&data, &repo] {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}
