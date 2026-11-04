//! More than one CLI login per agent. Each extra login lives in a folder of its own
//! (`<data dir>/accounts/<agent>/<id>`), which the agent is pointed at with `CLAUDE_CONFIG_DIR` or
//! `CODEX_HOME`: its credentials, transcripts, history and usage stay there, while the user's
//! settings, skills and rules are linked in so every account behaves the same.
//!
//! Credentials stay with the CLIs themselves (the Keychain or a `0600` file, as each CLI chooses).
//! `accounts.json` only names the accounts. The login in `~/.claude` / `~/.codex` is not listed: it
//! is the account `None` stands for everywhere in this module, and nothing changes for it.

use crate::agent_auth::{self, AuthAgent, EnvChange};
use crate::fsutil;
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Entries of the user's Claude Code folder every account shares. The rest (`projects`, `sessions`,
/// `history.jsonl`, `.claude.json`) belongs to one account.
const CLAUDE_SHARED_DIRS: &[&str] = &["agents", "commands", "hooks", "output-styles", "plugins", "rules", "skills"];
const CLAUDE_SHARED_FILES: &[&str] = &["settings.json", "CLAUDE.md", "keybindings.json"];
const CODEX_SHARED_DIRS: &[&str] = &["memories", "prompts", "rules", "skills"];
const CODEX_SHARED_FILES: &[&str] = &["config.toml", "AGENTS.md"];
/// Settings of `.claude.json` a new account takes from the user's, so it doesn't start with the
/// first-run questions again.
const CLAUDE_FIRST_RUN_KEYS: &[&str] = &["hasCompletedOnboarding", "lastOnboardingVersion", "theme"];

/// One extra login, as far as Agentty needs to show it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    pub email: Option<String>,
    pub org: Option<String>,
    pub plan: Option<String>,
}

impl Account {
    /// Two logins are the same account when they name the same person in the same organization.
    pub fn same_login(&self, other: &Account) -> bool {
        self.email.is_some() && self.email == other.email && self.org == other.org
    }
}

/// The accounts of one window, or of one pane. `None` is the agent's own login.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AccountChoice {
    pub claude: Option<String>,
    pub codex: Option<String>,
}

impl AccountChoice {
    /// The account `agent` runs on here; `None` also for agents without accounts.
    pub fn of_agent(&self, agent: crate::model::Agent) -> Option<&str> {
        match agent {
            crate::model::Agent::Claude => self.claude.as_deref(),
            crate::model::Agent::Codex => self.codex.as_deref(),
            _ => None,
        }
    }

    pub fn get(&self, agent: AuthAgent) -> Option<&str> {
        match agent {
            AuthAgent::Claude => self.claude.as_deref(),
            AuthAgent::Codex => self.codex.as_deref(),
        }
    }

    pub fn set(&mut self, agent: AuthAgent, id: Option<String>) {
        match agent {
            AuthAgent::Claude => self.claude = id,
            AuthAgent::Codex => self.codex = id,
        }
    }

    /// Takes `other`'s account for each agent this choice leaves open.
    pub fn fill_from(&mut self, other: &AccountChoice) {
        self.claude = self.claude.take().or_else(|| other.claude.clone());
        self.codex = self.codex.take().or_else(|| other.codex.clone());
    }
}

/// `accounts.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AccountStore {
    pub claude: Vec<Account>,
    pub codex: Vec<Account>,
}

impl AccountStore {
    fn path() -> PathBuf {
        fsutil::data_dir().join("accounts.json")
    }

    pub fn load() -> Self {
        std::fs::read(Self::path()).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        fsutil::write_private(&Self::path(), &serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }

    pub fn of(&self, agent: AuthAgent) -> &[Account] {
        match agent {
            AuthAgent::Claude => &self.claude,
            AuthAgent::Codex => &self.codex,
        }
    }

    fn of_mut(&mut self, agent: AuthAgent) -> &mut Vec<Account> {
        match agent {
            AuthAgent::Claude => &mut self.claude,
            AuthAgent::Codex => &mut self.codex,
        }
    }

    /// Adds `account`, or refreshes what is known about the one with its id. A different id for a
    /// login already listed is left out and that account's id returned, for the caller to discard
    /// the new folder.
    pub fn add(&mut self, agent: AuthAgent, account: Account) -> Added {
        let list = self.of_mut(agent);
        if let Some(found) = list.iter().find(|known| known.id != account.id && known.same_login(&account)) {
            return Added::Duplicate(found.id.clone());
        }
        match list.iter_mut().find(|known| known.id == account.id) {
            Some(known) => *known = account,
            None => list.push(account),
        }
        Added::Listed
    }

    pub fn forget(&mut self, agent: AuthAgent, id: &str) {
        self.of_mut(agent).retain(|account| account.id != id);
    }

    pub fn contains(&self, agent: AuthAgent, id: &str) -> bool {
        self.of(agent).iter().any(|account| account.id == id)
    }
}

/// What [`AccountStore::add`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Added {
    Listed,
    Duplicate(String),
}

