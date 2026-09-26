//! The sync repository: a clone under the data folder, driven by the user's own `git` and `gh`.
//!
//! Nothing here stores a credential. A GitHub repository is reached over HTTPS with `gh` as the
//! credential helper (what `gh auth setup-git` would configure, passed per command so the user's
//! git config is left alone); any other remote uses the user's git setup as it is (SSH keys, their
//! credential helper). Every command is non-interactive and bounded by a timeout.

use crate::process;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Where the sync repository lives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Remote {
    /// A GitHub repository (`owner/name`), listed and checked through the GitHub CLI. Reached over
    /// HTTPS with `gh` as the credential helper, or over SSH with the user's own keys (`ssh`).
    Github {
        repo: String,
        #[serde(default)]
        ssh: bool,
    },
    /// Any git URL (SSH, HTTPS with the user's credential helper, or a local path for tests).
    Git { url: String },
}

impl Remote {
    pub fn url(&self) -> String {
        match self {
            Remote::Github { repo, ssh: false } => format!("https://github.com/{repo}.git"),
            Remote::Github { repo, ssh: true } => format!("git@github.com:{repo}.git"),
            Remote::Git { url } => url.clone(),
        }
    }

    /// Short label for the UI: `owner/name`, or the URL without credentials.
    pub fn label(&self) -> String {
        match self {
            Remote::Github { repo, .. } => repo.clone(),
            Remote::Git { url } => strip_credentials(url),
        }
    }

    /// Which program carries the data: `gh` (HTTPS with its login) or `git` (SSH keys or the
    /// user's own credential helper).
    pub fn uses_gh(&self) -> bool {
        matches!(self, Remote::Github { ssh: false, .. })
    }
}

/// How `gh` clones: its `git_protocol` setting (`https` unless the user chose `ssh`).
pub fn gh_prefers_ssh() -> bool {
    gh(&["config", "get", "git_protocol"]).is_ok_and(|p| p.trim().eq_ignore_ascii_case("ssh"))
}

/// What the owner of a repository lets others see.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Visibility {
    Private,
    Public,
    /// Could not be told (offline, a host that cannot be asked). Sync goes on; the next check asks again.
    Unknown,
}

/// Author e-mail of sync commits: no person's address goes into the repository's history.
const SYNC_EMAIL: &str = "user.email=sync@agentty.invalid"; // audit: ok — reserved `.invalid` domain, nobody's e-mail

const NETWORK_TIMEOUT: Duration = Duration::from_secs(120);
const LOCAL_TIMEOUT: Duration = Duration::from_secs(30);

/// Runs `command`, killing it after `timeout`. Returns (success, stdout, stderr).
fn run(mut command: Command, timeout: Duration) -> Result<(bool, String, String)> {
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn()?;
    // Drain both pipes on their own threads so a chatty command cannot block on a full pipe.
    let mut stdout = child.stdout.take().context("no stdout")?;
    let mut stderr = child.stderr.take().context("no stderr")?;
    let out = std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = stdout.read_to_end(&mut buffer);
        buffer
    });
    let err = std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = stderr.read_to_end(&mut buffer);
        buffer
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            bail!("timed out after {} s", timeout.as_secs());
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let stdout = String::from_utf8_lossy(&out.join().unwrap_or_default()).to_string();
    let stderr = String::from_utf8_lossy(&err.join().unwrap_or_default()).trim().to_string();
    Ok((status.success(), stdout, stderr))
}

fn gh_command() -> Command {
    let mut command = process::command(process::which("gh").unwrap_or_else(|| PathBuf::from("gh")));
    // Never wait on a login prompt, and keep the CLI from paging or colouring.
    command.env("GH_PROMPT_DISABLED", "1").env("GH_PAGER", "cat").env("NO_COLOR", "1");
    command
}

fn gh(args: &[&str]) -> Result<String> {
    let mut command = gh_command();
    command.args(args);
    let (ok, stdout, stderr) = run(command, NETWORK_TIMEOUT).context("the GitHub CLI (gh) is not installed")?;
    if !ok {
        bail!("{}", if stderr.is_empty() { format!("gh {} failed", args.first().unwrap_or(&"")) } else { stderr });
    }
    Ok(stdout)
}

