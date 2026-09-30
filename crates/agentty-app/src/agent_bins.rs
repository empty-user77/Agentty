//! Which copy of an agent CLI runs. A machine often has more than one `claude` or `codex` (the
//! native installer's and an older npm one, a Homebrew one…), and the one first on `PATH` need not
//! be the newest. Agentty looks for every copy where a new terminal or an installer would put it,
//! asks each for its version, and starts the newest: in the tabs it opens, and — through
//! `$AGENTTY_<NAME>_BIN`, which the shell wrappers read — for an agent typed into a pane.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The agent CLIs chosen this way.
pub const AGENTS: [&str; 2] = ["claude", "codex"];

/// One copy of an agent CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Install {
    pub path: PathBuf,
    /// The first line of `--version`, when it answered.
    pub version: Option<String>,
}

impl Install {
    fn numbers(&self) -> Option<Vec<u64>> {
        self.version.as_deref().and_then(version_numbers)
    }
}

/// The copies of `name` on `path_env` (a new terminal's `PATH` plus the installers' folders), in
/// search order, each with its version. Runs `--version` on each: off the UI thread.
pub fn installs(name: &str, path_env: &std::ffi::OsStr) -> Vec<Install> {
    agentty_bridge::process::which_all(name, path_env)
        .into_iter()
        .map(|path| {
            let version = version_of(&path, path_env);
            Install { path, version }
        })
        .collect()
}

fn version_of(path: &Path, path_env: &std::ffi::OsStr) -> Option<String> {
    let output = agentty_bridge::process::command(path)
        .arg("--version")
        .env("PATH", path_env)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let line = String::from_utf8_lossy(&output.stdout).lines().next().unwrap_or_default().trim().to_string();
    (output.status.success() && !line.is_empty()).then_some(line)
}

/// `1.2.3` out of a version line (`2.1.285 (Claude Code)`, `codex-cli 0.46.0`): the first run of
/// dot-separated numbers.
pub fn version_numbers(line: &str) -> Option<Vec<u64>> {
    line.split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .find(|word| word.contains('.') && word.split('.').all(|part| !part.is_empty()))
        .map(|word| word.split('.').filter_map(|part| part.parse().ok()).collect())
}

/// The newest of `installs` by version; the first found when none says (or they tie).
pub fn newest(installs: &[Install]) -> Option<&Install> {
    let mut best: Option<&Install> = None;
    for install in installs {
        best = match best {
            None => Some(install),
            Some(current) if install.numbers() > current.numbers() => Some(install),
            keep => keep,
        };
    }
    best
}

/// The copy chosen for each agent (only the ones found).
static CHOSEN: Mutex<Option<HashMap<String, PathBuf>>> = Mutex::new(None);

/// The last choice, kept for the tabs a new start restores before [`refresh`] has answered.
fn saved_file() -> PathBuf {
    agentty_bridge::fsutil::data_dir().join("agent-bins.json")
}

/// Takes the choice the last run saved (a small file read): call at startup, then [`refresh`] in
/// the background.
pub fn load_saved() {
    let saved: Option<HashMap<String, PathBuf>> =
        std::fs::read_to_string(saved_file()).ok().and_then(|text| serde_json::from_str(&text).ok());
    if let (Some(saved), Ok(mut slot)) = (saved, CHOSEN.lock()) {
        if slot.is_none() {
            *slot = Some(saved);
        }
    }
}

/// Looks for every agent's copies again and keeps the newest of each. Spawns login shells and
/// `--version`s: off the UI thread.
pub fn refresh() {
    let path = crate::setup_check::search_path();
    let chosen: HashMap<String, PathBuf> =
        AGENTS.iter().filter_map(|name| newest(&installs(name, &path)).map(|install| (name.to_string(), install.path.clone()))).collect();
    if let Ok(text) = serde_json::to_string(&chosen) {
        let _ = std::fs::write(saved_file(), text);
    }
    if let Ok(mut slot) = CHOSEN.lock() {
        *slot = Some(chosen);
    }
    REFRESHED.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Whether [`refresh`] has answered in this run (the saved choice may be out of date).
static REFRESHED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// [`refresh`] unless it already ran in this run. Off the UI thread.
pub fn ensure_refreshed() {
    if !REFRESHED.load(std::sync::atomic::Ordering::Relaxed) {
        refresh();
    }
}

/// The copy of `name` to start, while it is still there. `None` before the first [`refresh`], when
/// none was found, and on Windows (whose `.cmd` shims and PowerShell wrappers look programs up
/// themselves).
pub fn chosen(name: &str) -> Option<PathBuf> {
    if cfg!(windows) {
        return None;
    }
    let slot = CHOSEN.lock().ok()?;
    slot.as_ref()?.get(name).filter(|path| path.is_file()).cloned()
}

/// What a tab starts for `name`: the chosen copy's path, or the bare name for `PATH` to resolve.
pub fn program(name: &str) -> String {
    chosen(name).map(|path| path.display().to_string()).unwrap_or_else(|| name.to_string())
}

/// `$AGENTTY_<NAME>_BIN` for a pane: the copy an agent typed there starts.
pub fn pane_environment() -> Vec<(String, String)> {
    AGENTS.iter().filter_map(|name| chosen(name).map(|path| (env_name(name), path.display().to_string()))).collect()
}

pub fn env_name(name: &str) -> String {
    format!("AGENTTY_{}_BIN", name.to_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn install(path: &str, version: Option<&str>) -> Install {
        Install { path: PathBuf::from(path), version: version.map(str::to_string) }
    }

    #[test]
    fn reads_version_lines() {
        assert_eq!(version_numbers("2.1.285 (Claude Code)"), Some(vec![2, 1, 285]));
        assert_eq!(version_numbers("codex-cli 0.46.0"), Some(vec![0, 46, 0]));
        assert_eq!(version_numbers("v22.3.1"), Some(vec![22, 3, 1]));
        assert_eq!(version_numbers("no version here"), None);
    }

    #[test]
    fn picks_the_newest_copy_and_the_first_on_a_tie() {
        let list = [
            install("/opt/homebrew/bin/claude", Some("1.0.99 (Claude Code)")),
            install("/Users/me/.local/bin/claude", Some("2.1.285 (Claude Code)")),
            install("/usr/local/bin/claude", Some("2.1.9 (Claude Code)")),
        ];
        assert_eq!(newest(&list).unwrap().path, PathBuf::from("/Users/me/.local/bin/claude"));
        let tie = [install("/a/codex", Some("codex-cli 0.46.0")), install("/b/codex", Some("codex-cli 0.46.0"))];
        assert_eq!(newest(&tie).unwrap().path, PathBuf::from("/a/codex"));
        // A copy that does not answer loses to one that does, and alone it is still used.
        let silent = [install("/a/claude", None), install("/b/claude", Some("2.0.0"))];
        assert_eq!(newest(&silent).unwrap().path, PathBuf::from("/b/claude"));
        assert_eq!(newest(&[install("/a/claude", None)]).unwrap().path, PathBuf::from("/a/claude"));
        assert!(newest(&[]).is_none());
    }
}
