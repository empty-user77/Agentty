//! Settings in the sync repository: `settings/<device id>.json`, one file per computer, each written
//! only by its computer like everything else here. The configuration changed last wins: a computer
//! that finds another's newer than its own last change takes it, and records that it did so it does
//! not send it back as a change of its own.
//!
//! Only [`backup::Scope::Sync`] travels (no secrets, no plugins, nothing tied to one computer).

use super::model::{now_stamp, stamp_ms, stamp_to_ms};
use super::SyncConfig;
use crate::backup::{self, Bundle};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

pub const FOLDER: &str = "settings";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsFile {
    pub device_id: String,
    pub device_name: String,
    /// When the configuration last changed on that computer (or was taken from another).
    pub changed_at: String,
    pub fingerprint: String,
    pub bundle: Bundle,
}

/// Another computer's configuration, newer than this one's.
#[derive(Debug, Clone, PartialEq)]
pub struct Incoming {
    pub device_name: String,
    pub changed_at: String,
    pub bundle: Bundle,
}

fn others(root: &Path, device_id: &str) -> Vec<SettingsFile> {
    let Ok(entries) = fs::read_dir(root.join(FOLDER)) else { return Vec::new() };
    entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| serde_json::from_slice::<SettingsFile>(&fs::read(e.path()).ok()?).ok())
        .filter(|f| f.device_id != device_id && stamp_to_ms(&f.changed_at).is_some())
        .collect()
}

/// What one pass did with the settings.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Step {
    /// Another computer's configuration, newer than this one's.
    pub incoming: Option<Incoming>,
    /// Parts not uploaded because they hold something shaped like a credential (`settings.json`,
    /// a configuration file's name, or `themes/<name>`). Other computers keep their own.
    pub held: Vec<String>,
}

/// `local` without the parts that hold something shaped like a credential, and their names. The
/// repository is private, but a token typed into a custom command should not travel at all; the
/// user is told instead (Settings → Sync).
pub fn hold_back_secrets(local: &Bundle) -> (Bundle, Vec<String>) {
    let shaped = |text: &str| super::mask::mask(text) != text;
    let mut clean = local.clone();
    let mut held = Vec::new();
    if clean.settings.as_ref().is_some_and(|v| shaped(&v.to_string())) {
        clean.settings = None;
        held.push("settings.json".to_string());
    }
    clean.files.retain(|name, value| {
        let keep = !shaped(&value.to_string());
        if !keep {
            held.push(name.clone());
        }
        keep
    });
    clean.themes.retain(|name, text| {
        let keep = !shaped(text);
        if !keep {
            held.push(format!("themes/{name}"));
        }
        keep
    });
    (clean, held)
}

/// Records whether this computer's configuration changed, writes its file into the clone (without
/// the parts holding credentials) and returns another computer's configuration when that one is
/// newer.
pub(super) fn step(root: &Path, config: &mut SyncConfig, local: &Bundle) -> Result<Step> {
    let others = others(root, &config.device_id);
    let fingerprint = backup::fingerprint(local);
    if fingerprint != config.settings_fingerprint {
        // The first time, a computer joining others takes theirs: its own settings have not been
        // "changed" after theirs, they were only there first.
        let first = config.settings_fingerprint.is_empty();
        config.settings_changed_at = if first && !others.is_empty() { stamp_ms(0) } else { now_stamp() };
        config.settings_fingerprint = fingerprint.clone();
    }
    let mine = stamp_to_ms(&config.settings_changed_at).unwrap_or(0);
    let newest = others.into_iter().filter(|f| f.fingerprint != fingerprint).max_by_key(|f| stamp_to_ms(&f.changed_at).unwrap_or(0));
    let incoming = newest.filter(|f| stamp_to_ms(&f.changed_at).unwrap_or(0) > mine).map(|f| Incoming {
        device_name: f.device_name,
        changed_at: f.changed_at,
        bundle: f.bundle,
    });
    // The fingerprint stays the whole configuration's, so holding a part back is not a change.
    let (clean, held) = hold_back_secrets(local);
    let file = SettingsFile {
        device_id: config.device_id.clone(),
        device_name: config.device_name.clone(),
        changed_at: config.settings_changed_at.clone(),
        fingerprint,
        bundle: Bundle { created_at: String::new(), ..clean },
    };
    super::session::write_json(&root.join(FOLDER).join(format!("{}.json", config.device_id)), &file)?;
    Ok(Step { incoming, held })
}

