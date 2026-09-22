//! Which browser on this machine an agent can drive.
//!
//! Claude Code and Codex each ship a browser extension that works inside the browser the user
//! already has, with the sessions they are already signed into. Driving a browser that way needs
//! two halves, and both are per browser rather than per machine:
//!
//! - the **extension** itself, unpacked in one of that browser's profiles;
//! - the **native messaging host**, a small file the agent's CLI writes so the extension can talk
//!   back to it. A browser reads that file at startup from its own configuration folder.
//!
//! So Chrome can be ready while Edge beside it is not, which is why the user picks the browser to
//! work in rather than Agentty guessing. Nothing here launches or talks to a browser: it only
//! reports what is on disk, so a settings page can say what is ready and what is missing.

use std::path::{Path, PathBuf};

/// An agent that can drive a browser, and what it leaves on disk when it can.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentBrowser {
    Claude,
    Codex,
}

impl AgentBrowser {
    pub const ALL: [AgentBrowser; 2] = [AgentBrowser::Claude, AgentBrowser::Codex];

    pub fn id(self) -> &'static str {
        match self {
            AgentBrowser::Claude => "claude",
            AgentBrowser::Codex => "codex",
        }
    }

    /// The native messaging host the agent's CLI writes, without the `.json`.
    fn host(self) -> &'static str {
        match self {
            AgentBrowser::Claude => "com.anthropic.claude_browser_extension",
            AgentBrowser::Codex => "com.openai.codexextension",
        }
    }

    /// Every extension id the host accepts. More than one because the store build, the beta and
    /// the unpacked development build are different extensions.
    fn extension_ids(self) -> &'static [&'static str] {
        match self {
            AgentBrowser::Claude => {
                &["fcoeoabgfenejglbffodgkkbkcdhcgfn", "dihbgbndebgnbjfmelmegjepbnkhlgni", "dngcpimnedloihjnnfngkgjoidhnaolf"]
            }
            AgentBrowser::Codex => &["hehggadaopoacecdllhhajmbjkdcmajg", "odlomjlbamekndcpllcnffbgeohgkmjh"],
        }
    }

    /// Where someone installs it by hand.
    pub fn store_url(self) -> &'static str {
        match self {
            AgentBrowser::Claude => "https://chromewebstore.google.com/detail/claude/fcoeoabgfenejglbffodgkkbkcdhcgfn",
            AgentBrowser::Codex => "https://chromewebstore.google.com/detail/hehggadaopoacecdllhhajmbjkdcmajg",
        }
    }
}

/// A Chromium browser Agentty knows where to look for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Browser {
    pub id: &'static str,
    pub name: &'static str,
    /// Folder under the platform's application-support directory.
    folder: &'static str,
}

/// The browsers both extensions are published for. Firefox and Safari are left out: neither
/// extension exists for them.
pub const BROWSERS: &[Browser] = &[
    Browser { id: "chrome", name: "Google Chrome", folder: if cfg!(target_os = "linux") { "google-chrome" } else { "Google/Chrome" } },
    Browser { id: "edge", name: "Microsoft Edge", folder: if cfg!(target_os = "linux") { "microsoft-edge" } else { "Microsoft Edge" } },
    Browser { id: "brave", name: "Brave", folder: "BraveSoftware/Brave-Browser" },
    Browser { id: "vivaldi", name: "Vivaldi", folder: if cfg!(target_os = "linux") { "vivaldi" } else { "Vivaldi" } },
    Browser {
        id: "opera",
        name: "Opera",
        folder: if cfg!(target_os = "macos") { "com.operasoftware.Opera" } else { "Opera Software/Opera Stable" },
    },
    Browser { id: "arc", name: "Arc", folder: "Arc" },
];

/// What is ready in one browser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserStatus {
    pub id: &'static str,
    pub name: &'static str,
    /// The browser itself is installed (its configuration folder exists).
    pub present: bool,
    /// Agents whose extension is installed in one of its profiles.
    pub extensions: Vec<&'static str>,
    /// Agents whose CLI has written its native messaging host for this browser.
    pub hosts: Vec<&'static str>,
}

