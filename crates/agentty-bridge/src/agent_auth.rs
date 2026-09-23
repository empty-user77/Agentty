//! Credentials for starting Claude Code and Codex without their own CLI login — for machines
//! where signing in through the CLI is not possible (no browser, company proxy, CI boxes) or when
//! a team shares an API account.
//!
//! Security model (same as [`crate::connectors`]):
//! - The choice of method and non-secret values (base URL, region, project) live in
//!   `<data dir>/agent-auth.json`, which contains **no secrets**.
//! - API keys and tokens are kept in the OS credential store ([`crate::secret_store`]) and read
//!   only when a pane starts. They reach the agent as environment variables of that one process;
//!   they are never put on a command line, in agent config files or in logs.
//! - Codex reads its credentials from `auth.json` in `CODEX_HOME`, so Codex methods use a private
//!   Codex home (`<data dir>/codex-home`, `0700`, `auth.json` `0600`) whose other entries
//!   (config, sessions, history, prompts, …) link to the user's own `~/.codex`, so sessions,
//!   resume and settings keep working and are shared with the CLI login.
//!
//! The default for both agents is [`ClaudeAuth::Cli`] / [`CodexAuth::Cli`]: nothing is injected
//! and the agents use their own login exactly as before.

use crate::fsutil;
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Credential store service for agent credentials.
pub const SERVICE: &str = "run.agentty.agent-auth";

/// [`SERVICE`] for this install: a second install (`AGENTTY_DATA_DIR`) gets its own, like the
/// connectors' Keychain service, so a development build can't overwrite the app's keys.
fn service() -> String {
    crate::connectors::scoped_service(SERVICE, std::env::var_os("AGENTTY_DATA_DIR").as_deref())
}

/// Secret slots in the credential store.
pub mod account {
    pub const CLAUDE_API_KEY: &str = "claude.api_key";
    pub const CLAUDE_AUTH_TOKEN: &str = "claude.auth_token";
    pub const CLAUDE_OAUTH_TOKEN: &str = "claude.oauth_token";
    pub const CLAUDE_BEDROCK_TOKEN: &str = "claude.bedrock_token";
    pub const CODEX_API_KEY: &str = "codex.api_key";
}

/// Every variable an auth method may set, so a method can clear the others' (the most specific
/// one would otherwise win: e.g. an `ANTHROPIC_API_KEY` exported in `.zshrc` beats an OAuth token).
const CLAUDE_VARIABLES: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "AWS_BEARER_TOKEN_BEDROCK",
    "ANTHROPIC_VERTEX_PROJECT_ID",
    "CLOUD_ML_REGION",
];
const CODEX_VARIABLES: &[&str] = &["OPENAI_API_KEY", "OPENAI_BASE_URL", "CODEX_API_KEY"];

/// How Claude Code authenticates.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "method")]
pub enum ClaudeAuth {
    /// Claude Code's own login (`claude` → `/login`). Nothing is injected.
    #[default]
    Cli,
    /// Anthropic Console API key (`ANTHROPIC_API_KEY`), optionally through a gateway.
    #[serde(rename_all = "camelCase")]
    ApiKey {
        #[serde(default)]
        base_url: String,
    },
    /// Bearer token for an LLM gateway or proxy (`ANTHROPIC_AUTH_TOKEN` + `ANTHROPIC_BASE_URL`).
    #[serde(rename_all = "camelCase")]
    AuthToken {
        #[serde(default)]
        base_url: String,
    },
    /// Long-lived subscription token from `claude setup-token` (`CLAUDE_CODE_OAUTH_TOKEN`).
    OAuthToken,
    /// Amazon Bedrock with the AWS credential chain (profile / SSO / env) or a Bedrock API key.
    #[serde(rename_all = "camelCase")]
    Bedrock {
        region: String,
        #[serde(default)]
        profile: String,
    },
    /// Google Vertex AI with Application Default Credentials (`gcloud auth application-default login`).
    #[serde(rename_all = "camelCase")]
    Vertex { project_id: String, region: String },
}

/// How Codex authenticates.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "method")]
pub enum CodexAuth {
    /// Codex's own login (`codex login`). Nothing is injected.
    #[default]
    Cli,
    /// OpenAI API key, optionally with a compatible base URL.
    #[serde(rename_all = "camelCase")]
    ApiKey {
        #[serde(default)]
        base_url: String,
    },
    /// An imported `auth.json` (ChatGPT sign-in copied from a machine where `codex login` works,
    /// or an API-key `auth.json`). Codex refreshes it in place.
    AuthJson,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AuthProfile {
    pub claude: ClaudeAuth,
    pub codex: CodexAuth,
}

impl AuthProfile {
    fn path() -> PathBuf {
        fsutil::data_dir().join("agent-auth.json")
    }

