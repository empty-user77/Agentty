//! Reads the user's sign-ins from Google Chrome, for the in-app browser, when the user agreed to it.
//!
//! Only the cookies a browser would send to one host are ever read: nothing else leaves Chrome's
//! file, and a page can never ask for another site's. Values are decrypted in memory, handed to
//! the caller and never logged or written anywhere. The copy of Chrome's file (Chrome keeps the
//! original locked) lives in a private folder for as long as the read takes, and holds values that
//! are still encrypted.
//!
//! macOS only for now: Windows Chrome binds its keys to Chrome itself, and Linux has no in-app
//! browser.

/// One cookie as Chrome keeps it.
#[derive(Clone, Debug, PartialEq)]
pub struct ChromeCookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    /// Seconds since the Unix epoch; `None` for a cookie that ends with the browser session.
    pub expires: Option<f64>,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: Option<String>,
}

/// A Chrome profile: its folder and the name the user knows it by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChromeProfile {
    pub dir: String,
    pub name: String,
}

/// What one profile holds for a host.
#[derive(Clone, Debug, PartialEq)]
pub struct ProfileCookies {
    pub profile: ChromeProfile,
    pub cookies: Vec<ChromeCookie>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadError {
    /// macOS did not hand over Chrome's key (the user chose Deny).
    Refused,
    /// The file or its contents could not be read.
    Unreadable,
}

/// The host of a web page, lowercased; `None` for anything but http and https.
pub fn page_host(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    parsed.host_str().filter(|host| !host.is_empty()).map(|host| host.to_ascii_lowercase())
}

/// Whether the address itself says "sign in here" (the first step of many sign-ins has no password
/// field yet).
fn sign_in_address(url: &url::Url) -> bool {
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    const SIGN_IN_HOSTS: [&str; 4] = ["accounts.google.com", "login.microsoftonline.com", "login.live.com", "appleid.apple.com"];
    if SIGN_IN_HOSTS.contains(&host.as_str()) || ["login.", "signin.", "sso."].iter().any(|prefix| host.starts_with(prefix)) {
        return true;
    }
    // At the top of the site, or under a folder that holds accounts: `/acme/login` is more likely
    // someone's repository than a sign-in, and a real one there shows its password field anyway.
    let segments: Vec<String> = url.path_segments().into_iter().flatten().filter(|s| !s.is_empty()).map(str::to_ascii_lowercase).collect();
    let Some(last) = segments.last() else { return false };
    let last = last.strip_suffix(".html").or(last.strip_suffix(".php")).unwrap_or(last);
    if !matches!(last, "login" | "log-in" | "log_in" | "signin" | "sign-in" | "sign_in" | "sso" | "servicelogin") {
        return false;
    }
    const PARENTS: [&str; 9] = ["auth", "account", "accounts", "user", "users", "u", "id", "identity", "oauth"];
    match segments.as_slice() {
        [_] => true,
        [parent, _] => PARENTS.contains(&parent.as_str()) || parent.len() == 2,
        _ => false,
    }
}

/// Pages that take a password but are not a sign-in, and pages that are about signing in.
fn not_a_sign_in(url: &url::Url) -> bool {
    const ABOUT: [&str; 9] = ["docs", "doc", "blog", "help", "guide", "guides", "support", "wiki", "articles"];
    let first = url.path_segments().and_then(|mut segments| segments.next()).unwrap_or_default().to_ascii_lowercase();
    if ABOUT.contains(&first.as_str()) {
        return true;
    }
    url.path_segments().into_iter().flatten().any(|segment| {
        let segment = segment.to_ascii_lowercase();
        ["signup", "sign-up", "register", "logout", "signout", "sign-out", "forgot", "reset", "recover"]
            .iter()
            .any(|word| segment.contains(word))
    })
}

/// Whether a page is asking the user to sign in, from its address and whether it shows a field for
/// a password.
pub fn is_sign_in_page(url: &str, has_password_field: bool) -> bool {
    let Ok(parsed) = url::Url::parse(url) else { return false };
    if !matches!(parsed.scheme(), "http" | "https") || not_a_sign_in(&parsed) {
        return false;
    }
    has_password_field || sign_in_address(&parsed)
}

pub use imp::{available, forget_key, open_in_chrome, profiles, read};

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::{ChromeProfile, ProfileCookies, ReadError};

