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
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::sync::Arc;
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

/// [`check`] for the tests in the crate that cover what a plugin can send.
#[cfg(test)]
pub(super) fn check_for_test(request: &FetchRequest) -> Result<()> {
    check(request).map(|_| ())
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
///
/// This is the check on the address as written, so an obvious refusal comes back before anything
/// is sent. What a name resolves to is checked where the connection is made ([`guarded`]).
fn is_metadata_host(url: &url::Url) -> bool {
    match url.host() {
        Some(url::Host::Ipv4(ip)) => is_forbidden_ip(IpAddr::V4(ip)),
        Some(url::Host::Ipv6(ip)) => is_forbidden_ip(IpAddr::V6(ip)),
        Some(url::Host::Domain(name)) => {
            let name = name.trim_end_matches('.');
            name.eq_ignore_ascii_case("metadata.google.internal") || name.eq_ignore_ascii_case("metadata")
        }
        None => false,
    }
}

/// An address a plugin may not connect to: link-local (`169.254.0.0/16`, `fe80::/10`), where
/// cloud providers answer instance credentials, and AWS's IPv6 metadata address.
///
/// An IPv6 address that carries an IPv4 one inside it (`::ffff:169.254.169.254`, the old
/// `::a9fe:a9fe`, NAT64's `64:ff9b::a9fe:a9fe`) is that IPv4 address as far as the network is
/// concerned, so it is that one here too: checking only the IPv6 prefix would let a plugin write
/// the same address a different way and go straight past this.
///
/// Loopback and private ranges stay reachable: plugins talk to local dev servers and to machines
/// on the user's network, and every request already needs the `net.request` permission the user
/// granted. (Asking the manifest to name them as well would break every plugin that does so
/// today, so it is not done.)
fn is_forbidden_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ip.octets()[..2] == [169, 254],
        IpAddr::V6(ip) => {
            let segments = ip.segments();
            let nat64 = segments[..6] == [0x64, 0xff9b, 0, 0, 0, 0];
            if let Some(v4) = ip.to_ipv4_mapped().or_else(|| ip.to_ipv4()) {
                return is_forbidden_ip(IpAddr::V4(v4));
            }
            if nat64 {
                let [a, b] = segments[6].to_be_bytes();
                let [c, d] = segments[7].to_be_bytes();
                return is_forbidden_ip(IpAddr::V4(Ipv4Addr::new(a, b, c, d)));
            }
            segments[0] & 0xffc0 == 0xfe80 || ip == AWS_METADATA_V6
        }
    }
}

/// AWS's instance metadata service on IPv6.
const AWS_METADATA_V6: Ipv6Addr = Ipv6Addr::new(0xfd00, 0xec2, 0, 0, 0, 0, 0, 0x254);

/// How names are looked up: `host:port` in, addresses out. The real one asks the operating system;
/// the tests hand in their own.
type Lookup = Arc<dyn Fn(&str) -> std::io::Result<Vec<SocketAddr>> + Send + Sync>;

fn system_lookup() -> Lookup {
    Arc::new(|netloc: &str| netloc.to_socket_addrs().map(Iterator::collect))
}

/// The resolver the HTTP client connects through: it looks the name up, leaves out every address
/// in the answer a plugin may not reach, and hands the client exactly the addresses it checked —
/// refusing only when none is left. A name with a link-local address next to a routable one
/// (an mDNS `.local` host, `localhost` with `fe80::1%lo0` in `/etc/hosts`) is reached through the
/// routable one; the forbidden one is never connected to.
///
/// Checking here rather than before the request is what closes the gap a name leaves: a name that
/// resolves to `169.254.169.254` passes every check on the URL's text, and a name looked up once
/// for the check and again for the connection can answer differently the second time (DNS
/// rebinding). The client connects only to what this returns, so what was checked is what is
/// connected to — on the first request and on every redirect hop.
///
/// Through a proxy the client resolves the proxy's address, which is checked the same way; the
/// address the request is for is then looked up by the proxy, which the plugin chose.
fn guarded(lookup: Lookup) -> impl ureq::Resolver {
    move |netloc: &str| -> std::io::Result<Vec<SocketAddr>> {
        let addresses = reachable(lookup(netloc)?);
        if addresses.is_empty() {
            return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, NOT_REACHABLE));
        }
        Ok(addresses)
    }
}

