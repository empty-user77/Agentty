//! `agentty statusline`: Claude Code statusline wrapper for Agentty panes.
//!
//! Claude Code hands statusline commands a JSON snapshot on stdin, including the account's
//! rate-limit usage. Agentty registers this wrapper (via `--settings`) to report that usage to the
//! pane header, then runs the user's own statusline command with the same input, so their
//! statusline keeps working unchanged.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Five-hour window usage (0–100) from the statusline input.
pub fn usage_percent(input: &serde_json::Value) -> Option<f64> {
    let limits = &input["rate_limits"];
    limits["five_hour"]["used_percentage"]
        .as_f64()
        .or_else(|| limits["five_hour"]["used_percentage"].as_str()?.trim_end_matches('%').parse().ok())
}

fn statusline_command(settings: &Path) -> Option<String> {
    let text = std::fs::read_to_string(settings).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    json["statusLine"]["command"].as_str().map(str::to_string).filter(|c| !c.trim().is_empty())
}

/// The user's statusline, following Claude Code's precedence: project local > project > user.
pub fn user_command(cwd: Option<&Path>, config_dir: &Path) -> Option<String> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(cwd) = cwd {
        candidates.push(cwd.join(".claude/settings.local.json"));
        candidates.push(cwd.join(".claude/settings.json"));
    }
    candidates.push(config_dir.join("settings.json"));
    candidates.iter().find_map(|p| statusline_command(p))
}

pub fn run() -> i32 {
    let mut input = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut input);
    let json: serde_json::Value = serde_json::from_slice(&input).unwrap_or_default();

    if let (Some(percent), Ok(socket), Ok(pane)) = (usage_percent(&json), std::env::var("AGENTTY_SOCKET"), std::env::var("AGENTTY_PANE_ID"))
    {
        if let Ok(mut stream) = std::os::unix::net::UnixStream::connect(socket) {
            let _ = writeln!(stream, "{pane}\tusage\t{percent}");
        }
    }

    // Remember plan limits and reset times for the menu bar.
    let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    if let Some(limits) = agentty_bridge::limits::parse_claude(&json["rate_limits"], now_ms) {
        agentty_bridge::limits::save_claude(&limits);
    }

    let cwd = json["workspace"]["current_dir"].as_str().or(json["cwd"].as_str()).map(PathBuf::from);
    let config_dir = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| crate::launch::home_dir().join(".claude"));
    let Some(command) = user_command(cwd.as_deref(), &config_dir) else { return 0 };
    let Ok(mut child) = Command::new("sh").args(["-c", &command]).stdin(Stdio::piped()).spawn() else { return 0 };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(&input);
    }
    child.wait().ok().and_then(|s| s.code()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_five_hour_usage() {
        let input =
            serde_json::json!({ "rate_limits": { "five_hour": { "used_percentage": 36.5 }, "seven_day": { "used_percentage": 50 } } });
        assert_eq!(usage_percent(&input), Some(36.5));
        assert_eq!(usage_percent(&serde_json::json!({})), None);
    }

    #[test]
    fn project_settings_win_over_user() {
        let root = std::env::temp_dir().join(format!("agentty-statusline-{}", std::process::id()));
        let (project, config) = (root.join("project"), root.join("config"));
        std::fs::create_dir_all(project.join(".claude")).unwrap();
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(config.join("settings.json"), r#"{"statusLine":{"type":"command","command":"user-line"}}"#).unwrap();
        assert_eq!(user_command(Some(&project), &config).as_deref(), Some("user-line"));
        std::fs::write(project.join(".claude/settings.json"), r#"{"statusLine":{"type":"command","command":"project-line"}}"#).unwrap();
        assert_eq!(user_command(Some(&project), &config).as_deref(), Some("project-line"));
        assert_eq!(user_command(None, &root.join("missing")), None);
        std::fs::remove_dir_all(root).ok();
    }
}
