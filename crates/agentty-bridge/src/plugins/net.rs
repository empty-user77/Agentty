//! `net/fetch`: the one way a plugin reaches the network.
//!
//! A wasm plugin has no sockets at all, and a plugin running as a process could open its own — the
//! point of routing requests through here is that the user can see the permission in the plugin's
//! card, that the request is bounded (method, headers, sizes, time, redirects), and that a plugin
//! cannot use header or URL text to smuggle a second request past the HTTP client.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Read;
use std::time::{Duration, Instant};

/// Body a plugin may send.
pub const MAX_REQUEST_BODY: usize = 1024 * 1024;
/// Response kept; anything beyond is cut off and the answer says so.
pub const MAX_RESPONSE_BODY: usize = 4 * 1024 * 1024;
pub const MAX_HEADERS: usize = 32;
const MAX_HEADER_NAME: usize = 128;
const MAX_HEADER_VALUE: usize = 4096;
const MAX_URL: usize = 2048;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_REDIRECTS: u32 = 3;

/// Methods with a meaning the client can carry out; `CONNECT` and `TRACE` are not among them.
const METHODS: &[&str] = &["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"];

/// Headers that belong to the connection, not to the request a plugin is making. Letting a plugin
/// set them is how one request becomes two (`Host` picks the server, `Content-Length` and
/// `Transfer-Encoding` decide where the body ends).
const RESERVED_HEADERS: &[&str] = &["host", "content-length", "transfer-encoding", "connection", "upgrade", "expect"];

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchRequest {
    pub url: String,
    #[serde(default = "get")]
    pub method: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// `http://host:port` (with `user:password@` if the proxy asks for it). The request goes
    /// through it instead of straight out.
    #[serde(default)]
    pub proxy: Option<String>,
}

fn get() -> String {
    "GET".to_string()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchResponse {
    pub status: u16,
    pub status_text: String,
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub body: String,
    /// The body was longer than [`MAX_RESPONSE_BODY`] and is cut off.
    pub truncated: bool,
    /// The body was not UTF-8 and is shown with replacement characters.
    pub binary: bool,
    /// Bytes read from the network (before any cut).
    pub bytes: usize,
    pub duration_ms: u64,
}

/// A checked request, ready to send.
struct Checked {
    method: String,
    url: url::Url,
    headers: Vec<(String, String)>,
    body: Option<String>,
    timeout: Duration,
    proxy: Option<String>,
}

fn check(request: &FetchRequest) -> Result<Checked> {
    let method = request.method.trim().to_ascii_uppercase();
    if !METHODS.contains(&method.as_str()) {
        bail!("{} is not a method a plugin can send", request.method);
    }
    if request.url.len() > MAX_URL {
        bail!("the URL is longer than {MAX_URL} characters");
    }
    // Control characters and spaces in a URL are how a request line grows a second line.
    if request.url.chars().any(|c| c.is_control() || c == ' ') {
        bail!("the URL contains spaces or control characters");
    }
    let url = url::Url::parse(request.url.trim()).map_err(|e| anyhow::anyhow!("{e}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        bail!("only http and https addresses can be requested");
    }
    if url.host().is_none() {
        bail!("the URL has no host");
    }
    if is_metadata_host(&url) {
        bail!("that address is not reachable from a plugin");
    }
    if request.headers.len() > MAX_HEADERS {
        bail!("at most {MAX_HEADERS} headers");
    }
    let mut headers = Vec::new();
    for (name, value) in &request.headers {
        let lower = name.to_ascii_lowercase();
        if name.is_empty() || name.len() > MAX_HEADER_NAME || !name.bytes().all(is_token_byte) {
            bail!("\"{name}\" is not a header name");
        }
        if RESERVED_HEADERS.contains(&lower.as_str()) {
            bail!("the {name} header is set by Agentty, not by a plugin");
        }
        if value.len() > MAX_HEADER_VALUE {
            bail!("the {name} header is longer than {MAX_HEADER_VALUE} characters");
        }
        // No CR, LF or NUL: those end a header and start whatever follows.
        if value.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0) {
            bail!("the {name} header contains a line break");
        }
        headers.push((name.clone(), value.clone()));
    }
    let body = match &request.body {
        Some(body) if body.len() > MAX_REQUEST_BODY => bail!("the body is larger than 1 MB"),
        Some(body) if matches!(method.as_str(), "GET" | "HEAD") && !body.is_empty() => {
            bail!("a {method} request has no body")
        }
        other => other.clone(),
    };
    let timeout = request.timeout_ms.map_or(DEFAULT_TIMEOUT, |ms| Duration::from_millis(ms).clamp(Duration::from_millis(100), MAX_TIMEOUT));
    let proxy = match request.proxy.as_deref().map(str::trim).filter(|proxy| !proxy.is_empty()) {
        Some(proxy) => Some(check_proxy(proxy)?),
        None => None,
    };
    Ok(Checked { method, url, headers, body, timeout, proxy })
}

/// The proxy a plugin asks to go through, as text `ureq` accepts. HTTP proxies only: SOCKS is not
/// compiled in, and a proxy that is not one of these is refused rather than quietly ignored.
fn check_proxy(proxy: &str) -> Result<String> {
    if proxy.len() > MAX_URL || proxy.chars().any(|c| c.is_control() || c == ' ') {
        bail!("that is not a proxy address");
    }
    let parsed = url::Url::parse(proxy).map_err(|e| anyhow::anyhow!("{e}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        bail!("a proxy must be http:// or https://");
    }
    let Some(host) = parsed.host_str() else { bail!("the proxy has no host") };
    let credentials = match (parsed.username(), parsed.password()) {
        ("", None) => String::new(),
        (user, password) => format!("{user}:{}@", password.unwrap_or_default()),
    };
    let port = parsed.port().map(|port| format!(":{port}")).unwrap_or_default();
    Ok(format!("{}://{credentials}{host}{port}", parsed.scheme()))
}

/// RFC 9110 token characters — what a header name may contain.
fn is_token_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)
}

