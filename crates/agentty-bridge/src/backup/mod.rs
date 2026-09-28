//! The user's configuration as one file: exported by hand, imported on another computer or after
//! a reinstall, and kept in step between computers through the sync repository
//! (`settings/<device id>.json`, see [`crate::sync`]).
//!
//! What travels:
//! - `settings.json` without what belongs to one computer ([`DEVICE_SETTINGS`]: panel sizes, recent
//!   folders, timers, first-run state);
//! - the files beside it that describe how the user works ([`FILES`]): custom commands, connectors,
//!   database connections, how agents sign in;
//! - imported terminal themes (`themes/*.itermcolors`);
//! - installed plugins, as a list to install again (never their code);
//! - on request, the secrets those settings refer to, read from the credential store and sealed
//!   with a password ([`crypto`]). Secrets are never synced.
//!
//! Browser site sessions, workspaces and agent conversations are not part of it (session sync
//! carries workspaces and conversations).

pub mod crypto;

use crate::plugins::store::{self, Source, StateFile};
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

pub const KIND: &str = "agentty-config";
pub const FORMAT_VERSION: u32 = 1;
/// Extension of an exported file.
pub const EXTENSION: &str = "agenttyconfig";

/// Credential-store service of saved database passwords (`agentty-db` saves them there).
pub const DATABASE_SERVICE: &str = "run.agentty.database";

/// Top-level keys of `settings.json` that describe this computer rather than the user's choices:
/// they are never exported, and an import keeps this computer's values.
pub const DEVICE_SETTINGS: &[&str] = &[
    "settingsVersion",
    "sidebarWidth",
    "filesPanelWidth",
    "pluginPanelWidth",
    "pluginWorkspacePanelWidth",
    "pluginWorkspaceTerminalHeight",
    "dockerPanelWidth",
    "dbPanelWidth",
    "filesPanelTreesHeight",
    "recentDirs",
    "favoriteSessions",
    "preventSleep",
    "preventSleepHours",
    "preventSleepUntil",
    "setupCheckShown",
    "onboardingDone",
    "updateLaterVersion",
    "updateLaterUntil",
];

/// A configuration file beside `settings.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigFile {
    pub name: &'static str,
    /// Also kept in step by sync. Files tied to one computer's secrets or folders are not: a
    /// sign-in method without its key on this computer would break the agents here.
    pub synced: bool,
}

pub const FILES: &[ConfigFile] = &[
    ConfigFile { name: "commands.json", synced: true },
    ConfigFile { name: "connectors.json", synced: true },
    ConfigFile { name: "db-connections.json", synced: false },
    ConfigFile { name: "agent-auth.json", synced: false },
];

const THEME_EXTENSION: &str = "itermcolors";
/// A theme file larger than this is not a colour scheme.
const MAX_THEME_BYTES: usize = 512 * 1024;
const MAX_BUNDLE_BYTES: u64 = 32 * 1024 * 1024;

/// An exported configuration.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Bundle {
    pub kind: String,
    pub version: u32,
    pub created_at: String,
    pub app_version: String,
    pub device_name: String,
    /// `settings.json` without [`DEVICE_SETTINGS`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings: Option<Value>,
    /// The version `settings` was written by, so an older one is migrated on import.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings_version: Option<u64>,
    /// File name → its JSON.
    pub files: BTreeMap<String, Value>,
    /// Theme file name → its text.
    pub themes: BTreeMap<String, String>,
    pub plugins: Vec<PluginEntry>,
    /// Plugin id → its settings (what it keeps with `storage/set`). A plugin's settings holding
    /// something shaped like a credential are not here but among the sealed secrets, or left out
    /// without a password. Export only: sync keeps plugin settings in their own files.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub plugin_settings: BTreeMap<String, Value>,
    /// [`Secret`]s as JSON, sealed with the export password.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secrets: Option<crypto::Sealed>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginEntry {
    pub id: String,
    pub source: Source,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    pub enabled: bool,
}

