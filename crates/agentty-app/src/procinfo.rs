//! Process queries used to follow where the user actually is (e.g. after `cd`).

use std::path::PathBuf;

/// Current working directory of a process.
#[cfg(target_os = "macos")]
pub fn cwd_of(pid: u32) -> Option<PathBuf> {
    use std::ffi::CStr;
    use std::mem::{size_of, MaybeUninit};

    let mut info = MaybeUninit::<libc::proc_vnodepathinfo>::zeroed();
    let size = size_of::<libc::proc_vnodepathinfo>() as libc::c_int;
    // SAFETY: the buffer is exactly the size proc_pidinfo expects for PROC_PIDVNODEPATHINFO.
    let written = unsafe { libc::proc_pidinfo(pid as libc::c_int, libc::PROC_PIDVNODEPATHINFO, 0, info.as_mut_ptr().cast(), size) };
    if written != size {
        return None;
    }
    // SAFETY: proc_pidinfo filled the whole struct; vip_path is a NUL-terminated C string.
    let info = unsafe { info.assume_init() };
    let raw = &info.pvi_cdir.vip_path;
    // libc models the MAXPATHLEN buffer as [[c_char; 32]; 32]; read it as one flat byte slice.
    // SAFETY: the slice covers exactly the bytes of `raw`, which outlives it.
    let bytes: &[u8] = unsafe { std::slice::from_raw_parts(raw.as_ptr().cast(), std::mem::size_of_val(raw)) };
    let path = CStr::from_bytes_until_nul(bytes).ok()?.to_str().ok()?;
    (!path.is_empty()).then(|| PathBuf::from(path))
}

#[cfg(target_os = "linux")]
pub fn cwd_of(pid: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
}

