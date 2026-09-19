//! `agentty db …`: an agent in an Agentty pane looks at its project's databases — the connections
//! Agentty found in the project's configuration or the user added on the database page. Reads run at
//! once inside a read-only transaction; a statement that writes or changes the schema is shown to the
//! user, who executes or declines it. Passwords never reach the agent.

use std::io::{BufRead, BufReader, Write};
use std::time::Duration;

pub const HELP: &str = "agentty db — read this project's databases (writes need the user's approval)

  agentty db list                                   connections of this project
  agentty db tables    [--conn <name|n>]            tables / collections
  agentty db describe  <table> [--conn <name|n>]    columns of a table
  agentty db preview   <table> [--conn <name|n>] [--limit N]
  agentty db query     \"<SQL>\" [--conn <name|n>] [--limit N]
  agentty db query     --file <query.sql> [--conn <name|n>]
  agentty db mongo     <op> <collection> ['<json>'] [--conn <name|n>] [--limit N]

--conn is needed when the project has several connections (a name or the number from `list`).
Read statements (SELECT, SHOW, DESCRIBE, EXPLAIN, WITH … SELECT that call only built-in functions)
run right away in a read-only transaction. Anything else (INSERT, UPDATE, DELETE, DDL, several statements in one) waits until the
user executes or declines it in Agentty; the command waits for that answer. MongoDB operations:
find, count, distinct, aggregate, collections (read); insertOne, insertMany, updateOne,
updateMany, replaceOne, deleteOne, deleteMany, createIndex, dropIndex, drop (need approval). The JSON
is a filter for find / count / delete*, a pipeline array for aggregate, {\"field\", \"filter\"} for
distinct, {\"filter\", \"update\"} for update*, {\"filter\", \"replacement\"} for replaceOne, a
document (or an array for insertMany) for insert*. Prints JSON; exits 1 on an error or a declined write.
Works in terminals opened by Agentty (uses $AGENTTY_SOCKET).";

/// The request for the command line: `{"action": …, …}`.
fn request_from_args(args: &[String]) -> Result<serde_json::Value, String> {
    let mut rest = args.iter();
    let action = rest.next().cloned().unwrap_or_default();
    let mut positional = Vec::new();
    let mut request = serde_json::json!({ "action": action });
    while let Some(arg) = rest.next() {
        let mut value = || rest.next().cloned().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--conn" | "-c" => request["conn"] = value()?.into(),
            "--limit" | "-n" => {
                let limit: u64 = value()?.parse().map_err(|_| "--limit takes a number".to_string())?;
                request["limit"] = limit.into();
            }
            "--file" | "-f" => {
                let path = value()?;
                positional.push(std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?);
            }
            other if other.starts_with("--") => return Err(format!("unknown option {other} (see agentty db --help)")),
            other => positional.push(other.to_string()),
        }
    }
    let one = |what: &str| positional.first().cloned().ok_or_else(|| format!("agentty db {action} needs {what}"));
    match action.as_str() {
        "list" | "tables" => {}
        "describe" | "preview" => request["table"] = one("a table name")?.into(),
        "query" | "sql" => {
            request["action"] = "query".into();
            request["sql"] = positional.join(" ").into();
            if positional.is_empty() {
                return Err("agentty db query needs a statement (or --file)".into());
            }
        }
        "mongo" => {
            let op = one("an operation")?;
            let collection = positional.get(1).cloned().ok_or("agentty db mongo needs a collection")?;
            let args = match positional.get(2) {
                Some(json) => serde_json::from_str(json).map_err(|e| format!("the arguments are not JSON: {e}"))?,
                None => serde_json::json!({}),
            };
            request["op"] = op.into();
            request["collection"] = collection.into();
            request["args"] = args;
        }
        other => return Err(format!("unknown command {other} (see agentty db --help)")),
    }
    Ok(request)
}

pub fn run(args: &[String]) -> i32 {
    if args.is_empty() || args.iter().any(|a| matches!(a.as_str(), "help" | "-h" | "--help")) {
        println!("{HELP}");
        return 0;
    }
    let mut request = match request_from_args(args) {
        Ok(request) => request,
        Err(err) => {
            eprintln!("agentty db: {err}");
            return 1;
        }
    };
    let Ok(socket) = std::env::var("AGENTTY_SOCKET") else {
        eprintln!("agentty db: not running inside an Agentty terminal ($AGENTTY_SOCKET is not set)");
        return 1;
    };
    request["cwd"] = std::env::current_dir().unwrap_or_default().display().to_string().into();
    let reply = (|| -> std::io::Result<String> {
        let mut stream = crate::ipc::connect(&socket)?;
        writeln!(stream, "db\t{request}")?;
        // A write waits for the user (Agentty gives up after 15 minutes).
        stream.set_read_timeout(Some(Duration::from_secs(16 * 60)))?;
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line)?;
        Ok(line)
    })();
    let line = match reply {
        Ok(line) => line,
        Err(err) => {
            eprintln!("agentty db: {err}");
            return 1;
        }
    };
    let reply: serde_json::Value = serde_json::from_str(line.trim()).unwrap_or_default();
    if reply["ok"].as_bool() == Some(true) {
        println!("{}", serde_json::to_string_pretty(&reply["result"]).unwrap_or_default());
        0
    } else {
        eprintln!("agentty db: {}", reply["error"].as_str().unwrap_or("no response from Agentty"));
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
    fn builds_requests() {
        let list = request_from_args(&strings(&["list"])).unwrap();
        assert_eq!(list, serde_json::json!({ "action": "list" }));
        let query = request_from_args(&strings(&["query", "SELECT 1", "--conn", "2", "--limit", "5"])).unwrap();
        assert_eq!(query, serde_json::json!({ "action": "query", "sql": "SELECT 1", "conn": "2", "limit": 5 }));
        let preview = request_from_args(&strings(&["preview", "users"])).unwrap();
        assert_eq!(preview["table"], "users");
        let mongo = request_from_args(&strings(&["mongo", "find", "users", r#"{"age": 3}"#])).unwrap();
        assert_eq!(mongo["args"]["age"], 3);
        assert!(request_from_args(&strings(&["describe"])).is_err());
        assert!(request_from_args(&strings(&["query"])).is_err());
        assert!(request_from_args(&strings(&["drop"])).is_err());
        assert!(request_from_args(&strings(&["mongo", "find", "users", "{not json"])).is_err());
    }
}
