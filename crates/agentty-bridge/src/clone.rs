//! Cloning a repository into the folder a terminal is in: from an address the user pastes
//! (`https://…`, `git@host:owner/repo`, `ssh://…`, `git://…`), or from a GitHub repository found
//! through the `gh` CLI the user has already signed in to.
//!
//! Nothing here asks for or stores credentials: `git` uses the user's own (SSH keys, credential
//! helper) and `gh` its own login. Every command runs without a prompt it could hang on.

use crate::process;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Stdio;

/// A repository on GitHub, as `gh` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteRepo {
    /// `owner/name`.
    pub full_name: String,
    pub description: String,
    pub private: bool,
}

/// What to clone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// An address for `git clone`.
    Url(String),
    /// `owner/name` for `gh repo clone` (which picks the protocol the user set up in `gh`).
    GitHub(String),
}

impl Source {
    /// The folder the clone gets: the repository's own name.
    pub fn folder_name(&self) -> Option<String> {
        let path = match self {
            Source::Url(url) => url.trim_end_matches('/'),
            Source::GitHub(full_name) => full_name.as_str(),
        };
        let last = path.rsplit(['/', ':']).next()?;
        let name = last.strip_suffix(".git").unwrap_or(last);
        (!name.is_empty() && name != "." && name != ".." && name.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)))
            .then(|| name.to_string())
    }

    /// The same repository written the same way whatever the protocol: `host/owner/name`,
    /// lowercase, without `.git`. `None` for an address that names no repository.
    pub fn identity(&self) -> Option<String> {
        match self {
            Source::GitHub(full_name) => Some(format!("github.com/{}", full_name.to_lowercase())),
            Source::Url(url) => remote_identity(url),
        }
    }
}

/// Reads what the user typed: an address, or `owner/name` (a GitHub repository).
pub fn parse_source(text: &str) -> Result<Source> {
    let text = text.trim();
    if text.is_empty() {
        bail!("enter a repository address");
    }
    // Read as an option by git, whatever follows (`--upload-pack=…` runs a program).
    if text.starts_with('-') || text.chars().any(|c| c.is_whitespace() || c.is_control()) {
        bail!("that is not a repository address");
    }
    if is_full_name(text) {
        return Ok(Source::GitHub(text.to_string()));
    }
    let lower = text.to_ascii_lowercase();
    let scheme_ok = ["https://", "http://", "ssh://", "git://"].iter().any(|s| lower.starts_with(s));
    // scp-like `user@host:path`, never a local path or `ext::` transport.
    let scp_ok = !lower.contains("://")
        && text.split_once(':').is_some_and(|(host, path)| host.contains('@') && !host.contains('/') && !path.is_empty());
    if !(scheme_ok || scp_ok) || lower.starts_with("ext::") || lower.starts_with("file:") {
        bail!("use an https, ssh or git address");
    }
    let source = Source::Url(text.to_string());
    if source.folder_name().is_none() || source.identity().is_none() {
        bail!("the address names no repository");
    }
    Ok(source)
}

fn is_full_name(text: &str) -> bool {
    let valid = |part: &str| {
        !part.is_empty() && !part.starts_with(['.', '-']) && part.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
    };
    matches!(text.split_once('/'), Some((owner, name)) if valid(owner) && valid(name) && !name.contains('/'))
}

/// `host/owner/name` of a remote address, lowercase and without `.git` or credentials.
pub fn remote_identity(url: &str) -> Option<String> {
    let url = url.trim().trim_end_matches('/');
    let rest = match url.split_once("://") {
        Some((_, rest)) => rest.to_string(),
        // scp-like: `git@github.com:owner/repo`
        None => url.replacen(':', "/", 1),
    };
    let rest = rest.rsplit_once('@').map_or(rest.as_str(), |(_, after)| after);
    // A port after the host (`host:22/owner/repo`) is not part of which repository it is.
    let (host, path) = rest.split_once('/')?;
    let host = host.split(':').next().unwrap_or(host);
    let path = path.strip_suffix(".git").unwrap_or(path);
    (!host.is_empty() && !path.is_empty()).then(|| format!("{host}/{path}").to_lowercase())
}

/// Why `source` must not be cloned into `dir`: a folder of its name is there already, or a
/// clone of the same repository under another name.
pub fn already_there(dir: &Path, source: &Source) -> Option<PathBuf> {
    if let Some(name) = source.folder_name() {
        let target = dir.join(&name);
        if target.exists() {
            return Some(target);
        }
    }
    let identity = source.identity()?;
    if origin_identity(dir).as_deref() == Some(identity.as_str()) {
        return Some(dir.to_path_buf());
    }
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .find(|path| origin_identity(path).as_deref() == Some(identity.as_str()))
}

/// The `origin` remote of the repository at `dir`, read from its config file (no process per folder).
fn origin_identity(dir: &Path) -> Option<String> {
    let config = std::fs::read_to_string(dir.join(".git").join("config")).ok()?;
    let mut in_origin = false;
    for line in config.lines().map(str::trim) {
        if line.starts_with('[') {
            in_origin = line == "[remote \"origin\"]";
        } else if in_origin {
            if let Some(url) = line.strip_prefix("url").map(str::trim_start).and_then(|l| l.strip_prefix('=')) {
                return remote_identity(url.trim());
            }
        }
    }
    None
}

