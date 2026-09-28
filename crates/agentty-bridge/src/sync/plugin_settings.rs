//! Plugin settings in the sync repository: `plugin/<plugin id>/settings/<device id>.json`, beside
//! that plugin's workspaces. What travels is a plugin's own store (`storage/set`, kept in
//! `<data dir>/plugin-data/<plugin>/storage.json`) — its settings, not the other files it writes
//! there, which may be caches or tied to one computer. Like the rest of the settings, it goes with
//! "Sync settings", each computer writes only its own file, and the change made last wins. A
//! store holding something shaped like a credential is neither uploaded nor replaced.

use super::model::{now_stamp, stamp_ms, stamp_to_ms};
use super::{safe_segment, SyncConfig};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

/// This computer's last known state of one plugin's settings (in `SyncConfig`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Mark {
    pub fingerprint: String,
    pub changed_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginSettingsFile {
    pub device_id: String,
    pub device_name: String,
    /// When the settings last changed on that computer (or were taken from another).
    pub changed_at: String,
    pub fingerprint: String,
    pub storage: Value,
}

/// What one pass did with the plugins' settings.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Step {
    /// Plugins whose settings were taken from another computer: (plugin id, that computer's name).
    pub applied: Vec<(String, String)>,
    /// Plugins whose settings stayed here because they hold a credential.
    pub held: Vec<String>,
}

/// Nothing in it is shaped like a credential (the transcript masking patterns).
pub fn holds_no_secret(value: &Value) -> bool {
    let text = value.to_string();
    super::mask::mask(&text) == text
}

fn storage_path(data: &Path, plugin: &str) -> PathBuf {
    data.join("plugin-data").join(plugin).join("storage.json")
}

fn settings_dir(root: &Path, plugin: &str) -> PathBuf {
    root.join("plugin").join(plugin).join("settings")
}

fn fingerprint(value: &Value) -> String {
    use sha2::{Digest, Sha256};
    // serde_json keeps object keys sorted, so the same settings always give the same text.
    let digest = Sha256::digest(value.to_string().as_bytes());
    digest.iter().take(16).map(|b| format!("{b:02x}")).collect()
}

fn others(root: &Path, plugin: &str, device_id: &str) -> Vec<PluginSettingsFile> {
    fs::read_dir(settings_dir(root, plugin))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| serde_json::from_slice::<PluginSettingsFile>(&fs::read(e.path()).ok()?).ok())
        .filter(|f| f.device_id != device_id && stamp_to_ms(&f.changed_at).is_some() && f.storage.is_object())
        .collect()
}