    pub fn load() -> Self {
        std::fs::read(Self::path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        restrict_file(&tmp)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }
}

impl ClaudeAuth {
    /// The credential-store slot holding this method's secret; `None` for methods without one.
    /// (Bedrock's API key is optional: the AWS credential chain is used when it is absent.)
    pub fn secret_account(&self) -> Option<&'static str> {
        match self {
            ClaudeAuth::ApiKey { .. } => Some(account::CLAUDE_API_KEY),
            ClaudeAuth::AuthToken { .. } => Some(account::CLAUDE_AUTH_TOKEN),
            ClaudeAuth::OAuthToken => Some(account::CLAUDE_OAUTH_TOKEN),
            ClaudeAuth::Bedrock { .. } => Some(account::CLAUDE_BEDROCK_TOKEN),
            ClaudeAuth::Cli | ClaudeAuth::Vertex { .. } => None,
        }
    }

    pub fn secret_required(&self) -> bool {
        matches!(self, ClaudeAuth::ApiKey { .. } | ClaudeAuth::AuthToken { .. } | ClaudeAuth::OAuthToken)
    }

    pub fn validate(&self) -> Result<()> {
        match self {
            ClaudeAuth::ApiKey { base_url } => validate_base_url(base_url, true),
            ClaudeAuth::AuthToken { base_url } => validate_base_url(base_url, true),
            ClaudeAuth::Bedrock { region, profile } => {
                ensure!(is_identifier(region), "enter an AWS region such as us-east-1");
                ensure!(profile.is_empty() || is_identifier(profile), "the AWS profile name contains unsupported characters");
                Ok(())
            }
            ClaudeAuth::Vertex { project_id, region } => {
                ensure!(is_identifier(project_id), "enter a Google Cloud project ID");
                ensure!(is_identifier(region), "enter a Vertex AI region such as us-east5 or global");
                Ok(())
            }
            ClaudeAuth::Cli | ClaudeAuth::OAuthToken => Ok(()),
        }
    }

    /// Environment for Claude Code; `secret` is the stored credential for this method, if any.
    pub fn environment(&self, secret: Option<&str>) -> Result<Vec<EnvChange>> {
        if *self == ClaudeAuth::Cli {
            return Ok(Vec::new());
        }
        let secret = secret.map(str::trim).filter(|s| !s.is_empty());
        ensure!(secret.is_some() || !self.secret_required(), "no credential is saved for this sign-in method");
        let mut set: Vec<(&str, String)> = Vec::new();
        match self {
            ClaudeAuth::Cli => {}
            ClaudeAuth::ApiKey { base_url } => {
                set.push(("ANTHROPIC_API_KEY", secret.unwrap_or_default().to_string()));
                push_base_url(&mut set, "ANTHROPIC_BASE_URL", base_url);
            }
            ClaudeAuth::AuthToken { base_url } => {
                set.push(("ANTHROPIC_AUTH_TOKEN", secret.unwrap_or_default().to_string()));
                push_base_url(&mut set, "ANTHROPIC_BASE_URL", base_url);
            }
            ClaudeAuth::OAuthToken => set.push(("CLAUDE_CODE_OAUTH_TOKEN", secret.unwrap_or_default().to_string())),
            ClaudeAuth::Bedrock { region, profile } => {
                set.push(("CLAUDE_CODE_USE_BEDROCK", "1".into()));
                set.push(("AWS_REGION", region.trim().to_string()));
                if !profile.trim().is_empty() {
                    set.push(("AWS_PROFILE", profile.trim().to_string()));
                }
                if let Some(token) = secret {
                    set.push(("AWS_BEARER_TOKEN_BEDROCK", token.to_string()));
                }
            }
            ClaudeAuth::Vertex { project_id, region } => {
                set.push(("CLAUDE_CODE_USE_VERTEX", "1".into()));
                set.push(("ANTHROPIC_VERTEX_PROJECT_ID", project_id.trim().to_string()));
                set.push(("CLOUD_ML_REGION", region.trim().to_string()));
            }
        }
        Ok(changes(CLAUDE_VARIABLES, set))
    }
}

impl CodexAuth {
    pub fn secret_account(&self) -> Option<&'static str> {
        match self {
            CodexAuth::ApiKey { .. } => Some(account::CODEX_API_KEY),
            CodexAuth::Cli | CodexAuth::AuthJson => None,
        }
    }

    pub fn validate(&self) -> Result<()> {
        match self {
            CodexAuth::ApiKey { base_url } => validate_base_url(base_url, true),
            CodexAuth::Cli | CodexAuth::AuthJson => Ok(()),
        }
    }

