//! Local servers started in panes (`npm run dev`, …): their ports for the workspace list, opening a
//! new one in the in-app browser, and stopping them when the pane they belong to closes.

use super::{Pane, Workbench};
use crate::i18n::tf;
use crate::procinfo::Listener;
use crate::settings::{settings, LinkOpener};
use gpui::{AppContext, Context};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How often listening ports are sampled (each time a `ps` and an `lsof`).
const SAMPLE_EVERY: Duration = Duration::from_secs(3);
/// The same while the window is not on screen (minimized, hidden, covered).
const SAMPLE_EVERY_HIDDEN: Duration = Duration::from_secs(10);
/// A new port gets this many chances to answer with a page (a dev server listens before its first
/// build is done) before it is left alone.
const PROBE_ATTEMPTS: u8 = 8;
/// Samples a port must be seen in, one after the other, before it is offered: a port that comes and
/// goes within seconds (a test run's server, a tool's callback) is no page to open.
const STEADY_SAMPLES: u8 = 2;

#[derive(Default)]
pub(super) struct ServerWatch {
    /// Listeners by pane id, from the last sample.
    pub listeners: HashMap<u64, Vec<Listener>>,
    /// Ports that were already looked at, so each server is offered to the browser once.
    seen: HashSet<(u64, u16)>,
    /// New ports waiting for a page to answer: (pane, port, attempts so far).
    pending: Vec<(u64, u16, u8)>,
    /// New ports not offered yet, with the samples in a row they were seen in.
    settling: HashMap<(u64, u16), u8>,
    probing: bool,
    /// `ps` output reused while several panes close at once (a workspace, "close other tabs").
    parents: Option<(Instant, Arc<HashMap<u32, u32>>)>,
}

impl Workbench {
    /// Ports of servers started in `pane_ids`' panes, sorted and without repeats.
    pub(super) fn ports_of(&self, pane_ids: impl Iterator<Item = u64>) -> Vec<u16> {
        let mut ports: Vec<u16> = pane_ids.filter_map(|id| self.servers.listeners.get(&id)).flatten().map(|l| l.port).collect();
        ports.sort_unstable();
        ports.dedup();
        ports
    }

    pub(super) fn start_server_watch(&mut self, cx: &mut Context<Self>) {
        let handle = self.window_handle;
        cx.spawn(async move |this, cx| loop {
            let Ok(roots) = this.read_with(cx, |this, cx| {
                this.all_panes().iter().filter_map(|p| Some((p.read(cx).pane_id, p.read(cx).shell_pid()?))).collect::<Vec<_>>()
            }) else {
                break;
            };
            let pids: Vec<u32> = roots.iter().map(|(_, pid)| *pid).collect();
            let found = if pids.is_empty() {
                Default::default()
            } else {
                cx.background_spawn(async move { crate::procinfo::listeners(&pids) }).await
            };
            let listeners: HashMap<u64, Vec<Listener>> =
                roots.into_iter().filter_map(|(pane, pid)| Some((pane, found.get(&pid)?.clone()))).collect();
            if this
                .update(cx, |this, cx| {
                    if this.servers.listeners != listeners {
                        this.servers.listeners = listeners;
                        cx.notify();
                    }
                    this.offer_new_servers(cx);
                })
                .is_err()
            {
                break;
            }
            let shown = Self::on_screen(handle, cx);
            cx.background_executor().timer(if shown { SAMPLE_EVERY } else { SAMPLE_EVERY_HIDDEN }).await;
        })
        .detach();
    }