fn accounts_root(agent: AuthAgent) -> PathBuf {
    fsutil::data_dir().join("accounts").join(match agent {
        AuthAgent::Claude => "claude",
        AuthAgent::Codex => "codex",
    })
}

/// Ids are made here and name a folder that gets deleted: nothing else is ever taken for one.
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 40 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// The folder of account `id`.
pub fn home(agent: AuthAgent, id: &str) -> Result<PathBuf> {
    ensure!(valid_id(id), "not an account id");
    Ok(accounts_root(agent).join(id))
}

/// A new, empty account folder that isn't listed yet: the login pane fills it. Returns its id.
pub fn create_folder(agent: AuthAgent) -> Result<String> {
    let root = accounts_root(agent);
    std::fs::create_dir_all(&root)?;
    agent_auth::restrict_dir(&root)?;
    let mut stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    loop {
        let id = format!("a{stamp:x}");
        // `create_dir` is what claims the name: it fails when another call took it first.
        match std::fs::create_dir(root.join(&id)) {
            Ok(()) => {
                agent_auth::restrict_dir(&root.join(&id))?;
                return Ok(id);
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => stamp += 1,
            Err(err) => return Err(err.into()),
        }
    }
}

/// Deletes account `id`'s folder and everything in it, its transcripts included.
pub fn delete_folder(agent: AuthAgent, id: &str) -> Result<()> {
    let folder = home(agent, id)?;
    match std::fs::remove_dir_all(&folder) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
            Err(err).with_context(|| format!("could not delete {}", folder.display()))
        }
        _ => Ok(()),
    }
}

/// Every account folder on disk, listed or not: a session stays findable while its account's login
/// is still being finished, and after the list was damaged.
pub fn homes(agent: AuthAgent) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(accounts_root(agent)) else { return Vec::new() };
    let mut folders: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).filter(|path| path.is_dir()).collect();
    folders.sort();
    folders
}

/// The account a file inside an account folder belongs to; `None` for anything else.
pub fn owner_of(agent: AuthAgent, path: &Path) -> Option<String> {
    owner_in(&accounts_root(agent), path)
}

fn owner_in(root: &Path, path: &Path) -> Option<String> {
    let inside = path.strip_prefix(root).ok()?;
    let id = inside.components().next()?.as_os_str().to_str()?;
    valid_id(id).then(|| id.to_string())
}

/// Whether `path` is an account folder or inside one: a nested Agentty inherits `CLAUDE_CONFIG_DIR`
/// and `CODEX_HOME` from its pane, and those are not the user's own folders.
pub fn is_account_path(agent: AuthAgent, path: &Path) -> bool {
    path.starts_with(accounts_root(agent))
}

/// The user's own Claude Code folder.
fn user_claude_dir() -> PathBuf {
    match std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from) {
        Some(dir) if dir.is_absolute() && !is_account_path(AuthAgent::Claude, &dir) => dir,
        _ => fsutil::home().join(".claude"),
    }
}

/// The user's own `.claude.json`: inside their config folder only when they moved it themselves.
fn user_claude_config() -> PathBuf {
    match std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from) {
        Some(dir) if dir.is_absolute() && !is_account_path(AuthAgent::Claude, &dir) => dir.join(".claude.json"),
        _ => fsutil::home().join(".claude.json"),
    }
}

/// `.claude.json` of every account, for changes that must reach all of them.
pub fn claude_configs() -> Vec<PathBuf> {
    homes(AuthAgent::Claude).into_iter().map(|home| home.join(".claude.json")).collect()
}

