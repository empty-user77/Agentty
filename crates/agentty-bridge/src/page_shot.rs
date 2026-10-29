//! A picture of a web page, taken by a Chromium-family browser installed here (Chrome, Edge,
//! Brave, Chromium) running headless with a new, empty profile: none of the user's cookies,
//! sign-ins or extensions go with it, and the profile is deleted afterwards. Without such a
//! browser there is no picture, and the agents open the page with `agentty browser` instead.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(45);
/// A desktop width, and tall enough for most of a long page.
const WINDOW: &str = "--window-size=1440,3000";

/// The browsers tried, first found first.
fn browser() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let apps = [
            "Google Chrome.app/Contents/MacOS/Google Chrome",
            "Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            "Brave Browser.app/Contents/MacOS/Brave Browser",
            "Chromium.app/Contents/MacOS/Chromium",
        ];
        let roots = [PathBuf::from("/Applications"), crate::fsutil::home().join("Applications")];
        roots.iter().flat_map(|root| apps.iter().map(move |app| root.join(app))).find(|path| path.is_file())
    }
    #[cfg(not(target_os = "macos"))]
    {
        ["google-chrome", "google-chrome-stable", "chromium", "chromium-browser", "microsoft-edge", "brave-browser", "chrome", "msedge"]
            .iter()
            .find_map(|name| crate::process::which(name))
    }
}

/// A PNG of `url` (http or https only), or `None` when no browser here can take one.
pub fn capture(url: &str) -> Option<Vec<u8>> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return None;
    }
    let browser = browser()?;
    let dir = std::env::temp_dir().join(format!("agentty-shot-{}-{}", std::process::id(), nanos()));
    crate::fsutil::create_private_dir(&dir).ok()?;
    let out = dir.join("page.png");
    let mut child = crate::process::command(&browser)
        .args([
            "--headless=new",
            "--disable-gpu",
            "--hide-scrollbars",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-extensions",
            "--mute-audio",
            WINDOW,
        ])
        .arg(format!("--user-data-dir={}", dir.join("profile").display()))
        .arg(format!("--screenshot={}", out.display()))
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < TIMEOUT => std::thread::sleep(Duration::from_millis(100)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
        }
    }
    let png = std::fs::read(&out).ok().filter(|bytes| bytes.starts_with(b"\x89PNG"));
    let _ = std::fs::remove_dir_all(&dir);
    png
}

fn nanos() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0)
}