    /// Environment for Codex. Prepares the private Codex home (writing `auth.json` for API keys).
    pub fn environment(&self, secret: Option<&str>) -> Result<Vec<EnvChange>> {
        let home = codex_home_for(self);
        let mut set: Vec<(&str, String)> = Vec::new();
        match self {
            CodexAuth::Cli => return Ok(Vec::new()),
            CodexAuth::ApiKey { base_url } => {
                let key = secret.map(str::trim).filter(|s| !s.is_empty()).context("no API key is saved for Codex")?;
                prepare_codex_home(&home)?;
                write_private(&home.join("auth.json"), &serde_json::to_vec_pretty(&serde_json::json!({ "OPENAI_API_KEY": key }))?)?;
                set.push(("OPENAI_API_KEY", key.to_string()));
                push_base_url(&mut set, "OPENAI_BASE_URL", base_url);
            }
            CodexAuth::AuthJson => {
                ensure!(home.join("auth.json").is_file(), "no auth.json has been imported for Codex");
                prepare_codex_home(&home)?;
            }
        }
        set.push(("CODEX_HOME", home.display().to_string()));
        let mut all = CODEX_VARIABLES.to_vec();
        all.push("CODEX_HOME");
        Ok(changes(&all, set))
    }
}

/// One change to a pane's environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvChange {
    Set(String, String),
    /// Removed so a value from the user's shell profile cannot override the chosen method.
    Unset(String),
}

impl EnvChange {
    pub fn name(&self) -> &str {
        match self {
            EnvChange::Set(name, _) | EnvChange::Unset(name) => name,
        }
    }
}

fn changes(all: &[&str], set: Vec<(&str, String)>) -> Vec<EnvChange> {
    let mut out: Vec<EnvChange> =
        all.iter().filter(|name| !set.iter().any(|(n, _)| n == *name)).map(|name| EnvChange::Unset(name.to_string())).collect();
    out.extend(set.into_iter().map(|(name, value)| EnvChange::Set(name.to_string(), value)));
    out
}

fn push_base_url(set: &mut Vec<(&str, String)>, name: &'static str, base_url: &str) {
    let base_url = base_url.trim().trim_end_matches('/');
    if !base_url.is_empty() {
        set.push((name, base_url.to_string()));
    }
}

fn is_identifier(text: &str) -> bool {
    let text = text.trim();
    !text.is_empty() && text.len() <= 128 && text.chars().all(|c| c.is_ascii_alphanumeric() || "-_.@".contains(c))
}

/// Base URLs must be HTTPS (plain HTTP only for localhost gateways).
pub fn validate_base_url(base_url: &str, optional: bool) -> Result<()> {
    let base_url = base_url.trim();
    if base_url.is_empty() {
        ensure!(optional, "a base URL is required");
        return Ok(());
    }
    let url = url::Url::parse(base_url).context("the base URL is not a valid URL")?;
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]" | "::1"));
    ensure!(url.scheme() == "https" || (url.scheme() == "http" && local), "the base URL must use https:// (http:// only for localhost)");
    ensure!(url.username().is_empty() && url.password().is_none(), "put credentials in the key field, not in the URL");
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Profile + secrets
// ---------------------------------------------------------------------------------------------

/// Which agent a credential belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthAgent {
    Claude,
    Codex,
}

pub fn store_secret(account: &str, secret: &str) -> Result<()> {
    let secret = secret.trim();
    ensure!(!secret.is_empty(), "the key is empty");
    ensure!(!secret.chars().any(char::is_control), "the key contains line breaks or control characters");
    crate::secret_store::store(&service(), account, secret)
}

pub fn load_secret(account: &str) -> Option<String> {
    crate::secret_store::load(&service(), account).ok().filter(|s| !s.is_empty())
}

pub fn delete_secret(account: &str) -> Result<()> {
    crate::secret_store::delete(&service(), account)
}

/// `••••1a2b` — enough to recognize which key is saved, never enough to use it.
pub fn masked(secret: &str) -> String {
    let tail: String = secret.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
    if secret.chars().count() <= 8 {
        "••••".to_string()
    } else {
        format!("••••{tail}")
    }
}

/// Environment changes for a new pane of `agent`, from the saved profile. `Ok(empty)` means the
/// agent's own login is used.
pub fn environment(agent: AuthAgent) -> Result<Vec<EnvChange>> {
    let profile = AuthProfile::load();
    match agent {
        AuthAgent::Claude => {
            let secret = profile.claude.secret_account().and_then(load_secret);
            profile.claude.environment(secret.as_deref())
        }
        AuthAgent::Codex => {
            let secret = profile.codex.secret_account().and_then(load_secret);
            profile.codex.environment(secret.as_deref())
        }
    }
}

/// Removes every stored credential of `agent` and switches it back to the CLI login.
pub fn reset(agent: AuthAgent) -> Result<()> {
    let mut profile = AuthProfile::load();
    match agent {
        AuthAgent::Claude => {
            for slot in [account::CLAUDE_API_KEY, account::CLAUDE_AUTH_TOKEN, account::CLAUDE_OAUTH_TOKEN, account::CLAUDE_BEDROCK_TOKEN] {
                let _ = delete_secret(slot);
            }
            profile.claude = ClaudeAuth::Cli;
        }
        AuthAgent::Codex => {
            let _ = delete_secret(account::CODEX_API_KEY);
            let _ = std::fs::remove_file(codex_home().join("auth.json"));
            let _ = std::fs::remove_file(codex_api_key_home().join("auth.json"));
            profile.codex = CodexAuth::Cli;
        }
    }
    profile.save()
}

