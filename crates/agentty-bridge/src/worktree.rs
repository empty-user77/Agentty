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

/// The repository's main working tree for `path`, whether it is in that tree or in one of its
/// linked worktrees. Read from the `.git` entries, no git process; `None` outside git and for a
/// repository whose shared folder is not a tree's `.git` (bare, `--separate-git-dir`).
pub fn main_tree(path: &Path) -> Option<PathBuf> {
    let root = tree_root(path)?;
    if root.join(".git").is_dir() {
        return Some(root);
    }
    // A linked worktree: `.git` is a file naming `<repo>/.git/worktrees/<name>`, whose `commondir`
    // points back at the shared folder (relative to it).
    let gitdir = linked_gitdir(&root)?;
    let common = std::fs::read_to_string(gitdir.join("commondir")).ok()?;
    let common = if Path::new(common.trim()).is_absolute() { PathBuf::from(common.trim()) } else { gitdir.join(common.trim()) };
    // Resolved by name, not `canonicalize`: on Windows that turns `C:\x` into `\\?\C:\x`,
    // which no longer starts with the folders it is compared with.
    let common = lexical(&common);
    if common.file_name()? != ".git" {
        return None;
    }
    common.parent().filter(|main| main.is_dir()).map(Path::to_path_buf)
}

/// `path` with its `.` and `..` parts resolved by name (symbolic links are left as they are).
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// The private git folder of the linked worktree at `root` (`<repo>/.git/worktrees/<name>`): its
/// `.git` file names it, and it has a `commondir`. A submodule's `.git` file names a folder without
/// one (`<super>/.git/modules/<name>`): no linked worktree, just a repository of its own.
fn linked_gitdir(root: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(root.join(".git")).ok()?;
    let gitdir = PathBuf::from(text.strip_prefix("gitdir:")?.trim());
    let gitdir = if gitdir.is_absolute() { gitdir } else { root.join(gitdir) };
    gitdir.join("commondir").is_file().then_some(gitdir)
}

/// Whether the working tree at `root` is a linked worktree of another repository (not its main
/// tree, not a submodule). Read from files, no git process.
pub fn is_linked(root: &Path) -> bool {
    linked_gitdir(root).is_some()
}

/// Where a pane works when its folder may be gone (a worktree removed under it): `path` while it is
/// a folder, else the first of `fallbacks` that is one, else `home`. A relative path counts as gone:
/// a shell started in a removed folder calls it `.`.
pub fn existing_dir(path: &Path, fallbacks: &[&Path], home: &Path) -> PathBuf {
    std::iter::once(path).chain(fallbacks.iter().copied()).find(|dir| dir.is_absolute() && dir.is_dir()).unwrap_or(home).to_path_buf()
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
    create_in(path, label, None, &managed_dir())
}

/// Like [`create`], but the new branch starts at `base` (a branch or commit of the repository)
/// instead of the default branch — a chat's workers start from its integration branch.
pub fn create_from(path: &Path, label: &str, base: &str) -> Result<Worktree> {
    create_in(path, label, Some(base), &managed_dir())
}

