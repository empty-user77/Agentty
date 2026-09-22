//! `agentty-plugin.json`: what a plugin is, how it runs and what it adds to Agentty.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

pub const MANIFEST_FILE: &str = "agentty-plugin.json";

/// Protocol version this Agentty speaks (`apiVersion` in the manifest must not be newer).
///
/// | Version | What it added |
/// |---|---|
/// | 1 | the panel, commands, links, storage, `net/fetch`, `prompt/inject`, `session/get` |
/// | 2 | `host/timer` and `pane/status` — what a plugin needs to walk work through agents |
///
/// A plugin that uses something a version added says so, and an Agentty that speaks less than
/// that tells the user to update instead of installing a module it cannot run.
pub const API_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    /// Lower-case id: letters, digits and `-` (also the folder name under `~/.agentty/plugins`).
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub publisher: String,
    #[serde(default)]
    pub description: String,
    /// Icon name from Agentty's icon set (unknown names fall back to a generic icon).
    #[serde(default)]
    pub icon: Option<String>,
    /// The plugin's own logo, preferred over `icon` when it is there: a file inside the plugin
    /// folder (`logo.png`). A plugin that is one module carries its logo in the module instead.
    #[serde(default)]
    pub logo: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    /// Pages worth opening from the store card (project site, docs, source).
    #[serde(default)]
    pub links: Vec<LinkEntry>,
    /// The app or service this plugin is for, so the store can point at it when it is missing.
    #[serde(default)]
    pub requires: Option<Requirement>,
    /// Entry point, relative to the plugin folder.
    pub main: String,
    #[serde(default)]
    pub runtime: Runtime,
    #[serde(default = "default_api_version")]
    pub api_version: u32,
    /// `onStartup` starts the plugin with Agentty; otherwise it starts on first use.
    #[serde(default)]
    pub activation_events: Vec<String>,
    #[serde(default)]
    pub contributes: Contributes,
    #[serde(default)]
    pub permissions: Vec<String>,
    /// Apps or files this plugin integrates with; the store marks the plugin as recommended when found.
    #[serde(default)]
    pub detect: Vec<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
}

