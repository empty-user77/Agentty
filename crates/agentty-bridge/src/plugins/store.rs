//! Installed plugins (`~/.agentty/plugins/<id>`), their enabled state, the built-in catalog and
//! installing from the catalog, a folder or a Git repository.

use super::manifest::{valid_id, version_newer, Manifest, MANIFEST_FILE};
use crate::fsutil;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Where plugins are installed: `<data dir>/plugins`.
pub fn plugins_dir() -> PathBuf {
    #[cfg(test)]
    if let Some(root) = tests::ROOT.with(|r| r.borrow().clone()) {
        return root.join("plugins");
    }
    fsutil::data_dir().join("plugins")
}

/// Private working folder of a plugin (`AGENTTY_PLUGIN_DATA`).
pub fn plugin_data_dir(id: &str) -> PathBuf {
    fsutil::data_dir().join("plugin-data").join(id)
}

fn state_path() -> PathBuf {
    plugins_dir().join("state.json")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// Shipped with Agentty.
    Builtin,
    /// Copied from a local folder.
    Folder,
    /// Cloned from a Git repository.
    Git,
    /// Created with "New plugin" (edited in place).
    Local,
    /// Linked for development: loaded from its own folder, never copied.
    Dev,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginState {
    pub enabled: bool,
    pub source: Source,
    /// Folder of a development link.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct StateFile {
    #[serde(default)]
    pub plugins: BTreeMap<String, PluginState>,
}

impl StateFile {
    pub fn load() -> Self {
        std::fs::read(state_path()).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        std::fs::create_dir_all(plugins_dir())?;
        let tmp = state_path().with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(tmp, state_path())?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct InstalledPlugin {
    pub id: String,
    pub dir: PathBuf,
    /// `None` when the manifest could not be read (see `error`).
    pub manifest: Option<Manifest>,
    pub error: Option<String>,
    pub enabled: bool,
    pub source: Source,
}

impl InstalledPlugin {
    pub fn name(&self) -> &str {
        self.manifest.as_ref().map_or(&self.id, |m| &m.name)
    }

    /// Enabled and loadable.
    pub fn active(&self) -> bool {
        self.enabled && self.manifest.is_some()
    }
}

/// Every installed plugin, sorted by name. Folders without a manifest are skipped.
pub fn installed() -> Vec<InstalledPlugin> {
    let state = StateFile::load();
    let mut found: BTreeMap<String, InstalledPlugin> = BTreeMap::new();
    if let Ok(entries) = std::fs::read_dir(plugins_dir()) {
        for entry in entries.flatten() {
            let dir = entry.path();
            let Some(id) = dir.file_name().and_then(|n| n.to_str()).map(str::to_string) else { continue };
            if !dir.is_dir() || !valid_id(&id) || !dir.join(MANIFEST_FILE).exists() {
                continue;
            }
            let saved = state.plugins.get(&id);
            found.insert(id.clone(), read_plugin(&id, dir, saved.is_none_or(|s| s.enabled), saved.map_or(Source::Folder, |s| s.source)));
        }
    }
    for (id, saved) in &state.plugins {
        if let (Source::Dev, Some(path)) = (saved.source, &saved.path) {
            found.insert(id.clone(), read_plugin(id, path.clone(), saved.enabled, Source::Dev));
        }
    }
    let mut list: Vec<InstalledPlugin> = found.into_values().collect();
    list.sort_by_key(|p| p.name().to_lowercase());
    list
}

fn read_plugin(id: &str, dir: PathBuf, enabled: bool, source: Source) -> InstalledPlugin {
    let (manifest, error) = match Manifest::load(&dir) {
        Ok(manifest) if manifest.id != id => (None, Some(format!("manifest id \"{}\" does not match the folder \"{id}\"", manifest.id))),
        Ok(manifest) => match manifest.entry(&dir) {
            Ok(entry) if entry.exists() => (Some(manifest), None),
            Ok(entry) => (None, Some(format!("entry point {} is missing", entry.display()))),
            Err(err) => (None, Some(format!("{err:#}"))),
        },
        Err(err) => (None, Some(format!("{err:#}"))),
    };
    InstalledPlugin { id: id.to_string(), dir, manifest, error, enabled, source }
}

pub fn set_enabled(id: &str, enabled: bool) -> Result<()> {
    let mut state = StateFile::load();
    let entry = state.plugins.entry(id.to_string()).or_insert(PluginState { enabled, source: Source::Folder, path: None, origin: None });
    entry.enabled = enabled;
    state.save()
}

/// A plugin shipped inside Agentty: its files are embedded in the binary.
pub struct BuiltinPlugin {
    pub manifest_json: &'static str,
    pub files: &'static [(&'static str, &'static str)],
}

impl BuiltinPlugin {
    pub fn manifest(&self) -> Manifest {
        Manifest::parse(self.manifest_json.as_bytes()).expect("built-in plugin manifest is valid")
    }
}

macro_rules! embedded {
    ($path:literal) => {
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../", $path))
    };
}

/// The Node.js SDK every JavaScript plugin can import (`./agentty-plugin.mjs`).
pub const NODE_SDK: &str = embedded!("sdk/node/agentty-plugin.mjs");
pub const NODE_SDK_TYPES: &str = embedded!("sdk/node/agentty-plugin.d.ts");
/// Developer guide and the prompt for building a plugin with an AI agent.
pub const GUIDE: &str = embedded!("docs/plugins/README.md");
pub const USAGE: &str = embedded!("docs/plugins/usage.md");
pub const AI_PROMPT: &str = embedded!("docs/plugins/ai-prompt.md");
pub const PROTOCOL: &str = embedded!("docs/plugins/protocol.md");

pub const BUILTIN: &[BuiltinPlugin] = &[BuiltinPlugin {
    manifest_json: embedded!("plugins/cosmica/agentty-plugin.json"),
    files: &[
        ("agentty-plugin.json", embedded!("plugins/cosmica/agentty-plugin.json")),
        ("main.mjs", embedded!("plugins/cosmica/main.mjs")),
        ("cosmica.mjs", embedded!("plugins/cosmica/cosmica.mjs")),
        ("README.md", embedded!("plugins/cosmica/README.md")),
        ("agentty-plugin.mjs", NODE_SDK),
    ],
}];

/// Files of a new plugin made with "New plugin" (`{{id}}` and `{{name}}` are filled in).
const TEMPLATE: &[(&str, &str)] = &[
    ("agentty-plugin.json", embedded!("plugins/template/agentty-plugin.json")),
    ("main.mjs", embedded!("plugins/template/main.mjs")),
    ("README.md", embedded!("plugins/template/README.md")),
    ("agentty-plugin.mjs", NODE_SDK),
    ("agentty-plugin.d.ts", NODE_SDK_TYPES),
    ("PLUGIN_GUIDE.md", GUIDE),
    ("protocol.md", PROTOCOL),
    ("AGENTS.md", AI_PROMPT),
    ("CLAUDE.md", AI_PROMPT),
];

pub fn builtin(id: &str) -> Option<&'static BuiltinPlugin> {
    BUILTIN.iter().find(|b| b.manifest().id == id)
}

/// Installs (or updates) a built-in plugin, keeping its enabled state.
pub fn install_builtin(id: &str) -> Result<InstalledPlugin> {
    let plugin = builtin(id).with_context(|| format!("no built-in plugin {id}"))?;
    let manifest = plugin.manifest();
    let staging = staging_dir(&manifest.id)?;
    for (name, contents) in plugin.files {
        write_file(&staging.join(name), contents)?;
    }
    finish_install(&manifest, &staging, Source::Builtin, None)
}

/// Whether the built-in version is newer than the installed copy.
pub fn builtin_update_available(plugin: &InstalledPlugin) -> bool {
    let (Some(installed), Source::Builtin) = (&plugin.manifest, plugin.source) else { return false };
    builtin(&plugin.id).is_some_and(|b| version_newer(&b.manifest().version, &installed.version))
}

/// Copies a plugin folder into the plugins directory.
pub fn install_from_folder(folder: &Path) -> Result<InstalledPlugin> {
    let manifest = Manifest::load(folder)?;
    let staging = staging_dir(&manifest.id)?;
    copy_tree(folder, &staging, 0)?;
    finish_install(&manifest, &staging, Source::Folder, Some(folder.display().to_string()))
}

/// Loads a plugin from its own folder (edits take effect after a restart of the plugin).
pub fn link_dev_folder(folder: &Path) -> Result<InstalledPlugin> {
    let manifest = Manifest::load(folder)?;
    let target = plugins_dir().join(&manifest.id);
    if target.exists() {
        bail!("a plugin with id \"{}\" is already installed; uninstall it first", manifest.id);
    }
    let mut state = StateFile::load();
    state.plugins.insert(
        manifest.id.clone(),
        PluginState { enabled: true, source: Source::Dev, path: Some(folder.to_path_buf()), origin: Some(folder.display().to_string()) },
    );
    state.save()?;
    Ok(read_plugin(&manifest.id, folder.to_path_buf(), true, Source::Dev))
}

/// Clones an `https://` Git repository (shallow) and installs the plugin at its root.
pub fn install_from_git(url: &str) -> Result<InstalledPlugin> {
    let url = url.trim();
    if !url.starts_with("https://") || url.chars().any(|c| c.is_whitespace()) {
        bail!("use an https:// Git URL");
    }
    let tmp = plugins_dir().join(format!(".clone-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(plugins_dir())?;
    let output = std::process::Command::new("git")
        .args(["clone", "--depth", "1", "--", url])
        .arg(&tmp)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .output()
        .context("git is not available")?;
    if !output.status.success() {
        let _ = std::fs::remove_dir_all(&tmp);
        bail!("git clone failed: {}", String::from_utf8_lossy(&output.stderr).trim());
    }
    // Copied like a folder install, so symlinks in the repository can't point outside the plugin.
    let result = Manifest::load(&tmp).and_then(|manifest| {
        let staging = staging_dir(&manifest.id)?;
        copy_tree(&tmp, &staging, 0)?;
        finish_install(&manifest, &staging, Source::Git, Some(url.to_string()))
    });
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

/// Creates a new plugin from the template, ready to be developed in place.
pub fn create_plugin(name: &str) -> Result<InstalledPlugin> {
    let name = name.trim();
    let id: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let id: String = id.chars().take(40).collect::<String>().trim_end_matches('-').to_string();
    if !valid_id(&id) {
        bail!("use a name with at least two letters or digits (a-z, 0-9)");
    }
    let dir = plugins_dir().join(&id);
    if dir.exists() {
        bail!("a plugin with id \"{id}\" already exists");
    }
    let display = if name.is_empty() { id.clone() } else { name.to_string() };
    let display_json = serde_json::to_string(&display)?;
    for (file, contents) in TEMPLATE {
        let text = if file.ends_with(".json") {
            // JSON gets a properly escaped name.
            contents.replace("{{id}}", &id).replace("\"{{name}}\"", &display_json)
        } else {
            contents.replace("{{id}}", &id).replace("{{name}}", &display)
        };
        write_file(&dir.join(file), &text)?;
    }
    Manifest::load(&dir)?;
    let mut state = StateFile::load();
    state.plugins.insert(id.clone(), PluginState { enabled: true, source: Source::Local, path: None, origin: None });
    state.save()?;
    Ok(read_plugin(&id, dir, true, Source::Local))
}

/// Removes an installed plugin (a development link is only unlinked; its folder stays).
pub fn uninstall(id: &str) -> Result<()> {
    if !valid_id(id) {
        bail!("invalid plugin id");
    }
    let mut state = StateFile::load();
    let dev = state.plugins.get(id).is_some_and(|s| s.source == Source::Dev);
    if !dev {
        let dir = plugins_dir().join(id);
        if dir.exists() {
            std::fs::remove_dir_all(&dir).with_context(|| format!("could not remove {}", dir.display()))?;
        }
    }
    state.plugins.remove(id);
    state.save()
}

fn staging_dir(id: &str) -> Result<PathBuf> {
    if !valid_id(id) {
        bail!("invalid plugin id {id}");
    }
    let dir = plugins_dir().join(format!(".staging-{id}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Validates the staged copy and swaps it into place.
fn finish_install(manifest: &Manifest, staged: &Path, source: Source, origin: Option<String>) -> Result<InstalledPlugin> {
    let check = Manifest::load(staged).and_then(|m| {
        let entry = m.entry(staged)?;
        anyhow::ensure!(entry.exists(), "entry point {} is missing", m.main);
        Ok(m)
    });
    let staged_manifest = match check {
        Ok(m) => m,
        Err(err) => {
            let _ = std::fs::remove_dir_all(staged);
            return Err(err);
        }
    };
    anyhow::ensure!(staged_manifest.id == manifest.id, "plugin id changed while installing");
    let mut state = StateFile::load();
    if state.plugins.get(&manifest.id).is_some_and(|s| s.source == Source::Dev) {
        let _ = std::fs::remove_dir_all(staged);
        bail!("\"{}\" is linked for development; unlink it first", manifest.id);
    }
    let target = plugins_dir().join(&manifest.id);
    if target.exists() {
        std::fs::remove_dir_all(&target)?;
    }
    std::fs::rename(staged, &target)?;
    let enabled = state.plugins.get(&manifest.id).is_none_or(|s| s.enabled);
    state.plugins.insert(manifest.id.clone(), PluginState { enabled, source, path: None, origin });
    state.save()?;
    Ok(read_plugin(&manifest.id, target, enabled, source))
}

fn write_file(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, contents)?;
    Ok(())
}

/// Copies a folder, skipping VCS data and `node_modules` is kept (plugins may bundle dependencies).
fn copy_tree(from: &Path, to: &Path, depth: usize) -> Result<()> {
    if depth > 12 {
        bail!("plugin folder is nested too deeply");
    }
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)?.flatten() {
        let name = entry.file_name();
        if name == ".git" || name == ".DS_Store" {
            continue;
        }
        let file_type = entry.file_type()?;
        let target = to.join(&name);
        if file_type.is_dir() {
            copy_tree(&entry.path(), &target, depth + 1)?;
        } else if file_type.is_file() {
            std::fs::copy(entry.path(), &target)?;
        }
        // Symlinks are not followed: a plugin must not pull in files from elsewhere.
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    thread_local! {
        /// Data directory of the test running on this thread (instead of `~/.agentty`).
        pub static ROOT: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
    }

    fn with_data_dir(test: impl FnOnce(&Path)) {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("agentty-plugins-test-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        ROOT.with(|r| *r.borrow_mut() = Some(dir.clone()));
        test(&dir);
        ROOT.with(|r| *r.borrow_mut() = None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The installed app has no `AGENTTY_DATA_DIR`: everything lives under `~/.agentty`.
    #[test]
    fn plugins_live_under_the_data_dir() {
        if std::env::var_os("AGENTTY_DATA_DIR").is_none() {
            let home = crate::fsutil::home();
            assert_eq!(plugins_dir(), home.join(".agentty").join("plugins"));
            assert_eq!(plugin_data_dir("cosmica"), home.join(".agentty").join("plugin-data").join("cosmica"));
        }
        // A data directory chosen by the environment (tests, a second install) moves them together.
        assert!(plugins_dir().ends_with("plugins"));
    }

    #[test]
    fn builtin_plugins_are_valid() {
        for plugin in BUILTIN {
            let manifest = plugin.manifest();
            assert!(plugin.files.iter().any(|(name, _)| *name == manifest.main), "{} misses its entry point", manifest.id);
        }
    }

    #[test]
    fn installs_toggles_and_uninstalls_builtin() {
        with_data_dir(|_| {
            let plugin = install_builtin("cosmica").unwrap();
            assert!(plugin.active(), "{:?}", plugin.error);
            assert!(plugin.dir.join("agentty-plugin.mjs").exists());
            assert_eq!(installed().len(), 1);

            set_enabled("cosmica", false).unwrap();
            assert!(!installed()[0].enabled);
            // Reinstalling (an update) keeps the choice.
            assert!(!install_builtin("cosmica").unwrap().enabled);
            assert!(!builtin_update_available(&installed()[0]));

            uninstall("cosmica").unwrap();
            assert!(installed().is_empty());
        });
    }

    #[test]
    fn creates_plugin_from_template_and_links_dev_folders() {
        with_data_dir(|root| {
            let plugin = create_plugin("My Notes \"Bridge\"").unwrap();
            assert_eq!(plugin.id, "my-notes-bridge");
            let manifest = plugin.manifest.clone().unwrap();
            assert_eq!(manifest.name, "My Notes \"Bridge\"");
            assert!(plugin.dir.join("CLAUDE.md").exists());
            assert!(create_plugin("My Notes Bridge").is_err());
            assert!(create_plugin("!").is_err());

            // A copy elsewhere, with another id, linked for development.
            let dev = root.join("dev-plugin");
            copy_tree(&plugin.dir, &dev, 0).unwrap();
            let json = std::fs::read_to_string(dev.join(MANIFEST_FILE)).unwrap().replace("my-notes-bridge", "dev-plugin");
            std::fs::write(dev.join(MANIFEST_FILE), json).unwrap();
            let linked = link_dev_folder(&dev).unwrap();
            assert_eq!(linked.source, Source::Dev);
            assert_eq!(installed().len(), 2);
            uninstall("dev-plugin").unwrap();
            assert!(dev.exists(), "unlinking keeps the developer's folder");
            assert_eq!(installed().len(), 1);
        });
    }

    #[test]
    fn folder_install_rejects_mismatched_or_broken_plugins() {
        with_data_dir(|root| {
            let source = root.join("src");
            std::fs::create_dir_all(&source).unwrap();
            std::fs::write(source.join(MANIFEST_FILE), r#"{"id":"broken","name":"Broken","version":"1.0.0","main":"missing.mjs"}"#)
                .unwrap();
            assert!(install_from_folder(&source).is_err());
            assert!(!plugins_dir().join("broken").exists());
            std::fs::write(source.join("missing.mjs"), "").unwrap();
            assert!(install_from_folder(&source).unwrap().active());
            assert!(install_from_git("git@github.com:x/y.git").is_err());
        });
    }
}
