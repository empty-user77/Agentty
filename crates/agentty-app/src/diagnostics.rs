//! Automatic troubleshooting: the checks behind Help → Diagnose problems and the wrench on a
//! pane's bar. Each check says what it found and, where Agentty can put it right itself, carries a
//! [`Fix`] the user starts with a button. A fix keeps a copy of every file it changes.
//!
//! The checks spawn login shells and `--version`s and read the agents' settings: run them off the
//! UI thread.

use crate::agent_bins::{self, Install};
use crate::settings::CommandAlias;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Ok,
    Info,
    Warn,
    Problem,
}

/// What a button can put right.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fix {
    /// Put `dir` on `PATH` in the shell's startup file `rc`.
    AddToPath { dir: PathBuf, rc: PathBuf },
    /// Forget Claude Code's record that its fullscreen renderer failed to start.
    ClearFullscreenOff { config: PathBuf },
    /// Replace an unreadable `target` with its last good copy `backup`.
    Restore { target: PathBuf, backup: PathBuf },
    /// Turn off (comment out) line `line` (1-based) of `file`, which sets a variable that breaks agents.
    CommentOut { file: PathBuf, line: usize },
    /// Write Agentty's shell integration files again.
    RewriteShellFiles { aliases: Vec<CommandAlias>, bypass: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub level: Level,
    /// i18n key of the one-line summary; `args` fill its `{placeholders}` and the detail's.
    pub title: &'static str,
    /// i18n key of the explanation, when there is one.
    pub detail: Option<&'static str>,
    pub args: Vec<(&'static str, String)>,
    /// Facts shown as they are (paths, versions, error messages): not translated.
    pub lines: Vec<String>,
    pub fix: Option<Fix>,
}

impl Finding {
    fn new(level: Level, title: &'static str) -> Self {
        Self { level, title, detail: None, args: Vec::new(), lines: Vec::new(), fix: None }
    }
    fn detail(mut self, key: &'static str) -> Self {
        self.detail = Some(key);
        self
    }
    fn arg(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.args.push((name, value.into()));
        self
    }
    fn line(mut self, line: impl Into<String>) -> Self {
        self.lines.push(line.into());
        self
    }
    fn fix(mut self, fix: Fix) -> Self {
        self.fix = Some(fix);
        self
    }
}

/// What the checks need from the UI thread.
#[derive(Debug, Clone)]
pub struct Context {
    pub aliases: Vec<CommandAlias>,
    pub bypass: bool,
}

/// Variables that, set in a shell's startup files, make agents typed there misbehave: Claude Code
/// takes itself for a session started inside another one (no transcript, no updates), or never
/// uses its fullscreen screen.
const LEAKED_VARIABLES: [&str; 5] =
    ["CLAUDECODE", "CLAUDE_CODE_CHILD_SESSION", "CLAUDE_CODE_ENTRYPOINT", "CLAUDE_CODE_SESSION_ID", "CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN"];

/// Runs every check. Problems first, then warnings, notes and what is fine.
pub fn run(context: &Context) -> Vec<Finding> {
    let shell = crate::launch::LaunchSpec::shell_program();
    let login = login_environment(&shell);
    let search = crate::setup_check::search_path();
    let mut findings = Vec::new();
    for agent in agent_bins::AGENTS {
        findings.extend(check_agent(agent, &agent_bins::installs(agent, &search), login.as_ref(), &shell));
    }
    findings.extend(check_fullscreen());
    findings.extend(check_settings_files());
    if let Some(login) = &login {
        findings.extend(check_leaked_variables(login, &shell));
    }
    findings.push(check_shell_files(context));
    findings.sort_by_key(|finding| std::cmp::Reverse(finding.level));
    findings
}

/// The environment of a new login shell (what a new terminal gets), or `None` on Windows or when
/// the shell didn't answer.
fn login_environment(shell: &str) -> Option<HashMap<String, String>> {
    if cfg!(windows) {
        return None;
    }
    const MARKER: &str = "__agentty_diagnose_env__";
    let mut command = std::process::Command::new(shell);
    command.args(["-l", "-i", "-c", &format!("echo {MARKER}; env")]);
    // Only what the startup files set counts, not what reached this process.
    for name in LEAKED_VARIABLES {
        command.env_remove(name);
    }
    let output = command.stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output().ok()?;
    Some(parse_env(&String::from_utf8_lossy(&output.stdout), MARKER)).filter(|env| env.contains_key("PATH"))
}

/// `NAME=value` lines after `marker` (the banners and prompts of an interactive shell come before).
fn parse_env(output: &str, marker: &str) -> HashMap<String, String> {
    output
        .lines()
        .skip_while(|line| line.trim() != marker)
        .skip(1)
        .filter_map(|line| line.split_once('='))
        .filter(|(name, _)| !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

fn describe(install: &Install) -> String {
    format!("{} — {}", install.path.display(), install.version.as_deref().unwrap_or("?"))
}

fn check_agent(agent: &'static str, installs: &[Install], login: Option<&HashMap<String, String>>, shell: &str) -> Vec<Finding> {
    let name = crate::brand::brand(agent).name.to_string();
    let Some(newest) = agent_bins::newest(installs) else {
        // Codex is optional: only a missing Claude Code is worth saying.
        return if agent == "claude" {
            vec![Finding::new(Level::Warn, "diag.agent_missing").detail("diag.agent_missing_body").arg("agent", name)]
        } else {
            Vec::new()
        };
    };
    let mut findings = Vec::new();
    let first_on_login = login.and_then(|env| env.get("PATH")).and_then(|path| agentty_bridge::process::which_in(agent, path.as_ref()));
    if login.is_some() && first_on_login.is_none() {
        let mut finding = Finding::new(Level::Problem, "diag.not_on_path")
            .detail("diag.not_on_path_body")
            .arg("agent", name.clone())
            .line(describe(newest));
        if let (Some(dir), Some(rc)) = (newest.path.parent(), startup_file(shell)) {
            finding = finding.arg("rc", tilde(&rc)).fix(Fix::AddToPath { dir: dir.to_path_buf(), rc });
        }
        findings.push(finding);
    }
    if installs.len() > 1 {
        let older_first = first_on_login.as_ref().is_some_and(|first| !same_file(first, &newest.path));
        let mut finding = Finding::new(if older_first { Level::Warn } else { Level::Info }, "diag.many_copies")
            .detail(if older_first { "diag.many_copies_older_first" } else { "diag.many_copies_body" })
            .arg("agent", name.clone())
            .arg("count", installs.len().to_string())
            .arg("path", newest.path.display().to_string());
        for install in installs {
            finding = finding.line(describe(install));
        }
        findings.push(finding);
    } else if findings.is_empty() {
        findings.push(Finding::new(Level::Ok, "diag.agent_ok").arg("agent", name).line(describe(newest)));
    }
    findings
}

fn same_file(a: &Path, b: &Path) -> bool {
    let real = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    real(a) == real(b)
}

/// The startup file a new terminal of `shell` reads, where a `PATH` line belongs.
fn startup_file(shell: &str) -> Option<PathBuf> {
    let home = agentty_bridge::fsutil::home();
    match crate::shell_integration::flavor(shell) {
        crate::shell_integration::ShellFlavor::Zsh => {
            let dir = std::env::var_os("ZDOTDIR").map(PathBuf::from).unwrap_or(home);
            Some(dir.join(".zshrc"))
        }
        // macOS terminals start bash as a login shell, which reads `.bash_profile`, not `.bashrc`.
        crate::shell_integration::ShellFlavor::Bash if cfg!(target_os = "macos") => Some(home.join(".bash_profile")),
        crate::shell_integration::ShellFlavor::Bash => Some(home.join(".bashrc")),
        crate::shell_integration::ShellFlavor::Other if cfg!(windows) => None,
        crate::shell_integration::ShellFlavor::Other => Some(home.join(".profile")),
    }
}

fn tilde(path: &Path) -> String {
    let home = agentty_bridge::fsutil::home();
    match path.strip_prefix(&home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// Claude Code's own state file (`~/.claude.json`, or inside `$CLAUDE_CONFIG_DIR`).
fn claude_state_file() -> PathBuf {
    match std::env::var_os("CLAUDE_CONFIG_DIR") {
        Some(dir) => PathBuf::from(dir).join(".claude.json"),
        None => agentty_bridge::fsutil::home().join(".claude.json"),
    }
}

fn claude_dir() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| agentty_bridge::fsutil::home().join(".claude"))
}

/// The keys where Claude Code records failed fullscreen starts.
const FULLSCREEN_RECORDS: [&str; 3] = ["fullscreenAutoDisabled", "fullscreenBootStrikes", "fullscreenBootPending"];

fn check_fullscreen() -> Option<Finding> {
    if !agentty_bridge::claude::prefers_fullscreen() {
        return None;
    }
    let config = claude_state_file();
    let state: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&config).ok()?).ok()?;
    let off = state.get("fullscreenAutoDisabled").is_some_and(|v| !v.is_null())
        || state.get("fullscreenBootStrikes").is_some_and(|v| !v.is_null());
    Some(if off {
        Finding::new(Level::Warn, "diag.fullscreen_off").detail("diag.fullscreen_off_body").fix(Fix::ClearFullscreenOff { config })
    } else {
        Finding::new(Level::Ok, "diag.fullscreen_ok")
    })
}

fn check_settings_files() -> Vec<Finding> {
    let mut findings = Vec::new();
    let settings = claude_dir().join("settings.json");
    if let Some(error) = json_error(&settings) {
        let backup = settings.with_extension("json.bak");
        let mut finding = Finding::new(Level::Problem, "diag.broken_file").arg("file", tilde(&settings)).line(error);
        if json_error(&backup).is_none() && backup.is_file() {
            finding = finding.detail("diag.broken_file_backup").fix(Fix::Restore { target: settings, backup });
        } else {
            finding = finding.detail("diag.broken_file_body");
        }
        findings.push(finding);
    }
    let state = claude_state_file();
    if let Some(error) = json_error(&state) {
        let mut finding = Finding::new(Level::Problem, "diag.broken_file").arg("file", tilde(&state)).line(error);
        match newest_good_backup(&claude_dir().join("backups"), ".claude.json.backup") {
            Some(backup) => finding = finding.detail("diag.broken_file_backup").fix(Fix::Restore { target: state, backup }),
            None => finding = finding.detail("diag.broken_file_body"),
        }
        findings.push(finding);
    }
    let codex = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| agentty_bridge::fsutil::home().join(".codex"))
        .join("config.toml");
    if let Ok(text) = std::fs::read_to_string(&codex) {
        if let Err(error) = text.parse::<toml::Table>() {
            findings.push(
                Finding::new(Level::Problem, "diag.broken_file")
                    .detail("diag.broken_file_body")
                    .arg("file", tilde(&codex))
                    .line(error.to_string().trim().to_string()),
            );
        }
    }
    if findings.is_empty() {
        findings.push(Finding::new(Level::Ok, "diag.files_ok"));
    }
    findings
}

