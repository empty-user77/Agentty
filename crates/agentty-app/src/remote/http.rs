//! The little HTTP/1.1 the remote page needs, read strictly. Requests come only from Tailscale's
//! proxy on this machine, but anything on the machine can open the port too, so a request is
//! refused rather than guessed at: one request per connection, no chunked bodies, no duplicate
//! `Host` or `Content-Length`, bounded head and body, and a deadline for both.

use std::io::{Read, Write};
use std::time::{Duration, Instant};

pub const MAX_HEAD: usize = 16 * 1024;
pub const MAX_BODY: usize = 64 * 1024;
const MAX_HEADERS: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub query: String,
    /// Names lowercased, in the order sent.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpError {
    /// Malformed: answered 400.
    Bad,
    /// Head or body over the limit: 431 / 413.
    HeadTooLarge,
    BodyTooLarge,
    /// A body framing this server does not take (chunked): 501.
    Unsupported,
    /// Its head did not pass `admit` (wrong host or sender), answered with its status; the body is
    /// never read.
    Refused(u16),
    /// The connection closed or timed out: nothing is answered.
    Gone,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }

    /// A query parameter, percent-decoded.
    pub fn query_param(&self, name: &str) -> Option<String> {
        self.query.split('&').filter_map(|pair| pair.split_once('=')).find(|(k, _)| *k == name).and_then(|(_, v)| percent_decode(v))
    }

    /// A cookie's value from the `Cookie` header.
    pub fn cookie(&self, name: &str) -> Option<&str> {
        self.header("cookie")?.split(';').map(str::trim).filter_map(|pair| pair.split_once('=')).find(|(k, _)| *k == name).map(|(_, v)| v)
    }
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = text.get(i + 1..i + 3)?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

/// A stream whose reads can be bounded by a deadline for the whole request, not each read: one
/// byte every few seconds must not hold a connection open (slowloris).
pub trait Deadline: Read {
    /// Bounds the next read to `left`.
    fn limit_next_read(&mut self, left: Duration) -> std::io::Result<()>;
}

impl Deadline for std::net::TcpStream {
    fn limit_next_read(&mut self, left: Duration) -> std::io::Result<()> {
        self.set_read_timeout(Some(left.max(Duration::from_millis(1))))
    }
}

impl Deadline for &[u8] {
    fn limit_next_read(&mut self, _: Duration) -> std::io::Result<()> {
        Ok(())
    }
}

fn read_some(stream: &mut impl Deadline, chunk: &mut [u8], deadline: Instant) -> Result<usize, HttpError> {
    let left = deadline.checked_duration_since(Instant::now()).filter(|d| !d.is_zero()).ok_or(HttpError::Gone)?;
    stream.limit_next_read(left).map_err(|_| HttpError::Gone)?;
    match stream.read(chunk) {
        Ok(0) | Err(_) => Err(HttpError::Gone),
        Ok(n) => Ok(n),
    }
}

/// Reads one request by `deadline`. `admit` sees the head before any body is read: a request it
/// refuses ends there, so a stranger can't make the server wait for a body.
pub fn read_request(
    stream: &mut impl Deadline,
    deadline: Instant,
    admit: impl Fn(&Request) -> Result<(), u16>,
) -> Result<Request, HttpError> {
    let mut buf = Vec::with_capacity(2048);
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(end) = find(&buf, b"\r\n\r\n") {
            break end;
        }
        if buf.len() > MAX_HEAD {
            return Err(HttpError::HeadTooLarge);
        }
        let n = read_some(stream, &mut chunk, deadline)?;
        buf.extend_from_slice(&chunk[..n]);
    };
    if head_end > MAX_HEAD {
        return Err(HttpError::HeadTooLarge);
    }
    let head = std::str::from_utf8(&buf[..head_end]).map_err(|_| HttpError::Bad)?;
    let mut request = parse_head(head)?;
    if let Err(status) = admit(&request) {
        return Err(HttpError::Refused(status));
    }
    let length = match request.header("content-length") {
        Some(value) => value.parse::<usize>().map_err(|_| HttpError::Bad)?,
        None => 0,
    };
    if length > MAX_BODY {
        return Err(HttpError::BodyTooLarge);
    }
    let mut body = buf[head_end + 4..].to_vec();
    if body.len() > length {
        // A second request pipelined behind this one is not served.
        body.truncate(length);
    }
    while body.len() < length {
        let n = read_some(stream, &mut chunk, deadline)?;
        body.extend_from_slice(&chunk[..n.min(length - body.len())]);
    }
    request.body = body;
    Ok(request)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c)
}