/// The GitHub account `gh` is signed in to, if any.
pub fn gh_login() -> Option<String> {
    process::which("gh")?;
    gh(&["api", "user", "--jq", ".login"]).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubRepo {
    pub name_with_owner: String,
    /// `PRIVATE`, `PUBLIC` or `INTERNAL`.
    pub visibility: String,
}

impl GithubRepo {
    pub fn is_private(&self) -> bool {
        self.visibility.eq_ignore_ascii_case("PRIVATE")
    }
}

/// The signed-in account's repositories, private ones first.
pub fn gh_repos() -> Result<Vec<GithubRepo>> {
    let json = gh(&["repo", "list", "--limit", "200", "--json", "nameWithOwner,visibility"])?;
    let mut repos: Vec<GithubRepo> = serde_json::from_str(&json).context("unexpected output from gh repo list")?;
    repos.sort_by_key(|r| (!r.is_private(), r.name_with_owner.to_lowercase()));
    Ok(repos)
}

/// Creates a private repository for the sync and returns its `owner/name`.
pub fn gh_create_private(name: &str) -> Result<String> {
    anyhow::ensure!(valid_repo_name(name), "use letters, digits, '-', '_' or '.' for the repository name");
    gh(&["repo", "create", name, "--private", "--description", "Agentty session sync"])?;
    let login = gh_login().context("gh is not signed in")?;
    Ok(format!("{login}/{name}"))
}

fn valid_repo_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 100
        && !name.starts_with(['-', '.'])
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// `owner/name` of a GitHub URL (`https://github.com/o/n(.git)`, `git@github.com:o/n(.git)`).
pub fn github_repo_of(url: &str) -> Option<String> {
    let url = url.trim().trim_end_matches('/').trim_end_matches(".git");
    let path = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("git@github.com:"))
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))?;
    let mut parts = path.split('/');
    let (owner, name) = (parts.next()?, parts.next()?);
    (parts.next().is_none() && !owner.is_empty() && valid_repo_name(name) && !owner.starts_with('-')).then(|| format!("{owner}/{name}"))
}

/// Whether the repository can be read by anyone. GitHub repositories are asked through `gh`; other
/// remotes by reading them over HTTPS with no credentials at all — which must fail.
pub fn visibility(remote: &Remote) -> Visibility {
    if let Some(repo) = match remote {
        Remote::Github { repo, .. } => Some(repo.clone()),
        Remote::Git { url } => github_repo_of(url).filter(|_| process::which("gh").is_some()),
    } {
        return match gh(&["repo", "view", &repo, "--json", "visibility", "--jq", ".visibility"]) {
            Ok(v) if v.trim().eq_ignore_ascii_case("PRIVATE") => Visibility::Private,
            // Internal repositories are visible to a whole enterprise: not private enough.
            Ok(v) if !v.trim().is_empty() => Visibility::Public,
            _ => Visibility::Unknown,
        };
    }
    let Some(https) = anonymous_url(&remote.url()) else {
        // A local path or file:// URL: nobody else reads it.
        return if is_local(&remote.url()) { Visibility::Private } else { Visibility::Unknown };
    };
    let mut command = process::command("git");
    // No user or system config at all: a credential helper configured for that host (the Keychain,
    // `gh auth setup-git`) would sign the read in and make a private repository look public.
    // Never a file in a shared temp folder: another user could put a config there that runs a program.
    let no_config = if cfg!(windows) {
        let path = crate::fsutil::data_dir().join("sync").join("empty.gitconfig");
        let _ = crate::fsutil::write_private(&path, b"");
        path
    } else {
        PathBuf::from("/dev/null")
    };
    command
        .args(["-c", "credential.helper=", "ls-remote", "--heads", &https])
        .env("GIT_CONFIG_GLOBAL", &no_config)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "")
        .env("SSH_ASKPASS", "")
        .env("LC_ALL", "C");
    match run(command, Duration::from_secs(30)) {
        Ok((true, _, _)) => Visibility::Public,
        Ok((false, _, stderr))
            if stderr.contains("Authentication failed") || stderr.contains("could not read Username") || stderr.contains("not found") =>
        {
            Visibility::Private
        }
        _ => Visibility::Unknown,
    }
}

fn is_local(url: &str) -> bool {
    url.starts_with("file://") || url.starts_with('/') || Path::new(url).is_absolute()
}

/// The HTTPS form of a remote, without any credentials, for the anonymous read.
fn anonymous_url(url: &str) -> Option<String> {
    let url = url.trim();
    if let Some(rest) = url.strip_prefix("git@") {
        let (host, path) = rest.split_once(':')?;
        return Some(format!("https://{host}/{path}"));
    }
    if let Some(rest) = url.strip_prefix("ssh://") {
        let rest = rest.split_once('@').map_or(rest, |(_, r)| r);
        let (host, path) = rest.split_once('/')?;
        let host = host.split(':').next()?;
        return Some(format!("https://{host}/{path}"));
    }
    url.starts_with("https://").then(|| strip_credentials(url))
}

fn strip_credentials(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(mut parsed) if parsed.scheme().starts_with("http") => {
            let _ = parsed.set_username("");
            let _ = parsed.set_password(None);
            parsed.to_string()
        }
        _ => url.to_string(),
    }
}

