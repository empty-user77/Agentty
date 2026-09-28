//! Session sync: workspaces and their agent sessions follow the user between computers through a
//! private git repository they own. See `docs/worklog/2026-09-26-session-sync.md`.
//!
//! Every device writes only its own files (its device file, its section of each workspace's
//! metadata, its sessions' folders), so pushes never conflict on content: a push that loses the
//! race fetches, resets to the remote and writes its own files again.

pub mod mask;
pub mod model;
pub mod repo;
pub mod restore;
pub mod session;
pub mod settings;
#[cfg(test)]
mod tests;

use crate::model::Agent;
use anyhow::{Context, Result};
use model::*;
use repo::{Author, Pushed, Remote, Repo, Visibility};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// This computer's sync settings: `<data dir>/sync/config.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SyncConfig {
    pub remote: Option<Remote>,
    pub branch: String,
    pub device_id: String,
    pub device_name: String,
    pub paused: bool,
    /// Local workspace (`<window slot>:<workspace id>`) → its id in the repository.
    pub workspaces: BTreeMap<String, String>,
    pub last_sync_at: Option<String>,
    /// Sessions continued here from another computer (`<agent>:<id>` → where they came from).
    pub origins: BTreeMap<String, ParentRef>,
    /// Settings travel through the repository too (`settings/<device id>.json`).
    pub settings: bool,
    /// The synced configuration as last seen here, and when it last changed.
    pub settings_fingerprint: String,
    pub settings_changed_at: String,
}

impl SyncConfig {
    pub fn connected(&self) -> bool {
        self.remote.is_some()
    }

    /// The repository id of a local workspace, made up the first time it is asked for.
    pub fn sync_id_for(&mut self, local: &str) -> String {
        self.workspaces.entry(local.to_string()).or_insert_with(new_id).clone()
    }
}

/// Serialises read-change-write of the config between windows of this app.
static CONFIG_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Loads the config, changes it and saves it, as one step.
pub fn update_config<T>(change: impl FnOnce(&mut SyncConfig) -> T) -> Result<T> {
    let _guard = CONFIG_LOCK.lock().map_err(|_| anyhow::anyhow!("sync settings are locked"))?;
    let mut config = load_config();
    let out = change(&mut config);
    save_config(&config)?;
    Ok(out)
}

pub fn sync_dir() -> PathBuf {
    crate::fsutil::data_dir().join("sync")
}

fn config_path() -> PathBuf {
    sync_dir().join("config.json")
}

fn clone_dir() -> PathBuf {
    sync_dir().join("repo")
}

/// The saved settings, with a device id and name made up on first use.
pub fn load_config() -> SyncConfig {
    let mut config: SyncConfig = fs::read_to_string(config_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let mut dirty = false;
    if config.device_id.is_empty() {
        config.device_id = new_id();
        dirty = true;
    }
    if config.device_name.trim().is_empty() {
        config.device_name = computer_name();
        dirty = true;
    }
    if config.branch.is_empty() {
        config.branch = "main".into();
    }
    // Nothing is written until a repository is connected: opening the app leaves no sync file.
    if dirty && config.connected() {
        let _ = save_config(&config);
    }
    config
}

pub fn save_config(config: &SyncConfig) -> Result<()> {
    fs::create_dir_all(sync_dir())?;
    repo::private_dir(&sync_dir());
    crate::fsutil::write_private(&config_path(), serde_json::to_string_pretty(config)?.as_bytes())?;
    Ok(())
}

/// Why a sync did not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncError {
    NotConnected,
    /// The repository can be read by anyone: sync stops until it is private again.
    Public,
    /// Another sync is running (this app or a second one on the same data folder).
    Busy,
    /// The repository holds other things than Agentty's sync.
    NotSyncRepository,
    Failed(String),
}

impl std::fmt::Display for SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SyncError::NotConnected => f.write_str("sync is not connected to a repository"),
            SyncError::Public => f.write_str("the sync repository is public"),
            SyncError::Busy => f.write_str("another sync is running"),
            SyncError::NotSyncRepository => f.write_str("the repository is not empty and is not an Agentty sync repository"),
            SyncError::Failed(reason) => f.write_str(reason),
        }
    }
}

impl std::error::Error for SyncError {}

impl From<anyhow::Error> for SyncError {
    fn from(err: anyhow::Error) -> Self {
        SyncError::Failed(format!("{err:#}"))
    }
}

/// A workspace as the app sees it now.
#[derive(Debug, Clone)]
pub struct WorkspaceInput {
    /// This computer's key for it (`<window slot>:<workspace id>`).
    pub local_key: String,
    /// Its id in the repository; `None` the first time, when the sync finds or makes one.
    pub sync_id: Option<String>,
    pub plugin: Option<String>,
    pub name: Option<String>,
    pub group: Option<String>,
    pub color: Option<u32>,
    pub cwd: PathBuf,
    pub state: WorkspaceState,
    pub sessions: Vec<SessionInput>,
}

#[derive(Debug, Clone)]
pub struct SessionInput {
    pub agent: Agent,
    pub id: String,
    pub title: String,
    /// In a tab right now.
    pub open: bool,
    /// Where the pane works (defaults to the workspace's folder).
    pub cwd: Option<PathBuf>,
    pub parent: Option<ParentRef>,
    /// `id` is not known (an agent run as a command): the newest session of `agent` in `cwd`
    /// written after this time is the pane's.
    pub started_ms: Option<u64>,
}

/// Agents whose session is one text file that only grows: those are synced as files. Antigravity
/// keeps SQLite databases (chunks of those would be broken) and Amp keeps its threads on its own
/// servers (they follow the user already): their sessions are listed, never uploaded.
pub fn syncs_files(agent: Agent) -> bool {
    matches!(agent, Agent::Claude | Agent::Codex | Agent::Gemini | Agent::Kimi)
}

