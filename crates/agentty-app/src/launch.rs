//! What a pane runs. Every launch goes through the user's login shell so PATH (nvm, brew, …)
//! matches their normal terminal, and command panes drop back to a shell when the command exits.
//!
//! Agent panes are started with lightweight hooks that report activity (working / finished /
//! needs input) to Agentty's local socket — Claude Code via `--settings` hooks, Codex via its
//! `notify` program. The user's own configuration files are never modified.
//!
//! On Windows the pane runs PowerShell (`pwsh`, else Windows PowerShell): agent and command panes
//! get their script as `-EncodedCommand` (no quoting pitfalls) and `-NoExit` keeps the shell
//! afterwards, like the Unix fallback shell.

use crate::settings::AdvisorChoice;
use agentty_bridge::agent_auth::{AuthAgent, EnvChange};
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

    /// The user's interactive shell: `$SHELL` (zsh / bash fallback), or on Windows
    /// `$AGENTTY_SHELL`, else PowerShell.
    #[cfg(unix)]
    pub fn shell_program() -> String {
        let fallback = if cfg!(target_os = "macos") { "/bin/zsh" } else { "/bin/bash" };
        std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| fallback.into())
    }

    #[cfg(windows)]
    pub fn shell_program() -> String {
        // `$SHELL` is ignored here: Git Bash exports a POSIX path (`/usr/bin/bash`) that isn't runnable.
        std::env::var("AGENTTY_SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(windows::powershell_program)
    }

    /// Credentials for this pane's agent (Settings → Accounts), read when the pane starts.
    pub fn auth_setup(&self) -> AuthSetup {
        let agent = match self.kind {
            PaneKind::Shell => return AuthSetup::default(),
            PaneKind::Claude => AuthAgent::Claude,
            PaneKind::Codex => AuthAgent::Codex,
        };
        match agentty_bridge::agent_auth::environment(agent) {
            Ok(changes) => AuthSetup { changes, warning: None },
            Err(err) => AuthSetup { changes: Vec::new(), warning: Some(format!("{err:#}")) },
        }
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
                args.extend(["--settings".into(), inline_or_file("claude-settings.json", claude_hook_settings())]);
                if crate::settings::browser_tools_enabled() {
                    // The in-app browser as MCP tools (added to the user's own servers).
                    args.extend(["--mcp-config".into(), inline_or_file("browser-mcp.json", browser_mcp_config())]);
                }
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
            args.push(prompt.clone());
        }
        Some(args)
    }

    /// Program and arguments handed to the PTY (without agent credentials).
    #[cfg(test)]
    pub fn argv(&self) -> (String, Vec<String>) {
        self.argv_with_auth(&AuthSetup::default())
    }

    /// Program and arguments handed to the PTY. Credentials in `auth` are expected in the pane's
    /// environment as `AGENTTY_AUTH_<NAME>` ([`AuthSetup::pane_variables`]); the command line only
    /// refers to them, so no secret ever appears in a process list.
    #[cfg(unix)]
    pub fn argv_with_auth(&self, auth: &AuthSetup) -> (String, Vec<String>) {
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
                let script = format!("{}{line}; {}", auth.shell_prefix(), auth.clear_before(&fallback_shell(&shell)));
                (shell, vec!["-l".into(), "-i".into(), "-c".into(), script])
            }
        }
    }

    #[cfg(windows)]
    pub fn argv_with_auth(&self, auth: &AuthSetup) -> (String, Vec<String>) {
        windows::argv(self, auth)
    }
}

/// Environment changes for an agent pane from `agentty_bridge::agent_auth`.
#[derive(Debug, Clone, Default)]
pub struct AuthSetup {
    pub changes: Vec<EnvChange>,
    /// Why the saved sign-in method couldn't be used (the agent then falls back to its own login).
    pub warning: Option<String>,
}

/// Prefix of the pane variables that carry credentials to the agent command.
const AUTH_PREFIX: &str = "AGENTTY_AUTH_";

