//! Monitoring → Worktrees: every git working tree on this computer that Agentty can find, with
//! what state it is in — when it was last worked on, uncommitted changes, commits the default
//! branch doesn't have, and whether its pull request was merged — so old ones can go.
//!
//! "Every" means: the repositories under the home folder (a few folders deep, skipping the
//! folders that never hold projects), the ones Agentty's own worktree folder points to, and the
//! folders the caller already knows (open workspaces). Each repository then lists its trees
//! itself (`git worktree list`), so a tree kept anywhere else on disk is found through its project.

use crate::github::{self, PullRequest};
use crate::worktree::{self, git, Removal, Worktree};
use anyhow::{ensure, Context, Result};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// How deep under the home folder a repository is looked for (`~/a/b/c/repo`).
const SEARCH_DEPTH: usize = 4;
/// Folders under the home folder a search never enters: they hold no projects, or reading them
/// makes macOS ask for permissions.
const NOT_PROJECTS: &[&str] = &[
    "Library",
    "Applications",
    "Pictures",
    "Music",
    "Movies",
    "Public",
    "AppData",
    "node_modules",
    "target",
    "vendor",
    "Pods",
    "DerivedData",
    "site-packages",
];

/// A working tree and what it holds that the default branch doesn't.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeStatus {
    /// The repository's main working tree.
    pub repo: PathBuf,
    #[serde(skip)]
    pub tree: Worktree,
    /// The default branch it is compared with (`main`, `origin/main`).
    pub base: String,
    /// Files with uncommitted changes (untracked ones included).
    pub dirty: u32,
    /// Commits on it that `base` doesn't have.
    pub ahead: u32,
    /// When it was last committed to (seconds since the epoch; 0 when unknown).
    pub last_commit: i64,
    /// The newest change among its uncommitted files (seconds; 0 when there are none).
    pub last_change: i64,
    /// Its branch's latest pull request on GitHub.
    pub pr: Option<PullRequest>,
    /// The pull request was merged with exactly the commit this tree is on: nothing here is lost
    /// if the branch goes, even when GitHub squashed it (which `git branch -d` can't tell).
    pub pr_merged_here: bool,
    /// Size on disk, once measured.
    pub size: Option<u64>,
}

impl TreeStatus {
    /// When it was last worked on: its last commit or its newest uncommitted change.
    pub fn last_worked(&self) -> i64 {
        self.last_commit.max(self.last_change)
    }

    /// Nothing would be lost if it went: no uncommitted work, and its commits are in the default
    /// branch (or in a merged pull request).
    pub fn safe_to_remove(&self) -> bool {
        !self.tree.main && self.dirty == 0 && (self.ahead == 0 || self.pr_merged_here)
    }
}

fn is_hidden(name: &str) -> bool {
    name.starts_with('.')
}

/// The main working tree of the repository `dir` (a folder holding `.git`) belongs to.
fn main_root(dir: &Path) -> Option<PathBuf> {
    let dot_git = dir.join(".git");
    let meta = fs::symlink_metadata(&dot_git).ok()?;
    if meta.is_dir() {
        return Some(dir.to_path_buf());
    }
    // A linked tree (or a submodule): git knows where its repository is.
    let common = git(dir, &["rev-parse", "--path-format=absolute", "--git-common-dir"]).ok()?;
    let common = PathBuf::from(common.trim());
    (common.file_name()? == ".git").then(|| common.parent().map(Path::to_path_buf)).flatten()
}

/// Folders holding a `.git` under `root`, at most `depth` levels down. A repository's own
/// folders are not searched further.
fn repos_under(root: &Path, depth: usize, skip_names: bool, out: &mut Vec<PathBuf>, cancel: &AtomicBool) {
    if cancel.load(Ordering::Relaxed) {
        return;
    }
    if root.join(".git").exists() {
        out.push(root.to_path_buf());
        return;
    }
    if depth == 0 {
        return;
    }
    let Ok(entries) = fs::read_dir(root) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !entry.file_type().is_ok_and(|t| t.is_dir()) || is_hidden(&name) || (skip_names && NOT_PROJECTS.contains(&name.as_str())) {
            continue;
        }
        repos_under(&entry.path(), depth - 1, skip_names, out, cancel);
    }
}

