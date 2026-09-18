//! What a pane runs. Every launch goes through the user's login shell so PATH (nvm, brew, …)
//! matches their normal terminal, and command panes drop back to a shell when the command exits.
//!
//! Agent panes are started with lightweight hooks that report activity (working / finished /
//! needs input) to Agentty's local socket — Claude Code via `--settings` hooks, Codex via its
//! `notify` program. The user's own configuration files are never modified.

use crate::settings::AdvisorChoice;
use agentty_bridge::model::Agent;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PaneKind {
    Shell,
    Claude,
    Codex,
}

impl PaneKind {
    pub fn agent(self) -> Option<Agent> {
        match self {
            PaneKind::Shell => None,
            PaneKind::Claude => Some(Agent::Claude),
            PaneKind::Codex => Some(Agent::Codex),
        }
    }
}

impl From<Agent> for PaneKind {
    fn from(agent: Agent) -> Self {
        match agent {
            Agent::Claude => PaneKind::Claude,
            Agent::Codex => PaneKind::Codex,
            // Other CLIs run as commands in shell panes.
            _ => PaneKind::Shell,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Start {
    /// Plain shell, or a fresh agent session.
    New,
    /// Resume an existing agent session by id.
    Resume(String),
    /// Fresh agent session seeded with an initial prompt (handoffs).
    Prompt(String),
    /// A shell pane that runs a command line first (custom commands); the shell stays afterwards.
    Command(String),
}

#[derive(Debug, Clone)]
pub struct LaunchSpec {
    pub kind: PaneKind,
    pub title: String,
    pub cwd: PathBuf,
    pub start: Start,
    /// Claude Code session id, fixed up front so the pane can be resumed and shared later.
    pub session_id: Option<String>,
    /// Model override (`claude --model`, `codex -m`).
    pub model: Option<String>,
    /// Claude Code advisor; `None` takes the setting for new tabs.
    pub advisor: Option<AdvisorChoice>,
}

/// What the user picked in the launcher.
#[derive(Debug, Clone, PartialEq)]
pub enum LaunchChoice {
    Kind(PaneKind),
    /// Claude Code or Codex with a specific model.
    Model(PaneKind, String),
    /// Another agent CLI or a local model, run as a command in a shell pane.
    Command {
        title: String,
        command: String,
    },
}

impl From<PaneKind> for LaunchChoice {
    fn from(kind: PaneKind) -> Self {
        LaunchChoice::Kind(kind)
    }
}

impl LaunchChoice {
    pub fn kind(&self) -> PaneKind {
        match self {
            LaunchChoice::Kind(kind) | LaunchChoice::Model(kind, _) => *kind,
            LaunchChoice::Command { .. } => PaneKind::Shell,
        }
    }

    /// Name shown in the folder picker for choices that aren't a plain pane kind.
    pub fn label(&self) -> Option<String> {
        match self {
            LaunchChoice::Kind(_) => None,
            LaunchChoice::Model(kind, model) => Some(format!("{} · {model}", LaunchSpec::new(*kind, PathBuf::new()).title)),
            LaunchChoice::Command { title, .. } => Some(title.clone()),
        }
    }

    pub fn spec(&self, cwd: PathBuf) -> LaunchSpec {
        match self {
            LaunchChoice::Kind(kind) => LaunchSpec::new(*kind, cwd),
            LaunchChoice::Model(kind, model) => {
                let mut spec = LaunchSpec::new(*kind, cwd);
                spec.model = Some(model.clone());
                spec
            }
            LaunchChoice::Command { title, command } => LaunchSpec::shell_command(command.clone(), title.clone(), cwd),
        }
    }
}

static NEXT_PANE_ID: AtomicU64 = AtomicU64::new(1);

pub fn next_pane_id() -> u64 {
    NEXT_PANE_ID.fetch_add(1, Ordering::Relaxed)
}

impl LaunchSpec {
    pub fn new(kind: PaneKind, cwd: PathBuf) -> Self {
        let title = match kind {
            PaneKind::Shell => "Terminal".to_string(),
            PaneKind::Claude => "Claude Code".to_string(),
            PaneKind::Codex => "Codex".to_string(),
        };
        let session_id = (kind == PaneKind::Claude).then(|| uuid::Uuid::new_v4().to_string());
        Self { kind, title, cwd, start: Start::New, session_id, model: None, advisor: None }
    }

    pub fn shell_command(command: String, title: String, cwd: PathBuf) -> Self {
        Self { kind: PaneKind::Shell, title, cwd, start: Start::Command(command), session_id: None, model: None, advisor: None }
    }

    pub fn resume(agent: Agent, id: String, title: String, cwd: PathBuf) -> Self {
        if !agent.is_first_class() {
            let line = agent.resume_args(&id).iter().map(|a| shell_quote(a)).collect::<Vec<_>>().join(" ");
            return Self::shell_command(line, title, cwd);
        }
        Self { kind: agent.into(), title, cwd, session_id: Some(id.clone()), start: Start::Resume(id), model: None, advisor: None }
    }

    pub fn with_prompt(agent: Agent, prompt: String, title: String, cwd: PathBuf) -> Self {
        let mut spec = Self::new(agent.into(), cwd);
        spec.title = title;
        spec.start = Start::Prompt(prompt);
        spec
    }

    pub fn shell_program() -> String {
        std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/zsh".into())
    }

    /// The agent command line, including activity hooks.
    pub fn command(&self) -> Option<Vec<String>> {
        let mut args: Vec<String> = Vec::new();
        match self.kind {
            PaneKind::Shell => return None,
            PaneKind::Claude => {
                let advisor = self.advisor.unwrap_or_else(crate::settings::advisor_default);
                if advisor == AdvisorChoice::Off {
                    // `--advisor` has no "off"; this also overrides an `advisorModel` setting.
                    args.extend(["env".into(), "CLAUDE_CODE_DISABLE_ADVISOR_TOOL=1".into()]);
                }
                args.push("claude".into());
                match &self.start {
                    Start::Resume(id) => args.extend(["--resume".into(), id.clone()]),
                    _ => {
                        if let Some(id) = &self.session_id {
                            args.extend(["--session-id".into(), id.clone()]);
                        }
                    }
                }
                if let Some(model) = &self.model {
                    args.extend(["--model".into(), model.clone()]);
                }
                if let Some(model) = advisor.model() {
                    args.extend(["--advisor".into(), model.into()]);
                }
                if crate::agents::claude_auto_mode() && agentty_bridge::idea::is_idea_project(&self.cwd) {
                    // "Build my idea" projects belong to people who cannot judge a permission prompt:
                    // Claude Code's auto mode decides instead. Only the command line can turn it on —
                    // `"defaultMode": "auto"` in the project's own settings is ignored.
                    args.extend(["--permission-mode".into(), "auto".into()]);
                }
                if crate::settings::browser_tools_enabled() {
                    // The in-app browser as MCP tools (added to the user's own servers). `--mcp-config`
                    // takes any number of values, so it must not be the last option: a prompt right
                    // after it is read as another config file ("Invalid MCP configuration").
                    args.extend(["--mcp-config".into(), browser_mcp_config()]);
                }
                args.extend(["--settings".into(), claude_hook_settings()]);
            }
            PaneKind::Codex => {
                args.extend(["codex".into(), "-c".into(), codex_notify_override()]);
                if crate::settings::browser_tools_enabled() {
                    args.extend(["-c".into(), codex_browser_mcp_override()]);
                }
                if let Some(model) = &self.model {
                    args.extend(["-m".into(), model.clone()]);
                }
                if let Start::Resume(id) = &self.start {
                    args.extend(["resume".into(), id.clone()]);
                }
            }
        }
        if let Start::Prompt(prompt) = &self.start {
            // A prompt that starts with a dash is an option to Claude Code ("unknown option") unless
            // options are ended first.
            if self.kind == PaneKind::Claude {
                args.push("--".into());
            }
            args.push(prompt.clone());
        }
        Some(args)
    }

    /// Program and arguments handed to the PTY.
    pub fn argv(&self) -> (String, Vec<String>) {
        let shell = Self::shell_program();
        if let Start::Command(line) = &self.start {
            // User-defined command lines are shell syntax by design; run them as written.
            let script = format!("{line}; {}", fallback_shell(&shell));
            return (shell, vec!["-l".into(), "-i".into(), "-c".into(), script]);
        }
        match self.command() {
            // bash reads --rcfile only when not a login shell; the rc file sources the profiles itself.
            None if matches!(crate::shell_integration::flavor(&shell), crate::shell_integration::ShellFlavor::Bash) => {
                let rc = crate::shell_integration::integration_dir().join("rc.bash");
                (shell, vec!["--rcfile".into(), rc.display().to_string(), "-i".into()])
            }
            None => (shell, vec!["-l".into()]),
            Some(args) => {
                let line = args.iter().map(|a| shell_quote(a)).collect::<Vec<_>>().join(" ");
                let script = format!("{line}; {}", fallback_shell(&shell));
                (shell, vec!["-l".into(), "-i".into(), "-c".into(), script])
            }
        }
    }
}

/// The interactive shell a pane falls back to when its command exits. The first shell's generated
/// `.zshenv` restores the user's `ZDOTDIR`, so the integration environment is passed again here;
/// otherwise reserved words would be missing after an agent exits.
fn fallback_shell(shell: &str) -> String {
    use crate::shell_integration::{flavor, integration_dir, ShellFlavor};
    let quoted = shell_quote(shell);
    match flavor(shell) {
        ShellFlavor::Zsh => {
            let env = crate::shell_integration::environment(shell)
                .iter()
                .map(|(key, value)| shell_quote(&format!("{key}={value}")))
                .collect::<Vec<_>>()
                .join(" ");
            format!("exec env {env} {quoted} -l")
        }
        ShellFlavor::Bash => {
            format!("exec {quoted} --rcfile {} -i", shell_quote(&integration_dir().join("rc.bash").display().to_string()))
        }
        ShellFlavor::Other => format!("exec {quoted} -l"),
    }
}

/// Shell snippet that forwards one agent signal (plus optional payload) to Agentty.
fn signal_command(kind: &str, payload: &str) -> String {
    format!(
        "[ -n \"$AGENTTY_SOCKET\" ] && {{ printf '%s\\t{kind}\\t' \"$AGENTTY_PANE_ID\"; {payload}; printf '\\n'; }} | nc -U -w 1 \"$AGENTTY_SOCKET\" >/dev/null 2>&1; exit 0"
    )
}

fn statusline_wrapper() -> String {
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "agentty".into());
    format!("{} statusline", shell_quote(&exe))
}

fn claude_hook_settings() -> String {
    // Hook input arrives as JSON on stdin; newlines are stripped so one signal is one line.
    let hook = |kind: &str| serde_json::json!([{ "hooks": [{ "type": "command", "command": signal_command(kind, "tr -d '\\n'") }] }]);
    serde_json::json!({
        "hooks": {
            "UserPromptSubmit": hook("working"),
            "PreToolUse": hook("working"),
            "PostToolUse": hook("working"),
            "PostToolUseFailure": hook("working"),
            "PermissionRequest": hook("permission"),
            "Stop": hook("stop"),
            "StopFailure": hook("stop"),
            "Notification": hook("notification"),
            "SubagentStart": hook("subagent_start"),
            "SubagentStop": hook("subagent_stop"),
            "SessionEnd": hook("session_end"),
        },
        // Reports usage to the pane header, then runs the user's own statusline unchanged.
        "statusLine": { "type": "command", "command": statusline_wrapper() },
    })
    .to_string()
}

fn agentty_exe() -> String {
    std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "agentty".into())
}

fn browser_mcp_config() -> String {
    serde_json::json!({ "mcpServers": { "agentty-browser": { "command": agentty_exe(), "args": ["mcp-browser"] } } }).to_string()
}

fn codex_browser_mcp_override() -> String {
    let toml_string = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
    // Codex passes only listed environment variables to MCP servers.
    format!(
        "mcp_servers.agentty_browser={{command={},args=[\"mcp-browser\"],env_vars=[\"AGENTTY_SOCKET\",\"AGENTTY_PANE_ID\"]}}",
        toml_string(&agentty_exe())
    )
}

fn codex_notify_override() -> String {
    // Codex runs `notify` with the event JSON as the last argument.
    let script = signal_command("stop", "printf '%s' \"$1\" | tr -d '\\n'");
    let toml_string = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
    format!("notify=[{},{},{},{}]", toml_string("sh"), toml_string("-c"), toml_string(&script), toml_string("agentty"))
}

/// Runs a command through the user's login shell (so PATH matches their terminal) and returns
/// stdout. Arguments are quoted individually; nothing is interpolated.
pub fn run_in_login_shell(argv: &[String]) -> anyhow::Result<String> {
    use std::process::{Command, Stdio};
    let line = argv.iter().map(|a| shell_quote(a)).collect::<Vec<_>>().join(" ");
    // Interactive as well as login: `.zshrc` is where many installs put themselves on PATH.
    let output = Command::new(LaunchSpec::shell_program()).args(["-l", "-i", "-c", &line]).stdin(Stdio::null()).output()?;
    let tail = |bytes: &[u8]| {
        let text = String::from_utf8_lossy(bytes).trim().to_string();
        let start = text.char_indices().rev().nth(600).map(|(i, _)| i).unwrap_or(0);
        text[start..].to_string()
    };
    if output.status.success() {
        Ok(tail(&output.stdout))
    } else {
        let stderr = tail(&output.stderr);
        anyhow::bail!(if stderr.is_empty() { tail(&output.stdout) } else { stderr })
    }
}

pub fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

pub fn shell_quote(arg: &str) -> String {
    let safe = !arg.is_empty() && arg.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:@%+,".contains(c));
    if safe {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_only_when_needed() {
        assert_eq!(shell_quote("claude"), "claude");
        assert_eq!(shell_quote("it's here"), r"'it'\''s here'");
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn claude_gets_session_id_and_hooks() {
        let mut spec = LaunchSpec::new(PaneKind::Claude, PathBuf::from("/tmp"));
        spec.advisor = Some(AdvisorChoice::Inherit);
        let args = spec.command().unwrap();
        assert_eq!(args[..2], ["claude", "--session-id"]);
        assert_eq!(args[2], spec.session_id.clone().unwrap());
        let at = args.iter().position(|a| a == "--settings").unwrap();
        let settings: serde_json::Value = serde_json::from_str(&args[at + 1]).unwrap();
        assert!(settings["hooks"]["Stop"][0]["hooks"][0]["command"].as_str().unwrap().contains("AGENTTY_SOCKET"));
    }

    #[test]
    fn claude_prompt_is_not_swallowed_by_mcp_config() {
        let spec =
            LaunchSpec::with_prompt(agentty_bridge::model::Agent::Claude, "Build my idea".into(), "Idea".into(), PathBuf::from("/tmp"));
        let args = spec.command().unwrap();
        assert_eq!(args.last().map(String::as_str), Some("Build my idea"));
        // `--mcp-config <configs...>` would take the prompt as a second config file, and a prompt
        // starting with a dash would be read as an option: `--settings <json> -- <prompt>`.
        assert_eq!(args[args.len() - 2], "--", "{args:?}");
        assert_eq!(args[args.len() - 4], "--settings", "{args:?}");
    }

    #[test]
    fn claude_advisor_arguments() {
        let mut spec = LaunchSpec::new(PaneKind::Claude, PathBuf::from("/tmp"));
        spec.advisor = Some(AdvisorChoice::Opus);
        let args = spec.command().unwrap();
        assert_eq!(args[0], "claude");
        assert!(args.windows(2).any(|w| w == ["--advisor", "opus"]));

        spec.advisor = Some(AdvisorChoice::Off);
        let args = spec.command().unwrap();
        assert_eq!(args[..3], ["env", "CLAUDE_CODE_DISABLE_ADVISOR_TOOL=1", "claude"]);
        assert!(!args.iter().any(|a| a == "--advisor"));

        spec.advisor = Some(AdvisorChoice::Inherit);
        let args = spec.command().unwrap();
        assert_eq!(args[0], "claude");
        assert!(!args.iter().any(|a| a == "--advisor"));
    }

    #[test]
    fn resume_and_prompt_arguments() {
        let resume = LaunchSpec::resume(Agent::Codex, "abc".into(), "t".into(), PathBuf::from("/tmp"));
        let args = resume.command().unwrap();
        assert_eq!(args[0], "codex");
        assert!(args[2].starts_with("notify=[\"sh\""));
        assert_eq!(args[args.len() - 2..], ["resume", "abc"]);

        let prompt = LaunchSpec::with_prompt(Agent::Claude, "go on".into(), "t".into(), PathBuf::from("/tmp"));
        assert_eq!(prompt.command().unwrap().last().unwrap(), "go on");
    }

    /// Runs the generated hook commands through `sh` against a real socket.
    #[test]
    fn hook_commands_reach_the_socket() {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixListener;
        use std::process::{Command, Stdio};

        let dir = std::env::temp_dir().join(format!("agentty-hook-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("s.sock");
        let listener = UnixListener::bind(&socket).unwrap();

        let settings: serde_json::Value = serde_json::from_str(&claude_hook_settings()).unwrap();
        let command = settings["hooks"]["Notification"][0]["hooks"][0]["command"].as_str().unwrap().to_string();
        let mut child = Command::new("sh")
            .args(["-c", &command])
            .env("AGENTTY_SOCKET", &socket)
            .env("AGENTTY_PANE_ID", "9")
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(b"{\n\"message\": \"waiting\"}\n").unwrap();
        let (stream, _) = listener.accept().unwrap();
        let line = BufReader::new(stream).lines().next().unwrap().unwrap();
        assert!(child.wait().unwrap().success());
        let signal = crate::agent_signal::parse_line(&line).unwrap();
        assert_eq!(signal.pane_id, 9);
        assert_eq!(signal.message.as_deref(), Some("waiting"));

        // Codex: `notify` receives the event JSON as its last argument.
        let override_value = codex_notify_override();
        let toml: toml_like::Array = toml_like::parse(&override_value);
        let mut child = Command::new(&toml[0])
            .args(&toml[1..])
            .arg("{\"type\":\"agent-turn-complete\",\"last-assistant-message\":\"done\"}")
            .env("AGENTTY_SOCKET", &socket)
            .env("AGENTTY_PANE_ID", "4")
            .spawn()
            .unwrap();
        let (stream, _) = listener.accept().unwrap();
        let line = BufReader::new(stream).lines().next().unwrap().unwrap();
        assert!(child.wait().unwrap().success());
        let signal = crate::agent_signal::parse_line(&line).unwrap();
        assert_eq!(signal.pane_id, 4);
        assert_eq!(signal.message.as_deref(), Some("done"));
        std::fs::remove_dir_all(dir).ok();
    }

    /// Minimal parser for the `notify=["..", ..]` override (TOML basic strings only).
    mod toml_like {
        pub type Array = Vec<String>;

        pub fn parse(value: &str) -> Array {
            let body = value.strip_prefix("notify=[").and_then(|v| v.strip_suffix(']')).unwrap();
            let mut items = Vec::new();
            let mut chars = body.chars().peekable();
            while let Some(c) = chars.next() {
                if c != '"' {
                    continue;
                }
                let mut item = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '\\' => item.push(chars.next().unwrap()),
                        '"' => break,
                        other => item.push(other),
                    }
                }
                items.push(item);
            }
            items
        }
    }

    #[test]
    fn command_panes_fall_back_to_shell() {
        let spec = LaunchSpec::new(PaneKind::Codex, PathBuf::from("/tmp"));
        let (_, args) = spec.argv();
        assert_eq!(args[..3], ["-l", "-i", "-c"]);
        assert!(args[3].starts_with("codex -c "));
        assert!(args[3].contains("; exec "));
    }

    #[test]
    fn fallback_shell_keeps_reserved_words() {
        let zsh = fallback_shell("/bin/zsh");
        assert!(zsh.starts_with("exec env "), "{zsh}");
        assert!(zsh.contains("ZDOTDIR="), "{zsh}");
        assert!(zsh.ends_with("/bin/zsh -l"), "{zsh}");
        assert!(fallback_shell("/bin/bash").contains("--rcfile"));
        assert_eq!(fallback_shell("/usr/local/bin/fish"), "exec /usr/local/bin/fish -l");
    }
}