fn create_in(path: &Path, label: &str, base: Option<&str>, managed: &Path) -> Result<Worktree> {
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
    let base = base.map(str::to_string).unwrap_or_else(|| base_ref(path));
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
pub(crate) fn mask_credentials(text: &str) -> String {
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
/// `delete_branch`, its branch goes too, but only when the project's default branch has all its
/// commits ([`merged_into_default`]); a branch Agentty made (`agentty/…`) goes the same way when it
/// is merged. Not `git branch -d` alone: that also passes a branch merged only into its own upstream
/// (pushed with `-u`), whose commits the remote copy deleted next would take with it. With
/// `delete_remote` the branch goes on `origin` as well — but only once the local branch really went:
/// a branch that is kept holds commits the default branch does not have.
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
    // `-D` once the check passed: `-d` knows only `HEAD` and the branch's upstream, not `origin/HEAD`.
    if !merged_into_default(&main, &branch) || git(&main, &["branch", "-D", &branch]).is_err() {
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

/// Linked working trees of the repository whose main tree is `main_root`, counted from the folders
/// git keeps for them (`.git/worktrees/*`, whose `gitdir` names the tree's `.git`). No git process:
/// cheap enough for a pane's status probe. A tree whose folder is gone (prunable) doesn't count, as
/// the worktree menu doesn't list it either.
pub fn linked_count(main_root: &Path) -> usize {
    // `gitdir` is relative to this folder when git writes relative paths (`worktree.useRelativePaths`).
    let alive = |entry: &std::fs::DirEntry| {
        std::fs::read_to_string(entry.path().join("gitdir")).is_ok_and(|gitdir| entry.path().join(gitdir.trim()).exists())
    };
    std::fs::read_dir(main_root.join(".git").join("worktrees"))
        .map(|entries| entries.filter_map(|e| e.ok()).filter(|e| e.path().is_dir() && alive(e)).count())
        .unwrap_or(0)
}

/// What cleaning up a linked working tree would lose, as [`cleanup_check`] finds it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CleanupCheck {
    /// The tree's branch (`None` while detached).
    pub branch: Option<String>,
    /// Files with uncommitted changes, untracked ones included.
    pub changed_files: usize,
    /// Files git ignores that removing the tree would delete too (a `.env.local`, a local
    /// database), build output and dependency folders left out. Up to a few names, then the count.
    pub ignored_files: Vec<String>,
    /// Commits on the tree's branch that the default branch does not have.
    pub unmerged_commits: u32,
    /// What it was compared with (`origin/main`, else the local `main`); `None` in a repository
    /// without a default branch, where nothing can be called merged.
    pub default_branch: Option<String>,
}

impl CleanupCheck {
    /// Nothing would be lost: the branch is merged and the tree has no changes.
    pub fn is_safe(&self) -> bool {
        self.default_branch.is_some() && self.changed_files == 0 && self.unmerged_commits == 0 && self.ignored_files.is_empty()
    }
}

/// The project's default branch as refs to compare with: `origin/HEAD` (as last fetched) and the
/// local branch of the same name ([`pick_base`]'s choice). A branch is merged when either has all
/// its commits — the remote once a pull request landed, the local one after a merge by hand.
/// Returns (the name to show, the refs, the local branch's name).
fn default_refs(path: &Path) -> (Option<String>, Vec<String>, Option<String>) {
    let exists = |reference: &str| git(path, &["rev-parse", "--verify", "--quiet", &format!("{reference}^{{commit}}")]).is_ok();
    let remote_default = git(path, &["symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"]).ok();
    let remote_default = remote_default.as_deref().map(str::trim).filter(|r| !r.is_empty());
    let mut refs = Vec::new();
    let mut shown = None;
    if let Some(remote) = remote_default.filter(|_| exists("refs/remotes/origin/HEAD")) {
        refs.push("refs/remotes/origin/HEAD".to_string());
        shown = Some(remote.to_string());
    }
    let base = pick_base(remote_default, exists);
    let local = (base != "HEAD" && !base.starts_with("origin/")).then_some(base);
    if let Some(local) = &local {
        refs.push(format!("refs/heads/{local}"));
        shown = shown.or_else(|| Some(local.clone()));
    }
    (shown, refs, local)
}

/// Whether the project's default branch ([`default_refs`]: `origin/HEAD` or the local default
/// branch) has every commit of the local branch `branch`, so deleting it loses nothing. Never for
/// the default branch itself, and never in a repository without one, where nothing can be called
/// merged. Merged into its own upstream does not count: that copy may be deleted next.
pub(crate) fn merged_into_default(repo: &Path, branch: &str) -> bool {
    let (_, refs, local) = default_refs(repo);
    if refs.is_empty() || local.as_deref() == Some(branch) || !branch_ok(repo, branch) {
        return false;
    }
    let tip = format!("refs/heads/{branch}");
    let mut args = vec!["rev-list", "--count", tip.as_str(), "--not"];
    args.extend(refs.iter().map(String::as_str));
    git(repo, &args).ok().and_then(|out| out.trim().parse::<u32>().ok()) == Some(0)
}

/// Whether the linked working tree `tree` can go without losing anything: its uncommitted files and
/// the commits its branch has that the project's default branch (`origin/HEAD`, else the local
/// default branch) does not. Nothing is fetched: merged means merged as far as the last fetch knows.
pub fn cleanup_check(tree: &Path) -> Result<CleanupCheck> {
    let status = git(tree, &["status", "--porcelain", "--untracked-files=all"])?;
    let ignored = git(tree, &["status", "--porcelain", "--ignored=matching", "--untracked-files=normal"])?;
    // Paths are relative to the tree's top folder, wherever `tree` is in it.
    let top = git(tree, &["rev-parse", "--show-toplevel"]).map(|t| PathBuf::from(t.trim())).unwrap_or_else(|_| tree.to_path_buf());
    let mut ignored_files = Vec::new();
    for path in ignored.lines().filter_map(|line| line.strip_prefix("!! ")).map(|path| path.trim_matches('"')) {
        if is_rebuildable(path) {
            continue;
        }
        // A folder whose name is only maybe build output: judged by the files in it.
        if is_maybe_rebuildable_folder(path) {
            if let Some(files) = files_in(&top, path) {
                ignored_files.extend(files.into_iter().filter(|file| !is_rebuildable(file)));
                continue;
            }
        }
        ignored_files.push(path.to_string());
    }
    let branch = git(tree, &["symbolic-ref", "--quiet", "--short", "HEAD"]).ok().map(|b| b.trim().to_string()).filter(|b| !b.is_empty());
    let (default_branch, refs, _) = default_refs(tree);
    let unmerged_commits = if refs.is_empty() {
        0
    } else {
        let mut args = vec!["rev-list", "--count", "HEAD", "--not"];
        args.extend(refs.iter().map(String::as_str));
        git(tree, &args)?.trim().parse().unwrap_or(0)
    };
    Ok(CleanupCheck {
        branch,
        changed_files: status.lines().filter(|l| !l.trim().is_empty()).count(),
        ignored_files,
        unmerged_commits,
        default_branch,
    })
}

/// Folders that hold nothing but build output, dependencies and caches, whatever is in them: a
/// tool made them and a build makes them again.
const REBUILT: &[&str] = &[
    "target",
    "node_modules",
    ".next",
    ".nuxt",
    ".turbo",
    ".parcel-cache",
    ".svelte-kit",
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
    ".ruff_cache",
    ".gradle",
];

/// Folder names that are often build output but just as often hold somebody's files
/// (`build/local.properties`, `out/secrets.json`, a `dist/` of hand-made release notes): never
/// rebuildable as a whole, only through what is in them.
const MAYBE_REBUILT: &[&str] = &["dist", "build", "out", ".cache", "coverage"];

/// Ignored paths nothing is lost with: build output, dependencies and caches, made again by a build.
fn is_rebuildable(path: &str) -> bool {
    // Only a whole ignored folder of build output counts (git reports it as `target/`): a file inside
    // a folder of that name (`build/local.properties`) is somebody's file.
    let folder = path.ends_with('/');
    let path = path.trim_end_matches('/');
    let last = path.rsplit('/').next().unwrap_or(path);
    (folder && REBUILT.contains(&last)) || last == ".DS_Store" || last.ends_with(".pyc") || last.ends_with(".log")
}

/// A whole ignored folder (`build/`) whose name alone doesn't make it build output ([`MAYBE_REBUILT`]).
fn is_maybe_rebuildable_folder(path: &str) -> bool {
    let Some(path) = path.strip_suffix('/') else { return false };
    MAYBE_REBUILT.contains(&path.rsplit('/').next().unwrap_or(path))
}

/// Entries [`files_in`] reads at most before it gives up on listing a folder.
const FILES_IN_LIMIT: usize = 2000;

/// The files in the ignored folder `folder` (relative to the tree's top folder `top`, as git reports
/// it), relative to `top` the same way; folders of build output inside it ([`REBUILT`]) are left out
/// whole. Symbolic links count as files and are not followed. `None` when the folder can't be read
/// or holds more than [`FILES_IN_LIMIT`] entries, so the caller reports the folder itself rather
/// than nothing (a large build folder is still somebody's to confirm, and is never walked whole).
fn files_in(top: &Path, folder: &str) -> Option<Vec<String>> {
    let folder = folder.trim_end_matches('/');
    if folder.is_empty() || folder.split('/').any(|part| part == ".." || part == ".") {
        return None;
    }
    let mut files = Vec::new();
    let mut pending = vec![folder.to_string()];
    let mut seen = 0usize;
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(top.join(&dir)).ok()? {
            seen += 1;
            if seen > FILES_IN_LIMIT {
                return None;
            }
            let entry = entry.ok()?;
            let name = entry.file_name().to_string_lossy().to_string();
            let path = format!("{dir}/{name}");
            if entry.file_type().ok()?.is_dir() {
                if !REBUILT.contains(&name.as_str()) {
                    pending.push(path);
                }
            } else {
                files.push(path);
            }
        }
    }
    files.sort();
    Some(files)
}

/// What a clean-up did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cleanup {
    /// The repository's main working tree: where the panes that were in the tree go back to.
    pub main: PathBuf,
    /// The branch that went with the tree.
    pub branch_deleted: Option<String>,
}

