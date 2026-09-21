//! Capture proxy (Monitoring → Proxy): a forward HTTP proxy on the loopback interface that records
//! what tabs talk to — which host, how much, for how long.
//!
//! - Tabs opened **while capture is on** get `HTTPS_PROXY` / `HTTP_PROXY` pointing here. The user name
//!   in that URL names the pane, the password is a random token made at startup: records say which
//!   tab a connection came from, and nothing else on the machine can use the proxy (407 otherwise).
//! - `CONNECT` (every HTTPS request) is tunnelled untouched: the content stays encrypted, only the
//!   host, port, byte counts and timing are seen. No certificate is installed, nothing is decrypted.
//! - Plain `http://` requests are forwarded one per connection; method, path and status are recorded,
//!   never headers or bodies.
//! - Records live in memory only (a bounded ring) and are gone when the app quits.
//! - The listener keeps forwarding for as long as the app runs once it started: panes that were given
//!   the proxy would lose their network otherwise. Stopping capture only stops recording.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, Shutdown, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const MAX_RECORDS: usize = 5000;
const MAX_HEAD: usize = 64 * 1024;
const MAX_CONNECTIONS: usize = 512;
/// Longest head kept per direction when headers are recorded.
const MAX_RECORDED_HEAD: usize = 8 * 1024;

/// Whether plain-HTTP heads are recorded (off unless the user turns it on).
static RECORD_HEADS: AtomicBool = AtomicBool::new(false);

/// While the machine's own proxy settings point here (Monitoring → Proxy → capture everything),
/// connections without this run's token are served too and recorded as coming from the system.
/// The listener is on the loopback interface either way, so this grants no process network access
/// it does not already have — it only lets their traffic be listed.
static ALLOW_SYSTEM: AtomicBool = AtomicBool::new(false);

pub fn allows_system() -> bool {
    ALLOW_SYSTEM.load(Ordering::Relaxed)
}

pub fn set_allow_system(on: bool) {
    ALLOW_SYSTEM.store(on, Ordering::Relaxed);
}

pub fn records_heads() -> bool {
    RECORD_HEADS.load(Ordering::Relaxed)
}

pub fn set_record_heads(on: bool) {
    RECORD_HEADS.store(on, Ordering::Relaxed);
}

/// A head as it is safe to keep: the values of headers that carry credentials are replaced.
/// Bodies are never read, so a recorded head stops at the blank line.
pub fn redact_head(head: &str) -> String {
    let secret = |name: &str| {
        let name = name.trim().to_ascii_lowercase();
        // Whole names first: a credential header is not always spelled with hyphens. `apikey` is
        // what Supabase's own client sends, and plain `token` / `secret` are common enough.
        matches!(
            name.as_str(),
            "authorization" | "proxy-authorization" | "cookie" | "set-cookie" | "apikey" | "token" | "secret" | "password" | "session"
        ) || name.ends_with("-token")
            || name.ends_with("-key")
            || name.ends_with("-secret")
            || name.ends_with("-password")
            || name.ends_with("_token")
            || name.ends_with("_key")
            || name.ends_with("_secret")
            || name.contains("api-key")
            || name.contains("apikey")
            || name.contains("auth")
            || name.contains("credential")
    };
    let mut out = String::new();
    for (index, line) in head.split("\r\n").take_while(|l| !l.is_empty()).enumerate() {
        // Checked before the line, not after one particular kind of line: a header block can be
        // tens of kilobytes of cookies, and every record keeps one of these.
        if out.len() > MAX_RECORDED_HEAD {
            out.push_str("…\n");
            break;
        }
        match line.split_once(':').filter(|_| index > 0) {
            Some((name, _)) if secret(name) => {
                out.push_str(name);
                out.push_str(": ***\n");
            }
            Some((name, value)) => {
                out.push_str(name);
                out.push_str(": ");
                out.push_str(value.trim());
                out.push('\n');
            }
            None => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    out
}
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const HEAD_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub id: u64,
    pub started_ms: u64,
    /// Pane the connection came from (from the proxy user name).
    pub pane: Option<u64>,
    /// `CONNECT` for tunnels (HTTPS), else the HTTP method.
    pub method: String,
    pub host: String,
    pub port: u16,
    /// Path of a plain HTTP request (query values masked); empty for tunnels.
    pub path: String,
    /// Status of a plain HTTP response.
    pub status: Option<u16>,
    pub sent: u64,
    pub received: u64,
    /// `None` while the connection is open.
    pub duration_ms: Option<u64>,
    pub error: Option<String>,
    /// Request head of a plain HTTP request, with credentials redacted. Only kept while "record
    /// headers" is on, and never for `CONNECT`: an HTTPS tunnel is not read.
    pub request_head: Option<String>,
    /// Response head of that request, the same way.
    pub response_head: Option<String>,
}

impl Record {
    pub fn endpoint(&self) -> String {
        match (self.method.as_str(), self.port) {
            ("CONNECT", 443) | (_, 80) => self.host.clone(),
            _ => format!("{}:{}", self.host, self.port),
        }
    }
}

struct Shared {
    port: u16,
    token: String,
    recording: AtomicBool,
    records: Mutex<VecDeque<Record>>,
    next_id: AtomicU64,
    /// Bumped on every change, so the page only re-reads when there is something new.
    revision: AtomicU64,
    connections: AtomicUsize,
    upstream: Option<Upstream>,
}

/// A proxy the user's environment already requires (a company proxy): connections go through it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Upstream {
    host: String,
    port: u16,
    /// Ready-made `Proxy-Authorization` value, when the URL carried credentials.
    authorization: Option<String>,
}

