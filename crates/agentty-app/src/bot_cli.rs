//! `agentty bot …`: an agent in a pane hands work to a bot and reads its answer. One JSON line to
//! the app's socket, one back, like `agentty tasks`. The command only talks to bots that exist:
//! making one, changing its permission or connecting a channel is done by the user in the app.

use std::io::{BufRead, BufReader, Write};
use std::time::Duration;

const HELP: &str = "agentty bot — hand work to a bot and read its answer

  agentty bot list
      Every bot: handle, purpose, state (idle, working, needs the user, off).
  agentty bot send <handle> --prompt \"<text>\"
  agentty bot send <handle> --prompt-file <file>
      Puts a message in the bot's mailbox and returns at once. The bot's answer arrives later in
      this session as a message that starts with [Agentty]. Do not wait, poll or sleep for it.
  agentty bot result <handle>
      The bot's last answer again.
  agentty bot status [<handle>]
      Who is working, who has messages waiting, who needs the user.

Bots are made and changed in Agentty (the Bots page), not from here.";

/// The request for the app's socket, from the command's arguments.
pub fn request_from_args(args: &[String]) -> Result<serde_json::Value, String> {
    let action = args[0].clone();
    let (mut to, mut prompt) = (String::new(), String::new());
    let mut rest = args[1..].iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--prompt" => prompt = rest.next().ok_or("--prompt needs a text")?.clone(),
            "--prompt-file" => {
                let path = rest.next().ok_or("--prompt-file needs a path")?;
                prompt = std::fs::read_to_string(path).map_err(|err| format!("{path}: {err}"))?;
            }
            "--to" => to = rest.next().ok_or("--to needs a handle")?.clone(),
            option if option.starts_with("--") => return Err(format!("unknown option {option} (see agentty bot --help)")),
            handle if to.is_empty() => to = handle.to_string(),
            extra => return Err(format!("unexpected \"{extra}\": put the message in --prompt or --prompt-file")),
        }
    }
    let request = serde_json::json!({ "action": action, "to": to, "prompt": prompt });
    crate::agent_signal::parse_bot_ctl(&request)?;
    Ok(request)
}

pub fn run(args: &[String]) -> i32 {
    if args.is_empty() || matches!(args[0].as_str(), "help" | "-h" | "--help") {
        println!("{HELP}");
        return 0;
    }
    let request = match request_from_args(args) {
        Ok(request) => request,
        Err(error) => {
            eprintln!("agentty bot: {error}");
            return 1;
        }
    };
    let Ok(socket) = std::env::var("AGENTTY_SOCKET") else {
        eprintln!("agentty bot: not running inside an Agentty terminal ($AGENTTY_SOCKET is not set)");
        return 1;
    };
    let reply = (|| -> std::io::Result<String> {
        let mut stream = crate::ipc::connect(&socket)?;
        writeln!(stream, "bot\t{request}")?;
        stream.set_read_timeout(Some(Duration::from_secs(20)))?;
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line)?;
        Ok(line)
    })();
    let line = match reply {
        Ok(line) => line,
        Err(err) => {
            eprintln!("agentty bot: {err}");
            return 1;
        }
    };
    let reply: serde_json::Value = serde_json::from_str(line.trim()).unwrap_or_default();
    if reply["ok"].as_bool() == Some(true) {
        println!("{}", reply["result"]);
        0
    } else {
        eprintln!("agentty bot: {}", reply["error"].as_str().unwrap_or("no response from Agentty"));
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(line: &str) -> Vec<String> {
        line.split(' ').map(str::to_string).collect()
    }

    #[test]
    fn requests_are_checked_before_they_are_sent() {
        assert_eq!(request_from_args(&args("list")).unwrap()["action"], "list");
        assert_eq!(request_from_args(&args("status reviewer")).unwrap()["to"], "reviewer");
        let send = request_from_args(&["send".into(), "@reviewer".into(), "--prompt".into(), "look at the branch".into()]).unwrap();
        assert_eq!((send["to"].as_str(), send["prompt"].as_str()), (Some("@reviewer"), Some("look at the branch")));
        assert!(request_from_args(&args("send reviewer")).is_err(), "nothing to say");
        assert!(request_from_args(&args("send --prompt hi")).is_err(), "nobody to say it to");
        assert!(request_from_args(&args("result")).is_err());
        assert!(request_from_args(&args("send Reviewer! --prompt hi")).is_err(), "not a handle");
        assert!(request_from_args(&args("send reviewer hi there")).is_err(), "a message outside --prompt");
        // There is nothing here that makes a bot or widens what one may do.
        for action in ["create", "delete", "permission", "channel", "enable"] {
            assert!(request_from_args(&args(&format!("{action} reviewer"))).is_err(), "{action}");
        }
    }

    #[test]
    fn a_prompt_can_come_from_a_file() {
        let path = std::env::temp_dir().join(format!("agentty-bot-prompt-{}.md", std::process::id()));
        std::fs::write(&path, "Review the parser.\n").unwrap();
        let request = request_from_args(&["send".into(), "reviewer".into(), "--prompt-file".into(), path.display().to_string()]).unwrap();
        assert_eq!(request["prompt"], "Review the parser.\n");
        let _ = std::fs::remove_file(path);
    }
}
