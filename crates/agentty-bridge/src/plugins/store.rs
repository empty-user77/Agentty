//! Installed plugins (`~/.agentty/plugins/<id>`), their enabled state, the built-in catalog and
//! installing from the catalog, a folder or a Git repository.

use super::manifest::{parse_logo, relative_path, valid_id, version_newer, Logo, Manifest, MANIFEST_FILE};
use crate::fsutil;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Read;
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
    #[cfg(test)]
    if let Some(root) = tests::ROOT.with(|r| r.borrow().clone()) {
        return root.join("plugin-data").join(id);
    }
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
    /// Downloaded from the marketplace.
    Market,
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
    /// What the user has installed, and which of it they turned off.
    ///
    /// A file that is not there yet is an empty one — nothing is installed. A file that is there
    /// but cannot be read is not: every plugin the user disabled would come back enabled, every
    /// development link would disappear, and the next write would make that permanent. It is
    /// moved aside instead, so what it said survives for anyone who wants to look.
    pub fn load() -> Self {
        let path = state_path();
        let Ok(bytes) = std::fs::read(&path) else { return Self::default() };
        match serde_json::from_slice::<StateFile>(&bytes) {
            Ok(state) => state,
            Err(_) => {
                let aside = path.with_file_name("state.damaged.json");
                if std::fs::rename(&path, &aside).is_ok() {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let _ = std::fs::set_permissions(&aside, std::fs::Permissions::from_mode(0o600));
                    }
                    eprintln!("agentty: {} could not be read and was moved to {}", path.display(), aside.display());
                }
                Self::default()
            }
        }
    }

    pub fn save(&self) -> Result<()> {
        std::fs::create_dir_all(plugins_dir())?;
        // A name of its own: an install runs on a background thread while the main thread can be
        // turning a plugin off, and two writers sharing one temporary file publish a half-written
        // state between them.
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let unique = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = state_path().with_extension(format!("{}.{unique}.json.tmp", std::process::id()));
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        // It says what the user installed and where from — nobody else's business, and it is
        // written before it is narrowed nowhere: the mode travels with the rename.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }
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

/// Folders an install left behind when it did not finish — Agentty closed, or the machine did,
/// between staging a plugin and swapping it into place. They are named so that nothing reads them
/// as a plugin, but they are a copy of one and they would sit there for good.
fn sweep_unfinished() {
    let Ok(entries) = std::fs::read_dir(plugins_dir()) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with(".staging-") && !name.starts_with(".clone-") {
            continue;
        }
        // Not one this Agentty is using right now: an install in flight has this process's id.
        if name.ends_with(&format!("-{}", std::process::id())) {
            continue;
        }
        let _ = std::fs::remove_dir_all(entry.path());
    }
}

