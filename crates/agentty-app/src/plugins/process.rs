//! One running plugin: its process, a writer thread for stdin and reader threads for stdout
//! (protocol messages) and stderr (log lines).

use agentty_bridge::plugins::manifest::Runtime;
use agentty_bridge::plugins::store::{plugin_data_dir, InstalledPlugin};
use agentty_bridge::plugins::Incoming;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};

/// Longest protocol line accepted from a plugin.
const MAX_LINE_BYTES: usize = 16 * 1024 * 1024;
/// Messages a second at which the reader stops reading at all. The window checks the same thing
/// once messages reach it, but only as it drains them on the main thread: a plugin can queue far
/// more than that in the meantime, so the flood has to be stopped where it is read.
const READ_CEILING_PER_SECOND: u32 = 480;

#[derive(Debug)]
pub enum ProcessEvent {
    Started,
    Failed(String),
    Message(Incoming),
    Log(String),
    Exited(Option<i32>),
}

pub struct PluginProcess {
    writer: mpsc::Sender<String>,
    pid: std::sync::Arc<Mutex<Option<u32>>>,
}

impl PluginProcess {
    /// Starts the plugin in the background; `events` receives everything it does. Lines written
    /// before the process is up are delivered once it is.
    pub fn start(plugin: &InstalledPlugin, language: &str, events: impl Fn(ProcessEvent) + Send + Sync + 'static) -> Self {
        let (writer, lines) = mpsc::channel::<String>();
        let pid = std::sync::Arc::new(Mutex::new(None));
        let plugin = plugin.clone();
        let language = language.to_string();
        let pid_slot = pid.clone();
        let spawn = std::thread::Builder::new().name(format!("plugin-{}", plugin.id)).spawn(move || {
            let events = std::sync::Arc::new(events);
            let mut child = match command(&plugin, &language).and_then(|mut c| c.spawn().map_err(|e| format!("could not start: {e}"))) {
                Ok(child) => child,
                Err(err) => return events(ProcessEvent::Failed(err)),
            };
            if let Ok(mut slot) = pid_slot.lock() {
                *slot = Some(child.id());
            }
            events(ProcessEvent::Started);
            if let Some(mut stdin) = child.stdin.take() {
                std::thread::spawn(move || {
                    for line in lines {
                        if stdin.write_all(line.as_bytes()).and_then(|_| stdin.write_all(b"\n")).and_then(|_| stdin.flush()).is_err() {
                            break;
                        }
                    }
                });
            }
            if let Some(stderr) = child.stderr.take() {
                let events = events.clone();
                // Capped like stdout: a plugin writing endlessly without a newline must not
                // grow this buffer until Agentty runs out of memory.
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(stderr);
                    let mut line = Vec::new();
                    loop {
                        line.clear();
                        let over_limit = match read_line_limited(&mut reader, &mut line) {
                            Ok(0) => break,
                            Ok(_) => false,
                            Err(_) => true,
                        };
                        let text = String::from_utf8_lossy(&line).trim_end().to_string();
                        events(ProcessEvent::Log(if over_limit { format!("{text} …(cut)") } else { text }));
                        if over_limit {
                            break;
                        }
                    }
                });
            }
            if let Some(stdout) = child.stdout.take() {
                let events = events.clone();
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(stdout);
                    let mut line = Vec::new();
                    let mut window = (std::time::Instant::now(), 0u32);
                    loop {
                        line.clear();
                        match read_line_limited(&mut reader, &mut line) {
                            Ok(0) => break,
                            Ok(_) => {
                                if window.0.elapsed() >= std::time::Duration::from_secs(1) {
                                    window = (std::time::Instant::now(), 0);
                                }
                                window.1 += 1;
                                if window.1 > READ_CEILING_PER_SECOND {
                                    // Closing stdout here is the backpressure: nothing more is
                                    // parsed or queued, and the plugin's next write fails.
                                    events(ProcessEvent::Failed(format!("sent more than {READ_CEILING_PER_SECOND} messages a second")));
                                    break;
                                }
                                let text = String::from_utf8_lossy(&line);
                                match Incoming::parse(&text) {
                                    Some(message) => events(ProcessEvent::Message(message)),
                                    None if text.trim().is_empty() => {}
                                    // Stray output (console.log): keep it visible in the log.
                                    None => events(ProcessEvent::Log(format!("stdout: {}", text.trim_end()))),
                                }
                            }
                            Err(err) => {
                                events(ProcessEvent::Log(format!("agentty: {err}")));
                                break;
                            }
                        }
                    }
                });
            }
            let code = child.wait().ok().and_then(|status| status.code());
            if let Ok(mut slot) = pid_slot.lock() {
                *slot = None;
            }
            events(ProcessEvent::Exited(code));
        });
        if let Err(err) = spawn {
            eprintln!("agentty: could not start plugin thread: {err}");
        }
        Self { writer, pid }
    }

    pub fn send(&self, line: String) {
        let _ = self.writer.send(line);
    }

    /// Terminates the plugin before Agentty exits, without waiting for it to agree.
    pub fn kill(&self) {
        self.signal(false);
        // Agentty is going away; a plugin that ignores SIGTERM must not outlive it.
        std::thread::sleep(std::time::Duration::from_millis(150));
        self.signal(true);
    }

    /// Asks the plugin to exit, then terminates it, then makes sure it is gone.
    pub fn stop(&self) {
        self.send(agentty_bridge::plugins::notification("shutdown", serde_json::json!({})));
        let pid = self.pid.clone();
        std::thread::spawn(move || {
            for (wait, force) in [(1500, false), (1500, true)] {
                std::thread::sleep(std::time::Duration::from_millis(wait));
                match pid.lock().ok().and_then(|p| *p) {
                    Some(pid) => terminate(pid, force),
                    // It exited on its own.
                    None => break,
                }
            }
        });
    }

    fn signal(&self, force: bool) {
        if let Some(pid) = self.pid.lock().ok().and_then(|p| *p) {
            terminate(pid, force);
        }
    }
}

