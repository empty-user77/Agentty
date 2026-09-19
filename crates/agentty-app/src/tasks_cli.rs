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

A plan is a JSON array of up to 6 tasks: [{\"title\": \"…\", \"prompt\": \"…\", \"agent\": \"claude\"}, …].
Agentty shows the tasks to the user, who starts or declines them. Each started task runs in a
split pane, in its own git worktree on a new branch from the project's default branch, and
receives its prompt as the first message: make every prompt self-contained (goal, files, rules,
how to verify, whether to commit and open a pull request).

Prints the started tasks as JSON (title, branch, folder); exits 1 when the user declined or
Agentty could not start them. Works in terminals opened by Agentty (uses $AGENTTY_SOCKET).";

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

pub fn run(args: &[String]) -> i32 {
    if args.is_empty() || args.iter().any(|a| matches!(a.as_str(), "help" | "-h" | "--help")) {
        println!("{HELP}");
        return 0;
    }
    let tasks = match tasks_from_args(args).and_then(|t| crate::agent_signal::parse_tasks(&t).map(|_| t)) {
        Ok(tasks) => tasks,
        Err(err) => {
            eprintln!("agentty tasks: {err}");
            return 1;
        }
    };
    let Ok(socket) = std::env::var("AGENTTY_SOCKET") else {
        eprintln!("agentty tasks: not running inside an Agentty terminal ($AGENTTY_SOCKET is not set)");
        return 1;
    };
    let cwd = std::env::current_dir().unwrap_or_default();
    let reply = (|| -> std::io::Result<String> {
        let mut stream = crate::ipc::connect(&socket)?;
        writeln!(stream, "tasks\t{}", serde_json::json!({ "cwd": cwd, "tasks": tasks }))?;
        // The user answers in a dialog: wait for them (Agentty gives up after 15 minutes).
        stream.set_read_timeout(Some(Duration::from_secs(16 * 60)))?;
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
}
