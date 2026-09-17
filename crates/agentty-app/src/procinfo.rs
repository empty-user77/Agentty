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

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn cwd_of(_pid: u32) -> Option<PathBuf> {
    None
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_own_cwd() {
        let expected = std::env::current_dir().unwrap().canonicalize().unwrap();
        let actual = super::cwd_of(std::process::id()).unwrap().canonicalize().unwrap();
        assert_eq!(actual, expected);
    }
}

/// Process group currently in the foreground of a terminal (what the user is running).
#[cfg(unix)]
pub fn foreground_pid(tty_fd: std::os::fd::RawFd) -> Option<u32> {
    // SAFETY: tcgetpgrp only reads the terminal state of a valid descriptor; errors return -1.
    let pgid = unsafe { libc::tcgetpgrp(tty_fd) };
    (pgid > 0).then_some(pgid as u32)
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

/// Which agent CLI (by `crate::brand` id) a process is running: its executable, or for script
/// runtimes (node, bun, python) the script path in its arguments.
pub fn tool_for_process(path: &std::path::Path, args: &[String]) -> Option<&'static str> {
    let text = path.to_string_lossy();
    let name = path.file_name()?.to_string_lossy().to_string();
    if name == "claude" || text.contains("/claude/versions/") {
        return Some("claude");
    }
    if name == "codex" || (text.contains("/codex/") && name.starts_with("codex")) {
        return Some("codex");
    }
    if let Some(agent) = crate::agents::OTHER_AGENTS.iter().find(|a| a.binary == name) {
        return Some(agent.id);
    }
    if matches!(name.as_str(), "node" | "bun" | "deno" | "python" | "python3") {
        // `node /…/bin/gemini …`, `node /…/@openai/codex/bin/codex.js`
        for arg in args.iter().skip(1).take(3) {
            let script = std::path::Path::new(arg);
            let stem = script.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
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

#[cfg(not(target_os = "macos"))]
pub fn group_pids(pgid: u32) -> Vec<u32> {
    vec![pgid]
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

#[cfg(not(target_os = "macos"))]
pub fn process_args(_pid: u32) -> Vec<String> {
    Vec::new()
}

/// `argc` (i32), the executable path, NUL padding, then `argc` NUL-terminated arguments.
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

/// The agent CLI running in the foreground of a terminal, if any.
pub fn foreground_tool(tty_fd: std::os::fd::RawFd, fallback_pid: u32) -> Option<&'static str> {
    let pgid = foreground_pid(tty_fd).unwrap_or(fallback_pid);
    let mut pids = group_pids(pgid);
    if !pids.contains(&pgid) {
        pids.insert(0, pgid);
    }
    pids.into_iter().take(32).find_map(|pid| {
        let path = executable_path(pid)?;
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let args = if matches!(name.as_str(), "node" | "bun" | "deno" | "python" | "python3") { process_args(pid) } else { Vec::new() };
        tool_for_process(&path, &args)
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
        assert!(!process_args(std::process::id()).is_empty());
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

/// TCP ports listened on by each of `roots`' process trees (like cmux's sidebar ports).
pub fn listening_ports(roots: &[u32]) -> std::collections::HashMap<u32, Vec<u16>> {
    use std::collections::HashMap;
    let mut result: HashMap<u32, Vec<u16>> = HashMap::new();
    if roots.is_empty() {
        return result;
    }
    let run = |program: &str, args: &[&str]| {
        std::process::Command::new(program)
            .args(args)
            .stderr(std::process::Stdio::null())
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default()
    };
    let parents = parse_ps(&run("/bin/ps", &["-A", "-o", "pid=,ppid="]));
    let listeners = parse_lsof(&run("/usr/sbin/lsof", &["-nP", "-iTCP", "-sTCP:LISTEN", "-Fpn"]));
    for (pid, ports) in listeners {
        // Walk up to a pane's shell.
        let mut current = pid;
        for _ in 0..64 {
            if roots.contains(&current) {
                let entry = result.entry(current).or_default();
                for port in &ports {
                    if !entry.contains(port) {
                        entry.push(*port);
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
    result.values_mut().for_each(|ports| ports.sort_unstable());
    result
}

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
    fn maps_listeners_to_pane_trees() {
        let lsof = "p900\nf5\nn*:5173\nf6\nn[::1]:5173\np901\nf3\nn127.0.0.1:8080\n";
        assert_eq!(parse_lsof(lsof), vec![(900, vec![5173]), (901, vec![8080])]);
        let ps = "  900   850\n  850   800\n  901     1\n";
        let parents = parse_ps(ps);
        assert_eq!(parents.get(&900), Some(&850));
    }
}
