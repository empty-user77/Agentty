//! Jira Cloud for the board: an issue named by its key or its page's address, read with the user's
//! API token, becomes a ticket in the backlog. The token lives in the Keychain (service
//! `run.agentty.jira`); the site and the e-mail address are plain settings.

use anyhow::Result;
use serde_json::Value;
use std::time::Duration;

pub const KEYCHAIN_SERVICE: &str = "run.agentty.jira";
const ACCOUNT: &str = "api-token";

fn service() -> String {
    crate::connectors::scoped_service(KEYCHAIN_SERVICE, std::env::var_os("AGENTTY_DATA_DIR").as_deref())
}

pub fn store_token(token: &str) -> Result<()> {
    crate::secret_store::store(&service(), ACCOUNT, token.trim())
}

pub fn has_token() -> bool {
    crate::secret_store::load(&service(), ACCOUNT).is_ok_and(|t| !t.is_empty())
}

pub fn delete_token() -> Result<()> {
    crate::secret_store::delete(&service(), ACCOUNT)
}

/// Why a Jira call failed. The `Display` text is English (logs, tests); the app shows its own
/// translation of each kind.
#[derive(Debug, Clone, PartialEq)]
pub enum JiraError {
    NotWebAddress,
    NotHttps,
    CredentialsInUrl,
    NoHost,
    NoEmail,
    NoToken,
    /// What was entered is neither an issue key (`AB-12`) nor the address of an issue's page.
    NotIssue,
    /// The address is of another site than the one in the settings (the token goes to that one only).
    OtherSite(String),
    /// No such issue, or not one this account may see.
    NotFound(String),
    Unexpected,
    /// Jira refused the e-mail address or API token.
    Refused,
    Status(u16),
    /// The site could not be reached (the origin).
    Unreachable(String),
}

impl std::fmt::Display for JiraError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotWebAddress => f.write_str("the Jira site is not a web address"),
            Self::NotHttps => f.write_str("the Jira site must use https"),
            Self::CredentialsInUrl => f.write_str("the Jira site must not carry a user name or password"),
            Self::NoHost => f.write_str("the Jira site has no host"),
            Self::NoEmail => f.write_str("the Jira e-mail address is missing"),
            Self::NoToken => f.write_str("no Jira API token is saved"),
            Self::NotIssue => f.write_str("not a Jira issue key or issue address"),
            Self::OtherSite(host) => write!(f, "{host} is not the Jira site in the settings"),
            Self::NotFound(key) => write!(f, "Jira has no issue {key} this account can see"),
            Self::Unexpected => f.write_str("Jira sent something unexpected"),
            Self::Refused => f.write_str("Jira refused the e-mail address or API token"),
            Self::Status(status) => write!(f, "Jira answered {status}"),
            Self::Unreachable(origin) => write!(f, "could not reach {origin}"),
        }
    }
}

impl std::error::Error for JiraError {}

/// An issue as a ticket needs it.
#[derive(Debug, Clone, PartialEq)]
pub struct Issue {
    pub key: String,
    pub summary: String,
    pub description: String,
    pub status: String,
    /// The issue's page on the site.
    pub url: String,
}

