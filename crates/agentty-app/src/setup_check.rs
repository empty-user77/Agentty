//! System check (Windows and Linux): the tools Agentty relies on there, whether they are
//! installed, and the command that installs each one (winget / the distribution's package
//! manager / the vendor's installer). Shown in Settings → System check and on first launch when
//! something important is missing. macOS has no such page: everything it needs ships with it.

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    /// A core feature doesn't work without it (only Windows has such tools).
    #[cfg_attr(not(windows), allow(dead_code))]
    Required,
    Recommended,
    Optional,
}

impl Need {
    pub fn label_key(self) -> &'static str {
        match self {
            Need::Required => "system.required",
            Need::Recommended => "system.recommended",
            Need::Optional => "system.optional",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tool {
    pub id: &'static str,
    pub name: &'static str,
    pub need: Need,
    /// i18n key: what Agentty uses it for.
    pub purpose: &'static str,
    /// Command line that installs it, run in a new terminal tab (PowerShell on Windows).
    pub install: Option<String>,
    /// Where to read more / download manually.
    pub guide: &'static str,
    /// Version or location when installed.
    pub found: Option<String>,
}

impl Tool {
    pub fn missing(&self) -> bool {
        self.found.is_none()
    }
}

/// Anything required or recommended is missing (first-launch prompt).
pub fn needs_attention(tools: &[Tool]) -> bool {
    tools.iter().any(|t| t.missing() && t.need != Need::Optional)
}

/// First line of `<program> --version`, when the program is on PATH.
fn version_of(name: &str) -> Option<String> {
    let path = agentty_bridge::process::which(name)?;
    let output = agentty_bridge::process::command(&path)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok();
    let version =
        output.map(|o| String::from_utf8_lossy(&o.stdout).lines().next().unwrap_or_default().trim().to_string()).unwrap_or_default();
    Some(if version.is_empty() { path.display().to_string() } else { version })
}

fn path_of(name: &str) -> Option<String> {
    agentty_bridge::process::which(name).map(|p| p.display().to_string())
}

/// Runs every check for this platform (spawns `--version` probes; call off the UI thread).
pub fn check() -> Vec<Tool> {
    #[cfg(windows)]
    return windows_tools();
    #[cfg(not(windows))]
    linux_tools(&PackageManager::detect())
}

#[cfg(windows)]
fn windows_tools() -> Vec<Tool> {
    let winget =
        |id: &str| Some(format!("winget install --id {id} -e --source winget --accept-package-agreements --accept-source-agreements"));
    vec![
        Tool {
            id: "windows",
            name: "Windows 10 1809+ (ConPTY)",
            need: Need::Required,
            purpose: "system.purpose.windows",
            install: None,
            guide: "https://support.microsoft.com/windows/update-windows-3c5ae7fc-9fb6-9af1-1984-b5e0412c556a",
            found: windows_build().filter(|build| *build >= 17763).map(|build| format!("build {build}")),
        },
        Tool {
            id: "git",
            name: "Git for Windows (Git Bash)",
            need: Need::Required,
            purpose: "system.purpose.git_bash",
            install: winget("Git.Git"),
            guide: "https://git-scm.com/download/win",
            found: agentty_bridge::process::git_bash().map(|p| p.display().to_string()),
        },
        Tool {
            id: "claude",
            name: "Claude Code",
            need: Need::Recommended,
            purpose: "system.purpose.claude",
            install: Some("irm https://claude.ai/install.ps1 | iex".into()),
            guide: "https://docs.claude.com/en/docs/claude-code/setup",
            found: version_of("claude"),
        },
        Tool {
            id: "node",
            name: "Node.js LTS",
            need: Need::Recommended,
            purpose: "system.purpose.node",
            install: winget("OpenJS.NodeJS.LTS"),
            guide: "https://nodejs.org/",
            found: version_of("node"),
        },
        Tool {
            id: "codex",
            name: "Codex CLI",
            need: Need::Optional,
            purpose: "system.purpose.codex",
            install: Some("npm install -g @openai/codex".into()),
            guide: "https://github.com/openai/codex",
            found: version_of("codex"),
        },
        Tool {
            id: "pwsh",
            name: "PowerShell 7",
            need: Need::Recommended,
            purpose: "system.purpose.pwsh",
            install: winget("Microsoft.PowerShell"),
            guide: "https://aka.ms/powershell",
            found: version_of("pwsh"),
        },
        Tool {
            id: "winget",
            name: "WinGet (App Installer)",
            need: Need::Recommended,
            purpose: "system.purpose.winget",
            install: None,
            guide: "https://aka.ms/getwinget",
            found: path_of("winget"),
        },
    ]
}

/// Windows build number (`10.0.<build>.<rev>`).
#[cfg(windows)]
fn windows_build() -> Option<u32> {
    parse_windows_build(&crate::platform::os_version())
}

#[cfg_attr(not(windows), allow(dead_code))]
fn parse_windows_build(version: &str) -> Option<u32> {
    version.split(|c: char| !c.is_ascii_digit() && c != '.').find(|part| part.matches('.').count() >= 2)?.split('.').nth(2)?.parse().ok()
}

/// The distribution's package manager, for install commands on Linux.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(windows, allow(dead_code))]
pub enum PackageManager {
    Apt,
    Dnf,
    Pacman,
    Zypper,
    Unknown,
}

#[cfg_attr(windows, allow(dead_code))]
impl PackageManager {
    fn detect() -> Self {
        let has = |name: &str| agentty_bridge::process::which(name).is_some();
        if has("apt-get") {
            Self::Apt
        } else if has("dnf") {
            Self::Dnf
        } else if has("pacman") {
            Self::Pacman
        } else if has("zypper") {
            Self::Zypper
        } else {
            Self::Unknown
        }
    }

    /// Install command for a package, by its name per package manager (apt, dnf, pacman, zypper).
    fn install(self, names: [&str; 4]) -> Option<String> {
        Some(match self {
            Self::Apt => format!("sudo apt-get install -y {}", names[0]),
            Self::Dnf => format!("sudo dnf install -y {}", names[1]),
            Self::Pacman => format!("sudo pacman -S --needed {}", names[2]),
            Self::Zypper => format!("sudo zypper install -y {}", names[3]),
            Self::Unknown => return None,
        })
    }
}

#[cfg_attr(windows, allow(dead_code))]
fn linux_tools(pm: &PackageManager) -> Vec<Tool> {
    let pm = *pm;
    vec![
        Tool {
            id: "claude",
            name: "Claude Code",
            need: Need::Recommended,
            purpose: "system.purpose.claude",
            install: Some("curl -fsSL https://claude.ai/install.sh | bash".into()),
            guide: "https://docs.claude.com/en/docs/claude-code/setup",
            found: version_of("claude"),
        },
        Tool {
            id: "node",
            name: "Node.js",
            need: Need::Recommended,
            purpose: "system.purpose.node",
            install: pm.install(["nodejs npm", "nodejs npm", "nodejs npm", "nodejs npm"]),
            guide: "https://nodejs.org/en/download/package-manager",
            found: version_of("node"),
        },
        Tool {
            id: "codex",
            name: "Codex CLI",
            need: Need::Optional,
            purpose: "system.purpose.codex",
            install: Some("npm install -g @openai/codex".into()),
            guide: "https://github.com/openai/codex",
            found: version_of("codex"),
        },
        Tool {
            id: "git",
            name: "Git",
            need: Need::Recommended,
            purpose: "system.purpose.git",
            install: pm.install(["git", "git", "git", "git"]),
            guide: "https://git-scm.com/download/linux",
            found: version_of("git"),
        },
        Tool {
            id: "secret-tool",
            name: "secret-tool (libsecret)",
            need: Need::Recommended,
            purpose: "system.purpose.secret_tool",
            install: pm.install(["libsecret-tools", "libsecret", "libsecret", "libsecret-tools"]),
            guide: "https://wiki.gnome.org/Projects/Libsecret",
            found: path_of("secret-tool"),
        },
        Tool {
            id: "xdg-open",
            name: "xdg-utils",
            need: Need::Recommended,
            purpose: "system.purpose.xdg_open",
            install: pm.install(["xdg-utils", "xdg-utils", "xdg-utils", "xdg-utils"]),
            guide: "https://www.freedesktop.org/wiki/Software/xdg-utils/",
            found: path_of("xdg-open"),
        },
        Tool {
            id: "notify-send",
            name: "notify-send (libnotify)",
            need: Need::Optional,
            purpose: "system.purpose.notify_send",
            install: pm.install(["libnotify-bin", "libnotify", "libnotify", "libnotify-tools"]),
            guide: "https://gitlab.gnome.org/GNOME/libnotify",
            found: path_of("notify-send"),
        },
        Tool {
            id: "lsof",
            name: "lsof",
            need: Need::Optional,
            purpose: "system.purpose.lsof",
            install: pm.install(["lsof", "lsof", "lsof", "lsof"]),
            guide: "https://github.com/lsof-org/lsof",
            found: path_of("lsof"),
        },
    ]
}

/// Folder new install tabs start in.
pub fn install_dir() -> PathBuf {
    crate::launch::home_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_windows_builds() {
        assert_eq!(parse_windows_build("Windows 10.0.19045.4529"), Some(19045));
        assert_eq!(parse_windows_build("Windows 10.0.17134.1"), Some(17134));
        assert_eq!(parse_windows_build("Windows"), None);
    }

    #[test]
    fn package_manager_commands() {
        let names = ["libsecret-tools", "libsecret", "libsecret", "libsecret-tools"];
        assert_eq!(PackageManager::Apt.install(names).as_deref(), Some("sudo apt-get install -y libsecret-tools"));
        assert_eq!(PackageManager::Pacman.install(names).as_deref(), Some("sudo pacman -S --needed libsecret"));
        assert_eq!(PackageManager::Unknown.install(names), None);
    }

    #[test]
    fn attention_only_for_important_tools() {
        let tool = |need, found: Option<&str>| Tool {
            id: "x",
            name: "x",
            need,
            purpose: "p",
            install: None,
            guide: "",
            found: found.map(str::to_string),
        };
        assert!(!needs_attention(&[tool(Need::Optional, None), tool(Need::Required, Some("ok"))]));
        assert!(needs_attention(&[tool(Need::Recommended, None)]));
        // Every Linux entry has a guide and an i18n purpose.
        for tool in linux_tools(&PackageManager::Apt) {
            assert!(tool.guide.starts_with("https://") && tool.purpose.starts_with("system.purpose."), "{}", tool.id);
            assert!(tool.install.is_some(), "{}", tool.id);
        }
    }
}