/// A manifest that leaves `apiVersion` out is from before the field existed, which can only mean
/// the first protocol — not whatever this Agentty happens to speak.
fn default_api_version() -> u32 {
    1
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Runtime {
    /// `node <main>` (Node.js 18+ from the user's login shell PATH).
    #[default]
    Node,
    /// `python3 <main>`.
    Python,
    /// `<main>` is an executable.
    Executable,
    /// `<main>` is a WebAssembly module, run inside Agentty by an interpreter. It reaches nothing
    /// but the host calls in this protocol: no files, no network, no environment, no processes.
    Wasm,
}

impl Runtime {
    /// Whether the plugin runs as a program of the user's, with everything the user can reach.
    /// `wasm` does not: that is the point of it.
    pub fn is_process(self) -> bool {
        !matches!(self, Runtime::Wasm)
    }
}

/// Where a plugin's panel is reached from. A plugin picks one place; `pane` is what plugins that
/// say nothing have always had.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Surface {
    /// An icon in the activity bar down the left edge, like Agentty's own pages.
    Sidebar,
    /// An icon in the tab strip above the terminals.
    #[default]
    Pane,
    /// An icon at the left end of the status bar along the bottom.
    Status,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Contributes {
    #[serde(default)]
    pub commands: Vec<CommandContribution>,
    /// A panel docked right of the terminals that the plugin fills with UI.
    #[serde(default)]
    pub panel: Option<PanelContribution>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandContribution {
    /// Unique within the plugin, e.g. `cosmica.saveSession`.
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub icon: Option<String>,
    /// Listed in the command palette (default true).
    #[serde(default = "yes")]
    pub palette: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkEntry {
    pub label: String,
    /// `https://…` only.
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Requirement {
    /// What to install ("Cosmica", "Docker", …).
    pub name: String,
    /// Where to get it (`https://…`).
    #[serde(default)]
    pub url: Option<String>,
    /// One line about what it is for.
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelContribution {
    pub title: String,
    #[serde(default)]
    pub icon: Option<String>,
    /// Which of Agentty's three surfaces the panel's icon sits on.
    #[serde(default)]
    pub surface: Surface,
    /// How the panel opens. The user can change it; this is what it does first.
    #[serde(default)]
    pub mode: PanelMode,
}

/// How a panel takes its place in the window.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PanelMode {
    /// Docked beside the terminals, which move over to make room.
    #[default]
    Push,
    /// Floating above the window at its right edge; nothing else moves.
    Overlay,
    /// A window of its own, which can be moved and resized like any other.
    Window,
    /// The whole area the terminals and pages use, like the Git page.
    Full,
}

impl PanelMode {
    pub const ALL: &'static [PanelMode] = &[PanelMode::Push, PanelMode::Overlay, PanelMode::Window, PanelMode::Full];

    /// The name used in `agentty-plugin.json` and in the settings.
    pub fn id(self) -> &'static str {
        match self {
            PanelMode::Push => "push",
            PanelMode::Overlay => "overlay",
            PanelMode::Window => "window",
            PanelMode::Full => "full",
        }
    }

    pub fn from_id(id: &str) -> Option<PanelMode> {
        PanelMode::ALL.iter().copied().find(|mode| mode.id() == id)
    }

    /// Whether the panel sits in the window's layout (and so takes room from it).
    pub fn is_docked(self) -> bool {
        self == PanelMode::Push
    }
}

/// Capabilities a plugin must declare before the matching host methods work.
pub const PERMISSIONS: &[(&str, &str)] = &[
    ("net.request", "Make HTTP requests to the addresses you give it"),
    ("prompt.inject", "Open agent sessions of its own and send them prompts"),
    ("terminal.write", "Type into any open terminal and press Enter, a shell included"),
    ("session.read", "Read the conversation of AI sessions open in Agentty"),
    ("workspace.read", "See open workspaces, tabs, folders and agent status"),
];

impl Manifest {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let manifest: Manifest = serde_json::from_slice(bytes).context("invalid agentty-plugin.json")?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn load(dir: &Path) -> Result<Self> {
        let path = dir.join(MANIFEST_FILE);
        let bytes = std::fs::read(&path).with_context(|| format!("no {MANIFEST_FILE} in {}", dir.display()))?;
        Self::parse(&bytes)
    }

    pub fn validate(&self) -> Result<()> {
        if !valid_id(&self.id) {
            bail!("plugin id \"{}\" must be 2–40 lower-case letters, digits or '-'", self.id);
        }
        if self.name.trim().is_empty() {
            bail!("plugin \"{}\" has no name", self.id);
        }
        if self.api_version > API_VERSION {
            bail!("plugin \"{}\" needs a newer Agentty (plugin API {} > {API_VERSION})", self.id, self.api_version);
        }
        let main = relative_path(&self.main).with_context(|| format!("plugin \"{}\": invalid main", self.id))?;
        if self.runtime == Runtime::Wasm && main.extension().is_none_or(|e| e != "wasm") {
            bail!("plugin \"{}\": a wasm plugin's main must be a .wasm module", self.id);
        }
        let mut seen = std::collections::HashSet::new();
        for command in &self.contributes.commands {
            if command.id.trim().is_empty() || command.title.trim().is_empty() {
                bail!("plugin \"{}\": every command needs an id and a title", self.id);
            }
            if !seen.insert(command.id.as_str()) {
                bail!("plugin \"{}\": duplicate command {}", self.id, command.id);
            }
        }
        let web = |url: &str| url.starts_with("https://") && url.len() < 300 && !url.chars().any(char::is_whitespace);
        if self.homepage.as_deref().is_some_and(|url| !web(url)) {
            bail!("plugin \"{}\": homepage must be an https:// URL", self.id);
        }
        if self.links.len() > 6 {
            bail!("plugin \"{}\": at most 6 links", self.id);
        }
        for link in &self.links {
            if link.label.trim().is_empty() || link.label.chars().count() > 40 || !web(&link.url) {
                bail!("plugin \"{}\": every link needs a short label and an https:// URL", self.id);
            }
        }
        if let Some(requires) = &self.requires {
            if requires.name.trim().is_empty() || requires.url.as_deref().is_some_and(|url| !web(url)) {
                bail!("plugin \"{}\": \"requires\" needs a name and, if given, an https:// URL", self.id);
            }
        }
        for permission in &self.permissions {
            if !PERMISSIONS.iter().any(|(name, _)| name == permission) {
                bail!("plugin \"{}\": unknown permission {permission}", self.id);
            }
        }
        Ok(())
    }

    pub fn has_permission(&self, permission: &str) -> bool {
        self.permissions.iter().any(|p| p == permission)
    }

    /// Where this plugin's panel is reached from (`Pane` when it contributes no panel).
    pub fn surface(&self) -> Surface {
        self.contributes.panel.as_ref().map_or(Surface::Pane, |panel| panel.surface)
    }

    /// How this plugin's panel opens until the user says otherwise.
    pub fn panel_mode(&self) -> PanelMode {
        self.contributes.panel.as_ref().map_or(PanelMode::Push, |panel| panel.mode)
    }

    pub fn starts_with_agentty(&self) -> bool {
        self.activation_events.iter().any(|e| e == "onStartup")
    }

    /// Entry point inside `dir`.
    pub fn entry(&self, dir: &Path) -> Result<PathBuf> {
        Ok(dir.join(relative_path(&self.main)?))
    }

    /// Whether something listed in `detect` exists (`~` expands to the home folder).
    pub fn detected(&self) -> bool {
        self.detect.iter().any(|p| expand_home(p).exists())
    }
}

/// Reads a manifest's `logo`: a file inside the plugin folder. Anything else — a path that
/// climbs out, an address, a `data:` blob, something enormous — is no logo at all, and the plugin
/// falls back to its icon name.
///
/// A logo is never fetched from anywhere. It travels with the plugin: in its folder, or in the
/// module itself (`LOGO_SECTION`). That way the bytes drawn are the bytes the user installed and
/// nothing about installing a plugin reaches its author.
pub fn parse_logo(logo: &str) -> Option<PathBuf> {
    let logo = logo.trim();
    if logo.is_empty() || logo.len() > 400 || logo.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    // Anything carrying a scheme is an address, not a file name — `https://host/x.png` is a
    // relative path as far as the filesystem is concerned, and taking it as one would quietly look
    // for a folder called `https:`.
    if logo.contains(':') || logo.contains("//") {
        return None;
    }
    // The same rule the entry point is held to, so it cannot point outside the plugin folder.
    relative_path(logo).ok()
}

pub fn valid_id(id: &str) -> bool {
    (2..=40).contains(&id.len())
        && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !id.starts_with('-')
        && !id.ends_with('-')
}

/// A path that stays inside the folder it is relative to.
pub fn relative_path(path: &str) -> Result<PathBuf> {
    let candidate = Path::new(path);
    if path.is_empty() || candidate.is_absolute() || candidate.components().any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        bail!("\"{path}\" must be a relative path inside the plugin folder");
    }
    Ok(candidate.to_path_buf())
}

pub fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => crate::fsutil::home().join(rest),
        None => PathBuf::from(path),
    }
}

