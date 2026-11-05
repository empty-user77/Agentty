//! Windows: `agentty.com`, installed next to `agentty.exe`.
//!
//! `agentty.exe` is a GUI-subsystem program, or every hook and status line an agent runs would open
//! a console window. But PowerShell and cmd neither wait for such a program nor read its exit code,
//! so `agentty browser wait-load; agentty browser text h1` raced, and `$x = agentty …` lost the
//! output. Both look for `.com` before `.exe`, so a bare `agentty` in a terminal runs this console
//! program: it runs `agentty.exe` with its output through pipes, passes the output on, and ends
//! with its exit code. Starting the app itself (no command, a folder, a link) doesn't wait.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// What `agentty.exe` answers without starting the app (`single_instance::CLI_COMMANDS`, which a
/// test there compares with this list, plus help and version).
const COMMANDS: &[&str] = &[
    "mcp-connector",
    "worktree-guard",
    "statusline",
    "mcp-browser",
    "mcp-plugins",
    "browser",
    "signal",
    "tasks",
    "plan",
    "db",
    "worktree-for",
    "notify",
    "help",
    "--help",
    "-h",
    "version",
    "--version",
    "-V",
];

/// How long output may still arrive after `agentty.exe` ended. A program it left running may hold
/// the pipe open for as long as it runs; that must not keep the terminal waiting.
const DRAIN: Duration = Duration::from_secs(1);

/// Whether these arguments ask for a command to wait for, rather than for the app to open.
fn waits(args: &[String]) -> bool {
    args.first().is_some_and(|first| COMMANDS.contains(&first.as_str()))
}

/// Copies all of `from` to `to`. When `to` goes away (`| Select-Object -First 1`) it goes on
/// reading, so `agentty.exe` doesn't stop on a pipe nobody reads.
fn relay(mut from: impl Read, mut to: impl Write) {
    let mut buffer = [0u8; 8192];
    let mut open = true;
    while let Ok(read) = from.read(&mut buffer) {
        if read == 0 {
            break;
        }
        open = open && to.write_all(&buffer[..read]).and_then(|()| to.flush()).is_ok();
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let name = if cfg!(windows) { "agentty.exe" } else { "agentty" };
    let exe = match std::env::current_exe() {
        Ok(path) => path.with_file_name(name),
        Err(err) => {
            eprintln!("agentty: {err}");
            std::process::exit(1);
        }
    };
    let mut command = Command::new(&exe);
    command.args(&args);
    if !waits(&args) {
        // The app runs on after this program: it holds on to nothing of the terminal's.
        command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        if let Err(err) = command.spawn() {
            eprintln!("agentty: {}: {err}", exe.display());
            std::process::exit(1);
        }
        return;
    }
    let mut child = match command.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
        Ok(child) => child,
        Err(err) => {
            eprintln!("agentty: {}: {err}", exe.display());
            std::process::exit(1);
        }
    };
    let (done, finished) = mpsc::channel();
    let mut relays = 0;
    if let Some(out) = child.stdout.take() {
        let done = done.clone();
        relays += 1;
        std::thread::spawn(move || {
            relay(out, std::io::stdout());
            let _ = done.send(());
        });
    }
    if let Some(err) = child.stderr.take() {
        relays += 1;
        std::thread::spawn(move || {
            relay(err, std::io::stderr());
            let _ = done.send(());
        });
    }
    let code = child.wait().ok().and_then(|status| status.code()).unwrap_or(1);
    for _ in 0..relays {
        if finished.recv_timeout(DRAIN).is_err() {
            break;
        }
    }
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| arg.to_string()).collect()
    }

    #[test]
    fn only_commands_are_waited_for() {
        assert!(waits(&strings(&["browser", "text", "h1"])));
        assert!(waits(&strings(&["--version"])));
        assert!(waits(&strings(&["tasks", "--plan", "plan.json"])));
        assert!(!waits(&[]), "no arguments open the app");
        assert!(!waits(&strings(&[r"C:\code\project"])), "a folder opens in the app");
        assert!(!waits(&strings(&["agentty://open"])), "a link opens in the app");
    }

    #[test]
    fn relay_copies_everything_and_reads_on_after_the_reader_left() {
        let mut copied = Vec::new();
        relay(&b"hello"[..], &mut copied);
        assert_eq!(copied, b"hello");

        struct Gone;
        impl Write for Gone {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut source = std::io::Cursor::new(vec![0u8; 20_000]);
        relay(&mut source, Gone);
        assert_eq!(source.position(), 20_000);
    }
}