/// A value from the credential store. `service` is the unscoped name: a second install (its own
/// data folder) scopes it again on import.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Secret {
    pub service: String,
    pub account: String,
    pub value: String,
}

/// What an export covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Everything, for a file the user keeps.
    Full,
    /// What sync keeps in step: settings, commands, connectors and themes.
    Sync,
}

// ---------------------------------------------------------------------------------------------
// Collect

/// Reads the configuration from the data folder. `password` seals the secrets in; without one
/// the file has none.
pub fn collect(scope: Scope, password: Option<&str>, app_version: &str) -> Result<Bundle> {
    collect_from(&crate::fsutil::data_dir(), scope, password, app_version, &store_secrets)
}

/// Reads one secret; `None` when there is none.
type ReadSecret<'a> = dyn Fn(&str, &str) -> Option<String> + 'a;

fn store_secrets(service: &str, account: &str) -> Option<String> {
    crate::secret_store::load(&scoped(service), account).ok().filter(|v| !v.is_empty())
}

fn scoped(service: &str) -> String {
    crate::connectors::scoped_service(service, std::env::var_os("AGENTTY_DATA_DIR").as_deref())
}

fn collect_from(dir: &Path, scope: Scope, password: Option<&str>, app_version: &str, read_secret: &ReadSecret) -> Result<Bundle> {
    let mut bundle = Bundle {
        kind: KIND.into(),
        version: FORMAT_VERSION,
        created_at: crate::sync::model::now_stamp(),
        app_version: app_version.into(),
        device_name: crate::sync::computer_name(),
        ..Default::default()
    };
    if let Some(settings) = read_json(&dir.join("settings.json")) {
        bundle.settings_version = settings.get("settingsVersion").and_then(Value::as_u64);
        bundle.settings = Some(portable_settings(settings));
    }
    for file in FILES.iter().filter(|f| scope == Scope::Full || f.synced) {
        if let Some(mut value) = read_json(&dir.join(file.name)) {
            if file.name == "db-connections.json" {
                map_projects(&mut value, &home_to_tilde);
            }
            bundle.files.insert(file.name.into(), value);
        }
    }
    bundle.themes = read_themes(&dir.join("themes"));
    let mut sealed_plugins = Vec::new();
    if scope == Scope::Full {
        bundle.plugins = plugin_list(&StateFile::load());
        for (id, value) in plugin_stores(dir) {
            if crate::sync::plugin_settings::holds_no_secret(&value) {
                bundle.plugin_settings.insert(id, value);
            } else {
                // Holding a credential: only sealed with the password, like the other secrets.
                sealed_plugins.push(Secret { service: PLUGIN_SETTINGS_SERVICE.into(), account: id, value: value.to_string() });
            }
        }
    }
    if let Some(password) = password {
        let mut secrets = secrets_of(&bundle, read_secret);
        secrets.extend(sealed_plugins);
        bundle.secrets = Some(crypto::seal(password, &serde_json::to_vec(&secrets)?)?);
    }
    Ok(bundle)
}

/// Stands in a sealed [`Secret`] for a plugin's settings that hold a credential (not a keychain
/// service: the value goes back into the plugin's `storage.json`).
const PLUGIN_SETTINGS_SERVICE: &str = "agentty.plugin-settings";

/// Each plugin's settings kept in `dir` (`plugin-data/<id>/storage.json`).
fn plugin_stores(dir: &Path) -> Vec<(String, Value)> {
    let mut stores = Vec::new();
    for entry in fs::read_dir(dir.join("plugin-data")).into_iter().flatten().flatten() {
        let id = entry.file_name().to_string_lossy().to_string();
        if !crate::plugins::manifest::valid_id(&id) {
            continue;
        }
        if let Some(value) = read_json(&entry.path().join("storage.json")).filter(Value::is_object) {
            stores.push((id, value));
        }
    }
    stores
}