    pub fn available() -> bool {
        false
    }

    pub fn profiles() -> Vec<ChromeProfile> {
        Vec::new()
    }

    pub fn read(_host: &str, _only_profile: Option<&str>) -> Result<Vec<ProfileCookies>, ReadError> {
        Ok(Vec::new())
    }

    pub fn forget_key() {}

    pub fn open_in_chrome(_url: &str) {}
}

#[cfg(target_os = "macos")]
mod imp {
    use super::{ChromeCookie, ChromeProfile, ProfileCookies, ReadError};
    use cbc::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
    use sha2::{Digest, Sha256};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Mutex;

    /// Chrome's entry in the login Keychain.
    const KEYCHAIN_SERVICE: &str = "Chrome Safe Storage";
    const KEYCHAIN_ACCOUNT: &str = "Chrome";
    /// Seconds between 1601 (Chrome's clock starts there) and 1970.
    const EPOCH_GAP: f64 = 11_644_473_600.;
    /// More cookies than any one host keeps; a file that says otherwise is not read further.
    const MAX_COOKIES: i64 = 2_000;

    /// The key, once macOS handed it over: the question is asked once per run. One read at a time,
    /// so two pages signing in together don't ask twice either.
    static KEY: Mutex<Option<[u8; 16]>> = Mutex::new(None);
    /// How many times macOS's question got a no.
    static REFUSALS: AtomicU32 = AtomicU32::new(0);

    pub(super) fn answered_no_meanwhile(before: u32, now: u32) -> bool {
        before != now
    }

    /// A test build's stand-ins for Chrome's folder and Keychain entry. Never in a release build.
    fn debug_override(name: &str) -> Option<String> {
        if cfg!(debug_assertions) && std::env::var("AGENTTY_DEBUG").as_deref() == Ok("1") {
            std::env::var(name).ok()
        } else {
            None
        }
    }

    fn chrome_dir() -> PathBuf {
        match debug_override("AGENTTY_TEST_CHROME_DIR") {
            Some(dir) => PathBuf::from(dir),
            None => crate::fsutil::home().join("Library/Application Support/Google/Chrome"),
        }
    }

    pub fn available() -> bool {
        chrome_dir().join("Local State").is_file()
    }

    pub fn profiles() -> Vec<ChromeProfile> {
        profiles_in(&chrome_dir())
    }

    /// The profile last used comes first.
    fn profiles_in(dir: &Path) -> Vec<ChromeProfile> {
        let state: serde_json::Value =
            std::fs::read(dir.join("Local State")).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
        let last_used = state["profile"]["last_used"].as_str().unwrap_or("Default");
        let mut found: Vec<ChromeProfile> = state["profile"]["info_cache"]
            .as_object()
            .map(|cache| {
                cache
                    .iter()
                    .map(|(folder, info)| ChromeProfile { dir: folder.clone(), name: info["name"].as_str().unwrap_or(folder).to_string() })
                    .collect()
            })
            .unwrap_or_default();
        if found.is_empty() {
            found.push(ChromeProfile { dir: "Default".into(), name: "Default".into() });
        }
        found.retain(|profile| cookie_file(dir, &profile.dir).is_some());
        found.sort_by_key(|profile| (profile.dir != last_used, profile.dir.clone()));
        found
    }

    fn cookie_file(dir: &Path, profile: &str) -> Option<PathBuf> {
        // A folder name comes from Chrome's own file, but it still only ever names a child.
        if profile.is_empty() || profile.contains(['/', '\\']) || profile == ".." {
            return None;
        }
        [dir.join(profile).join("Cookies"), dir.join(profile).join("Network/Cookies")].into_iter().find(|path| path.is_file())
    }

    /// The `host_key`s whose cookies a browser sends to `host`: its own, and those set on a domain
    /// above it, never on a bare suffix like `.com`.
    pub(super) fn cookie_hosts(host: &str) -> Vec<String> {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        let mut hosts = vec![host.clone()];
        if host.parse::<std::net::IpAddr>().is_ok() || host.starts_with('[') || !host.contains('.') {
            return hosts;
        }
        let labels: Vec<&str> = host.split('.').collect();
        for start in 0..labels.len().saturating_sub(1) {
            hosts.push(format!(".{}", labels[start..].join(".")));
        }
        hosts
    }