/// Every installed plugin, sorted by name. Folders without a manifest are skipped.
pub fn installed() -> Vec<InstalledPlugin> {
    sweep_unfinished();
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

pub const BUILTIN: &[BuiltinPlugin] = &[
    BuiltinPlugin {
        manifest_json: embedded!("plugins/cosmica/agentty-plugin.json"),
        files: &[
            ("agentty-plugin.json", embedded!("plugins/cosmica/agentty-plugin.json")),
            ("main.mjs", embedded!("plugins/cosmica/main.mjs")),
            ("cosmica.mjs", embedded!("plugins/cosmica/cosmica.mjs")),
            ("README.md", embedded!("plugins/cosmica/README.md")),
            ("agentty-plugin.mjs", NODE_SDK),
        ],
    },
    BuiltinPlugin {
        manifest_json: embedded!("plugins/launch/agentty-plugin.json"),
        files: &[
            ("agentty-plugin.json", embedded!("plugins/launch/agentty-plugin.json")),
            ("main.mjs", embedded!("plugins/launch/main.mjs")),
            ("lib/exec.mjs", embedded!("plugins/launch/lib/exec.mjs")),
            ("lib/parse.mjs", embedded!("plugins/launch/lib/parse.mjs")),
            ("lib/github.mjs", embedded!("plugins/launch/lib/github.mjs")),
            ("lib/vercel.mjs", embedded!("plugins/launch/lib/vercel.mjs")),
            ("lib/supabase.mjs", embedded!("plugins/launch/lib/supabase.mjs")),
            ("lib/tools.mjs", embedded!("plugins/launch/lib/tools.mjs")),
            ("lib/state.mjs", embedded!("plugins/launch/lib/state.mjs")),
            ("lib/hosting.mjs", embedded!("plugins/launch/lib/hosting.mjs")),
            ("README.md", embedded!("plugins/launch/README.md")),
            ("agentty-plugin.mjs", NODE_SDK),
        ],
    },
];

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

/// Writes a plugin that is one module and a manifest: what the marketplace installs. The bytes
/// were already weighed against the checksum in the entry; nothing reaches the plugins folder
/// before that.
pub fn install_module(manifest: &Manifest, module: &[u8], source: Source, origin: Option<String>) -> Result<InstalledPlugin> {
    anyhow::ensure!(manifest.runtime == super::manifest::Runtime::Wasm, "only a module is installed this way");
    manifest.validate()?;
    let staging = staging_dir(&manifest.id)?;
    write_file(&staging.join(MANIFEST_FILE), &serde_json::to_string_pretty(manifest)?)?;
    write_bytes(&staging.join(relative_path(&manifest.main)?), module)?;
    finish_install(manifest, &staging, source, origin)
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
    let output = crate::process::command("git")
        .args(["clone", "--depth", "1", "--", url])
        .arg(&tmp)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .output()
        .context("git is not available")?;
    if !output.status.success() {
        let _ = std::fs::remove_dir_all(&tmp);
        // git repeats the address it was given, and the address may have carried a token.
        bail!("git clone failed: {}", crate::extensions::mask_words(String::from_utf8_lossy(&output.stderr).trim()));
    }
    // Copied like a folder install, so symlinks in the repository can't point outside the plugin.
    let result = Manifest::load(&tmp).and_then(|manifest| {
        let staging = staging_dir(&manifest.id)?;
        copy_tree(&tmp, &staging, 0)?;
        // Where it came from is worth keeping; the token somebody pasted into the address is not,
        // and `state.json` outlives the moment it was pasted.
        finish_install(&manifest, &staging, Source::Git, Some(crate::extensions::redact_url(url)))
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
///
/// What the plugin kept goes with it. That folder holds what the user gave the plugin — an HTTP
/// client's saved requests hold the tokens they were sent with — and a plugin that is no longer
/// installed has no business leaving them on disk. An update does not come through here: it
/// replaces the plugin's folder and leaves its data alone.
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
    // A development link keeps its own folder, but what it kept while it ran was Agentty's to
    // hold and is removed either way.
    let data = plugin_data_dir(id);
    if data.exists() {
        std::fs::remove_dir_all(&data).with_context(|| format!("could not remove {}", data.display()))?;
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
    write_bytes(path, contents.as_bytes())
}

fn write_bytes(path: &Path, contents: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, contents)?;
    Ok(())
}

/// Copies a folder, skipping VCS data; `node_modules` is kept (plugins may bundle dependencies).
/// A Rust plugin's `target/` is not: it holds gigabytes of build output, and the module it
/// produced is already beside `agentty-plugin.json`.
fn copy_tree(from: &Path, to: &Path, depth: usize) -> Result<()> {
    if depth > 12 {
        bail!("plugin folder is nested too deeply");
    }
    let rust_build = depth == 0 && from.join("Cargo.toml").exists();
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)?.flatten() {
        let name = entry.file_name();
        if name == ".git" || name == ".DS_Store" || (rust_build && name == "target") {
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

/// Largest logo kept. A logo is a small square; anything bigger is a download, not an icon.
const MAX_LOGO_BYTES: usize = 512 * 1024;

/// Whether a fetched body may be kept as a logo: a picture of a kind Agentty draws, within the
/// cap, and really of that kind — the type in the header alone is whatever the server chose to say.
///
/// SVG is refused on purpose, however it is labelled. An SVG is a document, not a picture: the
/// renderer resolves the addresses inside it, and a local path there is opened and drawn. A logo
/// that arrives from an address someone else controls must not be able to draw one of the user's
/// own files into Agentty's window, so only the formats below — which hold pixels and nothing
/// else — are kept. A logo is a small square; none of this costs a real logo anything.
fn logo_body_ok(content_type: &str, bytes: &[u8]) -> bool {
    let kind = content_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    let named = matches!(kind.as_str(), "image/png" | "image/jpeg" | "image/jpg" | "image/gif" | "image/webp");
    named && (1..=MAX_LOGO_BYTES).contains(&bytes.len()) && is_raster_image(bytes)
}

/// Whether the bytes begin the way one of the formats above does.
fn is_raster_image(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        || bytes.starts_with(&[0xff, 0xd8, 0xff])
        || bytes.starts_with(b"GIF87a")
        || bytes.starts_with(b"GIF89a")
        || (bytes.len() > 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP")
}

/// Where a fetched logo is kept: in the plugin's own data folder, so uninstalling takes it too.
fn logo_cache(id: &str) -> PathBuf {
    plugin_data_dir(id).join("logo.img")
}

/// The file to draw as `plugin`'s logo, if there is one ready. A logo shipped in the plugin folder
/// is used straight away; one given as an address is only used once it has been fetched, so the
/// icon name stands in until then and nothing blocks on the network to draw a row.
pub fn logo_file(plugin: &InstalledPlugin) -> Option<PathBuf> {
    let logo = plugin.manifest.as_ref()?.logo.as_deref()?;
    match parse_logo(logo)? {
        Logo::File(relative) => {
            let path = plugin.dir.join(relative);
            path.is_file().then_some(path)
        }
        Logo::Url(_) => {
            let cached = logo_cache(&plugin.id);
            cached.is_file().then_some(cached)
        }
    }
}

/// Fetches `plugin`'s logo if it is an address and is not kept yet. Returns whether a new file
/// landed, so the caller knows to draw again. Failure is not an error worth showing: the plugin
/// keeps the icon it had.
pub fn fetch_logo(plugin: &InstalledPlugin) -> bool {
    let Some(logo) = plugin.manifest.as_ref().and_then(|m| m.logo.as_deref()) else { return false };
    let Some(Logo::Url(url)) = parse_logo(logo) else { return false };
    let cached = logo_cache(&plugin.id);
    if cached.is_file() {
        return false;
    }
    // `parse_logo` held the address in the manifest to https with a plain host, and a redirect is
    // a second address nothing checked: `https_only` keeps it off plain http, and two hops is all
    // an image host needs. Nothing is sent but the request itself.
    let agent = crate::http::agent_builder().timeout(std::time::Duration::from_secs(15)).https_only(true).redirects(2).build();
    let Ok(response) = agent.get(&url).set("User-Agent", "Agentty").call() else { return false };
    let kind = response.header("content-type").unwrap_or_default().to_string();
    let mut bytes = Vec::new();
    // Read one byte past the cap, so a body that is too long is seen as too long rather than cut
    // down to the limit and kept.
    if response.into_reader().take(MAX_LOGO_BYTES as u64 + 1).read_to_end(&mut bytes).is_err() {
        return false;
    }
    if !logo_body_ok(&kind, &bytes) {
        return false;
    }
    let Some(dir) = cached.parent() else { return false };
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    // Written beside the target and renamed, so a half-written file is never drawn.
    let tmp = cached.with_extension("part");
    if std::fs::write(&tmp, &bytes).is_err() {
        return false;
    }
    std::fs::rename(&tmp, &cached).is_ok()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A fetched logo is only kept when it is actually a small image: the bytes are handed to an
    /// image decoder, and the cap is what stops a "logo" from being a download.
    #[test]
    fn only_a_small_image_is_kept_as_a_logo() {
        let png = |len: usize| {
            let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
            bytes.resize(len, 0);
            bytes
        };
        let small = png(1024);
        assert!(logo_body_ok("image/png", &small));
        assert!(logo_body_ok("image/png; charset=binary", &small), "parameters after the type are fine");
        assert!(logo_body_ok("IMAGE/PNG", &small), "the header's case does not matter");
        assert!(logo_body_ok("image/jpeg", &[&[0xff, 0xd8, 0xff][..], &[0u8; 64][..]].concat()));
        assert!(logo_body_ok("image/gif", &[&b"GIF89a"[..], &[0u8; 64][..]].concat()));
        assert!(logo_body_ok("image/webp", &[&b"RIFF\0\0\0\0WEBP"[..], &[0u8; 64][..]].concat()));
        assert!(!logo_body_ok("text/html", &small), "not an image");
        assert!(!logo_body_ok("application/octet-stream", &small), "not an image");
        assert!(!logo_body_ok("", &small), "no type at all");
        assert!(!logo_body_ok("image/png", &[]), "nothing to draw");
        assert!(!logo_body_ok("image/png", &png(MAX_LOGO_BYTES + 1)), "over the cap");
        assert!(logo_body_ok("image/png", &png(MAX_LOGO_BYTES)), "exactly the cap is fine");
    }

    /// An SVG is a document: whatever it says it is, the renderer would resolve the addresses
    /// inside it, and a local path there is opened and drawn. A logo arriving from someone else's
    /// address must not be able to put one of the user's own files on screen.
    #[test]
    fn an_svg_is_never_kept_as_a_logo() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg"><image href="/etc/hosts"/></svg>"#;
        assert!(!logo_body_ok("image/svg+xml", svg), "not kept under its own type");
        assert!(!logo_body_ok("image/png", svg), "and not kept by calling itself a PNG either");
        assert!(!logo_body_ok("image/png", b"<?xml version=\"1.0\"?><svg/>"), "nor with a prologue in front");
        assert!(!is_raster_image(svg));
    }

    /// Every script of a built-in plugin's folder is embedded: one left out would make the
    /// installed plugin fail on its first `import`.
    #[test]
    fn builtin_plugins_embed_every_script() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for plugin in BUILTIN {
            let id = plugin.manifest().id;
            let dir = root.join("plugins").join(&id);
            let mut scripts = Vec::new();
            for sub in ["", "lib"] {
                let Ok(entries) = std::fs::read_dir(dir.join(sub)) else { continue };
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.ends_with(".mjs") && name != "agentty-plugin.mjs" {
                        scripts.push(if sub.is_empty() { name } else { format!("{sub}/{name}") });
                    }
                }
            }
            for script in scripts {
                assert!(plugin.files.iter().any(|(name, _)| *name == script), "{id}: {script} is not embedded");
            }
        }
    }

    thread_local! {
        /// Data directory of the test running on this thread (instead of `~/.agentty`).
        ///
        /// Per thread, because tests run side by side and each needs its own. That also means a
        /// thread a test *spawns* does not have it and would write into the real `~/.agentty`:
        /// a thread that touches the store must call [`adopt`] first.
        pub static ROOT: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
    }

    /// Puts this thread in the same data directory as the test that spawned it. Without it, a
    /// spawned thread writes to the user's own `~/.agentty` — which is not a test failure, it is
    /// a test reaching outside the machine's test data and changing what the user has installed.
    pub(crate) fn adopt(root: &Path) {
        ROOT.with(|r| *r.borrow_mut() = Some(root.to_path_buf()));
    }

    pub(crate) fn with_data_dir(test: impl FnOnce(&Path)) {
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

    #[test]
    fn a_token_in_a_git_address_is_not_what_is_kept() {
        // Someone pastes the address their host gave them. It is used, and then it is gone:
        // `state.json` is read long after the moment it was pasted.
        let with_credentials = "https://someone:ghp_example_not_a_real_token@github.com/someone/a-plugin.git";
        let kept = crate::extensions::redact_url(with_credentials);
        assert!(!kept.contains("ghp_example_not_a_real_token"), "{kept}");
        assert!(!kept.contains("someone:"), "{kept}");
        // And what it is for — which repository — is still readable.
        assert!(kept.contains("github.com/someone/a-plugin.git"), "{kept}");
    }

    #[cfg(unix)]
    #[test]
    fn what_is_installed_is_written_where_only_the_user_can_read_it() {
        use std::os::unix::fs::PermissionsExt;
        with_data_dir(|_| {
            install_builtin("cosmica").unwrap();
            let mode = std::fs::metadata(state_path()).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "{mode:o}");
        });
    }

    #[test]
    fn a_damaged_state_file_does_not_turn_every_plugin_back_on() {
        with_data_dir(|_| {
            install_builtin("cosmica").unwrap();
            set_enabled("cosmica", false).unwrap();
            assert!(!installed()[0].enabled);

            // Truncated by a machine that lost power mid-write.
            std::fs::write(state_path(), b"{\"plugins\": {\"cosm").unwrap();
            // Read as empty, the plugin the user turned off would be running again.
            let state = StateFile::load();
            assert!(state.plugins.is_empty(), "it is not read as something it is not");
            let aside = state_path().with_file_name("state.damaged.json");
            assert!(aside.exists(), "what it said is kept");
            assert!(!state_path().exists(), "and is out of the way");

            // The plugin is still installed, and saving works again.
            set_enabled("cosmica", false).unwrap();
            assert!(!installed()[0].enabled);
        });
    }

    #[test]
    fn two_saves_at_once_do_not_share_a_temporary_file() {
        with_data_dir(|root| {
            let mut a = StateFile::load();
            a.plugins.insert("one".into(), PluginState { enabled: true, source: Source::Folder, path: None, origin: None });
            let mut b = StateFile::load();
            b.plugins.insert("two".into(), PluginState { enabled: false, source: Source::Folder, path: None, origin: None });
            // Whoever writes last wins, which is the old behaviour — what must not happen is the
            // two of them writing into one file and publishing the mixture.
            std::thread::scope(|scope| {
                scope.spawn(|| {
                    adopt(root);
                    a.save().unwrap()
                });
                scope.spawn(|| {
                    adopt(root);
                    b.save().unwrap()
                });
            });
            let state = StateFile::load();
            assert!(state.plugins.len() == 1, "a whole file was published, not a mixture");
            let leftovers =
                std::fs::read_dir(plugins_dir()).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().contains(".tmp")).count();
            assert_eq!(leftovers, 0, "no temporary file was left behind");
        });
    }

    #[test]
    fn what_a_plugin_kept_goes_when_the_plugin_does() {
        with_data_dir(|_| {
            let plugin = install_builtin("cosmica").unwrap();
            // What the plugin kept while it ran: an HTTP client's saved requests hold the tokens
            // they were sent with, and an uninstalled plugin should not leave them behind.
            super::super::storage::set(&plugin.id, "token", serde_json::json!("example_not_a_real_value")).unwrap();
            let data = plugin_data_dir(&plugin.id);
            assert!(data.exists());

            uninstall(&plugin.id).unwrap();
            assert!(!plugins_dir().join(&plugin.id).exists());
            assert!(!data.exists(), "what the plugin kept is still on disk");
            assert!(installed().is_empty());

            // And uninstalling something that kept nothing is not an error.
            uninstall("never-installed").unwrap();
        });
    }

    #[test]
    fn an_update_leaves_what_the_plugin_kept_alone() {
        with_data_dir(|_| {
            let plugin = install_builtin("cosmica").unwrap();
            super::super::storage::set(&plugin.id, "saved", serde_json::json!("a request")).unwrap();
            // Installing over it is what an update does.
            install_builtin("cosmica").unwrap();
            assert_eq!(super::super::storage::get(&plugin.id, "saved").unwrap(), "a request");
        });
    }

    #[cfg(unix)]
    #[test]
    fn a_link_in_a_plugin_folder_does_not_pull_in_what_it_points_at() {
        with_data_dir(|_| {
            let source = std::env::temp_dir().join(format!("agentty-link-test-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&source);
            std::fs::create_dir_all(source.join("secrets")).unwrap();
            std::fs::write(source.join("secrets/key"), b"not_a_real_key").unwrap();
            let folder = source.join("plugin");
            std::fs::create_dir_all(&folder).unwrap();
            std::fs::write(folder.join(MANIFEST_FILE), r#"{"id":"linky","name":"Linky","version":"0.1.0","main":"main.mjs"}"#).unwrap();
            std::fs::write(folder.join("main.mjs"), "// nothing").unwrap();
            // A plugin folder that reaches outside itself: a link to a file, and one to a folder.
            std::os::unix::fs::symlink(source.join("secrets/key"), folder.join("stolen")).unwrap();
            std::os::unix::fs::symlink(source.join("secrets"), folder.join("stolen-dir")).unwrap();

            let plugin = install_from_folder(&folder).unwrap();
            assert!(plugin.active(), "{:?}", plugin.error);
            assert!(plugin.dir.join("main.mjs").exists());
            assert!(!plugin.dir.join("stolen").exists(), "a link to a file was copied");
            assert!(!plugin.dir.join("stolen-dir").exists(), "a link to a folder was copied");
            let _ = std::fs::remove_dir_all(&source);
        });
    }

    #[test]
    fn a_folder_left_by_an_install_that_did_not_finish_is_swept_up() {
        with_data_dir(|_| {
            let stale = plugins_dir().join(".staging-hello-999999");
            let clone = plugins_dir().join(".clone-999999");
            let mine = plugins_dir().join(format!(".staging-hello-{}", std::process::id()));
            for dir in [&stale, &clone, &mine] {
                std::fs::create_dir_all(dir).unwrap();
            }
            let _ = installed();
            assert!(!stale.exists(), "a staging folder from a run that is gone stays");
            assert!(!clone.exists(), "a clone folder from a run that is gone stays");
            assert!(mine.exists(), "an install this Agentty is in the middle of was swept away");
        });
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
            // Agentty ships scripts, not modules: a plugin that is a module comes from the
            // marketplace, where its source and its checksum are.
            assert!(manifest.runtime.is_process(), "{} is a module; those are not bundled", manifest.id);
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

    /// A Rust plugin's folder holds its sources and, after a build, gigabytes of `target/`. What
    /// gets installed is the module and the manifest, not the build output.
    #[test]
    fn installing_a_wasm_plugin_leaves_the_build_output_behind() {
        with_data_dir(|root| {
            let source = root.join("wasm-plugin");
            std::fs::create_dir_all(source.join("target/wasm32-unknown-unknown/release")).unwrap();
            std::fs::create_dir_all(source.join("src")).unwrap();
            std::fs::write(
                source.join(MANIFEST_FILE),
                r#"{"id":"wasm-demo","name":"Demo","version":"1.0.0","runtime":"wasm","main":"demo.wasm"}"#,
            )
            .unwrap();
            std::fs::write(source.join("Cargo.toml"), "[package]\nname = \"demo\"\n").unwrap();
            std::fs::write(source.join("src/lib.rs"), "// source").unwrap();
            std::fs::write(source.join("demo.wasm"), b"\0asm").unwrap();
            std::fs::write(source.join("target/wasm32-unknown-unknown/release/huge.rlib"), vec![0u8; 4096]).unwrap();

            let installed = install_from_folder(&source).unwrap();
            assert!(installed.active(), "{:?}", installed.error);
            assert!(installed.dir.join("demo.wasm").exists());
            assert!(installed.dir.join("src/lib.rs").exists());
            assert!(!installed.dir.join("target").exists(), "target/ is not copied");
            assert!(source.join("target").exists(), "the developer's build output is left alone");
        });
    }

    /// A manifest that says `wasm` but points at a script is refused before it is installed.
    #[test]
    fn a_wasm_manifest_needs_a_module() {
        with_data_dir(|root| {
            let source = root.join("not-wasm");
            std::fs::create_dir_all(&source).unwrap();
            std::fs::write(
                source.join(MANIFEST_FILE),
                r#"{"id":"not-wasm","name":"No","version":"1.0.0","runtime":"wasm","main":"main.mjs"}"#,
            )
            .unwrap();
            std::fs::write(source.join("main.mjs"), "").unwrap();
            assert!(install_from_folder(&source).is_err());
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