#[derive(Debug, Clone, Default)]
pub struct SyncRequest {
    pub workspaces: Vec<WorkspaceInput>,
    /// A new retention period chosen here (days), written to the repository for every computer.
    pub retention: Option<u32>,
    /// Old sessions the user agreed to delete (from `SyncOutcome::prune_pending`). Only these
    /// go, and only while they are still past the period.
    pub prune: Vec<PruneItem>,
    /// Workspaces whose data is removed from the repository ("Also delete sync data").
    pub purge: Vec<(Option<String>, String)>,
    pub app_version: String,
    /// This computer's configuration; read by the sync itself when settings are synced.
    pub settings: Option<crate::backup::Bundle>,
}

/// What the repository holds, for the sync popover. Read from the clone, no network.
#[derive(Debug, Clone, Default)]
pub struct Overview {
    /// One per Agentty data folder that synced here, newest first. Several can share a computer
    /// (a reinstall, a second app): `computers` counts those once.
    pub devices: Vec<DeviceInfo>,
    pub workspaces: Vec<WorkspaceMetadata>,
    /// Ids of workspaces whose data was deleted from the repository ("Also delete sync data").
    pub removed: Vec<String>,
    /// How long a session is kept after it last changed (`retention.json`).
    pub retention_days: u32,
}

impl Overview {
    /// One device per computer (the one that synced last), newest first. A device written before
    /// machine keys existed joins the computer of the same name that has one.
    pub fn computers(&self) -> Vec<DeviceInfo> {
        let keyed: BTreeMap<String, String> =
            self.devices.iter().filter(|d| !d.machine.is_empty()).map(|d| (d.name.clone(), d.machine.clone())).collect();
        let mut seen: Vec<String> = Vec::new();
        let mut out = Vec::new();
        // `devices` is newest first, so the first of each computer is its latest.
        for device in &self.devices {
            let key = if device.machine.is_empty() {
                keyed.get(&device.name).cloned().unwrap_or_else(|| device.computer_key())
            } else {
                device.machine.clone()
            };
            if !seen.contains(&key) {
                seen.push(key);
                out.push(DeviceInfo { machine: device.machine.clone(), ..device.clone() });
            }
        }
        out
    }
}

#[derive(Debug, Clone, Default)]
pub struct SyncOutcome {
    /// A commit was pushed (something had changed).
    pub pushed: bool,
    pub sessions_uploaded: usize,
    pub overview: Overview,
    pub at: String,
    /// What this computer keeps in the repository after the pass.
    pub computer: ComputerStats,
    /// Another computer's newer configuration, for the app to put in place.
    pub settings: Option<settings::Incoming>,
    /// Sessions past the retention period, waiting for the user's OK before they are deleted.
    pub prune_pending: Vec<PruneItem>,
    /// Parts of this computer's settings not uploaded because they hold a credential.
    pub settings_held: Vec<String>,
}

/// A session past the retention period, as the popover lists it for the user's OK.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PruneItem {
    pub plugin: Option<String>,
    pub sync_id: String,
    pub workspace_name: String,
    pub device: String,
    pub device_name: String,
    pub agent: Agent,
    pub id: String,
    pub title: String,
    pub updated_at: String,
}

impl PruneItem {
    fn same(&self, other: &PruneItem) -> bool {
        self.plugin == other.plugin
            && self.sync_id == other.sync_id
            && self.device == other.device
            && self.agent == other.agent
            && self.id == other.id
    }
}

/// One sync pass: fetch, write this device's files, commit, push; again if another device pushed
/// in between. Blocking; run it off the UI thread.
pub fn sync(request: &SyncRequest) -> Result<SyncOutcome, SyncError> {
    let mut config = load_config();
    let remote = config.remote.clone().ok_or(SyncError::NotConnected)?;
    let _lock = Lock::take()?;
    let repo = Repo::new(clone_dir(), remote.clone(), config.branch.clone());
    if !repo.is_cloned() {
        repo.ensure_clone()?;
    }
    // Checked every time: a repository made public later must stop the sync, not feed it.
    if repo::visibility(&remote) == Visibility::Public {
        return Err(SyncError::Public);
    }
    let with_settings;
    let mut request = request;
    if config.settings && request.settings.is_none() {
        let settings = crate::backup::collect(crate::backup::Scope::Sync, None, &request.app_version).ok();
        with_settings = SyncRequest { settings, ..request.clone() };
        request = &with_settings;
    }
    let outcome = run(&repo, &mut config, request, &Locate::agents());
    // Ids given out in this pass are kept even when the push failed: the next pass reuses them.
    let assigned: Vec<(String, String)> =
        request.workspaces.iter().filter_map(|w| config.workspaces.get(&w.local_key).map(|id| (w.local_key.clone(), id.clone()))).collect();
    let outcome = outcome.map(|o| SyncOutcome { computer: stats_in(&repo.dir, &this_computer_devices(&o.overview, &config)), ..o });
    let at = outcome.as_ref().ok().map(|o| o.at.clone());
    let (fingerprint, changed_at) = (config.settings_fingerprint.clone(), config.settings_changed_at.clone());
    let _ = update_config(|c| {
        if c.settings {
            c.settings_fingerprint = fingerprint;
            c.settings_changed_at = changed_at;
        }
        for (key, id) in assigned {
            c.workspaces.entry(key).or_insert(id);
        }
        if at.is_some() {
            c.last_sync_at = at;
        }
    });
    outcome
}

/// Finds a session's transcript by agent and id.
pub type FindTranscript = Box<dyn Fn(Agent, &str) -> Result<PathBuf>>;

/// Whether an agent knows the project folder a restored transcript goes into.
pub type KnowsFolder = Box<dyn Fn(Agent, &Path, &Path) -> bool>;

/// Where transcripts are found: the agents' own folders, or a test's.
pub struct Locate {
    pub transcript: FindTranscript,
    pub root: Box<dyn Fn(Agent) -> PathBuf>,
    /// Whether the agent already knows the project folder a restored transcript goes into
    /// (`relative` under its root, for work dir `cwd`).
    pub knows_folder: KnowsFolder,
    /// This computer's key (`machine_key`).
    pub machine: String,
}

