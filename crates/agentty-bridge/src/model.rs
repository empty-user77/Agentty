use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, serde::Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Agent {
    Claude,
    Codex,
    /// Antigravity CLI (`agy`).
    Agy,
    Gemini,
    Kimi,
    Amp,
}

impl Agent {
    pub const ALL: [Agent; 6] = [Agent::Claude, Agent::Codex, Agent::Agy, Agent::Gemini, Agent::Kimi, Agent::Amp];

    pub fn display_name(self) -> &'static str {
        match self {
            Agent::Claude => "Claude Code",
            Agent::Codex => "Codex",
            Agent::Agy => "Antigravity CLI",
            Agent::Gemini => "Gemini CLI",
            Agent::Kimi => "Kimi CLI",
            Agent::Amp => "Amp",
        }
    }

    /// Short label for filters.
    pub fn short_name(self) -> &'static str {
        match self {
            Agent::Claude => "Claude",
            Agent::Codex => "Codex",
            Agent::Agy => "Antigravity",
            Agent::Gemini => "Gemini",
            Agent::Kimi => "Kimi",
            Agent::Amp => "Amp",
        }
    }

    /// Stable id (brand, settings keys): `claude`, `codex`, `agy`, `gemini`, `kimi`, `amp`.
    pub fn id(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
            Agent::Agy => "agy",
            Agent::Gemini => "gemini",
            Agent::Kimi => "kimi",
            Agent::Amp => "amp",
        }
    }

    /// Claude Code and Codex get first-class panes (hooks, handoffs, usage); the others run as commands.
    pub fn is_first_class(self) -> bool {
        matches!(self, Agent::Claude | Agent::Codex)
    }

    /// Arguments that resume a native session of this agent.
    pub fn resume_args(self, id: &str) -> Vec<String> {
        let args: &[&str] = match self {
            Agent::Claude => &["claude", "--resume"],
            Agent::Codex => &["codex", "resume"],
            Agent::Agy => &["agy", "--conversation"],
            Agent::Gemini => &["gemini", "--resume"],
            Agent::Kimi => &["kimi", "--session"],
            Agent::Amp => &["amp", "threads", "continue"],
        };
        args.iter().map(|a| a.to_string()).chain(std::iter::once(id.to_string())).collect()
    }

    /// Arguments that start a fresh session of this agent with an initial prompt.
    pub fn prompt_args(self, prompt: &str) -> Vec<String> {
        match self {
            Agent::Claude => vec!["claude".into(), prompt.into()],
            Agent::Codex => vec!["codex".into(), prompt.into()],
            Agent::Agy => vec!["agy".into(), "-i".into(), prompt.into()],
            Agent::Gemini => vec!["gemini".into(), "-i".into(), prompt.into()],
            Agent::Kimi => vec!["kimi".into(), "--prompt".into(), prompt.into()],
            Agent::Amp => vec!["amp".into(), prompt.into()],
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub agent: Agent,
    pub id: String,
    pub title: String,
    pub cwd: Option<String>,
    /// Unix epoch milliseconds of the last write to the session file.
    pub updated_at: u64,
    pub path: PathBuf,
    pub resume_args: Vec<String>,
    /// The most recent prompt and reply (one line each), to tell sessions apart.
    pub last_prompt: Option<String>,
    pub last_reply: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize)]
pub struct Turn {
    pub role: Role,
    pub text: String,
}
