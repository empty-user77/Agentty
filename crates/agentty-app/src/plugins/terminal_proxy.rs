//! `terminal/setProxy` (`terminal.proxy`, API 6): a plugin that runs a proxy on the loopback
//! interface asks for terminals opened from now on to go through it — what the Proxy Capture
//! plugin uses to list what each tab talks to.
//!
//! - Only `127.0.0.1`: the address is built here from a port, never taken from the plugin, so a
//!   terminal's traffic cannot be sent off the machine this way.
//! - The proxy user name names the pane (`pane-<id>`), the password is the plugin's token: the
//!   proxy can tell which tab a connection came from, and refuse anything without the token.
//! - One plugin at a time. It holds until the plugin clears it, stops or restarts; terminals that
//!   started meanwhile keep the variables they were given.
//! - Read from the terminal backend, off the main thread — hence a lock rather than app state.

use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Eq)]
struct TerminalProxy {
    plugin: String,
    port: u16,
    token: String,
}

static PROXY: Mutex<Option<TerminalProxy>> = Mutex::new(None);

/// What a plugin's token may be: long enough to guess at no better than chance, and nothing that
/// would need escaping in a URL or a shell.
fn valid_token(token: &str) -> bool {
    (16..=128).contains(&token.len()) && token.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Sets (`port` given) or clears (`port` null) the proxy for `plugin`. Refused while another
/// plugin holds it: two capture tools would each think the terminals were theirs.
pub fn set(plugin: &str, port: Option<u64>, token: Option<&str>) -> Result<(), String> {
    let mut proxy = PROXY.lock().map_err(|_| "unavailable".to_string())?;
    if let Some(other) = proxy.as_ref().filter(|p| p.plugin != plugin) {
        return Err(format!("terminals already go through the proxy of \"{}\"", other.plugin));
    }
    let Some(port) = port else {
        *proxy = None;
        return Ok(());
    };
    let port = u16::try_from(port).ok().filter(|p| *p > 0).ok_or("port must be 1–65535")?;
    let token = token.filter(|t| valid_token(t)).ok_or("token must be 16–128 letters, digits, '-' or '_'")?;
    *proxy = Some(TerminalProxy { plugin: plugin.to_string(), port, token: token.to_string() });
    Ok(())
}

/// Called when a plugin stops: whatever it set goes with it, since its proxy went too.
pub fn release(plugin: &str) {
    if let Ok(mut proxy) = PROXY.lock() {
        if proxy.as_ref().is_some_and(|p| p.plugin == plugin) {
            *proxy = None;
        }
    }
}

/// Which plugin terminals go through now, if any.
#[cfg(test)]
fn holder() -> Option<String> {
    PROXY.lock().ok()?.as_ref().map(|p| p.plugin.clone())
}

/// Environment for a pane that starts now: the proxy variables while a plugin asked for them,
/// nothing otherwise.
pub fn pane_environment(pane_id: u64) -> Vec<(String, String)> {
    let Some(proxy) = PROXY.lock().ok().and_then(|p| p.clone()) else { return Vec::new() };
    // The proxy protocol itself is plain HTTP, and this hop never leaves the machine (loopback); what
    // travels inside a CONNECT tunnel stays TLS end to end.
    let url = format!("http://pane-{pane_id}:{}@127.0.0.1:{}", proxy.token, proxy.port); // audit: ok — loopback proxy URL, not a network call
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_addresses_and_the_users_own_stay_direct() {
        assert_eq!(no_proxy_list(None), "localhost,127.0.0.1,::1");
        assert_eq!(
            no_proxy_list(Some(" .corp.example.com, LOCALHOST ,10.0.0.0/8,")),
            "localhost,127.0.0.1,::1,.corp.example.com,10.0.0.0/8"
        );
    }

    /// The whole life of it in one test: the lock is global, and tests run side by side.
    #[test]
    fn one_plugin_points_new_terminals_at_its_loopback_port() {
        let token = "token_example_not_a_real_one";
        assert!(pane_environment(3).is_empty(), "nothing until a plugin asks");
        // Bad requests change nothing.
        assert!(set("capture-test", Some(0), Some(token)).is_err());
        assert!(set("capture-test", Some(70_000), Some(token)).is_err());
        assert!(set("capture-test", Some(5000), Some("short")).is_err());
        assert!(set("capture-test", Some(5000), Some("has space in it_example")).is_err());
        assert!(set("capture-test", Some(5000), Some("x@evil.example.com:1/#_example")).is_err());
        assert!(holder().is_none());

        set("capture-test", Some(50123), Some(token)).unwrap();
        let env = pane_environment(3);
        let https = env.iter().find(|(k, _)| k == "HTTPS_PROXY").map(|(_, v)| v.clone()).unwrap();
        assert!(https.starts_with("http://pane-3:"), "{https}");
        assert!(https.contains(token) && https.ends_with("@127.0.0.1:50123"), "{https}");
        assert!(env.iter().any(|(k, v)| k == "NO_PROXY" && v.starts_with("localhost,127.0.0.1,::1")));

        // Another plugin cannot take it over, nor clear it.
        assert!(set("other-test", Some(6000), Some(token)).is_err());
        assert!(set("other-test", None, None).is_err());
        release("other-test");
        assert_eq!(holder().as_deref(), Some("capture-test"));

        // Stopping the plugin takes it away.
        release("capture-test");
        assert!(pane_environment(3).is_empty());
        assert!(holder().is_none());
    }
}