impl Locate {
    pub fn agents() -> Self {
        Self {
            transcript: Box::new(transcript_of),
            root: Box::new(agent_root),
            knows_folder: Box::new(agent_knows_folder),
            machine: machine_key(),
        }
    }
}

/// This computer, as the same key from every Agentty on it: a random id made on first use and
/// kept in `~/.agentty/computer-id`. Deliberately outside the data folder (`AGENTTY_DATA_DIR`), so
/// a dev build, a test copy or a second data folder on this computer counts as the same computer;
/// nothing about the hardware is read. Deleting `~/.agentty` makes a new one (Settings → Sync says
/// so). Empty when it can be neither read nor written.
pub fn machine_key() -> String {
    static KEY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    KEY.get_or_init(|| computer_id_in(&crate::fsutil::home().join(".agentty"))).clone()
}

/// The id in `dir/computer-id`, made when missing.
fn computer_id_in(dir: &Path) -> String {
    let path = dir.join("computer-id");
    if let Ok(text) = fs::read_to_string(&path) {
        let id = text.trim();
        if safe_segment(id) {
            return id.to_string();
        }
    }
    let id = new_id();
    match crate::fsutil::write_private(&path, format!("{id}\n").as_bytes()) {
        Ok(()) => id,
        Err(_) => String::new(),
    }
}

fn agent_knows_folder(agent: Agent, relative: &Path, cwd: &Path) -> bool {
    let first = relative.components().next().map(|c| c.as_os_str().to_string_lossy().to_string()).unwrap_or_default();
    match agent {
        // `<project slug>/chats/…`: the slug must be the one Gemini registered for this folder.
        Agent::Gemini => crate::gemini::project_path(&first).is_some_and(|p| Path::new(&p) == cwd),
        // `sessions/<md5 of the work dir>/…`: listed in kimi.json.
        Agent::Kimi => {
            let folder = relative.components().nth(1).map(|c| c.as_os_str().to_string_lossy().to_string()).unwrap_or_default();
            folder == crate::kimi::work_dir_folder(cwd) && crate::kimi::knows_work_dir(cwd)
        }
        _ => true,
    }
}

fn run(repo: &Repo, config: &mut SyncConfig, request: &SyncRequest, locate: &Locate) -> Result<SyncOutcome, SyncError> {
    let device_name = config.device_name.clone();
    let author = Author { device_name: &device_name };
    let mut last_error = None;
    for _ in 0..5 {
        repo.fetch()?;
        repo.reset_to_remote()?;
        if !repo.dir.join(MARKER_FILE).is_file() {
            if !repo.is_empty_or_readme_only() {
                return Err(SyncError::NotSyncRepository);
            }
            initialize(&repo.dir, &config.device_id)?;
        }
        write_folder_readmes(&repo.dir)?;
        if let Some(days) = request.retention.filter(|d| *d > 0) {
            let retention = Retention { days, changed_at: now_stamp(), changed_by: config.device_id.clone() };
            if read_retention(&repo.dir) != days {
                session::write_json(&repo.dir.join(RETENTION_FILE), &retention)?;
            }
        }
        let uploaded = write_device_files(&repo.dir, config, request, locate)?;
        // Deleting waits for the user's OK: only what they agreed to, and only while still old.
        let old = old_sessions(&repo.dir, retention_cutoff(&repo.dir));
        let (approved, prune_pending): (Vec<PruneItem>, Vec<PruneItem>) =
            old.into_iter().partition(|item| request.prune.iter().any(|ok| ok.same(item)));
        delete_sessions(&repo.dir, &approved)?;
        let (incoming, settings_held) = match &request.settings {
            Some(local) => {
                let step = settings::step(&repo.dir, config, local)?;
                (step.incoming, step.held)
            }
            None => (None, Vec::new()),
        };
        let message = format!(
            "sync: {} ({} workspace{})",
            config.device_name,
            request.workspaces.len(),
            if request.workspaces.len() == 1 { "" } else { "s" }
        );
        if !repo.commit_all(&message, &author)? {
            return Ok(SyncOutcome {
                pushed: false,
                sessions_uploaded: 0,
                overview: read_overview(&repo.dir),
                at: now_stamp(),
                settings: incoming,
                settings_held: settings_held.clone(),
                prune_pending: prune_pending.clone(),
                ..Default::default()
            });
        }
        match repo.push() {
            Ok(Pushed::Done) => {
                return Ok(SyncOutcome {
                    pushed: true,
                    sessions_uploaded: uploaded,
                    overview: read_overview(&repo.dir),
                    at: now_stamp(),
                    settings: incoming,
                    settings_held,
                    prune_pending,
                    ..Default::default()
                });
            }
            Ok(Pushed::Rejected) => last_error = Some("another device kept pushing; try again".to_string()),
            Err(err) => return Err(err.into()),
        }
    }
    Err(SyncError::Failed(last_error.unwrap_or_else(|| "push failed".into())))
}

/// What the clone holds now, without contacting the remote.
pub fn overview() -> Overview {
    read_overview(&clone_dir())
}

/// What this computer keeps in the repository (every Agentty on it), for Settings → Sync.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComputerStats {
    pub workspaces: usize,
    pub sessions: usize,
    /// Transcript chunks and side files.
    pub files: usize,
    pub bytes: u64,
}

/// Counted in the clone, no network.
pub fn computer_stats() -> ComputerStats {
    let root = clone_dir();
    stats_in(&root, &this_computer_devices(&read_overview(&root), &load_config()))
}

/// This app's device and the others on this computer.
fn this_computer_devices(overview: &Overview, config: &SyncConfig) -> Vec<String> {
    let machine = machine_key();
    let mut devices: Vec<String> =
        overview.devices.iter().filter(|d| !machine.is_empty() && d.machine == machine).map(|d| d.id.clone()).collect();
    if !devices.contains(&config.device_id) {
        devices.push(config.device_id.clone());
    }
    devices
}

