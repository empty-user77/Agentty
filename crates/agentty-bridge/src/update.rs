//! Update checks against the public release channel (GitHub Releases of `agentty-releases`) and a
//! verified download of the new DMG. Installing (mount, signature check, swap, relaunch) is done by
//! the app; everything here is platform-neutral and testable.

use anyhow::{bail, ensure, Context, Result};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::Path;
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
        page_url: json["html_url"].as_str().filter(|u| trusted_url(u)).unwrap_or(RELEASES_PAGE).to_string(),
        dmg_name: dmg.as_ref().map(|d| d.0.clone()),
        dmg_url: dmg.map(|d| d.1),
        checksums_url: checksums.map(|c| c.1),
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
pub fn check(current: &str, arch: &str) -> Result<Option<Release>> {
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
            return Ok(parse_release(&json, arch).filter(|r| is_newer(&r.version, current)));
        }
        // No published release yet.
        Err(ureq::Error::Status(404, _)) => return Ok(None),
        Err(ureq::Error::Status(code, response)) => {
            let message = response.into_json::<serde_json::Value>().ok().and_then(|j| j["message"].as_str().map(str::to_string));
            anyhow::anyhow!("{url}: status code {code}{}", message.map(|m| format!(" ({m})")).unwrap_or_default())
        }
        Err(err) => anyhow::Error::new(err),
    };
    check_via_web(current, arch)
        .map_err(|web| api_error.context(format!("fallback via github.com also failed: {web:#}")))
        .context("update check failed")
}

/// The latest release from github.com itself: `releases/latest` redirects to the release's tag, and
/// the checksum list names its files.
fn check_via_web(current: &str, arch: &str) -> Result<Option<Release>> {
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
    release_from_checksums(&tag, &listing, arch).context("no DMG listed in the release checksums").map(Some)
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
fn release_from_checksums(tag: &str, listing: &str, arch: &str) -> Option<Release> {
    let names: Vec<&str> = listing.lines().filter_map(|l| l.split_whitespace().nth(1)).map(|n| n.trim_start_matches('*')).collect();
    let arch_aliases: &[&str] = if arch == "aarch64" || arch == "arm64" { &["arm64", "aarch64"] } else { &["x86_64", "x64", "intel"] };
    let dmg = names
        .iter()
        .find(|n| n.ends_with(".dmg") && arch_aliases.iter().any(|a| n.contains(a)))
        .or_else(|| names.iter().find(|n| n.ends_with(".dmg") && !n.contains("x86_64") && !n.contains("arm64")))?
        .to_string();
    if dmg.contains('/') || dmg.contains("..") {
        return None;
    }
    let download = |name: &str| format!("https://github.com/{RELEASE_REPO}/releases/download/{tag}/{name}");
    let version = tag.trim_start_matches('v').to_string();
    Some(Release {
        notes: String::new(),
        page_url: format!("https://github.com/{RELEASE_REPO}/releases/tag/{tag}"),
        dmg_url: Some(download(&dmg)),
        checksums_url: Some(download(&format!("Agentty-{version}-SHA256SUMS.txt"))),
        dmg_name: Some(dmg),
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

/// Downloads the release DMG into `dir` and verifies its SHA-256 against the published checksums.
/// `progress` receives the bytes written so far and the total size when the server reports one.
pub fn download(release: &Release, dir: &Path, current: &str, progress: &mut dyn FnMut(u64, Option<u64>)) -> Result<std::path::PathBuf> {
    let (Some(url), Some(name)) = (&release.dmg_url, &release.dmg_name) else { bail!("this release has no DMG for your Mac") };
    ensure!(trusted_url(url), "untrusted download location");
    ensure!(!name.contains('/') && !name.contains(".."), "invalid asset name");
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
    fn web_fallback_reads_tag_and_checksums() {
        let tag = tag_from_release_url("https://github.com/empty-user77/agentty-releases/releases/tag/v0.1.6");
        assert_eq!(tag.as_deref(), Some("v0.1.6"));
        assert_eq!(tag_from_release_url("https://github.com/empty-user77/agentty-releases/releases"), None);
        assert_eq!(tag_from_release_url("https://example.com/empty-user77/agentty-releases/releases/tag/v0.1.6"), None);

        let listing = "aaa  Agentty-0.1.6-release20260918131331-arm64.dmg\nbbb  Agentty-0.1.6-arm64.zip\n";
        let release = release_from_checksums("v0.1.6", listing, "aarch64").unwrap();
        assert_eq!(release.version, "0.1.6");
        assert_eq!(release.dmg_name.as_deref(), Some("Agentty-0.1.6-release20260918131331-arm64.dmg"));
        let dmg_url = release.dmg_url.unwrap();
        assert!(trusted_url(&dmg_url));
        assert_eq!(
            dmg_url,
            "https://github.com/empty-user77/agentty-releases/releases/download/v0.1.6/Agentty-0.1.6-release20260918131331-arm64.dmg"
        );
        assert_eq!(
            release.checksums_url.as_deref(),
            Some("https://github.com/empty-user77/agentty-releases/releases/download/v0.1.6/Agentty-0.1.6-SHA256SUMS.txt")
        );
        assert!(release_from_checksums("v0.1.6", "aaa  Agentty-0.1.6-arm64.zip\n", "aarch64").is_none());
        assert!(release_from_checksums("v0.1.6", "aaa  ../evil-arm64.dmg\n", "aarch64").is_none());
    }

    #[test]
    #[ignore = "needs network"]
    fn web_fallback_finds_the_published_release() {
        let release = check_via_web("0.0.1", "aarch64").unwrap().expect("a published release");
        assert!(release.dmg_url.is_some_and(|u| u.ends_with("-arm64.dmg")));
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