/// The main working trees of every repository found: under the home folder, through Agentty's
/// worktree folder, and at `known` (folders the caller has open). Sorted by path.
pub fn find_repos(known: &[PathBuf], cancel: &AtomicBool) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    repos_under(&crate::fsutil::home(), SEARCH_DEPTH, true, &mut candidates, cancel);
    repos_under(&worktree::managed_dir(), 2, false, &mut candidates, cancel);
    candidates.extend(known.iter().filter_map(|dir| worktree::tree_root(dir)));
    unique_roots(candidates)
}

/// The main working trees of the repositories `candidates` (folders holding `.git`) belong to,
/// each once, sorted.
fn unique_roots(candidates: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut repos: Vec<PathBuf> = candidates
        .into_iter()
        .filter_map(|dir| main_root(&dir))
        .map(|root| root.canonicalize().unwrap_or(root))
        .filter(|root| seen.insert(root.clone()))
        .collect();
    repos.sort();
    repos
}

/// `git status --porcelain` → changed paths (the new name of a rename).
fn changed_paths(porcelain: &str) -> Vec<String> {
    porcelain
        .lines()
        .filter(|line| line.len() > 3)
        .map(|line| {
            let path = &line[3..];
            path.rsplit_once(" -> ").map_or(path, |(_, new)| new).trim_matches('"').to_string()
        })
        .collect()
}

fn mtime_secs(path: &Path) -> i64 {
    fs::symlink_metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs() as i64)
}

/// The state of one tree of `repo`, compared with `base`.
fn tree_status(repo: &Path, tree: Worktree, base: &str) -> TreeStatus {
    let mut status = TreeStatus {
        repo: repo.to_path_buf(),
        base: base.to_string(),
        dirty: 0,
        ahead: 0,
        last_commit: 0,
        last_change: 0,
        pr: None,
        pr_merged_here: false,
        size: None,
        tree,
    };
    if status.tree.prunable || !status.tree.path.is_dir() {
        return status;
    }
    let path = status.tree.path.clone();
    if let Ok(out) = git(&path, &["status", "--porcelain"]) {
        let changed = changed_paths(&out);
        status.dirty = changed.len() as u32;
        status.last_change = changed.iter().take(500).map(|p| mtime_secs(&path.join(p))).max().unwrap_or(0);
    }
    status.last_commit = git(&path, &["log", "-1", "--format=%ct"]).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
    let head = &status.tree.head;
    if !head.is_empty() && head.chars().all(|c| c.is_ascii_hexdigit()) && base != "HEAD" {
        status.ahead =
            git(repo, &["rev-list", "--count", &format!("{base}..{head}")]).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
    }
    status
}

/// Every tree of the repository at `repo` with its state (no pull requests yet: see
/// [`attach_pull_requests`]).
pub fn repo_trees(repo: &Path) -> Vec<TreeStatus> {
    let Ok(trees) = worktree::list(repo) else { return Vec::new() };
    let base = worktree::base_ref(repo);
    trees.into_iter().map(|tree| tree_status(repo, tree, &base)).collect()
}

/// The trees of all `repos`, read in parallel.
pub fn all_trees(repos: &[PathBuf], cancel: &AtomicBool) -> Vec<TreeStatus> {
    let mut trees: Vec<TreeStatus> =
        crate::disk::in_parallel(repos.to_vec(), 6, |repo| if cancel.load(Ordering::Relaxed) { Vec::new() } else { repo_trees(&repo) })
            .into_iter()
            .flatten()
            .collect();
    trees.sort_by(|a, b| a.repo.cmp(&b.repo).then(b.tree.main.cmp(&a.tree.main)).then(a.tree.path.cmp(&b.tree.path)));
    trees
}

/// Whether the repository's `origin` is on GitHub (only those are asked about pull requests).
fn on_github(repo: &Path) -> bool {
    git(repo, &["remote", "get-url", "origin"]).is_ok_and(|url| url.contains("github.com"))
}