/// Accepts either a bare token or Claude Code's credentials JSON (`~/.claude/.credentials.json`,
/// `{"claudeAiOauth":{"accessToken":…}}`) and returns the token.
pub fn parse_claude_token(input: &str) -> Result<String> {
    let input = input.trim();
    if input.starts_with('{') {
        let value: Value = serde_json::from_str(input).context("the pasted text is not valid JSON")?;
        let token = value["claudeAiOauth"]["accessToken"]
            .as_str()
            .or_else(|| value["accessToken"].as_str())
            .or_else(|| value["oauth_token"].as_str())
            .context("no accessToken found in the pasted credentials")?;
        return Ok(token.to_string());
    }
    ensure!(!input.is_empty(), "the token is empty");
    Ok(input.to_string())
}

// ---------------------------------------------------------------------------------------------
// Codex home
// ---------------------------------------------------------------------------------------------

/// Private Codex home holding an imported `auth.json`.
pub fn codex_home() -> PathBuf {
    fsutil::data_dir().join("codex-home")
}

/// Private Codex home for the API-key method. Separate from [`codex_home`] so switching methods
/// never overwrites an imported ChatGPT `auth.json` with an API key (or the other way round).
pub fn codex_api_key_home() -> PathBuf {
    fsutil::data_dir().join("codex-home-api-key")
}

pub fn codex_home_for(auth: &CodexAuth) -> PathBuf {
    match auth {
        CodexAuth::ApiKey { .. } => codex_api_key_home(),
        CodexAuth::Cli | CodexAuth::AuthJson => codex_home(),
    }
}

/// The user's own Codex home: `$CODEX_HOME` when it is an absolute, existing directory that isn't
/// one of Agentty's private homes (a nested Agentty inherits those), else `~/.codex`.
pub fn user_codex_home() -> PathBuf {
    user_codex_home_from(std::env::var_os("CODEX_HOME"))
}