/// Why `path` is no valid JSON; `None` when it is, or when there is no such file.
fn json_error(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<serde_json::Value>(&text).err().map(|e| e.to_string())
}

/// The most recent file in `dir` whose name starts with `prefix` and that holds valid JSON.
fn newest_good_backup(dir: &Path, prefix: &str) -> Option<PathBuf> {
    let mut backups: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(prefix))
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .collect();
    backups.sort_by_key(|backup| std::cmp::Reverse(backup.0));
    backups.into_iter().map(|(_, path)| path).find(|path| json_error(path).is_none())
}

fn check_leaked_variables(login: &HashMap<String, String>, shell: &str) -> Vec<Finding> {
    let files = startup_files(shell);
    LEAKED_VARIABLES
        .iter()
        .filter(|name| login.contains_key(**name))
        .map(|name| {
            let mut finding = Finding::new(Level::Problem, "diag.leaked_variable").arg("name", *name);
            match files.iter().find_map(|file| setting_line(file, name).map(|line| (file, line))) {
                Some((file, line)) => {
                    finding = finding
                        .detail("diag.leaked_variable_body")
                        .arg("file", format!("{}:{line}", tilde(file)))
                        .fix(Fix::CommentOut { file: file.clone(), line });
                }
                None => finding = finding.detail("diag.leaked_variable_unknown"),
            }
            finding
        })
        .collect()
}