/// Windows: the folder in the process's parameters block. PowerShell keeps its location apart
/// from that folder; the prompt Agentty installs (`shell_integration`) brings the two together.
#[cfg(windows)]
pub fn cwd_of(pid: u32) -> Option<PathBuf> {
    windows::cwd_of(pid)
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
pub fn cwd_of(_pid: u32) -> Option<PathBuf> {
    None
}

#[cfg(test)]
mod tests {
    #[test]
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn reads_own_cwd() {
        let expected = std::env::current_dir().unwrap().canonicalize().unwrap();
        let actual = super::cwd_of(std::process::id()).unwrap().canonicalize().unwrap();
        assert_eq!(actual, expected);
    }
}

/// A terminal's controller descriptor (`-1` placeholder on Windows).
#[cfg(unix)]
pub type TtyFd = std::os::fd::RawFd;
#[cfg(not(unix))]
pub type TtyFd = i32;

/// Process group currently in the foreground of a terminal (what the user is running).
#[cfg(unix)]
pub fn foreground_pid(tty_fd: TtyFd) -> Option<u32> {
    // SAFETY: tcgetpgrp only reads the terminal state of a valid descriptor; errors return -1.
    let pgid = unsafe { libc::tcgetpgrp(tty_fd) };
    (pgid > 0).then_some(pgid as u32)
}

/// The parent of a process, while it is alive.
#[cfg(target_os = "macos")]
pub fn parent_pid(pid: u32) -> Option<u32> {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: the buffer is exactly the size proc_pidinfo expects for PROC_PIDTBSDINFO.
    let written = unsafe { libc::proc_pidinfo(pid as libc::c_int, libc::PROC_PIDTBSDINFO, 0, info.as_mut_ptr().cast(), size) };
    if written != size {
        return None;
    }
    // SAFETY: proc_pidinfo filled the whole struct.
    Some(unsafe { info.assume_init() }.pbi_ppid)
}

/// Linux: the 4th field of `/proc/<pid>/stat`.
#[cfg(target_os = "linux")]
pub fn parent_pid(pid: u32) -> Option<u32> {
    stat_parent(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
}

/// Windows panes are identified by their token, not their process tree.
#[cfg(windows)]
#[allow(dead_code)]
pub fn parent_pid(pid: u32) -> Option<u32> {
    windows::processes().get(&pid).copied()
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
#[allow(dead_code)]
pub fn parent_pid(_pid: u32) -> Option<u32> {
    None
}

/// ConPTY has no foreground process group; callers fall back to the shell's pid.
#[cfg(not(unix))]
pub fn foreground_pid(_tty_fd: TtyFd) -> Option<u32> {
    None
}

/// Absolute path of a process's executable.
#[cfg(target_os = "macos")]
pub fn executable_path(pid: u32) -> Option<PathBuf> {
    let mut buffer = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: the buffer is PROC_PIDPATHINFO_MAXSIZE bytes as proc_pidpath requires.
    let len = unsafe { libc::proc_pidpath(pid as libc::c_int, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
    (len > 0).then(|| PathBuf::from(String::from_utf8_lossy(&buffer[..len as usize]).to_string()))
}

#[cfg(target_os = "linux")]
pub fn executable_path(pid: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/exe")).ok()
}

#[cfg(windows)]
pub fn executable_path(pid: u32) -> Option<PathBuf> {
    windows::executable_path(pid)
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
pub fn executable_path(_pid: u32) -> Option<PathBuf> {
    None
}

/// A program's name without the folder and without Windows' `.exe` (`claude`, `node`).
fn program_name(path: &str) -> String {
    let file = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let lower = file.to_ascii_lowercase();
    match lower.strip_suffix(".exe") {
        Some(stem) => stem.to_string(),
        None => file.to_string(),
    }
}

/// Which agent CLI (by `crate::brand` id) a process is running: its executable, or for script
/// runtimes (node, bun, python) the script path in its arguments.
pub fn tool_for_process(path: &std::path::Path, args: &[String]) -> Option<&'static str> {
    // Windows paths are compared with forward slashes, and programs without their `.exe`.
    let text = path.to_string_lossy().replace('\\', "/");
    let name = program_name(&text);
    if name.is_empty() {
        return None;
    }
    if name == "claude" || text.contains("/claude/versions/") {
        return Some("claude");
    }
    if name == "codex" || (text.contains("/codex/") && name.starts_with("codex")) {
        return Some("codex");
    }
    if let Some(agent) = crate::agents::OTHER_AGENTS.iter().find(|a| a.binary == name) {
        return Some(agent.id);
    }
    // CLIs whose process is not called like their command. Cursor's `agent` / `cursor-agent` start
    // `~/.local/share/cursor-agent/versions/<version>/…` (its own node, or a single executable);
    // xAI's `grok` / `agent` are links to `~/.grok/downloads/grok-<os>-<arch>`.
    if text.contains("/cursor-agent/versions/")
        || args.iter().skip(1).take(3).any(|a| a.replace('\\', "/").contains("/cursor-agent/versions/"))
    {
        return Some("cursor");
    }
    if name.starts_with("grok-") && text.contains("/.grok/") {
        return Some("grok");
    }
    if matches!(name.as_str(), "node" | "bun" | "deno" | "python" | "python3") {
        // `node /…/bin/gemini …`, `node /…/@openai/codex/bin/codex.js`
        for arg in args.iter().skip(1).take(3) {
            let arg = &arg.replace('\\', "/");
            let file = arg.rsplit('/').next().unwrap_or(arg);
            let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem).to_string();
            if arg.contains("@openai/codex") || stem == "codex" {
                return Some("codex");
            }
            if arg.contains("@anthropic-ai/claude-code") {
                return Some("claude");
            }
            if let Some(agent) =
                crate::agents::OTHER_AGENTS.iter().find(|a| a.binary == stem || (!a.package.is_empty() && arg.contains(a.package)))
            {
                return Some(agent.id);
            }
        }
    }
    None
}

/// Process ids in a process group.
#[cfg(target_os = "macos")]
pub fn group_pids(pgid: u32) -> Vec<u32> {
    let mut pids = vec![0 as libc::pid_t; 256];
    let bytes = (pids.len() * std::mem::size_of::<libc::pid_t>()) as libc::c_int;
    // SAFETY: the buffer holds `bytes` bytes of pid_t; the call returns the count written.
    let count = unsafe { libc::proc_listpgrppids(pgid as libc::pid_t, pids.as_mut_ptr().cast(), bytes) };
    pids.truncate(count.max(0) as usize);
    pids.into_iter().filter(|p| *p > 0).map(|p| p as u32).collect()
}

/// Linux: every process whose `/proc/<pid>/stat` names `pgid` as its group.
#[cfg(target_os = "linux")]
pub fn group_pids(pgid: u32) -> Vec<u32> {
    let Ok(entries) = std::fs::read_dir("/proc") else { return vec![pgid] };
    let mut pids: Vec<u32> = entries
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
        .filter(|pid| std::fs::read_to_string(format!("/proc/{pid}/stat")).ok().and_then(|stat| stat_group(&stat)) == Some(pgid))
        .take(256)
        .collect();
    if pids.is_empty() {
        pids.push(pgid);
    }
    pids
}

/// Parent pid (4th field) of a `/proc/<pid>/stat` line; see [`stat_group`] for the parsing.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn stat_parent(stat: &str) -> Option<u32> {
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(1)?.parse().ok()
}

/// Process group (5th field) of a `/proc/<pid>/stat` line; the command name may contain spaces
/// and parentheses, so fields are counted after its closing parenthesis.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn stat_group(stat: &str) -> Option<u32> {
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(2)?.parse().ok()
}

/// Windows has no process groups: every process below the pane's shell, nearest first.
#[cfg(windows)]
pub fn group_pids(root: u32) -> Vec<u32> {
    let mut pids = vec![root];
    pids.extend(nearest_first(root, &windows::processes()));
    pids
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
pub fn group_pids(pgid: u32) -> Vec<u32> {
    vec![pgid]
}

/// Every pid below `root`, children before grandchildren: the program the user started comes
/// before whatever it runs itself (an agent's MCP servers, a build it started).
#[cfg_attr(not(windows), allow(dead_code))]
fn nearest_first(root: u32, parents: &std::collections::HashMap<u32, u32>) -> Vec<u32> {
    let mut found = Vec::new();
    let mut level = vec![root];
    while !level.is_empty() && found.len() < 256 {
        let mut next: Vec<u32> = parents
            .iter()
            .filter(|(pid, parent)| level.contains(parent) && **pid != **parent && !found.contains(*pid) && **pid != root)
            .map(|(pid, _)| *pid)
            .collect();
        next.sort_unstable();
        found.extend(&next);
        level = next;
    }
    found
}

/// Command-line arguments of a process (`KERN_PROCARGS2`).
#[cfg(target_os = "macos")]
pub fn process_args(pid: u32) -> Vec<String> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as libc::c_int];
    let mut size: libc::size_t = 0;
    // SAFETY: querying the size first with a null buffer is the documented sysctl pattern.
    if unsafe { libc::sysctl(mib.as_mut_ptr(), 3, std::ptr::null_mut(), &mut size, std::ptr::null_mut(), 0) } != 0 || size == 0 {
        return Vec::new();
    }
    let mut buffer = vec![0u8; size];
    // SAFETY: `buffer` is `size` bytes long, as reported by the previous call.
    if unsafe { libc::sysctl(mib.as_mut_ptr(), 3, buffer.as_mut_ptr().cast(), &mut size, std::ptr::null_mut(), 0) } != 0 {
        return Vec::new();
    }
    parse_procargs(&buffer[..size.min(buffer.len())])
}

#[cfg(target_os = "linux")]
pub fn process_args(pid: u32) -> Vec<String> {
    let Ok(raw) = std::fs::read(format!("/proc/{pid}/cmdline")) else { return Vec::new() };
    raw.split(|b| *b == 0).filter(|a| !a.is_empty()).map(|a| String::from_utf8_lossy(a).to_string()).collect()
}

#[cfg(windows)]
pub fn process_args(pid: u32) -> Vec<String> {
    windows::command_line(pid).map(|line| split_command_line(&line)).unwrap_or_default()
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
pub fn process_args(_pid: u32) -> Vec<String> {
    Vec::new()
}

/// A Windows command line split into arguments the way the C runtime does: spaces separate,
/// double quotes group, and backslashes escape only the quotes they come before.
#[cfg_attr(not(windows), allow(dead_code))]
fn split_command_line(line: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut started = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                let mut slashes = 1;
                while chars.peek() == Some(&'\\') {
                    chars.next();
                    slashes += 1;
                }
                if chars.peek() == Some(&'"') {
                    current.extend(std::iter::repeat_n('\\', slashes / 2));
                    if slashes % 2 == 1 {
                        current.push('"');
                        chars.next();
                    }
                } else {
                    current.extend(std::iter::repeat_n('\\', slashes));
                }
                started = true;
            }
            '"' => {
                quoted = !quoted;
                started = true;
            }
            ' ' | '\t' if !quoted => {
                if started {
                    args.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            _ => {
                current.push(c);
                started = true;
            }
        }
    }
    if started {
        args.push(current);
    }
    args
}

/// `argc` (i32), the executable path, NUL padding, then `argc` NUL-terminated arguments.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn parse_procargs(buffer: &[u8]) -> Vec<String> {
    if buffer.len() < 4 {
        return Vec::new();
    }
    let argc = i32::from_ne_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]).max(0) as usize;
    let mut rest = &buffer[4..];
    // Skip the executable path and its padding.
    let path_end = rest.iter().position(|b| *b == 0).unwrap_or(rest.len());
    rest = &rest[path_end..];
    let start = rest.iter().position(|b| *b != 0).unwrap_or(rest.len());
    rest = &rest[start..];
    rest.split(|b| *b == 0).take(argc).map(|a| String::from_utf8_lossy(a).to_string()).collect()
}

