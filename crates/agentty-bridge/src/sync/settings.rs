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

/// Records whether this computer's configuration changed, writes its file into the clone and
/// returns another computer's configuration when that one is newer.
pub(super) fn step(root: &Path, config: &mut SyncConfig, local: &Bundle) -> Result<Option<Incoming>> {
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
    let file = SettingsFile {
        device_id: config.device_id.clone(),
        device_name: config.device_name.clone(),
        changed_at: config.settings_changed_at.clone(),
        fingerprint,
        bundle: Bundle { created_at: String::new(), ..local.clone() },
    };
    super::session::write_json(&root.join(FOLDER).join(format!("{}.json", config.device_id)), &file)?;
    Ok(incoming)
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
    fn a_computer_joining_takes_the_settings_already_there() {
        let root = root();
        let mut a = config("a");
        assert_eq!(step(&root, &mut a, &bundle("Nord")).unwrap(), None);
        let mut b = config("b");
        let incoming = step(&root, &mut b, &bundle("Default")).unwrap().expect("b takes a's");
        assert_eq!(incoming.bundle.settings.unwrap()["theme"], "Nord");
        // A does not take b's, which was never changed.
        assert_eq!(step(&root, &mut a, &bundle("Nord")).unwrap(), None);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn the_last_change_wins_and_is_not_sent_back() {
        let root = root();
        let mut a = config("a");
        let mut b = config("b");
        step(&root, &mut a, &bundle("Nord")).unwrap();
        step(&root, &mut b, &bundle("Default")).unwrap().unwrap();
        // B takes it: from now on b has a's configuration, changed when a changed it.
        b.settings_fingerprint = backup::fingerprint(&bundle("Nord"));
        b.settings_changed_at = a.settings_changed_at.clone();
        assert_eq!(step(&root, &mut b, &bundle("Nord")).unwrap(), None);
        std::thread::sleep(std::time::Duration::from_millis(5));
        // B changes its theme later: a takes it.
        assert_eq!(step(&root, &mut b, &bundle("Dracula")).unwrap(), None);
        let incoming = step(&root, &mut a, &bundle("Nord")).unwrap().expect("a takes b's change");
        assert_eq!(incoming.bundle.settings.unwrap()["theme"], "Dracula");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn the_same_settings_everywhere_bring_nothing() {
        let root = root();
        let mut a = config("a");
        let mut b = config("b");
        step(&root, &mut a, &bundle("Nord")).unwrap();
        assert_eq!(step(&root, &mut b, &bundle("Nord")).unwrap(), None);
        let _ = fs::remove_dir_all(root);
    }
}