fn parse_head(head: &str) -> Result<Request, HttpError> {
    let mut lines = head.split("\r\n");
    let request_line = lines.next().ok_or(HttpError::Bad)?;
    let mut parts = request_line.split(' ');
    let (Some(method), Some(target), Some(version), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        return Err(HttpError::Bad);
    };
    if !matches!(method, "GET" | "POST" | "HEAD") {
        return Err(HttpError::Bad);
    }
    if version != "HTTP/1.1" && version != "HTTP/1.0" {
        return Err(HttpError::Bad);
    }
    if !target.starts_with('/') || target.starts_with("//") || target.len() > 2048 || !target.chars().all(|c| c.is_ascii_graphic()) {
        return Err(HttpError::Bad);
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let mut headers: Vec<(String, String)> = Vec::new();
    for line in lines {
        if headers.len() >= MAX_HEADERS {
            return Err(HttpError::HeadTooLarge);
        }
        // Folded continuation lines are obsolete and a smuggling vector.
        if line.starts_with(' ') || line.starts_with('\t') {
            return Err(HttpError::Bad);
        }
        let (name, value) = line.split_once(':').ok_or(HttpError::Bad)?;
        if name.is_empty() || !name.chars().all(token_char) {
            return Err(HttpError::Bad);
        }
        let value = value.trim_matches(|c| c == ' ' || c == '\t');
        if value.chars().any(|c| c.is_control() && c != '\t') {
            return Err(HttpError::Bad);
        }
        let name = name.to_ascii_lowercase();
        if matches!(name.as_str(), "host" | "content-length" | "cookie" | "origin" | "tailscale-user-login")
            && headers.iter().any(|(n, _)| *n == name)
        {
            return Err(HttpError::Bad);
        }
        if name == "transfer-encoding" {
            return Err(HttpError::Unsupported);
        }
        headers.push((name, value.to_string()));
    }
    Ok(Request { method: method.to_string(), path: path.to_string(), query: query.to_string(), headers, body: Vec::new() })
}

#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn new(status: u16, content_type: &str, body: impl Into<Vec<u8>>) -> Self {
        Response { status, headers: vec![("Content-Type".into(), content_type.into())], body: body.into() }
    }

    pub fn json(status: u16, value: &serde_json::Value) -> Self {
        Response::new(status, "application/json; charset=utf-8", value.to_string())
    }

    pub fn with(mut self, name: &str, value: impl Into<String>) -> Self {
        self.headers.push((name.to_string(), value.into()));
        self
    }
}

pub fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        421 => "Misdirected Request",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        501 => "Not Implemented",
        503 => "Service Unavailable",
        _ => "Error",
    }
}

/// Headers every answer carries: nothing is cached, framed, sniffed or sent elsewhere, and the page
/// runs only its own script.
pub const SECURITY_HEADERS: &[(&str, &str)] = &[
    (
        "Content-Security-Policy",
        "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; font-src 'self'; connect-src 'self'; manifest-src 'self'; \
         base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
    ),
    ("X-Content-Type-Options", "nosniff"),
    ("X-Frame-Options", "DENY"),
    ("Referrer-Policy", "no-referrer"),
    ("Cache-Control", "no-store"),
    ("Cross-Origin-Opener-Policy", "same-origin"),
    ("Cross-Origin-Resource-Policy", "same-origin"),
    ("Permissions-Policy", "camera=(), microphone=(), geolocation=(), payment=(), usb=()"),
];