/// Clones `source` into `dir/<name>` and returns that folder.
pub fn clone(dir: &Path, source: &Source) -> Result<PathBuf> {
    if !dir.is_dir() {
        bail!("{} is not a folder", dir.display());
    }
    if let Some(existing) = already_there(dir, source) {
        bail!("already here: {}", existing.display());
    }
    let name = source.folder_name().context("the address names no repository")?;
    let mut command = match source {
        Source::Url(url) => {
            let mut command = process::command("git");
            command.args(["clone", "--", url, &name]);
            command
        }
        Source::GitHub(full_name) => {
            let mut command = process::command("gh");
            command.args(["repo", "clone", full_name, &name]);
            command
        }
    };
    let output = command
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        // The user's own ssh setup (core.sshCommand, agent) is left alone: without a terminal of
        // its own, ssh fails on an unknown host key instead of waiting for a "yes".
        .env("GH_PROMPT_DISABLED", "1")
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .output()
        .context(match source {
            Source::Url(_) => "git is not installed",
            Source::GitHub(_) => "gh is not installed",
        })?;
    if !output.status.success() {
        let reason = String::from_utf8_lossy(&output.stderr).trim().lines().last().unwrap_or("").to_string();
        bail!("{}", if reason.is_empty() { "the clone failed".to_string() } else { reason });
    }
    Ok(dir.join(name))
}

fn gh(args: &[&str]) -> Option<Vec<u8>> {
    let output = process::command("gh")
        .args(args)
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_PAGER", "cat")
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
}

/// Whether `gh` is installed and signed in.
pub fn gh_ready() -> bool {
    process::which("gh").is_some() && gh(&["auth", "status"]).is_some()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhRepo {
    #[serde(alias = "fullName")]
    name_with_owner: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default, alias = "isPrivate")]
    is_private: bool,
}

fn parse_repos(json: &[u8]) -> Vec<RemoteRepo> {
    serde_json::from_slice::<Vec<GhRepo>>(json)
        .unwrap_or_default()
        .into_iter()
        .filter(|r| is_full_name(&r.name_with_owner))
        .map(|r| RemoteRepo { full_name: r.name_with_owner, description: r.description.unwrap_or_default(), private: r.is_private })
        .collect()
}

/// The user's own repositories, most recently pushed first.
pub fn my_repos() -> Vec<RemoteRepo> {
    gh(&["repo", "list", "--limit", "100", "--json", "nameWithOwner,description,isPrivate"])
        .map(|out| parse_repos(&out))
        .unwrap_or_default()
}

/// Repositories on GitHub matching `query` (the user's own first, then everyone's).
pub fn search_repos(query: &str, mine: &[RemoteRepo]) -> Vec<RemoteRepo> {
    let query = query.trim();
    let needle = query.to_lowercase();
    let mut found: Vec<RemoteRepo> = mine.iter().filter(|r| r.full_name.to_lowercase().contains(&needle)).cloned().collect();
    // A query read as an option would change what `gh` does.
    if !query.is_empty() && !query.starts_with('-') {
        if let Some(out) = gh(&["search", "repos", "--limit", "20", "--json", "fullName,description,isPrivate", "--", query]) {
            for repo in parse_repos(&out) {
                if !found.iter().any(|r| r.full_name.eq_ignore_ascii_case(&repo.full_name)) {
                    found.push(repo);
                }
            }
        }
    }
    found.truncate(40);
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_every_kind_of_address() {
        assert_eq!(parse_source("owner/repo").unwrap(), Source::GitHub("owner/repo".into()));
        for url in ["https://github.com/o/r.git", "git@github.com:o/r.git", "ssh://git@host:22/o/r", "git://host/o/r"] {
            let source = parse_source(url).unwrap();
            assert_eq!(source.folder_name().as_deref(), Some("r"), "{url}");
        }
    }

    #[test]
    fn refuses_what_git_would_read_as_something_else() {
        for text in
            ["", "--upload-pack=touch x", "-c core.x=y", "ext::sh -c touch", "file:///tmp/x", "/tmp/repo", "../repo", "https://host/"]
        {
            assert!(parse_source(text).is_err(), "{text:?} must be refused");
        }
    }

    #[test]
    fn the_same_repository_is_recognised_whatever_the_protocol() {
        let https = remote_identity("https://github.com/Owner/Repo.git");
        assert_eq!(https.as_deref(), Some("github.com/owner/repo"));
        assert_eq!(remote_identity("git@github.com:owner/repo"), https);
        assert_eq!(remote_identity("ssh://git@github.com:22/owner/repo.git"), https);
        assert_eq!(remote_identity("https://user:example_not_a_real_token@github.com/owner/repo"), https);
        assert_eq!(Source::GitHub("Owner/Repo".into()).identity(), https);
    }

    #[test]
    fn a_clone_already_in_the_folder_is_found() {
        let dir = std::env::temp_dir().join(format!("agentty-clone-test-{}", std::process::id()));
        let other = dir.join("renamed");
        std::fs::create_dir_all(other.join(".git")).unwrap();
        std::fs::write(
            other.join(".git").join("config"),
            "[core]\n\tbare = false\n[remote \"origin\"]\n\turl = git@github.com:owner/repo.git\n",
        )
        .unwrap();
        let source = parse_source("https://github.com/owner/repo").unwrap();
        assert_eq!(already_there(&dir, &source), Some(other.clone()));
        std::fs::create_dir_all(dir.join("tool")).unwrap();
        assert_eq!(already_there(&dir, &parse_source("https://example.com/x/tool.git").unwrap()), Some(dir.join("tool")));
        assert_eq!(already_there(&dir, &parse_source("https://example.com/x/new.git").unwrap()), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reads_what_gh_lists() {
        let list = br#"[{"nameWithOwner":"o/a","description":"A","isPrivate":true},{"nameWithOwner":"--bad/x","description":null,"isPrivate":false}]"#;
        assert_eq!(parse_repos(list), vec![RemoteRepo { full_name: "o/a".into(), description: "A".into(), private: true }]);
        let search = br#"[{"fullName":"x/y","description":"","isPrivate":false}]"#;
        assert_eq!(parse_repos(search)[0].full_name, "x/y");
    }
}