/// The agent CLI running in the foreground of a terminal, if any, and the process that runs it.
pub fn foreground_tool(tty_fd: TtyFd, fallback_pid: u32) -> Option<(&'static str, u32)> {
    let pgid = foreground_pid(tty_fd).unwrap_or(fallback_pid);
    let mut pids = group_pids(pgid);
    if !pids.contains(&pgid) {
        pids.insert(0, pgid);
    }
    pids.into_iter().take(32).find_map(|pid| {
        let path = executable_path(pid)?;
        let name = program_name(&path.to_string_lossy());
        let args = if matches!(name.as_str(), "node" | "bun" | "deno" | "python" | "python3") { process_args(pid) } else { Vec::new() };
        tool_for_process(&path, &args).map(|tool| (tool, pid))
    })
}

/// Current git branch (or short commit when detached) for a directory, without spawning git.
pub fn git_branch(dir: &std::path::Path) -> Option<String> {
    let mut current = Some(dir);
    while let Some(path) = current {
        let dot_git = path.join(".git");
        let git_dir = if dot_git.is_dir() {
            Some(dot_git)
        } else if dot_git.is_file() {
            // Worktrees and submodules: `gitdir: <path>`.
            std::fs::read_to_string(&dot_git).ok().and_then(|s| s.strip_prefix("gitdir:").map(|p| path.join(p.trim())))
        } else {
            None
        };
        if let Some(git_dir) = git_dir {
            let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
            let head = head.trim();
            return Some(match head.strip_prefix("ref: refs/heads/") {
                Some(branch) => branch.to_string(),
                None => head.chars().take(7).collect(),
            });
        }
        current = path.parent();
    }
    None
}