const NOT_REACHABLE: &str = "that address is not reachable from a plugin";

/// The addresses of an answer a plugin may connect to, in the order given.
fn reachable(mut addresses: Vec<SocketAddr>) -> Vec<SocketAddr> {
    addresses.retain(|address| !is_forbidden_ip(address.ip()));
    addresses
}

/// The client for one request: Agentty's TLS, a deadline, no redirects of its own (see [`send`]),
/// the plugin's proxy if it named one, and names resolved through [`guarded`].
fn agent(checked: &Checked, lookup: Lookup) -> Result<ureq::Agent> {
    let mut builder = crate::http::agent_builder().timeout(checked.timeout).redirects(0).resolver(guarded(lookup));
    if let Some(proxy) = &checked.proxy {
        builder = builder.proxy(ureq::Proxy::new(proxy).map_err(|_| anyhow::anyhow!("that proxy address cannot be used"))?);
    }
    Ok(builder.build())
}

/// Whether a status is one that names another address to go to.
fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

/// Sends the request, following redirects here rather than leaving them to the client.
///
/// It matters which: the checks above refuse a link-local or metadata address, and a server that
/// answers `302 Location: http://169.254.169.254/…` would walk straight through them if the
/// client followed on its own. Every hop goes through the same check as the address the plugin
/// asked for, and what was meant for one host — the authorization it was given, its cookies — is
/// not carried to another.
fn send(agent: &ureq::Agent, checked: &Checked) -> Result<ureq::Response> {
    let mut url = checked.url.clone();
    let mut method = checked.method.clone();
    let mut body = checked.body.clone();
    let mut headers = checked.headers.clone();
    let mut hops = 0;
    loop {
        let mut call = agent.request_url(&method, &url);
        for (name, value) in &headers {
            call = call.set(name, value);
        }
        let result = match &body {
            Some(body) => call.send_string(body),
            None => call.call(),
        };
        let response = match result {
            Ok(response) => response,
            // A 4xx / 5xx is an answer, not a failure: a REST client shows it like any other.
            Err(ureq::Error::Status(_, response)) => response,
            // The resolver's refusal, said plainly rather than as a DNS failure.
            Err(err) if refused_by_resolver(&err) => bail!("{NOT_REACHABLE}"),
            Err(err) => bail!("{err}"),
        };
        let status = response.status();
        let Some(location) = response.header("location").map(str::to_string).filter(|_| is_redirect(status)) else {
            return Ok(response);
        };
        hops += 1;
        if hops > MAX_REDIRECTS {
            bail!("the address redirected more than {MAX_REDIRECTS} times");
        }
        let next = url.join(location.trim()).map_err(|e| anyhow::anyhow!("the redirect names an address that cannot be read: {e}"))?;
        if !matches!(next.scheme(), "http" | "https") {
            bail!("a redirect to {} is not followed", next.scheme());
        }
        if next.host().is_none() {
            bail!("the redirect has no host");
        }
        if is_metadata_host(&next) {
            bail!("{NOT_REACHABLE}");
        }
        if next.host_str() != url.host_str() || next.port_or_known_default() != url.port_or_known_default() {
            headers.retain(|(name, _)| !matches!(name.to_ascii_lowercase().as_str(), "authorization" | "cookie" | "proxy-authorization"));
        }
        // 303 is always read as "go and GET this"; 301 and 302 are, in practice, for anything that
        // was not already a GET. 307 and 308 keep the method and the body, which is their point.
        if status == 303 || (matches!(status, 301 | 302) && !matches!(method.as_str(), "GET" | "HEAD")) {
            method = "GET".to_string();
            body = None;
        }
        url = next;
    }
}