impl BrowserStatus {
    /// Both halves are there for `agent`: the extension to act, and the host to be reached.
    pub fn ready_for(&self, agent: AgentBrowser) -> bool {
        self.extensions.contains(&agent.id()) && self.hosts.contains(&agent.id())
    }

    /// Any agent can work here.
    pub fn ready(&self) -> bool {
        AgentBrowser::ALL.iter().any(|agent| self.ready_for(*agent))
    }
}

/// Where a browser keeps its configuration on this platform.
fn support_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    #[cfg(target_os = "macos")]
    return home.map(|home| home.join("Library/Application Support"));
    #[cfg(target_os = "linux")]
    return std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| home.map(|home| home.join(".config")));
    #[cfg(target_os = "windows")]
    {
        let _ = home;
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    return home;
}

/// The folders a browser's profiles live in. Windows and some browsers put them under
/// `User Data`; the rest keep them beside the configuration itself.
fn profile_roots(root: &Path) -> Vec<PathBuf> {
    let with_user_data = root.join("User Data");
    if with_user_data.is_dir() {
        vec![with_user_data]
    } else {
        vec![root.to_path_buf()]
    }
}

/// Whether any profile of this browser holds one of `ids`.
fn has_extension(root: &Path, ids: &[&str]) -> bool {
    for base in profile_roots(root) {
        let Ok(entries) = std::fs::read_dir(&base) else { continue };
        for profile in entries.flatten() {
            let extensions = profile.path().join("Extensions");
            if !extensions.is_dir() {
                continue;
            }
            if ids.iter().any(|id| extensions.join(id).is_dir()) {
                return true;
            }
        }
    }
    false
}

/// What every browser Agentty knows about has, in the order they are listed.
///
/// A browser that is not installed is still reported, so a settings page can say so rather than
/// leave a gap where someone expected their browser to be.
pub fn browsers() -> Vec<BrowserStatus> {
    let Some(support) = support_dir() else {
        return BROWSERS
            .iter()
            .map(|b| BrowserStatus { id: b.id, name: b.name, present: false, extensions: Vec::new(), hosts: Vec::new() })
            .collect();
    };
    BROWSERS
        .iter()
        .map(|browser| {
            let root = support.join(browser.folder);
            let present = root.is_dir();
            let mut extensions = Vec::new();
            let mut hosts = Vec::new();
            if present {
                for agent in AgentBrowser::ALL {
                    if has_extension(&root, agent.extension_ids()) {
                        extensions.push(agent.id());
                    }
                    // Windows keeps these in the registry instead of a file; there the extension
                    // alone is what can be seen from here.
                    let host = root.join("NativeMessagingHosts").join(format!("{}.json", agent.host()));
                    if host.is_file() || (cfg!(target_os = "windows") && extensions.contains(&agent.id())) {
                        hosts.push(agent.id());
                    }
                }
            }
            BrowserStatus { id: browser.id, name: browser.name, present, extensions, hosts }
        })
        .collect()
}

/// What each browser is called to the operating system, for opening it. Only macOS opens a
/// browser by its application name — the other platforms have an executable name instead — but the
/// mapping is tested everywhere, so it is kept everywhere.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn launch_name(id: &str) -> &'static str {
    match id {
        "edge" => "Microsoft Edge",
        "brave" => "Brave Browser",
        "vivaldi" => "Vivaldi",
        "opera" => "Opera",
        "arc" => "Arc",
        _ => "Google Chrome",
    }
}

/// Opens `url` in `browser`, starting it if it is not running.
///
/// The extension an agent drives lives inside the browser, so a browser that is not running has
/// nothing to drive — the agent's tools simply find no browser and the page never appears. Opening
/// the address here also means the user sees something immediately, before the agent has read
/// anything.
pub fn open_in(browser: &str, url: &str) {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("/usr/bin/open").args(["-a", launch_name(browser), url]).spawn();
    }
    #[cfg(target_os = "windows")]
    {
        // `start` needs a window title argument before the program; the empty string is that.
        let exe = match browser {
            "edge" => "msedge",
            "brave" => "brave",
            "vivaldi" => "vivaldi",
            "opera" => "opera",
            _ => "chrome",
        };
        let _ = crate::process::command("cmd").args(["/C", "start", "", exe, url]).spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let exe = match browser {
            "edge" => "microsoft-edge",
            "brave" => "brave-browser",
            "vivaldi" => "vivaldi",
            "opera" => "opera",
            _ => "google-chrome",
        };
        let _ = std::process::Command::new(exe).arg(url).spawn();
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = (browser, url);
    }
}

