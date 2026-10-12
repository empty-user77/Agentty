//! Jira Cloud for the board: the issues a JQL search finds, read with the user's API token, become
//! tickets in the backlog. The token lives in the Keychain (service `run.agentty.jira`); the site,
//! the e-mail address and the search are plain settings.

use anyhow::Result;
use serde_json::Value;
use std::time::Duration;

pub const KEYCHAIN_SERVICE: &str = "run.agentty.jira";
const ACCOUNT: &str = "api-token";
/// Issues read in one import.
const MAX_ISSUES: usize = 50;

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
    NoSearch,
    NoToken,
    Unexpected,
    /// Jira refused the e-mail address or API token.
    Refused,
    /// Jira could not run the search; its own explanation, when it gave one.
    SearchFailed(Option<String>),
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
            Self::NoSearch => f.write_str("the Jira search (JQL) is empty"),
            Self::NoToken => f.write_str("no Jira API token is saved"),
            Self::Unexpected => f.write_str("Jira sent something unexpected"),
            Self::Refused => f.write_str("Jira refused the e-mail address or API token"),
            Self::SearchFailed(detail) => {
                write!(f, "Jira could not run the search{}", detail.as_ref().map(|d| format!(": {d}")).unwrap_or_default())
            }
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

fn issues_from(value: &Value, origin: &str) -> Vec<Issue> {
    value["issues"]
        .as_array()
        .map(|issues| {
            issues
                .iter()
                .filter_map(|issue| {
                    let key = issue["key"].as_str()?.to_string();
                    let fields = &issue["fields"];
                    Some(Issue {
                        url: format!("{origin}/browse/{key}"),
                        summary: fields["summary"].as_str().unwrap_or_default().to_string(),
                        description: adf_text(&fields["description"]),
                        status: fields["status"]["name"].as_str().unwrap_or_default().to_string(),
                        key,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The issues `jql` finds on `site`, signed in as `email` with the token in the Keychain.
pub fn search(site: &str, email: &str, jql: &str) -> Result<Vec<Issue>> {
    let origin = site_origin(site)?;
    if email.trim().is_empty() {
        return Err(JiraError::NoEmail.into());
    }
    if jql.trim().is_empty() {
        return Err(JiraError::NoSearch.into());
    }
    let token = crate::secret_store::load(&service(), ACCOUNT).map_err(|_| JiraError::NoToken)?;
    use base64::Engine as _;
    let auth = base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", email.trim(), token));
    let agent = crate::http::agent_builder().redirects(0).timeout(Duration::from_secs(20)).build();
    let response = agent
        .get(&format!("{origin}/rest/api/3/search/jql"))
        .query("jql", jql.trim())
        .query("fields", "summary,description,status")
        .query("maxResults", &MAX_ISSUES.to_string())
        .set("Authorization", &format!("Basic {auth}"))
        .set("Accept", "application/json")
        .set("User-Agent", "Agentty")
        .call();
    let value: Value = match response {
        Ok(response) => response.into_json().map_err(|_| JiraError::Unexpected)?,
        Err(ureq::Error::Status(401 | 403, _)) => return Err(JiraError::Refused.into()),
        Err(ureq::Error::Status(400, response)) => {
            // Jira explains a bad search ("Field 'x' does not exist"); its text never holds the token.
            let detail = response
                .into_json::<Value>()
                .ok()
                .and_then(|v| v["errorMessages"][0].as_str().map(|m| m.chars().take(160).collect::<String>()));
            return Err(JiraError::SearchFailed(detail).into());
        }
        Err(ureq::Error::Status(status, _)) => return Err(JiraError::Status(status).into()),
        Err(_) => return Err(JiraError::Unreachable(origin).into()),
    };
    Ok(issues_from(&value, &origin))
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
    fn search_results_become_issues_with_their_page() {
        let value = json!({ "issues": [
            { "key": "AB-1", "fields": { "summary": "Fix login", "description": null, "status": { "name": "To Do" } } },
            { "fields": { "summary": "no key, skipped" } }
        ] });
        let issues = issues_from(&value, "https://team.atlassian.net");
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].key, "AB-1");
        assert_eq!(issues[0].status, "To Do");
        assert_eq!(issues[0].url, "https://team.atlassian.net/browse/AB-1");
    }
}