pub fn write_response(stream: &mut impl Write, response: &Response, head_only: bool) -> std::io::Result<()> {
    let mut out = format!("HTTP/1.1 {} {}\r\n", response.status, reason(response.status));
    // A response may set its own caching (the fonts); every other default always goes out.
    let own = |name: &str| response.headers.iter().any(|(n, _)| n.eq_ignore_ascii_case(name));
    let defaults = SECURITY_HEADERS.iter().filter(|(n, _)| !(*n == "Cache-Control" && own(n))).map(|(n, v)| (*n, *v));
    for (name, value) in defaults.chain(response.headers.iter().map(|(n, v)| (n.as_str(), v.as_str()))) {
        // Values are written by this module's callers; a line break in one would split the head.
        if value.contains(['\r', '\n']) {
            continue;
        }
        out.push_str(&format!("{name}: {value}\r\n"));
    }
    out.push_str(&format!("Content-Length: {}\r\nConnection: close\r\n\r\n", response.body.len()));
    stream.write_all(out.as_bytes())?;
    if !head_only {
        stream.write_all(&response.body)?;
    }
    stream.flush()
}

/// Starts a server-sent event stream; events follow with `write_event`.
pub fn write_event_stream_head(stream: &mut impl Write) -> std::io::Result<()> {
    let mut out = String::from("HTTP/1.1 200 OK\r\n");
    for (name, value) in SECURITY_HEADERS {
        out.push_str(&format!("{name}: {value}\r\n"));
    }
    out.push_str("Content-Type: text/event-stream; charset=utf-8\r\nX-Accel-Buffering: no\r\nConnection: close\r\n\r\n");
    stream.write_all(out.as_bytes())?;
    stream.flush()
}

