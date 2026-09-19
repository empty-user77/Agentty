//! Claude Code asks "Do you trust the files in this folder?" the first time it starts in a folder,
//! and a session's working tree is always a new folder. A working tree of a project the user already
//! trusts holds the same repository, so it inherits that trust: the session starts at once instead of
//! waiting at the question. A project the user never trusted (nor any folder above it) stays asked.
//!
//! Only the `hasTrustDialogAccepted` flag of the new tree's entry in `~/.claude.json` is written;
//! everything else in the file is kept as it is, and the file is replaced atomically with its mode.

use anyhow::{Context, Result};
use serde_json::Value;
use std::path::Path;

/// Marks `tree` trusted when `project` (or a folder above it) is. Returns whether it did.
pub fn inherit_trust(project: &Path, tree: &Path) -> Result<bool> {
    inherit_trust_in(&crate::fsutil::home().join(".claude.json"), project, tree)
}

fn trusted(config: &Value, path: &Path) -> bool {
    path.ancestors().any(|dir| config["projects"][dir.to_string_lossy().as_ref()]["hasTrustDialogAccepted"].as_bool() == Some(true))
}

fn inherit_trust_in(config_path: &Path, project: &Path, tree: &Path) -> Result<bool> {
    let Ok(bytes) = std::fs::read(config_path) else { return Ok(false) };
    let mut config: Value = serde_json::from_slice(&bytes).context("~/.claude.json is not JSON")?;
    if !trusted(&config, project) || trusted(&config, tree) {
        return Ok(false);
    }
    let Some(projects) = config.get_mut("projects").and_then(Value::as_object_mut) else { return Ok(false) };
    let entry = projects.entry(tree.to_string_lossy().to_string()).or_insert_with(|| Value::Object(Default::default()));
    let Some(entry) = entry.as_object_mut() else { return Ok(false) };
    entry.insert("hasTrustDialogAccepted".into(), Value::Bool(true));
    let text = serde_json::to_vec_pretty(&config)?;
    let tmp = config_path.with_extension(format!("json.agentty-{}", std::process::id()));
    // The file holds sign-in details and MCP server arguments: the copy is private from the start
    // (0600), never readable by others between writing and renaming, then takes the original's mode.
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    {
        use std::io::Write;
        let mut file = options.open(&tmp)?;
        file.write_all(&text)?;
        file.sync_all()?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(config_path).map(|m| m.permissions().mode()).unwrap_or(0o600); // audit: ok — capped at 0600 below
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode & 0o600))?;
    }
    std::fs::rename(&tmp, config_path)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tree_of_a_trusted_project_is_trusted_too() {
        let dir = std::env::temp_dir().join(format!("agentty-claude-trust-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join(".claude.json");
        let project = Path::new("/work/app");
        let tree = Path::new("/data/worktrees/app-1234/claude-0919-2104");
        std::fs::write(
            &config,
            serde_json::json!({
                "numStartups": 3,
                "projects": { "/work/app": { "hasTrustDialogAccepted": true, "allowedTools": [] }, "/other": {} }
            })
            .to_string(),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        assert!(inherit_trust_in(&config, project, tree).unwrap());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&config).unwrap().permissions().mode() & 0o777, 0o600, "stays private");
        }
        let written: Value = serde_json::from_slice(&std::fs::read(&config).unwrap()).unwrap();
        assert_eq!(written["projects"][tree.to_string_lossy().as_ref()]["hasTrustDialogAccepted"], true);
        // Everything else is kept.
        assert_eq!(written["numStartups"], 3);
        assert_eq!(written["projects"]["/work/app"]["allowedTools"], serde_json::json!([]));
        // Already trusted: nothing to do.
        assert!(!inherit_trust_in(&config, project, tree).unwrap());

        // A project nobody trusted stays asked; a trusted folder above it counts.
        assert!(!inherit_trust_in(&config, Path::new("/other"), Path::new("/data/worktrees/other-1/x")).unwrap());
        std::fs::write(&config, serde_json::json!({ "projects": { "/work": { "hasTrustDialogAccepted": true } } }).to_string()).unwrap();
        assert!(inherit_trust_in(&config, Path::new("/work/app"), Path::new("/data/t")).unwrap());
        // No config at all: nothing is created.
        assert!(!inherit_trust_in(&dir.join("missing.json"), project, tree).unwrap());
        assert!(!dir.join("missing.json").exists());
        std::fs::remove_dir_all(dir).ok();
    }
}
