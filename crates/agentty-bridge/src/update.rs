//! Update checks against the public release channel (GitHub Releases of `agentty-releases`) and a
//! verified download of the file that installs the new version here: the DMG on macOS, the setup
//! program on Windows. Linux has none (the .deb / .rpm go through the package manager), so a
//! release there only links to its page. Installing (mount, signature check, swap, relaunch; or
//! running the setup program) is done by the app; everything here is platform-neutral and testable.

use anyhow::{bail, ensure, Context, Result};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const RELEASE_REPO: &str = "empty-user77/agentty-releases";
/// Where every version can be downloaded by hand.
pub const RELEASES_PAGE: &str = "https://github.com/empty-user77/agentty-releases/releases";
const MAX_DOWNLOAD: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub page_url: String,
    /// The file that installs this release on this system (see [`installer_for`]), when it has one.
    pub installer_url: Option<String>,
    pub installer_name: Option<String>,
    pub checksums_url: Option<String>,
}

/// `v1.2.3`, `1.2.3-beta` → (1, 2, 3).
pub fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let core = text.trim().trim_start_matches(['v', 'V']).split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>());
    let version = (parts.next()?.ok()?, parts.next().unwrap_or(Ok(0)).ok()?, parts.next().unwrap_or(Ok(0)).ok()?);
    Some(version)
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    matches!((parse_version(candidate), parse_version(current)), (Some(a), Some(b)) if a > b)
}

/// Only GitHub-hosted HTTPS assets are ever downloaded.
fn trusted_url(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|u| {
        u.scheme() == "https"
            && matches!(
                u.host_str(),
                Some("github.com" | "api.github.com" | "objects.githubusercontent.com" | "release-assets.githubusercontent.com")
            )
    })
}

fn arch_aliases(arch: &str) -> &'static [&'static str] {
    if arch == "aarch64" || arch == "arm64" {
        &["arm64", "aarch64"]
    } else {
        &["x86_64", "x64", "intel"]
    }
}

/// A release file name that is safe to create in the download folder on every platform.
fn plain_file_name(name: &str) -> bool {
    !name.is_empty() && !name.contains(['/', '\\', ':']) && !name.contains("..") && !name.chars().any(char::is_control)
}

/// Which of a release's files installs it on `os` (`std::env::consts::OS`) / `arch`: the DMG for the
/// architecture on macOS (or one without an architecture in its name), `…-windows-x64-setup.exe` on
/// Windows, and nothing elsewhere.
pub fn installer_for<'a>(names: &[&'a str], os: &str, arch: &str) -> Option<&'a str> {
    let aliases = arch_aliases(arch);
    let for_arch = |name: &str| aliases.iter().any(|a| name.contains(a));
    let found = match os {
        "macos" => names
            .iter()
            .find(|n| n.ends_with(".dmg") && for_arch(n))
            .or_else(|| names.iter().find(|n| n.ends_with(".dmg") && !n.contains("x86_64") && !n.contains("arm64"))),
        "windows" => names.iter().find(|n| n.contains("-windows-") && n.ends_with("-setup.exe") && for_arch(n)),
        _ => None,
    };
    found.copied().filter(|n| plain_file_name(n))
}

/// Picks the published (non-draft, non-prerelease) release and its installer for `os` / `arch`.
pub fn parse_release(json: &serde_json::Value, os: &str, arch: &str) -> Option<Release> {
    if json["draft"].as_bool() == Some(true) || json["prerelease"].as_bool() == Some(true) {
        return None;
    }
    let version = json["tag_name"].as_str().filter(|t| parse_version(t).is_some())?.trim_start_matches('v').to_string();
    // Only files served from GitHub over HTTPS count.
    let assets: Vec<(&str, &str)> = json["assets"]
        .as_array()
        .map(|assets| {
            assets
                .iter()
                .filter_map(|a| Some((a["name"].as_str()?, a["browser_download_url"].as_str().filter(|u| trusted_url(u))?)))
                .collect()
        })
        .unwrap_or_default();
    let names: Vec<&str> = assets.iter().map(|a| a.0).collect();
    let url_of = |name: &str| assets.iter().find(|a| a.0 == name).map(|a| a.1.to_string());
    let installer = installer_for(&names, os, arch);
    let checksums = names.iter().find(|n| n.ends_with("SHA256SUMS.txt"));
    Some(Release {
        version,
        notes: json["body"].as_str().unwrap_or_default().to_string(),
        page_url: json["html_url"].as_str().filter(|u| trusted_url(u)).unwrap_or(RELEASES_PAGE).to_string(),
        installer_url: installer.and_then(url_of),
        installer_name: installer.map(str::to_string),
        checksums_url: checksums.and_then(|n| url_of(n)),
    })
}

