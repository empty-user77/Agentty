//! `agentty group …`: a member of an agent group lists the other members, or hands a request that
//! belongs to another member's role to that member. Works only in a pane Agentty started as part of
//! a group.

use std::io::{BufRead, BufReader, Write};
use std::time::Duration;

pub const HELP: &str = "agentty group — work with the other members of your agent group

  agentty group list
  agentty group send <member> <request>
  agentty group send <member> --file <file>

`list` prints the members with their roles and whether they are working, idle or closed.
`send` hands a request to the member whose role it belongs to: it arrives in that member's
terminal as a new message (after its current turn, if it is working). Put everything the member
needs in the request: it does not see your conversation.

Works in the terminals of an agent group started by Agentty (uses $AGENTTY_SOCKET).";

fn request_from_args(args: &[String]) -> Result<serde_json::Value, String> {
    match args.first().map(String::as_str) {
        Some("list") if args.len() == 1 => Ok(serde_json::json!({ "action": "list" })),
        Some("send") => {
            let to = args.get(1).ok_or("send needs a member and a request")?;
            let message = match args.get(2).map(String::as_str) {
                Some("--file") => {
                    let path = args.get(3).ok_or("--file needs a path")?;
                    std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?
                }
                Some(_) => args[2..].join(" "),
                None => return Err("send needs a request after the member's name".into()),
            };
            if message.trim().is_empty() {
                return Err("the request is empty".into());
            }
            Ok(serde_json::json!({ "action": "send", "to": to, "message": message }))
        }
        _ => Err("use list or send (see agentty group --help)".into()),
    }
}

pub fn run(args: &[String]) -> i32 {
    if args.is_empty() || args.iter().any(|a| matches!(a.as_str(), "help" | "-h" | "--help")) {
        println!("{HELP}");
        return 0;
    }
    let request = match request_from_args(args) {
        Ok(request) => request,
        Err(err) => {
            eprintln!("agentty group: {err}");
            return 1;
        }
    };
    let Ok(socket) = std::env::var("AGENTTY_SOCKET") else {
        eprintln!("agentty group: not running inside an Agentty terminal ($AGENTTY_SOCKET is not set)");
        return 1;
    };
    let reply = (|| -> std::io::Result<String> {
        let mut stream = crate::ipc::connect(&socket)?;
        writeln!(stream, "group\t{request}")?;
        stream.set_read_timeout(Some(Duration::from_secs(40)))?;
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line)?;
        Ok(line)
    })();
    let line = match reply {
        Ok(line) => line,
        Err(err) => {
            eprintln!("agentty group: {err}");
            return 1;
        }
    };
    let reply: serde_json::Value = serde_json::from_str(line.trim()).unwrap_or_default();
    if reply["ok"].as_bool() == Some(true) {
        println!("{}", reply["result"]);
        0
    } else {
        eprintln!("agentty group: {}", reply["error"].as_str().unwrap_or("no response from Agentty"));
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
    fn list_and_send_requests() {
        assert_eq!(request_from_args(&strings(&["list"])).unwrap(), serde_json::json!({ "action": "list" }));
        let send = request_from_args(&strings(&["send", "Deploy", "ship", "e-ticket"])).unwrap();
        assert_eq!(send, serde_json::json!({ "action": "send", "to": "Deploy", "message": "ship e-ticket" }));
        assert!(request_from_args(&strings(&["send", "Deploy"])).is_err());
        assert!(request_from_args(&strings(&["send", "Deploy", "  "])).is_err());
        assert!(request_from_args(&strings(&["send", "Deploy", "--file"])).is_err());
        assert!(request_from_args(&strings(&["delete"])).is_err());
    }
}
