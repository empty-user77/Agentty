//! `agentty tasks …`: an agent in an Agentty pane hands pieces of work to new sessions. Agentty asks
//! the user first; once they agree, every task gets a working tree of its own (a new branch from the
//! project's default branch) and a pane split off the asking agent's, where Claude Code or Codex
//! starts with the task's prompt.

use std::io::{BufRead, BufReader, Write};
use std::time::Duration;

pub const HELP: &str = "agentty tasks — start work in parallel sessions next to this one

  agentty tasks --title <title> --prompt-file <file> [--agent claude|codex]
  agentty tasks --title <title> --prompt <text> [--agent claude|codex]
  agentty tasks --plan <plan.json>
  agentty tasks status
  agentty tasks send --to <title> --prompt <text>

A plan is a JSON array of up to 6 tasks: [{\"title\": \"…\", \"prompt\": \"…\", \"agent\": \"claude\"}, …].
Agentty shows the tasks to the user, who starts or declines them. Each started task runs in a
split pane (from three tasks on, in a tab of its own), in its own git worktree on a new branch
from the project's default branch (the project is this folder, or the asking agent's folder
when this one is outside git), and receives its prompt as the first message: make every prompt self-contained (goal, files, rules,
how to verify, whether to commit and open a pull request).

Prints the started tasks as JSON (title, branch, folder); exits 1 when the user declined or
Agentty could not start them. Works in terminals opened by Agentty (uses $AGENTTY_SOCKET).

In a chat workspace the lead agent's tasks start at once as workers in the grid below the chat
(at most 4; a finished worker makes room for a new one). There `status` lists the workers and
what each is doing, and `send` gives one of them a follow-up message.";

/// The tasks named on the command line: one from `--title` / `--prompt[-file]` / `--agent`, or all
/// of `--plan`.
fn tasks_from_args(args: &[String]) -> Result<serde_json::Value, String> {
    let mut title = None;
    let mut prompt = None;
    let mut agent = None;
    let mut plan = None;
    let mut rest = args.iter();
    while let Some(flag) = rest.next() {
        let mut value = || rest.next().cloned().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--title" => title = Some(value()?),
            "--prompt" => prompt = Some(value()?),
            "--prompt-file" => {
                let path = value()?;
                prompt = Some(std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?);
            }
            "--agent" => agent = Some(value()?),
            "--plan" => {
                let path = value()?;
                let text = std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
                plan = Some(serde_json::from_str::<serde_json::Value>(&text).map_err(|e| format!("{path}: {e}"))?);
            }
            other => return Err(format!("unknown option {other} (see agentty tasks --help)")),
        }
    }
    match (plan, title, prompt) {
        (Some(plan), None, None) => Ok(plan),
        (None, Some(title), Some(prompt)) => Ok(serde_json::json!([{ "title": title, "prompt": prompt, "agent": agent }])),
        (Some(_), _, _) => Err("use --plan alone, or --title with --prompt / --prompt-file".into()),
        _ => Err("a task needs --title and --prompt (or --prompt-file); see agentty tasks --help".into()),
    }
}

/// The request of `agentty tasks status` / `send …`.
fn control_from_args(args: &[String]) -> Result<serde_json::Value, String> {
    let (action, rest) = args.split_first().ok_or("no action")?;
    let (mut to, mut prompt) = (None, None);
    let mut rest = rest.iter();
    while let Some(flag) = rest.next() {
        let mut value = || rest.next().cloned().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--to" => to = Some(value()?),
            "--prompt" => prompt = Some(value()?),
            "--prompt-file" => {
                let path = value()?;
                prompt = Some(std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?);
            }
            other => return Err(format!("unknown option {other} (see agentty tasks --help)")),
        }
    }
    let request = serde_json::json!({ "action": action, "to": to, "prompt": prompt });
    crate::agent_signal::parse_tasks_ctl(&request)?;
    Ok(request)
}