/// The site's origin (`https://team.atlassian.net`), from whatever was pasted (a page of the site,
/// a trailing slash). Only https: the token goes with every request.
pub fn site_origin(site: &str) -> Result<String> {
    let site = site.trim();
    let with_scheme = if site.contains("://") { site.to_string() } else { format!("https://{site}") };
    let url = url::Url::parse(&with_scheme).map_err(|_| JiraError::NotWebAddress)?;
    if url.scheme() != "https" {
        return Err(JiraError::NotHttps.into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(JiraError::CredentialsInUrl.into());
    }
    let host = url.host_str().ok_or(JiraError::NoHost)?;
    Ok(match url.port() {
        Some(port) => format!("https://{host}:{port}"),
        None => format!("https://{host}"),
    })
}

/// Plain text of an Atlassian Document Format value (issue descriptions in API v3): text nodes,
/// a line break after each paragraph, heading, list item and code block.
pub fn adf_text(node: &Value) -> String {
    fn walk(node: &Value, out: &mut String) {
        match node["type"].as_str() {
            Some("text") => out.push_str(node["text"].as_str().unwrap_or_default()),
            Some("hardBreak") => out.push('\n'),
            Some("mention") => out.push_str(node["attrs"]["text"].as_str().unwrap_or_default()),
            _ => {}
        }
        if node["type"].as_str() == Some("listItem") {
            out.push_str("- ");
        }
        if let Some(children) = node["content"].as_array() {
            for child in children {
                walk(child, out);
            }
        }
        if matches!(node["type"].as_str(), Some("paragraph" | "heading" | "codeBlock" | "blockquote")) && !out.ends_with('\n') {
            out.push('\n');
        }
    }
    let mut out = String::new();
    match node {
        Value::String(text) => out.push_str(text),
        Value::Null => {}
        _ => walk(node, &mut out),
    }
    out.trim().to_string()
}

/// Whether `key` looks like an issue key: a project key (a letter, then letters, digits or `_`),
/// a dash and a number.
fn is_issue_key(key: &str) -> bool {
    let Some((project, number)) = key.split_once('-') else { return false };
    project.len() >= 2
        && project.starts_with(|c: char| c.is_ascii_uppercase())
        && project.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && !number.is_empty()
        && number.len() <= 12
        && number.chars().all(|c| c.is_ascii_digit())
}

/// The issue key in what was entered: a key (`AB-12`, any case) or the address of the issue on
/// `origin` (`…/browse/AB-12`, or a board or search page with `selectedIssue=AB-12`).
pub fn issue_key(entered: &str, origin: &str) -> std::result::Result<String, JiraError> {
    let entered = entered.trim();
    if !entered.contains('/') {
        let key = entered.to_ascii_uppercase();
        return if is_issue_key(&key) { Ok(key) } else { Err(JiraError::NotIssue) };
    }
    let with_scheme = if entered.contains("://") { entered.to_string() } else { format!("https://{entered}") };
    let url = url::Url::parse(&with_scheme).map_err(|_| JiraError::NotIssue)?;
    let host = url.host_str().unwrap_or_default().to_string();
    let site = url::Url::parse(origin).ok().and_then(|o| o.host_str().map(str::to_string)).unwrap_or_default();
    if !host.eq_ignore_ascii_case(&site) {
        return Err(JiraError::OtherSite(host));
    }
    let from_query = url.query_pairs().find(|(name, _)| name == "selectedIssue").map(|(_, value)| value.to_string());
    let from_path = url.path_segments().and_then(|mut segments| {
        segments.by_ref().find(|segment| *segment == "browse")?;
        segments.next().map(str::to_string)
    });
    let key = from_query.or(from_path).map(|key| key.to_ascii_uppercase()).ok_or(JiraError::NotIssue)?;
    if is_issue_key(&key) {
        Ok(key)
    } else {
        Err(JiraError::NotIssue)
    }
}

fn issue_from(issue: &Value, origin: &str) -> Option<Issue> {
    let key = issue["key"].as_str()?.to_string();
    let fields = &issue["fields"];
    Some(Issue {
        url: format!("{origin}/browse/{key}"),
        summary: fields["summary"].as_str().unwrap_or_default().to_string(),
        description: adf_text(&fields["description"]),
        status: fields["status"]["name"].as_str().unwrap_or_default().to_string(),
        key,
    })
}

/// A request to the site in the settings, signed in as `email` with the token in the Keychain.
fn get(site: &str, email: &str, path: &str) -> Result<(String, std::result::Result<Value, u16>)> {
    let origin = site_origin(site)?;
    if email.trim().is_empty() {
        return Err(JiraError::NoEmail.into());
    }
    let token = crate::secret_store::load(&service(), ACCOUNT).map_err(|_| JiraError::NoToken)?;
    use base64::Engine as _;
    let auth = base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", email.trim(), token));
    let agent = crate::http::agent_builder().redirects(0).timeout(Duration::from_secs(20)).build();
    let response = agent
        .get(&format!("{origin}{path}"))
        .set("Authorization", &format!("Basic {auth}"))
        .set("Accept", "application/json")
        .set("User-Agent", "Agentty")
        .call();
    let answer = match response {
        Ok(response) => Ok(response.into_json().map_err(|_| JiraError::Unexpected)?),
        Err(ureq::Error::Status(401 | 403, _)) => return Err(JiraError::Refused.into()),
        Err(ureq::Error::Status(status, _)) => Err(status),
        Err(_) => return Err(JiraError::Unreachable(origin).into()),
    };
    Ok((origin, answer))
}

/// Whether the site, the e-mail address and the token work: Jira says who they sign in as.
pub fn check(site: &str, email: &str) -> Result<String> {
    match get(site, email, "/rest/api/3/myself")? {
        (_, Ok(me)) => Ok(me["displayName"].as_str().unwrap_or_default().to_string()),
        (_, Err(status)) => Err(JiraError::Status(status).into()),
    }
}

/// The issue named by `entered` (its key or its page's address) on `site`.
pub fn fetch_issue(site: &str, email: &str, entered: &str) -> Result<Issue> {
    let key = issue_key(entered, &site_origin(site)?)?;
    match get(site, email, &format!("/rest/api/3/issue/{key}?fields=summary,description,status"))? {
        (origin, Ok(value)) => Ok(issue_from(&value, &origin).ok_or(JiraError::Unexpected)?),
        (_, Err(404)) => Err(JiraError::NotFound(key).into()),
        (_, Err(status)) => Err(JiraError::Status(status).into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_site_is_reduced_to_its_https_origin() {
        assert_eq!(site_origin("team.atlassian.net").unwrap(), "https://team.atlassian.net");
        assert_eq!(site_origin(" https://team.atlassian.net/jira/software/projects/AB/boards/1 ").unwrap(), "https://team.atlassian.net");
        assert_eq!(site_origin("https://jira.example.com:8443/").unwrap(), "https://jira.example.com:8443");
        assert!(site_origin("http://team.atlassian.net").is_err());
        assert!(site_origin("https://user:pass@team.atlassian.net").is_err()); // audit: ok — fake credentials in a URL, not an e-mail address
        assert!(site_origin("not a url at all").is_err());
    }

    #[test]
    fn descriptions_in_adf_read_as_plain_text() {
        let doc = json!({
            "type": "doc",
            "content": [
                { "type": "paragraph", "content": [{ "type": "text", "text": "Users are " }, { "type": "text", "text": "logged out." }] },
                { "type": "bulletList", "content": [
                    { "type": "listItem", "content": [{ "type": "paragraph", "content": [{ "type": "text", "text": "after a minute" }] }] }
                ] },
                { "type": "paragraph", "content": [{ "type": "mention", "attrs": { "text": "@dana" } }, { "type": "hardBreak" }, { "type": "text", "text": "thanks" }] }
            ]
        });
        assert_eq!(adf_text(&doc), "Users are logged out.\n- after a minute\n@dana\nthanks");
        assert_eq!(adf_text(&Value::Null), "");
        assert_eq!(adf_text(&json!("plain")), "plain");
    }

    #[test]
    fn an_issue_comes_with_its_page() {
        let value = json!({ "key": "AB-1", "fields": { "summary": "Fix login", "description": null, "status": { "name": "To Do" } } });
        let issue = issue_from(&value, "https://team.atlassian.net").unwrap();
        assert_eq!((issue.key.as_str(), issue.status.as_str()), ("AB-1", "To Do"));
        assert_eq!(issue.url, "https://team.atlassian.net/browse/AB-1");
        assert!(issue_from(&json!({ "fields": {} }), "https://team.atlassian.net").is_none());
    }

    #[test]
    fn keys_and_issue_addresses_name_an_issue() {
        let site = "https://team.atlassian.net";
        for (entered, key) in [
            ("AB-12", "AB-12"),
            (" ab-12 ", "AB-12"),
            ("WEB2_X-7", "WEB2_X-7"),
            ("https://team.atlassian.net/browse/AB-12", "AB-12"),
            ("team.atlassian.net/browse/ab-12?focusedCommentId=1", "AB-12"),
            ("https://team.atlassian.net/jira/software/projects/AB/boards/1?selectedIssue=AB-34", "AB-34"),
        ] {
            assert_eq!(issue_key(entered, site).as_deref(), Ok(key), "{entered}");
        }
        for entered in ["", "AB", "AB-", "A-1", "1AB-2", "AB-12; rm", "AB-1-2", "https://team.atlassian.net/jira/your-work"] {
            assert_eq!(issue_key(entered, site), Err(JiraError::NotIssue), "{entered}");
        }
        assert_eq!(
            issue_key("https://evil.example/browse/AB-1", site),
            Err(JiraError::OtherSite("evil.example".into())),
            "the token never goes to another site"
        );
    }
}