/// Result of a push.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pushed {
    Done,
    /// Another device pushed first: fetch and try again.
    Rejected,
}

/// The local clone.
pub struct Repo {
    pub dir: PathBuf,
    pub remote: Remote,
    pub branch: String,
}

/// Author of sync commits: the device, never the user's own identity (no e-mail in the history).
pub struct Author<'a> {
    pub device_name: &'a str,
}

impl Repo {
    pub fn new(dir: PathBuf, remote: Remote, branch: impl Into<String>) -> Self {
        Self { dir, remote, branch: branch.into() }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = process::command("git");
        if self.remote.uses_gh() {
            // The path goes into a shell snippet git runs: one with a quote in it is not used.
            if let Some(gh) = process::which("gh").filter(|p| !p.to_string_lossy().contains(['\'', '\n'])) {
                // An empty helper first clears any configured ones for this command only.
                let helper = format!("credential.https://github.com.helper=!'{}' auth git-credential", gh.display());
                command.args(["-c", "credential.https://github.com.helper=", "-c", &helper]);
            }
        }
        command
            // A signing prompt (pinentry, a hardware key) would stall a background sync.
            .args(["-c", "commit.gpgsign=false", "-c", "core.autocrlf=false", "-c", "core.quotepath=false"])
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GCM_INTERACTIVE", "never")
            .env("LC_ALL", "C")
            .env("GH_PROMPT_DISABLED", "1");
        if self.dir.join(".git").is_dir() {
            command.arg("-C").arg(&self.dir);
        }
        command.args(args);
        command
    }

    fn git(&self, args: &[&str], timeout: Duration) -> Result<String> {
        let (ok, stdout, stderr) = run(self.command(args), timeout).context("git is not installed")?;
        if !ok {
            bail!(
                "{}",
                if stderr.is_empty() { format!("git {} failed", args.first().unwrap_or(&"")) } else { crate::git::failure_reason(&stderr) }
            );
        }
        Ok(stdout)
    }

    pub fn is_cloned(&self) -> bool {
        self.dir.join(".git").is_dir()
    }

    /// Clones the remote (an empty repository is fine), with the folder private to the user.
    pub fn ensure_clone(&self) -> Result<()> {
        if self.is_cloned() {
            return Ok(());
        }
        if let Some(parent) = self.dir.parent() {
            std::fs::create_dir_all(parent)?;
            private_dir(parent);
        }
        let _ = std::fs::remove_dir_all(&self.dir);
        let dir = self.dir.to_string_lossy().to_string();
        self.git(&["clone", "--quiet", "--", &self.remote.url(), &dir], NETWORK_TIMEOUT)?;
        private_dir(&self.dir);
        Ok(())
    }

    /// The remote's default branch after a clone (`main` for an empty repository).
    pub fn detect_branch(&self) -> String {
        self.git(&["symbolic-ref", "--short", "refs/remotes/origin/HEAD"], LOCAL_TIMEOUT)
            .ok()
            .and_then(|s| s.trim().strip_prefix("origin/").map(str::to_string))
            // The remote names it: one that could read as an option or an odd ref is not used.
            .filter(|b| {
                !b.is_empty() && !b.starts_with('-') && b.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'))
            })
            .unwrap_or_else(|| "main".to_string())
    }

    /// Points the clone at another address of the same repository (HTTPS ⇄ SSH).
    pub fn set_origin(&self, url: &str) -> Result<()> {
        self.git(&["remote", "set-url", "origin", url], LOCAL_TIMEOUT).map(|_| ())
    }

    pub fn fetch(&self) -> Result<()> {
        self.git(&["fetch", "--quiet", "--prune", "origin"], NETWORK_TIMEOUT).map(|_| ())
    }