    fn derive_key(password: &[u8]) -> [u8; 16] {
        let mut key = [0u8; 16];
        pbkdf2::pbkdf2_hmac::<sha1::Sha1>(password, b"saltysalt", 1003, &mut key);
        key
    }

    /// A value Chrome encrypted: `v10`, then AES-128-CBC. Since file version 24 the plain text
    /// starts with the digest of the cookie's host, which ties it to that host.
    pub(super) fn decrypt(encrypted: &[u8], key: &[u8; 16], host_key: &str, file_version: i64) -> Option<String> {
        let body = encrypted.strip_prefix(b"v10")?;
        if body.is_empty() || !body.len().is_multiple_of(16) {
            return None;
        }
        let mut buffer = body.to_vec();
        let plain = cbc::Decryptor::<aes::Aes128>::new(key.into(), &[b' '; 16].into()).decrypt_padded_mut::<Pkcs7>(&mut buffer).ok()?;
        let plain = if file_version >= 24 {
            let (digest, rest) = plain.split_at_checked(32)?;
            if digest != Sha256::digest(host_key.as_bytes()).as_slice() {
                return None;
            }
            rest
        } else {
            plain
        };
        String::from_utf8(plain.to_vec()).ok()
    }

    /// Chrome's clock (microseconds since 1601) to seconds since 1970; 0 means a session cookie.
    pub(super) fn expiry(chrome_time: i64) -> Option<f64> {
        (chrome_time > 0).then(|| chrome_time as f64 / 1_000_000. - EPOCH_GAP)
    }

    pub(super) fn same_site(value: i64) -> Option<String> {
        match value {
            1 => Some("lax".into()),
            2 => Some("strict".into()),
            _ => None,
        }
    }

    struct Row {
        name: String,
        plain: String,
        encrypted: Vec<u8>,
        domain: String,
        path: String,
        expires: Option<f64>,
        secure: bool,
        http_only: bool,
        same_site: Option<String>,
    }

    /// The rows for `host` in one profile's file, still encrypted. Chrome holds the file itself
    /// locked while it runs, so a private copy is read and removed again.
    fn rows_of(file: &Path, host: &str, work: &Path) -> Result<(Vec<Row>, i64), ReadError> {
        let _ = std::fs::remove_dir_all(work);
        crate::fsutil::create_private_dir(work).map_err(|_| ReadError::Unreadable)?;
        let result = (|| {
            let copy = work.join("Cookies");
            std::fs::copy(file, &copy).map_err(|_| ReadError::Unreadable)?;
            let mut journal = file.as_os_str().to_owned();
            journal.push("-journal");
            let _ = std::fs::copy(&journal, work.join("Cookies-journal"));
            read_rows(&copy, host).map_err(|_| ReadError::Unreadable)
        })();
        let _ = std::fs::remove_dir_all(work);
        result
    }

