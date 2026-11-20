//! Linux: each pane's program in a control group of its own.
//!
//! Everything Agentty starts lands in Agentty's own control group. A low-memory daemon
//! (`systemd-oomd`, on by default on Ubuntu) weighs and ends a group as one: with ten agents open
//! and memory short, what it ends is the app with every one of them. Moved into a scope of its
//! own, a pane's shell and whatever it starts afterwards are a group the daemon can end alone —
//! the heaviest agent goes, and the app and the other panes stay.
//!
//! The session's service manager is asked to make the scope around the pane's process once it
//! runs. Nothing about how the pane is started changes, and where the manager is not there or
//! says no, the process stays where it was.

/// The scope's name: unique to this app and this pane's process.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn unit_name(app: u32, pane: u32) -> String {
    format!("agentty-pane-{app}-{pane}.scope")
}

/// `busctl`'s arguments for `StartTransientUnit`: a scope in the apps' slice, holding `pane`
/// (what it starts from now on is born into it), forgotten by the manager once it is empty.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn request(app: u32, pane: u32) -> Vec<String> {
    let pane_text = pane.to_string();
    let unit = unit_name(app, pane);
    [
        "--user",
        "--quiet",
        "call",
        "org.freedesktop.systemd1",
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
        "StartTransientUnit",
        "ssa(sv)a(sa(sv))",
        unit.as_str(),
        "fail",
        // Four properties, each a name, its type, and its value.
        "4",
        "PIDs",
        "au",
        "1",
        pane_text.as_str(),
        "Description",
        "s",
        "Agentty pane",
        "Slice",
        "s",
        "app.slice",
        "CollectMode",
        "s",
        "inactive-or-failed",
        // No auxiliary units.
        "0",
    ]
    .iter()
    .map(|part| part.to_string())
    .collect()
}

/// Moves the pane's process `pane` into a scope of its own, off the caller's thread. Nothing
/// waits for it and nothing depends on it.
#[cfg(target_os = "linux")]
pub fn give(pane: u32) {
    use std::sync::atomic::{AtomicU8, Ordering};
    // 0: not tried, 1: works, 2: does not (no service manager for the session, or no `busctl`):
    // asked once, then left alone.
    static WORKS: AtomicU8 = AtomicU8::new(0);
    if pane == 0 || WORKS.load(Ordering::Relaxed) == 2 {
        return;
    }
    let _ = std::thread::Builder::new().name("pane-scope".into()).spawn(move || {
        let done = std::process::Command::new("busctl")
            .args(request(std::process::id(), pane))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        // A pane whose program was gone already fails too: only the first answer decides.
        if WORKS.compare_exchange(0, if done { 1 } else { 2 }, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
            crate::crash_log::note("pane groups", if done { "each pane in its own" } else { "not available (all in the app's)" });
        }
    });
}

#[cfg(not(target_os = "linux"))]
pub fn give(_pane: u32) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_request_names_the_pane_and_its_own_scope() {
        let request = request(4100, 4242);
        let joined = request.join(" ");
        assert!(joined.contains("StartTransientUnit ssa(sv)a(sa(sv)) agentty-pane-4100-4242.scope fail 4 PIDs au 1 4242"), "{joined}");
        assert!(joined.contains("Slice s app.slice") && joined.ends_with("CollectMode s inactive-or-failed 0"), "{joined}");
        // The session's manager, never the system's.
        assert_eq!(request[0], "--user");
        // Two panes, or the same process number under another run of the app: two scopes.
        assert_ne!(unit_name(4100, 1), unit_name(4100, 2));
        assert_ne!(unit_name(4100, 1), unit_name(4101, 1));
    }
}