/// SIGTERM (or SIGKILL when `force`) to the plugin's own child process.
#[cfg(unix)]
fn terminate(pid: u32, force: bool) {
    // SAFETY: plain kill(2) on the plugin's own child process id.
    unsafe {
        libc::kill(pid as libc::pid_t, if force { libc::SIGKILL } else { libc::SIGTERM });
    }
}

/// Windows has no SIGTERM for console programs; the plugin's process tree is ended.
#[cfg(windows)]
fn terminate(pid: u32, _force: bool) {
    let _ = agentty_bridge::process::command("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Reads one `\n`-terminated line, refusing lines longer than [`MAX_LINE_BYTES`].
fn read_line_limited(reader: &mut impl BufRead, out: &mut Vec<u8>) -> std::io::Result<usize> {
    let mut total = 0;
    loop {
        let (done, used) = {
            let available = reader.fill_buf()?;
            if available.is_empty() {
                return Ok(total);
            }
            match available.iter().position(|b| *b == b'\n') {
                Some(index) => {
                    out.extend_from_slice(&available[..index]);
                    (true, index + 1)
                }
                None => {
                    out.extend_from_slice(available);
                    (false, available.len())
                }
            }
        };
        reader.consume(used);
        total += used;
        if out.len() > MAX_LINE_BYTES {
            return Err(std::io::Error::other("plugin sent a message larger than 16 MB"));
        }
        if done {
            return Ok(total);
        }
    }
}

fn command(plugin: &InstalledPlugin, language: &str) -> Result<Command, String> {
    let manifest = plugin.manifest.as_ref().ok_or_else(|| plugin.error.clone().unwrap_or_else(|| "invalid plugin".into()))?;
    let entry = manifest.entry(&plugin.dir).map_err(|e| format!("{e:#}"))?;
    let mut path_env = login_path();
    let mut command = match manifest.runtime {
        Runtime::Node => {
            let node = find_program("node", &path_env).ok_or("Node.js was not found. Install Node.js 18 or newer (https://nodejs.org).")?;
            // npm and the CLIs it installs start with `#!/usr/bin/env node`: the Node.js that runs
            // the plugin has to be on its PATH as well, wherever it was found.
            path_env = with_dir_first(&path_env, node.parent());
            let mut command = Command::new(node);
            command.arg(&entry);
            command
        }
        Runtime::Python => {
            let python = find_program("python3", &path_env).ok_or("python3 was not found.")?;
            let mut command = Command::new(python);
            command.arg(&entry);
            command
        }
        Runtime::Executable => Command::new(&entry),
    };
    agentty_bridge::process::hide_window(&mut command);
    let data = plugin_data_dir(&plugin.id);
    let _ = std::fs::create_dir_all(&data);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o700));
    }
    command
        .current_dir(&plugin.dir)
        .env("PATH", &path_env)
        .env("AGENTTY_PLUGIN_ID", &plugin.id)
        .env("AGENTTY_PLUGIN_DIR", &plugin.dir)
        .env("AGENTTY_PLUGIN_DATA", &data)
        .env("AGENTTY_VERSION", env!("CARGO_PKG_VERSION"))
        .env("AGENTTY_LANGUAGE", language)
        .env("AGENTTY_BIN", std::env::current_exe().unwrap_or_else(|_| PathBuf::from("agentty")))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    Ok(command)
}