static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn random_token() -> String {
    // Two v4 UUIDs: 244 random bits from the OS generator.
    format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple())
}

/// Starts recording (and the listener, the first time). Returns the port.
pub fn start() -> std::io::Result<u16> {
    if let Some(shared) = SHARED.get() {
        shared.recording.store(true, Ordering::SeqCst);
        shared.revision.fetch_add(1, Ordering::SeqCst);
        return Ok(shared.port);
    }
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let port = listener.local_addr()?.port();
    let shared = Arc::new(Shared {
        port,
        token: random_token(),
        recording: AtomicBool::new(true),
        records: Mutex::new(VecDeque::new()),
        next_id: AtomicU64::new(1),
        revision: AtomicU64::new(1),
        connections: AtomicUsize::new(0),
        upstream: upstream_from_env(port),
    });
    if SHARED.set(shared.clone()).is_err() {
        return Ok(SHARED.get().map(|s| s.port).unwrap_or(port)); // another thread won the race
    }
    std::thread::Builder::new().name("agentty-capture".into()).spawn(move || {
        for stream in listener.incoming().flatten() {
            if shared.connections.load(Ordering::SeqCst) >= MAX_CONNECTIONS {
                continue; // dropped: the client sees a closed connection
            }
            let shared = shared.clone();
            shared.connections.fetch_add(1, Ordering::SeqCst);
            let spawned = std::thread::Builder::new().name("agentty-capture-conn".into()).spawn({
                let shared = shared.clone();
                move || {
                    serve(stream, &shared);
                    shared.connections.fetch_sub(1, Ordering::SeqCst);
                }
            });
            if spawned.is_err() {
                shared.connections.fetch_sub(1, Ordering::SeqCst);
            }
        }
    })?;
    Ok(port)
}

pub fn stop() {
    if let Some(shared) = SHARED.get() {
        shared.recording.store(false, Ordering::SeqCst);
        shared.revision.fetch_add(1, Ordering::SeqCst);
    }
}

pub fn is_recording() -> bool {
    SHARED.get().is_some_and(|s| s.recording.load(Ordering::SeqCst))
}

/// Port of the listener, once it started.
pub fn port() -> Option<u16> {
    SHARED.get().map(|s| s.port)
}

pub fn revision() -> u64 {
    SHARED.get().map_or(0, |s| s.revision.load(Ordering::SeqCst))
}

/// Records, oldest first.
pub fn records() -> Vec<Record> {
    SHARED.get().and_then(|s| s.records.lock().ok().map(|r| r.iter().cloned().collect())).unwrap_or_default()
}

pub fn clear() {
    if let Some(shared) = SHARED.get() {
        if let Ok(mut records) = shared.records.lock() {
            // Open connections stay listed: they would reappear half-recorded otherwise.
            records.retain(|r| r.duration_ms.is_none());
        }
        shared.revision.fetch_add(1, Ordering::SeqCst);
    }
}