    /// The commit the remote branch is at, `None` while the repository is empty.
    pub fn remote_head(&self) -> Option<String> {
        let reference = format!("refs/remotes/origin/{}", self.branch);
        self.git(&["rev-parse", "--verify", "--quiet", &reference], LOCAL_TIMEOUT)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    /// Makes the working tree exactly the remote branch. Local commits that never made it out are
    /// dropped: everything this device writes is written again from its own state.
    pub fn reset_to_remote(&self) -> Result<()> {
        match self.remote_head() {
            Some(head) => {
                self.git(&["checkout", "--quiet", "--force", "-B", &self.branch, &head], LOCAL_TIMEOUT)?;
                self.git(&["reset", "--quiet", "--hard", &head], LOCAL_TIMEOUT)?;
            }
            None => {
                // Empty remote: start the branch over with nothing in it.
                let reference = format!("refs/heads/{}", self.branch);
                self.git(&["symbolic-ref", "HEAD", &reference], LOCAL_TIMEOUT)?;
                let _ = self.git(&["update-ref", "-d", &reference], LOCAL_TIMEOUT);
                let _ = self.git(&["rm", "-r", "--cached", "--quiet", "--ignore-unmatch", "."], LOCAL_TIMEOUT);
            }
        }
        self.git(&["clean", "-fdxq"], LOCAL_TIMEOUT)?;
        Ok(())
    }

    /// Stages everything and commits. `false` when there was nothing to commit.
    pub fn commit_all(&self, message: &str, author: &Author) -> Result<bool> {
        self.git(&["add", "-A"], LOCAL_TIMEOUT)?;
        let (clean, _, _) = run(self.command(&["diff", "--cached", "--quiet"]), LOCAL_TIMEOUT)?;
        if clean {
            return Ok(false);
        }
        let name = format!("user.name=Agentty ({})", author.device_name.replace(['<', '>', '\n'], ""));
        self.git(&["-c", &name, "-c", SYNC_EMAIL, "commit", "--quiet", "-m", message], LOCAL_TIMEOUT)?;
        Ok(true)
    }

    pub fn push(&self) -> Result<Pushed> {
        let refspec = format!("HEAD:refs/heads/{}", self.branch);
        let (ok, _, stderr) =
            run(self.command(&["push", "--quiet", "origin", &refspec]), NETWORK_TIMEOUT).context("git is not installed")?;
        if ok {
            return Ok(Pushed::Done);
        }
        if stderr.contains("[rejected]")
            || stderr.contains("fetch first")
            || stderr.contains("non-fast-forward")
            || stderr.contains("cannot lock ref")
        {
            return Ok(Pushed::Rejected);
        }
        bail!("{}", crate::git::failure_reason(&stderr))
    }

    /// Files in the working tree other than the repository's own.
    pub fn is_empty_or_readme_only(&self) -> bool {
        let Ok(entries) = std::fs::read_dir(&self.dir) else { return true };
        entries.flatten().all(|e| {
            let name = e.file_name().to_string_lossy().to_lowercase();
            name == ".git" || name.starts_with("readme") || name.starts_with("license") || name == ".gitignore" || name == ".gitattributes"
        })
    }
}

/// `0700` on Unix: transcripts are as private as the agents' own folders.
pub fn private_dir(dir: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    #[cfg(not(unix))]
    let _ = dir;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_urls_name_their_repository() {
        assert_eq!(github_repo_of("https://github.com/o/sync.git").as_deref(), Some("o/sync"));
        assert_eq!(github_repo_of("git@github.com:o/sync.git").as_deref(), Some("o/sync"));
        assert_eq!(github_repo_of("ssh://git@github.com/o/sync").as_deref(), Some("o/sync"));
        assert_eq!(github_repo_of("https://gitlab.com/o/sync.git"), None);
        assert_eq!(github_repo_of("https://github.com/o/sync/extra"), None);
        assert_eq!(github_repo_of("https://github.com/-o/sync"), None);
    }

    #[test]
    fn remotes_are_read_anonymously_over_https() {
        assert_eq!(anonymous_url("git@gitlab.com:o/r.git").as_deref(), Some("https://gitlab.com/o/r.git"));
        assert_eq!(anonymous_url("ssh://git@host.example:2222/o/r.git").as_deref(), Some("https://host.example/o/r.git"));
        let with_user = format!("https://someone:{}@host.example/o/r.git", "not_a_real_password");
        assert_eq!(anonymous_url(&with_user).as_deref(), Some("https://host.example/o/r.git"));
        assert_eq!(anonymous_url("/tmp/local.git"), None);
    }

    #[test]
    fn repository_names_are_checked_before_gh_sees_them() {
        assert!(valid_repo_name("agentty-sync"));
        assert!(!valid_repo_name("--public"));
        assert!(!valid_repo_name("a b"));
        assert!(!valid_repo_name(""));
    }

    #[test]
    fn a_local_remote_is_private() {
        assert_eq!(visibility(&Remote::Git { url: "/tmp/agentty-sync-test.git".into() }), Visibility::Private);
    }

    #[test]
    fn a_github_repository_goes_through_gh_or_git() {
        let https = Remote::Github { repo: "o/sync".into(), ssh: false };
        let ssh = Remote::Github { repo: "o/sync".into(), ssh: true };
        assert_eq!(https.url(), "https://github.com/o/sync.git");
        assert_eq!(ssh.url(), "git@github.com:o/sync.git");
        assert!(https.uses_gh());
        assert!(!ssh.uses_gh());
        assert!(!Remote::Git { url: "git@github.com:o/sync.git".into() }.uses_gh());
        // Settings saved before the choice existed read as gh over HTTPS.
        let old: Remote = serde_json::from_str(r#"{"kind":"github","repo":"o/sync"}"#).unwrap();
        assert_eq!(old, https);
    }
}