/// Fills in each tree's pull request, one `gh` call per repository, in parallel. Trees without a
/// branch, of repositories not on GitHub, or when `gh` is missing, keep `None`.
pub fn attach_pull_requests(trees: &mut [TreeStatus]) {
    let repos: Vec<PathBuf> = {
        let mut repos: Vec<PathBuf> = trees.iter().filter(|t| !t.tree.main && t.tree.branch.is_some()).map(|t| t.repo.clone()).collect();
        repos.sort();
        repos.dedup();
        repos
    };
    let lists: HashMap<PathBuf, Vec<github::BranchPullRequest>> = std::thread::scope(|scope| {
        let handles: Vec<_> = repos
            .iter()
            .map(|repo| scope.spawn(move || (repo.clone(), if on_github(repo) { github::pull_requests(repo) } else { None })))
            .collect();
        handles.into_iter().filter_map(|h| h.join().ok()).filter_map(|(repo, list)| list.map(|l| (repo, l))).collect()
    });
    for tree in trees.iter_mut() {
        let (Some(list), Some(branch)) = (lists.get(&tree.repo), tree.tree.branch.as_deref()) else { continue };
        // Newest first: the branch's latest pull request.
        if let Some(found) = list.iter().find(|p| p.head_ref_name == branch) {
            tree.pr_merged_here = found.pr.is_merged() && !found.head_ref_oid.is_empty() && found.head_ref_oid == tree.tree.head;
            tree.pr = Some(found.pr.clone());
        }
    }
}

/// `path` with links resolved, also once it is gone (through its folder, which still exists):
/// `/var/…` and `/private/var/…` are the same tree on macOS.
fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| match (path.parent().and_then(|p| p.canonicalize().ok()), path.file_name()) {
        (Some(parent), Some(name)) => parent.join(name),
        _ => path.to_path_buf(),
    })
}

