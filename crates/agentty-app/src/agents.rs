//! Which AI coding agents and local models are installed, so the launcher only offers what runs.
//! Detection goes through the login shell once (PATH from nvm, brew, … matches the user's terminal).

use crate::launch::shell_quote;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};

/// A command-line agent Agentty can launch in a terminal pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentCli {
    /// Shown directly in the launcher (well-known agents); the rest sit under "Other models".
    pub primary: bool,
    pub id: &'static str,
    pub name: &'static str,
    pub binary: &'static str,
    pub color: u32,
    /// npm/pip package path fragment, to recognize the CLI when it runs under node or python ("" if none).
    pub package: &'static str,
    /// Who makes it, for the start page ("Google's coding agent"); "" says nothing.
    pub maker: &'static str,
}

/// Agents besides Claude Code and Codex (those have first-class panes with hooks and sessions).
pub const OTHER_AGENTS: &[AgentCli] = &[
    AgentCli {
        primary: true,
        id: "gemini",
        name: "Gemini CLI",
        binary: "gemini",
        color: 0x4796e3,
        package: "@google/gemini-cli",
        maker: "Google",
    },
    AgentCli { primary: true, id: "agy", name: "Antigravity CLI", binary: "agy", color: 0x3c82f6, package: "", maker: "Google" },
    AgentCli { primary: true, id: "amp", name: "Amp", binary: "amp", color: 0xf34e3f, package: "@sourcegraph/amp", maker: "" },
    AgentCli {
        primary: false,
        id: "copilot",
        name: "GitHub Copilot CLI",
        binary: "copilot",
        color: 0x8957e5,
        package: "@github/copilot",
        maker: "",
    },
    AgentCli { primary: false, id: "cursor", name: "Cursor CLI", binary: "cursor-agent", color: 0xe6e6e6, package: "", maker: "Cursor" },
    AgentCli { primary: false, id: "opencode", name: "OpenCode", binary: "opencode", color: 0xf5a623, package: "opencode-ai", maker: "" },
    AgentCli { primary: false, id: "qwen", name: "Qwen Code", binary: "qwen", color: 0x615ced, package: "@qwen-code/", maker: "" },
    AgentCli { primary: false, id: "droid", name: "Factory Droid", binary: "droid", color: 0xee6018, package: "", maker: "" },
    AgentCli { primary: false, id: "goose", name: "Goose", binary: "goose", color: 0x9aa0a6, package: "", maker: "" },
    AgentCli { primary: false, id: "crush", name: "Crush", binary: "crush", color: 0xff5fd2, package: "", maker: "" },
    AgentCli { primary: false, id: "aider", name: "Aider", binary: "aider", color: 0x14b014, package: "/aider/", maker: "" },
    AgentCli { primary: false, id: "kimi", name: "Kimi CLI", binary: "kimi", color: 0x1783ff, package: "kimi-cli", maker: "" },
    AgentCli { primary: false, id: "kiro", name: "Kiro CLI", binary: "kiro-cli", color: 0x9046ff, package: "", maker: "" },
    AgentCli { primary: false, id: "cline", name: "Cline CLI", binary: "cline", color: 0xd4d4d4, package: "@cline/", maker: "" },
    AgentCli { primary: false, id: "grok", name: "Grok Build", binary: "grok", color: 0xe5e5e5, package: "grok-cli", maker: "xAI" },
];

/// Fallback when nothing is configured or used locally: aliases `claude --model` always accepts.
const CLAUDE_ALIASES: &[(&str, &str)] = &[("opus", "Opus"), ("sonnet", "Sonnet"), ("haiku", "Haiku")];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Installed {
    pub binaries: BTreeSet<String>,
    /// Models pulled into a local Ollama install.
    pub ollama_models: Vec<String>,
    /// Codex models seen in `~/.codex/config.toml` and recent sessions, most relevant first.
    pub codex_models: Vec<String>,
    /// Claude Code models as (value for `--model`, label): configured default, then recently used.
    pub claude_models: Vec<(String, String)>,
    /// CLI versions by binary (`claude`, `codex`), e.g. "2.1.274".
    pub versions: BTreeMap<String, String>,
}

impl Installed {
    pub fn has(&self, binary: &str) -> bool {
        self.binaries.contains(binary)
    }

    pub fn version(&self, binary: &str) -> Option<&str> {
        self.versions.get(binary).map(String::as_str)
    }

    pub fn other_agents(&self) -> impl Iterator<Item = &'static AgentCli> + '_ {
        OTHER_AGENTS.iter().filter(|a| self.has(a.binary))
    }

    /// How many agent CLIs this machine has. With one there is nothing a logo can tell apart, so
    /// the workspace cards leave it out. Claude Code and Codex are known apart from the list.
    pub fn agent_count(&self) -> usize {
        ["claude", "codex"].iter().filter(|binary| self.has(binary)).count() + self.other_agents().count()
    }
}

