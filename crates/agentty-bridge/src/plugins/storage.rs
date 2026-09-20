//! `storage/get`, `storage/set` and `storage/keys`: what a plugin remembers between runs.
//!
//! A plugin that runs as a process can write its own files in `AGENTTY_PLUGIN_DATA`. A
//! WebAssembly plugin has no files at all, so this is how it keeps anything — environments,
//! saved requests, what the user last typed. It is the plugin's own folder and nothing else:
//! the key names a value inside one JSON document under
//! `<data dir>/plugin-data/<plugin>/storage.json`, which is created `0600`.

use anyhow::{bail, Result};
use serde_json::{Map, Value};
use std::path::PathBuf;

/// Keys one plugin may keep.
pub const MAX_KEYS: usize = 64;
const MAX_KEY_CHARS: usize = 64;
/// Everything one plugin may keep, serialized.
pub const MAX_BYTES: usize = 1024 * 1024;

fn path(plugin: &str) -> PathBuf {
    super::store::plugin_data_dir(plugin).join("storage.json")
}

fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.chars().count() <= MAX_KEY_CHARS
        && key.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-' | '_'))
}

fn read(plugin: &str) -> Map<String, Value> {
    std::fs::read(path(plugin))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default()
}

fn write(plugin: &str, document: &Map<String, Value>) -> Result<()> {
    let bytes = serde_json::to_vec(document)?;
    if bytes.len() > MAX_BYTES {
        bail!("a plugin may keep at most {} KB", MAX_BYTES / 1024);
    }
    let path = path(plugin);
    let dir = path.parent().unwrap_or(&path).to_path_buf();
    std::fs::create_dir_all(&dir)?;
    // What a plugin keeps is the user's: their saved requests, their tokens, their notes.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// One value, or `null` when it was never set.
pub fn get(plugin: &str, key: &str) -> Result<Value> {
    if !valid_key(key) {
        bail!("\"{key}\" is not a storage key (a-z, 0-9, '.', '-', '_')");
    }
    Ok(read(plugin).get(key).cloned().unwrap_or(Value::Null))
}

/// Sets a value; `null` removes it.
pub fn set(plugin: &str, key: &str, value: Value) -> Result<()> {
    if !valid_key(key) {
        bail!("\"{key}\" is not a storage key (a-z, 0-9, '.', '-', '_')");
    }
    let mut document = read(plugin);
    if value.is_null() {
        document.remove(key);
    } else {
        if !document.contains_key(key) && document.len() >= MAX_KEYS {
            bail!("a plugin may keep at most {MAX_KEYS} keys");
        }
        document.insert(key.to_string(), value);
    }
    write(plugin, &document)
}

pub fn keys(plugin: &str) -> Vec<String> {
    read(plugin).keys().cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Everything here writes under the data directory, which the store's tests move aside.
    fn with_plugin(test: impl FnOnce(&str)) {
        super::super::store::tests::with_data_dir(|_| test("storage-test"));
    }

    #[test]
    fn keeps_values_between_calls() {
        with_plugin(|plugin| {
            assert_eq!(get(plugin, "env").unwrap(), Value::Null);
            set(plugin, "env", serde_json::json!({ "base": "https://example.com" })).unwrap();
            assert_eq!(get(plugin, "env").unwrap()["base"], "https://example.com");
            assert_eq!(keys(plugin), ["env"]);
            // Null removes it rather than storing a null.
            set(plugin, "env", Value::Null).unwrap();
            assert!(keys(plugin).is_empty());
        });
    }

    #[test]
    fn refuses_keys_that_are_not_keys_and_stops_at_the_limits() {
        with_plugin(|plugin| {
            for bad in ["", "../escape", "Upper", "with space", &"k".repeat(MAX_KEY_CHARS + 1)] {
                assert!(set(plugin, bad, serde_json::json!(1)).is_err(), "accepted {bad:?}");
                assert!(get(plugin, bad).is_err(), "accepted {bad:?}");
            }
            for i in 0..MAX_KEYS {
                set(plugin, &format!("k{i}"), serde_json::json!(i)).unwrap();
            }
            assert!(set(plugin, "one-too-many", serde_json::json!(1)).is_err());
            // An existing key can still be written.
            set(plugin, "k0", serde_json::json!("changed")).unwrap();
            assert!(set(plugin, "k0", serde_json::json!("x".repeat(MAX_BYTES))).is_err(), "too large");
        });
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        with_plugin(|plugin| {
            set(plugin, "token", serde_json::json!("example_not_a_real_value")).unwrap();
            let mode = std::fs::metadata(path(plugin)).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "{mode:o}");
        });
    }
}