#[cfg(test)]
mod agent_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn recognizes_agent_binaries() {
        assert_eq!(tool_for_process(Path::new("/Users/me/.local/share/claude/versions/2.1.273"), &[]), Some("claude"));
        assert_eq!(tool_for_process(Path::new("/opt/homebrew/bin/codex"), &[]), Some("codex"));
        assert_eq!(tool_for_process(Path::new("/bin/zsh"), &[]), None);
        let node = Path::new("/opt/homebrew/bin/node");
        assert_eq!(tool_for_process(node, &["node".into(), "/x/lib/node_modules/@openai/codex/bin/codex.js".into()]), Some("codex"));
        assert_eq!(tool_for_process(node, &["node".into(), "/x/bin/gemini".into()]), Some("gemini"));
        assert_eq!(tool_for_process(Path::new("/Users/me/.local/bin/agy"), &[]), Some("agy"));
        assert_eq!(tool_for_process(node, &["node".into(), "server.js".into()]), None);
    }

    #[test]
    fn parses_process_arguments() {
        let mut raw = 2i32.to_ne_bytes().to_vec();
        raw.extend_from_slice(b"/usr/bin/node\0\0\0node\0/x/codex.js\0PATH=/bin\0");
        assert_eq!(parse_procargs(&raw), vec!["node".to_string(), "/x/codex.js".to_string()]);
        // Our own process has at least its program name.
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        assert!(!process_args(std::process::id()).is_empty());
    }

    #[test]
    fn reads_proc_stat_groups() {
        assert_eq!(stat_group("123 (my (odd) prog) S 1 456 456 0 -1"), Some(456));
        assert_eq!(stat_parent("123 (my (odd) prog) S 77 456 456 0 -1"), Some(77));
        assert_eq!(stat_group("garbage"), None);
    }

    #[test]
    fn finds_git_branch() {
        let dir = std::env::temp_dir().join(format!("agentty-git-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::create_dir_all(dir.join("sub/deeper")).unwrap();
        std::fs::write(dir.join(".git/HEAD"), "ref: refs/heads/feature/x\n").unwrap();
        assert_eq!(git_branch(&dir.join("sub/deeper")).as_deref(), Some("feature/x"));
        std::fs::remove_dir_all(dir).ok();
    }
}

/// A process under a pane's shell that listens on a TCP port: a dev server, most of the time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Listener {
    pub pid: u32,
    pub port: u16,
}

