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

/// The context window of the model in use, as Claude Code itself states it. Nothing else says: the
/// transcript names only the model, and a model's name does not tell its window.
pub fn context_window(input: &serde_json::Value) -> Option<u64> {
    input["context_window"]["context_window_size"].as_u64().filter(|w| *w > 0)
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

    let (percent, window) = (usage_percent(&json), context_window(&json));
    if let (true, Ok(socket), Ok(pane)) =
        (percent.is_some() || window.is_some(), std::env::var("AGENTTY_SOCKET"), std::env::var("AGENTTY_PANE_ID"))
    {
        if let Ok(mut stream) = crate::ipc::connect(&socket) {
            if let Some(percent) = percent {
                let _ = writeln!(stream, "{pane}\tusage\t{percent}");
            }
            if let Some(window) = window {
                let _ = writeln!(stream, "{pane}\tcontext_window\t{window}");
            }
            crate::agent_signal::linger(stream);
        }
    }

    // Remember the window for sessions Claude Code has not told it for yet (see `context_windows`).
    if let (Some(window), Some(model)) = (window, json["model"]["id"].as_str()) {
        agentty_bridge::context_windows::remember(model, window);
    }

    // Remember plan limits and reset times for the menu bar.
    let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    if let Some(limits) = agentty_bridge::limits::parse_claude(&json["rate_limits"], now_ms) {
        agentty_bridge::limits::save_claude(&limits);
    }

    let cwd = json["workspace"]["current_dir"].as_str().or(json["cwd"].as_str()).map(PathBuf::from);
    let config_dir = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| crate::launch::home_dir().join(".claude"));
    let Some(command) = user_command(cwd.as_deref(), &config_dir) else { return 0 };
    let Ok(mut child) = chained(shell(), &command).spawn() else { return 0 };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(&input);
    }
    child.wait().ok().and_then(|s| s.code()).unwrap_or(0)
}

/// The user's command is POSIX shell syntax (Claude Code runs it with Git Bash on Windows).
fn shell() -> std::ffi::OsString {
    let shell = if cfg!(windows) { agentty_bridge::process::which("sh").or_else(|| agentty_bridge::process::which("bash")) } else { None };
    shell.map_or_else(|| "sh".into(), |p| p.into_os_string())
}

/// The user's status line command, given Claude Code's input on stdin. Without a console window:
/// Agentty is a GUI program, so Windows would give the shell a new console, which Windows Terminal
/// (as the default terminal) shows on every refresh.
fn chained(shell: std::ffi::OsString, command: &str) -> Command {
    let mut child = Command::new(shell);
    child.args(["-c", command]).stdin(Stdio::piped());
    agentty_bridge::process::hide_window(&mut child);
    child
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
    fn reads_the_context_window_claude_code_states() {
        let input = serde_json::json!({ "model": { "id": "claude-haiku-5-5" }, "context_window": { "context_window_size": 1_000_000, "used_percentage": 23 } });
        assert_eq!(context_window(&input), Some(1_000_000));
        assert_eq!(context_window(&serde_json::json!({ "context_window": { "context_window_size": 0 } })), None);
        assert_eq!(context_window(&serde_json::json!({})), None);
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

    /// The chained command reads Claude Code's input and its output reaches Claude Code.
    #[test]
    fn chained_command_gets_the_input_and_prints() {
        let mut child = chained(shell(), "read line; echo \"line: $line\"");
        let mut child = child.stdout(Stdio::piped()).spawn().expect("a shell");
        child.stdin.take().unwrap().write_all(b"{\"model\":\"opus\"}\n").unwrap();
        let output = child.wait_with_output().unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), r#"line: {"model":"opus"}"#);
    }

    /// On Windows itself: the shell runs without a console window of its own.
    #[cfg(windows)]
    #[test]
    fn chained_command_has_no_console_window() {
        let script = "Add-Type -Namespace Probe -Name Console -MemberDefinition '[DllImport(\"kernel32.dll\")] public static extern System.IntPtr GetConsoleWindow();'; Write-Output ('window=' + [Probe.Console]::GetConsoleWindow())";
        let mut child = chained("powershell.exe".into(), script);
        let output = child.stdin(Stdio::null()).output().expect("PowerShell");
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "window=0", "{}", String::from_utf8_lossy(&output.stderr));
    }
}
