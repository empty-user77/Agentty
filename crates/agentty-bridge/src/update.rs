//! Update checks against the public release channel (GitHub Releases of `agentty-releases`) and a
//! verified download of the new DMG. Installing (mount, signature check, swap, relaunch) is done by
//! the app; everything here is platform-neutral and testable.

use anyhow::{bail, ensure, Context, Result};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

pub const RELEASE_REPO: &str = "empty-user77/agentty-releases";
const MAX_DOWNLOAD: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub page_url: String,
    pub dmg_url: Option<String>,
    pub dmg_name: Option<String>,
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

/// Picks the published (non-draft, non-prerelease) release and its DMG for `arch`.
pub fn parse_release(json: &serde_json::Value, arch: &str) -> Option<Release> {
    if json["draft"].as_bool() == Some(true) || json["prerelease"].as_bool() == Some(true) {
        return None;
    }
    let version = json["tag_name"].as_str().filter(|t| parse_version(t).is_some())?.trim_start_matches('v').to_string();
    let assets = json["assets"].as_array().cloned().unwrap_or_default();
    let asset = |pred: &dyn Fn(&str) -> bool| {
        assets.iter().find_map(|a| {
            let name = a["name"].as_str()?;
            let url = a["browser_download_url"].as_str().filter(|u| trusted_url(u))?;
            pred(name).then(|| (name.to_string(), url.to_string()))
        })
    };
    let arch_aliases: &[&str] = if arch == "aarch64" || arch == "arm64" { &["arm64", "aarch64"] } else { &["x86_64", "x64", "intel"] };
    let dmg = asset(&|n: &str| n.ends_with(".dmg") && arch_aliases.iter().any(|a| n.contains(a)))
        .or_else(|| asset(&|n: &str| n.ends_with(".dmg") && !n.contains("x86_64") && !n.contains("arm64")));
    let checksums = asset(&|n: &str| n.ends_with("SHA256SUMS.txt"));
    Some(Release {
        version,
        notes: json["body"].as_str().unwrap_or_default().to_string(),
        page_url: json["html_url"]
            .as_str()
            .filter(|u| trusted_url(u))
            .unwrap_or("https://github.com/empty-user77/agentty-releases/releases")
            .to_string(),
        dmg_name: dmg.as_ref().map(|d| d.0.clone()),
        dmg_url: dmg.map(|d| d.1),
        checksums_url: checksums.map(|c| c.1),
    })
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(10)).timeout(Duration::from_secs(600)).build()
}

/// The newest published release, if it is newer than `current`.
pub fn check(current: &str, arch: &str) -> Result<Option<Release>> {
    let url = format!("https://api.github.com/repos/{RELEASE_REPO}/releases/latest");
    let response = match agent()
        .get(&url)
        .set("Accept", "application/vnd.github+json")
        .set("User-Agent", &format!("Agentty/{current}"))
        .timeout(Duration::from_secs(20))
        .call()
    {
        Ok(response) => response,
        // No published release yet.
        Err(ureq::Error::Status(404, _)) => return Ok(None),
        Err(err) => return Err(err).context("update check failed"),
    };
    let json: serde_json::Value = response.into_json()?;
    Ok(parse_release(&json, arch).filter(|r| is_newer(&r.version, current)))
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

/// Downloads the release DMG into `dir` and verifies its SHA-256 against the published checksums.
pub fn download(release: &Release, dir: &Path, current: &str) -> Result<std::path::PathBuf> {
    let (Some(url), Some(name)) = (&release.dmg_url, &release.dmg_name) else { bail!("this release has no DMG for your Mac") };
    ensure!(trusted_url(url), "untrusted download location");
    ensure!(!name.contains('/') && !name.contains(".."), "invalid asset name");
    std::fs::create_dir_all(dir)?;
    let path = dir.join(name);
    let response = agent().get(url).set("User-Agent", &format!("Agentty/{current}")).call().context("download failed")?;
    let mut reader = response.into_reader().take(MAX_DOWNLOAD + 1);
    let mut file = std::fs::File::create(&path)?;
    let copied = std::io::copy(&mut reader, &mut file)?;
    file.flush()?;
    ensure!(copied <= MAX_DOWNLOAD, "download is unexpectedly large");

    let sums_url = release.checksums_url.as_ref().context("release has no checksums; refusing to install")?;
    ensure!(trusted_url(sums_url), "untrusted checksum location");
    let listing = agent().get(sums_url).set("User-Agent", &format!("Agentty/{current}")).call()?.into_string()?;
    let expected = checksum_for(&listing, name).context("checksum missing for the DMG")?;
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
        let release = parse_release(&json, "aarch64").unwrap();
        assert_eq!(release.version, "0.2.0");
        assert_eq!(release.dmg_name.as_deref(), Some("Agentty-0.2.0-release1-arm64.dmg"));
        assert!(release.checksums_url.is_some());
        let mut draft = json.clone();
        draft["draft"] = true.into();
        assert!(parse_release(&draft, "aarch64").is_none());
    }

    #[test]
    fn reads_checksum_listing() {
        let listing = "abc123  Agentty-0.2.0-arm64.zip\nDEF456  /tmp/dist/Agentty-0.2.0-release1-arm64.dmg\n";
        assert_eq!(checksum_for(listing, "Agentty-0.2.0-release1-arm64.dmg").as_deref(), Some("def456"));
        assert_eq!(checksum_for(listing, "missing.dmg"), None);
    }
}