/// Links what `from` has of the shared entries into `into`. One that can't be linked is reported
/// and skipped: the account then works without it rather than not at all.
fn link_shared(from: &Path, into: &Path, dirs: &[&str], files: &[&str]) {
    for name in dirs {
        let (source, link) = (from.join(name), into.join(name));
        if source.is_dir() && (std::fs::symlink_metadata(&link).is_err() || agent_auth::clear_dead_link(&link)) {
            if let Err(err) = agent_auth::link_dir(&source, &link) {
                eprintln!("agentty: could not link {name} into an account folder: {err:#}");
            }
        }
    }
    for name in files {
        let (source, link) = (from.join(name), into.join(name));
        if source.is_file() {
            agent_auth::refresh_file_link(&source, &link);
        }
    }
}

/// Brings `user`'s MCP servers, trusted folders and first-run answers into `account`. Returns
/// whether anything changed. The user's servers win on the same name; a server added in this
/// account only stays. Sign-in details are never copied: only the keys named here are.
fn seed_claude_config(user: &Value, account: &mut Value) -> bool {
    if !account.is_object() {
        *account = Value::Object(Default::default());
    }
    let mut changed = false;
    for key in CLAUDE_FIRST_RUN_KEYS {
        if account.get(*key).is_none() {
            if let Some(value) = user.get(*key) {
                account[*key] = value.clone();
                changed = true;
            }
        }
    }
    if let Some(servers) = user["mcpServers"].as_object() {
        if !account["mcpServers"].is_object() {
            account["mcpServers"] = Value::Object(Default::default());
        }
        for (name, server) in servers {
            if account["mcpServers"].get(name) != Some(server) {
                account["mcpServers"][name] = server.clone();
                changed = true;
            }
        }
    }
    if let Some(projects) = user["projects"].as_object() {
        for (folder, project) in projects {
            if project["hasTrustDialogAccepted"].as_bool() != Some(true) {
                continue;
            }
            if account["projects"][folder]["hasTrustDialogAccepted"].as_bool() != Some(true) {
                if !account["projects"].is_object() {
                    account["projects"] = Value::Object(Default::default());
                }
                if !account["projects"][folder].is_object() {
                    account["projects"][folder] = Value::Object(Default::default());
                }
                account["projects"][folder]["hasTrustDialogAccepted"] = Value::Bool(true);
                changed = true;
            }
        }
    }
    changed
}

/// Updates an account's `.claude.json` from the user's. A file that can't be read is left alone:
/// it holds this account's sign-in details and writing over it would lose them.
fn refresh_claude_config(user_path: &Path, account_path: &Path) -> Result<()> {
    let Ok(bytes) = std::fs::read(user_path) else { return Ok(()) };
    let user: Value = serde_json::from_slice(&bytes).context("~/.claude.json is not JSON")?;
    let mut account: Value = match std::fs::read(account_path) {
        Ok(bytes) => serde_json::from_slice(&bytes).context("the account's .claude.json is not JSON")?,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Value::Object(Default::default()),
        Err(err) => return Err(err.into()),
    };
    if seed_claude_config(&user, &mut account) {
        fsutil::write_private(account_path, &serde_json::to_vec_pretty(&account)?)?;
    }
    Ok(())
}

/// Readies account `id`'s folder for a pane: shared entries linked, settings up to date.
pub fn prepare(agent: AuthAgent, id: &str) -> Result<PathBuf> {
    let folder = home(agent, id)?;
    std::fs::create_dir_all(&folder)?;
    agent_auth::restrict_dir(&folder)?;
    match agent {
        AuthAgent::Claude => {
            link_shared(&user_claude_dir(), &folder, CLAUDE_SHARED_DIRS, CLAUDE_SHARED_FILES);
            refresh_claude_config(&user_claude_config(), &folder.join(".claude.json"))?;
        }
        AuthAgent::Codex => link_shared(&agent_auth::user_codex_home(), &folder, CODEX_SHARED_DIRS, CODEX_SHARED_FILES),
    }
    Ok(folder)
}

/// Environment for an agent pane on account `id`. Whatever a shell profile exports for another
/// sign-in method is cleared, or it would win over this login.
pub fn environment(agent: AuthAgent, id: &str) -> Result<Vec<EnvChange>> {
    let folder = prepare(agent, id)?.display().to_string();
    Ok(agent_auth::account_environment(agent, folder))
}

