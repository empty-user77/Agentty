//! `gh` is found from the `PATH` a GUI app starts with. An app opened from the Finder, the Dock,
//! the Start menu or a desktop launcher does not get the `PATH` a terminal has; Homebrew's folder
//! in particular is missing on macOS. Its own test binary: it changes this process's `PATH`.
//!
//! GitHub's runner images have `gh` installed, so this runs in CI only, on every platform.

#[test]
fn gh_is_found_from_a_gui_apps_path() {
    if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true") {
        return;
    }
    let gui_path = if cfg!(windows) {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        format!(r"{root}\System32;{root}")
    } else {
        "/usr/bin:/bin:/usr/sbin:/sbin".to_string()
    };
    std::env::set_var("PATH", gui_path);

    let gh = agentty_bridge::sync::repo::find_tool("gh").expect("gh is installed on the runner");
    assert!(gh.is_absolute(), "{}", gh.display());
    let output = agentty_bridge::sync::repo::gh_command().arg("--version").output().expect("gh runs");
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("gh version"));
    // What Settings → Integrations shows (signed in or not depends on the runner).
    assert_ne!(agentty_bridge::clone::gh_status(), agentty_bridge::clone::GhStatus::Missing);
}