fn stats_in(root: &Path, devices: &[String]) -> ComputerStats {
    let mut stats = ComputerStats::default();
    let mut workspaces: Vec<PathBuf> = fs::read_dir(root.join("workspace")).into_iter().flatten().flatten().map(|e| e.path()).collect();
    for plugin in fs::read_dir(root.join("plugin")).into_iter().flatten().flatten() {
        workspaces.extend(fs::read_dir(plugin.path()).into_iter().flatten().flatten().map(|e| e.path()));
    }
    for workspace in workspaces.iter().filter(|p| p.is_dir()) {
        let mut here = false;
        for device in devices.iter().filter(|d| safe_segment(d)) {
            for agent in fs::read_dir(workspace.join("devices").join(device)).into_iter().flatten().flatten() {
                for session in fs::read_dir(agent.path()).into_iter().flatten().flatten() {
                    if !session.path().join(MANIFEST_FILE).is_file() {
                        continue;
                    }
                    here = true;
                    stats.sessions += 1;
                    let mut files = Vec::new();
                    all_files(&session.path(), 4, &mut files);
                    for file in files.iter().filter(|f| f.file_name().is_some_and(|n| n != MANIFEST_FILE)) {
                        stats.files += 1;
                        stats.bytes += fs::metadata(file).map(|m| m.len()).unwrap_or(0);
                    }
                }
            }
        }
        if here {
            stats.workspaces += 1;
        }
    }
    stats
}

fn all_files(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth > 0 {
                all_files(&path, depth - 1, out);
            }
        } else {
            out.push(path);
        }
    }
}

/// Result of connecting to a repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Connected {
    /// An empty repository, set up now.
    Created,
    /// A sync repository other devices already use.
    Joined { devices: usize },
}

/// Connects this computer to `remote`: must be private, and empty or already a sync repository.
pub fn connect(remote: Remote) -> Result<Connected, SyncError> {
    match repo::visibility(&remote) {
        Visibility::Public => return Err(SyncError::Public),
        // A GitHub repository gh cannot even see: wrong name, or no access.
        Visibility::Unknown if matches!(remote, Remote::Github { .. }) => {
            return Err(SyncError::Failed(format!("{} could not be found with the signed-in GitHub account", remote.label())))
        }
        _ => {}
    }
    let _lock = Lock::take()?;
    let dir = clone_dir();
    let _ = fs::remove_dir_all(&dir);
    let mut repo = Repo::new(dir, remote.clone(), "main");
    repo.ensure_clone()?;
    repo.branch = repo.detect_branch();
    // Saved now, so the device id written into the repository is the one this computer keeps.
    let config = update_config(|c| c.clone())?;
    let outcome = if repo.dir.join(MARKER_FILE).is_file() {
        Connected::Joined { devices: read_overview(&repo.dir).computers().len() }
    } else if repo.is_empty_or_readme_only() {
        repo.reset_to_remote()?;
        initialize(&repo.dir, &config.device_id)?;
        repo.commit_all("sync: set up", &Author { device_name: &config.device_name })?;
        if repo.push()? == Pushed::Rejected {
            return Err(SyncError::Failed("the repository changed while it was being set up; try again".into()));
        }
        Connected::Created
    } else {
        let _ = fs::remove_dir_all(&repo.dir);
        return Err(SyncError::NotSyncRepository);
    };
    let branch = repo.branch.clone();
    update_config(|c| {
        c.remote = Some(remote);
        c.branch = branch;
        c.paused = false;
    })?;
    Ok(outcome)
}

/// Switches a GitHub repository between `gh` (HTTPS) and `git` (SSH). The clone follows.
pub fn set_github_ssh(ssh: bool) -> Result<()> {
    let _lock = Lock::take().map_err(|e| anyhow::anyhow!("{e}"))?;
    let config = load_config();
    let Some(Remote::Github { repo, .. }) = config.remote.clone() else { anyhow::bail!("not a GitHub repository") };
    let remote = Remote::Github { repo, ssh };
    let clone = Repo::new(clone_dir(), remote.clone(), config.branch.clone());
    if clone.is_cloned() {
        clone.set_origin(&remote.url())?;
    }
    update_config(|c| c.remote = Some(remote))?;
    Ok(())
}

/// Forgets the repository on this computer. Nothing is deleted from the repository.
pub fn disconnect() -> Result<()> {
    let _lock = Lock::take().map_err(|e| anyhow::anyhow!("{e}"))?;
    update_config(|c| {
        c.remote = None;
        c.last_sync_at = None;
    })?;
    let _ = fs::remove_dir_all(clone_dir());
    Ok(())
}

fn initialize(dir: &Path, device_id: &str) -> Result<()> {
    let marker = Marker { kind: Marker::KIND.into(), version: FORMAT_VERSION, created_at: now_stamp(), created_by: device_id.into() };
    session::write_json(&dir.join(MARKER_FILE), &marker)?;
    // Transcripts are stored byte for byte: no line-ending conversion on any platform.
    fs::write(dir.join(".gitattributes"), "* -text\n")?;
    if !dir.join("README.md").exists() {
        fs::write(dir.join("README.md"), README)?;
    }
    write_folder_readmes(dir)?;
    Ok(())
}