/// The startup files a shell may read, in the order it reads them.
fn startup_files(shell: &str) -> Vec<PathBuf> {
    let home = agentty_bridge::fsutil::home();
    let names: &[&str] = match crate::shell_integration::flavor(shell) {
        crate::shell_integration::ShellFlavor::Zsh => &[".zshenv", ".zprofile", ".zshrc", ".zlogin"],
        crate::shell_integration::ShellFlavor::Bash => &[".bash_profile", ".bash_login", ".profile", ".bashrc"],
        crate::shell_integration::ShellFlavor::Other => &[".profile"],
    };
    let zsh_dir = std::env::var_os("ZDOTDIR").map(PathBuf::from);
    names
        .iter()
        .map(|name| match (&zsh_dir, name.starts_with(".z")) {
            (Some(dir), true) => dir.join(name),
            _ => home.join(name),
        })
        .collect()
}

/// The 1-based number of the line of `file` that sets `name` (`export NAME=…`, `NAME=…`), if any.
fn setting_line(file: &Path, name: &str) -> Option<usize> {
    let text = std::fs::read_to_string(file).ok()?;
    text.lines().position(|line| sets_variable(line, name)).map(|index| index + 1)
}

fn sets_variable(line: &str, name: &str) -> bool {
    let line = line.trim_start();
    if line.starts_with('#') {
        return false;
    }
    let rest = line.strip_prefix("export ").map(str::trim_start).unwrap_or(line);
    rest.strip_prefix(name).is_some_and(|after| after.starts_with('='))
}

