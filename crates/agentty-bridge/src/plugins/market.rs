//! The marketplace: the list of plugins Agentty offers, and installing one from it.
//!
//! The list is `index.json` in `empty-user77/Agentty-Marketplace`, where plugins are added by
//! pull request. Everything in it was written by whoever submitted the plugin, so nothing here
//! trusts it: every field is checked again on this side, the module is weighed against the
//! checksum in the entry before it is written anywhere, and only a WebAssembly module is
//! installed at all — a plugin that runs as a program of the user's is something they choose
//! themselves, from a folder or a repository, not something a list on the internet hands them.

use super::manifest::{valid_id, version_newer, Manifest, PanelMode, Surface, PERMISSIONS};
use super::store::{self, InstalledPlugin, Source};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Read;

/// Where the list lives. `AGENTTY_MARKETPLACE_INDEX` points it somewhere else while developing.
const INDEX_URL: &str = "https://raw.githubusercontent.com/empty-user77/Agentty-Marketplace/main/index.json";

/// The index a newer Agentty might serve is not read by this one.
pub const API_VERSION: u32 = 1;
/// Largest index and largest module.
const MAX_INDEX_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_MODULE_BYTES: usize = 8 * 1024 * 1024;
const MAX_ENTRIES: usize = 500;
/// An `apiVersion` beyond this is not a future protocol, it is a typo or rubbish, and the entry is
/// dropped rather than listed as needing a newer Agentty.
const MAX_API_VERSION: u32 = 1000;
const MAX_NAME: usize = 60;
const MAX_DESCRIPTION: usize = 300;
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// Hosts a module may be served from: a release asset of a repository, not someone's server. The
/// host the list itself came from counts too — it is the same trust as the list, and it is what
/// makes a marketplace of one's own (or a local one, while developing) work.
const MODULE_HOSTS: &[&str] = &["github.com", "raw.githubusercontent.com", "objects.githubusercontent.com"];

fn module_host_allowed(host: &str) -> bool {
    if MODULE_HOSTS.contains(&host) {
        return true;
    }
    url::Url::parse(&index_url()).ok().and_then(|url| url.host_str().map(str::to_ascii_lowercase)).is_some_and(|index| index == host)
}

pub fn index_url() -> String {
    std::env::var("AGENTTY_MARKETPLACE_INDEX").unwrap_or_else(|_| INDEX_URL.to_string())
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    #[serde(default)]
    pub publisher: String,
    #[serde(default)]
    pub license: String,
    /// The public repository the module is built from.
    pub source: String,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub surface: Surface,
    #[serde(default)]
    pub mode: Option<PanelMode>,
    #[serde(default)]
    pub permissions: Vec<String>,
    /// The sites `browser.control` works on, shown on the card before installing.
    #[serde(default)]
    pub browser: Option<super::sites::BrowserContribution>,
    /// The plugin protocol the module is built against. An entry that leaves it out is from
    /// before the field existed, which can only mean the first one.
    #[serde(default = "first_api_version")]
    pub api_version: u32,
    /// `["onStartup"]`: the plugin starts with Agentty (an automation that runs on a schedule).
    /// Nothing else is taken from a listing.
    #[serde(default)]
    pub activation_events: Vec<String>,
    pub module: Module,
}