    /// A server that just started opens in the in-app browser — when links open there. With the
    /// default browser nothing opens by itself: a window of another app popping up is not welcome.
    fn offer_new_servers(&mut self, cx: &mut Context<Self>) {
        let live: HashSet<(u64, u16)> =
            self.servers.listeners.iter().flat_map(|(pane, found)| found.iter().map(|l| (*pane, l.port))).collect();
        // A port that went away may come back (the server was restarted): it is new again then.
        self.servers.seen.retain(|key| live.contains(key));
        self.servers.pending.retain(|(pane, port, _)| live.contains(&(*pane, *port)));
        let prefs = settings(cx);
        let wanted = prefs.link_opener == LinkOpener::InApp
            && prefs.browser.auto_open_servers
            && crate::platform::HAS_WEBVIEW
            && crate::webview::available();
        self.servers.settling.retain(|key, _| live.contains(key));
        // Agentty's own processes (a dev build, test binaries) serve pages for Agentty, not for the user.
        let own: HashSet<(u64, u16)> = self
            .servers
            .listeners
            .iter()
            .flat_map(|(pane, found)| found.iter().map(move |l| (*pane, l)))
            .filter(|(pane, l)| !self.servers.seen.contains(&(*pane, l.port)) && is_agentty_process(l.pid))
            .map(|(pane, l)| (pane, l.port))
            .collect();
        for key in live {
            if self.servers.seen.contains(&key) {
                continue;
            }
            let samples = self.servers.settling.entry(key).or_insert(0);
            *samples += 1;
            if *samples < STEADY_SAMPLES {
                continue;
            }
            self.servers.settling.remove(&key);
            self.servers.seen.insert(key);
            if wanted && !own.contains(&key) {
                self.servers.pending.push((key.0, key.1, 0));
            }
        }
        if !wanted {
            self.servers.pending.clear();
        }
        if self.servers.probing || self.page.is_some() {
            return; // a page (Settings, Git, …) is open: the browser would close it. Later.
        }
        // Only what the user is looking at: a server of a workspace in the background waits.
        let visible: Vec<u64> = self
            .workspaces
            .get(self.active_workspace)
            .map(|ws| ws.tabs.iter().flat_map(|t| t.root.leaves()).map(|p| p.read(cx).pane_id).collect())
            .unwrap_or_default();
        let Some(index) = self.servers.pending.iter().position(|(pane, _, _)| visible.contains(pane)) else { return };
        let (pane, port, attempts) = self.servers.pending.remove(index);
        // Its terminal's tab may show it already (its agent opened it): don't reload that page.
        let shown = self.terminal_tab_address(pane).unwrap_or_default();
        if [format!("://localhost:{port}"), format!("://127.0.0.1:{port}")].iter().any(|origin| shown.contains(origin.as_str())) {
            return;
        }
        self.servers.probing = true;
        cx.spawn(async move |this, cx| {
            let serves_page = cx.background_spawn(async move { serves_page(port) }).await;
            let _ = this.update(cx, |this, cx| {
                this.servers.probing = false;
                // Gone while it was asked (a short-lived server): an empty browser tab helps nobody.
                let still_there = this.servers.listeners.get(&pane).is_some_and(|found| found.iter().any(|l| l.port == port));
                match serves_page {
                    Some(true) if this.page.is_none() && still_there => {
                        let url = format!("http://localhost:{port}");
                        this.open_link_for_terminal(pane, url, super::terminal_browser::Opener::Server, cx);
                    }
                    // Not a web page (an API, a debugger, a database): nothing to show.
                    Some(_) => {}
                    // Nothing answered yet: the first build may still be running.
                    None if attempts + 1 < PROBE_ATTEMPTS => this.servers.pending.push((pane, port, attempts + 1)),
                    None => {}
                }
            });
        })
        .detach();
    }

