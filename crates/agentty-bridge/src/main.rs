//! `agentty-bridge` — discovers local Claude Code / Codex sessions and migrates context between them.
//! Every command prints JSON to stdout for scripting; the Agentty app links the library directly.

use agentty_bridge::{handoff, model::Agent};
use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use serde::Serialize;

#[derive(Parser)]
#[command(name = "agentty-bridge", version, about)]
struct Cli {
    /// Pretty-print JSON output.
    #[arg(long, global = true)]
    pretty: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List local sessions, newest first.
    List {
        #[arg(long)]
        agent: Option<Agent>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Print the user/assistant turns of a session.
    Transcript {
        #[arg(long)]
        agent: Agent,
        #[arg(long)]
        id: String,
    },
    /// Aggregate token usage and cost for the last N days.
    Usage {
        #[arg(long)]
        agent: Agent,
        #[arg(long, default_value_t = 7)]
        days: u32,
    },
    /// List skills, subagents, commands, plugins and MCP servers available to an agent.
    Extensions {
        #[arg(long)]
        agent: Agent,
        /// Project directory for project-scoped items.
        #[arg(long)]
        project: Option<std::path::PathBuf>,
    },
    /// List the subagents a Claude Code session ran.
    Subagents {
        #[arg(long)]
        id: String,
    },
    /// Run an API connector as an MCP server over stdio.
    McpConnector {
        #[arg(long)]
        id: String,
    },
    /// Write a handoff document and print the command that continues it in the target agent.
    Handoff {
        #[arg(long)]
        agent: Agent,
        #[arg(long)]
        id: String,
        #[arg(long)]
        to: Agent,
    },
}

fn main() {
    let cli = Cli::parse();
    if let Err(err) = run(&cli) {
        eprintln!("{err:#}");
        std::process::exit(1);
    }
}

fn run(cli: &Cli) -> Result<()> {
    match &cli.command {
        Command::List { agent, limit } => print(cli, &agentty_bridge::list(*agent, *limit)),
        Command::Transcript { agent, id } => {
            let (_, turns) = agentty_bridge::load(*agent, id)?;
            print(cli, &turns)
        }
        Command::Usage { agent, days } => {
            let usage = agentty_bridge::usage::UsageScanner::default().scan(*agent);
            let today = chrono::Local::now().date_naive();
            print(cli, &agentty_bridge::usage::UsageReport::build(&usage, today, *days))
        }
        Command::Extensions { agent, project } => print(cli, &agentty_bridge::extensions::discover(*agent, project.as_deref())),
        Command::Subagents { id } => print(cli, &agentty_bridge::claude::subagents(id)),
        Command::McpConnector { id } => agentty_bridge::connectors::serve(id),
        Command::Handoff { agent, id, to } => {
            if agent == to {
                bail!("source and target agent are the same; use resume instead");
            }
            let (cwd, turns) = agentty_bridge::load(*agent, id)?;
            if turns.is_empty() {
                bail!("session {id} has no conversation to hand off");
            }
            print(cli, &handoff::create(*agent, id, *to, cwd, &turns)?)
        }
    }
}

fn print<T: Serialize + ?Sized>(cli: &Cli, value: &T) -> Result<()> {
    let json = if cli.pretty { serde_json::to_string_pretty(value)? } else { serde_json::to_string(value)? };
    println!("{json}");
    Ok(())
}