fn refused_by_resolver(err: &ureq::Error) -> bool {
    use std::error::Error as _;
    let mut source = err.source();
    while let Some(cause) = source {
        if cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::PermissionDenied && io.to_string() == NOT_REACHABLE)
        {
            return true;
        }
        source = cause.source();
    }
    false
}

/// Sends the request. Blocking: callers run it off the main thread.
pub fn fetch(request: &FetchRequest) -> Result<FetchResponse> {
    fetch_with(request, system_lookup())
}

fn fetch_with(request: &FetchRequest, lookup: Lookup) -> Result<FetchResponse> {
    let checked = check(request)?;
    let agent = agent(&checked, lookup)?;
    let started = Instant::now();
    let response = send(&agent, &checked)?;
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

/// How long a download may take, whatever the plugin asks: a video is not a request.
const MAX_DOWNLOAD_TIME: Duration = Duration::from_secs(15 * 60);

/// What `files/download` answers.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Downloaded {
    pub status: u16,
    pub url: String,
    pub content_type: Option<String>,
    pub bytes: u64,
    pub duration_ms: u64,
}

/// Fetches `request` (a GET, the same checks as `fetch`) into the file `to`, streamed, at most
/// `max_bytes`. The file appears only once it is whole: a download that fails or is cut off
/// leaves nothing behind, not half a picture.
pub fn download(request: &FetchRequest, to: &std::path::Path, max_bytes: u64) -> Result<Downloaded> {
    download_with(request, to, max_bytes, system_lookup())
}