/// The browser to work in when the user has not chosen one: the first that is ready, else the
/// first that is installed, else Chrome — which is what the install instructions name.
pub fn preferred(statuses: &[BrowserStatus]) -> &'static str {
    statuses.iter().find(|s| s.ready()).or_else(|| statuses.iter().find(|s| s.present)).map_or("chrome", |s| s.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both halves are needed: an extension with no host cannot reach the agent, and a host with
    /// no extension has nothing to talk to.
    #[test]
    fn ready_needs_the_extension_and_the_host() {
        let both = BrowserStatus { id: "chrome", name: "Google Chrome", present: true, extensions: vec!["claude"], hosts: vec!["claude"] };
        assert!(both.ready_for(AgentBrowser::Claude));
        assert!(both.ready());
        assert!(!both.ready_for(AgentBrowser::Codex));

        let half = BrowserStatus { id: "edge", name: "Microsoft Edge", present: true, extensions: Vec::new(), hosts: vec!["codex"] };
        assert!(!half.ready_for(AgentBrowser::Codex), "a host without the extension is not ready");
        assert!(!half.ready());
    }

    #[test]
    fn the_preferred_browser_is_the_one_that_can_work() {
        let none = BrowserStatus { id: "chrome", name: "Google Chrome", present: false, extensions: vec![], hosts: vec![] };
        let installed = BrowserStatus { id: "brave", name: "Brave", present: true, extensions: vec![], hosts: vec![] };
        let ready = BrowserStatus { id: "edge", name: "Microsoft Edge", present: true, extensions: vec!["claude"], hosts: vec!["claude"] };

        assert_eq!(preferred(&[none.clone(), installed.clone(), ready.clone()]), "edge", "ready beats merely installed");
        assert_eq!(preferred(&[none.clone(), installed.clone()]), "brave", "installed beats absent");
        assert_eq!(preferred(&[none]), "chrome", "nothing installed falls back to the one the instructions name");
        assert_eq!(preferred(&[]), "chrome");
    }

    /// Every id is a real extension id: 32 lowercase letters a–p, the alphabet Chrome uses.
    #[test]
    fn extension_ids_have_the_shape_chrome_gives_them() {
        for agent in AgentBrowser::ALL {
            let ids = agent.extension_ids();
            assert!(!ids.is_empty(), "{} has no extension id", agent.id());
            for id in ids {
                assert_eq!(id.len(), 32, "{id} is not 32 characters");
                assert!(id.chars().all(|c| ('a'..='p').contains(&c)), "{id} is not in Chrome's id alphabet");
            }
            assert!(agent.store_url().starts_with("https://chromewebstore.google.com/"), "{} has no install page", agent.id());
        }
    }

    #[test]
    fn every_browser_has_a_name_to_open_it_by() {
        for browser in BROWSERS {
            let name = launch_name(browser.id);
            assert!(!name.is_empty(), "{} cannot be opened", browser.id);
        }
        assert_eq!(launch_name("chrome"), "Google Chrome");
        assert_eq!(launch_name("brave"), "Brave Browser", "the application is not called Brave");
        assert_eq!(launch_name("something else"), "Google Chrome", "an unknown id falls back to the common one");
    }

    /// Reading the machine must not panic or depend on a browser being installed.
    #[test]
    fn looking_at_this_machine_answers_for_every_browser() {
        let statuses = browsers();
        assert_eq!(statuses.len(), BROWSERS.len());
        for status in &statuses {
            assert!(BROWSERS.iter().any(|b| b.id == status.id));
            if !status.present {
                assert!(status.extensions.is_empty() && status.hosts.is_empty(), "{} is absent but reports halves", status.id);
            }
        }
    }
}
