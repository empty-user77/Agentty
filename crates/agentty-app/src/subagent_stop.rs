//! Stopping one subagent of a Claude Code session from the app, the rest of the session going on.
//!
//! Claude Code has no outside handle on a running subagent, so the stop goes through its next tool
//! call: the app lists the subagent in `<data>/subagent-stop/<pane id>`, and `agentty subagent-guard`,
//! a `PreToolUse` hook, turns down every tool call of a listed subagent and ends it. The hook runs
//! only while that pane has such a list (the shell checks for the file first), so a session nobody
//! stops pays nothing for it. A command the subagent is already running finishes first.

use std::io::Read;
use std::path::{Path, PathBuf};

/// The folder of the lists, one file per pane.
pub fn dir() -> PathBuf {
    agentty_bridge::fsutil::data_dir().join("subagent-stop")
}

fn list_path(dir: &Path, pane_id: &str) -> Option<PathBuf> {
    // The pane id comes from the hook's environment: digits only, or it names no file.
    (!pane_id.is_empty() && pane_id.chars().all(|c| c.is_ascii_digit())).then(|| dir.join(pane_id))
}

fn read_list(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path).map(|text| text.lines().map(str::to_string).filter(|l| !l.is_empty()).collect()).unwrap_or_default()
}

fn write_list(path: &Path, ids: &[String]) {
    if ids.is_empty() {
        let _ = std::fs::remove_file(path);
        return;
    }
    if let Some(parent) = path.parent() {
        let _ = agentty_bridge::fsutil::create_private_dir(parent);
    }
    let _ = agentty_bridge::fsutil::write_private(path, format!("{}\n", ids.join("\n")).as_bytes());
}

/// Asks for `agent_id` of the session in `pane_id` to stop at its next tool call.
pub fn request(pane_id: u64, agent_id: &str) {
    let Some(path) = list_path(&dir(), &pane_id.to_string()) else { return };
    let mut ids = read_list(&path);
    if !ids.iter().any(|id| id == agent_id) {
        ids.push(agent_id.to_string());
    }
    write_list(&path, &ids);
}

/// `agent_id` has ended: nothing left to stop.
pub fn forget(pane_id: u64, agent_id: &str) {
    let Some(path) = list_path(&dir(), &pane_id.to_string()) else { return };
    let mut ids = read_list(&path);
    ids.retain(|id| id != agent_id);
    write_list(&path, &ids);
}

/// The session in `pane_id` has ended.
pub fn clear(pane_id: u64) {
    if let Some(path) = list_path(&dir(), &pane_id.to_string()) {
        let _ = std::fs::remove_file(path);
    }
}

/// What the hook answers for a tool call of `agent_id` (none from the main agent), if it is to stop.
fn verdict(listed: &[String], agent_id: Option<&str>) -> Option<serde_json::Value> {
    let agent_id = agent_id?;
    listed.iter().any(|id| id == agent_id).then(|| {
        serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": "The user stopped this subagent from Agentty. Do not call any more tools: \
                    end now with one line saying you were stopped and what you had finished.",
            },
        })
    })
}

/// `agentty subagent-guard <dir>`: the hook. Reads the call from stdin, the pane from the
/// environment, and answers on stdout.
pub fn run(dir: &Path) -> i32 {
    let Some(path) = std::env::var("AGENTTY_PANE_ID").ok().and_then(|pane| list_path(dir, &pane)) else { return 0 };
    let listed = read_list(&path);
    if listed.is_empty() {
        return 0;
    }
    let mut input = String::new();
    if std::io::stdin().take(4 * 1024 * 1024).read_to_string(&mut input).is_err() {
        return 0;
    }
    let Ok(event) = serde_json::from_str::<serde_json::Value>(&input) else { return 0 };
    if let Some(answer) = verdict(&listed, event["agent_id"].as_str()) {
        println!("{answer}");
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_listed_subagent_is_turned_down() {
        let listed = vec!["a1".to_string()];
        assert!(verdict(&listed, Some("a1")).is_some());
        assert!(verdict(&listed, Some("b2")).is_none());
        // The main agent's own calls carry no agent id.
        assert!(verdict(&listed, None).is_none());
        let answer = verdict(&listed, Some("a1")).unwrap();
        assert_eq!(answer["hookSpecificOutput"]["permissionDecision"], "deny");
    }

    #[test]
    fn a_pane_id_names_a_file_in_the_folder_only() {
        let dir = Path::new("/x");
        assert_eq!(list_path(dir, "42"), Some(PathBuf::from("/x/42")));
        assert_eq!(list_path(dir, "../42"), None);
        assert_eq!(list_path(dir, ""), None);
    }
}
