//! What differs between macOS, Windows and Linux, in one place.
//!
//! macOS keeps its native modules (`native`, `status_item`, `webview`, `file_drop`,
//! `notifications`: AppKit, WebKit, UserNotifications). Other platforms compile the same module
//! names from `platform/fallback/`, with the same API, so call sites need no `cfg`. The pieces
//! that never depended on AppKit live here and are shared by both.

pub mod app_nap;
pub mod drops;
pub mod frame;
pub mod system_proxy;
pub mod tray;
pub mod url;
pub mod video;
pub mod wakelock;

use std::path::Path;

/// The menu bar status item (NSStatusItem); elsewhere closing the last window quits.
pub const HAS_STATUS_ITEM: bool = cfg!(target_os = "macos");
/// The in-app browser (WKWebView); elsewhere links open in the default browser.
pub const HAS_WEBVIEW: bool = cfg!(target_os = "macos");
/// Mini mode folds the window with AppKit frame animations.
pub const HAS_MINI_MODE: bool = cfg!(target_os = "macos");

/// Raises the open-file limit for this process as far as the system allows.
///
/// An app launched from the Dock or Finder inherits a soft limit of 256 descriptors on macOS.
/// Every terminal pane holds a PTY and its event loop, and webviews, git commands and sockets take
/// more, so opening and closing panes for a while runs the process out of descriptors and new
/// terminals fail with "failed to open PTY".
pub fn raise_file_limit() {
    #[cfg(unix)]
    unsafe {
        let mut limit = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) != 0 {
            return;
        }
        // macOS refuses anything above OPEN_MAX even when the hard limit says "unlimited".
        let ceiling: libc::rlim_t = 24_576;
        let wanted = if limit.rlim_max == libc::RLIM_INFINITY { ceiling } else { limit.rlim_max.min(ceiling) };
        if wanted <= limit.rlim_cur {
            return;
        }
        limit.rlim_cur = wanted;
        if libc::setrlimit(libc::RLIMIT_NOFILE, &limit) != 0 {
            eprintln!("agentty: could not raise the open-file limit past {}", limit.rlim_cur);
        }
    }
}

/// Opens a folder in the file manager (Finder, Explorer, the desktop's default).
pub fn open_folder(path: &Path) {
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("/usr/bin/open");
    #[cfg(windows)]
    let mut command = agentty_bridge::process::command("explorer.exe");
    #[cfg(not(any(target_os = "macos", windows)))]
    let mut command = std::process::Command::new("xdg-open");
    let _ = command.arg(path).spawn();
}

/// Shows a file selected in the file manager, or opens a folder.
pub fn reveal(path: &Path) {
    if path.is_dir() {
        return open_folder(path);
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("/usr/bin/open").arg("-R").arg(path).spawn();
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = agentty_bridge::process::command("explorer.exe").raw_arg(explorer_select_argument(path)).spawn();
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        if let Some(parent) = path.parent() {
            open_folder(parent);
        }
    }
}

/// `/select,"<path>"` for Explorer, which wants it as one raw argument. Windows paths can't
/// contain `"`; trailing backslashes are doubled so they can't escape the closing quote.
#[cfg_attr(not(windows), allow(dead_code))]
fn explorer_select_argument(path: &Path) -> String {
    let text = path.display().to_string();
    let trailing = text.len() - text.trim_end_matches('\\').len();
    format!("/select,\"{text}{}\"", "\\".repeat(trailing))
}

/// Operating system and version for diagnostics, e.g. `15.6` on macOS (as before), `Windows 10.0`
/// or `Ubuntu 24.04`.
pub fn os_version() -> String {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("/usr/bin/sw_vers")
            .arg("-productVersion")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default()
    }
    #[cfg(windows)]
    {
        agentty_bridge::process::command("cmd")
            .args(["/C", "ver"])
            .output()
            .map(|o| {
                let text = String::from_utf8_lossy(&o.stdout);
                let version = text.split("Version").nth(1).unwrap_or("").trim().trim_end_matches(']').trim();
                format!("Windows {version}")
            })
            .unwrap_or_else(|_| "Windows".into())
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let release = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
        os_release_name(&release).unwrap_or_else(|| std::env::consts::OS.to_string())
    }
}

/// `NAME VERSION_ID` from `/etc/os-release`.
#[cfg_attr(any(target_os = "macos", windows), allow(dead_code))]
fn os_release_name(text: &str) -> Option<String> {
    let field =
        |key: &str| text.lines().find_map(|line| line.strip_prefix(key)?.strip_prefix('=').map(|v| v.trim().trim_matches('"').to_string()));
    let name = field("NAME")?;
    Some(match field("VERSION_ID") {
        Some(version) => format!("{name} {version}"),
        None => name,
    })
}

/// The user's preferred UI language as a BCP 47-ish code (`ko-KR`, `en_US.UTF-8`, …).
pub fn preferred_language() -> String {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("defaults")
            .args(["read", "-g", "AppleLanguages"])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .and_then(|s| s.lines().nth(1).map(|l| l.trim().trim_matches(|c| c == '"' || c == ',').to_string()))
            .or_else(|| std::env::var("LANG").ok())
            .unwrap_or_default()
    }
    #[cfg(windows)]
    {
        windows_locale().or_else(|| std::env::var("LANG").ok()).unwrap_or_default()
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        ["LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"]
            .iter()
            .filter_map(|key| std::env::var(key).ok())
            .find(|v| !v.is_empty() && v != "C" && v != "POSIX")
            .unwrap_or_default()
    }
}

/// The system's region setting as a locale name (`ko_KR`, `ko-KR`, `ko_KR.UTF-8`), for the
/// country in usage statistics. The setting, not where the computer is.
pub fn region_locale() -> String {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("defaults")
            .args(["read", "-g", "AppleLocale"])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| std::env::var("LANG").ok())
            .unwrap_or_default()
    }
    #[cfg(windows)]
    {
        windows_locale().unwrap_or_default()
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        ["LC_ALL", "LC_CTYPE", "LANG"]
            .iter()
            .filter_map(|key| std::env::var(key).ok())
            .find(|v| !v.is_empty() && v != "C" && v != "POSIX")
            .unwrap_or_default()
    }
}

#[cfg(windows)]
fn windows_locale() -> Option<String> {
    use windows_sys::Win32::Globalization::GetUserDefaultLocaleName;
    let mut buffer = [0u16; 85];
    // SAFETY: the buffer is LOCALE_NAME_MAX_LENGTH (85) wide characters.
    let len = unsafe { GetUserDefaultLocaleName(buffer.as_mut_ptr(), buffer.len() as i32) };
    (len > 1).then(|| String::from_utf16_lossy(&buffer[..len as usize - 1]))
}

#[cfg(test)]
mod tests {
    #[test]
    fn quotes_explorer_selection() {
        let arg = |p: &str| super::explorer_select_argument(std::path::Path::new(p));
        assert_eq!(arg(r"C:\Users\me\a b.txt"), r#"/select,"C:\Users\me\a b.txt""#);
        assert_eq!(arg(r"D:\"), r#"/select,"D:\\""#);
    }

    #[test]
    fn reads_os_release() {
        let text = "PRETTY_NAME=\"Ubuntu 24.04 LTS\"\nNAME=\"Ubuntu\"\nVERSION_ID=\"24.04\"\n";
        assert_eq!(super::os_release_name(text).as_deref(), Some("Ubuntu 24.04"));
        assert_eq!(super::os_release_name("NAME=Arch Linux\n").as_deref(), Some("Arch Linux"));
        assert_eq!(super::os_release_name(""), None);
    }
}
