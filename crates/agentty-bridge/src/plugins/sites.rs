//! Sites a plugin may use the in-app browser on (`browser.sites` in `agentty-plugin.json`).
//!
//! A plugin with `browser.control` drives pages in the in-app browser, which is signed in as the
//! user. What keeps that bounded is this list: the plugin's scripts run only on pages of the sites
//! it named, which the user saw when installing it. Cookies never reach a plugin; Agentty reads
//! them only to say whether a site is signed in and until when, and to keep the user signed in.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

/// Most sites one plugin may name.
pub const MAX_SITES: usize = 8;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserContribution {
    #[serde(default)]
    pub sites: Vec<Site>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Site {
    /// The site's domain (`x.com`); its subdomains are included.
    pub host: String,
    /// Other domains of the same site (`twitter.com`), included the same way.
    #[serde(default)]
    pub aliases: Vec<String>,
    /// A page that needs the user signed in (`https://x.com/home`): what Agentty opens to keep the
    /// session fresh, and where "sign in" starts when there is no `signIn` page.
    #[serde(default)]
    pub home: Option<String>,
    /// The site's sign-in page.
    #[serde(default)]
    pub sign_in: Option<String>,
    /// A cookie the site sets only while the user is signed in (`auth_token`). Agentty checks it
    /// is there — never what it holds — to say whether the site is signed in and until when.
    #[serde(default)]
    pub signed_in_cookie: Option<String>,
}

impl Site {
    /// Every domain of this site: `host` first, then the aliases.
    pub fn domains(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.host.as_str()).chain(self.aliases.iter().map(String::as_str))
    }

    /// Whether `host` (a URL's host, lower case) belongs to this site.
    pub fn covers_host(&self, host: &str) -> bool {
        self.domains().any(|domain| host == domain || host.strip_suffix(domain).is_some_and(|rest| rest.ends_with('.')))
    }

    /// Whether a cookie set for `domain` (`.x.com`, `x.com`, `api.x.com`) belongs to this site.
    pub fn covers_cookie_domain(&self, domain: &str) -> bool {
        self.covers_host(domain.trim_start_matches('.').to_ascii_lowercase().as_str())
    }

    /// The page to open to sign in: `signIn`, else `home`, else the site's front page.
    pub fn sign_in_url(&self) -> String {
        self.sign_in.clone().or_else(|| self.home.clone()).unwrap_or_else(|| format!("https://{}/", self.host))
    }

    /// The page to open to keep the session fresh: `home`, else the front page.
    pub fn home_url(&self) -> String {
        self.home.clone().unwrap_or_else(|| format!("https://{}/", self.host))
    }

    fn validate(&self) -> Result<()> {
        for domain in self.domains() {
            if !valid_domain(domain) {
                bail!("\"{domain}\" is not a site domain (lower-case, like example.com, no scheme or path)");
            }
        }
        if self.aliases.len() > 4 {
            bail!("site {}: at most 4 aliases", self.host);
        }
        for url in [&self.home, &self.sign_in].into_iter().flatten() {
            if !url_host(url).is_some_and(|host| self.covers_host(&host)) {
                bail!("site {}: {url} must be an https:// page of the site", self.host);
            }
        }
        if let Some(cookie) = &self.signed_in_cookie {
            let valid = !cookie.is_empty() && cookie.len() <= 64 && cookie.chars().all(|c| c.is_ascii_alphanumeric() || "_-.".contains(c));
            if !valid {
                bail!("site {}: signedInCookie must be a cookie name", self.host);
            }
        }
        Ok(())
    }
}

impl BrowserContribution {
    pub fn validate(&self) -> Result<()> {
        if self.sites.len() > MAX_SITES {
            bail!("at most {MAX_SITES} browser sites");
        }
        for site in &self.sites {
            site.validate()?;
        }
        Ok(())
    }

    /// The site `url` belongs to, if the plugin named it.
    pub fn site_for_url(&self, url: &str) -> Option<&Site> {
        let host = url_host(url)?;
        self.sites.iter().find(|site| site.covers_host(&host))
    }

    pub fn site_named(&self, host: &str) -> Option<&Site> {
        let host = host.trim().to_ascii_lowercase();
        self.sites.iter().find(|site| site.host == host || site.aliases.contains(&host))
    }
}

