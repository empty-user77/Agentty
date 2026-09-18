//! Git operations for the Git page, via the `git` CLI (the user's own git, config and credentials).
//! Every command runs non-interactively (`GIT_TERMINAL_PROMPT=0`) so it can never hang on a prompt.

use anyhow::{bail, ensure, Context, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Stdio;

fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let output = crate::process::command("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
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

pub fn repo_root(path: &Path) -> Option<PathBuf> {
    git(path, &["rev-parse", "--show-toplevel"]).ok().map(|s| PathBuf::from(s.trim())).filter(|p| p.is_dir())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileChange {
    pub path: String,
    /// Previous path for renames and copies.
    pub original: Option<String>,
    /// One of `M`, `A`, `D`, `R`, `C`, `U` (conflict) or `?` (untracked).
    pub kind: char,
    pub staged: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RepoStatus {
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub files: Vec<FileChange>,
}

/// Parses `git status --porcelain=v2 --branch -z`.
pub fn parse_status(raw: &str) -> RepoStatus {
    let mut status = RepoStatus::default();
    let mut entries = raw.split('\0').peekable();
    while let Some(entry) = entries.next() {
        if let Some(rest) = entry.strip_prefix("# branch.head ") {
            status.branch = (rest != "(detached)").then(|| rest.to_string());
        } else if let Some(rest) = entry.strip_prefix("# branch.upstream ") {
            status.upstream = Some(rest.to_string());
        } else if let Some(rest) = entry.strip_prefix("# branch.ab ") {
            for part in rest.split_whitespace() {
                if let Some(n) = part.strip_prefix('+') {
                    status.ahead = n.parse().unwrap_or(0);
                } else if let Some(n) = part.strip_prefix('-') {
                    status.behind = n.parse().unwrap_or(0);
                }
            }
        } else if let Some(rest) = entry.strip_prefix("1 ") {
            // 1 XY sub mH mI mW hH hI path
            let fields: Vec<&str> = rest.splitn(8, ' ').collect();
            if let (Some(xy), Some(path)) = (fields.first(), fields.get(7)) {
                status.files.push(change(xy, path, None));
            }
        } else if let Some(rest) = entry.strip_prefix("2 ") {
            // 2 XY sub mH mI mW hH hI Xscore path \0 origPath
            let fields: Vec<&str> = rest.splitn(9, ' ').collect();
            if let (Some(xy), Some(path)) = (fields.first(), fields.get(8)) {
                let original = entries.next().map(str::to_string);
                status.files.push(change(xy, path, original));
            }
        } else if let Some(rest) = entry.strip_prefix("u ") {
            let fields: Vec<&str> = rest.splitn(10, ' ').collect();
            if let Some(path) = fields.get(9) {
                status.files.push(FileChange { path: path.to_string(), original: None, kind: 'U', staged: false });
            }
        } else if let Some(path) = entry.strip_prefix("? ") {
            status.files.push(FileChange { path: path.to_string(), original: None, kind: '?', staged: false });
        }
    }
    status.files.sort_by(|a, b| a.path.cmp(&b.path));
    status
}

fn change(xy: &str, path: &str, original: Option<String>) -> FileChange {
    let mut chars = xy.chars();
    let (index, worktree) = (chars.next().unwrap_or('.'), chars.next().unwrap_or('.'));
    let kind = if worktree != '.' { worktree } else { index };
    FileChange { path: path.to_string(), original, kind, staged: index != '.' }
}

pub fn status(repo: &Path) -> Result<RepoStatus> {
    Ok(parse_status(&git(repo, &["status", "--porcelain=v2", "--branch", "-z", "--untracked-files=all"])?))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum LineKind {
    Hunk,
    Context,
    Added,
    Removed,
    Meta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiffLine {
    pub kind: LineKind,
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub text: String,
}

/// Parses unified diff output into numbered lines (file headers are dropped).
pub fn parse_diff(raw: &str) -> Vec<DiffLine> {
    let mut lines = Vec::new();
    let (mut old, mut new) = (0u32, 0u32);
    let mut in_hunk = false;
    for line in raw.lines() {
        if let Some(header) = line.strip_prefix("@@") {
            // @@ -a,b +c,d @@ context
            let mut parts = header.split_whitespace();
            old = parts.next().and_then(|p| p.trim_start_matches('-').split(',').next()?.parse().ok()).unwrap_or(0);
            new = parts.next().and_then(|p| p.trim_start_matches('+').split(',').next()?.parse().ok()).unwrap_or(0);
            in_hunk = true;
            lines.push(DiffLine { kind: LineKind::Hunk, old: None, new: None, text: line.to_string() });
        } else if !in_hunk {
            if line.starts_with("Binary files") {
                lines.push(DiffLine { kind: LineKind::Meta, old: None, new: None, text: line.to_string() });
            }
        } else if let Some(text) = line.strip_prefix('+') {
            lines.push(DiffLine { kind: LineKind::Added, old: None, new: Some(new), text: text.to_string() });
            new += 1;
        } else if let Some(text) = line.strip_prefix('-') {
            lines.push(DiffLine { kind: LineKind::Removed, old: Some(old), new: None, text: text.to_string() });
            old += 1;
        } else if let Some(text) = line.strip_prefix(' ') {
            lines.push(DiffLine { kind: LineKind::Context, old: Some(old), new: Some(new), text: text.to_string() });
            old += 1;
            new += 1;
        } else if line.starts_with('\\') {
            lines.push(DiffLine { kind: LineKind::Meta, old: None, new: None, text: line.to_string() });
        } else if line.starts_with("diff --git") {
            in_hunk = false;
        }
    }
    lines
}

/// Working tree diff for one file (against HEAD, including staged changes).
pub fn working_diff(repo: &Path, file: &FileChange) -> Result<Vec<DiffLine>> {
    if file.kind == '?' {
        // Untracked: show the whole file as added (no-index exits 1 when files differ).
        let path = repo.join(&file.path);
        let meta = std::fs::metadata(&path)?;
        ensure!(meta.len() <= 2 * 1024 * 1024, "file is too large to preview");
        let bytes = std::fs::read(&path)?;
        if bytes.contains(&0) {
            return Ok(vec![DiffLine { kind: LineKind::Meta, old: None, new: None, text: "Binary file".into() }]);
        }
        let text = String::from_utf8_lossy(&bytes);
        let count = text.lines().count();
        let mut lines = vec![DiffLine { kind: LineKind::Hunk, old: None, new: None, text: format!("@@ -0,0 +1,{count} @@") }];
        lines.extend(text.lines().enumerate().map(|(i, l)| DiffLine {
            kind: LineKind::Added,
            old: None,
            new: Some(i as u32 + 1),
            text: l.to_string(),
        }));
        return Ok(lines);
    }
    let has_head = git(repo, &["rev-parse", "--verify", "-q", "HEAD"]).is_ok();
    let raw = if has_head {
        git(repo, &["diff", "--no-color", "--no-ext-diff", "HEAD", "--", &file.path])?
    } else {
        git(repo, &["diff", "--no-color", "--no-ext-diff", "--cached", "--", &file.path])?
    };
    Ok(parse_diff(&raw))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Commit {
    pub sha: String,
    pub short: String,
    pub author: String,
    pub email: String,
    pub time: i64,
    pub summary: String,
    pub body: String,
}

pub fn log(repo: &Path, limit: usize) -> Result<Vec<Commit>> {
    let limit = limit.to_string();
    let raw = match git(repo, &["log", "-n", &limit, "--format=%H%x1f%h%x1f%an%x1f%ae%x1f%at%x1f%s%x1f%b%x1e"]) {
        Ok(raw) => raw,
        Err(err) if err.to_string().contains("does not have any commits") => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    Ok(raw
        .split('\u{1e}')
        .filter_map(|record| {
            let f: Vec<&str> = record.trim_start_matches('\n').split('\u{1f}').collect();
            (f.len() >= 7).then(|| Commit {
                sha: f[0].to_string(),
                short: f[1].to_string(),
                author: f[2].to_string(),
                email: f[3].to_string(),
                time: f[4].parse().unwrap_or(0),
                summary: f[5].to_string(),
                body: f[6].trim().to_string(),
            })
        })
        .collect())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommitFile {
    pub path: String,
    pub kind: char,
    pub additions: Option<u32>,
    pub deletions: Option<u32>,
}

fn valid_sha(sha: &str) -> bool {
    (4..=64).contains(&sha.len()) && sha.chars().all(|c| c.is_ascii_hexdigit())
}

pub fn commit_files(repo: &Path, sha: &str) -> Result<Vec<CommitFile>> {
    ensure!(valid_sha(sha), "invalid commit id");
    let names = git(repo, &["show", "--format=", "--name-status", "--no-renames", sha])?;
    let stats = git(repo, &["show", "--format=", "--numstat", "--no-renames", sha])?;
    let mut files: Vec<CommitFile> = names
        .lines()
        .filter_map(|l| {
            let (kind, path) = l.split_once('\t')?;
            Some(CommitFile { path: path.to_string(), kind: kind.chars().next().unwrap_or('M'), additions: None, deletions: None })
        })
        .collect();
    for line in stats.lines() {
        let mut parts = line.splitn(3, '\t');
        let (add, del, path) = (parts.next(), parts.next(), parts.next());
        if let Some(file) = files.iter_mut().find(|f| Some(f.path.as_str()) == path) {
            file.additions = add.and_then(|a| a.parse().ok());
            file.deletions = del.and_then(|d| d.parse().ok());
        }
    }
    Ok(files)
}

pub fn commit_diff(repo: &Path, sha: &str, path: &str) -> Result<Vec<DiffLine>> {
    ensure!(valid_sha(sha), "invalid commit id");
    Ok(parse_diff(&git(repo, &["show", "--format=", "--no-color", "--no-ext-diff", "--no-renames", sha, "--", path])?))
}

/// Stages exactly `paths` and commits them. Returns the new short sha.
pub fn commit(repo: &Path, summary: &str, description: &str, paths: &[String]) -> Result<String> {
    let summary = summary.trim();
    ensure!(!summary.is_empty(), "a commit summary is required");
    ensure!(!paths.is_empty(), "select at least one file");
    // Unstage anything the user did not select, then stage the selection.
    if git(repo, &["rev-parse", "--verify", "-q", "HEAD"]).is_ok() {
        git(repo, &["reset", "-q"])?;
    }
    let mut add = vec!["add", "-A", "--"];
    add.extend(paths.iter().map(String::as_str));
    git(repo, &add)?;
    let mut args = vec!["commit", "-q", "-m", summary];
    let description = description.trim();
    if !description.is_empty() {
        args.extend(["-m", description]);
    }
    git(repo, &args)?;
    Ok(git(repo, &["rev-parse", "--short", "HEAD"])?.trim().to_string())
}

/// Reverts a file to HEAD (or deletes it if untracked).
pub fn discard(repo: &Path, file: &FileChange) -> Result<()> {
    if file.kind == '?' {
        let path = repo.join(&file.path);
        ensure!(path.starts_with(repo), "refusing to delete outside the repository");
        std::fs::remove_file(path)?;
    } else {
        git(repo, &["restore", "--staged", "--worktree", "--source=HEAD", "--", &file.path])?;
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Branch {
    pub name: String,
    pub remote: bool,
    pub current: bool,
    pub time: i64,
}

pub fn branches(repo: &Path) -> Result<Vec<Branch>> {
    let raw = git(
        repo,
        &["for-each-ref", "--sort=-committerdate", "--format=%(refname)%1f%(HEAD)%1f%(committerdate:unix)", "refs/heads", "refs/remotes"],
    )?;
    Ok(raw
        .lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split('\u{1f}').collect();
            let refname = *f.first()?;
            let (name, remote) =
                if let Some(n) = refname.strip_prefix("refs/heads/") { (n, false) } else { (refname.strip_prefix("refs/remotes/")?, true) };
            (!name.ends_with("/HEAD")).then(|| Branch {
                name: name.to_string(),
                remote,
                current: f.get(1) == Some(&"*"),
                time: f.get(2).and_then(|t| t.parse().ok()).unwrap_or(0),
            })
        })
        .collect())
}

/// Branch names on the remotes matching `query` (case-insensitive), as `remote/name`, straight
/// from the servers (`git ls-remote`), so branches that were never fetched are found too.
pub fn search_remote_branches(repo: &Path, query: &str) -> Result<Vec<String>> {
    let query = query.to_lowercase();
    let mut found = Vec::new();
    for remote in git(repo, &["remote"])?.lines().map(str::trim).filter(|r| !r.is_empty()) {
        let Ok(raw) = git(repo, &["ls-remote", "--heads", remote]) else { continue };
        found.extend(parse_ls_remote(&raw, remote, &query));
    }
    Ok(found)
}

fn parse_ls_remote(raw: &str, remote: &str, query: &str) -> Vec<String> {
    raw.lines()
        .filter_map(|line| line.split_whitespace().nth(1)?.strip_prefix("refs/heads/"))
        .filter(|name| name.to_lowercase().contains(query))
        .map(|name| format!("{remote}/{name}"))
        .collect()
}

pub fn default_branch(repo: &Path) -> Option<String> {
    git(repo, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"]).ok().map(|s| s.trim().trim_start_matches("origin/").to_string())
}

fn valid_branch_name(repo: &Path, name: &str) -> bool {
    !name.starts_with('-') && git(repo, &["check-ref-format", "--branch", name]).is_ok()
}

pub fn checkout(repo: &Path, branch: &str, remote: bool) -> Result<()> {
    ensure!(valid_branch_name(repo, branch), "invalid branch name");
    if remote {
        // origin/feature → local tracking branch "feature".
        let (remote_name, local) = branch.split_once('/').unwrap_or(("origin", branch));
        // Found by a remote search but never fetched: fetch just that branch first.
        if git(repo, &["rev-parse", "--verify", "-q", &format!("refs/remotes/{branch}")]).is_err() {
            git(repo, &["fetch", remote_name, &format!("refs/heads/{local}:refs/remotes/{remote_name}/{local}")])?;
        }
        if git(repo, &["rev-parse", "--verify", "-q", &format!("refs/heads/{local}")]).is_ok() {
            git(repo, &["switch", local])?;
        } else {
            git(repo, &["switch", "--track", branch])?;
        }
    } else {
        git(repo, &["switch", branch])?;
    }
    Ok(())
}

pub fn create_branch(repo: &Path, name: &str) -> Result<()> {
    ensure!(valid_branch_name(repo, name), "invalid branch name");
    git(repo, &["switch", "-c", name])?;
    Ok(())
}

/// Merges `branch` into the current branch (aborting and reporting on conflicts).
pub fn merge(repo: &Path, branch: &str) -> Result<()> {
    ensure!(valid_branch_name(repo, branch), "invalid branch name");
    if let Err(err) = git(repo, &["merge", "--no-edit", branch]) {
        let _ = git(repo, &["merge", "--abort"]);
        return Err(err);
    }
    Ok(())
}

pub fn fetch(repo: &Path) -> Result<()> {
    git(repo, &["fetch", "--prune", "origin"]).map(|_| ())
}

pub fn pull(repo: &Path) -> Result<()> {
    git(repo, &["pull", "--ff-only"]).map(|_| ())
}

pub fn push(repo: &Path, branch: &str, has_upstream: bool) -> Result<()> {
    ensure!(valid_branch_name(repo, branch), "invalid branch name");
    if has_upstream {
        git(repo, &["push"]).map(|_| ())
    } else {
        git(repo, &["push", "-u", "origin", branch]).map(|_| ())
    }
}

/// Unix seconds of the last fetch (mtime of FETCH_HEAD).
pub fn last_fetch(repo: &Path) -> Option<i64> {
    let git_dir = git(repo, &["rev-parse", "--absolute-git-dir"]).ok()?;
    let modified = std::fs::metadata(Path::new(git_dir.trim()).join("FETCH_HEAD")).ok()?.modified().ok()?;
    modified.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs() as i64)
}

pub fn has_remote(repo: &Path) -> bool {
    git(repo, &["remote"]).map(|r| r.lines().any(|l| l == "origin")).unwrap_or(false)
}

/// Browser URL for `origin` (GitHub, GitLab, … over https or ssh).
pub fn remote_web_url(repo: &Path) -> Option<String> {
    let url = git(repo, &["remote", "get-url", "origin"]).ok()?;
    let url = url.trim().trim_end_matches(".git");
    if let Some(rest) = url.strip_prefix("git@") {
        let (host, path) = rest.split_once(':')?;
        return Some(format!("https://{host}/{path}"));
    }
    if let Some(rest) = url.strip_prefix("ssh://git@") {
        return Some(format!("https://{}", rest.replacen(':', "/", 1)));
    }
    (url.starts_with("https://") || url.starts_with("http://")).then(|| {
        // Drop credentials embedded in the URL.
        match url::Url::parse(url) {
            Ok(mut parsed) => {
                let _ = parsed.set_username("");
                let _ = parsed.set_password(None);
                parsed.to_string().trim_end_matches('/').to_string()
            }
            Err(_) => url.to_string(),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_repo(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("agentty-git-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]).unwrap();
        git(&dir, &["config", "user.email", "t@example.com"]).unwrap();
        git(&dir, &["config", "user.name", "Tester"]).unwrap();
        git(&dir, &["config", "commit.gpgsign", "false"]).unwrap();
        dir
    }

    #[test]
    fn parses_porcelain_v2() {
        let raw = "# branch.oid abc\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +2 -1\0\
1 .M N... 100644 100644 100644 a b src/lib.rs\0\
2 R. N... 100644 100644 100644 a b R100 new name.rs\0old.rs\0\
? notes.md\0";
        let status = parse_status(raw);
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert_eq!((status.ahead, status.behind), (2, 1));
        assert_eq!(status.files.len(), 3);
        let renamed = status.files.iter().find(|f| f.kind == 'R').unwrap();
        assert_eq!((renamed.path.as_str(), renamed.original.as_deref(), renamed.staged), ("new name.rs", Some("old.rs"), true));
        assert!(status.files.iter().any(|f| f.path == "notes.md" && f.kind == '?'));
    }

    #[test]
    fn parses_unified_diff_line_numbers() {
        let raw = "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -3,3 +3,4 @@ fn\n a\n-b\n+c\n+d\n e\n";
        let lines = parse_diff(raw);
        assert_eq!(lines[0].kind, LineKind::Hunk);
        assert_eq!((lines[1].old, lines[1].new), (Some(3), Some(3)));
        assert_eq!((lines[2].kind, lines[2].old), (LineKind::Removed, Some(4)));
        assert_eq!((lines[3].kind, lines[3].new), (LineKind::Added, Some(4)));
        assert_eq!((lines[5].old, lines[5].new), (Some(5), Some(6)));
    }

    #[test]
    fn commit_history_and_branches_roundtrip() {
        let repo = temp_repo("flow");
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
        std::fs::write(repo.join("b.txt"), "two\n").unwrap();
        let status = status(&repo).unwrap();
        assert_eq!(status.files.iter().filter(|f| f.kind == '?').count(), 2);
        let untracked = status.files.iter().find(|f| f.path == "a.txt").unwrap();
        assert_eq!(working_diff(&repo, untracked).unwrap()[1].text, "one");

        // Only the selected file is committed.
        let sha = commit(&repo, "feat: add a", "body text", &["a.txt".to_string()]).unwrap();
        assert!(!sha.is_empty());
        let status = super::status(&repo).unwrap();
        assert_eq!(status.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["b.txt"]);

        std::fs::write(repo.join("a.txt"), "one\nmore\n").unwrap();
        let modified = super::status(&repo).unwrap().files.into_iter().find(|f| f.path == "a.txt").unwrap();
        assert_eq!(modified.kind, 'M');
        assert!(working_diff(&repo, &modified).unwrap().iter().any(|l| l.kind == LineKind::Added && l.text == "more"));
        discard(&repo, &modified).unwrap();
        assert!(super::status(&repo).unwrap().files.iter().all(|f| f.path != "a.txt"));

        let history = log(&repo, 10).unwrap();
        assert_eq!(
            (history[0].summary.as_str(), history[0].body.as_str(), history[0].author.as_str()),
            ("feat: add a", "body text", "Tester")
        );
        let files = commit_files(&repo, &history[0].sha).unwrap();
        assert_eq!((files[0].path.as_str(), files[0].kind, files[0].additions), ("a.txt", 'A', Some(1)));
        assert!(commit_diff(&repo, &history[0].sha, "a.txt").unwrap().iter().any(|l| l.text == "one"));
        assert!(commit_files(&repo, "--help").is_err());

        create_branch(&repo, "feature/x").unwrap();
        assert!(create_branch(&repo, "-bad").is_err());
        assert!(create_branch(&repo, "bad..name").is_err());
        let list = branches(&repo).unwrap();
        assert!(list.iter().any(|b| b.name == "feature/x" && b.current));
        std::fs::write(repo.join("c.txt"), "three\n").unwrap();
        commit(&repo, "feat: c", "", &["c.txt".to_string()]).unwrap();
        checkout(&repo, "main", false).unwrap();
        merge(&repo, "feature/x").unwrap();
        assert!(repo.join("c.txt").exists());
        assert_eq!(super::status(&repo).unwrap().branch.as_deref(), Some("main"));
        std::fs::remove_dir_all(repo).ok();
    }

    #[test]
    fn remote_urls_become_web_urls() {
        let repo = temp_repo("remote");
        git(&repo, &["remote", "add", "origin", "git@github.com:empty-user77/agentty.git"]).unwrap();
        assert_eq!(remote_web_url(&repo).as_deref(), Some("https://github.com/empty-user77/agentty"));
        git(&repo, &["remote", "set-url", "origin", "https://user:token@github.com/a/b.git"]).unwrap();
        assert_eq!(remote_web_url(&repo).as_deref(), Some("https://github.com/a/b"));
        std::fs::remove_dir_all(repo).ok();
    }

    #[test]
    fn parses_remote_heads() {
        let raw = "a1b2\trefs/heads/main\nc3d4\trefs/heads/feature/Login-Page\n";
        assert_eq!(parse_ls_remote(raw, "origin", "login"), vec!["origin/feature/Login-Page".to_string()]);
        assert_eq!(parse_ls_remote(raw, "origin", "").len(), 2);
    }
}