/// After the app put `incoming` in place: this computer's configuration is now the one changed
/// at `changed_at`, whatever small differences its own files keep (a newer app writing a field
/// the other lacks), so they are not sent back as a new change.
pub fn applied(changed_at: &str, app_version: &str) -> Result<()> {
    let local = backup::collect(backup::Scope::Sync, None, app_version)?;
    let fingerprint = backup::fingerprint(&local);
    super::update_config(|c| {
        c.settings_fingerprint = fingerprint;
        c.settings_changed_at = changed_at.to_string();
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn config(device: &str) -> SyncConfig {
        SyncConfig { device_id: device.into(), device_name: device.into(), settings: true, ..Default::default() }
    }

    fn bundle(theme: &str) -> Bundle {
        Bundle { kind: backup::KIND.into(), version: 1, settings: Some(json!({ "theme": theme })), ..Default::default() }
    }

    fn root() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("agentty-sync-settings-{}", super::super::new_id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn parts_holding_credentials_stay_on_this_computer() {
        let root = root();
        let mut a = config("a");
        let token = ["ghp_", &"Z".repeat(36)].concat();
        let mut local = bundle("Nord");
        local
            .files
            .insert("commands.json".into(), json!([{ "name": "deploy", "command": format!("curl -H 'Authorization: token {token}'") }]));
        local.files.insert("connectors.json".into(), json!([{ "name": "api" }]));
        let step = step(&root, &mut a, &local).unwrap();
        assert_eq!(step.held, ["commands.json"]);
        let written = fs::read_to_string(root.join(FOLDER).join("a.json")).unwrap();
        assert!(!written.contains(&token));
        assert!(written.contains("connectors.json"));
        // Another computer taking it keeps its own commands (the file is not in the bundle).
        let mut b = config("b");
        let incoming = super::step(&root, &mut b, &bundle("Default")).unwrap().incoming.unwrap();
        assert!(!incoming.bundle.files.contains_key("commands.json"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_computer_joining_takes_the_settings_already_there() {
        let root = root();
        let mut a = config("a");
        assert_eq!(step(&root, &mut a, &bundle("Nord")).unwrap().incoming, None);
        let mut b = config("b");
        let incoming = step(&root, &mut b, &bundle("Default")).unwrap().incoming.expect("b takes a's");
        assert_eq!(incoming.bundle.settings.unwrap()["theme"], "Nord");
        // A does not take b's, which was never changed.
        assert_eq!(step(&root, &mut a, &bundle("Nord")).unwrap().incoming, None);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn the_last_change_wins_and_is_not_sent_back() {
        let root = root();
        let mut a = config("a");
        let mut b = config("b");
        step(&root, &mut a, &bundle("Nord")).unwrap();
        step(&root, &mut b, &bundle("Default")).unwrap().incoming.unwrap();
        // B takes it: from now on b has a's configuration, changed when a changed it.
        b.settings_fingerprint = backup::fingerprint(&bundle("Nord"));
        b.settings_changed_at = a.settings_changed_at.clone();
        assert_eq!(step(&root, &mut b, &bundle("Nord")).unwrap().incoming, None);
        std::thread::sleep(std::time::Duration::from_millis(5));
        // B changes its theme later: a takes it.
        assert_eq!(step(&root, &mut b, &bundle("Dracula")).unwrap().incoming, None);
        let incoming = step(&root, &mut a, &bundle("Nord")).unwrap().incoming.expect("a takes b's change");
        assert_eq!(incoming.bundle.settings.unwrap()["theme"], "Dracula");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn the_same_settings_everywhere_bring_nothing() {
        let root = root();
        let mut a = config("a");
        let mut b = config("b");
        step(&root, &mut a, &bundle("Nord")).unwrap();
        assert_eq!(step(&root, &mut b, &bundle("Nord")).unwrap().incoming, None);
        let _ = fs::remove_dir_all(root);
    }
}