/// What each top folder holds, as a README in it. Also makes GitHub show the folder on its own:
/// a folder with a single subfolder is otherwise shown folded into one path (`workspace/4c60…`).
const FOLDER_READMES: &[(&str, &str)] = &[
    (
        "devices",
        "# Devices\n\n\
One file per computer that syncs here (`<device id>.json`): its name, operating system, Agentty version and when \
it last pushed. Each computer writes only its own file.\n",
    ),
    (
        "workspace",
        "# Workspaces\n\n\
One folder per workspace (`<workspace id>/`):\n\n\
- `sync_metadata.json` — the workspace as each computer last saw it (name, folder, project, open and closed \
sessions), one section per computer\n\
- `devices/<device id>/<agent>/<session id>/` — a session's transcript in append-only chunks (`0001.jsonl`, …) \
with `session.json` describing it\n\n\
A workspace on another computer with the same project joins the same folder.\n",
    ),
    (
        "plugin",
        "# Plugin workspaces\n\n\
Workspaces of Agentty plugins, laid out like `workspace/`: `<plugin id>/<workspace id>/`.\n",
    ),
    (
        "settings",
        "# Settings\n\n\
One file per computer that syncs its settings (`<device id>.json`): its Agentty settings, custom commands, \
connectors and imported themes, and when they last changed. The newest change is taken by the other computers. \
Secrets (API keys, tokens, passwords) are never written here.\n",
    ),
];

/// Writes the folder READMEs that are missing (a repository set up before they existed gets them
/// on its next sync; ones already there are left as they are).
fn write_folder_readmes(root: &Path) -> Result<()> {
    for (folder, text) in FOLDER_READMES {
        let path = root.join(folder).join("README.md");
        if !path.exists() {
            fs::create_dir_all(root.join(folder))?;
            fs::write(path, text)?;
        }
    }
    Ok(())
}

const README: &str = "# Agentty session sync\n\n\
Written by [Agentty](https://www.agentty.run): workspaces and their agent sessions, so they can be picked up on \
another computer. Keep this repository **private** — it holds conversations with coding agents. Agentty stops \
syncing if it becomes public.\n\n\
- `devices/` — the computers that sync here\n\
- `workspace/<id>/sync_metadata.json` — each workspace, one section per computer\n\
- `workspace/<id>/devices/<device>/<agent>/<session>/` — a session's transcript in chunks\n\
- `plugin/<plugin>/<id>/` — the same for plugin workspaces\n\
- `settings/<device>.json` — each computer's settings, when it syncs them\n\
- `removed/<id>.json` — workspaces whose data was deleted, so no computer writes them back\n";

fn workspace_dir(root: &Path, plugin: Option<&str>, sync_id: &str) -> Option<PathBuf> {
    safe_segment(sync_id).then_some(())?;
    Some(match plugin {
        Some(plugin) => {
            safe_segment(plugin).then_some(())?;
            root.join("plugin").join(plugin).join(sync_id)
        }
        None => root.join("workspace").join(sync_id),
    })
}

fn removed_marker(root: &Path, sync_id: &str) -> Option<PathBuf> {
    safe_segment(sync_id).then(|| root.join(REMOVED_DIR).join(format!("{sync_id}.json")))
}