fn download_with(request: &FetchRequest, to: &std::path::Path, max_bytes: u64, lookup: Lookup) -> Result<Downloaded> {
    let mut checked = check(request)?;
    if checked.method != "GET" {
        bail!("a download is a GET");
    }
    checked.timeout = request.timeout_ms.map(Duration::from_millis).unwrap_or(MAX_DOWNLOAD_TIME).min(MAX_DOWNLOAD_TIME);
    let agent = agent(&checked, lookup)?;
    let started = Instant::now();
    let response = send(&agent, &checked)?;
    let status = response.status();
    if !(200..300).contains(&status) {
        bail!("HTTP {status}");
    }
    if let Some(length) = response.header("content-length").and_then(|v| v.parse::<u64>().ok()) {
        if length > max_bytes {
            bail!("the file is {length} bytes, over the {max_bytes} allowed");
        }
    }
    let url = response.get_url().to_string();
    let content_type = response.header("content-type").map(|v| v.chars().take(MAX_HEADER_VALUE).collect());
    let partial = to.with_extension(format!("{}part", to.extension().map(|e| format!("{}.", e.to_string_lossy())).unwrap_or_default()));
    let result = (|| -> Result<u64> {
        let mut file = crate::fsutil::create_private(&partial)?;
        let mut reader = response.into_reader().take(max_bytes + 1);
        let copied = std::io::copy(&mut reader, &mut file)?;
        if copied > max_bytes {
            bail!("the file is over the {max_bytes} bytes allowed");
        }
        file.sync_all()?;
        Ok(copied)
    })();
    match result {
        Ok(bytes) => {
            std::fs::rename(&partial, to)?;
            Ok(Downloaded { status, url, content_type, bytes, duration_ms: started.elapsed().as_millis() as u64 })
        }
        Err(err) => {
            let _ = std::fs::remove_file(&partial);
            Err(err)
        }
    }
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

    /// A server of one connection at a time that answers from a script of
    /// `(status line, headers, body)`, so the redirect rules can be tested against real sockets.
    struct Server {
        port: u16,
        seen: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl Server {
        fn answering(script: Vec<String>) -> Self {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let recorded = seen.clone();
            std::thread::spawn(move || {
                for (index, stream) in listener.incoming().enumerate() {
                    let Ok(mut stream) = stream else { return };
                    // Read the whole request — headers, then a body if it said it had one.
                    // Answering before the client has finished sending breaks the connection it
                    // is still writing into.
                    let mut request = Vec::new();
                    let mut byte = [0u8; 1];
                    while !request.ends_with(b"\r\n\r\n") {
                        match std::io::Read::read(&mut stream, &mut byte) {
                            Ok(1) => request.push(byte[0]),
                            _ => break,
                        }
                    }
                    let head = String::from_utf8_lossy(&request).to_ascii_lowercase();
                    let length = head
                        .split("\r\n")
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .and_then(|value| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    let mut body = vec![0u8; length];
                    if length > 0 && std::io::Read::read_exact(&mut stream, &mut body).is_ok() {
                        request.extend_from_slice(&body);
                    }
                    recorded.lock().unwrap().push(String::from_utf8_lossy(&request).into_owned());
                    let Some(answer) = script.get(index) else { return };
                    let _ = std::io::Write::write_all(&mut stream, answer.as_bytes());
                    let _ = std::io::Write::flush(&mut stream);
                }
            });
            Self { port, seen }
        }

        fn url(&self, path: &str) -> String {
            format!("http://127.0.0.1:{}{path}", self.port)
        }

        fn requests(&self) -> Vec<String> {
            self.seen.lock().unwrap().clone()
        }
    }

    fn redirect_to(target: &str) -> String {
        format!("HTTP/1.1 302 Found\r\nLocation: {target}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
    }

    fn ok(body: &str) -> String {
        format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
    }

    #[test]
    fn the_metadata_address_is_refused_however_it_is_written() {
        // The same address, spelled four ways. Checking only the IPv6 prefix let the third and
        // fourth through, and the operating system connects to 169.254.169.254 for all of them.
        for written in [
            "http://169.254.169.254/latest/meta-data/",
            "http://169.254.0.1/",
            "http://[::ffff:169.254.169.254]/latest/meta-data/",
            "http://[::ffff:a9fe:a9fe]/",
            "http://metadata.google.internal/computeMetadata/v1/",
            "http://metadata.google.internal./computeMetadata/v1/",
        ] {
            let Err(err) = check(&request(serde_json::json!({ "url": written }))) else {
                panic!("{written} was allowed");
            };
            assert!(format!("{err:#}").contains("not reachable from a plugin"), "{written}: {err:#}");
        }
        // And an ordinary address is still ordinary.
        for fine in ["https://example.com/", "http://[2606:4700::1111]/", "http://169.253.1.1/"] {
            assert!(check(&request(serde_json::json!({ "url": fine }))).is_ok(), "{fine}");
        }
    }

    #[test]
    fn a_redirect_to_an_address_a_plugin_may_not_ask_for_is_not_followed() {
        // The check on the first address is worth nothing if a server can name the second.
        let server = Server::answering(vec![redirect_to("http://169.254.169.254/latest/meta-data/")]);
        let err = fetch(&request(serde_json::json!({ "url": server.url("/go") }))).unwrap_err();
        assert!(format!("{err:#}").contains("not reachable from a plugin"), "{err:#}");
        assert_eq!(server.requests().len(), 1, "nothing beyond the first request was sent");
    }

    #[test]
    fn a_redirect_to_another_scheme_is_not_followed() {
        for target in ["file:///etc/passwd", "ftp://example.com/x"] {
            let server = Server::answering(vec![redirect_to(target)]);
            let err = fetch(&request(serde_json::json!({ "url": server.url("/go") }))).unwrap_err();
            assert!(format!("{err:#}").contains("is not followed"), "{target}: {err:#}");
        }
    }

    #[test]
    fn a_redirect_is_followed_and_the_answer_is_the_one_it_led_to() {
        let server = Server::answering(vec![redirect_to("/there"), ok("arrived")]);
        let response = fetch(&request(serde_json::json!({ "url": server.url("/here") }))).unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, "arrived");
        assert!(server.requests()[1].starts_with("GET /there "), "{:?}", server.requests()[1]);
    }

    #[test]
    fn authorization_is_not_carried_to_another_host() {
        // Two servers: the first hands the second a redirect, and what the plugin was given for
        // the first must not arrive at the second.
        let second = Server::answering(vec![ok("ok")]);
        let first = Server::answering(vec![redirect_to(&second.url("/x"))]);
        let response = fetch(&request(serde_json::json!({
            "url": first.url("/go"),
            "headers": { "Authorization": "Bearer example_not_a_real_token", "X-Trace": "kept" },
        })))
        .unwrap();
        assert_eq!(response.status, 200);
        let arrived = &second.requests()[0];
        assert!(!arrived.to_ascii_lowercase().contains("authorization"), "{arrived}");
        // Everything else still travels: only what is a credential is dropped.
        assert!(arrived.contains("X-Trace: kept"), "{arrived}");
    }

    #[test]
    fn a_post_that_is_redirected_becomes_a_get_without_its_body() {
        let server = Server::answering(vec![redirect_to("/there"), ok("done")]);
        let response = fetch(&request(serde_json::json!({
            "url": server.url("/here"),
            "method": "POST",
            "body": "a=1",
        })))
        .unwrap();
        assert_eq!(response.status, 200);
        let second = &server.requests()[1];
        assert!(second.starts_with("GET /there "), "{second}");
        assert!(!second.contains("a=1"), "{second}");
    }

    #[test]
    fn a_loop_of_redirects_ends() {
        let script: Vec<String> = (0..MAX_REDIRECTS as usize + 2).map(|_| redirect_to("/round")).collect();
        let server = Server::answering(script);
        let err = fetch(&request(serde_json::json!({ "url": server.url("/round") }))).unwrap_err();
        assert!(format!("{err:#}").contains("redirected more than"), "{err:#}");
    }

    /// A resolver that knows the names it is given and nothing else; an address written as one
    /// (`127.0.0.1:8080`) resolves to itself. Counts the lookups it answered.
    fn fake_dns(names: &[(&str, &[&str])]) -> (Lookup, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        let names: Vec<(String, Vec<IpAddr>)> =
            names.iter().map(|(name, ips)| (name.to_string(), ips.iter().map(|ip| ip.parse().unwrap()).collect())).collect();
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = calls.clone();
        let lookup: Lookup = Arc::new(move |netloc: &str| {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if let Ok(address) = netloc.parse::<SocketAddr>() {
                return Ok(vec![address]);
            }
            let (host, port) = netloc.rsplit_once(':').unwrap();
            let port: u16 = port.parse().unwrap();
            match names.iter().find(|(name, _)| name == host) {
                Some((_, ips)) => Ok(ips.iter().map(|ip| SocketAddr::new(*ip, port)).collect()),
                None => Err(std::io::Error::new(std::io::ErrorKind::NotFound, "no such name")),
            }
        });
        (lookup, calls)
    }

    #[test]
    fn a_name_that_resolves_to_the_metadata_address_is_refused() {
        // Every one of these passes the checks on the URL's text: it is the answer that is wrong.
        for answer in [
            &["169.254.169.254"][..],
            &["::ffff:169.254.169.254"],
            &["::ffff:a9fe:a9fe"],
            &["64:ff9b::a9fe:a9fe"],
            &["fe80::1"],
            &["fd00:ec2::254"],
            // Nothing left once the forbidden ones are out.
            &["169.254.169.254", "fe80::1", "fd00:ec2::254"],
        ] {
            let (lookup, calls) = fake_dns(&[("sneaky.example", answer)]);
            let err = fetch_with(&request(serde_json::json!({ "url": "http://sneaky.example/latest/meta-data/" })), lookup).unwrap_err();
            assert_eq!(format!("{err:#}"), NOT_REACHABLE, "{answer:?}");
            assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1, "{answer:?}");
        }
    }

    #[test]
    fn the_connection_goes_to_the_address_that_was_checked() {
        // `api.example` exists only in the fake resolver, so the request can only have arrived
        // through the address it handed out — and a name for a loopback dev server is allowed.
        let server = Server::answering(vec![ok("pinned")]);
        let (lookup, calls) = fake_dns(&[("api.example", &["127.0.0.1"])]);
        let response = fetch_with(&request(serde_json::json!({ "url": format!("http://api.example:{}/x", server.port) })), lookup).unwrap();
        assert_eq!(response.body, "pinned");
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1, "looked up once, for the connection");
        let arrived = server.requests()[0].to_ascii_lowercase();
        assert!(arrived.contains(&format!("host: api.example:{}", server.port)), "{arrived}");
    }

    #[test]
    fn a_link_local_address_next_to_a_routable_one_is_left_out() {
        // mDNS hosts answer with their fe80:: address too, and many Macs list `fe80::1%lo0` for
        // localhost: the request goes to the routable address, never to the link-local one.
        let server = Server::answering(vec![ok("reached"), ok("reached")]);
        for answer in [&["fe80::1", "127.0.0.1"][..], &["169.254.169.254", "127.0.0.1", "fd00:ec2::254"]] {
            let (lookup, _) = fake_dns(&[("printer.local", answer)]);
            let response =
                fetch_with(&request(serde_json::json!({ "url": format!("http://printer.local:{}/", server.port) })), lookup).unwrap();
            assert_eq!(response.body, "reached", "{answer:?}");
        }
        let checked = reachable(
            ["[fe80::1]:80", "10.0.0.2:80", "169.254.1.1:80", "[::1]:80"].iter().map(|address| address.parse().unwrap()).collect(),
        );
        assert_eq!(checked, vec!["10.0.0.2:80".parse::<SocketAddr>().unwrap(), "[::1]:80".parse().unwrap()]);
    }

    #[test]
    fn a_redirect_to_a_name_that_resolves_to_the_metadata_address_is_not_followed() {
        let server = Server::answering(vec![redirect_to("http://sneaky.example/latest/meta-data/")]);
        let (lookup, _) = fake_dns(&[("sneaky.example", &["169.254.169.254"])]);
        let err = fetch_with(&request(serde_json::json!({ "url": server.url("/go") })), lookup).unwrap_err();
        assert_eq!(format!("{err:#}"), NOT_REACHABLE);
        assert_eq!(server.requests().len(), 1, "nothing beyond the first request was sent");
    }

    #[test]
    fn a_name_that_answers_differently_the_second_time_is_checked_again() {
        // DNS rebinding: the first lookup is harmless, the next one is not. Each connection is
        // checked against the answer it is made to, so the second one never happens.
        let server = Server::answering(vec![redirect_to("/again"), ok("should not arrive")]);
        let port = server.port;
        let answers = std::sync::Mutex::new(vec!["169.254.169.254", "127.0.0.1"]);
        let lookup: Lookup = Arc::new(move |_: &str| {
            let ip: IpAddr = answers.lock().unwrap().pop().unwrap_or("169.254.169.254").parse().unwrap();
            Ok(vec![SocketAddr::new(ip, port)])
        });
        let err = fetch_with(&request(serde_json::json!({ "url": format!("http://rebind.example:{port}/") })), lookup).unwrap_err();
        assert_eq!(format!("{err:#}"), NOT_REACHABLE);
        assert_eq!(server.requests().len(), 1);
    }

    #[test]
    fn a_download_from_a_name_that_resolves_to_the_metadata_address_is_refused() {
        let to = std::env::temp_dir().join(format!("agentty-net-test-{}.bin", std::process::id()));
        let (lookup, _) = fake_dns(&[("sneaky.example", &["169.254.169.254"])]);
        let err = download_with(&request(serde_json::json!({ "url": "http://sneaky.example/a.png" })), &to, 1024, lookup).unwrap_err();
        assert_eq!(format!("{err:#}"), NOT_REACHABLE);
        assert!(!to.exists());
    }

    #[test]
    fn local_and_private_addresses_stay_reachable() {
        for fine in
            ["127.0.0.1", "10.0.0.5", "192.168.1.20", "172.16.0.1", "::1", "fd00::1", "fd00:ec2::253", "2606:4700::1111", "169.253.1.1"]
        {
            assert!(!is_forbidden_ip(fine.parse().unwrap()), "{fine}");
        }
    }
}