fn agent() -> ureq::Agent {
    crate::http::agent_builder().timeout_connect(Duration::from_secs(10)).timeout(Duration::from_secs(600)).build()
}

/// The newest published release, if it is newer than `current`.
///
/// Asks the GitHub API first. Unauthenticated API calls are limited per public IP address, which
/// is shared by everyone behind a company network, so when the API refuses (403/429) or cannot be
/// reached, the release is found through the plain github.com pages instead.
pub fn check(current: &str, os: &str, arch: &str) -> Result<Option<Release>> {
    let url = format!("https://api.github.com/repos/{RELEASE_REPO}/releases/latest");
    let response = agent()
        .get(&url)
        .set("Accept", "application/vnd.github+json")
        .set("User-Agent", &format!("Agentty/{current}"))
        .timeout(Duration::from_secs(20))
        .call();
    let api_error = match response {
        Ok(response) => {
            let json: serde_json::Value = response.into_json()?;
            return Ok(parse_release(&json, os, arch).filter(|r| is_newer(&r.version, current)));
        }
        // No published release yet.
        Err(ureq::Error::Status(404, _)) => return Ok(None),
        Err(ureq::Error::Status(code, response)) => {
            let message = response.into_json::<serde_json::Value>().ok().and_then(|j| j["message"].as_str().map(str::to_string));
            anyhow::anyhow!("{url}: status code {code}{}", message.map(|m| format!(" ({m})")).unwrap_or_default())
        }
        Err(err) => anyhow::Error::new(err),
    };
    check_via_web(current, os, arch)
        .map_err(|web| api_error.context(format!("fallback via github.com also failed: {web:#}")))
        .context("update check failed")
}

/// The latest release from github.com itself: `releases/latest` redirects to the release's tag, and
/// the checksum list names its files.
fn check_via_web(current: &str, os: &str, arch: &str) -> Result<Option<Release>> {
    let user_agent = format!("Agentty/{current}");
    let latest = format!("https://github.com/{RELEASE_REPO}/releases/latest");
    let response = match agent().get(&latest).set("User-Agent", &user_agent).timeout(Duration::from_secs(20)).call() {
        Ok(response) => response,
        Err(ureq::Error::Status(404, _)) => return Ok(None),
        Err(err) => return Err(err).context("github.com release page"),
    };
    let Some(tag) = tag_from_release_url(response.get_url()) else {
        // Redirected to the release list: nothing is published.
        return Ok(None);
    };
    let version = tag.trim_start_matches('v');
    if !is_newer(version, current) {
        return Ok(None);
    }
    let checksums_url = format!("https://github.com/{RELEASE_REPO}/releases/download/{tag}/Agentty-{version}-SHA256SUMS.txt");
    let listing = agent()
        .get(&checksums_url)
        .set("User-Agent", &user_agent)
        .timeout(Duration::from_secs(30))
        .call()
        .context("release checksums")?
        .into_string()?;
    release_from_checksums(&tag, &listing, os, arch).context("no DMG listed in the release checksums").map(Some)
}

/// `https://github.com/<repo>/releases/tag/v1.2.3` → `v1.2.3`.
fn tag_from_release_url(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    if parsed.host_str() != Some("github.com") {
        return None;
    }
    let tag = parsed.path().strip_prefix(&format!("/{RELEASE_REPO}/releases/tag/"))?;
    (!tag.contains('/') && parse_version(tag).is_some()).then(|| tag.to_string())
}

/// A release whose files are known from its `SHA256SUMS.txt` (no notes: the page link has them).
/// `None` on macOS when it lists no DMG (every macOS release has one); elsewhere a release without
/// an installer for this system still announces itself and links to its page.
fn release_from_checksums(tag: &str, listing: &str, os: &str, arch: &str) -> Option<Release> {
    let names: Vec<&str> = listing.lines().filter_map(|l| l.split_whitespace().nth(1)).map(|n| n.trim_start_matches('*')).collect();
    let installer = installer_for(&names, os, arch);
    if os == "macos" && installer.is_none() {
        return None;
    }
    let download = |name: &str| format!("https://github.com/{RELEASE_REPO}/releases/download/{tag}/{name}");
    let version = tag.trim_start_matches('v').to_string();
    Some(Release {
        notes: String::new(),
        page_url: format!("https://github.com/{RELEASE_REPO}/releases/tag/{tag}"),
        installer_url: installer.map(download),
        installer_name: installer.map(str::to_string),
        checksums_url: Some(download(&format!("Agentty-{version}-SHA256SUMS.txt"))),
        version,
    })
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Finds `name`'s digest in a `shasum -a 256` listing.
pub fn checksum_for(listing: &str, name: &str) -> Option<String> {
    listing.lines().find_map(|line| {
        let (digest, file) = line.split_once(char::is_whitespace)?;
        let file = file.trim().trim_start_matches('*');
        (file == name || file.ends_with(&format!("/{name}"))).then(|| digest.to_lowercase())
    })
}

fn copy_with_progress(
    reader: &mut dyn Read,
    writer: &mut dyn Write,
    total: Option<u64>,
    progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<u64> {
    let mut buffer = [0u8; 64 * 1024];
    let mut copied = 0u64;
    progress(0, total);
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err.into()),
        };
        writer.write_all(&buffer[..read])?;
        copied += read as u64;
        progress(copied, total);
    }
    Ok(copied)
}