/// Environment for a pane that starts now: the proxy variables while capture is on, nothing otherwise.
pub fn pane_environment(pane_id: u64) -> Vec<(String, String)> {
    let Some(shared) = SHARED.get().filter(|s| s.recording.load(Ordering::SeqCst)) else { return Vec::new() };
    // The proxy protocol itself is plain HTTP, and this hop never leaves the machine (loopback); what
    // travels inside a CONNECT tunnel stays TLS end to end.
    let url = format!("http://pane-{pane_id}:{}@127.0.0.1:{}", shared.token, shared.port); // audit: ok — loopback proxy URL, not a network call
    let mut env = Vec::new();
    for name in ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy"] {
        env.push((name.to_string(), url.clone()));
    }
    // Local servers (the app being built, the in-app browser tools) are reached directly — and so is
    // whatever the user's environment already keeps away from proxies.
    let own = ["NO_PROXY", "no_proxy"].iter().find_map(|name| std::env::var(name).ok().filter(|v| !v.trim().is_empty()));
    let direct = no_proxy_list(own.as_deref());
    for name in ["NO_PROXY", "no_proxy"] {
        env.push((name.to_string(), direct.clone()));
    }
    // Node's own fetch reads the variables only when told to.
    env.push(("NODE_USE_ENV_PROXY".into(), "1".into()));
    env.push(("AGENTTY_CAPTURE".into(), "1".into()));
    env
}

/// `NO_PROXY` for a captured pane: the local addresses, then the user's own entries.
fn no_proxy_list(own: Option<&str>) -> String {
    let mut entries: Vec<String> = ["localhost", "127.0.0.1", "::1"].iter().map(|e| e.to_string()).collect();
    for entry in own.unwrap_or_default().split(',').map(str::trim).filter(|e| !e.is_empty()) {
        if !entries.iter().any(|known| known.eq_ignore_ascii_case(entry)) {
            entries.push(entry.to_string());
        }
    }
    entries.join(",")
}

// -- serving -------------------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
struct Request {
    method: String,
    host: String,
    port: u16,
    /// Origin-form target of a plain request (`/path?query`).
    target: String,
    version: String,
    /// Header lines without the request line, as received.
    headers: Vec<String>,
}

fn header<'a>(headers: &'a [String], name: &str) -> Option<&'a str> {
    headers.iter().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim().eq_ignore_ascii_case(name).then(|| value.trim())
    })
}

/// Parses a proxy request head (`CONNECT host:port` or an absolute-form request).
fn parse_request(head: &str) -> Option<Request> {
    let mut lines = head.split("\r\n");
    let mut parts = lines.next()?.split(' ');
    let (method, target, version) = (parts.next()?, parts.next()?, parts.next()?);
    if !version.starts_with("HTTP/1.") || method.is_empty() || !method.chars().all(|c| c.is_ascii_uppercase()) {
        return None;
    }
    let headers: Vec<String> = lines.take_while(|l| !l.is_empty()).map(str::to_string).collect();
    let (host, port, target) = if method == "CONNECT" {
        let (host, port) = split_host_port(target, 443)?;
        (host, port, String::new())
    } else {
        let rest = target.strip_prefix("http://")?;
        let (authority, path) = match rest.find('/') {
            Some(at) => (&rest[..at], &rest[at..]),
            None => (rest, "/"),
        };
        // Credentials in a URL are for the origin, not for us, and never for a record.
        let authority = authority.rsplit('@').next()?;
        let (host, port) = split_host_port(authority, 80)?;
        (host, port, path.to_string())
    };
    Some(Request { method: method.to_string(), host, port, target, version: version.to_string(), headers })
}

/// `example.com:8443`, `[::1]:3000`, `example.com` → host and port.
fn split_host_port(authority: &str, default_port: u16) -> Option<(String, u16)> {
    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        (host, after.strip_prefix(':'))
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        }
    };
    let port = match port {
        Some(port) => port.parse().ok()?,
        None => default_port,
    };
    let valid = !host.is_empty() && host.len() <= 253 && host.chars().all(|c| c.is_ascii_alphanumeric() || "-._:".contains(c));
    valid.then(|| (host.to_ascii_lowercase(), port))
}

