//! `storage/get`, `storage/set` and `storage/keys`: what a plugin remembers between runs.
//!
//! A plugin that runs as a process can write its own files in `AGENTTY_PLUGIN_DATA`. A
//! WebAssembly plugin has no files at all, so this is how it keeps anything — environments,
//! saved requests, what the user last typed. It is the plugin's own folder and nothing else:
//! the key names a value inside one JSON document under
//! `<data dir>/plugin-data/<plugin>/storage.json`, which is created `0600`.

use anyhow::{bail, Context, Result};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// Keys one plugin may keep.
pub const MAX_KEYS: usize = 64;
const MAX_KEY_CHARS: usize = 64;
/// Everything one plugin may keep, serialized.
pub const MAX_BYTES: usize = 1024 * 1024;

fn path(plugin: &str) -> PathBuf {
    super::store::plugin_data_dir(plugin).join("storage.json")
}

/// Whether a plugin named a key at all — told apart from the folder being unreadable, so the
/// plugin knows whether retrying could ever work.
pub fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.chars().count() <= MAX_KEY_CHARS
        && key.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-' | '_'))
}

/// What the plugin has kept.
///
/// Three cases, and they are not the same. A file that is not there yet is an empty document. A
/// file that is there but cannot be read is an error: answering "empty" would let the next
/// `set` write a document with one key in it over everything the plugin had. A file that is no
/// longer JSON is moved aside once — kept, so nothing is destroyed, but out of the way, so a
/// plugin is not stuck on it forever.
fn read(plugin: &str) -> Result<Map<String, Value>> {
    let path = path(plugin);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(err) => return Err(err).context("this plugin's storage cannot be read"),
    };
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(Value::Object(document)) => Ok(document),
        _ => {
            set_aside(&path)?;
            Ok(Map::new())
        }
    }
}

/// Renames a storage file that is no longer readable to `storage.corrupt.json`, so what was in it
/// survives for anyone who wants to look, and the plugin starts again from empty. An older one is
/// replaced: two is already more than anybody reads.
fn set_aside(path: &Path) -> Result<()> {
    let aside = path.with_file_name("storage.corrupt.json");
    std::fs::rename(path, &aside).context("this plugin's storage is damaged and could not be moved aside")?;
    // It holds whatever the plugin kept — a file written before this was so may be 0644.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&aside, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
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
    // Created 0600 rather than created and then narrowed: between the two there is a moment in
    // which what a plugin keeps — tokens among it — is readable by anyone on the machine.
    let tmp = path.with_extension("json.tmp");
    let written = (|| -> std::io::Result<()> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        std::io::Write::write_all(&mut file, &bytes)?;
        file.sync_all()
    })();
    if let Err(err) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(err).context("this plugin's storage could not be written");
    }
    // An older file may already be 0644 from before this was so; the rename keeps the new mode.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    if let Err(err) = std::fs::rename(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(err).context("this plugin's storage could not be written");
    }
    Ok(())
}

/// One value, or `null` when it was never set.
pub fn get(plugin: &str, key: &str) -> Result<Value> {
    if !valid_key(key) {
        bail!("\"{key}\" is not a storage key (a-z, 0-9, '.', '-', '_')");
    }
    Ok(read(plugin)?.get(key).cloned().unwrap_or(Value::Null))
}

/// Sets a value; `null` removes it.
pub fn set(plugin: &str, key: &str, value: Value) -> Result<()> {
    if !valid_key(key) {
        bail!("\"{key}\" is not a storage key (a-z, 0-9, '.', '-', '_')");
    }
    let mut document = read(plugin)?;
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

pub fn keys(plugin: &str) -> Result<Vec<String>> {
    Ok(read(plugin)?.keys().cloned().collect())
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
            assert_eq!(keys(plugin).unwrap(), ["env"]);
            // Null removes it rather than storing a null.
            set(plugin, "env", Value::Null).unwrap();
            assert!(keys(plugin).unwrap().is_empty());
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

    #[test]
    fn a_damaged_file_is_kept_but_moved_out_of_the_way() {
        with_plugin(|plugin| {
            set(plugin, "env", serde_json::json!("before")).unwrap();
            std::fs::write(path(plugin), b"{ this is not json").unwrap();
            // The plugin starts again from empty rather than reading rubbish...
            assert_eq!(keys(plugin).unwrap(), Vec::<String>::new());
            // ...and what was there is still on disk, under a name nothing reads.
            let aside = path(plugin).with_file_name("storage.corrupt.json");
            assert_eq!(std::fs::read(&aside).unwrap(), b"{ this is not json");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(std::fs::metadata(&aside).unwrap().permissions().mode() & 0o777, 0o600);
            }
            // Writing again works, and writes a fresh document.
            set(plugin, "env", serde_json::json!("after")).unwrap();
            assert_eq!(get(plugin, "env").unwrap(), "after");
        });
    }

    #[test]
    fn a_document_that_is_not_an_object_is_not_read_as_one() {
        with_plugin(|plugin| {
            std::fs::create_dir_all(path(plugin).parent().unwrap()).unwrap();
            std::fs::write(path(plugin), b"[1, 2, 3]").unwrap();
            assert_eq!(get(plugin, "env").unwrap(), Value::Null);
            assert!(path(plugin).with_file_name("storage.corrupt.json").exists());
        });
    }

    #[cfg(unix)]
    #[test]
    fn storage_that_cannot_be_read_is_an_error_and_not_an_empty_document() {
        use std::os::unix::fs::PermissionsExt;
        with_plugin(|plugin| {
            set(plugin, "token", serde_json::json!("example_not_a_real_value")).unwrap();
            let file = path(plugin);
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
            // Reading as root ignores the mode; skip rather than claim something that was not shown.
            if std::fs::read(&file).is_ok() {
                return;
            }
            assert!(get(plugin, "token").is_err());
            assert!(keys(plugin).is_err());
            // And a write does not quietly replace what it could not read.
            assert!(set(plugin, "other", serde_json::json!(1)).is_err());
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
            assert_eq!(get(plugin, "token").unwrap(), "example_not_a_real_value");
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