/// Puts a plugin's settings in place (`0600`, in its `0700` folder).
fn write_plugin_store(dir: &Path, id: &str, value: &Value) -> Result<()> {
    anyhow::ensure!(crate::plugins::manifest::valid_id(id) && value.is_object(), "not a plugin's settings");
    let folder = dir.join("plugin-data").join(id);
    fs::create_dir_all(&folder)?;
    crate::sync::repo::private_dir(&folder);
    crate::fsutil::write_private(&folder.join("storage.json"), &serde_json::to_vec(value)?)?;
    Ok(())
}

fn read_json(path: &Path) -> Option<Value> {
    fs::read(path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

fn portable_settings(mut settings: Value) -> Value {
    if let Some(map) = settings.as_object_mut() {
        for key in DEVICE_SETTINGS {
            map.remove(*key);
        }
    }
    settings
}

fn read_themes(dir: &Path) -> BTreeMap<String, String> {
    let mut themes = BTreeMap::new();
    let Ok(entries) = fs::read_dir(dir) else { return themes };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !theme_name_ok(&name) {
            continue;
        }
        let Ok(text) = fs::read_to_string(entry.path()) else { continue };
        if text.len() <= MAX_THEME_BYTES {
            themes.insert(name, text);
        }
    }
    themes
}

fn theme_name_ok(name: &str) -> bool {
    Path::new(name).extension().is_some_and(|e| e.eq_ignore_ascii_case(THEME_EXTENSION))
        && name.len() <= 200
        && !name.starts_with('.')
        && !name.contains(['/', '\\', '\0'])
        && name != ".."
}

fn plugin_list(state: &StateFile) -> Vec<PluginEntry> {
    state
        .plugins
        .iter()
        .map(|(id, p)| PluginEntry { id: id.clone(), source: p.source, origin: p.origin.clone(), enabled: p.enabled })
        .collect()
}

/// Every secret the bundle's settings refer to, as far as the credential store has them.
fn secrets_of(bundle: &Bundle, read_secret: &ReadSecret) -> Vec<Secret> {
    let mut slots: Vec<(String, String)> = Vec::new();
    for id in ids(bundle.files.get("connectors.json").and_then(|v| v.get("connectors"))) {
        slots.push((crate::connectors::KEYCHAIN_SERVICE.into(), id));
    }
    for id in ids(bundle.files.get("db-connections.json")) {
        slots.push((DATABASE_SERVICE.into(), id));
    }
    if bundle.files.contains_key("agent-auth.json") {
        for account in crate::agent_auth::account::ALL {
            slots.push((crate::agent_auth::SERVICE.into(), account.to_string()));
        }
    }
    for channel in crate::notify::Channel::ALL {
        for transport in crate::notify::Transport::ALL {
            if channel.supports(transport) {
                slots.push((crate::notify::KEYCHAIN_SERVICE.into(), crate::notify::account(channel, transport)));
            }
        }
    }
    slots
        .into_iter()
        .filter_map(|(service, account)| read_secret(&service, &account).map(|value| Secret { service, account, value }))
        .collect()
}

/// The `id` of every object in a JSON array.
fn ids(list: Option<&Value>) -> Vec<String> {
    list.and_then(Value::as_array)
        .map(|items| items.iter().filter_map(|i| i.get("id")?.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

/// Rewrites the `project` folder of each database connection.
fn map_projects(value: &mut Value, map: &dyn Fn(&str) -> Option<String>) {
    let Some(items) = value.as_array_mut() else { return };
    for item in items {
        if let Some(project) = item.get_mut("project") {
            if let Some(mapped) = project.as_str().and_then(map) {
                *project = Value::String(mapped);
            }
        }
    }
}

/// `/Users/me/src/app` → `~/src/app`, so the connection finds the same folder under another home.
fn home_to_tilde(path: &str) -> Option<String> {
    let home = crate::fsutil::home();
    let rest = Path::new(path).strip_prefix(&home).ok()?;
    Some(format!("~/{}", rest.to_string_lossy().replace('\\', "/")))
}

fn tilde_to_home(path: &str) -> Option<String> {
    let rest = path.strip_prefix("~/")?;
    // Written with `/`: each part joined on its own, so Windows gets its own separator.
    let full = rest.split('/').filter(|part| !part.is_empty()).fold(crate::fsutil::home(), |dir, part| dir.join(part));
    Some(full.to_string_lossy().to_string())
}

// ---------------------------------------------------------------------------------------------
// File

pub fn write_file(path: &Path, bundle: &Bundle) -> Result<()> {
    // It can hold sealed secrets and says how the user works: private like the settings.
    crate::fsutil::write_private(path, &serde_json::to_vec_pretty(bundle)?)?;
    Ok(())
}

pub fn read_file(path: &Path) -> Result<Bundle> {
    let size = fs::metadata(path).with_context(|| format!("{} cannot be read", path.display()))?.len();
    if size > MAX_BUNDLE_BYTES {
        bail!("{} is too large to be an Agentty configuration", path.display());
    }
    parse(&fs::read(path)?)
}

pub fn parse(bytes: &[u8]) -> Result<Bundle> {
    let bundle: Bundle = serde_json::from_slice(bytes).map_err(|_| anyhow!("this is not an Agentty configuration file"))?;
    if bundle.kind != KIND {
        bail!("this is not an Agentty configuration file");
    }
    if bundle.version > FORMAT_VERSION {
        bail!("this configuration was written by a newer Agentty; update Agentty first");
    }
    Ok(bundle)
}

/// A suggested file name: `agentty-config-2026-09-27.agenttyconfig`.
pub fn suggested_file_name() -> String {
    format!("agentty-config-{}.{EXTENSION}", chrono::Local::now().format("%Y-%m-%d"))
}

// ---------------------------------------------------------------------------------------------
// Apply

/// Why an import did not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportError {
    WrongPassword,
    Failed(String),
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportError::WrongPassword => f.write_str("the password does not open the secrets in this file"),
            ImportError::Failed(reason) => f.write_str(reason),
        }
    }
}

impl std::error::Error for ImportError {}

impl From<anyhow::Error> for ImportError {
    fn from(err: anyhow::Error) -> Self {
        ImportError::Failed(format!("{err:#}"))
    }
}

#[derive(Debug, Clone, Default)]
pub struct ImportOptions {
    /// Opens the secrets. `None` imports everything else and leaves the secrets out.
    pub password: Option<String>,
    /// Installs the plugins the file lists that are missing here (downloads them).
    pub plugins: bool,
    /// Where the configuration as it was before goes (`None`: no copy).
    pub keep_copy_in: Option<PathBuf>,
    pub app_version: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportReport {
    /// `settings.json` merged with this computer's own values; the app puts it in place (it
    /// holds the settings in memory and must not be written under).
    pub settings: Option<Value>,
    pub files: usize,
    pub themes: usize,
    pub secrets: usize,
    /// Plugins whose settings were put in place.
    pub plugin_settings: usize,
    pub plugins_installed: Vec<String>,
    /// Listed plugins that could not come back (a folder on the other computer, a failed download).
    pub plugins_skipped: Vec<String>,
    /// The configuration from before the import.
    pub copy: Option<PathBuf>,
}

/// Puts a configuration in place of this computer's. Settings not in the file keep their values;
/// commands, connectors and the other files are replaced as a whole. Blocking (credential store,
/// downloads): run it off the UI thread.
pub fn import(bundle: &Bundle, options: &ImportOptions) -> Result<ImportReport, ImportError> {
    let secrets = match (&bundle.secrets, &options.password) {
        (Some(sealed), Some(password)) => {
            let plain = crypto::open(password, sealed).map_err(|e| match e {
                crypto::OpenError::WrongPassword => ImportError::WrongPassword,
                crypto::OpenError::Malformed => ImportError::Failed("the secrets in this file are damaged".into()),
            })?;
            serde_json::from_slice::<Vec<Secret>>(&plain).map_err(|_| ImportError::Failed("the secrets in this file are damaged".into()))?
        }
        _ => Vec::new(),
    };
    let dir = crate::fsutil::data_dir();
    let mut report = ImportReport::default();
    if let Some(folder) = &options.keep_copy_in {
        // With the secrets that are about to be replaced, sealed with the same password.
        let before = collect(Scope::Full, options.password.as_deref().filter(|_| !secrets.is_empty()), &options.app_version)?;
        let path = folder.join(format!("before-import-{}.{EXTENSION}", chrono::Local::now().format("%Y%m%d-%H%M%S")));
        write_file(&path, &before)?;
        report.copy = Some(path);
    }
    apply_to(&dir, bundle, &mut report)?;
    for secret in &secrets {
        if secret.service == PLUGIN_SETTINGS_SERVICE {
            if let Ok(value) = serde_json::from_str::<Value>(&secret.value) {
                if write_plugin_store(&dir, &secret.account, &value).is_ok() {
                    report.plugin_settings += 1;
                }
            }
            continue;
        }
        if !known_service(&secret.service) {
            continue;
        }
        crate::secret_store::store(&scoped(&secret.service), &secret.account, &secret.value)
            .map_err(|_| ImportError::Failed("a secret could not be saved in the credential store".into()))?;
        report.secrets += 1;
    }
    if report.secrets > 0 {
        crate::notify::forget_cached_secrets();
    }
    if options.plugins {
        install_plugins(&bundle.plugins, &mut report);
    }
    Ok(report)
}

/// Only the services this module exports: a file cannot plant a value anywhere else.
fn known_service(service: &str) -> bool {
    [crate::connectors::KEYCHAIN_SERVICE, DATABASE_SERVICE, crate::agent_auth::SERVICE, crate::notify::KEYCHAIN_SERVICE].contains(&service)
}

/// Writes the files and themes of `bundle` into `dir`, and merges its settings (returned in the
/// report). Secrets and plugins are separate steps.
fn apply_to(dir: &Path, bundle: &Bundle, report: &mut ImportReport) -> Result<()> {
    if let Some(incoming) = &bundle.settings {
        let current = read_json(&dir.join("settings.json")).unwrap_or(Value::Object(Default::default()));
        report.settings = Some(merge_settings(current, incoming, bundle.settings_version));
    }
    for file in FILES {
        let Some(value) = bundle.files.get(file.name) else { continue };
        let mut value = value.clone();
        if file.name == "db-connections.json" {
            map_projects(&mut value, &tilde_to_home);
        }
        crate::fsutil::write_private(&dir.join(file.name), &serde_json::to_vec_pretty(&value)?)?;
        report.files += 1;
    }
    for (id, value) in &bundle.plugin_settings {
        if write_plugin_store(dir, id, value).is_ok() {
            report.plugin_settings += 1;
        }
    }
    let themes = dir.join("themes");
    for (name, text) in &bundle.themes {
        if !theme_name_ok(name) || text.len() > MAX_THEME_BYTES {
            continue;
        }
        fs::create_dir_all(&themes)?;
        fs::write(themes.join(name), text)?;
        report.themes += 1;
    }
    Ok(())
}

/// This computer's settings with the portable ones from `incoming` put over them. An older file's
/// settings version is kept, so the app migrates what it brought when it loads it.
pub fn merge_settings(current: Value, incoming: &Value, incoming_version: Option<u64>) -> Value {
    let mut merged = match current {
        Value::Object(map) => map,
        _ => Default::default(),
    };
    if let Some(incoming) = incoming.as_object() {
        for (key, value) in incoming {
            if !DEVICE_SETTINGS.contains(&key.as_str()) {
                merged.insert(key.clone(), value.clone());
            }
        }
    }
    let here = merged.get("settingsVersion").and_then(Value::as_u64);
    if let (Some(theirs), Some(here)) = (incoming_version, here) {
        if theirs < here {
            merged.insert("settingsVersion".into(), theirs.into());
        }
    }
    Value::Object(merged)
}

fn install_plugins(plugins: &[PluginEntry], report: &mut ImportReport) {
    let installed: Vec<String> = store::installed().into_iter().map(|p| p.id).collect();
    let mut market: Option<Vec<crate::plugins::market::Entry>> = None;
    for plugin in plugins {
        if !installed.contains(&plugin.id) {
            let result = match plugin.source {
                Source::Builtin => store::install_builtin(&plugin.id).map(|_| ()),
                Source::Market => {
                    if market.is_none() {
                        market = crate::plugins::market::fetch().ok();
                    }
                    match market.as_ref().and_then(|m| m.iter().find(|e| e.id == plugin.id)) {
                        Some(entry) => crate::plugins::market::install(entry).map(|_| ()),
                        None => Err(anyhow!("not in the marketplace")),
                    }
                }
                Source::Git => match &plugin.origin {
                    Some(url) => store::install_from_git(url).map(|_| ()),
                    None => Err(anyhow!("no repository")),
                },
                // A folder on the other computer.
                Source::Folder | Source::Local | Source::Dev => Err(anyhow!("local")),
            };
            match result {
                Ok(()) => report.plugins_installed.push(plugin.id.clone()),
                Err(_) => {
                    report.plugins_skipped.push(plugin.id.clone());
                    continue;
                }
            }
        }
        let _ = store::set_enabled(&plugin.id, plugin.enabled);
    }
}

// ---------------------------------------------------------------------------------------------
// Sync

/// What sync compares: the synced parts of a bundle, keys sorted, as a digest. Two computers with
/// the same configuration get the same fingerprint whatever order their files keep keys in.
pub fn fingerprint(bundle: &Bundle) -> String {
    use sha2::{Digest, Sha256};
    let parts = serde_json::json!({
        "settings": bundle.settings,
        "files": bundle.files,
        "themes": bundle.themes,
    });
    let mut text = String::new();
    canonical(&parts, &mut text);
    Sha256::digest(text.as_bytes()).iter().take(16).map(|b| format!("{b:02x}")).collect()
}

fn canonical(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                canonical(&map[key], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical(item, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

/// Applies the synced parts of a bundle another computer wrote (no secrets, no plugins).
pub fn import_synced(bundle: &Bundle) -> Result<ImportReport> {
    import_synced_to(&crate::fsutil::data_dir(), bundle)
}

fn import_synced_to(dir: &Path, bundle: &Bundle) -> Result<ImportReport> {
    let mut synced = bundle.clone();
    synced.files.retain(|name, _| FILES.iter().any(|f| f.name == name && f.synced));
    synced.plugins.clear();
    // Plugins' settings travel in their own files (`sync::plugin_settings`).
    synced.plugin_settings.clear();
    synced.secrets = None;
    // A part this computer keeps to itself (it holds a credential, so it is never uploaded) is not
    // replaced either: another computer's newer copy would drop the command holding it.
    let local = collect_from(dir, Scope::Sync, None, "", &|_, _| None)?;
    let (_, held) = crate::sync::settings::hold_back_secrets(&local);
    for part in &held {
        match part.as_str() {
            "settings.json" => synced.settings = None,
            name => match name.strip_prefix("themes/") {
                Some(theme) => {
                    synced.themes.remove(theme);
                }
                None => {
                    synced.files.remove(name);
                }
            },
        }
    }
    let mut report = ImportReport::default();
    apply_to(dir, &synced, &mut report)?;
    Ok(report)
}

#[cfg(test)]
mod tests;