fn first_api_version() -> u32 {
    1
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Module {
    pub url: String,
    pub sha256: String,
    pub size: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Index {
    #[serde(default)]
    api_version: u32,
    #[serde(default)]
    plugins: Vec<serde_json::Value>,
}

/// Text that is shown to the user: bounded, one line, and there.
fn plain(value: &str, field: &str, limit: usize, required: bool) -> Result<String> {
    let value = value.trim();
    if required && value.is_empty() {
        bail!("{field} is missing");
    }
    if value.chars().count() > limit {
        bail!("{field} is longer than {limit} characters");
    }
    if value.chars().any(|c| c.is_control()) {
        bail!("{field} contains control characters");
    }
    Ok(value.to_string())
}

fn https_host(url: &str, field: &str) -> Result<String> {
    let parsed = url::Url::parse(url).with_context(|| format!("{field} is not a URL"))?;
    if parsed.scheme() != "https" {
        bail!("{field} must be an https:// URL");
    }
    Ok(parsed.host_str().unwrap_or_default().to_ascii_lowercase())
}

/// A module's host. `https` everywhere, except a list served over plain `http` — that is a
/// marketplace on this machine, being written.
fn module_url_host(url: &str) -> Result<String> {
    let parsed = url::Url::parse(url).context("module.url is not a URL")?;
    let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
    match parsed.scheme() {
        "https" => Ok(host),
        "http" if index_url().starts_with("http://") => Ok(host),
        _ => bail!("module.url must be an https:// URL"),
    }
}

impl Entry {
    /// An entry as the marketplace sent it, checked again here.
    pub fn checked(value: serde_json::Value) -> Result<Entry> {
        let mut entry: Entry = serde_json::from_value(value).context("not an entry")?;
        if !valid_id(&entry.id) {
            bail!("\"{}\" is not a plugin id", entry.id);
        }
        if super::store::is_builtin_id(&entry.id) {
            bail!("\"{}\" is the id of a plugin that comes with Agentty", entry.id);
        }
        entry.name = plain(&entry.name, "name", MAX_NAME, true)?;
        entry.description = plain(&entry.description, "description", MAX_DESCRIPTION, false)?;
        entry.publisher = plain(&entry.publisher, "publisher", MAX_NAME, false)?;
        entry.license = plain(&entry.license, "license", 40, false)?;
        entry.version = plain(&entry.version, "version", 40, true)?;
        if !entry.version.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            bail!("version is major.minor.patch");
        }
        https_host(&entry.source, "source")?;
        if let Some(homepage) = &entry.homepage {
            https_host(homepage, "homepage")?;
        }
        if entry.api_version == 0 || entry.api_version > MAX_API_VERSION {
            bail!("apiVersion is between 1 and {MAX_API_VERSION}");
        }
        for permission in &entry.permissions {
            if !PERMISSIONS.iter().any(|(name, _)| name == permission) {
                bail!("unknown permission {permission}");
            }
        }
        if let Some(browser) = &entry.browser {
            browser.validate()?;
        }
        let host = module_url_host(&entry.module.url)?;
        if !module_host_allowed(&host) {
            bail!("a module is not served from {host}");
        }
        if entry.module.sha256.len() != 64 || !entry.module.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
            bail!("module.sha256 is 64 hex characters");
        }
        if entry.module.size == 0 || entry.module.size > MAX_MODULE_BYTES {
            bail!("module.size is up to {} MB", MAX_MODULE_BYTES / 1024 / 1024);
        }
        // An icon name Agentty does not have falls back to a generic one; anything that is not a
        // name at all is dropped here rather than carried into the manifest.
        entry.icon = entry.icon.map(|icon| icon.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(40).collect());
        entry.keywords =
            entry.keywords.iter().filter_map(|word| plain(word, "keyword", 30, false).ok()).filter(|w| !w.is_empty()).take(10).collect();
        Ok(entry)
    }

    /// The manifest Agentty writes for this plugin. A marketplace plugin is always a module.
    pub fn manifest(&self) -> Manifest {
        Manifest {
            id: self.id.clone(),
            name: self.name.clone(),
            version: self.version.clone(),
            publisher: self.publisher.clone(),
            description: self.description.clone(),
            icon: self.icon.clone(),
            // A listing has no logo of its own: the module carries it, and it is written out when
            // the plugin is installed (`store::logo_in_module`).
            logo: None,
            homepage: self.homepage.clone(),
            links: vec![super::manifest::LinkEntry { label: "Source".into(), url: self.source.clone() }],
            requires: None,
            main: format!("{}.wasm", self.id),
            runtime: super::manifest::Runtime::Wasm,
            api_version: self.api_version,
            activation_events: self.activation_events.iter().filter(|e| e.as_str() == "onStartup").cloned().collect(),
            contributes: super::manifest::Contributes {
                commands: Vec::new(),
                panel: Some(super::manifest::PanelContribution {
                    title: self.name.clone(),
                    icon: self.icon.clone(),
                    surface: self.surface,
                    mode: self.mode,
                }),
            },
            permissions: self.permissions.clone(),
            browser: self.browser.clone(),
            detect: Vec::new(),
            keywords: self.keywords.clone(),
        }
    }

    /// Whether this entry is newer than what is installed.
    /// Whether this Agentty speaks the protocol the module is built against. An entry that needs
    /// a newer one is still listed — the page says so instead of offering to install it.
    pub fn supported(&self) -> bool {
        self.api_version <= super::manifest::API_VERSION
    }

    pub fn newer_than(&self, installed: &InstalledPlugin) -> bool {
        self.supported() && installed.manifest.as_ref().is_some_and(|m| version_newer(&self.version, &m.version))
    }
}