fn check_shell_files(context: &Context) -> Finding {
    let stale = crate::shell_integration::stale_files(&context.aliases, context.bypass);
    if stale.is_empty() {
        return Finding::new(Level::Ok, "diag.shell_ok");
    }
    let mut finding = Finding::new(Level::Warn, "diag.shell_stale")
        .detail("diag.shell_stale_body")
        .fix(Fix::RewriteShellFiles { aliases: context.aliases.clone(), bypass: context.bypass });
    for path in stale {
        finding = finding.line(tilde(&path));
    }
    finding
}

/// Carries out `fix`. Every file it changes is copied first (`<file>.agentty-<time>`).
pub fn apply(fix: &Fix) -> anyhow::Result<()> {
    match fix {
        Fix::AddToPath { dir, rc } => {
            let text = std::fs::read_to_string(rc).unwrap_or_default();
            keep_copy(rc)?;
            let mut out = text;
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&format!(
                "\n# Added by Agentty (Diagnose problems): agent CLIs installed here\nexport PATH=\"{}:$PATH\"\n",
                double_quoted(&dir.display().to_string())
            ));
            std::fs::write(rc, out)?;
            agent_bins::refresh();
        }
        Fix::ClearFullscreenOff { config } => {
            let text = std::fs::read_to_string(config)?;
            let mut state: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&text)?;
            for key in FULLSCREEN_RECORDS {
                state.remove(key);
            }
            keep_copy(config)?;
            replace_file(config, &serde_json::to_string_pretty(&state)?)?;
        }
        Fix::Restore { target, backup } => {
            keep_copy(target)?;
            let text = std::fs::read_to_string(backup)?;
            replace_file(target, &text)?;
        }
        Fix::CommentOut { file, line } => {
            let text = std::fs::read_to_string(file)?;
            let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
            let target = lines.get_mut(line.saturating_sub(1)).ok_or_else(|| anyhow::anyhow!("{} has no line {line}", file.display()))?;
            *target = format!("# Turned off by Agentty (Diagnose problems): {target}");
            keep_copy(file)?;
            let mut out = lines.join("\n");
            if text.ends_with('\n') {
                out.push('\n');
            }
            std::fs::write(file, out)?;
        }
        Fix::RewriteShellFiles { aliases, bypass } => crate::shell_integration::write_files(aliases, *bypass)?,
    }
    Ok(())
}