fn run_quiet(program: &str, args: &[&str]) -> String {
    std::process::Command::new(program)
        .args(args)
        .stderr(std::process::Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default()
}

/// Every pid and its parent, from one `ps` call (empty on Windows).
pub fn process_parents() -> std::collections::HashMap<u32, u32> {
    if cfg!(windows) {
        return Default::default();
    }
    parse_ps(&run_quiet("/bin/ps", &["-A", "-o", "pid=,ppid="]))
}

/// Every process listening on a TCP port (empty on Windows: no `lsof`).
fn tcp_listeners() -> Vec<(u32, Vec<u16>)> {
    if cfg!(windows) {
        return Vec::new();
    }
    let lsof = if cfg!(target_os = "macos") { "/usr/sbin/lsof" } else { "lsof" };
    parse_lsof(&run_quiet(lsof, &["-nP", "-iTCP", "-sTCP:LISTEN", "-Fpn"]))
}

/// Listeners in each of `roots`' process trees (like cmux's sidebar ports), keyed by root.
pub fn listeners(roots: &[u32]) -> std::collections::HashMap<u32, Vec<Listener>> {
    let mut result: std::collections::HashMap<u32, Vec<Listener>> = Default::default();
    if roots.is_empty() || cfg!(windows) {
        return result; // no ps / lsof; ports are not shown on Windows
    }
    let parents = process_parents();
    for (pid, ports) in tcp_listeners() {
        // Walk up to a pane's shell.
        let mut current = pid;
        for _ in 0..64 {
            if roots.contains(&current) {
                let entry = result.entry(current).or_default();
                for port in &ports {
                    if !entry.iter().any(|l| l.port == *port) {
                        entry.push(Listener { pid, port: *port });
                    }
                }
                break;
            }
            match parents.get(&current) {
                Some(&parent) if parent > 1 && parent != current => current = parent,
                _ => break,
            }
        }
    }
    result.values_mut().for_each(|found| found.sort_unstable_by_key(|l| l.port));
    result
}

/// Every pid below `root` in `parents` (the root itself is not included).
pub fn descendants(root: u32, parents: &std::collections::HashMap<u32, u32>) -> Vec<u32> {
    let mut found = Vec::new();
    for &pid in parents.keys() {
        let mut current = pid;
        for _ in 0..64 {
            match parents.get(&current) {
                Some(&parent) if parent == root => {
                    found.push(pid);
                    break;
                }
                Some(&parent) if parent > 1 && parent != current => current = parent,
                _ => break,
            }
        }
    }
    found.sort_unstable();
    found
}

/// Asks the local servers among `candidates` — processes of a pane that is closing — to stop.
///
/// Only a process that is listening on a TCP port *right now* gets the `SIGTERM`, so a pid that was
/// recycled in the meantime, or a child that never served anything, is left alone. Returns what was
/// signalled; `kill_remaining` a few seconds later ends whatever ignored it.
pub fn terminate_listeners(candidates: &[u32]) -> Vec<Listener> {
    let stopped = serving(candidates);
    for listener in &stopped {
        signal(listener.pid, false);
    }
    stopped
}

/// `SIGKILL` for the servers of `terminate_listeners` that are still listening.
pub fn kill_remaining(stopped: &[Listener]) {
    let pids: Vec<u32> = stopped.iter().map(|l| l.pid).collect();
    for listener in serving(&pids) {
        // Still the same server (same port), not a process that took over the pid.
        if stopped.contains(&listener) {
            signal(listener.pid, true);
        }
    }
}

/// Which of `candidates` listen on a TCP port now (never this app, never pid 1).
fn serving(candidates: &[u32]) -> Vec<Listener> {
    let own = std::process::id();
    tcp_listeners()
        .into_iter()
        .filter(|(pid, _)| *pid > 1 && *pid != own && candidates.contains(pid))
        .filter_map(|(pid, ports)| Some(Listener { pid, port: *ports.first()? }))
        .collect()
}

#[cfg(unix)]
fn signal(pid: u32, force: bool) {
    unsafe {
        libc::kill(pid as libc::pid_t, if force { libc::SIGKILL } else { libc::SIGTERM });
    }
}

#[cfg(not(unix))]
fn signal(_pid: u32, _force: bool) {}

pub fn parse_ps(output: &str) -> std::collections::HashMap<u32, u32> {
    output
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
        })
        .collect()
}

