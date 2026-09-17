//! `agentty browser …`: drives Agentty's in-app browser from a terminal pane — for people and for
//! AI agents (Claude Code, Codex) testing web apps they build.

use std::io::{BufRead, BufReader, Write};

pub const HELP: &str = "agentty browser — control Agentty's in-app browser

  agentty browser open [url]            show the browser (and load url)
  agentty browser navigate <url>        load a URL (localhost:3000, https://…, search words)
  agentty browser wait-load [ms]        wait until the page finished loading
  agentty browser url | title | status  current page
  agentty browser back | forward | reload | close
  agentty browser text [selector]       visible text of the page or an element
  agentty browser html [selector]       HTML of the page or an element
  agentty browser elements              clickable / input elements with selectors
  agentty browser click <selector>      click an element
  agentty browser type <selector> <text>   fill an input (fires input/change)
  agentty browser press <key>           key press on the focused element (Enter, Tab, …)
  agentty browser wait <selector> [ms]  wait for an element to appear
  agentty browser eval <javascript>     run JavaScript (await allowed); prints JSON
  agentty browser console [clear]       console logs and page errors
  agentty browser screenshot [path.png] save what the page shows

Results are JSON on stdout; errors go to stderr with exit status 1.
Works in terminals opened by Agentty (uses $AGENTTY_SOCKET).";

pub fn run(args: &[String]) -> i32 {
    let Some(command) = args.first().map(String::as_str) else {
        println!("{HELP}");
        return 0;
    };
    if matches!(command, "help" | "-h" | "--help") {
        println!("{HELP}");
        return 0;
    }
    let Ok(socket) = std::env::var("AGENTTY_SOCKET") else {
        eprintln!("agentty browser: not running inside a Agentty terminal ($AGENTTY_SOCKET is not set)");
        return 1;
    };
    let mut rest: Vec<String> = args[1..].to_vec();
    if command == "screenshot" && rest.is_empty() {
        let name = format!(
            "agentty-browser-{}.png",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
        );
        rest.push(std::env::temp_dir().join(name).display().to_string());
    }
    if command == "screenshot" {
        // The app writes the file; give it an absolute path.
        let path = std::path::PathBuf::from(&rest[0]);
        let absolute = if path.is_absolute() { path } else { std::env::current_dir().unwrap_or_default().join(path) };
        rest[0] = absolute.display().to_string();
    }
    let line = match request_line(command, &rest, &socket) {
        Ok(line) => line,
        Err(err) => {
            eprintln!("agentty browser: {err}");
            return 1;
        }
    };
    let reply: serde_json::Value = serde_json::from_str(line.trim()).unwrap_or_default();
    if reply["ok"].as_bool() == Some(true) {
        match &reply["result"] {
            serde_json::Value::String(text) => println!("{text}"),
            other => println!("{other}"),
        }
        0
    } else {
        eprintln!("agentty browser: {}", reply["error"].as_str().unwrap_or("no response from Agentty"));
        1
    }
}

/// Sends one browser command over the Agentty socket and returns the raw JSON reply line.
pub fn request_line(command: &str, args: &[String], socket: &str) -> std::io::Result<String> {
    let request = serde_json::json!({
        "pane": std::env::var("AGENTTY_PANE_ID").ok().and_then(|p| p.parse::<u64>().ok()),
        "command": command,
        "args": args,
    });
    let mut stream = std::os::unix::net::UnixStream::connect(socket)?;
    writeln!(stream, "browser\t{request}")?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    Ok(line)
}