/// `localhost` and `127.0.0.1` are sites too, so a plugin can be tried against a local page; they
/// are the only ones reached over plain `http`.
fn is_loopback(host: &str) -> bool {
    host == "localhost" || host == "127.0.0.1"
}

/// A registrable-looking domain: lower-case labels joined by dots, or a loopback name.
fn valid_domain(domain: &str) -> bool {
    if is_loopback(domain) {
        return true;
    }
    let labels: Vec<&str> = domain.split('.').collect();
    domain.len() <= 253
        && labels.len() >= 2
        && labels.iter().all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
        // A top-level label is never all digits: that is an IP address, not a site.
        && !labels.last().is_some_and(|tld| tld.chars().all(|c| c.is_ascii_digit()))
}

/// The host of an `https://` URL (or an `http://` one on this machine), lower case; `None` for
/// anything else.
pub fn url_host(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();
    match parsed.scheme() {
        "https" => Some(host),
        "http" if is_loopback(&host) => Some(host),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn x() -> Site {
        Site {
            host: "x.com".into(),
            aliases: vec!["twitter.com".into()],
            home: Some("https://x.com/home".into()),
            sign_in: Some("https://x.com/i/flow/login".into()),
            signed_in_cookie: Some("auth_token".into()),
        }
    }

    #[test]
    fn a_site_covers_itself_its_subdomains_and_its_aliases_only() {
        let site = x();
        assert!(site.covers_host("x.com"));
        assert!(site.covers_host("api.x.com"));
        assert!(site.covers_host("mobile.twitter.com"));
        assert!(!site.covers_host("notx.com"));
        assert!(!site.covers_host("x.com.evil.example"));
        assert!(site.covers_cookie_domain(".x.com"));
        assert!(!site.covers_cookie_domain(".evilx.com"));
    }

    #[test]
    fn only_https_pages_of_a_named_site_match() {
        let browser = BrowserContribution { sites: vec![x()] };
        assert!(browser.site_for_url("https://x.com/someone").is_some());
        assert!(browser.site_for_url("https://X.com/someone").is_some());
        assert!(browser.site_for_url("http://x.com/someone").is_none());
        assert!(browser.site_for_url("https://x.com.evil.example/").is_none());
        assert!(browser.site_for_url("https://user@evil.example/x.com").is_none()); // audit: ok — fake userinfo in a URL, not an address
        assert!(browser.site_for_url("javascript:alert(1)").is_none());
        assert!(browser.site_for_url("file:///etc/passwd").is_none());
    }

    #[test]
    fn loopback_is_a_site_over_plain_http() {
        let browser = BrowserContribution { sites: vec![Site { host: "localhost".into(), ..site("localhost") }] };
        assert!(browser.site_for_url("http://localhost:5173/feed").is_some());
        assert!(browser.validate().is_ok());
    }

    fn site(host: &str) -> Site {
        Site { host: host.into(), aliases: vec![], home: None, sign_in: None, signed_in_cookie: None }
    }

    #[test]
    fn bad_sites_are_refused() {
        for host in ["com", "X.com", "https://x.com", "x.com/home", "*.x.com", "1.2.3.4", "", "-x.com"] {
            let browser = BrowserContribution { sites: vec![site(host)] };
            assert!(browser.validate().is_err(), "{host} should be refused");
        }
        let mut elsewhere = x();
        elsewhere.home = Some("https://evil.example/home".into());
        assert!(BrowserContribution { sites: vec![elsewhere] }.validate().is_err());
        let mut plain = x();
        plain.sign_in = Some("http://x.com/login".into());
        assert!(BrowserContribution { sites: vec![plain] }.validate().is_err());
        let mut cookie = x();
        cookie.signed_in_cookie = Some("a b".into());
        assert!(BrowserContribution { sites: vec![cookie] }.validate().is_err());
        assert!(BrowserContribution { sites: vec![x()] }.validate().is_ok());
        let many = BrowserContribution { sites: (0..=MAX_SITES).map(|i| site(&format!("s{i}.example"))).collect() };
        assert!(many.validate().is_err());
    }

    #[test]
    fn urls_for_signing_in_and_keeping_fresh() {
        let site = x();
        assert_eq!(site.sign_in_url(), "https://x.com/i/flow/login");
        assert_eq!(site.home_url(), "https://x.com/home");
        let bare = self::site("example.com");
        assert_eq!(bare.sign_in_url(), "https://example.com/");
    }
}