/// `lsof -Fpn` output: `p<pid>` then `n<addr>:<port>` lines.
pub fn parse_lsof(output: &str) -> Vec<(u32, Vec<u16>)> {
    let mut out: Vec<(u32, Vec<u16>)> = Vec::new();
    for line in output.lines() {
        if let Some(pid) = line.strip_prefix('p').and_then(|p| p.parse().ok()) {
            out.push((pid, Vec::new()));
        } else if let (Some(addr), Some(last)) = (line.strip_prefix('n'), out.last_mut()) {
            if let Some(port) = addr.rsplit(':').next().and_then(|p| p.parse::<u16>().ok()) {
                if !last.1.contains(&port) {
                    last.1.push(port);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod port_tests {
    use super::*;

    #[test]
    fn recognises_clis_whose_process_has_another_name() {
        let path = |p: &str| std::path::PathBuf::from(p);
        // Cursor CLI: `agent` and `cursor-agent` are links into its versions folder.
        let sea = path("/Users/me/.local/share/cursor-agent/versions/2026.09.18-9a7762b/cursor-agent-sea");
        assert_eq!(tool_for_process(&sea, &[]), Some("cursor"));
        let node = path("/Users/me/.local/share/cursor-agent/versions/2026.09.18-9a7762b/node");
        let script = "/Users/me/.local/share/cursor-agent/versions/2026.09.18-9a7762b/index.js".to_string();
        assert_eq!(tool_for_process(&node, &["node".into(), script]), Some("cursor"));
        // xAI's Grok Build: `grok` and `agent` are links to a per-platform binary.
        assert_eq!(tool_for_process(&path("/Users/me/.grok/downloads/grok-macos-aarch64"), &[]), Some("grok"));
        assert_eq!(tool_for_process(&path("/Users/me/.grok/bin/grok"), &[]), Some("grok"));
        // Something else that merely starts with the same letters is not it.
        assert_eq!(tool_for_process(&path("/usr/local/bin/grok-exporter"), &[]), None);
        assert_eq!(tool_for_process(&path("/usr/bin/node"), &["node".into(), "/srv/app/index.js".into()]), None);
    }

    #[test]
    fn maps_listeners_to_pane_trees() {
        let lsof = "p900\nf5\nn*:5173\nf6\nn[::1]:5173\np901\nf3\nn127.0.0.1:8080\n";
        assert_eq!(parse_lsof(lsof), vec![(900, vec![5173]), (901, vec![8080])]);
        let ps = "  900   850\n  850   800\n  901     1\n";
        let parents = parse_ps(ps);
        assert_eq!(parents.get(&900), Some(&850));
        // Everything below a pane's shell, however deep; a process that left for pid 1 is not its.
        assert_eq!(descendants(800, &parents), vec![850, 900]);
        assert_eq!(descendants(850, &parents), vec![900]);
        assert!(descendants(900, &parents).is_empty());
    }
}

/// Process queries on Windows: the process table from a ToolHelp snapshot, the rest through the
/// process's handle. Only processes of the same user open; anything else reads as unknown.
#[cfg(windows)]
mod windows {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use windows_sys::Wdk::System::Threading::{NtQueryInformationProcess, ProcessBasicInformation, ProcessCommandLineInformation};
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_BASIC_INFORMATION, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        PROCESS_VM_READ,
    };

    struct Handle(HANDLE);

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: the handle was returned open by the call that created it and is closed once.
            unsafe { CloseHandle(self.0) };
        }
    }

    fn open(pid: u32, access: u32) -> Option<Handle> {
        // SAFETY: OpenProcess has no memory preconditions; a null handle means it failed.
        let handle = unsafe { OpenProcess(access, 0, pid) };
        (!handle.is_null()).then_some(Handle(handle))
    }

    /// Every process and its parent. Several panes ask within the same second, so one snapshot
    /// serves them all for a moment.
    pub fn processes() -> HashMap<u32, u32> {
        use std::sync::Mutex;
        use std::time::{Duration, Instant};
        static CACHE: Mutex<Option<(Instant, HashMap<u32, u32>)>> = Mutex::new(None);
        if let Ok(cache) = CACHE.lock() {
            if let Some((_, table)) = cache.as_ref().filter(|(at, _)| at.elapsed() < Duration::from_millis(500)) {
                return table.clone();
            }
        }
        let table = snapshot();
        if let Ok(mut cache) = CACHE.lock() {
            *cache = Some((Instant::now(), table.clone()));
        }
        table
    }

    fn snapshot() -> HashMap<u32, u32> {
        let mut table = HashMap::new();
        // SAFETY: a process snapshot has no preconditions; failure returns INVALID_HANDLE_VALUE.
        let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if raw == INVALID_HANDLE_VALUE {
            return table;
        }
        let snapshot = Handle(raw);
        let mut entry = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        // SAFETY: `entry` is a PROCESSENTRY32W with dwSize set, as both calls require.
        let mut more = unsafe { Process32FirstW(snapshot.0, &mut entry) } != 0;
        while more {
            table.insert(entry.th32ProcessID, entry.th32ParentProcessID);
            // SAFETY: as above.
            more = unsafe { Process32NextW(snapshot.0, &mut entry) } != 0;
        }
        table
    }

    pub fn executable_path(pid: u32) -> Option<PathBuf> {
        let process = open(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
        let mut buffer = vec![0u16; 32_768];
        let mut len = buffer.len() as u32;
        // SAFETY: `buffer` holds `len` UTF-16 units; the call writes at most that many.
        let ok = unsafe { QueryFullProcessImageNameW(process.0, PROCESS_NAME_WIN32, buffer.as_mut_ptr(), &mut len) };
        (ok != 0 && len > 0).then(|| PathBuf::from(String::from_utf16_lossy(&buffer[..len as usize])))
    }

    /// The command line as the process was given it (Windows 8.1 and later).
    pub fn command_line(pid: u32) -> Option<String> {
        let process = open(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
        let mut size = 0u32;
        // SAFETY: asking for the size with an empty buffer; the status is expected to be an error.
        unsafe { NtQueryInformationProcess(process.0, ProcessCommandLineInformation, std::ptr::null_mut(), 0, &mut size) };
        if !(16..=1 << 20).contains(&size) {
            return None;
        }
        // u64s keep the UNICODE_STRING at the front aligned.
        let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
        let bytes = (buffer.len() * 8) as u32;
        // SAFETY: `buffer` is `bytes` long, at least the size the previous call asked for.
        let status =
            unsafe { NtQueryInformationProcess(process.0, ProcessCommandLineInformation, buffer.as_mut_ptr().cast(), bytes, &mut size) };
        if status < 0 {
            return None;
        }
        // A UNICODE_STRING (length in bytes, then a pointer) whose text follows it in `buffer`.
        let length = (buffer[0] & 0xffff) as usize / 2;
        let text = buffer[1] as usize;
        let start = buffer.as_ptr() as usize;
        let end = start + buffer.len() * 8;
        if text < start || text + length * 2 > end {
            return None;
        }
        // SAFETY: the range was just checked to lie inside `buffer`, which is u16-aligned.
        let units = unsafe { std::slice::from_raw_parts(text as *const u16, length) };
        Some(String::from_utf16_lossy(units))
    }

    /// Reads a `T` from another process's memory.
    #[cfg(target_pointer_width = "64")]
    fn read<T: Copy + Default>(process: &Handle, address: usize) -> Option<T> {
        let mut value = T::default();
        let mut read = 0usize;
        let size = std::mem::size_of::<T>();
        // SAFETY: `value` is `size` bytes of plain data; the call writes at most that many.
        let ok = unsafe { ReadProcessMemory(process.0, address as *const _, (&mut value as *mut T).cast(), size, &mut read) };
        (ok != 0 && read == size).then_some(value)
    }

    /// The current folder in the process parameters (`PEB.ProcessParameters->CurrentDirectory`).
    /// Offsets are those of 64-bit Windows, the only Windows Agentty is built for.
    #[cfg(target_pointer_width = "64")]
    pub fn cwd_of(pid: u32) -> Option<PathBuf> {
        const PARAMETERS_IN_PEB: usize = 0x20;
        const CURRENT_DIRECTORY_IN_PARAMETERS: usize = 0x38;
        let process = open(pid, PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ)?;
        // SAFETY: a zeroed PROCESS_BASIC_INFORMATION is a valid value of that plain struct.
        let mut info: PROCESS_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: `info` is exactly the size ProcessBasicInformation writes.
        let status = unsafe {
            NtQueryInformationProcess(
                process.0,
                ProcessBasicInformation,
                (&mut info as *mut PROCESS_BASIC_INFORMATION).cast(),
                std::mem::size_of::<PROCESS_BASIC_INFORMATION>() as u32,
                std::ptr::null_mut(),
            )
        };
        if status < 0 || info.PebBaseAddress.is_null() {
            return None;
        }
        let parameters: usize = read(&process, info.PebBaseAddress as usize + PARAMETERS_IN_PEB)?;
        if parameters == 0 {
            return None;
        }
        // UNICODE_STRING: length in bytes (u16), capacity (u16), padding, then the text's address.
        let header: [u64; 2] = read(&process, parameters + CURRENT_DIRECTORY_IN_PARAMETERS)?;
        let length = (header[0] & 0xffff) as usize / 2;
        if length == 0 || header[1] == 0 {
            return None;
        }
        let mut units = vec![0u16; length];
        let mut read_bytes = 0usize;
        // SAFETY: `units` holds `length` u16s, the number of bytes asked for.
        let ok =
            unsafe { ReadProcessMemory(process.0, header[1] as usize as *const _, units.as_mut_ptr().cast(), length * 2, &mut read_bytes) };
        if ok == 0 || read_bytes != length * 2 {
            return None;
        }
        Some(PathBuf::from(super::trim_dir(&String::from_utf16_lossy(&units))))
    }

    #[cfg(not(target_pointer_width = "64"))]
    pub fn cwd_of(_pid: u32) -> Option<PathBuf> {
        None
    }
}

/// Windows keeps the current folder with a trailing backslash (`C:\work\`); only a drive's root
/// keeps it (`C:\`).
#[cfg_attr(not(windows), allow(dead_code))]
fn trim_dir(path: &str) -> String {
    let trimmed = path.trim_end_matches('\\');
    if trimmed.len() == 2 && trimmed.ends_with(':') {
        format!("{trimmed}\\")
    } else if trimmed.is_empty() {
        path.to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod windows_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn recognizes_windows_agent_processes() {
        let native = Path::new(r"C:\Users\me\.local\bin\claude.exe");
        assert_eq!(tool_for_process(native, &[]), Some("claude"));
        let codex = Path::new(r"C:\Users\me\AppData\Roaming\npm\node_modules\@openai\codex\vendor\x86_64-pc-windows-msvc\codex\codex.exe");
        assert_eq!(tool_for_process(codex, &[]), Some("codex"));
        let node = Path::new(r"C:\Program Files\nodejs\node.exe");
        let args = |script: &str| vec![node.display().to_string(), script.to_string()];
        assert_eq!(
            tool_for_process(node, &args(r"C:\Users\me\AppData\Roaming\npm\node_modules\@anthropic-ai\claude-code\cli.js")),
            Some("claude")
        );
        assert_eq!(
            tool_for_process(node, &args(r"C:\Users\me\AppData\Roaming\npm\node_modules\@openai\codex\bin\codex.js")),
            Some("codex")
        );
        assert_eq!(tool_for_process(node, &args(r"C:\work\server.js")), None);
        assert_eq!(tool_for_process(Path::new(r"C:\Program Files\PowerShell\7\pwsh.exe"), &[]), None);
    }

    #[test]
    fn splits_command_lines_like_the_c_runtime() {
        assert_eq!(
            split_command_line(r#""C:\Program Files\nodejs\node.exe" C:\npm\cli.js --resume "a b""#),
            [r"C:\Program Files\nodejs\node.exe", r"C:\npm\cli.js", "--resume", "a b"]
        );
        assert_eq!(split_command_line(r#"a\\\"b "c\\" d"#), [r#"a\"b"#, r"c\", "d"]);
        assert_eq!(split_command_line(r#"x "" y"#), ["x", "", "y"]);
        assert!(split_command_line("   ").is_empty());
    }

    #[test]
    fn trims_windows_folders() {
        assert_eq!(trim_dir(r"C:\work\app\"), r"C:\work\app");
        assert_eq!(trim_dir(r"C:\"), r"C:\");
        assert_eq!(trim_dir(r"\\server\share\dir\"), r"\\server\share\dir");
    }

    #[test]
    fn lists_descendants_nearest_first() {
        // shell 10 → claude 20 → (mcp 40 → 50), 30 started later by the shell.
        let parents = std::collections::HashMap::from([(10, 1), (20, 10), (30, 10), (40, 20), (50, 40), (60, 1)]);
        assert_eq!(nearest_first(10, &parents), [20, 30, 40, 50]);
        // A recycled pid that names itself as its parent does not loop.
        let looped = std::collections::HashMap::from([(10, 10), (20, 10)]);
        assert_eq!(nearest_first(10, &looped), [20]);
    }

    /// The real queries against this test process and a child it starts.
    #[test]
    #[cfg(windows)]
    fn reads_processes_on_windows() {
        let me = std::process::id();
        let expected = std::env::current_dir().unwrap().canonicalize().unwrap();
        assert_eq!(cwd_of(me).unwrap().canonicalize().unwrap(), expected);
        let exe = executable_path(me).unwrap();
        assert_eq!(exe.canonicalize().unwrap(), std::env::current_exe().unwrap().canonicalize().unwrap());
        assert!(!process_args(me).is_empty());
        let dir = std::env::temp_dir();
        let mut child =
            std::process::Command::new("cmd.exe").args(["/d", "/c", "ping -n 30 127.0.0.1 >NUL"]).current_dir(&dir).spawn().unwrap();
        let pid = child.id();
        std::thread::sleep(std::time::Duration::from_millis(600));
        let found = group_pids(me).contains(&pid);
        let parent = parent_pid(pid);
        let cwd = cwd_of(pid).and_then(|p| p.canonicalize().ok());
        let args = process_args(pid);
        let _ = child.kill();
        let _ = child.wait();
        assert!(found, "child {pid} not below {me}");
        assert_eq!(parent, Some(me));
        assert_eq!(cwd, dir.canonicalize().ok());
        assert!(args.iter().any(|a| a == "/c"), "{args:?}");
    }
}