/// Removes the linked tree `tree` of the repository `repo`. `force` removes it even with
/// uncommitted changes (the user ticked that). With `delete_branch`, its branch goes when nothing
/// on it would be lost: git's own check (`branch -d`), or — for a branch GitHub squashed or rebased
/// — `merged_head`, the commit its merged pull request ended on, being exactly the branch's tip.
/// Never the project's own tree, never its default branch.
pub fn remove(repo: &Path, tree: &Path, delete_branch: bool, force: bool, merged_head: Option<&str>) -> Result<Removal> {
    let trees = worktree::list(repo)?;
    let main = trees.iter().find(|t| t.main).context("the repository has no working tree")?.path.clone();
    let wanted = canonical(tree);
    let entry = trees.iter().find(|t| canonical(&t.path) == wanted).context("not a working tree of this repository")?.clone();
    ensure!(!entry.main, "the project's own working tree is never removed");
    if entry.prunable || !entry.path.exists() {
        // Its folder is already gone: git only has to forget it.
        worktree::prune(&main)?;
    } else {
        let path = entry.path.to_string_lossy().to_string();
        let mut args = vec!["worktree", "remove"];
        if force {
            args.push("--force");
        }
        args.push(&path);
        git(&main, &args)?;
    }
    let Some(branch) = entry.branch.filter(|_| delete_branch) else { return Ok(Removal::default()) };
    let base = worktree::base_ref(&main);
    let default = base.strip_prefix("origin/").unwrap_or(&base);
    // Another tree may still have it checked out; git refuses that too, but say it plainly.
    if branch == default || branch.starts_with('-') || trees.iter().any(|t| t.path != entry.path && t.branch.as_deref() == Some(&branch)) {
        return Ok(Removal { branch_kept: Some(branch), ..Removal::default() });
    }
    if git(&main, &["branch", "-d", &branch]).is_ok() {
        return Ok(Removal::default());
    }
    let tip = git(&main, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")])
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if merged_head.is_some_and(|head| !head.is_empty() && head == tip) && git(&main, &["branch", "-D", &branch]).is_ok() {
        return Ok(Removal::default());
    }
    Ok(Removal { branch_kept: Some(branch), ..Removal::default() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_in(dir: &Path) -> PathBuf {
        let repo = dir.join("repo");
        fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]).unwrap();
        for (key, value) in [("user.email", "t@example.com"), ("user.name", "Tester"), ("commit.gpgsign", "false")] {
            git(&repo, &["config", key, value]).unwrap();
        }
        fs::write(repo.join("a.txt"), "one\n").unwrap();
        git(&repo, &["add", "a.txt"]).unwrap();
        git(&repo, &["commit", "-q", "-m", "first"]).unwrap();
        repo
    }

    #[test]
    fn reads_changed_paths() {
        let out = " M src/a.rs\n?? new file.txt\nR  old.rs -> new.rs\n?? \"quoted name\"\n";
        assert_eq!(changed_paths(out), vec!["src/a.rs", "new file.txt", "new.rs", "quoted name"]);
    }

    #[test]
    fn lists_trees_with_their_state_and_removes_them_safely() {
        if crate::process::command("git").arg("--version").output().is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("agentty-inventory-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let repo = repo_in(&dir);
        let add = |name: &str| {
            let path = dir.join(name);
            git(&repo, &["worktree", "add", "-q", "-b", name, &path.to_string_lossy()]).unwrap();
            path
        };
        let clean = add("clean");
        let dirty = add("dirty");
        fs::write(dirty.join("wip.txt"), "wip\n").unwrap();
        let ahead = add("ahead");
        fs::write(ahead.join("b.txt"), "two\n").unwrap();
        git(&ahead, &["add", "b.txt"]).unwrap();
        git(&ahead, &["commit", "-q", "-m", "work"]).unwrap();

        // Found from a linked tree as well as from the project.
        let repo_real = repo.canonicalize().unwrap();
        let known: Vec<PathBuf> = [clean.clone(), repo.clone()].iter().filter_map(|d| worktree::tree_root(d)).collect();
        assert_eq!(unique_roots(known), vec![repo_real]);

        let trees = repo_trees(&repo);
        assert_eq!(trees.len(), 4);
        let by_name = |name: &str| trees.iter().find(|t| t.tree.name() == name).unwrap().clone();
        let main = trees.iter().find(|t| t.tree.main).unwrap();
        assert!(!main.safe_to_remove(), "the project's own tree never is");
        assert!(by_name("clean").safe_to_remove());
        let dirty_status = by_name("dirty");
        assert_eq!(dirty_status.dirty, 1);
        assert!(dirty_status.last_change > 0 && !dirty_status.safe_to_remove());
        let ahead_status = by_name("ahead");
        assert_eq!(ahead_status.ahead, 1);
        assert!(ahead_status.last_commit > 0 && !ahead_status.safe_to_remove());

        // The project's own tree: never.
        assert!(remove(&repo, &repo, true, true, None).is_err());
        // Changes keep a tree unless forced.
        assert!(remove(&repo, &dirty, true, false, None).is_err());
        assert!(dirty.exists());
        assert_eq!(remove(&repo, &dirty, true, true, None).unwrap(), Removal::default());
        assert!(!dirty.exists());
        // A branch with commits nothing else has stays with its tree gone…
        assert_eq!(remove(&repo, &ahead, true, false, None).unwrap().branch_kept.as_deref(), Some("ahead"));
        assert!(git(&repo, &["rev-parse", "--verify", "--quiet", "refs/heads/ahead"]).is_ok());
        // …unless its pull request was merged with exactly that commit (a squash merge).
        let clean_tip = git(&repo, &["rev-parse", "clean"]).unwrap().trim().to_string();
        git(&repo, &["worktree", "add", "-q", &dir.join("again").to_string_lossy(), "ahead"]).unwrap();
        let tip = git(&repo, &["rev-parse", "ahead"]).unwrap().trim().to_string();
        assert_eq!(remove(&repo, &dir.join("again"), true, false, Some(&clean_tip)).unwrap().branch_kept.as_deref(), Some("ahead"));
        git(&repo, &["worktree", "add", "-q", &dir.join("again").to_string_lossy(), "ahead"]).unwrap();
        assert_eq!(remove(&repo, &dir.join("again"), true, false, Some(&tip)).unwrap(), Removal::default());
        assert!(git(&repo, &["rev-parse", "--verify", "--quiet", "refs/heads/ahead"]).is_err());
        // A tree whose folder was deleted by hand is forgotten.
        fs::remove_dir_all(&clean).unwrap();
        assert_eq!(remove(&repo, &clean, true, false, None).unwrap(), Removal::default());
        assert_eq!(repo_trees(&repo).len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }
}