    fn read_rows(file: &Path, host: &str) -> rusqlite::Result<(Vec<Row>, i64)> {
        let db = rusqlite::Connection::open(file)?;
        let version: i64 = db
            .query_row("SELECT value FROM meta WHERE key = 'version'", [], |row| row.get::<_, String>(0))
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        let hosts = cookie_hosts(host);
        let marks = vec!["?"; hosts.len()].join(",");
        let now = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.) + EPOCH_GAP)
            * 1_000_000.;
        // A partitioned cookie belongs to this host only while embedded in another site.
        let partitioned: i64 =
            db.query_row("SELECT count(*) FROM pragma_table_info('cookies') WHERE name = 'top_frame_site_key'", [], |row| row.get(0))?;
        let own = if partitioned > 0 { "AND top_frame_site_key = ''" } else { "" };
        let mut statement = db.prepare(&format!(
            "SELECT name, value, encrypted_value, host_key, path, expires_utc, is_secure, is_httponly, samesite \
             FROM cookies WHERE host_key IN ({marks}) {own} AND (expires_utc = 0 OR expires_utc > {now}) LIMIT {MAX_COOKIES}",
            now = now as i64,
        ))?;
        let rows = statement.query_map(rusqlite::params_from_iter(hosts.iter()), |row| {
            Ok(Row {
                name: row.get(0)?,
                plain: row.get(1)?,
                encrypted: row.get(2)?,
                domain: row.get(3)?,
                path: row.get(4)?,
                expires: expiry(row.get(5)?),
                secure: row.get::<_, i64>(6)? != 0,
                http_only: row.get::<_, i64>(7)? != 0,
                same_site: same_site(row.get(8)?),
            })
        })?;
        Ok((rows.collect::<rusqlite::Result<Vec<_>>>()?, version))
    }

    /// The cookies that open with `key`: one that doesn't is left out. `None` when none of the
    /// encrypted ones opens, which means the key is wrong.
    fn open_rows(rows: Vec<Row>, key: &[u8; 16], version: i64) -> Option<Vec<ChromeCookie>> {
        let mut out = Vec::with_capacity(rows.len());
        let (mut sealed, mut opened) = (0, 0);
        for row in rows {
            let value = if row.encrypted.is_empty() {
                row.plain
            } else {
                sealed += 1;
                let Some(value) = decrypt(&row.encrypted, key, &row.domain, version) else { continue };
                opened += 1;
                value
            };
            out.push(ChromeCookie {
                name: row.name,
                value,
                domain: row.domain,
                path: row.path,
                expires: row.expires,
                secure: row.secure,
                http_only: row.http_only,
                same_site: row.same_site,
            });
        }
        (sealed == 0 || opened > 0).then_some(out)
    }

    /// The key, from memory or from the Keychain (where macOS may ask the user).
    fn key_for(cached: &mut Option<[u8; 16]>, password: &dyn Fn() -> Option<String>) -> Result<[u8; 16], ReadError> {
        if let Some(key) = *cached {
            return Ok(key);
        }
        let key = derive_key(password().ok_or(ReadError::Refused)?.as_bytes());
        *cached = Some(key);
        Ok(key)
    }

    pub(super) fn read_in(
        dir: &Path,
        host: &str,
        only_profile: Option<&str>,
        work: &Path,
        cached: &mut Option<[u8; 16]>,
        password: &dyn Fn() -> Option<String>,
    ) -> Result<Vec<ProfileCookies>, ReadError> {
        let mut found = Vec::new();
        let mut failed = None;
        let mut would_not_open = false;
        for profile in profiles_in(dir) {
            if only_profile.is_some_and(|only| only != profile.dir) {
                continue;
            }
            let Some(file) = cookie_file(dir, &profile.dir) else { continue };
            let (rows, version) = match rows_of(&file, host, work) {
                Ok(read) => read,
                Err(error) => {
                    failed = Some(error);
                    continue;
                }
            };
            if rows.is_empty() {
                continue;
            }
            // Only now, with something to open: no question about a site Chrome knows nothing of.
            let key = key_for(cached, password)?;
            match open_rows(rows, &key, version) {
                Some(cookies) => found.push(ProfileCookies { profile, cookies }),
                // An old profile may hold values under a key long gone: the others still count.
                None => {
                    failed = Some(ReadError::Unreadable);
                    would_not_open = true;
                }
            }
        }
        match failed {
            Some(error) if found.is_empty() => {
                // Possibly a key gone stale: the next try, which only the user starts, asks anew.
                if would_not_open {
                    *cached = None;
                }
                Err(error)
            }
            _ => Ok(found),
        }
    }

    pub fn read(host: &str, only_profile: Option<&str>) -> Result<Vec<ProfileCookies>, ReadError> {
        let refusals_before = REFUSALS.load(Ordering::Relaxed);
        let mut cached = KEY.lock().unwrap_or_else(|e| e.into_inner());
        let work = crate::fsutil::data_dir().join("chrome-read");
        let result = read_in(&chrome_dir(), host, only_profile, &work, &mut cached, &|| {
            // Waiting its turn while the user said no to another read: not asked again.
            if answered_no_meanwhile(refusals_before, REFUSALS.load(Ordering::Relaxed)) {
                return None;
            }
            match debug_override("AGENTTY_TEST_CHROME_PASSWORD") {
                Some(password) => Some(password),
                None if cfg!(debug_assertions) && debug_override("AGENTTY_TEST_CHROME_DIR").is_some() => None,
                None => crate::secret_store::load(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT).ok(),
            }
        });
        if result == Err(ReadError::Refused) {
            REFUSALS.fetch_add(1, Ordering::Relaxed);
        }
        result
    }

    /// Drops the key from memory (the user turned the feature off).
    pub fn forget_key() {
        *KEY.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// Opens a page in Chrome, where the user signs in. Only an http or https address gets here,
    /// so it can't be read as an option of `open`.
    pub fn open_in_chrome(url: &str) {
        if super::page_host(url).is_none() || debug_override("AGENTTY_TEST_CHROME_DIR").is_some() {
            return;
        }
        let _ = std::process::Command::new("open").args(["-a", "Google Chrome", url]).spawn();
    }

    #[cfg(test)]
    pub(super) fn test_key(password: &str) -> [u8; 16] {
        derive_key(password.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sign_in_page_is_told_from_its_address_or_its_password_field() {
        assert!(is_sign_in_page("https://github.com/login?return_to=x", false));
        assert!(is_sign_in_page("https://accounts.google.com/v3/signin/identifier", false));
        assert!(is_sign_in_page("https://login.example.com/", false));
        assert!(is_sign_in_page("https://example.com/account", true));
        assert!(is_sign_in_page("http://localhost:3000/auth/sign-in", false));
        assert!(is_sign_in_page("https://example.com/users/sign_in", false));
        assert!(is_sign_in_page("https://example.com/ko/login/", false));
        assert!(!is_sign_in_page("https://github.com/acme/login", false), "a repository");
        assert!(!is_sign_in_page("https://example.com/login/help", false));
        assert!(is_sign_in_page("https://github.com/acme/login", true), "unless it asks for a password");
        assert!(!is_sign_in_page("https://example.com/docs/login-guide", false));
        assert!(!is_sign_in_page("https://example.com/", false));
        assert!(!is_sign_in_page("http://localhost:3000/sessions", false), "a list of sessions");
        assert!(!is_sign_in_page("https://example.com/docs/login", false), "a page about signing in");
        assert!(!is_sign_in_page("https://example.com/help/sign-in", true));
    }

    #[test]
    fn pages_that_take_a_password_for_something_else_are_left_alone() {
        assert!(!is_sign_in_page("https://example.com/signup", true));
        assert!(!is_sign_in_page("https://example.com/login/forgot", true));
        assert!(!is_sign_in_page("https://example.com/password_reset", true));
        assert!(!is_sign_in_page("file:///tmp/login.html", true));
        assert!(!is_sign_in_page("not a url", true));
    }

    #[test]
    fn only_web_pages_have_a_host() {
        assert_eq!(page_host("https://GitHub.com/login").as_deref(), Some("github.com"));
        assert_eq!(page_host("http://localhost:3000/").as_deref(), Some("localhost"));
        assert_eq!(page_host("https://user:pass@example.com/").as_deref(), Some("example.com"));
        assert_eq!(page_host("file:///etc/passwd"), None);
        assert_eq!(page_host("about:blank"), None);
    }

    #[cfg(target_os = "macos")]
    mod reading {
        use super::super::imp::{answered_no_meanwhile, cookie_hosts, decrypt, expiry, read_in, same_site, test_key};
        use super::super::ReadError;
        use cbc::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit};
        use sha2::{Digest, Sha256};
        use std::path::Path;

        const PASSWORD: &str = "fake_chrome_password_example";

        fn seal(plain: &[u8], key: &[u8; 16], host: Option<&str>) -> Vec<u8> {
            let mut text = host.map(|host| Sha256::digest(host.as_bytes()).to_vec()).unwrap_or_default();
            text.extend_from_slice(plain);
            let length = text.len();
            text.resize(length + 16, 0);
            let sealed = cbc::Encryptor::<aes::Aes128>::new(key.into(), &[b' '; 16].into())
                .encrypt_padded_mut::<Pkcs7>(&mut text, length)
                .unwrap()
                .to_vec();
            [b"v10".to_vec(), sealed].concat()
        }

        fn profile_file(dir: &Path, profile: &str, cookies: &[(&str, &str, &str)]) {
            std::fs::create_dir_all(dir.join(profile)).unwrap();
            let db = rusqlite::Connection::open(dir.join(profile).join("Cookies")).unwrap();
            db.execute_batch(
                "CREATE TABLE meta (key TEXT, value TEXT); INSERT INTO meta VALUES ('version', '24');
                 CREATE TABLE cookies (name TEXT, value TEXT, encrypted_value BLOB, host_key TEXT, path TEXT,
                   expires_utc INTEGER, is_secure INTEGER, is_httponly INTEGER, samesite INTEGER);",
            )
            .unwrap();
            let key = test_key(PASSWORD);
            for (host, name, value) in cookies {
                db.execute(
                    "INSERT INTO cookies VALUES (?1, '', ?2, ?3, '/', 0, 1, 1, 1)",
                    rusqlite::params![name, seal(value.as_bytes(), &key, Some(host)), host],
                )
                .unwrap();
            }
        }

        /// A throwaway folder laid out like Chrome's, removed with the test.
        struct FakeChrome(std::path::PathBuf);

        impl FakeChrome {
            fn path(&self) -> &Path {
                &self.0
            }
        }

        impl Drop for FakeChrome {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }

        fn chrome(profiles: &[(&str, &str)], last_used: &str) -> FakeChrome {
            static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let unique = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let dir = FakeChrome(std::env::temp_dir().join(format!("agentty-fake-chrome-{}-{unique}", std::process::id())));
            std::fs::create_dir_all(dir.path()).unwrap();
            let cache: serde_json::Map<String, serde_json::Value> =
                profiles.iter().map(|(folder, name)| (folder.to_string(), serde_json::json!({ "name": name }))).collect();
            std::fs::write(
                dir.path().join("Local State"),
                serde_json::json!({ "profile": { "last_used": last_used, "info_cache": cache } }).to_string(),
            )
            .unwrap();
            dir
        }

        #[test]
        fn a_host_gets_what_a_browser_would_send_it_and_nothing_else() {
            assert_eq!(cookie_hosts("a.b.example.com"), ["a.b.example.com", ".a.b.example.com", ".b.example.com", ".example.com"]);
            assert_eq!(cookie_hosts("localhost"), ["localhost"]);
            assert_eq!(cookie_hosts("127.0.0.1"), ["127.0.0.1"]);
            assert!(!cookie_hosts("example.com").contains(&".com".to_string()));
        }

        #[test]
        fn a_value_opens_with_the_right_key_for_the_right_host_only() {
            let key = test_key(PASSWORD);
            let sealed = seal(b"session-example", &key, Some(".example.com"));
            assert_eq!(decrypt(&sealed, &key, ".example.com", 24).as_deref(), Some("session-example"));
            assert_eq!(decrypt(&sealed, &key, ".other.com", 24), None);
            assert_eq!(decrypt(&sealed, &test_key("wrong"), ".example.com", 24), None);
            let old = seal(b"old-example", &key, None);
            assert_eq!(decrypt(&old, &key, ".example.com", 23).as_deref(), Some("old-example"));
            assert_eq!(decrypt(b"v11abc", &key, ".example.com", 24), None);
        }

        #[test]
        fn chrome_times_and_same_site_read_as_a_browser_means_them() {
            assert_eq!(expiry(0), None);
            assert_eq!(expiry(11_644_473_600_000_000), Some(0.));
            assert_eq!(same_site(1).as_deref(), Some("lax"));
            assert_eq!(same_site(2).as_deref(), Some("strict"));
            assert_eq!(same_site(-1), None);
        }

        #[test]
        fn only_the_hosts_cookies_come_out_with_the_last_used_profile_first() {
            let dir = chrome(&[("Default", "Home"), ("Profile 1", "Work")], "Profile 1");
            profile_file(dir.path(), "Default", &[(".example.com", "sid", "home-example"), (".bank.test", "sid", "secret-example")]);
            profile_file(dir.path(), "Profile 1", &[("app.example.com", "sid", "work-example")]);
            let work = dir.path().join("work");
            let mut key = None;
            let found = read_in(dir.path(), "app.example.com", None, &work, &mut key, &|| Some(PASSWORD.into())).unwrap();
            assert_eq!(found.iter().map(|p| p.profile.name.as_str()).collect::<Vec<_>>(), ["Work", "Home"]);
            assert_eq!(found[0].cookies[0].value, "work-example");
            assert_eq!(found[1].cookies.len(), 1, "the other site's cookie stays in Chrome");
            assert_eq!(found[1].cookies[0].domain, ".example.com");
            assert!(found[1].cookies[0].http_only && found[1].cookies[0].secure);
            assert!(!work.exists(), "the copy of Chrome's file is gone");

            let only = read_in(dir.path(), "app.example.com", Some("Default"), &work, &mut key, &|| None).unwrap();
            assert_eq!(only.len(), 1);
            assert_eq!(only[0].profile.name, "Home");
        }

        #[test]
        fn a_cookie_kept_for_being_embedded_elsewhere_stays_in_chrome() {
            let dir = chrome(&[("Default", "Home")], "Default");
            profile_file(dir.path(), "Default", &[(".example.com", "own", "own-example"), (".example.com", "embedded", "other-example")]);
            let db = rusqlite::Connection::open(dir.path().join("Default/Cookies")).unwrap();
            db.execute_batch(
                "ALTER TABLE cookies ADD COLUMN top_frame_site_key TEXT NOT NULL DEFAULT '';
                 UPDATE cookies SET top_frame_site_key = 'https://other.test' WHERE name = 'embedded';",
            )
            .unwrap();
            drop(db);
            let mut key = None;
            let found = read_in(dir.path(), "example.com", None, &dir.path().join("work"), &mut key, &|| Some(PASSWORD.into())).unwrap();
            assert_eq!(found[0].cookies.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["own"]);
        }

        #[test]
        fn the_keychain_is_asked_only_when_there_is_something_to_open() {
            let dir = chrome(&[("Default", "Home")], "Default");
            profile_file(dir.path(), "Default", &[(".example.com", "sid", "home-example")]);
            let work = dir.path().join("work");
            let asked = std::cell::Cell::new(0);
            let password = || {
                asked.set(asked.get() + 1);
                Some(PASSWORD.to_string())
            };
            let mut key = None;
            assert!(read_in(dir.path(), "nothing.test", None, &work, &mut key, &password).unwrap().is_empty());
            assert_eq!(asked.get(), 0);
            read_in(dir.path(), "example.com", None, &work, &mut key, &password).unwrap();
            read_in(dir.path(), "example.com", None, &work, &mut key, &password).unwrap();
            assert_eq!(asked.get(), 1, "the key is kept for the run");
        }

        #[test]
        fn a_cookie_past_its_date_stays_in_chrome_and_a_session_cookie_comes_along() {
            let dir = chrome(&[("Default", "Home")], "Default");
            profile_file(
                dir.path(),
                "Default",
                &[
                    (".example.com", "old", "old-example"),
                    (".example.com", "later", "later-example"),
                    (".example.com", "session", "s-example"),
                ],
            );
            let db = rusqlite::Connection::open(dir.path().join("Default/Cookies")).unwrap();
            db.execute_batch(
                "UPDATE cookies SET expires_utc = 13000000000000000 WHERE name = 'old';
                 UPDATE cookies SET expires_utc = 99000000000000000 WHERE name = 'later';",
            )
            .unwrap();
            drop(db);
            let mut key = None;
            let found = read_in(dir.path(), "example.com", None, &dir.path().join("work"), &mut key, &|| Some(PASSWORD.into())).unwrap();
            let mut names: Vec<&str> = found[0].cookies.iter().map(|c| c.name.as_str()).collect();
            names.sort();
            assert_eq!(names, ["later", "session"]);
        }

        #[test]
        fn one_cookie_that_will_not_open_does_not_cost_the_others() {
            let dir = chrome(&[("Default", "Home")], "Default");
            profile_file(dir.path(), "Default", &[(".example.com", "good", "good-example"), (".example.com", "odd", "odd-example")]);
            let db = rusqlite::Connection::open(dir.path().join("Default/Cookies")).unwrap();
            db.execute("UPDATE cookies SET encrypted_value = ?1 WHERE name = 'odd'", [b"v10not-a-real-block".to_vec()]).unwrap();
            drop(db);
            let asked = std::cell::Cell::new(0);
            let password = || {
                asked.set(asked.get() + 1);
                Some(PASSWORD.to_string())
            };
            let mut key = None;
            let found = read_in(dir.path(), "example.com", None, &dir.path().join("work"), &mut key, &password).unwrap();
            assert_eq!(found[0].cookies.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["good"]);
            assert_eq!(asked.get(), 1, "not taken for a stale key");
        }

        #[test]
        fn a_refusal_and_a_stale_key_are_told_apart() {
            let dir = chrome(&[("Default", "Home")], "Default");
            profile_file(dir.path(), "Default", &[(".example.com", "sid", "home-example")]);
            let work = dir.path().join("work");
            let mut key = None;
            assert_eq!(read_in(dir.path(), "example.com", None, &work, &mut key, &|| None), Err(ReadError::Refused));
            // A stale key fails once and is dropped, and the next try asks anew.
            let mut stale = Some(test_key("old_key_example"));
            assert_eq!(read_in(dir.path(), "example.com", None, &work, &mut stale, &|| Some(PASSWORD.into())), Err(ReadError::Unreadable));
            assert_eq!(stale, None);
            let found = read_in(dir.path(), "example.com", None, &work, &mut stale, &|| Some(PASSWORD.into())).unwrap();
            assert_eq!(found[0].cookies[0].value, "home-example");
        }

        #[test]
        fn a_profile_that_will_not_open_costs_only_itself_and_asks_no_more_questions() {
            let dir = chrome(&[("Default", "Home"), ("Profile 1", "Old")], "Default");
            profile_file(dir.path(), "Default", &[(".example.com", "sid", "home-example")]);
            profile_file(dir.path(), "Profile 1", &[(".example.com", "sid", "old-example")]);
            let db = rusqlite::Connection::open(dir.path().join("Profile 1/Cookies")).unwrap();
            db.execute("UPDATE cookies SET encrypted_value = ?1", [b"v10not-a-real-block".to_vec()]).unwrap();
            drop(db);
            let asked = std::cell::Cell::new(0);
            let password = || {
                asked.set(asked.get() + 1);
                Some(PASSWORD.to_string())
            };
            let mut key = None;
            let work = dir.path().join("work");
            let found = read_in(dir.path(), "example.com", None, &work, &mut key, &password).unwrap();
            assert_eq!(found.iter().map(|p| p.profile.name.as_str()).collect::<Vec<_>>(), ["Home"]);
            read_in(dir.path(), "example.com", None, &work, &mut key, &password).unwrap();
            assert_eq!(asked.get(), 1);
            assert!(key.is_some(), "the key that opened one profile stays");
        }

        #[test]
        fn a_file_that_can_not_be_read_is_not_taken_for_an_empty_one() {
            let dir = chrome(&[("Default", "Home"), ("Profile 1", "Work")], "Default");
            profile_file(dir.path(), "Default", &[]);
            std::fs::create_dir_all(dir.path().join("Profile 1")).unwrap();
            std::fs::write(dir.path().join("Profile 1/Cookies"), b"this is no database").unwrap();
            let work = dir.path().join("work");
            let mut key = None;
            assert_eq!(read_in(dir.path(), "example.com", None, &work, &mut key, &|| None), Err(ReadError::Unreadable));
            // With something found elsewhere, the rest is what counts.
            std::fs::remove_file(dir.path().join("Default/Cookies")).unwrap();
            profile_file(dir.path(), "Default", &[(".example.com", "sid", "home-example")]);
            let found = read_in(dir.path(), "example.com", None, &work, &mut key, &|| Some(PASSWORD.into())).unwrap();
            assert_eq!(found.len(), 1);
        }

        #[test]
        fn a_read_that_waited_through_a_no_does_not_ask_again() {
            assert!(answered_no_meanwhile(3, 4));
            assert!(!answered_no_meanwhile(4, 4));
        }
    }
}