/// Cleans up the linked working tree `tree`: removes it and deletes its branch. Without `force` only
/// when [`cleanup_check`] finds nothing to lose; with it (the user saw what would be lost and said
/// yes), uncommitted changes and unmerged commits go too. Never the project's own tree, and never
/// the default branch itself or a branch another tree has checked out.
pub fn clean_up(tree: &Path, force: bool) -> Result<Cleanup> {
    let trees = list(tree)?;
    let main = trees.iter().find(|t| t.main).context("the repository has no working tree")?.path.clone();
    let same =
        |a: &Path| a.canonicalize().unwrap_or_else(|_| a.to_path_buf()) == tree.canonicalize().unwrap_or_else(|_| tree.to_path_buf());
    let entry = trees.iter().find(|t| same(&t.path)).context("not a working tree of this repository")?;
    ensure!(!entry.main, "the project's own working tree is never removed");
    if !force {
        let check = cleanup_check(&entry.path)?;
        ensure!(check.is_safe(), "the working tree has changes or commits the default branch does not have");
    }
    // Without a default branch to tell the project's main line by, no branch is deleted: the tree
    // goes, its branch (maybe `develop`) stays.
    let (_, refs, default_local) = default_refs(&main);
    let branch = entry.branch.clone().filter(|b| {
        !refs.is_empty()
            && Some(b) != default_local.as_ref()
            && !trees.iter().any(|t| !same(&t.path) && t.branch.as_ref() == Some(b))
            && branch_ok(&main, b)
    });
    let path_text = entry.path.to_string_lossy().to_string();
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(&path_text);
    git(&main, &args)?;
    // Merged (checked above) or given up on by the user: `-D`, since `-d` only knows the local
    // default branch and would keep a branch that landed through a pull request.
    let branch_deleted = branch.filter(|b| git(&main, &["branch", "-D", b]).is_ok());
    Ok(Cleanup { main, branch_deleted })
}

