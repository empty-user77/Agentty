//! Keeps the user signed in to the sites plugins use (`browser.sites` of enabled plugins with
//! `browser.control`).
//!
//! The in-app browser keeps cookies that carry an expiry date on disk by itself. Two things still
//! sign a user out:
//! - **Session cookies** (no expiry) end with the browser, so every restart of Agentty would. For
//!   the kept sites they are saved in the Keychain and put back when Agentty starts.
//! - **Idle sessions** that a site ends when it has not been used for a while. A kept site that
//!   says how to tell it is signed in (`signedInCookie`) is opened out of sight every few hours,
//!   which is a visit like any other and keeps the session going.
//!
//! Only the kept sites' cookies are ever read, and only here. They never reach a plugin.

use crate::webview::Cookie;
use agentty_bridge::plugins::sites::Site;
use gpui::App;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

const KEYCHAIN_SERVICE: &str = "run.agentty.browser";
/// How often session cookies are saved while Agentty runs.
const SAVE_EVERY: Duration = Duration::from_secs(10 * 60);
/// How often a signed-in site is visited to keep its session going.
const REFRESH_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// How long a refresh visit stays open before the page is closed again.
pub const REFRESH_STAY: Duration = Duration::from_secs(25);

/// The Keychain entries this install holds, so a site no plugin names any more is forgotten.
fn index_path() -> PathBuf {
    agentty_bridge::fsutil::data_dir().join("browser-sessions.json")
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Index {
    /// Host → when its session cookies were last saved and last refreshed (ms since the epoch).
    #[serde(default)]
    sites: BTreeMap<String, SiteRecord>,
}

#[derive(Default, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SiteRecord {
    #[serde(default)]
    saved_at: Option<u64>,
    #[serde(default)]
    refreshed_at: Option<u64>,
    /// A digest of what was saved last, so an unchanged session is not written again.
    #[serde(default)]
    digest: Option<String>,
}

static INDEX: Mutex<Option<Index>> = Mutex::new(None);