/// Printed on its own line before the PATH, to find it among whatever else the shell prints.
const PATH_MARKER: &str = "__agentty_login_path__";

/// PATH of the user's login shell (nvm, Homebrew, …), looked up once.
fn login_path() -> String {
    static PATH: OnceLock<String> = OnceLock::new();
    PATH.get_or_init(|| {
        let fallback = std::env::var("PATH").unwrap_or_default();
        if cfg!(windows) {
            // No login shell on Windows: the PATH a new login would get (see `current_path`).
            return agentty_bridge::process::current_path().to_string_lossy().into_owned();
        }
        login_shell_path().map(|login| if fallback.is_empty() { login.clone() } else { format!("{login}:{fallback}") }).unwrap_or(fallback)
    })
    .clone()
}

/// PATH of a fresh login shell, asked now (not cached): picks up what an installer just added to
/// `.zshrc` / `.profile`. Unix only; `None` when the shell didn't say.
pub(crate) fn login_shell_path() -> Option<String> {
    if cfg!(windows) {
        return None;
    }
    // Interactive as well as login: `.zshrc` is where many installs put themselves on PATH.
    // `printenv` rather than `$PATH`: it prints the colon-separated form in every shell.
    Command::new(crate::launch::LaunchSpec::shell_program())
        .args(["-l", "-i", "-c", &format!("echo {PATH_MARKER}; printenv PATH")])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .and_then(|out| parse_login_path(&String::from_utf8_lossy(&out.stdout)))
}

/// The PATH line among the banners and prompts of an interactive shell. It is found by the marker
/// before it, not by its looks: entries may contain spaces ("…/Application Support/…"), and a
/// long PATH must arrive whole.
fn parse_login_path(output: &str) -> Option<String> {
    let mut lines = output.lines().skip_while(|line| line.trim() != PATH_MARKER);
    lines.nth(1).map(|path| path.trim().to_string()).filter(|path| path.contains('/'))
}

/// `path_env` with `dir` in front, unless it is on it already.
fn with_dir_first(path_env: &str, dir: Option<&Path>) -> String {
    let Some(dir) = dir.filter(|dir| !dir.as_os_str().is_empty()) else { return path_env.to_string() };
    if std::env::split_paths(path_env).any(|entry| entry == dir) {
        return path_env.to_string();
    }
    // The platform's separator (`:`, or `;` on Windows), which `split_paths` above reads too.
    let entries = std::iter::once(dir.to_path_buf()).chain(std::env::split_paths(path_env).filter(|p| !p.as_os_str().is_empty()));
    std::env::join_paths(entries).map(|joined| joined.to_string_lossy().into_owned()).unwrap_or_else(|_| path_env.to_string())
}

