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

/// How often listening ports are sampled.
const SAMPLE_EVERY: Duration = Duration::from_secs(3);
/// A new port gets this many chances to answer with a page (a dev server listens before its first
/// build is done) before it is left alone.
const PROBE_ATTEMPTS: u8 = 8;

#[derive(Default)]
pub(super) struct ServerWatch {
    /// Listeners by pane id, from the last sample.
    pub listeners: HashMap<u64, Vec<Listener>>,
    /// Ports that were already looked at, so each server is offered to the browser once.
    seen: HashSet<(u64, u16)>,
    /// New ports waiting for a page to answer: (pane, port, attempts so far).
    pending: Vec<(u64, u16, u8)>,
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
        cx.spawn(async move |this, cx| loop {
            let Ok(roots) = this.read_with(cx, |this, cx| {
                this.all_panes().iter().filter_map(|p| Some((p.read(cx).pane_id, p.read(cx).shell_pid()?))).collect::<Vec<_>>()
            }) else {
                break;
            };
            let pids: Vec<u32> = roots.iter().map(|(_, pid)| *pid).collect();
            let found = cx.background_spawn(async move { crate::procinfo::listeners(&pids) }).await;
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
            cx.background_executor().timer(SAMPLE_EVERY).await;
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
        let wanted = prefs.link_opener == LinkOpener::InApp && prefs.browser.auto_open_servers && crate::platform::HAS_WEBVIEW;
        for key in live {
            if self.servers.seen.insert(key) && wanted {
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
        // The agent may have opened it already (browser tools): don't reload its page.
        let shown = self.browser.as_ref().and_then(|b| b.current_url()).unwrap_or_default();
        if [format!("://localhost:{port}"), format!("://127.0.0.1:{port}")].iter().any(|origin| shown.contains(origin.as_str())) {
            return;
        }
        self.servers.probing = true;
        cx.spawn(async move |this, cx| {
            let serves_page = cx.background_spawn(async move { serves_page(port) }).await;
            let _ = this.update(cx, |this, cx| {
                this.servers.probing = false;
                match serves_page {
                    Some(true) if this.page.is_none() => this.open_browser(Some(format!("http://localhost:{port}")), cx),
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

/// Whether `GET /` on the local `port` answers with a web page. `None`: nothing answered (yet).
fn serves_page(port: u16) -> Option<bool> {
    use std::io::{Read, Write};
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
    let addresses = [SocketAddr::from((Ipv4Addr::LOCALHOST, port)), SocketAddr::from((Ipv6Addr::LOCALHOST, port))];
    let mut stream = addresses.iter().find_map(|a| TcpStream::connect_timeout(a, Duration::from_millis(400)).ok())?;
    stream.set_read_timeout(Some(Duration::from_secs(4))).ok()?;
    stream.set_write_timeout(Some(Duration::from_secs(1))).ok()?;
    let request = format!("GET / HTTP/1.1\r\nHost: localhost:{port}\r\nAccept: text/html\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).ok()?;
    let mut head = Vec::with_capacity(2048);
    let mut buffer = [0u8; 1024];
    while head.len() < 8 * 1024 && !head.windows(4).any(|w| w == b"\r\n\r\n") {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(n) => head.extend_from_slice(&buffer[..n]),
        }
    }
    if head.is_empty() {
        return None;
    }
    Some(is_web_page(&String::from_utf8_lossy(&head)))
}

/// A response head that a browser would show as a page: HTML, or a redirect on the way to one.
fn is_web_page(head: &str) -> bool {
    let mut lines = head.lines();
    let status =
        lines.next().and_then(|l| l.strip_prefix("HTTP/")).and_then(|l| l.split_whitespace().nth(1)).and_then(|s| s.parse::<u16>().ok());
    let Some(status) = status else { return false };
    let header = |name: &str| {
        head.lines().skip(1).take_while(|l| !l.is_empty()).find_map(|l| {
            let (key, value) = l.split_once(':')?;
            key.trim().eq_ignore_ascii_case(name).then(|| value.trim().to_ascii_lowercase())
        })
    };
    match status {
        200..=299 => header("content-type").is_some_and(|t| t.starts_with("text/html") || t.starts_with("application/xhtml")),
        300..=399 => header("location").is_some(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::is_web_page;

    #[test]
    fn only_pages_open_by_themselves() {
        assert!(is_web_page("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<!doctype html>"));
        assert!(is_web_page("HTTP/1.1 307 Temporary Redirect\r\nlocation: /en\r\n\r\n"));
        // An API, a 404 from a server without an index, a debugger, something that is not HTTP.
        assert!(!is_web_page("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\r\n{}"));
        assert!(!is_web_page("HTTP/1.1 404 Not Found\r\ncontent-type: text/html\r\n\r\n"));
        assert!(!is_web_page("HTTP/1.1 200 OK\r\n\r\n"));
        assert!(!is_web_page("\u{0}\u{0}\u{1}binary"));
    }
}
