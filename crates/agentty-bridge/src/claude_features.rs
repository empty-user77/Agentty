//! Claude Code features turned on or off in its user settings (`settings.json` in its config folder,
//! `~/.claude` unless `CLAUDE_CONFIG_DIR` says otherwise), for Settings → Agents.
//!
//! Only the one key a switch stands for is written; everything else in the file is kept as it is
//! (key order too), and the file is replaced atomically with its mode. A file that is not JSON is
//! never overwritten: the switch reports the error instead.

use anyhow::{Context, Result};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudeFeature {
    /// Agent teams: a lead session that starts and coordinates teammate sessions
    /// (`env.CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS = "1"`).
    AgentTeams,
    /// Extended thinking on for every request (`alwaysThinkingEnabled`).
    AlwaysThinking,
    /// Claude Code updates itself in the background; off sets `env.DISABLE_AUTOUPDATER = "1"`.
    AutoUpdates,
}

const AGENT_TEAMS: &str = "CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS";
const NO_AUTO_UPDATES: &str = "DISABLE_AUTOUPDATER";

impl ClaudeFeature {
    pub const ALL: [ClaudeFeature; 3] = [Self::AgentTeams, Self::AlwaysThinking, Self::AutoUpdates];

    /// Whether the feature is on in `settings` (the file's JSON).
    pub fn is_on(self, settings: &Value) -> bool {
        let env_set = |name: &str| settings["env"][name].as_str().is_some_and(|v| !matches!(v.trim(), "" | "0" | "false"));
        match self {
            Self::AgentTeams => env_set(AGENT_TEAMS),
            Self::AlwaysThinking => settings["alwaysThinkingEnabled"].as_bool() == Some(true),
            Self::AutoUpdates => !env_set(NO_AUTO_UPDATES),
        }
    }

    /// Writes the feature's key into `settings` (an object) so that it reads as `on`.
    fn apply(self, settings: &mut Map<String, Value>, on: bool) {
        let mut set_env = |name: &str, value: bool| {
            let env = settings.entry("env").or_insert_with(|| Value::Object(Map::new()));
            if !env.is_object() {
                *env = Value::Object(Map::new());
            }
            let env = env.as_object_mut().expect("made an object above");
            if value {
                env.insert(name.into(), Value::String("1".into()));
            } else {
                env.remove(name);
            }
            if env.is_empty() {
                settings.remove("env");
            }
        };
        match self {
            Self::AgentTeams => set_env(AGENT_TEAMS, on),
            Self::AutoUpdates => set_env(NO_AUTO_UPDATES, !on),
            Self::AlwaysThinking => {
                if on {
                    settings.insert("alwaysThinkingEnabled".into(), Value::Bool(true));
                } else {
                    settings.remove("alwaysThinkingEnabled");
                }
            }
        }
    }
}

/// Claude Code's user settings file.
pub fn settings_path() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| crate::fsutil::home().join(".claude")).join("settings.json")
}

/// Which features are on now: `Err` when the file exists but is not JSON.
pub fn read() -> Result<Vec<(ClaudeFeature, bool)>> {
    read_in(&settings_path())
}

fn read_in(path: &Path) -> Result<Vec<(ClaudeFeature, bool)>> {
    let settings = load(path)?;
    Ok(ClaudeFeature::ALL.into_iter().map(|feature| (feature, feature.is_on(&settings))).collect())
}

fn load(path: &Path) -> Result<Value> {
    match std::fs::read(path) {
        Ok(bytes) if bytes.iter().all(u8::is_ascii_whitespace) => Ok(Value::Object(Map::new())),
        Ok(bytes) => serde_json::from_slice(&bytes).with_context(|| format!("{} is not valid JSON", path.display())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Value::Object(Map::new())),
        Err(err) => Err(err).with_context(|| format!("cannot read {}", path.display())),
    }
}

/// Turns `feature` on or off in Claude Code's user settings. New Claude Code sessions pick it up.
pub fn set(feature: ClaudeFeature, on: bool) -> Result<()> {
    set_in(&settings_path(), feature, on)
}

fn set_in(path: &Path, feature: ClaudeFeature, on: bool) -> Result<()> {
    let mut settings = load(path)?;
    let Some(object) = settings.as_object_mut() else { anyhow::bail!("{} is not a JSON object", path.display()) };
    feature.apply(object, on);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut text = serde_json::to_vec_pretty(&settings)?;
    text.push(b'\n');
    crate::claude_trust::replace_keeping_mode(path, &text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("agentty-claude-features-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("settings.json")
    }

    #[test]
    fn switches_write_only_their_own_key() {
        let path = temp_file("keys");
        std::fs::write(&path, r#"{"model":"opus","env":{"FOO":"bar"},"permissions":{"allow":[]}}"#).unwrap();
        set_in(&path, ClaudeFeature::AgentTeams, true).unwrap();
        set_in(&path, ClaudeFeature::AlwaysThinking, true).unwrap();
        let value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(value["model"], "opus");
        assert_eq!(value["env"]["FOO"], "bar");
        assert_eq!(value["env"][AGENT_TEAMS], "1");
        assert_eq!(value["alwaysThinkingEnabled"], true);
        // Key order is the file's own: the new keys come after the ones that were there.
        let keys: Vec<&String> = value.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["model", "env", "permissions", "alwaysThinkingEnabled"]);

        set_in(&path, ClaudeFeature::AgentTeams, false).unwrap();
        let value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(value["env"].get(AGENT_TEAMS).is_none());
        assert_eq!(value["env"]["FOO"], "bar");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_missing_file_is_created_and_an_empty_env_goes_away() {
        let path = temp_file("missing");
        assert!(read_in(&path).unwrap().contains(&(ClaudeFeature::AutoUpdates, true)));
        set_in(&path, ClaudeFeature::AutoUpdates, false).unwrap();
        assert!(read_in(&path).unwrap().contains(&(ClaudeFeature::AutoUpdates, false)));
        set_in(&path, ClaudeFeature::AutoUpdates, true).unwrap();
        let value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(value, serde_json::json!({}));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_broken_file_is_never_overwritten() {
        let path = temp_file("broken");
        std::fs::write(&path, "{ not json").unwrap();
        assert!(read_in(&path).is_err());
        assert!(set_in(&path, ClaudeFeature::AgentTeams, true).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn off_values_read_as_off() {
        let settings = serde_json::json!({"env": {AGENT_TEAMS: "0", NO_AUTO_UPDATES: "1"}});
        assert!(!ClaudeFeature::AgentTeams.is_on(&settings));
        assert!(!ClaudeFeature::AutoUpdates.is_on(&settings));
        assert!(!ClaudeFeature::AlwaysThinking.is_on(&settings));
    }
}