/// For each installed plugin: records whether its settings changed here, takes another
/// computer's newer ones, and writes this computer's file into the clone.
pub(super) fn step(root: &Path, data: &Path, config: &mut SyncConfig, plugins: &[String]) -> Result<Step> {
    let mut step = Step::default();
    for plugin in plugins.iter().filter(|p| safe_segment(p)) {
        let path = storage_path(data, plugin);
        let local: Option<Value> = fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).filter(Value::is_object);
        if local.as_ref().is_some_and(|v| !holds_no_secret(v)) {
            // Kept to this computer both ways: a token in it must not travel, and another
            // computer's copy must not drop it.
            step.held.push(plugin.clone());
            continue;
        }
        let others = others(root, plugin, &config.device_id);
        let current = local.as_ref().map(fingerprint).unwrap_or_default();
        let mark = config.plugin_settings.entry(plugin.clone()).or_default();
        if current != mark.fingerprint {
            // The first time, a computer joining others takes theirs.
            let first = mark.fingerprint.is_empty() && mark.changed_at.is_empty();
            mark.changed_at = if first && !others.is_empty() || local.is_none() { stamp_ms(0) } else { now_stamp() };
            mark.fingerprint = current.clone();
        }
        let mine = stamp_to_ms(&mark.changed_at).unwrap_or(0);
        let newest = others
            .into_iter()
            .filter(|f| f.fingerprint != current)
            .filter(|f| holds_no_secret(&f.storage))
            // No more than a plugin may keep itself (`storage/set`).
            .filter(|f| f.storage.to_string().len() <= crate::plugins::storage::MAX_BYTES)
            .max_by_key(|f| stamp_to_ms(&f.changed_at).unwrap_or(0))
            .filter(|f| stamp_to_ms(&f.changed_at).unwrap_or(0) > mine);
        let value = match newest {
            Some(file) => {
                let dir = path.parent().unwrap_or(&path).to_path_buf();
                fs::create_dir_all(&dir)?;
                super::repo::private_dir(&dir);
                crate::fsutil::write_private(&path, &serde_json::to_vec(&file.storage)?)?;
                mark.fingerprint = fingerprint(&file.storage);
                mark.changed_at = file.changed_at.clone();
                step.applied.push((plugin.clone(), file.device_name.clone()));
                Some(file.storage)
            }
            None => local,
        };
        // Nothing kept here yet: nothing to send.
        let Some(storage) = value else { continue };
        let file = PluginSettingsFile {
            device_id: config.device_id.clone(),
            device_name: config.device_name.clone(),
            changed_at: mark.changed_at.clone(),
            fingerprint: mark.fingerprint.clone(),
            storage,
        };
        super::session::write_json(&settings_dir(root, plugin).join(format!("{}.json", config.device_id)), &file)?;
    }
    Ok(step)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("agentty-plugin-settings-{name}-{}", super::super::new_id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn config(device: &str) -> SyncConfig {
        SyncConfig { device_id: device.into(), device_name: device.into(), settings: true, ..Default::default() }
    }

    fn keep(data: &Path, plugin: &str, value: &Value) {
        let path = storage_path(data, plugin);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, value.to_string()).unwrap();
    }

    fn kept(data: &Path, plugin: &str) -> Value {
        serde_json::from_slice(&fs::read(storage_path(data, plugin)).unwrap()).unwrap()
    }

    #[test]
    fn a_plugins_settings_follow_the_newest_change() {
        let root = temp("root");
        let (data_a, data_b) = (temp("a"), temp("b"));
        let (mut a, mut b) = (config("a"), config("b"));
        let plugins = vec!["launch".to_string()];
        keep(&data_a, "launch", &json!({ "region": "eu" }));
        step(&root, &data_a, &mut a, &plugins).unwrap();
        assert!(root.join("plugin/launch/settings/a.json").is_file());

        // B has the plugin but nothing kept yet: it takes A's.
        let taken = step(&root, &data_b, &mut b, &plugins).unwrap();
        assert_eq!(taken.applied, [("launch".to_string(), "a".to_string())]);
        assert_eq!(kept(&data_b, "launch"), json!({ "region": "eu" }));

        // B changes it later: A takes B's.
        std::thread::sleep(std::time::Duration::from_millis(5));
        keep(&data_b, "launch", &json!({ "region": "us" }));
        step(&root, &data_b, &mut b, &plugins).unwrap();
        let back = step(&root, &data_a, &mut a, &plugins).unwrap();
        assert_eq!(back.applied.len(), 1);
        assert_eq!(kept(&data_a, "launch"), json!({ "region": "us" }));
        // Settled: nothing more to take either way.
        assert!(step(&root, &data_a, &mut a, &plugins).unwrap().applied.is_empty());
        assert!(step(&root, &data_b, &mut b, &plugins).unwrap().applied.is_empty());
        for dir in [root, data_a, data_b] {
            let _ = fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn a_plugin_store_holding_a_credential_stays_here() {
        let root = temp("root");
        let (data_a, data_b) = (temp("a"), temp("b"));
        let (mut a, mut b) = (config("a"), config("b"));
        let plugins = vec!["launch".to_string()];
        let token = ["ghp_", &"Z".repeat(36)].concat();
        let secret = json!({ "token": token });
        keep(&data_a, "launch", &secret);
        let held = step(&root, &data_a, &mut a, &plugins).unwrap();
        assert_eq!(held.held, ["launch"]);
        assert!(!root.join("plugin/launch/settings/a.json").exists());

        // Another computer's newer settings do not replace it either.
        keep(&data_b, "launch", &json!({ "region": "us" }));
        step(&root, &data_b, &mut b, &plugins).unwrap();
        step(&root, &data_a, &mut a, &plugins).unwrap();
        assert_eq!(kept(&data_a, "launch"), secret);
        for dir in [root, data_a, data_b] {
            let _ = fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn plugin_ids_from_outside_never_become_paths() {
        let root = temp("root");
        let data = temp("data");
        let mut a = config("a");
        let step = step(&root, &data, &mut a, &["../escape".to_string()]).unwrap();
        assert_eq!(step, Step::default());
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(data);
    }
}
