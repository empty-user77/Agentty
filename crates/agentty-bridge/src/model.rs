use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, serde::Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Agent {
    Claude,
    Codex,
}

impl Agent {
    pub fn display_name(self) -> &'static str {
        match self {
            Agent::Claude => "Claude Code",
            Agent::Codex => "Codex",
        }
    }

    /// Arguments that resume a native session of this agent.
    pub fn resume_args(self, id: &str) -> Vec<String> {
        match self {
            Agent::Claude => vec!["claude".into(), "--resume".into(), id.into()],
            Agent::Codex => vec!["codex".into(), "resume".into(), id.into()],
        }
    }

    /// Arguments that start a fresh session of this agent with an initial prompt.
    pub fn prompt_args(self, prompt: &str) -> Vec<String> {
        match self {
            Agent::Claude => vec!["claude".into(), prompt.into()],
            Agent::Codex => vec!["codex".into(), prompt.into()],
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