fn safe_segment(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name != "."
        && name != ".."
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Writes this device's files into the clone. Returns how many sessions got new content.
/// The repository's retention period in days.
fn read_retention(root: &Path) -> u32 {
    fs::read_to_string(root.join(RETENTION_FILE))
        .ok()
        .and_then(|t| serde_json::from_str::<Retention>(&t).ok())
        .map(|r| r.days)
        .filter(|d| *d > 0)
        .unwrap_or(DEFAULT_RETENTION_DAYS)
}

/// Sessions last changed before this (epoch ms) are not kept.
fn retention_cutoff(root: &Path) -> u64 {
    now_ms().saturating_sub(u64::from(read_retention(root)) * 24 * 60 * 60 * 1000)
}

/// Sessions that last changed before `cutoff`, from every computer: listed in a workspace's
/// metadata with an old time, or a folder listed nowhere that was uploaded before it.
fn old_sessions(root: &Path, cutoff: u64) -> Vec<PruneItem> {
    let old = |stamp: &str| stamp_to_ms(stamp).is_some_and(|ms| ms < cutoff);
    let mut items = Vec::new();
    let mut folders: Vec<(Option<String>, PathBuf)> =
        fs::read_dir(root.join("workspace")).into_iter().flatten().flatten().map(|e| (None, e.path())).collect();
    for plugin in fs::read_dir(root.join("plugin")).into_iter().flatten().flatten() {
        let name = plugin.file_name().to_string_lossy().to_string();
        folders.extend(fs::read_dir(plugin.path()).into_iter().flatten().flatten().map(|e| (Some(name.clone()), e.path())));
    }
    let names: BTreeMap<String, String> = read_overview(root).devices.into_iter().map(|d| (d.id, d.name)).collect();
    for (plugin, workspace) in folders {
        let Some(metadata) =
            fs::read_to_string(workspace.join(METADATA_FILE)).ok().and_then(|t| serde_json::from_str::<WorkspaceMetadata>(&t).ok())
        else {
            continue;
        };
        let workspace_name = metadata.latest().and_then(|(_, s)| s.name.clone()).unwrap_or_default();
        let item = |device: &str, agent: Agent, id: &str, title: &str, updated_at: &str| PruneItem {
            plugin: plugin.clone(),
            sync_id: metadata.sync_id.clone(),
            workspace_name: workspace_name.clone(),
            device: device.to_string(),
            device_name: names.get(device).cloned().unwrap_or_else(|| device.to_string()),
            agent,
            id: id.to_string(),
            title: title.to_string(),
            updated_at: updated_at.to_string(),
        };
        for (device, section) in &metadata.devices {
            for entry in section.sessions.iter().filter(|e| old(&e.updated_at)) {
                items.push(item(device, entry.agent, &entry.id, &entry.title, &entry.updated_at));
            }
        }
        for device in fs::read_dir(workspace.join("devices")).into_iter().flatten().flatten() {
            let device_id = device.file_name().to_string_lossy().to_string();
            for agent in fs::read_dir(device.path()).into_iter().flatten().flatten() {
                for folder in fs::read_dir(agent.path()).into_iter().flatten().flatten() {
                    let id = folder.file_name().to_string_lossy().to_string();
                    let listed = metadata.devices.get(&device_id).is_some_and(|s| s.sessions.iter().any(|e| e.id == id));
                    let Ok(manifest) = session::read_manifest(&folder.path()) else { continue };
                    if !listed && old(&manifest.updated_at) {
                        items.push(item(&device_id, manifest.agent, &id, &manifest.title, &manifest.updated_at));
                    }
                }
            }
        }
    }
    items.sort_by(|a, b| a.updated_at.cmp(&b.updated_at));
    items
}

/// Deletes `items` from the repository: their folders and their entries in the metadata. A
/// computer that still has one does not upload it again (`write_device_files` skips it) unless it
/// changes.
fn delete_sessions(root: &Path, items: &[PruneItem]) -> Result<bool> {
    let mut changed = false;
    for item in items {
        let Some(workspace) = workspace_dir(root, item.plugin.as_deref(), &item.sync_id) else { continue };
        if !safe_segment(&item.device) || !safe_segment(&item.id) {
            continue;
        }
        let metadata_path = workspace.join(METADATA_FILE);
        if let Some(mut metadata) = fs::read_to_string(&metadata_path).ok().and_then(|t| serde_json::from_str::<WorkspaceMetadata>(&t).ok())
        {
            if let Some(section) = metadata.devices.get_mut(&item.device) {
                let before = section.sessions.len();
                section.sessions.retain(|e| !(e.id == item.id && e.agent == item.agent));
                if section.sessions.len() != before {
                    session::write_json(&metadata_path, &metadata)?;
                    changed = true;
                }
            }
        }
        let folder = workspace.join("devices").join(&item.device).join(item.agent.id()).join(&item.id);
        if folder.is_dir() {
            fs::remove_dir_all(folder)?;
            changed = true;
        }
    }
    Ok(changed)
}

fn write_device_files(root: &Path, config: &mut SyncConfig, request: &SyncRequest, locate: &Locate) -> Result<usize> {
    let cutoff = retention_cutoff(root);
    // Asking git about a folder takes a few commands: once per folder per pass.
    let mut projects: BTreeMap<PathBuf, ProjectRef> = BTreeMap::new();
    let mut project_of = |cwd: &Path| projects.entry(cwd.to_path_buf()).or_insert_with(|| project_of(cwd)).clone();
    let now = now_stamp();
    let mut uploaded = 0;
    let mut changed_any = false;
    let mut existing: Option<Overview> = None;
    for input in &request.workspaces {
        let project = project_of(&input.cwd);
        let known = input.sync_id.clone().or_else(|| config.workspaces.get(&input.local_key).cloned());
        let sync_id = match known {
            Some(id) => id,
            None => {
                // The same project synced from another computer is the same workspace.
                let overview = existing.get_or_insert_with(|| read_overview(root));
                let id = matching_workspace(overview, config, input, &project).unwrap_or_else(new_id);
                config.workspaces.insert(input.local_key.clone(), id.clone());
                id
            }
        };
        // Deleted from the repository on another computer: never written back from here.
        if removed_marker(root, &sync_id).is_some_and(|path| path.is_file()) {
            continue;
        }
        let Some(dir) = workspace_dir(root, input.plugin.as_deref(), &sync_id) else { continue };
        let metadata_path = dir.join(METADATA_FILE);
        let mut metadata: WorkspaceMetadata =
            fs::read_to_string(&metadata_path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| WorkspaceMetadata {
                version: FORMAT_VERSION,
                sync_id: sync_id.clone(),
                plugin: input.plugin.clone(),
                ..Default::default()
            });
        let previous = metadata.devices.get(&config.device_id).cloned().unwrap_or_default();

        // Sessions seen before and not open now stay listed (closed tabs), marked as closed.
        let mut sessions: Vec<SessionEntry> = previous
            .sessions
            .iter()
            .filter(|old| !input.sessions.iter().any(|s| s.id == old.id && s.agent == old.agent))
            .cloned()
            .map(|mut old| {
                if old.open {
                    old.open = false;
                    old.closed_at = Some(now.clone());
                }
                old
            })
            .collect();
        let mut seen: Vec<(Agent, String)> = Vec::new();
        for session in &input.sessions {
            // An agent run as a command: its session is the newest one in its folder.
            let found;
            let session = if session.id.is_empty() {
                let cwd = session.cwd.as_ref().unwrap_or(&input.cwd);
                let Some(id) = session.started_ms.and_then(|since| crate::find_recent(session.agent, cwd, since)) else { continue };
                found = SessionInput { id, ..session.clone() };
                &found
            } else {
                session
            };
            if seen.contains(&(session.agent, session.id.clone())) {
                continue;
            }
            seen.push((session.agent, session.id.clone()));
            let session_dir = dir.join("devices").join(&config.device_id).join(session.agent.id()).join(&session.id);
            // No title (its agent quit and the pane shows the shell's): the one it had before.
            // Masked like transcripts: a title can be the first prompt (Amp).
            let title = mask::mask(&if session.title.trim().is_empty() {
                previous
                    .sessions
                    .iter()
                    .find(|s| s.id == session.id && s.agent == session.agent)
                    .map(|s| s.title.clone())
                    .or_else(|| safe_segment(&session.id).then(|| session::read_manifest(&session_dir).ok()).flatten().map(|m| m.title))
                    .filter(|t| !t.trim().is_empty())
                    .unwrap_or_else(|| session.agent.display_name().to_string())
            } else {
                session.title.clone()
            });
            if !syncs_files(session.agent) {
                // Listed for the other computers; nothing to upload.
                let old = previous.sessions.iter().find(|s| s.id == session.id && s.agent == session.agent);
                sessions.push(SessionEntry {
                    agent: session.agent,
                    id: session.id.clone(),
                    title: title.clone(),
                    updated_at: old.map(|o| o.updated_at.clone()).unwrap_or_else(|| now.clone()),
                    open: session.open,
                    closed_at: None,
                    parent: None,
                });
                continue;
            }
            let Ok(transcript) = (locate.transcript)(session.agent, &session.id) else {
                // Deleted locally: keep what the repository already has.
                if let Some(old) = previous.sessions.iter().find(|s| s.id == session.id && s.agent == session.agent) {
                    sessions.push(SessionEntry { open: session.open, ..old.clone() });
                }
                continue;
            };
            // Unchanged for longer than the repository keeps sessions: not uploaded again.
            if crate::fsutil::mtime_ms(&transcript) < cutoff {
                continue;
            }
            if !safe_segment(&session.id) {
                continue;
            }
            let side = (session.agent == Agent::Claude).then(|| transcript.with_extension(""));
            let source = transcript
                .strip_prefix((locate.root)(session.agent))
                .map(|p| p.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect::<Vec<_>>().join("/"))
                .unwrap_or_default();
            let session_cwd = session.cwd.as_ref().unwrap_or(&input.cwd).to_string_lossy().to_string();
            let facts = session::SessionFacts {
                agent: session.agent,
                id: &session.id,
                title: &title,
                device: &config.device_id,
                cwd: &session_cwd,
                project: project_of(Path::new(&session_cwd)),
                source,
                parent: session.parent.clone().or_else(|| config.origins.get(&restore::origin_key(session.agent, &session.id)).cloned()),
            };
            let (manifest, changed) = session::upload(&session_dir, &transcript, side.as_deref(), &facts)?;
            if changed {
                uploaded += 1;
            }
            // Closed now (its workspace removed) keeps when; closed earlier keeps that time.
            let old = previous.sessions.iter().find(|s| s.id == session.id && s.agent == session.agent);
            let closed_at = (!session.open).then(|| old.and_then(|o| o.closed_at.clone()).unwrap_or_else(|| now.clone()));
            sessions.push(SessionEntry {
                agent: session.agent,
                id: session.id.clone(),
                title,
                updated_at: stamp_ms(crate::fsutil::mtime_ms(&transcript)),
                open: session.open,
                closed_at,
                parent: manifest.parent.clone(),
            });
        }
        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

        let state_at = if previous.state == input.state && !previous.state_at.is_empty() { previous.state_at.clone() } else { now.clone() };
        let section = DeviceSection {
            device_name: config.device_name.clone(),
            synced_at: previous.synced_at.clone(),
            name: input.name.clone(),
            group: input.group.clone(),
            color: input.color,
            cwd: input.cwd.to_string_lossy().to_string(),
            project,
            state: input.state,
            state_at,
            sessions,
        };
        // The time moves only when something in the section did, so an idle workspace commits nothing.
        let changed =
            DeviceSection { synced_at: String::new(), ..section.clone() } != DeviceSection { synced_at: String::new(), ..previous };
        if changed || !metadata.devices.contains_key(&config.device_id) {
            metadata.devices.insert(config.device_id.clone(), DeviceSection { synced_at: now.clone(), ..section });
            metadata.plugin = input.plugin.clone();
            session::write_json(&metadata_path, &metadata)?;
            changed_any = true;
        }
    }
    for (plugin, sync_id) in &request.purge {
        if let Some(dir) = workspace_dir(root, plugin.as_deref(), sync_id).filter(|d| d.exists()) {
            fs::remove_dir_all(dir)?;
            changed_any = true;
        }
        // Left behind so a computer that still has the workspace does not upload it again.
        if let Some(path) = removed_marker(root, sync_id).filter(|p| !p.exists()) {
            let removed =
                Removed { sync_id: sync_id.clone(), plugin: plugin.clone(), removed_at: now.clone(), removed_by: config.device_id.clone() };
            session::write_json(&path, &removed)?;
            changed_any = true;
        }
    }

    // The device file moves with what the device pushed, so a pass with nothing new commits nothing.
    let device_path = root.join("devices").join(format!("{}.json", config.device_id));
    let old: Option<DeviceInfo> = fs::read_to_string(&device_path).ok().and_then(|t| serde_json::from_str(&t).ok());
    let device = DeviceInfo {
        id: config.device_id.clone(),
        name: config.device_name.clone(),
        machine: locate.machine.clone(),
        os: std::env::consts::OS.into(),
        app_version: request.app_version.clone(),
        last_sync_at: now,
    };
    let same = old.as_ref().is_some_and(|o| {
        DeviceInfo { last_sync_at: String::new(), ..o.clone() } == DeviceInfo { last_sync_at: String::new(), ..device.clone() }
    });
    if changed_any || !same {
        session::write_json(&device_path, &device)?;
    }
    Ok(uploaded)
}

/// A workspace another computer synced that `input` is the same as: the same project (git remote
/// and folder in it), or — for a folder without a remote — the same name. Never one this computer
/// already syncs under another of its workspaces, so two local workspaces never share a section.
fn matching_workspace(overview: &Overview, config: &SyncConfig, input: &WorkspaceInput, project: &ProjectRef) -> Option<String> {
    let taken: Vec<&String> = config.workspaces.values().collect();
    overview
        .workspaces
        .iter()
        .filter(|meta| meta.plugin == input.plugin && !taken.contains(&&meta.sync_id))
        .filter(|meta| {
            meta.devices.values().any(|section| match &project.remote_url {
                Some(remote) => section.project.remote_url.as_ref() == Some(remote) && section.project.path_in_repo == project.path_in_repo,
                None => section.project.remote_url.is_none() && input.name.is_some() && section.name == input.name,
            })
        })
        .max_by(|a, b| {
            let latest = |m: &WorkspaceMetadata| m.latest().map(|(_, s)| s.synced_at.clone()).unwrap_or_default();
            latest(a).cmp(&latest(b))
        })
        .map(|meta| meta.sync_id.clone())
}

fn read_overview(root: &Path) -> Overview {
    let mut overview = Overview::default();
    if let Ok(entries) = fs::read_dir(root.join("devices")) {
        for entry in entries.flatten() {
            if let Some(device) = fs::read_to_string(entry.path()).ok().and_then(|t| serde_json::from_str::<DeviceInfo>(&t).ok()) {
                overview.devices.push(device);
            }
        }
    }
    overview.devices.sort_by(|a, b| b.last_sync_at.cmp(&a.last_sync_at));
    let mut folders = Vec::new();
    if let Ok(entries) = fs::read_dir(root.join("workspace")) {
        folders.extend(entries.flatten().map(|e| e.path()));
    }
    if let Ok(plugins) = fs::read_dir(root.join("plugin")) {
        for plugin in plugins.flatten().filter(|e| e.path().is_dir()) {
            if let Ok(entries) = fs::read_dir(plugin.path()) {
                folders.extend(entries.flatten().map(|e| e.path()));
            }
        }
    }
    if let Ok(entries) = fs::read_dir(root.join(REMOVED_DIR)) {
        overview.removed.extend(entries.flatten().filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.strip_suffix(".json").filter(|id| safe_segment(id)).map(str::to_string)
        }));
    }
    overview.retention_days = read_retention(root);
    for folder in folders {
        if let Some(meta) =
            fs::read_to_string(folder.join(METADATA_FILE)).ok().and_then(|t| serde_json::from_str::<WorkspaceMetadata>(&t).ok())
        {
            overview.workspaces.push(meta);
        }
    }
    overview
}