/// Whether the installed Claude Code accepts `--permission-mode auto`; an older one exits on it.
/// Read where the command line is built. Panes restored at startup spawn before the first
/// [`detect`] has answered, so the last answer is kept in the data folder until then.
static CLAUDE_AUTO_MODE: AtomicBool = AtomicBool::new(false);
static DETECTED: AtomicBool = AtomicBool::new(false);
const CLAUDE_AUTO_MODE_LINE: &str = "feature:claude-auto-mode";

fn auto_mode_marker() -> std::path::PathBuf {
    agentty_bridge::fsutil::data_dir().join("claude-auto-mode")
}

pub fn claude_auto_mode() -> bool {
    if DETECTED.load(Ordering::Relaxed) {
        CLAUDE_AUTO_MODE.load(Ordering::Relaxed)
    } else {
        auto_mode_marker().exists()
    }
}

fn remember_auto_mode(supported: bool) {
    CLAUDE_AUTO_MODE.store(supported, Ordering::Relaxed);
    DETECTED.store(true, Ordering::Relaxed);
    let marker = auto_mode_marker();
    if supported {
        let _ = std::fs::create_dir_all(agentty_bridge::fsutil::data_dir());
        let _ = std::fs::write(marker, "");
    } else {
        let _ = std::fs::remove_file(marker);
    }
}

/// Probe output in [`parse_probe`]'s format.
#[cfg(unix)]
fn probe() -> String {
    // Interactive login shell: many installs (e.g. `~/.local/bin` for Claude Code, nvm) only reach
    // PATH through `.zshrc`, which a login-only shell started from Finder never reads.
    std::process::Command::new(crate::launch::LaunchSpec::shell_program())
        .args(["-l", "-i", "-c", &probe_script()])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default()
}