/// Forgets working trees whose folder is gone (`git worktree prune`).
pub fn prune(repo: &Path) -> Result<()> {
    git(repo, &["worktree", "prune"]).map(|_| ())
}

/// The commit a working tree's own work is counted from: the remote's default branch
/// (`origin/HEAD`) as last fetched, else `main_head` (the project folder's `HEAD`) for a
/// repository without a remote. Counted from the project folder alone, a tree that is up to date
/// with GitHub showed as ahead whenever that folder had not been pulled.
pub fn ahead_base(repo: &Path, main_head: &str) -> String {
    git(repo, &["rev-parse", "--verify", "--quiet", "refs/remotes/origin/HEAD^{commit}"])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| main_head.to_string())
}

/// Commits on `tree`'s branch that `base` (see [`ahead_base`]) does not have yet.
pub fn commits_ahead(tree: &Path, base: &str) -> u32 {
    if base.is_empty() || !base.chars().all(|c| c.is_ascii_hexdigit()) {
        return 0;
    }
    git(tree, &["rev-list", "--count", &format!("{base}..HEAD")]).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0)
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

    /// A pane whose folder is gone lands in the first fallback that still exists, else at home;
    /// never in `.` or another relative path.
    #[test]
    fn existing_dir_skips_gone_folders() {
        let dir = std::env::temp_dir().join(format!("agentty-existing-dir-{}", std::process::id()));
        let (here, project, home) = (dir.join("here"), dir.join("project"), dir.join("home"));
        for folder in [&here, &project, &home] {
            std::fs::create_dir_all(folder).unwrap();
        }
        let gone = dir.join("gone");
        assert_eq!(existing_dir(&here, &[&project], &home), here);
        assert_eq!(existing_dir(&gone, &[&project], &home), project);
        assert_eq!(existing_dir(&gone, &[&gone, &project], &home), project);
        assert_eq!(existing_dir(&gone, &[&gone], &home), home);
        assert_eq!(existing_dir(&gone, &[], &home), home);
        assert_eq!(existing_dir(Path::new("."), &[&project], &home), project);
        assert_eq!(existing_dir(Path::new("."), &[Path::new("here")], &home), home);
        std::fs::remove_dir_all(dir).ok();
    }

    /// With no default branch to tell the main line by (a project on `develop`), a forced clean-up
    /// removes the tree and keeps its branch.
    #[test]
    fn keeps_the_branch_without_a_default_branch() {
        if crate::process::command("git").arg("--version").output().is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("agentty-no-default-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let repo = dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "develop"]).unwrap();
        git(&repo, &["-c", "user.email=t@example.com", "-c", "user.name=Tester", "commit", "-q", "--allow-empty", "-m", "first"]).unwrap();
        git(&repo, &["switch", "-q", "-c", "elsewhere"]).unwrap();
        let tree = dir.join("tree");
        git(&repo, &["worktree", "add", "-q", &tree.to_string_lossy(), "develop"]).unwrap();
        let done = clean_up(&tree, true).unwrap();
        assert_eq!(done.branch_deleted, None);
        assert!(!tree.exists());
        assert!(git(&repo, &["rev-parse", "--verify", "-q", "refs/heads/develop"]).is_ok(), "develop is kept");
        std::fs::remove_dir_all(dir).ok();
    }

    /// Build output counts as nothing to lose only as a whole ignored folder; files inside a folder
    /// that happens to share its name do not.
    #[test]
    fn only_whole_build_folders_are_rebuildable() {
        assert!(is_rebuildable("target/"));
        assert!(is_rebuildable("app/node_modules/"));
        assert!(is_rebuildable(".DS_Store"));
        assert!(is_rebuildable("logs/server.log"));
        assert!(!is_rebuildable("build/local.properties"));
        assert!(!is_rebuildable("out/secrets.json"));
        assert!(!is_rebuildable(".env.local"));
        assert!(!is_rebuildable(".vscode/"));
        // Names that are only maybe build output are never rebuildable as a whole folder.
        for folder in ["build/", "out/", "dist/", "coverage/", ".cache/", "app/build/"] {
            assert!(!is_rebuildable(folder), "{folder}");
            assert!(is_maybe_rebuildable_folder(folder), "{folder}");
        }
        assert!(!is_maybe_rebuildable_folder("target/") && !is_maybe_rebuildable_folder("build"));
    }

    /// A repository with one commit on `main` and a fake identity, in a fresh `dir`.
    fn test_repo(dir: &Path) -> PathBuf {
        let _ = std::fs::remove_dir_all(dir);
        let repo = dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]).unwrap();
        for (key, value) in [("user.email", "it@example.invalid"), ("user.name", "Tester"), ("commit.gpgsign", "false")] {
            git(&repo, &["config", key, value]).unwrap();
        }
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
        git(&repo, &["add", "a.txt"]).unwrap();
        git(&repo, &["commit", "-q", "-m", "first"]).unwrap();
        repo
    }

    /// A branch pushed with `-u` but not merged into the default branch: `git branch -d` would take it
    /// (it is merged into its own upstream), and deleting the remote copy next would lose its commits.
    /// Both stay; once merged into `main`, both go.
    #[test]
    fn a_pushed_but_unmerged_branch_keeps_both_copies() {
        if crate::process::command("git").arg("--version").output().is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("agentty-pushed-unmerged-{}", std::process::id()));
        let repo = test_repo(&dir);
        let origin = dir.join("origin.git");
        git(&repo, &["init", "-q", "--bare", &origin.to_string_lossy()]).unwrap();
        git(&repo, &["remote", "add", "origin", &origin.to_string_lossy()]).unwrap();
        let has = |at: &Path, name: &str| git(at, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{name}")]).is_ok();
        let add = |name: &str, new: bool| {
            let path = dir.join(name.replace('/', "-"));
            let text = path.to_string_lossy().to_string();
            if new {
                git(&repo, &["worktree", "add", "-q", "-b", name, &text]).unwrap();
                std::fs::write(path.join("u.txt"), "work\n").unwrap();
                git(&path, &["add", "u.txt"]).unwrap();
                git(&path, &["commit", "-q", "-m", "work"]).unwrap();
                git(&path, &["push", "-q", "-u", "origin", name]).unwrap();
            } else {
                git(&repo, &["worktree", "add", "-q", &text, name]).unwrap();
            }
            path
        };

        let tree = add("u", true);
        let removal = remove_linked(&repo, &tree, true, true).unwrap();
        assert_eq!(removal, Removal { branch_kept: Some("u".into()), ..Removal::default() });
        assert!(!tree.exists());
        assert!(has(&repo, "u"), "the local branch is kept");
        assert!(has(&origin, "u"), "the remote branch is kept");

        // An `agentty/…` branch in the same state is not deleted when the tree goes either.
        let session = add("agentty/s", true);
        assert_eq!(remove_linked(&repo, &session, false, false).unwrap(), Removal::default());
        assert!(has(&repo, "agentty/s"), "an unmerged session branch is kept");

        // Merged into the default branch: the local branch and its remote copy go.
        git(&repo, &["merge", "-q", "--no-edit", "u"]).unwrap();
        let tree = add("u", false);
        let removal = remove_linked(&repo, &tree, true, true).unwrap();
        assert_eq!(removal.branch_kept, None);
        assert_eq!(removal.remote_deleted.as_deref(), Some("origin/u"));
        assert!(!has(&repo, "u") && !has(&origin, "u"));
        git(&repo, &["merge", "-q", "--no-edit", "agentty/s"]).unwrap();
        let session = add("agentty/s", false);
        remove_linked(&repo, &session, false, false).unwrap();
        assert!(!has(&repo, "agentty/s"), "a merged session branch goes with its tree");

        // Without a default branch nothing counts as merged: the branch stays.
        git(&repo, &["branch", "-q", "-m", "main", "develop"]).unwrap();
        git(&repo, &["branch", "-q", "v", "develop"]).unwrap();
        let tree = add("v", false);
        assert_eq!(remove_linked(&repo, &tree, true, true).unwrap().branch_kept.as_deref(), Some("v"));
        assert!(has(&repo, "v"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A folder whose name is only maybe build output (`build/`, `out/`, …), ignored whole, is judged
    /// by the files in it: somebody's file makes the tree unsafe to clean up; logs and real build
    /// output inside it do not.
    #[test]
    fn files_in_an_ignored_build_folder_count() {
        if crate::process::command("git").arg("--version").output().is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("agentty-ignored-build-{}", std::process::id()));
        let repo = test_repo(&dir);
        std::fs::write(repo.join(".gitignore"), "build/\nout/\ndist/\ntarget/\n").unwrap();
        git(&repo, &["add", ".gitignore"]).unwrap();
        git(&repo, &["commit", "-q", "-m", "ignore"]).unwrap();
        let tree = dir.join("tree");
        git(&repo, &["worktree", "add", "-q", "-b", "t", &tree.to_string_lossy()]).unwrap();

        // Only logs and a nested dependency folder: nothing to lose.
        std::fs::create_dir_all(tree.join("dist/node_modules/pkg")).unwrap();
        std::fs::write(tree.join("dist/node_modules/pkg/index.js"), "x").unwrap();
        std::fs::write(tree.join("dist/build.log"), "x").unwrap();
        std::fs::create_dir_all(tree.join("target/debug")).unwrap();
        std::fs::write(tree.join("target/debug/app"), "bin").unwrap();
        assert!(cleanup_check(&tree).unwrap().is_safe());

        // Somebody's file in `build/` (and in a nested `out/`): named, not safe, kept.
        std::fs::create_dir_all(tree.join("build")).unwrap();
        std::fs::write(tree.join("build/local.properties"), "sdk.dir=/example\n").unwrap();
        std::fs::create_dir_all(tree.join("app/out")).unwrap();
        std::fs::write(tree.join("app/out/notes.txt"), "x").unwrap();
        let check = cleanup_check(&tree).unwrap();
        assert_eq!(check.ignored_files, vec!["app/out/notes.txt".to_string(), "build/local.properties".to_string()]);
        assert!(!check.is_safe());
        assert!(clean_up(&tree, false).is_err());
        assert!(tree.join("build/local.properties").exists());
        // Asked from a folder inside the tree, the same.
        assert_eq!(cleanup_check(&tree.join("app")).unwrap().ignored_files.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `.` and `..` go by name; nothing else changes (no symbolic link is followed, no `\\?\` prefix).
    #[test]
    fn resolves_dots_by_name() {
        assert_eq!(lexical(Path::new("/code/app/.git/worktrees/x/../..")), PathBuf::from("/code/app/.git"));
        assert_eq!(lexical(Path::new("/a/./b/../c")), PathBuf::from("/a/c"));
        assert_eq!(lexical(Path::new("/a/b")), PathBuf::from("/a/b"));
    }

    /// The project folder of a linked worktree is the main tree it came from, found without git.
    #[test]
    fn main_tree_of_linked_worktrees() {
        if crate::process::command("git").arg("--version").output().is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("agentty-main-tree-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let repo = dir.join("repo");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]).unwrap();
        git(&repo, &["-c", "user.email=t@example.com", "-c", "user.name=Tester", "commit", "-q", "--allow-empty", "-m", "first"]).unwrap();
        let linked = dir.join("linked");
        git(&repo, &["worktree", "add", "-q", "-b", "side", &linked.to_string_lossy()]).unwrap();
        std::fs::create_dir_all(linked.join("deep")).unwrap();
        let main = |path: &Path| main_tree(path).map(|p| p.canonicalize().unwrap());
        let repo = repo.canonicalize().unwrap();
        assert_eq!(main(&repo.join("src")), Some(repo.clone()));
        assert_eq!(main(&linked), Some(repo.clone()));
        assert_eq!(main(&linked.join("deep")), Some(repo.clone()));
        assert_eq!(main(&dir), None);
        assert!(is_linked(&linked) && !is_linked(&repo));
        // A submodule's `.git` file names a folder without `commondir`: no linked worktree.
        let module = dir.join("module");
        std::fs::create_dir_all(repo.join(".git/modules/module")).unwrap();
        std::fs::create_dir_all(&module).unwrap();
        std::fs::write(module.join(".git"), format!("gitdir: {}\n", repo.join(".git/modules/module").display())).unwrap();
        assert!(!is_linked(&module));
        assert_eq!(main_tree(&module), None);
        std::fs::remove_dir_all(dir).ok();
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

    /// A merged, clean tree is cleaned up at once; one with changes or unmerged commits only when
    /// forced — and the check says which, so the dialog can say what would be lost.
    #[test]
    fn cleans_up_merged_trees_and_forces_only_when_asked() {
        if crate::process::command("git").arg("--version").output().is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("agentty-worktree-cleanup-{}", std::process::id()));
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
        let commit = |tree: &Path, file: &str| {
            std::fs::write(tree.join(file), "work\n").unwrap();
            git(tree, &["add", file]).unwrap();
            git(tree, &["commit", "-q", "-m", file]).unwrap();
        };
        let has_branch = |name: &str| git(&repo, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{name}")]).is_ok();
        assert_eq!(linked_count(&repo), 0);

        // Nothing on it the default branch lacks: safe, and it goes with its branch.
        let merged = add("merged");
        assert_eq!(linked_count(&repo), 1);
        // A tree whose folder was deleted by hand is not counted (the menu doesn't list it either).
        let gone = add("gone");
        assert_eq!(linked_count(&repo), 2);
        std::fs::remove_dir_all(&gone).unwrap();
        assert_eq!(linked_count(&repo), 1);
        git(&repo, &["worktree", "prune"]).unwrap();
        git(&repo, &["branch", "-D", "gone"]).unwrap();
        let check = cleanup_check(&merged).unwrap();
        assert_eq!(check.branch.as_deref(), Some("merged"));
        assert_eq!(check.default_branch.as_deref(), Some("main"));
        assert!(check.is_safe());
        let done = clean_up(&merged, false).unwrap();
        assert_eq!(done.main.canonicalize().unwrap(), repo.canonicalize().unwrap());
        assert_eq!(done.branch_deleted.as_deref(), Some("merged"));
        assert!(!merged.exists() && !has_branch("merged"));

        // Work merged into main afterwards counts as merged too.
        let landed = add("landed");
        commit(&landed, "landed.txt");
        assert_eq!(cleanup_check(&landed).unwrap().unmerged_commits, 1);
        git(&repo, &["merge", "-q", "--no-edit", "landed"]).unwrap();
        assert!(cleanup_check(&landed).unwrap().is_safe());
        clean_up(&landed, false).unwrap();
        assert!(!has_branch("landed"));

        // A file git ignores (a local `.env`) would go with the tree: not safe. Build output would not matter.
        std::fs::write(repo.join(".gitignore"), ".env.local\ntarget/\n").unwrap();
        git(&repo, &["add", ".gitignore"]).unwrap();
        git(&repo, &["commit", "-q", "-m", "ignore"]).unwrap();
        let local = add("local");
        std::fs::create_dir_all(local.join("target/debug")).unwrap();
        std::fs::write(local.join("target/debug/app"), "bin").unwrap();
        assert!(cleanup_check(&local).unwrap().is_safe(), "build output alone is nothing to lose");
        std::fs::write(local.join(".env.local"), "SECRET=example_not_a_real_value\n").unwrap();
        let check = cleanup_check(&local).unwrap();
        assert_eq!(check.ignored_files, vec![".env.local".to_string()]);
        assert!(!check.is_safe());
        assert!(clean_up(&local, false).is_err());
        clean_up(&local, true).unwrap();

        // Uncommitted (even untracked) files: not safe, refused without force, gone with it.
        let dirty = add("dirty");
        std::fs::write(dirty.join("scratch.txt"), "x\n").unwrap();
        let check = cleanup_check(&dirty).unwrap();
        assert_eq!((check.changed_files, check.unmerged_commits), (1, 0));
        assert!(!check.is_safe());
        assert!(clean_up(&dirty, false).is_err());
        assert!(dirty.exists() && has_branch("dirty"), "a refused clean-up leaves everything as it was");
        clean_up(&dirty, true).unwrap();
        assert!(!dirty.exists() && !has_branch("dirty"));

        // Unmerged commits: the same, and the forced clean-up deletes the branch as well.
        let ahead = add("ahead");
        commit(&ahead, "b.txt");
        commit(&ahead, "c.txt");
        let check = cleanup_check(&ahead).unwrap();
        assert_eq!((check.changed_files, check.unmerged_commits), (0, 2));
        assert!(clean_up(&ahead, false).is_err());
        clean_up(&ahead, true).unwrap();
        assert!(!ahead.exists() && !has_branch("ahead"));

        // Merged on the remote (a pull request landed) while the local main was not pulled: merged.
        let pushed = add("pushed");
        commit(&pushed, "d.txt");
        git(&repo, &["update-ref", "refs/remotes/origin/main", "refs/heads/pushed"]).unwrap();
        git(&repo, &["symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/main"]).unwrap();
        let check = cleanup_check(&pushed).unwrap();
        assert_eq!(check.default_branch.as_deref(), Some("origin/main"));
        assert!(check.is_safe());
        assert_eq!(clean_up(&pushed, false).unwrap().branch_deleted.as_deref(), Some("pushed"));

        // A linked tree on the default branch itself: the tree goes, the branch never does.
        git(&repo, &["checkout", "-q", "-b", "elsewhere"]).unwrap();
        let on_main = dir.join("on-main");
        git(&repo, &["worktree", "add", "-q", &on_main.to_string_lossy(), "main"]).unwrap();
        assert_eq!(clean_up(&on_main, true).unwrap().branch_deleted, None);
        assert!(has_branch("main"));

        // The project's own tree: never.
        assert!(clean_up(&repo, true).is_err());
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
        assert!(create_in(&unborn, "claude", None, &data).is_err());
        assert_eq!(std::fs::read_dir(&data).unwrap().count(), 0, "nothing is left in the worktree folder");
        let _ = std::fs::remove_dir_all(&unborn);

        let tree = create_in(&repo, "claude", None, &data).unwrap();
        assert!(tree.managed && tree.path.join("a.txt").exists());
        assert!(tree.branch.as_deref().is_some_and(|b| b.starts_with("agentty/claude-")));
        let listed = list_in(&repo, &data).unwrap();
        assert_eq!(listed.len(), 2);
        assert!(listed[0].main && listed[1].managed);
        assert_eq!(commits_ahead(&tree.path, &listed[0].head), 0);
        // Without a remote the project folder is the base; with one, the remote's default branch,
        // so a project folder that was not pulled does not make every tree look ahead.
        assert_eq!(ahead_base(&repo, &listed[0].head), listed[0].head);
        git(&repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]).unwrap();
        git(&repo, &["symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/main"]).unwrap();
        let remote_head = git(&repo, &["rev-parse", "HEAD"]).unwrap().trim().to_string();
        assert_eq!(ahead_base(&repo, "0000000"), remote_head);

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
        let tree = create_in(&repo, "codex", None, &data).unwrap();
        assert!(tree.path.join("a.txt").exists() && !tree.path.join("feature.txt").exists());
        remove_in(&tree.path, true, &data).unwrap();
        // Named, the base is where the branch starts: a chat's workers build on its integration branch.
        let tree = create_in(&repo, "worker", Some("feature"), &data).unwrap();
        assert!(tree.path.join("feature.txt").exists());
        remove_in(&tree.path, true, &data).unwrap();

        for dir in [&data, &repo] {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}