fn with_index<T>(f: impl FnOnce(&mut Index) -> T) -> T {
    let mut guard = INDEX.lock().unwrap_or_else(|e| e.into_inner());
    let index =
        guard.get_or_insert_with(|| std::fs::read(index_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default());
    let result = f(index);
    if let Ok(bytes) = serde_json::to_vec_pretty(index) {
        // Only hosts and times: nothing secret, but kept private like the rest of the data folder.
        let _ = agentty_bridge::fsutil::write_private(&index_path(), &bytes);
    }
    result
}

fn service() -> String {
    agentty_bridge::connectors::scoped_service(KEYCHAIN_SERVICE, std::env::var_os("AGENTTY_DATA_DIR").as_deref())
}

fn account(host: &str) -> String {
    format!("session-cookies:{host}")
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Sites of enabled plugins that may use the browser, each once.
pub fn kept_sites(cx: &App) -> Vec<Site> {
    let mut sites: Vec<Site> = Vec::new();
    for (_, manifest) in crate::plugins::active(cx) {
        if !manifest.has_permission("browser.control") {
            continue;
        }
        for site in manifest.browser.iter().flat_map(|browser| browser.sites.iter()) {
            if !sites.iter().any(|kept| kept.host == site.host) {
                sites.push(site.clone());
            }
        }
    }
    sites
}

fn private_mode(cx: &App) -> bool {
    crate::settings::settings(cx).browser.private_mode
}

/// Puts saved session cookies back (at startup) and forgets sites no plugin names any more.
pub fn restore(cx: &App) {
    if !crate::platform::HAS_WEBVIEW || private_mode(cx) {
        return;
    }
    let kept = kept_sites(cx);
    let stale: Vec<String> =
        with_index(|index| index.sites.keys().filter(|host| !kept.iter().any(|s| &s.host == *host)).cloned().collect());
    for host in stale {
        forget(&host);
    }
    for site in kept {
        let saved = with_index(|index| index.sites.get(&site.host).is_some_and(|record| record.saved_at.is_some()));
        if !saved {
            continue;
        }
        let Ok(text) = agentty_bridge::secret_store::load(&service(), &account(&site.host)) else { continue };
        let Ok(cookies) = serde_json::from_str::<Vec<Cookie>>(&text) else { continue };
        // Only what belongs to the site, whatever the entry holds.
        let cookies: Vec<Cookie> = cookies.into_iter().filter(|c| site.covers_cookie_domain(&c.domain) && c.expires.is_none()).collect();
        crate::webview::set_cookies(&cookies);
    }
}

/// Saves the kept sites' session cookies now (after a sign-in, and every few minutes).
pub fn remember(cx: &App) {
    if !crate::platform::HAS_WEBVIEW || private_mode(cx) {
        return;
    }
    for site in kept_sites(cx) {
        let host = site.host.clone();
        crate::webview::cookies(
            move |domain| site.covers_cookie_domain(domain),
            move |cookies| {
                let session: Vec<Cookie> = cookies.into_iter().filter(|c| c.expires.is_none()).collect();
                if session.is_empty() {
                    return;
                }
                let Ok(text) = serde_json::to_string(&session) else { return };
                let digest = digest(&text);
                let unchanged = with_index(|index| index.sites.get(&host).and_then(|r| r.digest.clone()) == Some(digest.clone()));
                if unchanged {
                    return;
                }
                if agentty_bridge::secret_store::store(&service(), &account(&host), &text).is_ok() {
                    with_index(|index| {
                        let record = index.sites.entry(host).or_default();
                        record.saved_at = Some(now_ms());
                        record.digest = Some(digest);
                    });
                }
            },
        );
    }
}

/// Deletes what was saved for `host`.
pub fn forget(host: &str) {
    let _ = agentty_bridge::secret_store::delete(&service(), &account(host));
    with_index(|index| index.sites.remove(host));
}

/// Deletes everything saved (the browser's "clear cookies" button).
pub fn forget_all() {
    let hosts: Vec<String> = with_index(|index| index.sites.keys().cloned().collect());
    for host in hosts {
        forget(&host);
    }
}

/// When `host` was last refreshed, in ms since the epoch.
pub fn refreshed_at(host: &str) -> Option<u64> {
    with_index(|index| index.sites.get(host).and_then(|r| r.refreshed_at))
}

/// Sites due for a refresh visit: signed-in ones not visited for [`REFRESH_EVERY`].
fn due_for_refresh(cx: &App) -> Vec<Site> {
    let now = now_ms();
    kept_sites(cx)
        .into_iter()
        .filter(|site| site.signed_in_cookie.is_some())
        .filter(|site| refreshed_at(&site.host).is_none_or(|at| now.saturating_sub(at) >= REFRESH_EVERY.as_millis() as u64))
        .collect()
}

fn mark_refreshed(host: &str) {
    with_index(|index| index.sites.entry(host.to_string()).or_default().refreshed_at = Some(now_ms()));
}

/// Short digest of what was saved, only compared with the next one to skip unchanged writes.
fn digest(text: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Restores the saved sessions and keeps them fresh while Agentty runs.
pub fn start(cx: &mut App) {
    restore(cx);
    cx.spawn(async move |cx| {
        // First look a minute in: nothing should compete with the app starting up.
        cx.background_executor().timer(Duration::from_secs(60)).await;
        loop {
            let _ = cx.update(|cx| {
                remember(cx);
                if private_mode(cx) {
                    return;
                }
                for site in due_for_refresh(cx) {
                    let host = site.host.clone();
                    crate::workbench::plugin_browser::site_status(&site.clone(), {
                        let host = host.clone();
                        move |status| {
                            // Signed out: a visit would only show the sign-in page. The plugin
                            // hears it from `browser/sites` and asks the user.
                            if status.is_some_and(|(signed_in, _)| signed_in) {
                                REFRESH_QUEUE.lock().unwrap_or_else(|e| e.into_inner()).push(site);
                            } else {
                                mark_refreshed(&host);
                            }
                        }
                    });
                }
                let queued: Vec<Site> = std::mem::take(&mut *REFRESH_QUEUE.lock().unwrap_or_else(|e| e.into_inner()));
                for site in queued {
                    mark_refreshed(&site.host);
                    crate::with_active_workbench(cx, |workbench, window, cx| workbench.refresh_site(&site, window, cx));
                }
            });
            cx.background_executor().timer(SAVE_EVERY).await;
        }
    })
    .detach();
}

/// Signed-in sites found due, visited on the next round (the cookie check answers later).
static REFRESH_QUEUE: Mutex<Vec<Site>> = Mutex::new(Vec::new());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digests_differ_when_the_session_does() {
        assert_eq!(digest("a"), digest("a"));
        assert_ne!(digest("a"), digest("b"));
        assert_eq!(digest("a").len(), 16);
    }

    #[test]
    fn each_site_has_its_own_entry() {
        assert_eq!(account("x.com"), "session-cookies:x.com");
        assert_ne!(account("x.com"), account("twitter.com"));
    }
}