/// The project a folder belongs to, as another computer can recognise it.
fn project_of(cwd: &Path) -> ProjectRef {
    let Some(root) = crate::git::repo_root(cwd) else { return ProjectRef::default() };
    let path_in_repo = cwd
        .strip_prefix(&root)
        .map(|p| p.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect::<Vec<_>>().join("/"))
        .unwrap_or_default();
    let (branch, commit) = crate::git::head(&root).map_or((None, None), |(b, c)| (b, Some(c)));
    ProjectRef { remote_url: crate::git::remote_web_url(&root), path_in_repo, branch, commit }
}

fn agent_root(agent: Agent) -> PathBuf {
    match agent {
        Agent::Claude => crate::claude::session_root(),
        Agent::Codex => crate::codex::session_root(),
        Agent::Agy => crate::agy::session_root(),
        Agent::Amp => crate::amp::session_root(),
        Agent::Gemini => crate::gemini::session_root(),
        Agent::Kimi => crate::kimi::session_root(),
    }
}

fn transcript_of(agent: Agent, id: &str) -> Result<PathBuf> {
    let path = match agent {
        Agent::Claude => crate::claude::find(id),
        Agent::Codex => crate::codex::find(id),
        Agent::Agy => crate::agy::find(id),
        Agent::Amp => crate::amp::find(id),
        Agent::Gemini => crate::gemini::find(id),
        Agent::Kimi => crate::kimi::find(id),
    }?;
    // Only a file: some agents keep a session as a folder, which is not synced yet.
    anyhow::ensure!(path.is_file(), "{} is not a file", path.display());
    Ok(path)
}