/// Where the last list read is kept, so the page has something to show before the network
/// answers — and still does when it cannot.
fn cache_path() -> std::path::PathBuf {
    // The same data directory everything else uses, and the same hook the tests move aside: a
    // test must never write into the real ~/.agentty.
    #[cfg(test)]
    if let Some(root) = super::store::tests::ROOT.with(|root| root.borrow().clone()) {
        return root.join("marketplace.json");
    }
    crate::fsutil::data_dir().join("marketplace.json")
}

/// The list as it was last read, and when. Nothing here is trusted any more than a fresh read:
/// every entry goes through the same checks.
pub fn cached() -> Option<(Vec<Entry>, std::time::SystemTime)> {
    let path = cache_path();
    let read_at = std::fs::metadata(&path).and_then(|meta| meta.modified()).ok()?;
    let bytes = std::fs::read(&path).ok()?;
    parse(&bytes).ok().filter(|entries| !entries.is_empty()).map(|entries| (entries, read_at))
}

fn keep(bytes: &[u8]) {
    let path = cache_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, bytes).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// The list, as the app can use it. Blocking: callers run it off the main thread.
pub fn fetch() -> Result<Vec<Entry>> {
    let agent = crate::http::agent_builder().timeout(TIMEOUT).build();
    let response = agent.get(&index_url()).call().context("could not reach the marketplace")?;
    let mut bytes = Vec::new();
    response.into_reader().take(MAX_INDEX_BYTES as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_INDEX_BYTES {
        bail!("the marketplace list is larger than {} MB", MAX_INDEX_BYTES / 1024 / 1024);
    }
    let entries = parse(&bytes)?;
    // Only a list that could be read is kept, so a broken one does not become what the page shows.
    keep(&bytes);
    Ok(entries)
}

/// Reads a list, keeping the entries that check out and leaving the rest.
pub fn parse(bytes: &[u8]) -> Result<Vec<Entry>> {
    let index: Index = serde_json::from_slice(bytes).context("the marketplace list could not be read")?;
    if index.api_version > API_VERSION {
        bail!("this list needs a newer Agentty");
    }
    let mut entries = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for value in index.plugins.into_iter().take(MAX_ENTRIES) {
        match Entry::checked(value) {
            // One bad entry does not hide the rest of the list.
            Ok(entry) if seen.insert(entry.id.clone()) => entries.push(entry),
            Ok(_) => {}
            Err(err) => eprintln!("agentty: marketplace entry skipped: {err:#}"),
        }
    }
    entries.sort_by_key(|entry| entry.name.to_lowercase());
    Ok(entries)
}

/// Downloads the module, weighs it against the entry, and installs the plugin. Nothing is written
/// where Agentty looks for plugins until the checksum matches.
pub fn install(entry: &Entry) -> Result<InstalledPlugin> {
    if !entry.supported() {
        bail!("\"{}\" needs a newer Agentty (plugin API {} > {})", entry.id, entry.api_version, super::manifest::API_VERSION);
    }
    let module = download(entry)?;
    store::install_module(&entry.manifest(), &module, Source::Market, Some(entry.source.clone()))
}

/// The module behind an entry: at most what the entry says, and exactly what it promised.
pub fn download(entry: &Entry) -> Result<Vec<u8>> {
    let agent = crate::http::agent_builder().timeout(TIMEOUT).redirects(4).build();
    let response = agent.get(&entry.module.url).call().context("the module could not be downloaded")?;
    let mut bytes = Vec::new();
    response.into_reader().take(entry.module.size as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() != entry.module.size {
        bail!("the module is {} bytes, the marketplace says {}", bytes.len(), entry.module.size);
    }
    let digest = format!("{:x}", Sha256::digest(&bytes));
    if !digest.eq_ignore_ascii_case(&entry.module.sha256) {
        bail!("the module does not match its checksum");
    }
    if !bytes.starts_with(b"\0asm") {
        bail!("that file is not a WebAssembly module");
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry() -> serde_json::Value {
        json!({
            "id": "hello-world",
            "name": "Hello World",
            "version": "0.1.0",
            "description": "A panel that says hello.",
            "publisher": "Someone",
            "license": "MIT",
            "source": "https://github.com/someone/agentty-hello-world",
            "surface": "sidebar",
            "permissions": [],
            "module": {
                "url": "https://github.com/someone/agentty-hello-world/releases/download/v0.1.0/hello-world.wasm",
                "sha256": "a".repeat(64),
                "size": 93292
            }
        })
    }

    #[test]
    fn an_entry_becomes_a_manifest_for_a_module() {
        let checked = Entry::checked(entry()).unwrap();
        let manifest = checked.manifest();
        assert_eq!(manifest.runtime, super::super::manifest::Runtime::Wasm);
        assert_eq!(manifest.main, "hello-world.wasm");
        assert_eq!(manifest.surface(), Surface::Sidebar);
        assert_eq!(manifest.links[0].url, "https://github.com/someone/agentty-hello-world");
        manifest.validate().expect("the manifest Agentty writes is a valid one");
    }

    #[test]
    fn what_the_list_says_is_checked_again_here() {
        let cases: Vec<(&str, serde_json::Value)> = vec![
            ("id", json!("Hello World")),
            ("id", json!("../escape")),
            ("version", json!("latest")),
            ("name", json!("")),
            ("name", json!("x".repeat(100))),
            ("description", json!("two\nlines")),
            ("source", json!("http://github.com/a/b")),
            ("source", json!("javascript:alert(1)")),
            ("homepage", json!("http://example.com")),
            ("permissions", json!(["fs.everything"])),
        ];
        for (field, value) in cases {
            let mut bad = entry();
            bad[field] = value.clone();
            assert!(Entry::checked(bad).is_err(), "accepted {field} = {value}");
        }
    }

    #[test]
    fn a_module_comes_from_a_release_and_says_how_big_it_is() {
        for change in [
            json!({ "url": "https://example.com/x.wasm", "sha256": "a".repeat(64), "size": 10 }),
            json!({ "url": "http://github.com/a/b/x.wasm", "sha256": "a".repeat(64), "size": 10 }),
            json!({ "url": "https://github.com/a/b/x.wasm", "sha256": "nope", "size": 10 }),
            json!({ "url": "https://github.com/a/b/x.wasm", "sha256": "a".repeat(64), "size": 0 }),
            json!({ "url": "https://github.com/a/b/x.wasm", "sha256": "a".repeat(64), "size": MAX_MODULE_BYTES + 1 }),
        ] {
            let mut bad = entry();
            bad["module"] = change.clone();
            assert!(Entry::checked(bad).is_err(), "accepted {change}");
        }
    }

    /// The marketplace may serve the modules it lists; nowhere else may.
    #[test]
    fn a_module_may_come_from_where_the_list_came_from() {
        assert!(module_host_allowed("github.com"));
        assert!(!module_host_allowed("example.com"));
        // Pointed at a list of one's own, that list's host serves its modules too.
        std::env::set_var("AGENTTY_MARKETPLACE_INDEX", "https://plugins.example.com/index.json");
        assert!(module_host_allowed("plugins.example.com"));
        assert!(module_host_allowed("github.com"));
        assert!(!module_host_allowed("elsewhere.example"));
        std::env::remove_var("AGENTTY_MARKETPLACE_INDEX");
        assert!(!module_host_allowed("plugins.example.com"));
    }

    /// What was read last time is kept, and read back through the same checks.
    #[test]
    fn the_list_is_kept_between_runs() {
        super::super::store::tests::with_data_dir(|_| {
            assert!(cached().is_none(), "nothing kept yet");
            let list = serde_json::json!({ "apiVersion": 1, "plugins": [entry()] });
            keep(list.to_string().as_bytes());
            let (entries, _) = cached().expect("the list comes back");
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].id, "hello-world");

            // A file that is not a list at all is not what the page shows.
            keep(b"not json");
            assert!(cached().is_none());
            // Neither is an empty one: there would be nothing to show anyway.
            keep(serde_json::json!({ "apiVersion": 1, "plugins": [] }).to_string().as_bytes());
            assert!(cached().is_none());
        });
    }

    #[test]
    fn an_entry_without_an_api_version_is_the_first_protocol() {
        let parsed = Entry::checked(entry()).expect("a good entry");
        assert_eq!(parsed.api_version, 1);
        assert!(parsed.supported());
        // The manifest carries the entry's protocol, not whatever this Agentty happens to speak.
        assert_eq!(parsed.manifest().api_version, 1);
    }

    #[test]
    fn an_entry_built_against_a_newer_protocol_is_listed_but_not_installed() {
        let newer = crate::plugins::manifest::API_VERSION + 1;
        let mut value = entry();
        value["apiVersion"] = json!(newer);
        // It survives checking, so the page can say why it cannot be installed...
        let parsed = Entry::checked(value).expect("still an entry");
        assert!(!parsed.supported());
        // ...install refuses it even when something reaches past the page...
        let refusal = install(&parsed).expect_err("install refuses it").to_string();
        assert!(refusal.contains("needs a newer Agentty"), "{refusal}");
        // ...and the manifest it would produce is one the loader itself refuses.
        assert_eq!(parsed.manifest().api_version, newer);
        assert!(parsed.manifest().validate().is_err(), "a manifest needing a newer Agentty must not load");
    }

    #[test]
    fn rubbish_api_versions_are_dropped() {
        for bad in [json!(0), json!(MAX_API_VERSION + 1), json!(u32::MAX)] {
            let mut value = entry();
            value["apiVersion"] = bad.clone();
            assert!(Entry::checked(value).is_err(), "apiVersion {bad} must be refused");
        }
    }

    #[test]
    fn a_list_keeps_what_is_good_and_drops_the_rest() {
        let mut broken = entry();
        broken["id"] = json!("NOPE");
        let mut twin = entry();
        twin["name"] = json!("Hello Again");
        let list = json!({ "apiVersion": 1, "plugins": [entry(), broken, twin] });
        let entries = parse(list.to_string().as_bytes()).unwrap();
        // The broken one is left out, and an id that is already there is not taken twice.
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "Hello World");

        // A list from a newer Agentty is not guessed at.
        let newer = json!({ "apiVersion": API_VERSION + 1, "plugins": [] });
        assert!(parse(newer.to_string().as_bytes()).is_err());
        assert!(parse(b"not json").is_err());
    }
}