impl AuthSetup {
    /// `AGENTTY_AUTH_<NAME>` → value, for the PTY environment.
    pub fn pane_variables(&self) -> Vec<(String, String)> {
        self.changes
            .iter()
            .filter_map(|change| match change {
                EnvChange::Set(name, value) => Some((format!("{AUTH_PREFIX}{name}"), value.clone())),
                EnvChange::Unset(_) => None,
            })
            .collect()
    }

    fn sets(&self) -> impl Iterator<Item = &str> {
        self.changes.iter().filter(|c| matches!(c, EnvChange::Set(..))).map(EnvChange::name)
    }

    fn unsets(&self) -> impl Iterator<Item = &str> {
        self.changes.iter().filter(|c| matches!(c, EnvChange::Unset(_))).map(EnvChange::name)
    }

    /// POSIX prefix for the agent command:
    /// `NAME="$AGENTTY_AUTH_NAME" … env -u AGENTTY_AUTH_NAME … -u OTHER … `.
    /// Assignments put the values in the agent's environment only; `env -u` then drops the
    /// `AGENTTY_AUTH_` copies (so the agent and its tools see each secret once, under its real
    /// name) and variables a shell profile may export for another method. Only names ever appear
    /// on the command line. Empty without credentials.
    #[cfg_attr(windows, allow(dead_code))]
    fn shell_prefix(&self) -> String {
        let mut out = String::new();
        if let Some(warning) = &self.warning {
            out.push_str(&format!("printf '%s\\n' {}; ", shell_quote(&format!("Agentty: {warning} — using the CLI login instead."))));
        }
        for name in self.sets() {
            out.push_str(&format!("{name}=\"${AUTH_PREFIX}{name}\" "));
        }
        let unsets: Vec<String> =
            self.sets().map(|name| format!("-u {AUTH_PREFIX}{name}")).chain(self.unsets().map(|name| format!("-u {name}"))).collect();
        if !unsets.is_empty() {
            out.push_str(&format!("env {} ", unsets.join(" ")));
        }
        out
    }

    /// `exec …` of the fallback shell, with the credential variables removed first.
    #[cfg_attr(windows, allow(dead_code))]
    fn clear_before(&self, exec_line: &str) -> String {
        let clear: Vec<String> = self.sets().map(|name| format!("-u {AUTH_PREFIX}{name}")).collect();
        match exec_line.strip_prefix("exec ") {
            Some(rest) if !clear.is_empty() => match rest.strip_prefix("env ") {
                Some(assignments) => format!("exec env {} {assignments}", clear.join(" ")),
                None => format!("exec env {} {rest}", clear.join(" ")),
            },
            _ => exec_line.to_string(),
        }
    }
}

/// The interactive shell a pane falls back to when its command exits. The first shell's generated
/// `.zshenv` restores the user's `ZDOTDIR`, so the integration environment is passed again here;
/// otherwise reserved words would be missing after an agent exits.
#[cfg(unix)]
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
#[cfg(target_os = "macos")]
fn signal_command(kind: &str, payload: &str) -> String {
    format!(
        "[ -n \"$AGENTTY_SOCKET\" ] && {{ printf '%s\\t{kind}\\t' \"$AGENTTY_PANE_ID\"; {payload}; printf '\\n'; }} | nc -U -w 1 \"$AGENTTY_SOCKET\" >/dev/null 2>&1; exit 0"
    )
}

/// Claude Code hook command for one signal. macOS pipes through `nc -U` (as always); elsewhere
/// `nc -U` is often missing (and Windows has no Unix sockets), so `agentty signal` forwards it.
fn hook_command(kind: &str) -> String {
    #[cfg(target_os = "macos")]
    {
        // Hook input arrives as JSON on stdin; newlines are stripped so one signal is one line.
        signal_command(kind, "tr -d '\\n'")
    }
    #[cfg(not(target_os = "macos"))]
    {
        format!("{} signal {kind}", shell_quote(&hook_exe()))
    }
}

/// Agentty's executable as hook commands (POSIX shells; Git Bash on Windows) can run it.
fn hook_exe() -> String {
    let exe = agentty_exe();
    if cfg!(windows) {
        exe.replace('\\', "/")
    } else {
        exe
    }
}