/// The link-local address cloud providers answer instance credentials on. Agentty runs on
/// desktops, where nothing listens there — but a plugin has no business asking either.
fn is_metadata_host(url: &url::Url) -> bool {
    match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.octets()[..2] == [169, 254],
        Some(url::Host::Ipv6(ip)) => ip.segments()[0] & 0xffc0 == 0xfe80,
        Some(url::Host::Domain(name)) => name.eq_ignore_ascii_case("metadata.google.internal"),
        None => false,
    }
}

/// Sends the request. Blocking: callers run it off the main thread.
pub fn fetch(request: &FetchRequest) -> Result<FetchResponse> {
    let checked = check(request)?;
    let mut builder = crate::http::agent_builder().timeout(checked.timeout).redirects(MAX_REDIRECTS);
    if let Some(proxy) = &checked.proxy {
        builder = builder.proxy(ureq::Proxy::new(proxy).map_err(|_| anyhow::anyhow!("that proxy address cannot be used"))?);
    }
    let agent = builder.build();
    let started = Instant::now();
    let mut call = agent.request_url(&checked.method, &checked.url);
    for (name, value) in &checked.headers {
        call = call.set(name, value);
    }
    let result = match &checked.body {
        Some(body) => call.send_string(body),
        None => call.call(),
    };
    let response = match result {
        Ok(response) => response,
        // A 4xx / 5xx is an answer, not a failure: a REST client shows it like any other.
        Err(ureq::Error::Status(_, response)) => response,
        Err(err) => bail!("{err}"),
    };
    let status = response.status();
    let status_text = response.status_text().to_string();
    let url = response.get_url().to_string();
    let headers: BTreeMap<String, String> = response
        .headers_names()
        .into_iter()
        .filter_map(|name| {
            let value = response.header(&name)?;
            Some((name.to_ascii_lowercase(), value.chars().take(MAX_HEADER_VALUE).collect()))
        })
        .collect();
    // One byte past the limit, so a body of exactly the limit is not reported as cut off.
    let mut bytes = Vec::new();
    response.into_reader().take(MAX_RESPONSE_BODY as u64 + 1).read_to_end(&mut bytes)?;
    let truncated = bytes.len() > MAX_RESPONSE_BODY;
    bytes.truncate(MAX_RESPONSE_BODY);
    let binary = std::str::from_utf8(&bytes).is_err();
    Ok(FetchResponse {
        status,
        status_text,
        url,
        headers,
        body: String::from_utf8_lossy(&bytes).into_owned(),
        truncated,
        binary,
        bytes: bytes.len(),
        duration_ms: started.elapsed().as_millis() as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(value: serde_json::Value) -> FetchRequest {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn defaults_to_a_get_with_a_timeout() {
        let checked = check(&request(serde_json::json!({ "url": "https://example.com/a?b=1" }))).unwrap();
        assert_eq!(checked.method, "GET");
        assert_eq!(checked.timeout, DEFAULT_TIMEOUT);
        assert!(checked.body.is_none());
        // A timeout a plugin asks for is kept inside bounds rather than refused.
        let long = check(&request(serde_json::json!({ "url": "https://example.com", "timeoutMs": 600_000 }))).unwrap();
        assert_eq!(long.timeout, MAX_TIMEOUT);
    }

    #[test]
    fn refuses_what_would_smuggle_a_second_request() {
        for bad in [
            serde_json::json!({ "url": "https://example.com", "headers": { "X-A": "one\r\nHost: evil.example" } }),
            serde_json::json!({ "url": "https://example.com", "headers": { "Host": "evil.example" } }),
            serde_json::json!({ "url": "https://example.com", "headers": { "Content-Length": "0" } }),
            serde_json::json!({ "url": "https://example.com", "headers": { "X A": "v" } }),
            serde_json::json!({ "url": "https://example.com/ HTTP/1.1" }),
            serde_json::json!({ "url": "https://example.com\nGET /x" }),
            serde_json::json!({ "url": "file:///etc/passwd" }),
            serde_json::json!({ "url": "agentty://plugin/x" }),
            serde_json::json!({ "url": "http://169.254.169.254/latest/meta-data/" }),
            serde_json::json!({ "url": "https://example.com", "method": "CONNECT" }),
            serde_json::json!({ "url": "https://example.com", "method": "TRACE" }),
        ] {
            assert!(check(&request(bad.clone())).is_err(), "accepted {bad}");
        }
    }

    #[test]
    fn a_proxy_is_checked_like_the_url() {
        let ok = check(&request(serde_json::json!({ "url": "https://example.com", "proxy": "http://127.0.0.1:8888" }))).unwrap();
        assert_eq!(ok.proxy.as_deref(), Some("http://127.0.0.1:8888"));
        // Credentials are kept, since that is how a proxy asks for them.
        let creds = check_proxy("http://user:example_not_a_real_password@proxy.example:3128").unwrap();
        assert!(creds.starts_with("http://user:"));
        for bad in ["socks5://127.0.0.1:1080", "ftp://proxy.example", "127.0.0.1:8888", "http://", "http://proxy.example\r\nX: y"] {
            assert!(check_proxy(bad).is_err(), "accepted {bad}");
        }
        // Nothing asked for, nothing used.
        assert!(check(&request(serde_json::json!({ "url": "https://example.com" }))).unwrap().proxy.is_none());
    }

    #[test]
    fn caps_bodies_and_header_counts() {
        let big = "x".repeat(MAX_REQUEST_BODY + 1);
        assert!(check(&request(serde_json::json!({ "url": "https://example.com", "method": "POST", "body": big }))).is_err());
        assert!(check(&request(serde_json::json!({ "url": "https://example.com", "body": "no" }))).is_err(), "a GET has no body");
        let many: serde_json::Map<String, serde_json::Value> =
            (0..MAX_HEADERS + 1).map(|i| (format!("X-H{i}"), serde_json::json!("v"))).collect();
        assert!(check(&request(serde_json::json!({ "url": "https://example.com", "headers": many }))).is_err());
        // The everyday case still passes.
        let ok = check(&request(serde_json::json!({
            "url": "https://example.com/api",
            "method": "post",
            "headers": { "Authorization": "Bearer example_not_a_real_token", "Content-Type": "application/json" },
            "body": "{\"a\":1}"
        })))
        .unwrap();
        assert_eq!(ok.method, "POST");
        assert_eq!(ok.headers.len(), 2);
    }
}