/// Sends one line to Agentty and prints its answer; `wait` is how long the answer may take.
fn ask(line: String, wait: Duration) -> i32 {
    let Ok(socket) = std::env::var("AGENTTY_SOCKET") else {
        eprintln!("agentty tasks: not running inside an Agentty terminal ($AGENTTY_SOCKET is not set)");
        return 1;
    };
    let reply = (|| -> std::io::Result<String> {
        let mut stream = crate::ipc::connect(&socket)?;
        writeln!(stream, "{line}")?;
        stream.set_read_timeout(Some(wait))?;
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line)?;
        Ok(line)
    })();
    let line = match reply {
        Ok(line) => line,
        Err(err) => {
            eprintln!("agentty tasks: {err}");
            return 1;
        }
    };
    let reply: serde_json::Value = serde_json::from_str(line.trim()).unwrap_or_default();
    if reply["ok"].as_bool() == Some(true) {
        println!("{}", reply["result"]);
        0
    } else {
        eprintln!("agentty tasks: {}", reply["error"].as_str().unwrap_or("no response from Agentty"));
        1
    }
}

pub fn run(args: &[String]) -> i32 {
    if args.is_empty() || args.iter().any(|a| matches!(a.as_str(), "help" | "-h" | "--help")) {
        println!("{HELP}");
        return 0;
    }
    if matches!(args[0].as_str(), "status" | "send") {
        return match control_from_args(args) {
            Ok(request) => ask(format!("tasksctl\t{request}"), Duration::from_secs(15)),
            Err(err) => {
                eprintln!("agentty tasks: {err}");
                1
            }
        };
    }
    let tasks = match tasks_from_args(args).and_then(|t| crate::agent_signal::parse_tasks(&t).map(|_| t)) {
        Ok(tasks) => tasks,
        Err(err) => {
            eprintln!("agentty tasks: {err}");
            return 1;
        }
    };
    let cwd = std::env::current_dir().unwrap_or_default();
    // The user answers in a dialog: wait for them (Agentty gives up after 15 minutes).
    ask(format!("tasks\t{}", serde_json::json!({ "cwd": cwd, "tasks": tasks })), Duration::from_secs(16 * 60))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn one_task_from_flags_or_many_from_a_plan() {
        let one = tasks_from_args(&strings(&["--title", "Docker", "--prompt", "Build the panel", "--agent", "codex"])).unwrap();
        assert_eq!(one, serde_json::json!([{ "title": "Docker", "prompt": "Build the panel", "agent": "codex" }]));

        let dir = std::env::temp_dir().join(format!("agentty-tasks-cli-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let plan = dir.join("plan.json");
        std::fs::write(&plan, r#"[{"title":"A","prompt":"a"},{"title":"B","prompt":"b"}]"#).unwrap();
        let many = tasks_from_args(&strings(&["--plan", &plan.display().to_string()])).unwrap();
        assert_eq!(many.as_array().map(Vec::len), Some(2));
        let prompt = dir.join("prompt.md");
        std::fs::write(&prompt, "Do it\n").unwrap();
        let from_file = tasks_from_args(&strings(&["--title", "C", "--prompt-file", &prompt.display().to_string()])).unwrap();
        assert_eq!(from_file[0]["prompt"], "Do it\n");
        std::fs::remove_dir_all(dir).ok();

        assert!(tasks_from_args(&strings(&["--title", "no prompt"])).is_err());
        assert!(tasks_from_args(&strings(&["--plan", "/nonexistent/plan.json"])).is_err());
        assert!(tasks_from_args(&strings(&["--title"])).is_err());
        assert!(tasks_from_args(&strings(&["--bogus", "x"])).is_err());
    }

    #[test]
    fn status_and_send_are_checked_before_they_are_sent() {
        assert_eq!(control_from_args(&strings(&["status"])).unwrap()["action"], "status");
        let send = control_from_args(&strings(&["send", "--to", "API", "--prompt", "Add tests"])).unwrap();
        assert_eq!((send["to"].as_str(), send["prompt"].as_str()), (Some("API"), Some("Add tests")));
        assert!(control_from_args(&strings(&["send", "--to", "API"])).is_err());
        assert!(control_from_args(&strings(&["send", "--bogus"])).is_err());
    }
}