/// A new, empty folder only this user can use (`0700` on Unix; the per-user temp folder on Windows)
/// for downloading an update. It never reuses an existing folder, so nothing can be planted in it.
pub fn private_download_dir(prefix: &str) -> Result<PathBuf> {
    let base = std::env::temp_dir();
    for attempt in 0..16u32 {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        let dir = base.join(format!("{prefix}-{}-{nanos:08x}{attempt}", std::process::id()));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match builder.create(&dir) {
            Ok(()) => return Ok(dir),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err).with_context(|| format!("create {}", dir.display())),
        }
    }
    bail!("could not create a download folder in {}", base.display())
}

/// Removes download folders that earlier updates left behind (a Windows setup program can't delete
/// itself while it runs). Only folders named `<prefix>-…` in the temp folder, never `keep`.
pub fn remove_old_download_dirs(prefix: &str, keep: &Path) {
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let ours = entry.file_name().to_str().is_some_and(|n| n.starts_with(&format!("{prefix}-")));
        if ours && path != keep && entry.file_type().is_ok_and(|t| t.is_dir()) {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

/// Downloads the release's installer into `dir` and verifies its SHA-256 against the published
/// checksums. `progress` receives the bytes written so far and the total size when the server
/// reports one.
pub fn download(release: &Release, dir: &Path, current: &str, progress: &mut dyn FnMut(u64, Option<u64>)) -> Result<PathBuf> {
    let (Some(url), Some(name)) = (&release.installer_url, &release.installer_name) else {
        bail!("this release has no installer for this system")
    };
    ensure!(trusted_url(url), "untrusted download location");
    ensure!(plain_file_name(name), "invalid asset name");
    std::fs::create_dir_all(dir)?;
    let path = dir.join(name);
    let response = agent().get(url).set("User-Agent", &format!("Agentty/{current}")).call().context("download failed")?;
    let total = response.header("Content-Length").and_then(|v| v.parse::<u64>().ok()).filter(|&n| n > 0);
    let mut reader = response.into_reader().take(MAX_DOWNLOAD + 1);
    let mut file = std::fs::File::create(&path)?;
    let copied = copy_with_progress(&mut reader, &mut file, total, progress)?;
    file.flush()?;
    ensure!(copied <= MAX_DOWNLOAD, "download is unexpectedly large");

    let sums_url = release.checksums_url.as_ref().context("release has no checksums; refusing to install")?;
    ensure!(trusted_url(sums_url), "untrusted checksum location");
    let listing = agent().get(sums_url).set("User-Agent", &format!("Agentty/{current}")).call()?.into_string()?;
    let expected = checksum_for(&listing, name).context("checksum missing for the installer")?;
    let actual = sha256_file(&path)?;
    if actual != expected {
        let _ = std::fs::remove_file(&path);
        bail!("checksum mismatch; the download was discarded");
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_fallback_reads_tag_and_checksums() {
        let tag = tag_from_release_url("https://github.com/empty-user77/agentty-releases/releases/tag/v0.1.6");
        assert_eq!(tag.as_deref(), Some("v0.1.6"));
        assert_eq!(tag_from_release_url("https://github.com/empty-user77/agentty-releases/releases"), None);
        assert_eq!(tag_from_release_url("https://example.com/empty-user77/agentty-releases/releases/tag/v0.1.6"), None);

        let listing = "aaa  Agentty-0.1.6-release20260918131331-arm64.dmg\nbbb  Agentty-0.1.6-arm64.zip\n";
        let release = release_from_checksums("v0.1.6", listing, "macos", "aarch64").unwrap();
        assert_eq!(release.version, "0.1.6");
        assert_eq!(release.installer_name.as_deref(), Some("Agentty-0.1.6-release20260918131331-arm64.dmg"));
        let dmg_url = release.installer_url.unwrap();
        assert!(trusted_url(&dmg_url));
        assert_eq!(
            dmg_url,
            "https://github.com/empty-user77/agentty-releases/releases/download/v0.1.6/Agentty-0.1.6-release20260918131331-arm64.dmg"
        );
        assert_eq!(
            release.checksums_url.as_deref(),
            Some("https://github.com/empty-user77/agentty-releases/releases/download/v0.1.6/Agentty-0.1.6-SHA256SUMS.txt")
        );
        assert!(release_from_checksums("v0.1.6", "aaa  Agentty-0.1.6-arm64.zip\n", "macos", "aarch64").is_none());
        assert!(release_from_checksums("v0.1.6", "aaa  ../evil-arm64.dmg\n", "macos", "aarch64").is_none());
    }

    const FULL_LISTING: &str = "aaa  Agentty-0.2.0-release20261001120000-arm64.dmg\n\
        bbb  Agentty-0.2.0-arm64.zip\n\
        ccc  Agentty-0.2.0-windows-x64-setup.exe\n\
        ddd  Agentty-0.2.0-windows-x64-setup.zip\n\
        eee  Agentty-0.2.0-linux-amd64.deb\n\
        fff  Agentty-0.2.0-linux-x86_64.rpm\n";

    #[test]
    fn web_fallback_picks_the_installer_per_platform() {
        let windows = release_from_checksums("v0.2.0", FULL_LISTING, "windows", "x86_64").unwrap();
        assert_eq!(windows.installer_name.as_deref(), Some("Agentty-0.2.0-windows-x64-setup.exe"));
        assert_eq!(
            windows.installer_url.as_deref(),
            Some("https://github.com/empty-user77/agentty-releases/releases/download/v0.2.0/Agentty-0.2.0-windows-x64-setup.exe")
        );
        let mac = release_from_checksums("v0.2.0", FULL_LISTING, "macos", "aarch64").unwrap();
        assert_eq!(mac.installer_name.as_deref(), Some("Agentty-0.2.0-release20261001120000-arm64.dmg"));

        // Linux installs through the package manager: the release is announced with its page only.
        let linux = release_from_checksums("v0.2.0", FULL_LISTING, "linux", "x86_64").unwrap();
        assert_eq!(linux.version, "0.2.0");
        assert_eq!(linux.installer_url, None);
        assert_eq!(linux.page_url, "https://github.com/empty-user77/agentty-releases/releases/tag/v0.2.0");

        // A release without a Windows installer (older ones) still announces itself on Windows.
        let mac_only = "aaa  Agentty-0.1.6-release1-arm64.dmg\nbbb  Agentty-0.1.6-arm64.zip\n";
        let windows = release_from_checksums("v0.1.6", mac_only, "windows", "x86_64").unwrap();
        assert_eq!(windows.installer_name, None);
    }

    #[test]
    fn installer_names_are_checked() {
        let names: Vec<&str> = FULL_LISTING.lines().filter_map(|l| l.split_whitespace().nth(1)).collect();
        assert_eq!(installer_for(&names, "windows", "x86_64"), Some("Agentty-0.2.0-windows-x64-setup.exe"));
        // No ARM64 installer for Windows, and never the zip or a Linux package.
        assert_eq!(installer_for(&names, "windows", "aarch64"), None);
        assert_eq!(installer_for(&names, "linux", "x86_64"), None);
        assert_eq!(installer_for(&names, "freebsd", "x86_64"), None);
        // Names that would leave the download folder on Windows are refused.
        assert_eq!(installer_for(&["..\\evil-windows-x64-setup.exe"], "windows", "x86_64"), None);
        assert_eq!(installer_for(&["C:\\evil-windows-x64-setup.exe"], "windows", "x86_64"), None);
        assert_eq!(installer_for(&["a\\b-windows-x64-setup.exe"], "windows", "x86_64"), None);
    }

    #[test]
    fn download_dirs_are_new_and_private() {
        let first = private_download_dir("agentty-update-test").unwrap();
        let second = private_download_dir("agentty-update-test").unwrap();
        assert_ne!(first, second);
        assert!(std::fs::read_dir(&first).unwrap().next().is_none());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&first).unwrap().permissions().mode() & 0o777, 0o700);
        }
        remove_old_download_dirs("agentty-update-test", &second);
        assert!(!first.exists());
        assert!(second.exists());
        let _ = std::fs::remove_dir_all(&second);
    }

    #[test]
    #[ignore = "needs network"]
    fn web_fallback_finds_the_published_release() {
        let release = check_via_web("0.0.1", "macos", "aarch64").unwrap().expect("a published release");
        assert!(release.installer_url.is_some_and(|u| u.ends_with("-arm64.dmg")));
    }

    #[test]
    fn download_copy_reports_progress() {
        let data = vec![7u8; 200 * 1024];
        let mut out = Vec::new();
        let mut seen = Vec::new();
        let copied =
            copy_with_progress(&mut data.as_slice(), &mut out, Some(data.len() as u64), &mut |done, total| seen.push((done, total)))
                .unwrap();
        assert_eq!(copied, data.len() as u64);
        assert_eq!(out, data);
        assert_eq!(seen.first(), Some(&(0, Some(data.len() as u64))));
        assert_eq!(seen.last(), Some(&(data.len() as u64, Some(data.len() as u64))));
        assert!(seen.windows(2).all(|w| w[0].0 <= w[1].0));
    }

    #[test]
    fn compares_versions() {
        assert!(is_newer("v0.2.0", "0.1.9"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("garbage", "0.1.0"));
        assert_eq!(parse_version("v1.4"), Some((1, 4, 0)));
    }

    #[test]
    fn picks_published_dmg_for_arch() {
        let json = serde_json::json!({
            "tag_name": "v0.2.0", "draft": false, "prerelease": false, "body": "Notes",
            "html_url": "https://github.com/empty-user77/agentty-releases/releases/tag/v0.2.0",
            "assets": [
                { "name": "Agentty-0.2.0-arm64.zip", "browser_download_url": "https://github.com/x/y/releases/download/v0.2.0/Agentty-0.2.0-arm64.zip" },
                { "name": "Agentty-0.2.0-release1-arm64.dmg", "browser_download_url": "https://github.com/x/y/releases/download/v0.2.0/Agentty-0.2.0-release1-arm64.dmg" },
                { "name": "Agentty-0.2.0-SHA256SUMS.txt", "browser_download_url": "https://github.com/x/y/releases/download/v0.2.0/sums" },
                { "name": "evil-arm64.dmg", "browser_download_url": "http://example.com/evil-arm64.dmg" }
            ]
        });
        let release = parse_release(&json, "macos", "aarch64").unwrap();
        assert_eq!(release.version, "0.2.0");
        assert_eq!(release.installer_name.as_deref(), Some("Agentty-0.2.0-release1-arm64.dmg"));
        assert!(release.checksums_url.is_some());
        let mut draft = json.clone();
        draft["draft"] = true.into();
        assert!(parse_release(&draft, "macos", "aarch64").is_none());
    }

    #[test]
    fn picks_published_setup_for_windows() {
        let asset = |name: &str| serde_json::json!({ "name": name, "browser_download_url": format!("https://github.com/x/y/releases/download/v0.2.0/{name}") });
        let json = serde_json::json!({
            "tag_name": "v0.2.0", "draft": false, "prerelease": false,
            "html_url": "https://github.com/empty-user77/agentty-releases/releases/tag/v0.2.0",
            "assets": [
                asset("Agentty-0.2.0-release1-arm64.dmg"),
                asset("Agentty-0.2.0-windows-x64-setup.zip"),
                asset("Agentty-0.2.0-windows-x64-setup.exe"),
                asset("Agentty-0.2.0-linux-amd64.deb"),
                asset("Agentty-0.2.0-SHA256SUMS.txt"),
                { "name": "Agentty-0.2.1-windows-x64-setup.exe", "browser_download_url": "http://example.com/evil.exe" }
            ]
        });
        let windows = parse_release(&json, "windows", "x86_64").unwrap();
        assert_eq!(windows.installer_name.as_deref(), Some("Agentty-0.2.0-windows-x64-setup.exe"));
        assert!(windows.installer_url.as_deref().is_some_and(|u| u.ends_with("/Agentty-0.2.0-windows-x64-setup.exe")));
        assert!(windows.checksums_url.is_some());
        let linux = parse_release(&json, "linux", "x86_64").unwrap();
        assert_eq!((linux.installer_name, linux.installer_url), (None, None));
    }

    #[test]
    fn reads_checksum_listing() {
        let listing = "abc123  Agentty-0.2.0-arm64.zip\nDEF456  /tmp/dist/Agentty-0.2.0-release1-arm64.dmg\n";
        assert_eq!(checksum_for(listing, "Agentty-0.2.0-release1-arm64.dmg").as_deref(), Some("def456"));
        assert_eq!(checksum_for(listing, "missing.dmg"), None);
    }
}