fn user_codex_home_from(value: Option<std::ffi::OsString>) -> PathBuf {
    let private = [codex_home(), codex_api_key_home()];
    value
        .map(PathBuf::from)
        .filter(|p| p.is_absolute() && p.is_dir() && !private.iter().any(|home| same_path(home, p)))
        .unwrap_or_else(|| fsutil::home().join(".codex"))
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Entries of the user's Codex home shared with the private one. `auth.json` is never shared.
const SHARED_DIRS: &[&str] = &["sessions", "archived_sessions", "prompts", "rules", "skills", "memories"];
const SHARED_FILES: &[&str] = &["config.toml", "AGENTS.md", "history.jsonl", "session_index.jsonl"];

/// Creates the private Codex home and links the user's config and session folders into it.
pub fn prepare_codex_home(home: &Path) -> Result<()> {
    std::fs::create_dir_all(home)?;
    restrict_dir(home)?;
    let user = user_codex_home();
    if user == home {
        return Ok(());
    }
    // Sessions must end up where Agentty (and `codex resume` with the CLI login) look for them.
    std::fs::create_dir_all(user.join("sessions")).ok();
    for name in SHARED_DIRS {
        let (source, link) = (user.join(name), home.join(name));
        if source.is_dir() && std::fs::symlink_metadata(&link).is_err() {
            if let Err(err) = link_dir(&source, &link) {
                eprintln!("agentty: could not link {name} into the Codex home: {err:#}");
            }
        }
    }
    for name in SHARED_FILES {
        let (source, link) = (user.join(name), home.join(name));
        if source.is_file() {
            refresh_file_link(&source, &link);
        }
    }
    Ok(())
}

/// Sessions written into a private Codex home itself (only when linking was impossible).
pub fn codex_home_sessions() -> Vec<PathBuf> {
    [codex_home(), codex_api_key_home()]
        .into_iter()
        .map(|home| home.join("sessions"))
        .filter(|sessions| std::fs::symlink_metadata(sessions).is_ok_and(|m| m.file_type().is_dir()))
        .collect()
}

#[cfg(unix)]
fn link_dir(source: &Path, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(source, link)?;
    Ok(())
}

#[cfg(windows)]
fn link_dir(source: &Path, link: &Path) -> Result<()> {
    // Symbolic links need Developer Mode; directory junctions work for every user.
    if std::os::windows::fs::symlink_dir(source, link).is_ok() {
        return Ok(());
    }
    create_junction(source, link)
}

/// A directory junction made with the file system call itself: no `cmd /C mklink /J` (cmd.exe
/// re-parses its command line and would act on `&`, `%`, `^` in a path) and no PowerShell, which
/// failed without a word when started below a pseudo console (an Agentty pane, a CI runner
/// launched from one).
#[cfg(windows)]
fn create_junction(target: &Path, link: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, OPEN_EXISTING};
    use windows_sys::Win32::System::IO::DeviceIoControl;
    const FSCTL_SET_REPARSE_POINT: u32 = 0x0009_00A4;

    let target = std::fs::canonicalize(target).context("junction target")?;
    let target = target.to_string_lossy();
    // `canonicalize` answers `\\?\C:\…`; a junction wants the plain drive path.
    let target = target.strip_prefix(r"\\?\").unwrap_or(&target);
    ensure!(!target.starts_with(r"UNC\"), "a junction can't point to a network folder");
    let buffer = mount_point_buffer(target);
    std::fs::create_dir(link).context("junction folder")?;
    let wide: Vec<u16> = link.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: `wide` is a NUL-terminated path; the handle is closed below on every path.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_WRITE,
            0,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        let err = std::io::Error::last_os_error();
        let _ = std::fs::remove_dir(link);
        bail!("could not open the junction folder: {err}");
    }
    let mut returned = 0u32;
    // SAFETY: `buffer` is a complete REPARSE_DATA_BUFFER of `buffer.len()` bytes; no output.
    let ok = unsafe {
        DeviceIoControl(
            handle,
            FSCTL_SET_REPARSE_POINT,
            buffer.as_ptr().cast(),
            buffer.len() as u32,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    let err = std::io::Error::last_os_error();
    // SAFETY: `handle` came from CreateFileW above and is closed once.
    unsafe { CloseHandle(handle) };
    if ok == 0 {
        let _ = std::fs::remove_dir(link);
        bail!("could not create a directory junction: {err}");
    }
    Ok(())
}

/// A mount-point REPARSE_DATA_BUFFER for `target` (a drive path such as `C:\Users\me\.codex\sessions`):
/// tag, data length, reserved, then the substitute name (`\??\` + path) and the print name, each
/// NUL-terminated UTF-16.
#[cfg_attr(not(windows), allow(dead_code))]
fn mount_point_buffer(target: &str) -> Vec<u8> {
    const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
    let substitute: Vec<u16> = format!(r"\??\{target}").encode_utf16().collect();
    let print: Vec<u16> = target.encode_utf16().collect();
    let (substitute_bytes, print_bytes) = (substitute.len() * 2, print.len() * 2);
    // Four u16 offsets and lengths, then both names with their terminators.
    let data_length = 8 + substitute_bytes + 2 + print_bytes + 2;
    let mut buffer = Vec::with_capacity(8 + data_length);
    buffer.extend(IO_REPARSE_TAG_MOUNT_POINT.to_le_bytes());
    buffer.extend((data_length as u16).to_le_bytes());
    buffer.extend(0u16.to_le_bytes());
    buffer.extend(0u16.to_le_bytes());
    buffer.extend((substitute_bytes as u16).to_le_bytes());
    buffer.extend(((substitute_bytes + 2) as u16).to_le_bytes());
    buffer.extend((print_bytes as u16).to_le_bytes());
    for unit in substitute.iter().chain(&[0]).chain(&print).chain(&[0]) {
        buffer.extend(unit.to_le_bytes());
    }
    buffer
}

#[cfg(unix)]
fn refresh_file_link(source: &Path, link: &Path) {
    if std::fs::symlink_metadata(link).is_err() {
        let _ = std::os::unix::fs::symlink(source, link);
    }
}

#[cfg(windows)]
fn refresh_file_link(source: &Path, link: &Path) {
    let is_link = std::fs::symlink_metadata(link).is_ok_and(|m| m.file_type().is_symlink());
    if is_link {
        return;
    }
    if std::fs::symlink_metadata(link).is_err() && std::os::windows::fs::symlink_file(source, link).is_ok() {
        return;
    }
    // Without symlink rights the user's file is copied (read-only mirror, refreshed at every start).
    if fsutil::mtime_ms(source) > fsutil::mtime_ms(link) {
        let _ = std::fs::copy(source, link);
    }
}

/// Imports an `auth.json` (pasted text or file contents) into the private Codex home.
pub fn import_codex_auth_json(text: &str) -> Result<CodexAuthSummary> {
    let value: Value = serde_json::from_str(text.trim()).context("auth.json is not valid JSON")?;
    let summary = summarize_codex_auth(&value)?;
    let home = codex_home();
    prepare_codex_home(&home)?;
    write_private(&home.join("auth.json"), &serde_json::to_vec_pretty(&value)?)?;
    Ok(summary)
}

/// What an imported `auth.json` signs in with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexAuthSummary {
    ApiKey,
    ChatGpt { email: Option<String>, plan: Option<String> },
}

pub fn summarize_codex_auth(value: &Value) -> Result<CodexAuthSummary> {
    ensure!(value.is_object(), "auth.json must be a JSON object");
    let tokens = &value["tokens"];
    if tokens.is_object() {
        ensure!(
            tokens["access_token"].as_str().is_some_and(|t| !t.is_empty())
                || tokens["refresh_token"].as_str().is_some_and(|t| !t.is_empty()),
            "auth.json has no ChatGPT tokens"
        );
        let claims = tokens["id_token"].as_str().and_then(jwt_claims).unwrap_or(Value::Null);
        let email = claims["email"].as_str().map(str::to_string);
        let plan = claims["https://api.openai.com/auth"]["chatgpt_plan_type"].as_str().map(str::to_string);
        return Ok(CodexAuthSummary::ChatGpt { email, plan });
    }
    if value["OPENAI_API_KEY"].as_str().is_some_and(|k| !k.trim().is_empty()) {
        return Ok(CodexAuthSummary::ApiKey);
    }
    bail!("auth.json has neither ChatGPT tokens nor an OPENAI_API_KEY")
}

/// Summary of the `auth.json` currently imported, if any.
pub fn imported_codex_auth() -> Option<CodexAuthSummary> {
    let bytes = std::fs::read(codex_home().join("auth.json")).ok()?;
    summarize_codex_auth(&serde_json::from_slice(&bytes).ok()?).ok()
}

/// Payload of a JWT (not verified — only used to show which account an `auth.json` belongs to).
fn jwt_claims(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;
    serde_json::from_slice(&base64url_decode(payload)?).ok()
}

fn base64url_decode(text: &str) -> Option<Vec<u8>> {
    let value = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            _ => return None,
        } as u32)
    };
    let bytes: Vec<u8> = text.bytes().filter(|b| *b != b'=').collect();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let mut acc = 0u32;
        for (i, b) in chunk.iter().enumerate() {
            acc |= value(*b)? << (18 - 6 * i);
        }
        let n = chunk.len();
        if n < 2 {
            return None;
        }
        out.push((acc >> 16) as u8);
        if n > 2 {
            out.push((acc >> 8) as u8);
        }
        if n > 3 {
            out.push(acc as u8);
        }
    }
    Some(out)
}