/// The account `claude auth status --json` describes; `None` when it isn't logged in.
pub fn parse_claude_status(status: &Value) -> Option<Account> {
    if status["loggedIn"].as_bool() != Some(true) {
        return None;
    }
    let text = |key: &str| status[key].as_str().filter(|value| !value.is_empty()).map(str::to_string);
    Some(Account { id: String::new(), email: text("email"), org: text("orgName"), plan: text("subscriptionType") })
}

/// The account in a Codex folder's `auth.json`; `None` when it has no ChatGPT login in it.
pub fn read_codex_login(folder: &Path) -> Option<Account> {
    let bytes = std::fs::read(folder.join("auth.json")).ok()?;
    match agent_auth::summarize_codex_auth(&serde_json::from_slice(&bytes).ok()?).ok()? {
        agent_auth::CodexAuthSummary::ChatGpt { email, plan } => Some(Account { id: String::new(), email, org: None, plan }),
        agent_auth::CodexAuthSummary::ApiKey => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("agentty-accounts-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn account(id: &str, email: &str, org: &str) -> Account {
        Account { id: id.into(), email: Some(email.into()), org: Some(org.into()), plan: None }
    }

    #[test]
    fn the_same_login_is_not_listed_twice() {
        let mut store = AccountStore::default();
        assert_eq!(store.add(AuthAgent::Claude, account("a1", "me@example.com", "Org")), Added::Listed);
        assert_eq!(store.add(AuthAgent::Claude, account("a2", "me@example.com", "Org")), Added::Duplicate("a1".into()));
        assert_eq!(store.claude.len(), 1);
        // The same person in another organization has limits of its own.
        assert_eq!(store.add(AuthAgent::Claude, account("a3", "me@example.com", "Other")), Added::Listed);
        // Each agent has its own list.
        assert_eq!(store.add(AuthAgent::Codex, account("a4", "me@example.com", "Org")), Added::Listed);
    }

    #[test]
    fn adding_an_id_again_refreshes_it() {
        let mut store = AccountStore::default();
        store.add(AuthAgent::Claude, account("a1", "me@example.com", "Org"));
        let mut newer = account("a1", "me@example.com", "Org");
        newer.plan = Some("max".into());
        assert_eq!(store.add(AuthAgent::Claude, newer), Added::Listed);
        assert_eq!(store.claude.len(), 1);
        assert_eq!(store.claude[0].plan.as_deref(), Some("max"));
    }

    #[test]
    fn logins_without_an_email_are_never_merged() {
        let unknown = |id: &str| Account { id: id.into(), ..Default::default() };
        let mut store = AccountStore::default();
        store.add(AuthAgent::Codex, unknown("a1"));
        assert_eq!(store.add(AuthAgent::Codex, unknown("a2")), Added::Listed);
    }

    #[test]
    fn an_id_cannot_climb_out_of_the_accounts_folder() {
        for bad in ["", "..", "../..", "a/b", "a\\b", "a.b", &"x".repeat(41)] {
            assert!(home(AuthAgent::Claude, bad).is_err(), "{bad}");
        }
        assert!(home(AuthAgent::Claude, "a18f3-2").is_ok());
    }

    #[test]
    fn a_file_belongs_to_the_account_folder_it_is_in() {
        let root = Path::new("/data/accounts/claude");
        assert_eq!(owner_in(root, Path::new("/data/accounts/claude/a1/projects/x/s.jsonl")).as_deref(), Some("a1"));
        assert_eq!(owner_in(root, Path::new("/home/example/.claude/projects/x/s.jsonl")), None);
        assert_eq!(owner_in(root, root), None);
    }

    #[test]
    fn a_choice_fills_only_what_it_leaves_open() {
        let window = AccountChoice { claude: Some("w1".into()), codex: Some("w2".into()) };
        let mut pane = AccountChoice { claude: Some("p1".into()), codex: None };
        pane.fill_from(&window);
        assert_eq!(pane, AccountChoice { claude: Some("p1".into()), codex: Some("w2".into()) });
    }

    #[test]
    fn a_new_account_takes_servers_trust_and_first_run_answers() {
        let user = json!({
            "hasCompletedOnboarding": true,
            "theme": "dark",
            "mcpServers": { "fake": { "command": "true" } },
            "projects": { "/p/trusted": { "hasTrustDialogAccepted": true, "history": ["x"] }, "/p/other": { "hasTrustDialogAccepted": false } },
            "oauthAccount": { "emailAddress": "user@example.com" },
        });
        let mut account = json!({});
        assert!(seed_claude_config(&user, &mut account));
        assert_eq!(account["hasCompletedOnboarding"], true);
        assert_eq!(account["mcpServers"]["fake"]["command"], "true");
        assert_eq!(account["projects"]["/p/trusted"]["hasTrustDialogAccepted"], true);
        assert!(account["projects"]["/p/trusted"].get("history").is_none(), "only the flag travels");
        assert!(account["projects"].get("/p/other").is_none());
        assert!(account.get("oauthAccount").is_none(), "another login's details must not travel");
    }

    #[test]
    fn seeding_keeps_what_belongs_to_the_account() {
        let user = json!({ "theme": "dark", "mcpServers": { "shared": { "command": "new" } } });
        let mut account = json!({
            "theme": "light",
            "oauthAccount": { "emailAddress": "work@example.com" },
            "mcpServers": { "shared": { "command": "old" }, "mine": { "command": "true" } },
        });
        assert!(seed_claude_config(&user, &mut account));
        assert_eq!(account["theme"], "light");
        assert_eq!(account["oauthAccount"]["emailAddress"], "work@example.com");
        assert_eq!(account["mcpServers"]["shared"]["command"], "new");
        assert_eq!(account["mcpServers"]["mine"]["command"], "true");
    }

    #[test]
    fn seeding_twice_changes_nothing_the_second_time() {
        let user = json!({ "theme": "dark", "mcpServers": { "a": {} }, "projects": { "/p": { "hasTrustDialogAccepted": true } } });
        let mut account = json!({});
        assert!(seed_claude_config(&user, &mut account));
        assert!(!seed_claude_config(&user, &mut account));
    }

    #[test]
    fn a_damaged_account_config_is_not_written_over() {
        let dir = scratch("damaged");
        let (user, config) = (dir.join("user.json"), dir.join("account.json"));
        std::fs::write(&user, r#"{"theme":"dark"}"#).unwrap();
        std::fs::write(&config, "{ damaged").unwrap();
        assert!(refresh_claude_config(&user, &config).is_err());
        assert_eq!(std::fs::read_to_string(&config).unwrap(), "{ damaged");
    }

    #[test]
    fn config_is_written_only_when_something_changed() {
        let dir = scratch("changed");
        let (user, config) = (dir.join("user.json"), dir.join("account.json"));
        std::fs::write(&user, r#"{"theme":"dark"}"#).unwrap();
        refresh_claude_config(&user, &config).unwrap();
        assert!(std::fs::read_to_string(&config).unwrap().contains("dark"));
        std::fs::write(&config, r#"{"theme":"dark","kept":1}"#).unwrap();
        refresh_claude_config(&user, &config).unwrap();
        assert_eq!(std::fs::read_to_string(&config).unwrap(), r#"{"theme":"dark","kept":1}"#);
    }

    #[cfg(unix)]
    #[test]
    fn only_shared_entries_are_linked() {
        let dir = scratch("links");
        let (user, folder) = (dir.join("user"), dir.join("account"));
        std::fs::create_dir_all(user.join("rules")).unwrap();
        std::fs::create_dir_all(user.join("projects")).unwrap();
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(user.join("settings.json"), "{}").unwrap();
        std::fs::write(user.join("history.jsonl"), "").unwrap();
        link_shared(&user, &folder, CLAUDE_SHARED_DIRS, CLAUDE_SHARED_FILES);
        assert!(std::fs::symlink_metadata(folder.join("rules")).unwrap().file_type().is_symlink());
        assert!(std::fs::symlink_metadata(folder.join("settings.json")).unwrap().file_type().is_symlink());
        assert!(!folder.join("projects").exists());
        assert!(!folder.join("history.jsonl").exists());
        // A second start finds them in place.
        link_shared(&user, &folder, CLAUDE_SHARED_DIRS, CLAUDE_SHARED_FILES);
        assert!(folder.join("rules").is_dir());
    }

    #[test]
    fn claude_status_names_the_login() {
        let status = json!({ "loggedIn": true, "email": "me@example.com", "orgName": "Org", "subscriptionType": "max" });
        let found = parse_claude_status(&status).unwrap();
        assert_eq!(
            (found.email.as_deref(), found.org.as_deref(), found.plan.as_deref()),
            (Some("me@example.com"), Some("Org"), Some("max"))
        );
        assert_eq!(parse_claude_status(&json!({ "loggedIn": false })), None);
        assert_eq!(parse_claude_status(&json!({})), None);
    }
}
