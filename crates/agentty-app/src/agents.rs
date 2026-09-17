//! Which AI coding agents and local models are installed, so the launcher only offers what runs.
//! Detection goes through the login shell once (PATH from nvm, brew, … matches the user's terminal).

use crate::launch::shell_quote;
use std::collections::{BTreeMap, BTreeSet};

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
}

/// Agents besides Claude Code and Codex (those have first-class panes with hooks and sessions).
pub const OTHER_AGENTS: &[AgentCli] = &[
    AgentCli { primary: true, id: "gemini", name: "Gemini CLI", binary: "gemini", color: 0x4796e3, package: "@google/gemini-cli" },
    AgentCli { primary: true, id: "agy", name: "Antigravity CLI", binary: "agy", color: 0x3c82f6, package: "" },
    AgentCli { primary: true, id: "amp", name: "Amp", binary: "amp", color: 0xf34e3f, package: "@sourcegraph/amp" },
    AgentCli { primary: false, id: "copilot", name: "GitHub Copilot CLI", binary: "copilot", color: 0x8957e5, package: "@github/copilot" },
    AgentCli { primary: false, id: "cursor", name: "Cursor Agent", binary: "cursor-agent", color: 0xe6e6e6, package: "" },
    AgentCli { primary: false, id: "opencode", name: "OpenCode", binary: "opencode", color: 0xf5a623, package: "opencode-ai" },
    AgentCli { primary: false, id: "qwen", name: "Qwen Code", binary: "qwen", color: 0x615ced, package: "@qwen-code/" },
    AgentCli { primary: false, id: "droid", name: "Factory Droid", binary: "droid", color: 0xee6018, package: "" },
    AgentCli { primary: false, id: "goose", name: "Goose", binary: "goose", color: 0x9aa0a6, package: "" },
    AgentCli { primary: false, id: "crush", name: "Crush", binary: "crush", color: 0xff5fd2, package: "" },
    AgentCli { primary: false, id: "aider", name: "Aider", binary: "aider", color: 0x14b014, package: "/aider/" },
    AgentCli { primary: false, id: "kimi", name: "Kimi CLI", binary: "kimi", color: 0x1783ff, package: "kimi-cli" },
    AgentCli { primary: false, id: "kiro", name: "Kiro CLI", binary: "kiro-cli", color: 0x9046ff, package: "" },
    AgentCli { primary: false, id: "cline", name: "Cline CLI", binary: "cline", color: 0xd4d4d4, package: "@cline/" },
    AgentCli { primary: false, id: "grok", name: "Grok CLI", binary: "grok", color: 0xe5e5e5, package: "grok-cli" },
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
}

fn probe_script() -> String {
    let binaries: Vec<&str> = ["claude", "codex", "ollama"].into_iter().chain(OTHER_AGENTS.iter().map(|a| a.binary)).collect();
    let list = binaries.iter().map(|b| shell_quote(b)).collect::<Vec<_>>().join(" ");
    // Shims living inside other apps' bundles (e.g. cmux's `grok`) are not real installs.
    format!(
        "for b in {list}; do p=$(command -v \"$b\" 2>/dev/null) || continue; case \"$p\" in *.app/Contents/*) ;; *) echo \"bin:$b\";; esac; done; for b in claude codex; do command -v \"$b\" >/dev/null 2>&1 && echo \"version:$b:$(\"$b\" --version 2>/dev/null </dev/null | head -n 1)\"; done; command -v ollama >/dev/null 2>&1 && ollama list 2>/dev/null | sed 's/^/ollama:/'; true"
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
    // Interactive login shell: many installs (e.g. `~/.local/bin` for Claude Code, nvm) only reach
    // PATH through `.zshrc`, which a login-only shell started from Finder never reads.
    let output = std::process::Command::new(crate::launch::LaunchSpec::shell_program())
        .args(["-l", "-i", "-c", &probe_script()])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    let (binaries, ollama_models, versions) = parse_probe(&output);
    let codex_models = if binaries.contains("codex") { codex_models() } else { Vec::new() };
    let claude_models = if binaries.contains("claude") { claude_models() } else { Vec::new() };
    Installed { binaries, ollama_models, codex_models, claude_models, versions }
}

#[cfg(test)]
mod tests {
    use super::*;

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
