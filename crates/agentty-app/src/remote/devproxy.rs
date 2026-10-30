//! A local dev server (`npm run dev` on `localhost:3000`, …) shown on the tailnet for the remote
//! page: `https://<machine>:3000` is sent by Tailscale to this proxy, which hands each request to
//! the server only after the checks the page itself makes.
//!
//! Every request, in order (see `server` for the page's own):
//! 0. **The secret path** Tailscale was told to put in front: a request without it came from
//!    something on this Mac, not through Tailscale (none in the local test mode).
//! 1. **Host**: the machine's tailnet name with this server's port.
//! 2. **Tailscale identity**: the login that owns this machine.
//! 3. **A session of the page**: its cookie reaches this address too (cookies are kept per machine
//!    name, not per port), so signing in on the page is what opens the server.
//! 4. The port is still a server one of the page's terminals runs.
//!
//! Then one request goes to the server and its answer comes back, and the connection closes: the
//! next request is checked again, however Tailscale keeps its own connections. A WebSocket (a dev
//! server's live reload) stays open once checked. The page's cookie is taken out of what the server
//! sees, and a `Set-Cookie` for it out of what the server answers, so a dev server can neither read
//! the session nor replace it. The server is told it is asked as `localhost`, which dev servers
//! accept (Vite refuses hosts it does not know).

use super::server::Hub;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Requests in flight at once per server; past this a connection is closed at once.
const MAX_OPEN: usize = 128;
/// Longest request or response head read.
const MAX_HEAD: usize = 64 * 1024;
/// The request head arrives within this.
const HEAD_TIMEOUT: Duration = Duration::from_secs(15);
/// A dev server's first answer can wait for a whole build.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(120);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// How often a connection that stays open (a socket, a stream of events) is checked again: it
/// closes once the proxy stops, the session ends or the server is no longer one the page may see.
const RECHECK_EVERY: Duration = Duration::from_secs(5);

/// Request headers never passed on: hop-by-hop ones (set again below), and what Tailscale says
/// about the sender, which is the proxy's to read and nobody else's.
const DROPPED: &[&str] = &[
    "host",
    "connection",
    "keep-alive",
    "proxy-connection",
    "te",
    "upgrade",
    "origin",
    "cookie",
    "tailscale-user-login",
    "tailscale-user-name",
    "tailscale-user-profile-pic",
    "tailscale-headers-info",
];

/// One dev server's proxy, listening on a free port of `127.0.0.1` until stopped (or dropped).
pub struct DevProxy {
    local_port: u16,
    secret: Option<String>,
    stop: Arc<AtomicBool>,
}

/// What every connection of one proxy needs.
struct Gate {
    hub: Arc<Hub>,
    dev_port: u16,
    /// Accepted `Host` (`machine.tailnet.ts.net:3000`, or `127.0.0.1:<port>` in the test mode).
    host: String,
    secret: Option<String>,
    https: bool,
    open: AtomicUsize,
    /// Set when the proxy stops: connections still open (a live reload socket) close too.
    stop: Arc<AtomicBool>,
}

impl DevProxy {
    /// Starts a proxy for the server on `dev_port`, reached as `host` (`None`: as itself, in the
    /// local test mode, `127.0.0.1:<its port>`), with `secret` in front of every path when Tailscale
    /// forwards to it.
    pub fn start(hub: Arc<Hub>, dev_port: u16, host: Option<String>, secret: Option<String>, https: bool) -> io::Result<DevProxy> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let local_port = listener.local_addr()?.port();
        let host = host.unwrap_or_else(|| format!("127.0.0.1:{local_port}"));
        let stop = Arc::new(AtomicBool::new(false));
        let gate = Arc::new(Gate { hub, dev_port, host, secret: secret.clone(), https, open: AtomicUsize::new(0), stop: stop.clone() });
        let stopped = stop.clone();
        std::thread::Builder::new().name(format!("agentty-devproxy-{dev_port}")).spawn(move || {
            while !stopped.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        if gate.open.fetch_add(1, Ordering::SeqCst) >= MAX_OPEN {
                            gate.open.fetch_sub(1, Ordering::SeqCst);
                            continue;
                        }
                        let conn = gate.clone();
                        let spawned = std::thread::Builder::new().name("agentty-devproxy-conn".into()).spawn(move || {
                            let _ = handle(&conn, stream);
                            conn.open.fetch_sub(1, Ordering::SeqCst);
                        });
                        if spawned.is_err() {
                            // Never started, so it can't give its place back itself.
                            gate.open.fetch_sub(1, Ordering::SeqCst);
                        }
                    }
                    Err(err) if err.kind() == io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(25)),
                    Err(_) => std::thread::sleep(Duration::from_millis(100)),
                }
            }
        })?;
        Ok(DevProxy { local_port, secret, stop })
    }

    /// What Tailscale is pointed at: the proxy, with the secret path in front.
    pub fn serve_target(&self) -> String {
        match &self.secret {
            Some(secret) => format!("http://127.0.0.1:{}/{secret}", self.local_port),
            None => format!("http://127.0.0.1:{}", self.local_port),
        }
    }

    /// Where the proxy listens, for the local test mode's address.
    pub fn local_port(&self) -> u16 {
        self.local_port
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl Drop for DevProxy {
    fn drop(&mut self) {
        self.stop();
    }
}

/// A request head: method, target, and headers (names lower case).
struct Head {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
}

impl Head {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }
}