pub fn write_event(stream: &mut impl Write, event: &str, data: &str) -> std::io::Result<()> {
    // JSON has no raw line breaks; the event name is one of ours.
    let data = data.replace(['\r', '\n'], " ");
    stream.write_all(format!("event: {event}\ndata: {data}\n\n").as_bytes())?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(raw: &str) -> Result<Request, HttpError> {
        read_request(&mut raw.as_bytes(), Instant::now() + Duration::from_secs(5), |_| Ok(()))
    }

    #[test]
    fn a_refused_head_never_has_its_body_read() {
        let raw = "POST /api/login HTTP/1.1\r\nHost: evil\r\nContent-Length: 10\r\n\r\n";
        // The body never comes: refusing must not wait for it.
        let result = read_request(&mut raw.as_bytes(), Instant::now() + Duration::from_secs(5), |r| {
            if r.header("host") == Some("ok") {
                Ok(())
            } else {
                Err(421)
            }
        });
        assert_eq!(result, Err(HttpError::Refused(421)));
    }

    #[test]
    fn a_slow_sender_runs_out_of_time_for_the_whole_request() {
        use std::net::{TcpListener, TcpStream};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let mut client = TcpStream::connect(addr).unwrap();
            for byte in b"GET / HTTP/1.1\r\nHost: x\r\n" {
                if client.write_all(&[*byte]).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(150));
            }
        });
        let (mut server, _) = listener.accept().unwrap();
        let started = Instant::now();
        // Each byte arrives well inside any per-read timeout; the whole request does not.
        let result = read_request(&mut server, Instant::now() + Duration::from_millis(800), |_| Ok(()));
        assert_eq!(result, Err(HttpError::Gone));
        assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
    }

    #[test]
    fn reads_a_request() {
        let r = read("POST /api/input?pane=7&x=a%20b HTTP/1.1\r\nHost: mac.ts.net:8743\r\nContent-Length: 4\r\nCookie: a=1; agentty=abc\r\n\r\nbodyEXTRA").unwrap();
        assert_eq!((r.method.as_str(), r.path.as_str()), ("POST", "/api/input"));
        assert_eq!(r.query_param("pane").as_deref(), Some("7"));
        assert_eq!(r.query_param("x").as_deref(), Some("a b"));
        assert_eq!(r.header("host"), Some("mac.ts.net:8743"));
        assert_eq!(r.cookie("agentty"), Some("abc"));
        assert_eq!(r.body, b"body", "what follows the declared length is not read as this body");
    }

    #[test]
    fn refuses_what_it_does_not_take() {
        let cases = [
            ("GET / HTTP/1.1\r\nHost: a\r\nHost: b\r\n\r\n", HttpError::Bad),
            ("POST / HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\nab", HttpError::Bad),
            ("POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n", HttpError::Unsupported),
            ("GET / HTTP/1.1\r\nX: a\r\n folded\r\n\r\n", HttpError::Bad),
            ("GET / HTTP/1.1\r\nBad Header: a\r\n\r\n", HttpError::Bad),
            ("DELETE / HTTP/1.1\r\n\r\n", HttpError::Bad),
            ("GET http://evil/ HTTP/1.1\r\n\r\n", HttpError::Bad),
            ("GET //evil HTTP/1.1\r\n\r\n", HttpError::Bad),
            ("GET / HTTP/2\r\n\r\n", HttpError::Bad),
            ("GET /a b HTTP/1.1\r\n\r\n", HttpError::Bad),
            ("POST / HTTP/1.1\r\nContent-Length: -1\r\n\r\n", HttpError::Bad),
            ("GET / HTTP/1.1\r\nTailscale-User-Login: a\r\nTailscale-User-Login: b\r\n\r\n", HttpError::Bad),
            ("GET / HTTP/1.1\r\nX: a\u{7}b\r\n\r\n", HttpError::Bad),
        ];
        for (raw, expected) in cases {
            assert_eq!(read(raw), Err(expected), "{raw:?}");
        }
        let big_body = format!("POST / HTTP/1.1\r\nContent-Length: {}\r\n\r\n", MAX_BODY + 1);
        assert_eq!(read(&big_body), Err(HttpError::BodyTooLarge));
        let big_head = format!("GET / HTTP/1.1\r\nX: {}\r\n\r\n", "a".repeat(MAX_HEAD + 10));
        assert_eq!(read(&big_head), Err(HttpError::HeadTooLarge));
        let many = format!("GET / HTTP/1.1\r\n{}\r\n", "X: a\r\n".repeat(MAX_HEADERS + 1));
        assert_eq!(read(&many), Err(HttpError::HeadTooLarge));
        assert_eq!(read("GET / HTTP/1.1\r\n"), Err(HttpError::Gone), "never finished");
        assert_eq!(read("POST / HTTP/1.1\r\nContent-Length: 10\r\n\r\nabc"), Err(HttpError::Gone), "short body");
    }

    #[test]
    fn responses_carry_the_security_headers_and_no_injected_lines() {
        let mut out = Vec::new();
        let response = Response::new(200, "text/plain", "hi").with("X-Test", "a\r\nSet-Cookie: evil=1");
        write_response(&mut out, &response, false).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(text.contains("Content-Security-Policy: default-src 'none'"));
        assert!(text.contains("X-Frame-Options: DENY") && text.contains("Cache-Control: no-store"));
        assert!(!text.contains("evil"), "a header value with a line break is dropped");
        assert!(text.ends_with("\r\n\r\nhi"));
        let mut event = Vec::new();
        write_event(&mut event, "screen", "{\"a\":\"x\ny\"}").unwrap();
        assert_eq!(String::from_utf8(event).unwrap(), "event: screen\ndata: {\"a\":\"x y\"}\n\n");
    }
}
