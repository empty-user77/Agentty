//! Browser profiles of plugins: a second (third, …) sign-in to the same site.
//!
//! A plugin names a profile (`"work"`, `"brand-b"`); Agentty gives it a store of cookies and site
//! data of its own, kept apart from the in-app browser and from the plugin's other profiles, so
//! each can be signed in to a different account. The names are the plugin's; which store a name
//! means is kept here (`browser-profiles.json`: plugin → name → store id) and nowhere a plugin can
//! write. `"default"` (or no name) is the in-app browser's own store.

use crate::webview::Profile;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

pub const DEFAULT: &str = "default";
/// Profiles one plugin may have.
pub const MAX_PROFILES: usize = 12;

type Registry = BTreeMap<String, BTreeMap<String, String>>;

static REGISTRY: Mutex<Option<Registry>> = Mutex::new(None);

fn path() -> PathBuf {
    agentty_bridge::fsutil::data_dir().join("browser-profiles.json")
}

fn with_registry<T>(write: bool, f: impl FnOnce(&mut Registry) -> T) -> T {
    let mut guard = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
    let registry =
        guard.get_or_insert_with(|| std::fs::read(path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default());
    let result = f(registry);
    if write {
        if let Ok(bytes) = serde_json::to_vec_pretty(registry) {
            let _ = agentty_bridge::fsutil::write_private(&path(), &bytes);
        }
    }
    result
}

/// Lower-case letters, digits, `-` and `_`, 1 to 32 of them.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 32 && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

fn bytes_of(id: &str) -> Option<[u8; 16]> {
    uuid::Uuid::parse_str(id).ok().map(|uuid| *uuid.as_bytes())
}

/// The store `name` means for `plugin`: `Ok(None)` for the default, a new store the first time a
/// name is used (`create`), an error for a name that is not one or one too many.
pub fn resolve(plugin: &str, name: Option<&str>, create: bool) -> Result<Profile, String> {
    let name = name.unwrap_or(DEFAULT).trim();
    if name == DEFAULT {
        return Ok(None);
    }
    if !valid_name(name) {
        return Err(format!("\"{name}\" is not a profile name (lower-case letters, digits, - and _)"));
    }
    if !crate::webview::profiles_supported() {
        return Err("browser profiles need macOS 14 or later".into());
    }
    with_registry(create, |registry| {
        let profiles = registry.entry(plugin.to_string()).or_default();
        if let Some(id) = profiles.get(name) {
            return bytes_of(id).map(Some).ok_or_else(|| "the profile's store is unreadable".to_string());
        }
        if !create {
            return Err(format!("no profile \"{name}\""));
        }
        if profiles.len() >= MAX_PROFILES {
            return Err(format!("at most {MAX_PROFILES} browser profiles per plugin"));
        }
        let uuid = uuid::Uuid::new_v4();
        profiles.insert(name.to_string(), uuid.to_string());
        Ok(Some(*uuid.as_bytes()))
    })
}

/// The profile names `plugin` has made (the default is not listed).
pub fn names(plugin: &str) -> Vec<String> {
    with_registry(false, |registry| registry.get(plugin).map(|p| p.keys().cloned().collect()).unwrap_or_default())
}

/// Deletes a profile: its sign-ins and site data go with it.
pub fn remove(plugin: &str, name: &str) -> bool {
    let removed = with_registry(true, |registry| registry.get_mut(plugin).and_then(|p| p.remove(name)));
    match removed.as_deref().and_then(bytes_of) {
        Some(bytes) => {
            crate::webview::remove_profile(bytes);
            true
        }
        None => false,
    }
}

/// Every profile of a plugin that is being uninstalled.
pub fn remove_all(plugin: &str) {
    for name in names(plugin) {
        remove(plugin, &name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_names_are_plain() {
        assert!(valid_name("brand-b"));
        assert!(valid_name("work_2"));
        assert!(!valid_name(""));
        assert!(!valid_name("Work"));
        assert!(!valid_name("../x"));
        assert!(!valid_name(&"a".repeat(33)));
    }

    #[test]
    fn the_default_is_the_browser_itself() {
        assert_eq!(resolve("x", None, false), Ok(None));
        assert_eq!(resolve("x", Some("default"), true), Ok(None));
        assert!(resolve("x", Some("Not Valid"), true).is_err());
    }
}