    /// Stops the servers `pane` started, as the pane goes away. Call before the pane is dropped:
    /// once its shell is gone its children belong to pid 1 and can't be told apart any more.
    pub(super) fn stop_servers_of(&mut self, pane: &Pane, cx: &mut Context<Self>) {
        if !settings(cx).stop_servers_on_close || cfg!(windows) {
            return;
        }
        let (pane_id, shell) = (pane.read(cx).pane_id, pane.read(cx).shell_pid());
        let mut candidates: Vec<u32> = self.servers.listeners.remove(&pane_id).unwrap_or_default().iter().map(|l| l.pid).collect();
        if let Some(shell) = shell {
            let parents = match &self.servers.parents {
                Some((at, parents)) if at.elapsed() < Duration::from_millis(500) => parents.clone(),
                _ => {
                    let parents = Arc::new(crate::procinfo::process_parents());
                    self.servers.parents = Some((Instant::now(), parents.clone()));
                    parents
                }
            };
            candidates.extend(crate::procinfo::descendants(shell, &parents));
        }
        candidates.sort_unstable();
        candidates.dedup();
        if candidates.is_empty() {
            return;
        }
        cx.spawn(async move |this, cx| {
            let stopped = cx.background_spawn(async move { crate::procinfo::terminate_listeners(&candidates) }).await;
            if stopped.is_empty() {
                return;
            }
            let ports = stopped.iter().map(|l| format!(":{}", l.port)).collect::<Vec<_>>().join(" ");
            let _ = this.update(cx, |this, cx| {
                let text = tf(cx, "servers.stopped", &[("ports", &ports)]);
                this.show_toast(text, cx);
            });
            cx.background_executor().timer(Duration::from_secs(3)).await;
            cx.background_spawn(async move { crate::procinfo::kill_remaining(&stopped) }).await;
        })
        .detach();
    }
}

/// Whether `pid` is an Agentty binary (the app, a dev build, a test binary of its crates).
fn is_agentty_process(pid: u32) -> bool {
    crate::procinfo::executable_path(pid)
        .and_then(|path| path.file_name().map(|name| name.to_string_lossy().to_lowercase()))
        .is_some_and(|name| name.starts_with("agentty"))
}

/// Whether `GET /` on the local `port` answers with a web page. `None`: nothing answered (yet).
fn serves_page(port: u16) -> Option<bool> {
    use std::io::{Read, Write};
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
    let addresses = [SocketAddr::from((Ipv4Addr::LOCALHOST, port)), SocketAddr::from((Ipv6Addr::LOCALHOST, port))];
    let mut path = String::from("/");
    // A few redirects on the same server (`/` → `/en` → `/en/home`), as a browser would follow them.
    for _ in 0..4 {
        let mut stream = addresses.iter().find_map(|a| TcpStream::connect_timeout(a, Duration::from_millis(400)).ok())?;
        stream.set_read_timeout(Some(Duration::from_secs(4))).ok()?;
        stream.set_write_timeout(Some(Duration::from_secs(1))).ok()?;
        let request = format!("GET {path} HTTP/1.1\r\nHost: localhost:{port}\r\nAccept: text/html\r\nConnection: close\r\n\r\n");
        stream.write_all(request.as_bytes()).ok()?;
        // The head and the start of the body: enough to tell a document from a line of text.
        let mut response = Vec::with_capacity(4096);
        let mut buffer = [0u8; 4096];
        while response.len() < 16 * 1024 {
            match stream.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(n) => response.extend_from_slice(&buffer[..n]),
            }
        }
        if response.is_empty() {
            return None;
        }
        match classify(&String::from_utf8_lossy(&response), port) {
            Answer::Page => return Some(true),
            Answer::NotPage => return Some(false),
            Answer::Redirect(next) => path = next,
        }
    }
    Some(false)
}

#[derive(Debug, PartialEq)]
enum Answer {
    /// A document a browser would show.
    Page,
    /// On to another path of the same server.
    Redirect(String),
    NotPage,
}