/// Windows: programs are found on PATH (with PATHEXT) and asked for their version directly.
#[cfg(windows)]
fn probe() -> String {
    use agentty_bridge::process::{command, which};
    let run = |program: &std::path::Path, args: &[&str]| {
        command(program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default()
    };
    let mut out = String::new();
    for binary in ["claude", "codex", "ollama"].into_iter().chain(OTHER_AGENTS.iter().map(|a| a.binary)) {
        let Some(path) = which(binary) else { continue };
        out.push_str(&format!("bin:{binary}\n"));
        if matches!(binary, "claude" | "codex") {
            let version = run(&path, &["--version"]);
            out.push_str(&format!("version:{binary}:{}\n", version.lines().next().unwrap_or_default()));
        }
        // Same check as the Unix probe: does this Claude Code accept `--permission-mode auto`?
        if binary == "claude" && run(&path, &["--help"]).contains("\"auto\"") {
            out.push_str(&format!("{CLAUDE_AUTO_MODE_LINE}\n"));
        }
        if binary == "ollama" {
            for line in run(&path, &["list"]).lines() {
                out.push_str(&format!("ollama:{line}\n"));
            }
        }
    }
    out
}

#[cfg_attr(windows, allow(dead_code))]
fn probe_script() -> String {
    let binaries: Vec<&str> = ["claude", "codex", "ollama"].into_iter().chain(OTHER_AGENTS.iter().map(|a| a.binary)).collect();
    let list = binaries.iter().map(|b| shell_quote(b)).collect::<Vec<_>>().join(" ");
    // Shims living inside other apps' bundles (e.g. cmux's `grok`) are not real installs.
    format!(
        "for b in {list}; do p=$(command -v \"$b\" 2>/dev/null) || continue; case \"$p\" in *.app/Contents/*) ;; *) echo \"bin:$b\";; esac; done; for b in claude codex; do command -v \"$b\" >/dev/null 2>&1 && echo \"version:$b:$(\"$b\" --version 2>/dev/null </dev/null | head -n 1)\"; done; command -v claude >/dev/null 2>&1 && claude --help 2>/dev/null </dev/null | grep -q '\"auto\"' && echo \"{CLAUDE_AUTO_MODE_LINE}\"; command -v ollama >/dev/null 2>&1 && ollama list 2>/dev/null | sed 's/^/ollama:/'; true"
    )
}

/// "2.1.274 (Claude Code)", "codex-cli 0.154.0" → the version number.
fn parse_version(text: &str) -> Option<String> {
    text.split_whitespace()
        .map(|word| word.trim_start_matches(['v', 'V']))
        .find(|word| word.starts_with(|c: char| c.is_ascii_digit()) && word.contains('.'))
        .map(str::to_string)
}

pub fn parse_probe(output: &str) -> (BTreeSet<String>, Vec<String>, BTreeMap<String, String>) {
    let mut binaries = BTreeSet::new();
    let mut models = Vec::new();
    let mut versions = BTreeMap::new();
    for line in output.lines() {
        if let Some((bin, text)) = line.strip_prefix("version:").and_then(|rest| rest.split_once(':')) {
            if let Some(version) = parse_version(text) {
                versions.insert(bin.to_string(), version);
            }
        } else if let Some(bin) = line.strip_prefix("bin:") {
            binaries.insert(bin.trim().to_string());
        } else if let Some(row) = line.strip_prefix("ollama:") {
            // `ollama list`: NAME ID SIZE MODIFIED, with a header row.
            if let Some(name) = row.split_whitespace().next().filter(|n| *n != "NAME") {
                models.push(name.to_string());
            }
        }
    }
    (binaries, models, versions)
}

/// Model names from Codex's config (`model = "…"`) first, then from recent rollouts.
fn codex_models() -> Vec<String> {
    let mut models: Vec<String> = Vec::new();
    let home = crate::launch::home_dir();
    if let Ok(config) = std::fs::read_to_string(home.join(".codex/config.toml")) {
        for line in config.lines() {
            let line = line.trim();
            if let Some(value) = line.strip_prefix("model").map(str::trim_start).and_then(|v| v.strip_prefix('=')) {
                let value = value.trim().trim_matches('"').to_string();
                if !value.is_empty() && !models.contains(&value) {
                    models.push(value);
                }
                break;
            }
        }
    }
    for model in agentty_bridge::codex::recent_models(20) {
        if !models.contains(&model) {
            models.push(model);
        }
    }
    models.truncate(5);
    models
}

/// From `~/.claude/settings.json` and recent transcripts; the aliases only when neither has any.
fn claude_models() -> Vec<(String, String)> {
    let label = |model: &str| {
        let pretty = agentty_bridge::pretty_model(model.trim_end_matches("[1m]"));
        if model.ends_with("[1m]") {
            format!("{pretty} (1M)")
        } else {
            pretty
        }
    };
    let mut models: Vec<(String, String)> = Vec::new();
    for model in agentty_bridge::claude::configured_model().into_iter().chain(agentty_bridge::claude::recent_models(30)) {
        let entry = (model.clone(), label(&model));
        if !models.iter().any(|(m, l)| *m == entry.0 || *l == entry.1) {
            models.push(entry);
        }
    }
    models.truncate(5);
    if models.is_empty() {
        models = CLAUDE_ALIASES.iter().map(|(v, l)| (v.to_string(), l.to_string())).collect();
    }
    models
}

pub fn detect() -> Installed {
    let output = probe();
    let (binaries, ollama_models, versions) = parse_probe(&output);
    remember_auto_mode(output.lines().any(|line| line.trim() == CLAUDE_AUTO_MODE_LINE));
    let codex_models = if binaries.contains("codex") { codex_models() } else { Vec::new() };
    let claude_models = if binaries.contains("claude") { claude_models() } else { Vec::new() };
    Installed { binaries, ollama_models, codex_models, claude_models, versions }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One agent on the machine means the logo on a workspace card tells nothing apart, so the
    /// cards leave it out; two or more and it is worth the room again.
    #[test]
    fn one_agent_is_not_worth_a_logo() {
        let with = |bins: &[&str]| Installed { binaries: bins.iter().map(|b| (*b).to_string()).collect(), ..Default::default() };
        assert_eq!(with(&[]).agent_count(), 0);
        assert_eq!(with(&["claude"]).agent_count(), 1);
        assert_eq!(with(&["claude", "codex"]).agent_count(), 2);
        // A CLI from the list counts, anything else on PATH does not.
        assert_eq!(with(&["claude", "gemini"]).agent_count(), 2);
        assert_eq!(with(&["claude", "git", "node"]).agent_count(), 1);
    }

    #[test]
    fn parses_probe_output() {
        let out = "bin:claude\nbin:gemini\nversion:claude:2.1.274 (Claude Code)\nversion:codex:codex-cli 0.154.0\nversion:gemini:\nollama:NAME            ID    SIZE   MODIFIED\nollama:llama3.2:latest  a80c  2.0 GB 3 days ago\n";
        let (bins, models, versions) = parse_probe(out);
        assert_eq!(versions.get("claude").map(String::as_str), Some("2.1.274"));
        assert_eq!(versions.get("codex").map(String::as_str), Some("0.154.0"));
        assert!(!versions.contains_key("gemini"));
        assert!(bins.contains("claude") && bins.contains("gemini") && !bins.contains("codex"));
        assert_eq!(models, vec!["llama3.2:latest"]);
    }

    #[test]
    fn probe_script_is_quoted_and_complete() {
        let script = probe_script();
        assert!(script.contains("cursor-agent") && script.contains("ollama list"));
        assert_eq!(OTHER_AGENTS.iter().map(|a| a.id).collect::<BTreeSet<_>>().len(), OTHER_AGENTS.len());
    }
}