/// Copies `path` to `<path>.agentty-<unix time>` before a fix changes it (nothing to copy when it
/// does not exist yet). Never over an earlier copy: two fixes to one file in the same second
/// (Fix all) would otherwise lose the original.
fn keep_copy(path: &Path) -> anyhow::Result<()> {
    if !path.is_file() {
        return Ok(());
    }
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or_default();
    for attempt in 1..100 {
        let mut name = path.as_os_str().to_os_string();
        name.push(if attempt == 1 { format!(".agentty-{stamp}") } else { format!(".agentty-{stamp}-{attempt}") });
        let copy = PathBuf::from(name);
        if !copy.exists() {
            std::fs::copy(path, copy)?;
            return Ok(());
        }
    }
    anyhow::bail!("no free name for a copy of {}", path.display())
}

/// Writes `text` to `path` through a file beside it and a rename, keeping `path`'s permissions: a
/// reader never sees half of it.
fn replace_file(path: &Path, text: &str) -> anyhow::Result<()> {
    use std::io::Write;
    let mut name = path.as_os_str().to_os_string();
    name.push(".agentty-tmp");
    let tmp = PathBuf::from(name);
    // Private from the start: the file may hold what `path` keeps to its owner (MCP settings, …).
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(&tmp)?.write_all(text.as_bytes())?;
    if let Ok(meta) = std::fs::metadata(path) {
        std::fs::set_permissions(&tmp, meta.permissions())?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// `text` for inside double quotes in a shell script: `$`, `` ` ``, `"` and `\` taken literally.
fn double_quoted(text: &str) -> String {
    text.chars().fold(String::new(), |mut out, c| {
        if matches!(c, '$' | '`' | '"' | '\\') {
            out.push('\\');
        }
        out.push(c);
        out
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_environment_after_the_marker() {
        let out = "Welcome!\nPATH=/banner\n__m__\nPATH=/usr/bin:/bin\nCLAUDECODE=1\nnot a line\n=x\n";
        let env = parse_env(out, "__m__");
        assert_eq!(env.get("PATH").map(String::as_str), Some("/usr/bin:/bin"));
        assert_eq!(env.get("CLAUDECODE").map(String::as_str), Some("1"));
        assert_eq!(env.len(), 2);
    }

    /// A folder name with shell syntax in it goes into the startup file as plain text.
    #[test]
    #[cfg(unix)]
    fn a_path_line_expands_nothing_but_path() {
        let dir = r#"/tmp/a $(touch x) `id` "q" \b"#;
        let line = format!("export PATH=\"{}:$PATH\"; printf '%s' \"${{PATH%%:*}}\"", double_quoted(dir));
        let out = std::process::Command::new("/bin/sh").args(["-c", &line]).env("PATH", "/usr/bin:/bin").output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), dir);
    }

    #[test]
    fn finds_the_line_that_sets_a_variable() {
        assert!(sets_variable("export CLAUDECODE=1", "CLAUDECODE"));
        assert!(sets_variable("  CLAUDECODE=1", "CLAUDECODE"));
        assert!(!sets_variable("# export CLAUDECODE=1", "CLAUDECODE"));
        assert!(!sets_variable("export CLAUDECODE_X=1", "CLAUDECODE"));
        assert!(!sets_variable("echo CLAUDECODE=1", "CLAUDECODE"));
    }

    #[test]
    fn fixes_keep_a_copy_and_change_only_what_they_say() {
        let dir = std::env::temp_dir().join(format!("agentty-diagnose-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let copies = |name: &str| {
            std::fs::read_dir(&dir)
                .unwrap()
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().starts_with(&format!("{name}.agentty-")))
                .count()
        };

        let rc = dir.join("zshrc");
        std::fs::write(&rc, "alias ll='ls -l'\nexport CLAUDECODE=1\nexport EDITOR=vim").unwrap();
        apply(&Fix::CommentOut { file: rc.clone(), line: 2 }).unwrap();
        let text = std::fs::read_to_string(&rc).unwrap();
        assert_eq!(text.lines().nth(1), Some("# Turned off by Agentty (Diagnose problems): export CLAUDECODE=1"));
        assert_eq!(text.lines().count(), 3);
        assert_eq!(setting_line(&rc, "CLAUDECODE"), None);
        assert_eq!(copies("zshrc"), 1);
        // A second fix to the same file right after (Fix all) keeps its own copy: the first one, the
        // original, is still there.
        apply(&Fix::CommentOut { file: rc.clone(), line: 3 }).unwrap();
        assert_eq!(copies("zshrc"), 2);
        let first = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("zshrc.agentty-")))
            .min_by_key(|p| p.file_name().map(|n| n.len()))
            .unwrap();
        assert_eq!(std::fs::read_to_string(first).unwrap(), "alias ll='ls -l'\nexport CLAUDECODE=1\nexport EDITOR=vim");

        let config = dir.join("claude.json");
        std::fs::write(&config, r#"{"theme":"dark","fullscreenAutoDisabled":{"strikes":3},"fullscreenBootPending":{"1":{}}}"#).unwrap();
        apply(&Fix::ClearFullscreenOff { config: config.clone() }).unwrap();
        let state: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
        assert_eq!(state, serde_json::json!({ "theme": "dark" }));
        assert_eq!(copies("claude.json"), 1);

        let (broken, backup) = (dir.join("settings.json"), dir.join("settings.json.bak"));
        std::fs::write(&broken, "{ nope").unwrap();
        std::fs::write(&backup, r#"{"tui":"fullscreen"}"#).unwrap();
        apply(&Fix::Restore { target: broken.clone(), backup }).unwrap();
        assert_eq!(std::fs::read_to_string(&broken).unwrap(), r#"{"tui":"fullscreen"}"#);
        assert_eq!(copies("settings.json"), 1);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    #[cfg(unix)]
    fn an_agent_off_path_and_an_older_copy_first_are_reported() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("agentty-diagnose-agent-{}", std::process::id()));
        let (old, new) = (dir.join("old"), dir.join("new"));
        for folder in [&old, &new] {
            std::fs::create_dir_all(folder).unwrap();
            let path = folder.join("claude");
            std::fs::write(&path, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let copy = |folder: &Path, version: &str| Install { path: folder.join("claude"), version: Some(version.into()) };
        let login = |path: &str| HashMap::from([("PATH".to_string(), path.to_string())]);

        // Installed, but a new terminal's PATH does not reach it: the fix puts its folder on PATH.
        let found = check_agent("claude", &[copy(&new, "2.1.0")], Some(&login("/usr/bin:/bin")), "/bin/zsh");
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].level, found[0].title), (Level::Problem, "diag.not_on_path"));
        assert!(matches!(&found[0].fix, Some(Fix::AddToPath { dir, .. }) if *dir == new));

        // Two copies, the older one first on PATH: a warning naming the newest, which Agentty starts.
        let path = format!("{}:{}", old.display(), new.display());
        let found = check_agent("claude", &[copy(&old, "1.0.0"), copy(&new, "2.1.0")], Some(&login(&path)), "/bin/zsh");
        assert_eq!(found.len(), 1);
        assert_eq!(
            (found[0].level, found[0].title, found[0].detail),
            (Level::Warn, "diag.many_copies", Some("diag.many_copies_older_first"))
        );
        assert!(found[0].args.contains(&("path", new.join("claude").display().to_string())));

        // The newest first: only a note.
        let path = format!("{}:{}", new.display(), old.display());
        let found = check_agent("claude", &[copy(&new, "2.1.0"), copy(&old, "1.0.0")], Some(&login(&path)), "/bin/zsh");
        assert_eq!((found[0].level, found[0].detail), (Level::Info, Some("diag.many_copies_body")));
        std::fs::remove_dir_all(dir).ok();
    }
}