/// Reads up to the end of a head (`\r\n\r\n`): the head, and whatever came after it.
fn read_head(stream: &mut TcpStream) -> io::Result<(Vec<u8>, Vec<u8>)> {
    let mut buf = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let rest = buf.split_off(end + 4);
            return Ok((buf, rest));
        }
        if buf.len() > MAX_HEAD {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

fn parse_head(raw: &[u8]) -> Option<Head> {
    let text = std::str::from_utf8(raw).ok()?;
    let mut lines = text.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let (method, target, version) = (first.next()?, first.next()?, first.next()?);
    if !version.starts_with("HTTP/1.") || !method.bytes().all(|b| b.is_ascii_uppercase()) {
        return None;
    }
    let headers = lines
        .filter(|l| !l.is_empty())
        .map(|line| line.split_once(':').map(|(n, v)| (n.trim().to_ascii_lowercase(), v.trim().to_string())))
        .collect::<Option<Vec<_>>>()?;
    Some(Head { method: method.to_string(), target: target.to_string(), headers })
}

fn refuse(stream: &mut TcpStream, status: u16, reason: &str, text: &str) {
    let body = format!("{text}\n");
    let _ = write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\n\
         X-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

/// `cookie` without the page's own session cookie (`name`); `None` when nothing is left.
fn without_cookie(cookie: &str, name: &str) -> Option<String> {
    let kept: Vec<&str> =
        cookie.split(';').map(str::trim).filter(|pair| pair.split_once('=').is_none_or(|(k, _)| k.trim() != name)).collect();
    let joined = kept.into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join("; ");
    (!joined.is_empty()).then_some(joined)
}

/// The head sent to the dev server: the target without the secret, asked as `localhost`, the
/// page's cookie and Tailscale's headers left out.
fn upstream_head(head: &Head, target: &str, dev_port: u16, host: &str, https: bool, cookie_name: &str, websocket: bool) -> String {
    let mut out = format!("{} {target} HTTP/1.1\r\nHost: localhost:{dev_port}\r\n", head.method);
    for (name, value) in &head.headers {
        if DROPPED.contains(&name.as_str()) || !is_token(name) || value.contains(['\r', '\n']) {
            continue;
        }
        out.push_str(&format!("{name}: {value}\r\n"));
    }
    if let Some(cookie) = head.header("cookie").and_then(|c| without_cookie(c, cookie_name)) {
        out.push_str(&format!("Cookie: {cookie}\r\n"));
    }
    // A dev server's live reload checks where its socket is opened from: it is opened from itself.
    if head.header("origin").is_some() {
        out.push_str(&format!("Origin: http://localhost:{dev_port}\r\n"));
    }
    out.push_str(&format!("X-Forwarded-Host: {host}\r\nX-Forwarded-Proto: {}\r\n", if https { "https" } else { "http" }));
    if websocket {
        out.push_str("Connection: Upgrade\r\nUpgrade: websocket\r\n");
    } else {
        out.push_str("Connection: close\r\n");
    }
    out.push_str("\r\n");
    out
}

/// A header name as HTTP allows it; anything else is not passed on.
fn is_token(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

/// A response head with any `Set-Cookie` for the page's session cookie taken out.
fn filtered_answer(raw: &[u8], cookie_name: &str) -> Vec<u8> {
    let Ok(text) = std::str::from_utf8(raw) else { return raw.to_vec() };
    text.split_inclusive("\r\n")
        .filter(|line| {
            let Some((name, value)) = line.split_once(':') else { return true };
            // The name is compared after trimming: browsers accept `__Host-agentty = x` as the same cookie.
            let cookie = value.trim_start().split('=').next().unwrap_or_default().trim();
            !(name.trim().eq_ignore_ascii_case("set-cookie") && cookie == cookie_name)
        })
        .collect::<String>()
        .into_bytes()
}

fn connect(dev_port: u16) -> io::Result<TcpStream> {
    // `localhost` is `::1` first on macOS: Node and Vite often listen there only.
    let addresses = [SocketAddr::from((Ipv4Addr::LOCALHOST, dev_port)), SocketAddr::from((Ipv6Addr::LOCALHOST, dev_port))];
    let mut last = io::Error::from(io::ErrorKind::ConnectionRefused);
    for address in addresses {
        match TcpStream::connect_timeout(&address, CONNECT_TIMEOUT) {
            Ok(stream) => return Ok(stream),
            Err(err) => last = err,
        }
    }
    Err(last)
}

/// Closes `client` and `upstream` when the connection is no longer let in; ends with `done`.
fn recheck(gate: Arc<Gate>, login: Option<String>, cookie: Option<String>, done: Arc<AtomicBool>, client: TcpStream, upstream: TcpStream) {
    let step = Duration::from_millis(250);
    let mut waited = Duration::ZERO;
    while !done.load(Ordering::SeqCst) {
        std::thread::sleep(step);
        waited += step;
        let stopped = gate.stop.load(Ordering::SeqCst);
        if !stopped && waited < RECHECK_EVERY {
            continue;
        }
        waited = Duration::ZERO;
        if stopped || !gate.hub.admits(login.as_deref(), cookie.as_deref()) || !gate.hub.dev_port_allowed(gate.dev_port) {
            let _ = client.shutdown(Shutdown::Both);
            let _ = upstream.shutdown(Shutdown::Both);
            return;
        }
    }
}

fn handle(gate: &Arc<Gate>, mut client: TcpStream) -> io::Result<()> {
    client.set_nonblocking(false)?;
    client.set_read_timeout(Some(HEAD_TIMEOUT))?;
    client.set_write_timeout(Some(ANSWER_TIMEOUT))?;
    let (raw, rest) = read_head(&mut client)?;
    let Some(head) = parse_head(&raw) else {
        refuse(&mut client, 400, "Bad Request", "Bad request.");
        return Ok(());
    };
    // 0. Through Tailscale (or, in the test mode, straight).
    let target = match &gate.secret {
        Some(secret) => match super::server::strip_secret(&head.target, secret) {
            Some(path) => path,
            None => {
                refuse(&mut client, 404, "Not Found", "Not found.");
                return Ok(());
            }
        },
        None if head.target.starts_with('/') => head.target.clone(),
        None => {
            refuse(&mut client, 400, "Bad Request", "Bad request.");
            return Ok(());
        }
    };
    // 1.
    if !head.header("host").is_some_and(|h| h.eq_ignore_ascii_case(&gate.host)) {
        refuse(&mut client, 421, "Misdirected Request", "Misdirected request.");
        return Ok(());
    }
    // 2 and 3.
    if !gate.hub.admits(head.header("tailscale-user-login"), head.header("cookie")) {
        refuse(&mut client, 401, "Unauthorized", "Sign in on Agentty's remote page first, then open this server from there again.");
        return Ok(());
    }
    // 4.
    if !gate.hub.dev_port_allowed(gate.dev_port) {
        refuse(&mut client, 404, "Not Found", "This server is no longer running in Agentty.");
        return Ok(());
    }
    let websocket = head.header("upgrade").is_some_and(|u| u.eq_ignore_ascii_case("websocket"))
        && head.header("connection").is_some_and(|c| c.to_ascii_lowercase().contains("upgrade"));
    let upstream = match connect(gate.dev_port) {
        Ok(stream) => stream,
        Err(_) => {
            refuse(&mut client, 502, "Bad Gateway", &format!("The server on port {} isn't answering.", gate.dev_port));
            return Ok(());
        }
    };
    let cookie_name = gate.hub.cookie_name();
    let done = Arc::new(AtomicBool::new(false));
    let watcher = {
        let (gate, done) = (gate.clone(), done.clone());
        let (login, cookie) = (head.header("tailscale-user-login").map(str::to_string), head.header("cookie").map(str::to_string));
        let (client, upstream) = (client.try_clone()?, upstream.try_clone()?);
        std::thread::Builder::new()
            .name("agentty-devproxy-check".into())
            .spawn(move || recheck(gate, login, cookie, done, client, upstream))?
    };
    let relayed = relay(&head, &target, gate, client, upstream, rest, cookie_name, websocket);
    done.store(true, Ordering::SeqCst);
    let _ = watcher.join();
    relayed
}

/// One checked request to the server and its answer back (or a socket, both ways until closed).
#[allow(clippy::too_many_arguments)]
fn relay(
    head: &Head,
    target: &str,
    gate: &Gate,
    mut client: TcpStream,
    mut upstream: TcpStream,
    rest: Vec<u8>,
    cookie_name: &str,
    websocket: bool,
) -> io::Result<()> {
    upstream.set_write_timeout(Some(ANSWER_TIMEOUT))?;
    upstream.write_all(upstream_head(head, target, gate.dev_port, &gate.host, gate.https, cookie_name, websocket).as_bytes())?;
    upstream.write_all(&rest)?;
    // The rest of the request (a body, or a socket's frames) goes on as it comes.
    client.set_read_timeout(None)?;
    let (mut from_client, mut to_upstream) = (client.try_clone()?, upstream.try_clone()?);
    let sender = std::thread::Builder::new().name("agentty-devproxy-up".into()).spawn(move || {
        let _ = io::copy(&mut from_client, &mut to_upstream);
        let _ = to_upstream.shutdown(Shutdown::Write);
    })?;
    // The answer: its head checked for the page's cookie, then everything else as it comes.
    upstream.set_read_timeout(Some(ANSWER_TIMEOUT))?;
    let relayed = (|| -> io::Result<()> {
        let (answer, body) = read_head(&mut upstream)?;
        client.write_all(&filtered_answer(&answer, cookie_name))?;
        client.write_all(&body)?;
        upstream.set_read_timeout(None)?;
        client.set_write_timeout(None)?;
        io::copy(&mut upstream, &mut client)?;
        Ok(())
    })();
    let _ = client.shutdown(Shutdown::Both);
    let _ = upstream.shutdown(Shutdown::Both);
    let _ = sender.join();
    relayed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(raw: &str) -> Head {
        parse_head(raw.as_bytes()).unwrap()
    }

    /// Through a real proxy to a stand-in dev server: refused without a session, passed on with
    /// one — as `localhost`, without the session, its answer's attempt to set the session dropped.
    #[test]
    fn a_signed_in_page_reaches_the_server_and_nothing_else_does() {
        // The stand-in dev server: answers once with what it was asked, and tries to set the session.
        let upstream = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let dev_port = upstream.local_addr().unwrap().port();
        let seen = std::thread::spawn(move || {
            let (mut stream, _) = upstream.accept().unwrap();
            let (raw, _) = read_head(&mut stream).unwrap();
            let asked = String::from_utf8(raw).unwrap();
            let body = "hello from dev";
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nSet-Cookie: agentty_dev=evil\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            asked
        });

        let (hub, token) = page_with_server(dev_port);
        let proxy = DevProxy::start(hub.clone(), dev_port, None, None, false).unwrap();
        let host = format!("127.0.0.1:{}", proxy.local_port());
        let ask = |cookie: &str| {
            let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, proxy.local_port())).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            write!(stream, "GET /page?x=1 HTTP/1.1\r\nHost: {host}\r\nCookie: {cookie}\r\n\r\n").unwrap();
            let mut answer = String::new();
            let _ = stream.read_to_string(&mut answer);
            answer
        };
        let refused = ask("agentty_dev=made-up");
        assert!(refused.starts_with("HTTP/1.1 401"), "{refused}");
        let answer = ask(&format!("theme=dark; agentty_dev={token}"));
        assert!(answer.starts_with("HTTP/1.1 200") && answer.ends_with("hello from dev"), "{answer}");
        assert!(!answer.contains("evil"), "the server can't set the page's session: {answer}");
        let asked = seen.join().unwrap();
        assert!(asked.starts_with("GET /page?x=1 HTTP/1.1\r\nHost: localhost:"), "{asked}");
        assert!(asked.contains("Cookie: theme=dark\r\n") && !asked.contains(&token), "{asked}");
        proxy.stop();
        hub.shutdown();
    }

    /// A page hub in the local test mode whose overview has a server on `dev_port`, and a session.
    fn page_with_server(dev_port: u16) -> (Arc<Hub>, String) {
        use super::super::auth;
        use super::super::conn::Listener;
        use super::super::server::Config;
        use super::super::snapshot::{PortInfo, WorkspaceInfo};

        let config = Config { owner: None, hosts: Vec::new(), https: false, secret: None, folders: None };
        let hub = Hub::start(config, Some(auth::hash_for_tests("pw")), Listener::tcp().unwrap()).unwrap();
        let workspace = WorkspaceInfo {
            id: 1,
            name: "app".into(),
            group: None,
            group_color: None,
            color: None,
            branch: None,
            folder: "~/app".into(),
            root: std::path::PathBuf::new(),
            sleeping: false,
            active_tab: 0,
            tabs: Vec::new(),
            ports: vec![PortInfo { port: dev_port, url: None, error: None }],
        };
        hub.publish(vec![workspace], Vec::new(), Vec::new());
        let token = hub.session_for_tests("local");
        (hub, token)
    }

    /// A live reload socket open through the proxy closes when the proxy stops (the server went
    /// away, remote access was turned off): nothing stays reachable after it.
    #[test]
    fn an_open_socket_closes_when_the_proxy_stops() {
        let upstream = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let dev_port = upstream.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut stream, _) = upstream.accept().unwrap();
            let _ = read_head(&mut stream).unwrap();
            stream.write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n").unwrap();
            // Holds the socket open until the other side closes it.
            let _ = io::copy(&mut stream, &mut io::sink());
        });
        let (hub, token) = page_with_server(dev_port);
        let proxy = DevProxy::start(hub.clone(), dev_port, None, None, false).unwrap();
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, proxy.local_port())).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        write!(
            stream,
            "GET /ws HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nCookie: agentty_dev={token}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n",
            proxy.local_port()
        )
        .unwrap();
        let mut answer = [0u8; 12];
        stream.read_exact(&mut answer).unwrap();
        assert_eq!(&answer, b"HTTP/1.1 101");
        let started = std::time::Instant::now();
        proxy.stop();
        let mut rest = Vec::new();
        let _ = stream.read_to_end(&mut rest);
        assert!(started.elapsed() < Duration::from_secs(3), "closed after {:?}", started.elapsed());
        hub.shutdown();
    }

    #[test]
    fn the_server_never_sees_the_session_or_who_sent_it() {
        let h = head(
            "GET /x HTTP/1.1\r\nHost: mac.ts.net:3000\r\nCookie: theme=dark; __Host-agentty=secret-token; a=b\r\n\
             Tailscale-User-Login: me@example.com\r\nOrigin: https://mac.ts.net:3000\r\nAccept: */*",
        );
        let out = upstream_head(&h, "/x", 3000, "mac.ts.net:3000", true, "__Host-agentty", false);
        assert!(out.starts_with("GET /x HTTP/1.1\r\nHost: localhost:3000\r\n"), "{out}");
        assert!(out.contains("Cookie: theme=dark; a=b\r\n"), "{out}");
        assert!(!out.contains("secret-token") && !out.to_ascii_lowercase().contains("tailscale-user-login"), "{out}");
        assert!(out.contains("Origin: http://localhost:3000\r\n") && out.contains("Connection: close\r\n"), "{out}");
        assert!(out.contains("accept: */*\r\n"));
    }

    #[test]
    fn a_live_reload_socket_stays_a_socket() {
        let h = head("GET /hmr HTTP/1.1\r\nHost: h:5173\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: k");
        let out = upstream_head(&h, "/hmr", 5173, "h:5173", true, "__Host-agentty", true);
        assert!(out.contains("Connection: Upgrade\r\nUpgrade: websocket\r\n") && out.contains("sec-websocket-key: k\r\n"), "{out}");
    }

    #[test]
    fn the_server_can_not_replace_the_session() {
        let answer = "HTTP/1.1 200 OK\r\nSet-Cookie: __Host-agentty=evil; Path=/\r\nset-cookie: app=1\r\nContent-Length: 0\r\n\r\n";
        let kept = String::from_utf8(filtered_answer(answer.as_bytes(), "__Host-agentty")).unwrap();
        assert!(!kept.contains("evil") && kept.contains("set-cookie: app=1") && kept.ends_with("\r\n\r\n"), "{kept}");
    }

    #[test]
    fn only_a_session_cookie_is_taken_out() {
        assert_eq!(without_cookie("__Host-agentty=t", "__Host-agentty"), None);
        assert_eq!(without_cookie("a=1;  __Host-agentty=t ;b=2", "__Host-agentty").as_deref(), Some("a=1; b=2"));
        assert_eq!(without_cookie("x__Host-agentty=1", "__Host-agentty").as_deref(), Some("x__Host-agentty=1"));
    }

    #[test]
    fn odd_heads_are_refused() {
        assert!(parse_head(b"get / HTTP/1.1\r\nHost: a").is_none(), "lower-case method");
        assert!(parse_head(b"GET / HTTP/2\r\nHost: a").is_none());
        assert!(parse_head(b"GET / HTTP/1.1\r\nno colon").is_none());
    }
}