/// `1.2.10` > `1.2.9`; non-numeric parts compare as 0.
pub fn version_newer(candidate: &str, current: &str) -> bool {
    let parse = |v: &str| -> Vec<u64> { v.split(['.', '-', '+']).take(3).map(|p| p.parse().unwrap_or(0)).collect() };
    parse(candidate) > parse(current)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> serde_json::Value {
        serde_json::json!({
            "id": "hello-world",
            "name": "Hello",
            "version": "0.1.0",
            "main": "main.mjs",
            "permissions": ["prompt.inject"],
            "contributes": {
                // `when` is not a field any more (it only ever gated `paneBar`, which is gone
                // too); a manifest that still has it from before must keep loading.
                "commands": [{ "id": "hello.say", "title": "Say hello", "when": "agent" }],
                "panel": { "title": "Hello", "icon": "sparkles" }
            }
        })
    }

    #[test]
    fn a_plugin_built_against_a_protocol_this_agentty_does_not_speak_is_refused() {
        let mut manifest = sample();
        manifest["apiVersion"] = serde_json::json!(API_VERSION + 1);
        let err = Manifest::parse(manifest.to_string().as_bytes()).expect_err("it needs a newer Agentty");
        assert!(format!("{err:#}").contains("needs a newer Agentty"), "{err:#}");
        // The one this Agentty speaks, and every one before it, load.
        for version in 1..=API_VERSION {
            let mut manifest = sample();
            manifest["apiVersion"] = serde_json::json!(version);
            assert!(Manifest::parse(manifest.to_string().as_bytes()).is_ok(), "apiVersion {version}");
        }
    }

    /// A logo is a file of the plugin's own or an `https://` address, and nothing else: a path that
    /// climbs out of the folder, plain http, a `data:` blob or a credential in the host would each
    /// be a way to make the app fetch or read something it should not.
    #[test]
    fn a_logo_is_a_file_of_the_plugins_own() {
        assert_eq!(parse_logo("logo.png"), Some("logo.png".into()));
        assert_eq!(parse_logo("assets/logo.svg"), Some("assets/logo.svg".into()));
        assert_eq!(parse_logo("  logo.png  "), Some("logo.png".into()));

        assert_eq!(parse_logo("../../secrets.png"), None);
        assert_eq!(parse_logo("/etc/passwd"), None);

        // No logo is ever fetched: an address is not a file of the plugin's, and must not be read
        // as the relative path it looks like to the filesystem either.
        assert_eq!(parse_logo("https://example.com/logo.png"), None);
        assert_eq!(parse_logo("http://example.com/logo.png"), None);
        assert_eq!(parse_logo("data:image/png;base64,AAAA"), None);
        assert_eq!(parse_logo("//example.com/logo.png"), None);

        assert_eq!(parse_logo(""), None);
        assert_eq!(parse_logo("   "), None);
        assert_eq!(parse_logo(&format!("{}.png", "x".repeat(400))), None);
        assert_eq!(parse_logo("a b.png"), None);
    }

    #[test]
    fn parses_a_manifest_with_defaults() {
        let manifest = Manifest::parse(sample().to_string().as_bytes()).unwrap();
        assert_eq!(manifest.runtime, Runtime::Node);
        // A manifest that says nothing is from before the field: the first protocol, not this one.
        assert_eq!(manifest.api_version, 1);
        let command = &manifest.contributes.commands[0];
        assert!(command.palette);
        assert!(manifest.has_permission("prompt.inject"));
        assert!(!manifest.has_permission("session.read"));
        // Plugins written before surfaces existed keep the icon they had, in the tab strip.
        assert_eq!(manifest.surface(), Surface::Pane);
        assert!(manifest.runtime.is_process());
    }

    #[test]
    fn a_panel_picks_one_surface() {
        let mut json = sample();
        json["contributes"]["panel"] = serde_json::json!({ "title": "Hello", "surface": "sidebar" });
        assert_eq!(Manifest::parse(json.to_string().as_bytes()).unwrap().surface(), Surface::Sidebar);
        json["contributes"]["panel"] = serde_json::json!({ "title": "Hello", "surface": "status" });
        assert_eq!(Manifest::parse(json.to_string().as_bytes()).unwrap().surface(), Surface::Status);
        json["contributes"]["panel"] = serde_json::json!({ "title": "Hello", "surface": "everywhere" });
        assert!(Manifest::parse(json.to_string().as_bytes()).is_err(), "a made-up surface is refused");
    }

    #[test]
    fn a_panel_opens_the_way_it_asks_to() {
        let mut json = sample();
        assert_eq!(Manifest::parse(json.to_string().as_bytes()).unwrap().panel_mode(), PanelMode::Push);
        for (name, mode) in
            [("overlay", PanelMode::Overlay), ("window", PanelMode::Window), ("full", PanelMode::Full), ("push", PanelMode::Push)]
        {
            json["contributes"]["panel"] = serde_json::json!({ "title": "Hello", "mode": name });
            assert_eq!(Manifest::parse(json.to_string().as_bytes()).unwrap().panel_mode(), mode);
            assert_eq!(PanelMode::from_id(name), Some(mode));
        }
        json["contributes"]["panel"] = serde_json::json!({ "title": "Hello", "mode": "sideways" });
        assert!(Manifest::parse(json.to_string().as_bytes()).is_err(), "a made-up mode is refused");
        assert_eq!(PanelMode::from_id("sideways"), None);
    }

    #[test]
    fn a_wasm_plugin_points_at_a_module() {
        let mut json = sample();
        json["runtime"] = serde_json::json!("wasm");
        assert!(Manifest::parse(json.to_string().as_bytes()).is_err(), "main.mjs is not a module");
        json["main"] = serde_json::json!("plugin.wasm");
        let manifest = Manifest::parse(json.to_string().as_bytes()).unwrap();
        assert_eq!(manifest.runtime, Runtime::Wasm);
        // It is not a program of the user's: it reaches only what the host hands it.
        assert!(!manifest.runtime.is_process());
    }

    #[test]
    fn parses_project_information() {
        let mut manifest = sample();
        manifest["homepage"] = serde_json::json!("https://example.com");
        manifest["links"] = serde_json::json!([{ "label": "Docs", "url": "https://example.com/docs" }]);
        manifest["requires"] = serde_json::json!({ "name": "Example", "url": "https://example.com/get" });
        let parsed = Manifest::parse(manifest.to_string().as_bytes()).unwrap();
        assert_eq!(parsed.links[0].label, "Docs");
        assert_eq!(parsed.requires.unwrap().name, "Example");

        for (key, value) in [
            ("homepage", serde_json::json!("javascript:alert(1)")),
            ("links", serde_json::json!([{ "label": "x", "url": "http://insecure.example" }])),
            ("links", serde_json::json!([{ "label": "", "url": "https://example.com" }])),
            ("requires", serde_json::json!({ "name": "", "url": "https://example.com" })),
            ("requires", serde_json::json!({ "name": "App", "url": "ftp://example.com" })),
        ] {
            let mut manifest = sample();
            manifest[key] = value;
            assert!(Manifest::parse(manifest.to_string().as_bytes()).is_err(), "{key} accepted");
        }
    }

    #[test]
    fn rejects_bad_ids_paths_and_permissions() {
        for (key, value) in [
            ("id", serde_json::json!("Hello World")),
            ("id", serde_json::json!("-x")),
            ("main", serde_json::json!("../escape.mjs")),
            ("main", serde_json::json!("/usr/bin/env")),
            ("permissions", serde_json::json!(["fs.everything"])),
            ("apiVersion", serde_json::json!(API_VERSION + 1)),
        ] {
            let mut manifest = sample();
            manifest[key] = value;
            assert!(Manifest::parse(manifest.to_string().as_bytes()).is_err(), "{key} accepted");
        }
        let mut duplicate = sample();
        duplicate["contributes"]["commands"] = serde_json::json!([{ "id": "a", "title": "A" }, { "id": "a", "title": "B" }]);
        assert!(Manifest::parse(duplicate.to_string().as_bytes()).is_err());
    }

    #[test]
    fn compares_versions() {
        assert!(version_newer("1.2.10", "1.2.9"));
        assert!(version_newer("0.2.0", "0.1.9"));
        assert!(!version_newer("0.1.0", "0.1.0"));
        assert!(!version_newer("0.1.0-beta", "0.1.1"));
    }
}
