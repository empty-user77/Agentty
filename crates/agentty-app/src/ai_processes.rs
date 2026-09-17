//! Local AI agent processes (Claude Code, Codex, Gemini CLI, Ollama, …) with the resources their
//! process trees use — the data behind the "AI processes" page.

use std::collections::HashMap;
use std::path::PathBuf;

/// One line of `ps`.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcRow {
    pub pid: u32,
    pub ppid: u32,
    pub cpu: f32,
    pub rss_kb: u64,
    pub state: String,
    pub elapsed: String,
    pub tty: String,
    pub command: String,
}

impl ProcRow {
    /// Program name: the executable's file name.
    pub fn name(&self) -> String {
        let program = self.command.split_whitespace().next().unwrap_or_default();
        program.rsplit('/').next().unwrap_or(program).to_string()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentProcess {
    /// Brand id (`claude`, `codex`, `ollama`, …).
    pub agent: &'static str,
    pub root: u32,
    /// The root process and all its descendants (MCP servers, shells, tools).
    pub tree: Vec<ProcRow>,
    pub cpu: f32,
    pub rss_kb: u64,
    /// Bytes read and written by the tree since the processes started.
    pub disk_read: u64,
    pub disk_write: u64,
    pub elapsed: String,
    pub tty: String,
    pub cwd: Option<PathBuf>,
    /// Stopped (`T`) or zombie root.
    pub stopped: bool,
    /// Ancestor pids of the root, nearest first (to find the terminal pane it runs in).
    pub ancestors: Vec<u32>,
}

/// Parses `ps -axww -o pid=,ppid=,pcpu=,rss=,state=,etime=,tty=,command=`.
pub fn parse_ps_rows(output: &str) -> Vec<ProcRow> {
    output
        .lines()
        .filter_map(|line| {
            let mut rest = line.trim_start();
            let mut field = || {
                let end = rest.find(char::is_whitespace)?;
                let value = &rest[..end];
                rest = rest[end..].trim_start();
                Some(value.to_string())
            };
            let pid = field()?.parse().ok()?;
            let ppid = field()?.parse().ok()?;
            let cpu = field()?.parse().ok()?;
            let rss_kb = field()?.parse().ok()?;
            let state = field()?;
            let elapsed = field()?;
            let tty = field()?;
            let command = rest.trim_end().to_string();
            (!command.is_empty()).then_some(ProcRow { pid, ppid, cpu, rss_kb, state, elapsed, tty, command })
        })
        .collect()
}

/// Quick filter before the (syscall-backed) exact check.
fn maybe_agent(command: &str) -> bool {
    let lower = command.to_lowercase();
    ["claude", "codex", "ollama", "@google/gemini-cli", "opencode", "aider"].iter().any(|k| lower.contains(k))
        || crate::agents::OTHER_AGENTS
            .iter()
            .any(|a| lower.split_whitespace().take(3).any(|word| word.rsplit('/').next() == Some(a.binary)))
}

/// Which agent a process is, from its executable and arguments.
fn classify(row: &ProcRow) -> Option<&'static str> {
    if !maybe_agent(&row.command) {
        return None;
    }
    let path = crate::procinfo::executable_path(row.pid)?;
    if path.file_name().is_some_and(|n| n == "ollama") {
        return Some("ollama");
    }
    let args = crate::procinfo::process_args(row.pid);
    crate::procinfo::tool_for_process(&path, &args)
}

/// Groups `rows` into agent process trees, given which pids are agents.
pub fn group(rows: &[ProcRow], agents: &HashMap<u32, &'static str>) -> Vec<AgentProcess> {
    let by_pid: HashMap<u32, &ProcRow> = rows.iter().map(|r| (r.pid, r)).collect();
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for row in rows {
        children.entry(row.ppid).or_default().push(row.pid);
    }
    let ancestors = |pid: u32| {
        let mut chain = Vec::new();
        let mut current = by_pid.get(&pid).map(|r| r.ppid);
        while let Some(parent) = current.filter(|p| *p > 1 && !chain.contains(p) && chain.len() < 64) {
            chain.push(parent);
            current = by_pid.get(&parent).map(|r| r.ppid);
        }
        chain
    };
    let mut groups: Vec<AgentProcess> = rows
        .iter()
        .filter_map(|row| {
            let agent = *agents.get(&row.pid)?;
            let chain = ancestors(row.pid);
            // A wrapper (e.g. `node …/claude-code` under `claude`) belongs to its outer agent.
            if chain.iter().any(|p| agents.contains_key(p)) {
                return None;
            }
            let mut tree = Vec::new();
            let mut stack = vec![row.pid];
            while let Some(pid) = stack.pop() {
                if let Some(proc_row) = by_pid.get(&pid) {
                    tree.push((*proc_row).clone());
                }
                stack.extend(children.get(&pid).into_iter().flatten().copied());
            }
            Some(AgentProcess {
                agent,
                root: row.pid,
                cpu: tree.iter().map(|r| r.cpu).sum(),
                rss_kb: tree.iter().map(|r| r.rss_kb).sum(),
                disk_read: 0,
                disk_write: 0,
                elapsed: row.elapsed.clone(),
                tty: row.tty.clone(),
                cwd: None,
                stopped: row.state.starts_with('T') || row.state.starts_with('Z'),
                ancestors: chain,
                tree,
            })
        })
        .collect();
    groups.sort_by(|a, b| b.cpu.total_cmp(&a.cpu).then(b.rss_kb.cmp(&a.rss_kb)));
    groups
}

/// Bytes read and written by a process so far.
#[cfg(target_os = "macos")]
fn disk_io(pid: u32) -> (u64, u64) {
    let mut info: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
    // SAFETY: `info` is a rusage_info_v4, matching the RUSAGE_INFO_V4 flavor.
    let result =
        unsafe { libc::proc_pid_rusage(pid as libc::c_int, libc::RUSAGE_INFO_V4, (&mut info as *mut libc::rusage_info_v4).cast()) };
    if result == 0 {
        (info.ri_diskio_bytesread, info.ri_diskio_byteswritten)
    } else {
        (0, 0)
    }
}

#[cfg(not(target_os = "macos"))]
fn disk_io(_pid: u32) -> (u64, u64) {
    (0, 0)
}

/// Every local AI agent process tree, busiest first.
pub fn collect() -> Vec<AgentProcess> {
    let output = std::process::Command::new("/bin/ps")
        .args(["-axww", "-o", "pid=,ppid=,pcpu=,rss=,state=,etime=,tty=,command="])
        .stderr(std::process::Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    let own = std::process::id();
    let rows: Vec<ProcRow> = parse_ps_rows(&output).into_iter().filter(|r| r.pid != own).collect();
    let agents: HashMap<u32, &'static str> = rows.iter().filter_map(|r| Some((r.pid, classify(r)?))).collect();
    let mut groups = group(&rows, &agents);
    for process in &mut groups {
        for row in &process.tree {
            let (read, write) = disk_io(row.pid);
            process.disk_read += read;
            process.disk_write += write;
        }
        process.cwd = crate::procinfo::cwd_of(process.root);
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    const PS: &str = "\
    1     0   0.0  12000 Ss   10-01:00:00 ??       /sbin/launchd
  500     1   0.1   4000 S       01:00:00 ttys001  -zsh
  501   500  12.5 350000 S+      00:42:10 ttys001  claude --model opus
  502   501   0.0  80000 S+      00:42:09 ttys001  node /usr/local/lib/node_modules/example-mcp/index.js
  503   502   1.5  20000 S+         00:05 ttys001  rg foo
  600     1   0.0  90000 T       02:00:00 ??       /opt/homebrew/bin/codex exec
  700     1   0.0   1000 S       00:10:00 ttys002  vim notes.md
";

    #[test]
    fn parses_ps_rows() {
        let rows = parse_ps_rows(PS);
        assert_eq!(rows.len(), 7);
        let claude = &rows[2];
        assert_eq!((claude.pid, claude.ppid, claude.rss_kb), (501, 500, 350_000));
        assert_eq!(claude.cpu, 12.5);
        assert_eq!((claude.state.as_str(), claude.elapsed.as_str(), claude.tty.as_str()), ("S+", "00:42:10", "ttys001"));
        assert_eq!(claude.command, "claude --model opus");
        assert_eq!(rows[3].name(), "node");
    }

    #[test]
    fn groups_agent_trees() {
        let rows = parse_ps_rows(PS);
        let agents = HashMap::from([(501, "claude"), (600, "codex")]);
        let groups = group(&rows, &agents);
        assert_eq!(groups.len(), 2);
        let claude = &groups[0];
        assert_eq!((claude.agent, claude.root, claude.tree.len()), ("claude", 501, 3));
        assert!((claude.cpu - 14.0).abs() < 0.01);
        assert_eq!(claude.rss_kb, 450_000);
        assert_eq!(claude.ancestors, vec![500]);
        assert!(!claude.stopped);
        assert!(groups[1].stopped);
        // A nested agent process is part of its parent's tree, not a group of its own.
        let nested = HashMap::from([(501, "claude"), (502, "codex")]);
        assert_eq!(group(&rows, &nested).len(), 1);
    }
}