/// `name` on `path_env`, then in common install locations (newest nvm Node first).
fn find_program(name: &str, path_env: &str) -> Option<PathBuf> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<PathBuf>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(found) = cache.lock().ok().and_then(|c| c.get(name).cloned()) {
        return found;
    }
    let home = crate::launch::home_dir();
    if cfg!(windows) {
        let found = agentty_bridge::process::which_in(if name == "python3" { "python" } else { name }, std::ffi::OsStr::new(path_env));
        if let Ok(mut cache) = cache.lock() {
            cache.insert(name.to_string(), found.clone());
        }
        return found;
    }
    let mut candidates: Vec<PathBuf> = std::env::split_paths(path_env).map(|dir| dir.join(name)).collect();
    for dir in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"] {
        candidates.push(Path::new(dir).join(name));
    }
    candidates.push(home.join(".volta/bin").join(name));
    if name == "node" {
        let mut versions: Vec<PathBuf> =
            std::fs::read_dir(home.join(".nvm/versions/node")).map(|e| e.flatten().map(|e| e.path()).collect()).unwrap_or_default();
        versions.sort_by_key(|p| {
            let name = p.file_name().map(|n| n.to_string_lossy().trim_start_matches('v').to_string()).unwrap_or_default();
            name.split('.').map(|part| part.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>()
        });
        candidates.extend(versions.into_iter().rev().map(|v| v.join("bin/node")));
    }
    let found = candidates.into_iter().find(|p| is_executable(p));
    if let Ok(mut cache) = cache.lock() {
        cache.insert(name.to_string(), found.clone());
    }
    found
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_path_survives_spaces_and_shell_noise() {
        let path = "/Users/me/.nvm/versions/node/v22.0.0/bin:/Users/me/Library/Application Support/JetBrains/Toolbox/scripts:/usr/bin";
        let output = format!("Welcome back!\n{PATH_MARKER}\n{path}\n");
        assert_eq!(parse_login_path(&output).as_deref(), Some(path));
        assert_eq!(parse_login_path("no marker here\n/usr/bin\n"), None);
        assert_eq!(parse_login_path(&format!("{PATH_MARKER}\n")), None);
    }

    #[test]
    fn the_runtime_folder_leads_the_path_once() {
        // Joined with the platform's separator (`;` on Windows).
        let join = |parts: &[&str]| std::env::join_paths(parts).unwrap().to_string_lossy().into_owned();
        let node_dir = Path::new("/Users/me/.nvm/versions/node/v22.0.0/bin");
        let node = "/Users/me/.nvm/versions/node/v22.0.0/bin";
        assert_eq!(with_dir_first(&join(&["/usr/bin", "/bin"]), Some(node_dir)), join(&[node, "/usr/bin", "/bin"]));
        let already = join(&["/usr/bin", node]);
        assert_eq!(with_dir_first(&already, Some(node_dir)), already);
        assert_eq!(with_dir_first("", Some(node_dir)), node);
        assert_eq!(with_dir_first("/usr/bin", None), "/usr/bin");
    }

    #[test]
    fn reads_lines_with_a_limit() {
        let mut reader = BufReader::new(&b"{\"a\":1}\nsecond"[..]);
        let mut line = Vec::new();
        assert_eq!(read_line_limited(&mut reader, &mut line).unwrap(), 8);
        assert_eq!(line, b"{\"a\":1}");
        line.clear();
        read_line_limited(&mut reader, &mut line).unwrap();
        assert_eq!(line, b"second");
        line.clear();
        assert_eq!(read_line_limited(&mut reader, &mut line).unwrap(), 0);

        let huge = vec![b'x'; MAX_LINE_BYTES + 10];
        let mut reader = BufReader::new(&huge[..]);
        assert!(read_line_limited(&mut reader, &mut Vec::new()).is_err());
    }

    /// Runs the real Node SDK against a tiny plugin when Node.js is installed.
    #[test]
    fn talks_to_a_node_plugin() {
        let Some(_) = find_program("node", &std::env::var("PATH").unwrap_or_default()) else { return };
        let dir = std::env::temp_dir().join(format!("agentty-plugin-process-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("agentty-plugin.mjs"), agentty_bridge::plugins::store::NODE_SDK).unwrap();
        std::fs::write(
            dir.join("main.mjs"),
            "import { createPlugin, ui } from './agentty-plugin.mjs';\n\
             const plugin = createPlugin();\n\
             plugin.command('t.ping', async ({ context }) => { await plugin.setPanel(ui.column([ui.text(`pong ${context.pane.id}`)])); });\n\
             plugin.start();\n",
        )
        .unwrap();
        std::fs::write(dir.join("agentty-plugin.json"), r#"{"id":"proc-test","name":"T","version":"1.0.0","main":"main.mjs"}"#).unwrap();
        let manifest = agentty_bridge::plugins::manifest::Manifest::load(&dir).unwrap();
        let plugin = InstalledPlugin {
            id: "proc-test".into(),
            dir: dir.clone(),
            manifest: Some(manifest),
            error: None,
            enabled: true,
            source: agentty_bridge::plugins::store::Source::Dev,
        };
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let process = PluginProcess::start(&plugin, "en", move |event| {
            let _ = tx.lock().unwrap().send(event);
        });
        process.send(agentty_bridge::plugins::request(1, "initialize", serde_json::json!({ "plugin": { "id": "proc-test" } })));
        process.send(agentty_bridge::plugins::notification(
            "command/execute",
            serde_json::json!({ "command": "t.ping", "context": { "pane": { "id": 7 } } }),
        ));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let mut panel = None;
        while std::time::Instant::now() < deadline && panel.is_none() {
            match rx.recv_timeout(std::time::Duration::from_secs(20)) {
                Ok(ProcessEvent::Message(Incoming::Request { id, method, params })) if method == "ui/setPanel" => {
                    process.send(agentty_bridge::plugins::response(&id, Ok(serde_json::Value::Null)));
                    panel = Some(params);
                }
                Ok(ProcessEvent::Failed(err)) => panic!("{err}"),
                Ok(_) => {}
                Err(_) => break,
            }
        }
        process.stop();
        let panel = panel.expect("the plugin rendered a panel");
        assert_eq!(panel["tree"]["children"][0]["text"], "pong 7");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir(plugin_data_dir("proc-test"));
        let _ = std::fs::remove_dir(plugin_data_dir("proc-test").parent().unwrap_or(&dir));
    }
}
