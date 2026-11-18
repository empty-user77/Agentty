//! Claude Code's own computer use (its built-in `computer-use` MCP server: screenshots, pointer and
//! keyboard on the user's desktop, each application approved by the user in Claude Code): whether
//! this Claude Code has it, and whether it is on for a project.
//!
//! Read from Claude Code's config file (`.claude.json`), which is never written here: the server is
//! turned on in Claude Code itself (`/mcp enable computer-use`), per project.

use crate::agent_auth::AuthAgent;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// The server's name in Claude Code.
pub const SERVER: &str = "computer-use";

/// What to type in a Claude Code session to turn the server on for its project.
pub const ENABLE_COMMAND: &str = "/mcp enable computer-use";

/// What to type there to turn it off again.
pub const DISABLE_COMMAND: &str = "/mcp disable computer-use";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComputerUse {
    /// Nothing says this Claude Code knows computer use (another system, an older version, a
    /// config that can't be read).
    Unknown,
    /// It has it, and it is not on for this project.
    Off,
    /// On for this project: a session there can ask the user for applications to control.
    On,
}

/// Claude Code's config file of an account; `None` is the user's own login.
fn config_file(account: Option<&str>) -> Option<PathBuf> {
    match account {
        None => Some(crate::fsutil::home().join(".claude.json")),
        Some(id) => crate::accounts::home(AuthAgent::Claude, id).ok().map(|home| home.join(".claude.json")),
    }
}

/// Computer use for a session in `cwd`. Reads a file: call it off the main thread.
pub fn state(cwd: &Path, account: Option<&str>) -> ComputerUse {
    // macOS only: it is where Claude Code offers it.
    if !cfg!(target_os = "macos") {
        return ComputerUse::Unknown;
    }
    let config =
        config_file(account).and_then(|path| std::fs::read(path).ok()).and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    match config {
        Some(config) => state_in(&config, &project_keys(cwd)),
        None => ComputerUse::Unknown,
    }
}

/// The names a project can have in the config: the folder a session runs in, the working tree it
/// is in and the repository's main tree (a session started in the main folder and moved into a
/// worktree keeps the main folder's entry).
fn project_keys(cwd: &Path) -> Vec<PathBuf> {
    let mut keys = vec![cwd.to_path_buf()];
    for other in [crate::worktree::tree_root(cwd), crate::worktree::main_tree(cwd)].into_iter().flatten() {
        if !keys.contains(&other) {
            keys.push(other);
        }
    }
    keys
}

fn enables(project: &Value) -> bool {
    project["enabledMcpServers"].as_array().is_some_and(|servers| servers.iter().any(|s| s.as_str() == Some(SERVER)))
}

fn state_in(config: &Value, projects: &[PathBuf]) -> ComputerUse {
    let entries = config["projects"].as_object();
    let on = projects.iter().any(|path| entries.and_then(|e| e.get(path.to_string_lossy().as_ref())).is_some_and(enables));
    if on {
        return ComputerUse::On;
    }
    // Known to this Claude Code: some project has it on, or its feature settings name it (the
    // server itself is built in and listed nowhere while it is off).
    let used_elsewhere = entries.is_some_and(|e| e.values().any(enables));
    let named = ["cachedGrowthBookFeatures", "cachedFeatureFlags"].iter().any(|key| names_computer_use(&config[*key], 0));
    if used_elsewhere || named {
        ComputerUse::Off
    } else {
        ComputerUse::Unknown
    }
}

/// Whether a setting anywhere in `value` is about computer use (`computerUse…`), a few levels down.
fn names_computer_use(value: &Value, depth: u8) -> bool {
    match value {
        Value::Object(map) if depth < 4 => {
            map.iter().any(|(key, inner)| key.to_ascii_lowercase().contains("computeruse") || names_computer_use(inner, depth + 1))
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn keys(paths: &[&str]) -> Vec<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn it_is_on_where_the_project_enables_it() {
        let config = json!({ "projects": {
            "/work/app": { "enabledMcpServers": ["computer-use"] },
            "/work/site": { "enabledMcpServers": ["figma"], "disabledMcpServers": ["computer-use"] },
        } });
        assert_eq!(state_in(&config, &keys(&["/work/app"])), ComputerUse::On);
        // A worktree of the project: the main folder's entry counts.
        assert_eq!(state_in(&config, &keys(&["/work/app/.claude/worktrees/x", "/work/app"])), ComputerUse::On);
        // Another project: known (it is on somewhere), not on here.
        assert_eq!(state_in(&config, &keys(&["/work/site"])), ComputerUse::Off);
        assert_eq!(state_in(&config, &keys(&["/work/new"])), ComputerUse::Off);
    }

    #[test]
    fn it_is_known_from_the_feature_settings_and_unknown_without_any_sign() {
        let named = json!({ "projects": { "/work/app": {} }, "cachedGrowthBookFeatures": { "some_flag": { "computerUseChecks": true } } });
        assert_eq!(state_in(&named, &keys(&["/work/app"])), ComputerUse::Off);
        let silent =
            json!({ "projects": { "/work/app": { "enabledMcpServers": ["figma"] } }, "cachedGrowthBookFeatures": { "other": true } });
        assert_eq!(state_in(&silent, &keys(&["/work/app"])), ComputerUse::Unknown);
        assert_eq!(state_in(&json!({}), &keys(&["/work/app"])), ComputerUse::Unknown);
    }
}