fn restrict_dir(_dir: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(_dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn restrict_file(_path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(_path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// Writes a file only the user can read (created `0600` on Unix before any content is written).
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp)?;
    std::io::Write::write_all(&mut file, bytes)?;
    drop(file);
    std::fs::rename(tmp, path)?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Verification
// ---------------------------------------------------------------------------------------------

const ANTHROPIC_API: &str = "https://api.anthropic.com";
const OPENAI_API: &str = "https://api.openai.com/v1";

fn http() -> ureq::Agent {
    crate::http::agent_builder().timeout(Duration::from_secs(15)).redirects(0).build()
}

/// Number of models a `/models` response lists.
fn model_count(response: ureq::Response) -> usize {
    response.into_json::<Value>().ok().and_then(|v| v["data"].as_array().map(Vec::len)).unwrap_or(0)
}

fn describe(result: std::result::Result<ureq::Response, ureq::Error>) -> Result<usize> {
    match result {
        Ok(response) => Ok(model_count(response)),
        Err(ureq::Error::Status(401 | 403, _)) => bail!("the credential was rejected (401/403)"),
        Err(ureq::Error::Status(code, _)) => bail!("the server answered HTTP {code}"),
        Err(ureq::Error::Transport(err)) => bail!("could not reach the server: {}", err.kind()),
    }
}

/// Checks a Claude credential against the Anthropic API (or the configured gateway). Returns
/// the number of models the credential can see.
pub fn verify_claude(auth: &ClaudeAuth, secret: &str) -> Result<usize> {
    // Never send a key to a URL that wouldn't pass saving (plain http, credentials in the URL).
    auth.validate()?;
    let secret = secret.trim();
    let request = |base_url: &str| {
        let base = if base_url.trim().is_empty() { ANTHROPIC_API } else { base_url.trim().trim_end_matches('/') };
        http().get(&format!("{base}/v1/models")).set("anthropic-version", "2023-06-01").set("User-Agent", "Agentty")
    };
    match auth {
        ClaudeAuth::ApiKey { base_url } => describe(request(base_url).set("x-api-key", secret).call()),
        ClaudeAuth::AuthToken { base_url } => describe(request(base_url).set("Authorization", &format!("Bearer {secret}")).call()),
        ClaudeAuth::OAuthToken => {
            describe(request("").set("Authorization", &format!("Bearer {secret}")).set("anthropic-beta", "oauth-2025-04-20").call())
        }
        ClaudeAuth::Cli | ClaudeAuth::Bedrock { .. } | ClaudeAuth::Vertex { .. } => {
            bail!("this sign-in method is checked by Claude Code itself when it starts")
        }
    }
}

/// Checks an OpenAI API key (or a compatible gateway).
pub fn verify_codex(auth: &CodexAuth, secret: &str) -> Result<usize> {
    auth.validate()?;
    match auth {
        CodexAuth::ApiKey { base_url } => {
            let base = if base_url.trim().is_empty() { OPENAI_API } else { base_url.trim().trim_end_matches('/') };
            describe(
                http()
                    .get(&format!("{base}/models"))
                    .set("Authorization", &format!("Bearer {}", secret.trim()))
                    .set("User-Agent", "Agentty")
                    .call(),
            )
        }
        CodexAuth::Cli | CodexAuth::AuthJson => bail!("this sign-in method is checked by Codex itself when it starts"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake(kind: &str) -> String {
        // Built at runtime so nothing credential-shaped is committed.
        format!("{kind}-example-not_a_real_key-{}", "0".repeat(12))
    }

    fn set(changes: &[EnvChange], name: &str) -> Option<String> {
        changes.iter().find_map(|c| match c {
            EnvChange::Set(n, v) if n == name => Some(v.clone()),
            _ => None,
        })
    }

    fn unset(changes: &[EnvChange], name: &str) -> bool {
        changes.iter().any(|c| matches!(c, EnvChange::Unset(n) if n == name))
    }

    #[test]
    fn cli_login_changes_nothing() {
        assert!(ClaudeAuth::Cli.environment(None).unwrap().is_empty());
        assert!(CodexAuth::Cli.environment(None).unwrap().is_empty());
        assert_eq!(AuthProfile::default(), serde_json::from_str::<AuthProfile>("{}").unwrap());
    }

    #[test]
    fn claude_methods_set_their_variables_and_clear_the_rest() {
        let key = fake("api");
        let env = ClaudeAuth::ApiKey { base_url: "https://gateway.example.com/".into() }.environment(Some(&key)).unwrap();
        assert_eq!(set(&env, "ANTHROPIC_API_KEY"), Some(key.clone()));
        assert_eq!(set(&env, "ANTHROPIC_BASE_URL").as_deref(), Some("https://gateway.example.com"));
        assert!(unset(&env, "ANTHROPIC_AUTH_TOKEN") && unset(&env, "CLAUDE_CODE_OAUTH_TOKEN"));
        assert!(!unset(&env, "ANTHROPIC_API_KEY"));

        let env = ClaudeAuth::OAuthToken.environment(Some(&key)).unwrap();
        assert_eq!(set(&env, "CLAUDE_CODE_OAUTH_TOKEN"), Some(key.clone()));
        assert!(unset(&env, "ANTHROPIC_API_KEY") && unset(&env, "ANTHROPIC_BASE_URL"));

        let env = ClaudeAuth::Bedrock { region: "us-east-1".into(), profile: "work".into() }.environment(None).unwrap();
        assert_eq!(set(&env, "CLAUDE_CODE_USE_BEDROCK").as_deref(), Some("1"));
        assert_eq!(set(&env, "AWS_PROFILE").as_deref(), Some("work"));
        assert!(set(&env, "AWS_BEARER_TOKEN_BEDROCK").is_none());

        let env = ClaudeAuth::Vertex { project_id: "my-project".into(), region: "global".into() }.environment(None).unwrap();
        assert_eq!(set(&env, "ANTHROPIC_VERTEX_PROJECT_ID").as_deref(), Some("my-project"));

        assert!(ClaudeAuth::ApiKey { base_url: String::new() }.environment(None).is_err());
        assert!(ClaudeAuth::AuthToken { base_url: String::new() }.environment(Some("  ")).is_err());
    }

    #[test]
    fn profile_json_is_stable() {
        let profile = AuthProfile {
            claude: ClaudeAuth::AuthToken { base_url: "https://llm.example.com".into() },
            codex: CodexAuth::ApiKey { base_url: String::new() },
        };
        let json = serde_json::to_value(&profile).unwrap();
        assert_eq!(json["claude"]["method"], "authToken");
        assert_eq!(json["claude"]["baseUrl"], "https://llm.example.com");
        assert_eq!(json["codex"]["method"], "apiKey");
        assert_eq!(serde_json::from_value::<AuthProfile>(json).unwrap(), profile);
    }

    #[test]
    fn base_urls_must_be_https_except_localhost() {
        assert!(validate_base_url("", true).is_ok());
        assert!(validate_base_url("https://gateway.example.com", true).is_ok());
        assert!(validate_base_url("http://localhost:4000", true).is_ok());
        assert!(validate_base_url("http://gateway.example.com", true).is_err());
        assert!(validate_base_url("https://user:pw@gateway.example.com", true).is_err());
        assert!(validate_base_url("not a url", true).is_err());
    }

    #[test]
    fn claude_token_from_credentials_json() {
        let token = fake("oat");
        assert_eq!(parse_claude_token(&format!("  {token}\n")).unwrap(), token);
        let json = serde_json::json!({ "claudeAiOauth": { "accessToken": token, "refreshToken": "x" } }).to_string();
        assert_eq!(parse_claude_token(&json).unwrap(), token);
        assert!(parse_claude_token("{\"other\":1}").is_err());
    }

    #[test]
    fn summarizes_codex_auth_files() {
        // Header and signature are irrelevant; only the payload is read.
        let payload = serde_json::json!({
            "email": "dev@example.com",
            "https://api.openai.com/auth": { "chatgpt_plan_type": "plus" }
        })
        .to_string();
        let encoded = base64url_encode(payload.as_bytes());
        let chatgpt = serde_json::json!({ "tokens": { "id_token": format!("e30.{encoded}.sig"), "access_token": "placeholder" } });
        assert_eq!(
            summarize_codex_auth(&chatgpt).unwrap(),
            CodexAuthSummary::ChatGpt { email: Some("dev@example.com".into()), plan: Some("plus".into()) }
        );
        let key = serde_json::json!({ "OPENAI_API_KEY": fake("sk") });
        assert_eq!(summarize_codex_auth(&key).unwrap(), CodexAuthSummary::ApiKey);
        assert!(summarize_codex_auth(&serde_json::json!({ "tokens": {} })).is_err());
        assert!(summarize_codex_auth(&serde_json::json!([])).is_err());
    }

    fn base64url_encode(bytes: &[u8]) -> String {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let n = chunk.iter().enumerate().fold(0u32, |acc, (i, b)| acc | (*b as u32) << (16 - 8 * i));
            for i in 0..=chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            }
        }
        out
    }

    #[test]
    fn codex_methods_use_separate_homes() {
        assert_ne!(codex_home_for(&CodexAuth::ApiKey { base_url: String::new() }), codex_home_for(&CodexAuth::AuthJson));
        assert_eq!(codex_home_for(&CodexAuth::AuthJson), codex_home());
    }

    #[test]
    fn untrusted_codex_home_values_are_ignored() {
        let default = fsutil::home().join(".codex");
        assert_eq!(user_codex_home_from(None), default);
        // Relative (would resolve against the app's working directory), missing, or Agentty's own.
        assert_eq!(user_codex_home_from(Some("relative/codex".into())), default);
        assert_eq!(user_codex_home_from(Some(std::env::temp_dir().join("agentty-no-such-dir").into())), default);
        let own = codex_home();
        if own.is_dir() {
            assert_eq!(user_codex_home_from(Some(own.into())), default);
        }
        let real = std::env::temp_dir();
        assert_eq!(user_codex_home_from(Some(real.clone().into())), real);
    }

    #[test]
    fn verification_refuses_insecure_urls_before_sending_the_key() {
        // Fails validation locally, so no request is made (the host doesn't exist either way).
        let key = fake("sk");
        let err = verify_claude(&ClaudeAuth::ApiKey { base_url: "http://gateway.invalid".into() }, &key).unwrap_err();
        assert!(format!("{err:#}").contains("https"), "{err:#}");
        let err = verify_codex(&CodexAuth::ApiKey { base_url: "http://user:pw@gateway.invalid".into() }, &key).unwrap_err();
        assert!(format!("{err:#}").contains("https"), "{err:#}");
    }

    /// The junction's reparse data: header, then `\??\C:\x` and `C:\x` as NUL-terminated UTF-16.
    #[test]
    fn mount_point_buffer_layout() {
        let buffer = mount_point_buffer(r"C:\x");
        let u16_at = |i: usize| u16::from_le_bytes([buffer[i], buffer[i + 1]]);
        assert_eq!(u32::from_le_bytes(buffer[..4].try_into().unwrap()), 0xA000_0003);
        // "\??\C:\x" is 8 units, "C:\x" 4: 8 header bytes + 16 + 2 + 8 + 2.
        assert_eq!(u16_at(4), 36);
        assert_eq!(buffer.len(), 8 + 36);
        assert_eq!((u16_at(8), u16_at(10), u16_at(12), u16_at(14)), (0, 16, 18, 8));
        let text: Vec<u16> = buffer[16..].chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        assert_eq!(String::from_utf16(&text).unwrap(), "\\??\\C:\\x\0C:\\x\0");
    }

    #[test]
    fn masks_secrets() {
        assert_eq!(masked("short"), "••••");
        assert_eq!(masked(&fake("sk")), "••••0000");
    }

    #[test]
    fn codex_home_links_user_sessions() {
        let dir = std::env::temp_dir().join(format!("agentty-codex-home-{}", std::process::id()));
        let user = dir.join("user");
        std::fs::create_dir_all(user.join("sessions/2026")).unwrap();
        std::fs::write(user.join("config.toml"), "model = \"x\"\n").unwrap();
        std::env::set_var("CODEX_HOME", &user);
        let home = dir.join("private");
        prepare_codex_home(&home).unwrap();
        std::env::remove_var("CODEX_HOME");
        assert!(home.join("sessions/2026").is_dir());
        assert_eq!(std::fs::read_to_string(home.join("config.toml")).unwrap(), "model = \"x\"\n");
        assert!(!home.join("auth.json").exists());
        std::fs::remove_dir_all(dir).ok();
    }
}