fn statusline_wrapper() -> String {
    format!("{} statusline", shell_quote(&hook_exe()))
}

fn claude_hook_settings() -> String {
    let hook = |kind: &str| serde_json::json!([{ "hooks": [{ "type": "command", "command": hook_command(kind) }] }]);
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

/// JSON for a Claude Code flag: inline, or on Windows a file path (both are accepted), which keeps
/// JSON quotes away from PowerShell's native-argument quoting. The JSON holds no secrets.
fn inline_or_file(name: &str, json: String) -> String {
    if !cfg!(windows) {
        return json;
    }
    let dir = agentty_bridge::fsutil::data_dir().join("launch");
    let path = dir.join(name);
    if std::fs::read_to_string(&path).ok().as_deref() != Some(json.as_str()) {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(&path, &json);
    }
    path.display().to_string()
}

fn agentty_exe() -> String {
    std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "agentty".into())
}

fn browser_mcp_config() -> String {
    serde_json::json!({ "mcpServers": { "agentty-browser": { "command": agentty_exe(), "args": ["mcp-browser"] } } }).to_string()
}

/// A TOML string for `codex -c`. Windows uses literal ('…') strings: Windows PowerShell 5.1 would
/// mangle the double quotes of basic strings on their way to a native program.
fn toml_string(s: &str) -> String {
    if cfg!(windows) && !s.contains(['\'', '\n']) {
        format!("'{s}'")
    } else {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

fn codex_browser_mcp_override() -> String {
    // Codex passes only listed environment variables to MCP servers.
    let mut env = vec!["AGENTTY_SOCKET", "AGENTTY_PANE_ID"];
    if cfg!(windows) {
        env.push(crate::ipc::TOKEN_VARIABLE);
    }
    let env: Vec<String> = env.into_iter().map(toml_string).collect();
    format!(
        "mcp_servers.agentty_browser={{command={},args=[{}],env_vars=[{}]}}",
        toml_string(&agentty_exe()),
        toml_string("mcp-browser"),
        env.join(",")
    )
}

#[cfg(target_os = "macos")]
fn codex_notify_override() -> String {
    // Codex runs `notify` with the event JSON as the last argument.
    let script = signal_command("stop", "printf '%s' \"$1\" | tr -d '\\n'");
    format!("notify=[{},{},{},{}]", toml_string("sh"), toml_string("-c"), toml_string(&script), toml_string("agentty"))
}

/// `agentty signal stop <event JSON>` (the JSON is appended by Codex).
#[cfg(not(target_os = "macos"))]
fn codex_notify_override() -> String {
    format!("notify=[{},{},{}]", toml_string(&agentty_exe()), toml_string("signal"), toml_string("stop"))
}

/// A command run through the user's login shell (so PATH matches their terminal). Arguments are
/// quoted individually; nothing is interpolated. On Windows, where PATH needs no shell, the
/// program is looked up on PATH (absolute directories only) and started directly without a
/// console window; a program that isn't found is an error rather than a bare name, which Windows
/// would also look for next to Agentty's own executable.
pub fn login_shell_command(argv: &[String]) -> anyhow::Result<std::process::Command> {
    #[cfg(unix)]
    {
        let line = argv.iter().map(|a| shell_quote(a)).collect::<Vec<_>>().join(" ");
        // Interactive as well as login: `.zshrc` is where many installs put themselves on PATH.
        let mut command = std::process::Command::new(LaunchSpec::shell_program());
        command.args(["-l", "-i", "-c", &line]);
        Ok(command)
    }
    #[cfg(windows)]
    {
        let program = argv.first().map(String::as_str).unwrap_or_default();
        let resolved = agentty_bridge::process::which(program).ok_or_else(|| anyhow::anyhow!("{program} was not found on PATH"))?;
        let mut command = agentty_bridge::process::command(resolved);
        command.args(argv.iter().skip(1));
        Ok(command)
    }
}

/// Runs a command like [`login_shell_command`] and returns stdout.
pub fn run_in_login_shell(argv: &[String]) -> anyhow::Result<String> {
    use std::process::Stdio;
    let output = login_shell_command(argv)?.stdin(Stdio::null()).output()?;
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

/// A PowerShell script as `-EncodedCommand` expects it (Base64 of UTF-16LE).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn encode_powershell(script: &str) -> String {
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    crate::browser_mcp::base64(&bytes)
}

/// A PowerShell single-quoted string (no interpolation; `'` is doubled).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn powershell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// Windows PowerShell 5.1 passes arguments to native programs without escaping embedded double
/// quotes; pre-escaping them (CRT rules) makes the program see the original text. PowerShell 7.3+
/// passes arguments correctly and gets them unchanged.
#[cfg_attr(not(windows), allow(dead_code))]
fn legacy_native_argument(arg: &str) -> String {
    if !arg.contains('"') && !(arg.contains(char::is_whitespace) && arg.ends_with('\\')) {
        return arg.to_string();
    }
    let mut out = String::new();
    let mut backslashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.push_str(&"\\".repeat(backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                out.push_str(&"\\".repeat(backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    // PowerShell wraps arguments with spaces in quotes; trailing backslashes must not escape the closing one.
    let trailing = if arg.contains(char::is_whitespace) { backslashes * 2 } else { backslashes };
    out.push_str(&"\\".repeat(trailing));
    out
}

/// PowerShell script of an agent pane: credentials, the agent, then cleanup (the pane's
/// PowerShell stays open afterwards through `-NoExit`).
#[cfg_attr(not(windows), allow(dead_code))]
fn powershell_agent_script(args: &[String], auth: &AuthSetup, legacy: bool, profile: Option<&str>) -> String {
    let mut lines: Vec<String> = vec!["$PSNativeCommandArgumentPassing = 'Standard'".into()];
    if let Some(warning) = &auth.warning {
        lines.push(format!(
            "Write-Host {} -ForegroundColor Yellow",
            powershell_quote(&format!("Agentty: {warning} — using the CLI login instead."))
        ));
    }
    for name in auth.unsets() {
        lines.push(format!("Remove-Item -LiteralPath Env:{name} -ErrorAction SilentlyContinue"));
    }
    for name in auth.sets() {
        lines.push(format!("$env:{name} = $env:{AUTH_PREFIX}{name}"));
        // The copy is dropped before the agent starts, so it sees each secret once.
        lines.push(format!("Remove-Item -LiteralPath Env:{AUTH_PREFIX}{name} -ErrorAction SilentlyContinue"));
    }
    // `env NAME=value program …` (the advisor switch) becomes plain environment assignments.
    let mut rest = args;
    let mut temporary: Vec<String> = Vec::new();
    if rest.first().map(String::as_str) == Some("env") {
        rest = &rest[1..];
        while let Some((name, value)) = rest.first().and_then(|a| a.split_once('=')) {
            lines.push(format!("$env:{name} = {}", powershell_quote(value)));
            temporary.push(name.to_string());
            rest = &rest[1..];
        }
    }
    if let Some((program, arguments)) = rest.split_first() {
        let arguments: Vec<String> =
            arguments.iter().map(|a| powershell_quote(&if legacy { legacy_native_argument(a) } else { a.clone() })).collect();
        // Applications only: npm's `.ps1` shims fail under the default execution policy.
        lines.push(format!(
            "$__agentty = Get-Command {} -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1",
            powershell_quote(program)
        ));
        lines.push(format!(
            "if ($__agentty) {{ & $__agentty.Source {} }} else {{ Write-Host {} -ForegroundColor Red }}",
            arguments.join(" "),
            powershell_quote(&format!("Agentty: '{program}' was not found on PATH."))
        ));
    }
    let mut cleanup: Vec<String> = auth.sets().map(|name| format!("Env:{name}")).collect();
    cleanup.extend(temporary.iter().map(|name| format!("Env:{name}")));
    if !cleanup.is_empty() {
        lines.push(format!("Remove-Item -LiteralPath {} -ErrorAction SilentlyContinue", cleanup.join(", ")));
    }
    lines.push("Remove-Variable __agentty -ErrorAction SilentlyContinue".into());
    if let Some(profile) = profile {
        lines.push(profile.to_string());
    }
    lines.join("\n")
}

#[cfg(windows)]
mod windows {
    use super::{encode_powershell, powershell_agent_script, AuthSetup, LaunchSpec, Start};

    /// `pwsh` (PowerShell 7) when installed, else the built-in Windows PowerShell.
    pub fn powershell_program() -> String {
        static PROGRAM: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        PROGRAM
            .get_or_init(|| {
                agentty_bridge::process::which("pwsh").map(|p| p.display().to_string()).unwrap_or_else(|| "powershell.exe".into())
            })
            .clone()
    }

    fn is_powershell(program: &str) -> bool {
        let name = std::path::Path::new(program).file_stem().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        name == "pwsh" || name == "powershell"
    }

    /// Windows PowerShell 5.1 (legacy native argument passing).
    fn is_legacy(program: &str) -> bool {
        std::path::Path::new(program).file_stem().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("powershell"))
    }

    /// The reserved-word functions, inlined into the pane's command when there are any. Not
    /// dot-sourced from `aliases.ps1`: under Windows PowerShell's default `Restricted` execution
    /// policy script files are refused (a red error in every pane), commands are not.
    fn profile_line() -> Option<String> {
        let script = std::fs::read_to_string(crate::shell_integration::integration_dir().join("aliases.ps1")).ok()?;
        script.contains("function ").then_some(script)
    }

    fn encoded(program: String, script: &str) -> (String, Vec<String>) {
        (program, vec!["-NoLogo".into(), "-NoExit".into(), "-EncodedCommand".into(), encode_powershell(script)])
    }

    pub fn argv(spec: &LaunchSpec, auth: &AuthSetup) -> (String, Vec<String>) {
        let shell = LaunchSpec::shell_program();
        let powershell = if is_powershell(&shell) { shell.clone() } else { powershell_program() };
        let profile = profile_line();
        if let Start::Command(line) = &spec.start {
            // User-defined command lines are PowerShell syntax on Windows.
            let script = match &profile {
                Some(profile) => format!("{profile}\n{line}"),
                None => line.clone(),
            };
            return encoded(powershell, &script);
        }
        match spec.command() {
            None if !is_powershell(&shell) => (shell, Vec::new()),
            None => match profile {
                Some(profile) => encoded(shell, &profile),
                None => (shell, vec!["-NoLogo".into()]),
            },
            Some(args) => {
                let script = powershell_agent_script(&args, auth, is_legacy(&powershell), profile.as_deref());
                encoded(powershell, &script)
            }
        }
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
pub(crate) mod tests {
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
        let settings: serde_json::Value = serde_json::from_str(&claude_hook_settings()).unwrap();
        let stop = settings["hooks"]["Stop"][0]["hooks"][0]["command"].as_str().unwrap();
        if cfg!(target_os = "macos") {
            assert_eq!(args[4], claude_hook_settings());
            assert!(stop.contains("AGENTTY_SOCKET"), "{stop}");
        } else {
            assert!(stop.ends_with(" signal stop"), "{stop}");
        }
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
        if cfg!(target_os = "macos") {
            assert!(args[2].starts_with("notify=[\"sh\""));
        } else {
            assert!(args[2].contains("signal") && args[2].contains("stop"), "{}", args[2]);
        }
        assert_eq!(args[args.len() - 2..], ["resume", "abc"]);

        let prompt = LaunchSpec::with_prompt(Agent::Claude, "go on".into(), "t".into(), PathBuf::from("/tmp"));
        assert_eq!(prompt.command().unwrap().last().unwrap(), "go on");
    }

    /// Runs the generated hook commands through `sh` against a real socket.
    #[test]
    #[cfg(target_os = "macos")]
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
    #[cfg(target_os = "macos")]
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
    #[cfg(unix)]
    fn command_panes_fall_back_to_shell() {
        let spec = LaunchSpec::new(PaneKind::Codex, PathBuf::from("/tmp"));
        let (_, args) = spec.argv();
        assert_eq!(args[..3], ["-l", "-i", "-c"]);
        assert!(args[3].starts_with("codex -c "));
        assert!(args[3].contains("; exec "));
    }

    #[test]
    #[cfg(unix)]
    fn fallback_shell_keeps_reserved_words() {
        let zsh = fallback_shell("/bin/zsh");
        assert!(zsh.starts_with("exec env "), "{zsh}");
        assert!(zsh.contains("ZDOTDIR="), "{zsh}");
        assert!(zsh.ends_with("/bin/zsh -l"), "{zsh}");
        assert!(fallback_shell("/bin/bash").contains("--rcfile"));
        assert_eq!(fallback_shell("/usr/local/bin/fish"), "exec /usr/local/bin/fish -l");
    }

    fn auth(changes: Vec<EnvChange>) -> AuthSetup {
        AuthSetup { changes, warning: None }
    }

    fn sample_auth() -> AuthSetup {
        auth(vec![
            EnvChange::Unset("ANTHROPIC_AUTH_TOKEN".into()),
            EnvChange::Unset("CLAUDE_CODE_OAUTH_TOKEN".into()),
            EnvChange::Set("ANTHROPIC_API_KEY".into(), format!("{}-{}", "example", "not_a_real_key")),
        ])
    }

    #[test]
    fn credentials_stay_off_the_command_line() {
        let setup = sample_auth();
        let variables = setup.pane_variables();
        assert_eq!(variables.len(), 1);
        assert_eq!(variables[0].0, "AGENTTY_AUTH_ANTHROPIC_API_KEY");
        let prefix = setup.shell_prefix();
        assert_eq!(
            prefix,
            "ANTHROPIC_API_KEY=\"$AGENTTY_AUTH_ANTHROPIC_API_KEY\" env -u AGENTTY_AUTH_ANTHROPIC_API_KEY -u ANTHROPIC_AUTH_TOKEN -u CLAUDE_CODE_OAUTH_TOKEN "
        );
        assert!(!prefix.contains("not_a_real_key"));
        assert_eq!(setup.clear_before("exec env A=1 /bin/zsh -l"), "exec env -u AGENTTY_AUTH_ANTHROPIC_API_KEY A=1 /bin/zsh -l");
        assert_eq!(setup.clear_before("exec /bin/fish -l"), "exec env -u AGENTTY_AUTH_ANTHROPIC_API_KEY /bin/fish -l");
        // Without credentials nothing changes.
        assert_eq!(AuthSetup::default().shell_prefix(), "");
        assert_eq!(AuthSetup::default().clear_before("exec /bin/zsh -l"), "exec /bin/zsh -l");
    }

    /// The credential reaches the agent's environment (once, under its real name) and not the
    /// shell around it; a value exported by the profile for another method is removed.
    #[test]
    #[cfg(unix)]
    fn shell_prefix_exports_credentials_to_the_command_only() {
        let setup = sample_auth();
        let script = format!(
            "{}sh -c 'printf \"%s|%s|%s\" \"$ANTHROPIC_API_KEY\" \"${{ANTHROPIC_AUTH_TOKEN-unset}}\" \"${{AGENTTY_AUTH_ANTHROPIC_API_KEY-dropped}}\"'; printf '|%s' \"${{ANTHROPIC_API_KEY-none}}\"",
            setup.shell_prefix()
        );
        let output = std::process::Command::new("sh")
            .args(["-c", &script])
            .env("AGENTTY_AUTH_ANTHROPIC_API_KEY", "example-value")
            .env("ANTHROPIC_AUTH_TOKEN", "from-profile")
            .env_remove("ANTHROPIC_API_KEY")
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout), "example-value|unset|dropped|none");
    }

    /// The whole agent pane script as the PTY runs it: the agent sees the key, the process list
    /// never does, and the shell that stays after the agent exits has neither copy.
    #[test]
    #[cfg(unix)]
    fn pane_script_isolates_credentials_end_to_end() {
        let dir = std::env::temp_dir().join(format!("agentty-auth-e2e-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let report = dir.join("report");
        // A stand-in agent that records what it received, and a stand-in fallback shell.
        let script = format!(
            "{}sh -c 'printf \"agent=%s ps=%s\\n\" \"$ANTHROPIC_API_KEY\" \"$(ps -o args= -p $$)\" > {}'; {}",
            sample_auth().shell_prefix(),
            report.display(),
            sample_auth().clear_before(&format!(
                "exec env sh -c 'printf \"fallback=%s|%s\\n\" \"${{AGENTTY_AUTH_ANTHROPIC_API_KEY-gone}}\" \"${{ANTHROPIC_API_KEY-gone}}\" >> {}'",
                report.display()
            ))
        );
        assert!(!script.contains("example-value"), "{script}");
        let status = std::process::Command::new("sh")
            .args(["-c", &script])
            .env("AGENTTY_AUTH_ANTHROPIC_API_KEY", "example-value")
            .env_remove("ANTHROPIC_API_KEY")
            .status()
            .unwrap();
        assert!(status.success());
        let text = std::fs::read_to_string(&report).unwrap();
        assert!(text.contains("agent=example-value "), "{text}");
        assert!(!text.lines().next().unwrap().split("ps=").nth(1).unwrap().contains("example-value"), "{text}");
        assert!(text.contains("fallback=gone|gone"), "{text}");
        std::fs::remove_dir_all(dir).ok();
    }

    /// A secret with shell metacharacters never reaches the script: only the variable name does.
    #[test]
    #[cfg(unix)]
    fn agent_argv_refers_to_credentials_by_name() {
        let hostile = format!("x'; rm -rf ~; echo \"$(id)\" {}", "example");
        let setup = auth(vec![EnvChange::Unset("OPENAI_BASE_URL".into()), EnvChange::Set("OPENAI_API_KEY".into(), hostile.clone())]);
        let spec = LaunchSpec::new(PaneKind::Codex, PathBuf::from("/tmp"));
        let (_, args) = spec.argv_with_auth(&setup);
        let script = &args[3];
        assert!(!script.contains(&hostile) && !script.contains("rm -rf"), "{script}");
        assert!(
            script.starts_with(
                "OPENAI_API_KEY=\"$AGENTTY_AUTH_OPENAI_API_KEY\" env -u AGENTTY_AUTH_OPENAI_API_KEY -u OPENAI_BASE_URL codex "
            ),
            "{script}"
        );
        assert!(script.contains("; exec env -u AGENTTY_AUTH_OPENAI_API_KEY "), "{script}");
        assert_eq!(setup.pane_variables(), vec![("AGENTTY_AUTH_OPENAI_API_KEY".to_string(), hostile)]);
        // Shell panes and command panes never get credentials.
        assert!(LaunchSpec::new(PaneKind::Shell, PathBuf::from("/tmp")).auth_setup().changes.is_empty());
    }

    #[test]
    fn powershell_script_for_agents() {
        let args: Vec<String> =
            ["env", "CLAUDE_CODE_DISABLE_ADVISOR_TOOL=1", "claude", "--resume", "it's"].iter().map(|s| s.to_string()).collect();
        let script = powershell_agent_script(&args, &sample_auth(), false, Some(". 'C:\\a\\aliases.ps1'"));
        assert!(script.contains("Remove-Item -LiteralPath Env:ANTHROPIC_AUTH_TOKEN -ErrorAction SilentlyContinue"));
        assert!(script.contains("$env:ANTHROPIC_API_KEY = $env:AGENTTY_AUTH_ANTHROPIC_API_KEY"));
        assert!(script.contains("$env:CLAUDE_CODE_DISABLE_ADVISOR_TOOL = '1'"));
        assert!(script.contains("Get-Command 'claude' -CommandType Application"));
        assert!(script.contains("& $__agentty.Source '--resume' 'it''s'"));
        assert!(script.contains("Remove-Item -LiteralPath Env:AGENTTY_AUTH_ANTHROPIC_API_KEY"));
        assert!(script.contains("Env:ANTHROPIC_API_KEY, Env:CLAUDE_CODE_DISABLE_ADVISOR_TOOL"));
        // The copy is removed before the agent starts.
        let copy = script.find("Remove-Item -LiteralPath Env:AGENTTY_AUTH_").unwrap();
        assert!(copy < script.find("& $__agentty.Source").unwrap());
        assert!(script.ends_with(". 'C:\\a\\aliases.ps1'"));
        assert!(!script.contains("not_a_real_key"));
    }

    /// PowerShell to run generated scripts with: `$AGENTTY_TEST_PWSH` or `pwsh` on PATH. Tests
    /// that need it are skipped (and say so) when there is none.
    #[cfg(unix)]
    pub(crate) fn test_pwsh() -> Option<std::path::PathBuf> {
        std::env::var_os("AGENTTY_TEST_PWSH").map(Into::into).or_else(|| agentty_bridge::process::which("pwsh")).or_else(|| {
            eprintln!("pwsh not found: skipping PowerShell execution test");
            None
        })
    }

    /// A stand-in program printing its arguments as `[a][b]` and the credential variables.
    #[cfg(unix)]
    pub(crate) fn fake_program(dir: &std::path::Path, name: &str) {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(
            &path,
            "#!/bin/sh\nfor a in \"$@\"; do printf '[%s]' \"$a\"; done\nprintf ' key=%s copy=%s other=%s advisor=%s\\n' \"$ANTHROPIC_API_KEY\" \"${AGENTTY_AUTH_ANTHROPIC_API_KEY-none}\" \"${ANTHROPIC_AUTH_TOKEN-none}\" \"${CLAUDE_CODE_DISABLE_ADVISOR_TOOL-none}\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// Runs the Windows agent script in real PowerShell: arguments arrive intact (quotes,
    /// spaces), the agent sees the key once and not the profile's other method, and nothing is
    /// left in the shell afterwards.
    #[test]
    #[cfg(unix)]
    fn powershell_agent_script_runs() {
        let Some(pwsh) = test_pwsh() else { return };
        let dir = std::env::temp_dir().join(format!("agentty-pwsh-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        fake_program(&dir, "agenttyfake");
        let args: Vec<String> = ["env", "CLAUDE_CODE_DISABLE_ADVISOR_TOOL=1", "agenttyfake", "--x", "it's \"q\"", "a b", "$HOME"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut script = powershell_agent_script(&args, &sample_auth(), false, None);
        script.push_str(
            "\nWrite-Output \"after=$($env:ANTHROPIC_API_KEY)|$($env:AGENTTY_AUTH_ANTHROPIC_API_KEY)|$($env:CLAUDE_CODE_DISABLE_ADVISOR_TOOL)\"",
        );
        let missing = powershell_agent_script(&["agentty-no-such-agent".to_string()], &AuthSetup::default(), false, None);
        let path = format!("{}:{}", dir.display(), std::env::var("PATH").unwrap_or_default());
        let run = |script: &str| {
            let output = std::process::Command::new(&pwsh)
                .args(["-NoProfile", "-NonInteractive", "-EncodedCommand", &encode_powershell(script)])
                .env("PATH", &path)
                .env("AGENTTY_AUTH_ANTHROPIC_API_KEY", "example-value")
                .env("ANTHROPIC_AUTH_TOKEN", "from-profile")
                .env_remove("ANTHROPIC_API_KEY")
                .output()
                .unwrap();
            String::from_utf8_lossy(&output.stdout).to_string() + &String::from_utf8_lossy(&output.stderr)
        };
        let out = run(&script);
        assert!(out.contains("[--x][it's \"q\"][a b][$HOME] key=example-value copy=none other=none advisor=1"), "{out}");
        assert!(out.contains("after=||"), "{out}");
        assert!(run(&missing).contains("'agentty-no-such-agent' was not found on PATH"));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn legacy_powershell_arguments_are_pre_escaped() {
        assert_eq!(legacy_native_argument("plain"), "plain");
        assert_eq!(legacy_native_argument(r#"say "hi""#), r#"say \"hi\""#);
        assert_eq!(legacy_native_argument(r#"a\"b"#), r#"a\\\"b"#);
        assert_eq!(legacy_native_argument(r"C:\dir with space\"), r"C:\dir with space\\");
    }

    #[test]
    fn encodes_powershell_commands() {
        // "ls" in UTF-16LE is 6C 00 73 00.
        assert_eq!(encode_powershell("ls"), "bABzAA==");
    }
}