/// What a response to `GET /` is. Only an HTML document counts: asked with `Accept: text/html`, an
/// API answers `Hello World!` or `OK` as `text/html` too, and a redirect to another site (a sign-in
/// provider, a docs page) says nothing about this server.
fn classify(response: &str, port: u16) -> Answer {
    let (head, body) = response.split_once("\r\n\r\n").unwrap_or((response, ""));
    let mut lines = head.lines();
    let status =
        lines.next().and_then(|l| l.strip_prefix("HTTP/")).and_then(|l| l.split_whitespace().nth(1)).and_then(|s| s.parse::<u16>().ok());
    let Some(status) = status else { return Answer::NotPage };
    let header = |name: &str| {
        head.lines().skip(1).find_map(|l| {
            let (key, value) = l.split_once(':')?;
            key.trim().eq_ignore_ascii_case(name).then(|| value.trim().to_string())
        })
    };
    match status {
        200..=299 => {
            let html = header("content-type").is_some_and(|t| {
                let t = t.to_ascii_lowercase();
                t.starts_with("text/html") || t.starts_with("application/xhtml")
            });
            let body = body.to_ascii_lowercase();
            let document = ["<!doctype html", "<html", "<head", "<body"].iter().any(|tag| body.contains(tag));
            if html && document {
                Answer::Page
            } else {
                Answer::NotPage
            }
        }
        300..=399 => header("location").and_then(|l| same_server_path(&l, port)).map_or(Answer::NotPage, Answer::Redirect),
        _ => Answer::NotPage,
    }
}

/// The path a redirect leads to when it stays on this server (`/en`, `http://localhost:3000/en`).
fn same_server_path(location: &str, port: u16) -> Option<String> {
    if location.starts_with('/') {
        return (!location.starts_with("//")).then(|| location.to_string());
    }
    let rest = location.strip_prefix("http://").or_else(|| location.strip_prefix("https://"))?;
    let (host, path) = rest.split_once('/').map_or((rest, ""), |(host, path)| (host, path));
    let local = ["localhost", "127.0.0.1", "[::1]"].iter().any(|name| host.eq_ignore_ascii_case(&format!("{name}:{port}")));
    local.then(|| format!("/{path}"))
}

#[cfg(test)]
mod tests {
    use super::{classify, Answer};

    #[test]
    fn only_pages_open_by_themselves() {
        let page = |r: &str| classify(r, 3000) == Answer::Page;
        assert!(page("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<!doctype html><html>"));
        assert!(page("HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ntransfer-encoding: chunked\r\n\r\n1a2\r\n<!DOCTYPE html>"));
        assert!(page("HTTP/1.1 200 OK\r\ncontent-type: text/html\r\n\r\n\n  <html lang=\"en\"><head>"));
        // An API, a 404 from a server without an index, a debugger, something that is not HTTP.
        assert!(!page("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\r\n{}"));
        assert!(!page("HTTP/1.1 404 Not Found\r\ncontent-type: text/html\r\n\r\n<!doctype html>"));
        assert!(!page("HTTP/1.1 200 OK\r\n\r\n"));
        assert!(!page("\u{0}\u{0}\u{1}binary"));
        // An API's greeting, sent as text/html because the request asked for it (Spring, NestJS, Express).
        assert!(!page("HTTP/1.1 200 OK\r\nContent-Type: text/html;charset=UTF-8\r\n\r\nHello World!"));
        assert!(!page("HTTP/1.1 200 OK\r\ncontent-type: text/html; charset=utf-8\r\n\r\nOK"));
    }

    #[test]
    fn redirects_are_followed_only_on_the_same_server() {
        let redirect = |location: &str| classify(&format!("HTTP/1.1 302 Found\r\nLocation: {location}\r\n\r\n"), 3000);
        assert_eq!(redirect("/en"), Answer::Redirect("/en".into()));
        assert_eq!(redirect("http://localhost:3000/login?next=%2F"), Answer::Redirect("/login?next=%2F".into()));
        assert_eq!(redirect("http://127.0.0.1:3000"), Answer::Redirect("/".into()));
        // Elsewhere: a sign-in provider, another port, a protocol-relative address.
        assert_eq!(redirect("https://accounts.example.com/auth"), Answer::NotPage);
        assert_eq!(redirect("http://localhost:8080/"), Answer::NotPage);
        assert_eq!(redirect("//example.com/"), Answer::NotPage);
        assert_eq!(classify("HTTP/1.1 301 Moved\r\n\r\n", 3000), Answer::NotPage);
    }
}