/// Pane id when the `Proxy-Authorization` header carries this run's token.
fn authorized_pane(headers: &[String], token: &str) -> Option<Option<u64>> {
    let value = header(headers, "proxy-authorization")?;
    let encoded = value.strip_prefix("Basic ").or_else(|| value.strip_prefix("basic "))?;
    let decoded = String::from_utf8(base64_decode(encoded.trim())?).ok()?;
    let (user, password) = decoded.split_once(':')?;
    // Same length and every byte compared: no early exit that would leak a prefix through timing.
    let same = password.len() == token.len() && password.bytes().zip(token.bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0;
    same.then(|| user.strip_prefix("pane-").and_then(|id| id.parse().ok()))
}

fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut buffer, mut bits) = (0u32, 0u8);
    for byte in text.bytes().filter(|b| *b != b'=') {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(out)
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = u32::from(chunk[0]) << 16 | u32::from(*chunk.get(1).unwrap_or(&0)) << 8 | u32::from(*chunk.get(2).unwrap_or(&0));
        for (index, shift) in [18u32, 12, 6, 0].into_iter().enumerate() {
            out.push(if index <= chunk.len() { TABLE[((n >> shift) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

/// The proxy the environment of the app asks for, unless that is this proxy.
fn upstream_from_env(own_port: u16) -> Option<Upstream> {
    let url = ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"]
        .iter()
        .find_map(|name| std::env::var(name).ok().filter(|v| !v.is_empty()))?;
    if is_capture_proxy_url(&url) {
        return None; // started from a captured pane of another Agentty: two installs never share traffic
    }
    parse_upstream(&url).filter(|up| !(up.port == own_port && matches!(up.host.as_str(), "127.0.0.1" | "localhost")))
}

/// The shape `pane_environment` gives panes: `http://pane-<id>:<token>@127.0.0.1:<port>`.
fn is_capture_proxy_url(url: &str) -> bool {
    let rest = url.strip_prefix("http://pane-"); // audit: ok — text match on a loopback URL, not a network call
    rest.and_then(|rest| rest.split_once('@')).is_some_and(|(_, authority)| authority.starts_with("127.0.0.1:"))
}

fn parse_upstream(url: &str) -> Option<Upstream> {
    let rest = url.strip_prefix("http://")?.trim_end_matches('/');
    let (credentials, authority) = match rest.rsplit_once('@') {
        Some((credentials, authority)) => (Some(credentials), authority),
        None => (None, rest),
    };
    let (host, port) = split_host_port(authority, 8080)?;
    Some(Upstream { host, port, authorization: credentials.map(|c| format!("Basic {}", base64_encode(c.as_bytes()))) })
}

fn read_head(stream: &mut TcpStream) -> Option<(String, Vec<u8>)> {
    let mut bytes = Vec::with_capacity(2048);
    let mut buffer = [0u8; 4096];
    loop {
        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            let rest = bytes.split_off(end + 4);
            return Some((String::from_utf8_lossy(&bytes).to_string(), rest));
        }
        if bytes.len() > MAX_HEAD {
            return None;
        }
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => return None,
            Ok(n) => bytes.extend_from_slice(&buffer[..n]),
        }
    }
}

fn connect(host: &str, port: u16) -> std::io::Result<TcpStream> {
    let mut last = std::io::Error::new(std::io::ErrorKind::NotFound, "the host has no address");
    for address in (host, port).to_socket_addrs()? {
        match TcpStream::connect_timeout(&address, CONNECT_TIMEOUT) {
            Ok(stream) => return Ok(stream),
            Err(err) => last = err,
        }
    }
    Err(last)
}

/// A connection to `host:port`, through the upstream proxy when there is one.
fn open_tunnel(shared: &Shared, host: &str, port: u16) -> std::io::Result<TcpStream> {
    let Some(upstream) = &shared.upstream else { return connect(host, port) };
    let mut stream = connect(&upstream.host, upstream.port)?;
    let authority = if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") };
    let mut request = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n");
    if let Some(authorization) = &upstream.authorization {
        request.push_str(&format!("Proxy-Authorization: {authorization}\r\n"));
    }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes())?;
    stream.set_read_timeout(Some(HEAD_TIMEOUT))?;
    let (head, _) = read_head(&mut stream).ok_or_else(|| std::io::Error::other("the upstream proxy did not answer"))?;
    stream.set_read_timeout(None)?;
    let ok = head.split(' ').nth(1).is_some_and(|status| status == "200");
    ok.then_some(stream).ok_or_else(|| std::io::Error::other("the upstream proxy refused the connection"))
}

fn serve(mut client: TcpStream, shared: &Arc<Shared>) {
    let _ = client.set_read_timeout(Some(HEAD_TIMEOUT));
    let _ = client.set_nodelay(true);
    let Some((head, rest)) = read_head(&mut client) else { return };
    let Some(request) = parse_request(&head) else {
        let _ = client.write_all(b"HTTP/1.1 400 Bad Request\r\nConnection: close\r\nContent-Length: 0\r\n\r\n");
        return;
    };
    let pane = match authorized_pane(&request.headers, &shared.token) {
        Some(pane) => pane,
        // Anything else on the machine, while the user asked for the machine to be captured.
        None if allows_system() => None,
        None => {
            let _ = client
                .write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic realm=\"Agentty\"\r\nConnection: close\r\nContent-Length: 0\r\n\r\n");
            return;
        }
    };
    let _ = client.set_read_timeout(None);
    let started = Instant::now();
    let record = shared.recording.load(Ordering::SeqCst).then(|| {
        let id = shared.next_id.fetch_add(1, Ordering::SeqCst);
        push(
            shared,
            Record {
                id,
                started_ms: now_ms(),
                pane,
                method: request.method.clone(),
                host: request.host.clone(),
                port: request.port,
                path: display_path(&request.target),
                status: None,
                sent: 0,
                received: 0,
                duration_ms: None,
                error: None,
                request_head: None,
                response_head: None,
            },
        );
        id
    });
    let finish = |sent: u64, received: u64, status: Option<u16>, error: Option<String>| {
        let Some(id) = record else { return };
        update(shared, id, |r| {
            r.sent = sent;
            r.received = received;
            r.status = status.or(r.status);
            r.error = error;
            r.duration_ms = Some(started.elapsed().as_millis() as u64);
        });
    };

    let mut server = match open_tunnel(shared, &request.host, request.port) {
        Ok(server) => server,
        Err(err) => {
            let _ = client.write_all(b"HTTP/1.1 502 Bad Gateway\r\nConnection: close\r\nContent-Length: 0\r\n\r\n");
            return finish(0, 0, Some(502), Some(err.to_string()));
        }
    };
    let _ = server.set_nodelay(true);
    let mut sent = 0u64;
    if request.method == "CONNECT" {
        if client.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").is_err() {
            return finish(0, 0, None, Some("the client went away".into()));
        }
    } else {
        // One request per connection keeps this a relay, not an HTTP implementation: the origin is
        // asked to close after its response, and the client follows. A protocol upgrade (WebSocket)
        // keeps its `Connection: Upgrade` and simply becomes a stream of bytes both ways.
        let upgrade = header(&request.headers, "upgrade").is_some();
        let mut forwarded = format!("{} {} {}\r\n", request.method, request.target, request.version);
        for line in &request.headers {
            let name = line.split(':').next().unwrap_or_default().trim().to_ascii_lowercase();
            let hop = matches!(name.as_str(), "proxy-authorization" | "proxy-connection")
                || (!upgrade && matches!(name.as_str(), "connection" | "keep-alive"));
            if !hop {
                forwarded.push_str(line);
                forwarded.push_str("\r\n");
            }
        }
        forwarded.push_str(if upgrade { "\r\n" } else { "Connection: close\r\n\r\n" });
        if server.write_all(forwarded.as_bytes()).is_err() {
            return finish(0, 0, None, Some("the server went away".into()));
        }
        sent += forwarded.len() as u64;
        if records_heads() {
            if let Some(id) = record {
                // No slicing here: a byte index into a string can fall inside a character, and
                // `redact_head` stops on its own once it has kept enough.
                let head = redact_head(&forwarded);
                update(shared, id, |r| r.request_head = Some(head.clone()));
            }
        }
    }
    if !rest.is_empty() {
        if server.write_all(&rest).is_err() {
            return finish(sent, 0, None, Some("the server went away".into()));
        }
        sent += rest.len() as u64;
    }

    // Two directions, two threads; each ends the other's half when its side is done.
    let (Ok(mut client_read), Ok(mut server_write)) = (client.try_clone(), server.try_clone()) else {
        return finish(sent, 0, None, Some("out of file descriptors".into()));
    };
    let live = record.map(|id| (shared.clone(), id));
    let upload = std::thread::spawn({
        let live = live.clone();
        move || {
            let copied = relay(&mut client_read, &mut server_write, |n| {
                if let Some((shared, id)) = &live {
                    update(shared, *id, |r| r.sent += n);
                }
            });
            let _ = server_write.shutdown(Shutdown::Write);
            copied
        }
    });
    let plain = request.method != "CONNECT";
    let mut status = None;
    let mut first = true;
    let received = relay_with(&mut server, &mut client, |chunk| {
        if plain && std::mem::take(&mut first) {
            status = std::str::from_utf8(&chunk[..chunk.len().min(32)])
                .ok()
                .and_then(|text| text.strip_prefix("HTTP/"))
                .and_then(|text| text.split(' ').nth(1))
                .and_then(|code| code.parse().ok());
            // Only the head: the relay stops reading text at the blank line, the body is untouched.
            if records_heads() {
                if let Some((shared, id)) = &live {
                    // A byte slice, not a string slice: `from_utf8_lossy` takes whatever bytes it
                    // is given, so a cut inside a character cannot panic here.
                    let text = String::from_utf8_lossy(&chunk[..chunk.len().min(MAX_RECORDED_HEAD)]).to_string();
                    let head = redact_head(&text);
                    update(shared, *id, |r| r.response_head = Some(head.clone()));
                }
            }
        }
        if let Some((shared, id)) = &live {
            let (n, status) = (chunk.len() as u64, status);
            update(shared, *id, |r| {
                r.received += n;
                r.status = status.or(r.status);
            });
        }
    });
    let _ = client.shutdown(Shutdown::Both);
    let _ = server.shutdown(Shutdown::Both);
    let uploaded = upload.join().unwrap_or(0);
    finish(sent + uploaded, received, status, None);
}

fn relay(from: &mut TcpStream, to: &mut TcpStream, mut progress: impl FnMut(u64)) -> u64 {
    relay_with(from, to, |chunk| progress(chunk.len() as u64))
}

fn relay_with(from: &mut TcpStream, to: &mut TcpStream, mut seen: impl FnMut(&[u8])) -> u64 {
    let mut buffer = vec![0u8; 32 * 1024];
    let mut total = 0u64;
    loop {
        match from.read(&mut buffer) {
            Ok(0) | Err(_) => return total,
            Ok(n) => {
                if to.write_all(&buffer[..n]).is_err() {
                    return total;
                }
                total += n as u64;
                seen(&buffer[..n]);
            }
        }
    }
}

/// Path for a record: query values are masked (they carry tokens often enough).
fn display_path(target: &str) -> String {
    let Some((path, query)) = target.split_once('?') else { return target.chars().take(300).collect() };
    let masked: Vec<String> = query.split('&').take(20).map(|pair| format!("{}=•••", pair.split('=').next().unwrap_or_default())).collect();
    format!("{}?{}", path.chars().take(300).collect::<String>(), masked.join("&"))
}

fn push(shared: &Shared, record: Record) {
    if let Ok(mut records) = shared.records.lock() {
        if records.len() >= MAX_RECORDS {
            // The oldest finished record goes; open ones are still being counted.
            if let Some(at) = records.iter().position(|r| r.duration_ms.is_some()) {
                records.remove(at);
            } else {
                records.pop_front();
            }
        }
        records.push_back(record);
    }
    shared.revision.fetch_add(1, Ordering::SeqCst);
}

fn update(shared: &Shared, id: u64, change: impl FnOnce(&mut Record)) {
    if let Ok(mut records) = shared.records.lock() {
        if let Some(record) = records.iter_mut().rev().find(|r| r.id == id) {
            change(record);
        }
    }
    shared.revision.fetch_add(1, Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn basic(user: &str, password: &str) -> Vec<String> {
        vec![format!("Proxy-Authorization: Basic {}", base64_encode(format!("{user}:{password}").as_bytes()))]
    }

    #[test]
    fn parses_tunnels_and_plain_requests() {
        let connect = parse_request("CONNECT api.example.com:443 HTTP/1.1\r\nHost: api.example.com:443\r\n\r\n").unwrap();
        assert_eq!((connect.method.as_str(), connect.host.as_str(), connect.port), ("CONNECT", "api.example.com", 443));
        let plain = parse_request("GET http://user:pw@Example.com:8080/a/b?x=1 HTTP/1.1\r\nAccept: */*\r\n\r\n").unwrap();
        assert_eq!((plain.host.as_str(), plain.port, plain.target.as_str()), ("example.com", 8080, "/a/b?x=1"));
        assert_eq!(plain.headers, vec!["Accept: */*".to_string()]);
        assert_eq!(parse_request("GET http://example.com HTTP/1.1\r\n\r\n").unwrap().target, "/");
        assert_eq!(split_host_port("[::1]:3000", 80), Some(("::1".into(), 3000)));
        // Not a proxy request, a bad port, a host with characters that have no business there.
        assert!(parse_request("GET /index.html HTTP/1.1\r\n\r\n").is_none());
        assert!(parse_request("CONNECT example.com:99999 HTTP/1.1\r\n\r\n").is_none());
        assert!(parse_request("CONNECT exa mple.com:443 HTTP/1.1\r\n\r\n").is_none());
        assert!(parse_request("CONNECT evil.com/../x:443 HTTP/1.1\r\n\r\n").is_none());
    }

    #[test]
    fn only_this_runs_token_is_let_through() {
        let token = "token_example_not_a_real_one";
        assert_eq!(authorized_pane(&basic("pane-12", token), token), Some(Some(12)));
        assert_eq!(authorized_pane(&basic("someone", token), token), Some(None));
        assert_eq!(authorized_pane(&basic("pane-12", "wrong_example_value"), token), None);
        assert_eq!(authorized_pane(&basic("pane-12", ""), token), None);
        assert_eq!(authorized_pane(&[], token), None);
        assert_eq!(authorized_pane(&["Proxy-Authorization: Bearer x".into()], token), None);
    }

    #[test]
    fn base64_round_trips() {
        for text in ["", "a", "ab", "abc", "pane-3:fake_value_for_a_test"] {
            assert_eq!(base64_decode(&base64_encode(text.as_bytes())).unwrap(), text.as_bytes());
        }
        assert_eq!(base64_encode(b"user:pass"), "dXNlcjpwYXNz");
        assert!(base64_decode("not base64!").is_none());
    }

    #[test]
    fn local_addresses_and_the_users_own_stay_direct() {
        assert_eq!(no_proxy_list(None), "localhost,127.0.0.1,::1");
        assert_eq!(
            no_proxy_list(Some(" .corp.example.com, LOCALHOST ,10.0.0.0/8,")),
            "localhost,127.0.0.1,::1,.corp.example.com,10.0.0.0/8"
        );
    }

    #[test]
    fn heads_keep_the_shape_but_not_the_credentials() {
        let head = "GET /v1/things HTTP/1.1\r\nHost: api.example.com\r\nAuthorization: Bearer abcdef\r\nCookie: session=xyz\r\nX-Api-Key: 12345\r\nAccept: application/json\r\n\r\n";
        let redacted = redact_head(head);
        assert!(redacted.contains("GET /v1/things HTTP/1.1"));
        assert!(redacted.contains("Host: api.example.com"));
        assert!(redacted.contains("Accept: application/json"));
        assert!(redacted.contains("Authorization: ***"));
        assert!(redacted.contains("Cookie: ***"));
        assert!(redacted.contains("X-Api-Key: ***"));
        assert!(!redacted.contains("abcdef"));
        assert!(!redacted.contains("xyz"));
        assert!(!redacted.contains("12345"));
    }

    /// Credential headers are not all spelled with hyphens: `apikey` is what Supabase's client
    /// sends, and it went through unmasked.
    #[test]
    fn credential_headers_without_hyphens_are_masked_too() {
        let head = "GET /rest/v1/rows HTTP/1.1\r\nHost: project.supabase.example\r\napikey: sb_example_not_a_real_key\r\nToken: example_not_a_real_token\r\nX-Client-Info: supabase-js/2.0\r\n\r\n";
        let redacted = redact_head(head);
        assert!(redacted.contains("apikey: ***"), "{redacted}");
        assert!(redacted.contains("Token: ***"), "{redacted}");
        assert!(!redacted.contains("not_a_real"), "no credential survives: {redacted}");
        // What is not a credential still reads normally.
        assert!(redacted.contains("X-Client-Info: supabase-js/2.0"));
        assert!(redacted.contains("Host: project.supabase.example"));
    }

    /// A long header block is cut off. Before, the size check sat on a branch that an ordinary
    /// `Name: value` line never reached, so a head of any size was kept whole.
    #[test]
    fn a_long_head_is_cut_off() {
        let mut head = String::from("GET / HTTP/1.1\r\nHost: api.example.com\r\n");
        for i in 0..2_000 {
            head.push_str(&format!("X-Trace-{i}: {}\r\n", "v".repeat(40)));
        }
        head.push_str("\r\n");
        let redacted = redact_head(&head);
        assert!(redacted.ends_with("…\n"), "it says it was cut");
        assert!(redacted.len() < MAX_RECORDED_HEAD + 200, "kept {} bytes", redacted.len());
        // What came first is still there.
        assert!(redacted.starts_with("GET / HTTP/1.1\n"));
    }

    #[test]
    fn heads_are_only_recorded_when_asked_for() {
        assert!(!records_heads(), "off unless the user turns it on");
        set_record_heads(true);
        assert!(records_heads());
        set_record_heads(false);
    }

    #[test]
    fn records_never_keep_query_values() {
        assert_eq!(display_path("/v1/items?api_key=example_not_a_real_key&page=2"), "/v1/items?api_key=•••&page=•••");
        assert_eq!(display_path("/plain"), "/plain");
    }

    #[test]
    fn reads_an_upstream_proxy() {
        let upstream = parse_upstream("http://proxy.example.com:3128/").unwrap();
        assert_eq!((upstream.host.as_str(), upstream.port, upstream.authorization), ("proxy.example.com", 3128, None));
        let with_login = parse_upstream("http://user:fake_example_pw@proxy.example.com").unwrap();
        assert_eq!(with_login.port, 8080);
        assert!(with_login.authorization.unwrap().starts_with("Basic "));
        assert!(parse_upstream("socks5://127.0.0.1:1080").is_none());
        // A capture proxy of another Agentty (this process was started from one of its panes) is not chained.
        assert!(is_capture_proxy_url("http://pane-4:fake_token_example@127.0.0.1:50123"));
        assert!(!is_capture_proxy_url("http://pane-4:fake_token_example@proxy.example.com:3128"));
        assert!(!is_capture_proxy_url("http://user:fake_example_pw@127.0.0.1:3128"));
    }

    /// The whole path once: a client that speaks to the proxy, a server behind it.
    #[test]
    fn tunnels_forwards_and_refuses() {
        let origin = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let origin_port = origin.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for mut stream in origin.incoming().flatten().take(2) {
                let (head, _) = read_head(&mut stream).unwrap();
                let body = if head.starts_with("GET /hello") && !head.to_lowercase().contains("proxy-authorization") {
                    "hi"
                } else {
                    "unexpected"
                };
                let _ = stream
                    .write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
            }
        });
        let port = start().unwrap();
        let token = SHARED.get().unwrap().token.clone();
        let authorization = format!("Proxy-Authorization: Basic {}\r\n", base64_encode(format!("pane-7:{token}").as_bytes()));
        let talk = |request: String| {
            let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            stream.write_all(request.as_bytes()).unwrap();
            let mut answer = String::new();
            let _ = stream.read_to_string(&mut answer);
            answer
        };
        // Without the token: refused, and nothing is recorded.
        assert!(talk(format!("GET http://127.0.0.1:{origin_port}/hello HTTP/1.1\r\nHost: x\r\n\r\n")).starts_with("HTTP/1.1 407"));
        assert!(records().iter().all(|r| r.port != origin_port));
        // With it: forwarded without the proxy header, recorded with pane, path and status.
        let answer = talk(format!("GET http://127.0.0.1:{origin_port}/hello?key=example_value HTTP/1.1\r\nHost: x\r\n{authorization}\r\n"));
        assert!(answer.ends_with("hi"), "{answer}");
        // The record is closed just after the client saw the end of the response.
        let finished = || records().into_iter().find(|r| r.port == origin_port && r.duration_ms.is_some());
        let deadline = Instant::now() + Duration::from_secs(5);
        while finished().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let record = finished().expect("the record is finished");
        assert_eq!(
            (record.pane, record.method.as_str(), record.path.as_str(), record.status),
            (Some(7), "GET", "/hello?key=•••", Some(200))
        );
        assert!(record.duration_ms.is_some() && record.received > 0);
        // A tunnel: bytes pass both ways untouched.
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        stream.write_all(format!("CONNECT 127.0.0.1:{origin_port} HTTP/1.1\r\n{authorization}\r\n").as_bytes()).unwrap();
        let (head, _) = read_head(&mut stream).unwrap();
        assert!(head.starts_with("HTTP/1.1 200"));
        stream.write_all(b"GET /hello HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        let mut answer = String::new();
        let _ = stream.read_to_string(&mut answer);
        assert!(answer.ends_with("hi"), "{answer}");
        // Panes started now are pointed here; none of it once capture is off.
        let env = pane_environment(3);
        assert!(env
            .iter()
            .any(|(k, v)| k == "HTTPS_PROXY" && v.starts_with("http://pane-3:") && v.ends_with(&format!("@127.0.0.1:{port}"))));
        stop();
        assert!(pane_environment(3).is_empty());
    }
}