/// Keeps a second sync (a second app on the same data folder) from running at the same time.
struct Lock(PathBuf);

impl Lock {
    fn take() -> Result<Self, SyncError> {
        static IN_PROCESS: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = IN_PROCESS.lock().map_err(|_| SyncError::Busy)?;
        fs::create_dir_all(sync_dir()).map_err(|e| SyncError::Failed(e.to_string()))?;
        let path = sync_dir().join("sync.lock");
        // A lock left by a crash expires: no sync takes ten minutes.
        let stale = crate::fsutil::mtime_ms(&path) + 10 * 60 * 1000 < now_ms();
        if stale {
            let _ = fs::remove_file(&path);
        }
        match fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(_) => Ok(Lock(path)),
            Err(_) => Err(SyncError::Busy),
        }
    }
}

impl Lock {
    /// Waits up to `timeout` for a running sync to finish.
    fn wait(timeout: std::time::Duration) -> Result<Self, SyncError> {
        let started = std::time::Instant::now();
        loop {
            match Self::take() {
                Err(SyncError::Busy) if started.elapsed() < timeout => std::thread::sleep(std::time::Duration::from_millis(500)),
                other => return other,
            }
        }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// A random id (device, workspace): 16 hex digits from the OS's randomness via std's hasher keys.
pub fn new_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    hasher.write_u32(std::process::id());
    let high = hasher.finish();
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u64(high);
    format!("{:016x}{:016x}", high, hasher.finish())
}

/// What the user calls this computer.
pub fn computer_name() -> String {
    #[cfg(target_os = "macos")]
    if let Ok(output) = crate::process::command("scutil").args(["--get", "ComputerName"]).output() {
        let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if output.status.success() && !name.is_empty() {
            return name;
        }
    }
    if let Ok(name) = std::env::var("COMPUTERNAME") {
        if !name.trim().is_empty() {
            return name.trim().to_string();
        }
    }
    if let Ok(name) = fs::read_to_string("/etc/hostname") {
        if !name.trim().is_empty() {
            return name.trim().to_string();
        }
    }
    crate::process::command("hostname")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "Computer".into())
}

/// For the app: whether two stamps are far enough apart to call a sync "old".
pub fn stamp_age_ms(stamp: &str) -> Option<u64> {
    stamp_to_ms(stamp).map(|at| now_ms().saturating_sub(at))
}

/// Which of these sessions have a transcript on this computer.
pub fn present_locally(sessions: &[(Agent, String)]) -> std::collections::HashSet<String> {
    sessions.iter().filter(|(agent, id)| transcript_of(*agent, id).is_ok()).map(|(_, id)| id.clone()).collect()
}

/// Reads a manifest from the clone (used by the app to show a session's details).
pub fn manifest(plugin: Option<&str>, sync_id: &str, device: &str, agent: Agent, session: &str) -> Result<SessionManifest> {
    let dir = workspace_dir(&clone_dir(), plugin, sync_id).context("bad workspace id")?;
    session::read_manifest(&dir.join("devices").join(device).join(agent.id()).join(session))
}
