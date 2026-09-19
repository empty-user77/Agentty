//! Shell integration for panes Agentty starts: installs "reserved word" command aliases, e.g.
//! `zzzz = --dangerously-skip-permissions` for `claude`, so `claude zzzz` runs
//! `claude --dangerously-skip-permissions`.
//!
//! The user's startup files are never edited. For zsh, `ZDOTDIR` points at a generated directory
//! whose `.zshenv` restores the user's `ZDOTDIR`, sources their `.zshenv`, and registers a
//! `precmd` hook that (re)loads the alias functions after `.zshrc` has run. For bash, a generated
//! `--rcfile` sources the usual profile files first. On Windows, PowerShell panes dot-source a
//! generated `aliases.ps1` after the user's profile.
//!
//! The same files wrap `claude` and `codex`: typed into a pane, they first ask Agentty
//! (`agentty worktree-for`) whether another agent already works in this folder and, if so, start
//! in the working tree Agentty made for them (see `workbench::worktrees`).

use crate::settings::CommandAlias;
use anyhow::Result;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

pub fn integration_dir() -> PathBuf {
    agentty_bridge::fsutil::data_dir().join("shell")
}

pub fn is_valid_word(word: &str) -> bool {
    !word.is_empty() && word.chars().all(|c| c.is_ascii_alphanumeric() || "_.:+-@".contains(c))
}

pub fn is_valid_command(command: &str) -> bool {
    !command.is_empty() && command.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c))
}

/// Aliases that can be installed safely, grouped by command.
pub fn grouped(aliases: &[CommandAlias]) -> BTreeMap<String, Vec<&CommandAlias>> {
    let mut map: BTreeMap<String, Vec<&CommandAlias>> = BTreeMap::new();
    for alias in aliases.iter().filter(|a| is_valid_word(&a.keyword) && is_valid_command(&a.command)) {
        map.entry(alias.command.clone()).or_default().push(alias);
    }
    map
}

/// Agents typed into a shell get a wrapper even without reserved words: right before they start,
/// it asks Agentty (`agentty worktree-for`) whether another agent already works in this folder,
/// and moves to the working tree Agentty made for it if so.
const AGENTS: [&str; 2] = ["claude", "codex"];

/// Every command that gets a wrapper function, with its reserved words (maybe none).
fn wrapped(aliases: &[CommandAlias]) -> BTreeMap<String, Vec<&CommandAlias>> {
    let mut map = grouped(aliases);
    for agent in AGENTS {
        map.entry(agent.to_string()).or_default();
    }
    map
}

/// First words that don't start a new session in the folder (`--version`, `mcp`, …) and flags that
/// resume one: a resumed conversation belongs to the folder it was held in, so it stays there.
const STAY_FIRST: &str =
    "-v|--version|-h|--help|resume|mcp|config|doctor|update|install|login|logout|setup-token|plugin|completion|migrate-installer";
const STAY_ANY: &str = "-c|--continue|-r|--resume|--resume=*";

fn single_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// `__agentty_own_tree <agent> <args…>` for zsh and bash (`found` tests that the program is installed).
fn posix_tree_helper(found: &str) -> String {
    format!(
        r#"__agentty_own_tree() {{
  [ -n "$AGENTTY_SHELL_API" ] && [ -n "$AGENTTY_BIN" ] && [ -n "$AGENTTY_SOCKET" ] || return 0
  {found} || return 0
  case "$2" in {STAY_FIRST}) return 0;; esac
  local __agentty_word __agentty_tree
  for __agentty_word in "$@"; do
    case "$__agentty_word" in {STAY_ANY}) return 0;; esac
  done
  __agentty_tree=$("$AGENTTY_BIN" worktree-for "$1") || return 0
  if [ -n "$__agentty_tree" ] && [ -d "$__agentty_tree" ]; then builtin cd -- "$__agentty_tree"; fi
  return 0
}}
"#
    )
}

/// `__agentty_guide_ok <args…>`: whether a typed `claude` gets Agentty's guide and skills (the pane
/// has them, it isn't a subcommand like `mcp` or `--version`, and the user didn't pass options of the
/// same kind themselves).
fn posix_guide_helper() -> String {
    format!(
        r#"__agentty_guide_ok() {{
  [ -n "$AGENTTY_GUIDE_FILE" ] && [ -r "$AGENTTY_GUIDE_FILE" ] && [ -d "$AGENTTY_PLUGIN_DIR" ] || return 1
  case "$1" in {STAY_FIRST}) return 1;; esac
  local __agentty_word
  for __agentty_word in "$@"; do
    case "$__agentty_word" in --append-system-prompt*|--system-prompt*|--plugin-dir*) return 1;; esac
  done
  return 0
}}
"#
    )
}

/// Lines that set `__agentty_guide` to the guide options for `claude` (empty for other commands).
fn posix_guide_lines(command: &str) -> String {
    if command != "claude" {
        return "  local -a __agentty_guide=()\n".into();
    }
    concat!(
        "  local -a __agentty_guide=()\n",
        "  __agentty_guide_ok \"$@\" && __agentty_guide=(--append-system-prompt-file \"$AGENTTY_GUIDE_FILE\" --plugin-dir \"$AGENTTY_PLUGIN_DIR\")\n"
    )
    .into()
}

/// zsh functions wrapping each command; exact-match arguments are replaced by their expansion
/// (split into words with shell quoting rules). `claude` and `codex` first ask for a working tree.
pub fn zsh_script(aliases: &[CommandAlias]) -> String {
    let mut out = String::from("# Generated by Agentty: reserved words and working trees. Edit words in Settings.\n");
    out.push_str(&posix_tree_helper(r#"(( $+commands[$1] ))"#));
    out.push_str(&posix_guide_helper());
    for (command, entries) in wrapped(aliases) {
        let _ = writeln!(out, "unalias {command} 2>/dev/null");
        let _ = writeln!(out, "function {command} {{");
        if AGENTS.contains(&command.as_str()) {
            let _ = writeln!(out, "  __agentty_own_tree {command} \"$@\"");
        }
        out.push_str(&posix_guide_lines(&command));
        if entries.is_empty() {
            let _ = writeln!(out, "  command {command} \"${{__agentty_guide[@]}}\" \"$@\"");
            let _ = writeln!(out, "}}");
            continue;
        }
        let _ = writeln!(out, "  local -a __agentty_args; local __agentty_word");
        let _ = writeln!(out, "  for __agentty_word in \"$@\"; do");
        let _ = writeln!(out, "    case \"$__agentty_word\" in");
        for alias in entries {
            let _ = writeln!(
                out,
                "      {}) __agentty_args+=(${{(Q)${{(z)${{:-{}}}}}}});;",
                single_quote(&alias.keyword),
                single_quote(&alias.expansion)
            );
        }
        let _ = writeln!(out, "      *) __agentty_args+=(\"$__agentty_word\");;");
        let _ = writeln!(out, "    esac");
        let _ = writeln!(out, "  done");
        let _ = writeln!(out, "  command {command} \"${{__agentty_guide[@]}}\" \"${{__agentty_args[@]}}\"");
        let _ = writeln!(out, "}}");
    }
    out
}

/// bash equivalent (expansions are split on whitespace).
pub fn bash_script(aliases: &[CommandAlias]) -> String {
    let mut out = String::from("# Generated by Agentty: reserved words and working trees. Edit words in Settings.\n");
    out.push_str(&posix_tree_helper(r#"type -P "$1" >/dev/null"#));
    out.push_str(&posix_guide_helper());
    for (command, entries) in wrapped(aliases) {
        let _ = writeln!(out, "unalias {command} 2>/dev/null");
        let _ = writeln!(out, "{command}() {{");
        if AGENTS.contains(&command.as_str()) {
            let _ = writeln!(out, "  __agentty_own_tree {command} \"$@\"");
        }
        out.push_str(&posix_guide_lines(&command));
        if entries.is_empty() {
            let _ = writeln!(out, "  command {command} \"${{__agentty_guide[@]}}\" \"$@\"");
            let _ = writeln!(out, "}}");
            continue;
        }
        let _ = writeln!(out, "  local __agentty_args=() __agentty_word");
        let _ = writeln!(out, "  for __agentty_word in \"$@\"; do");
        let _ = writeln!(out, "    case \"$__agentty_word\" in");
        for alias in entries {
            let _ = writeln!(
                out,
                "      {}) read -r -a __agentty_split <<< {}; __agentty_args+=(\"${{__agentty_split[@]}}\");;",
                single_quote(&alias.keyword),
                single_quote(&alias.expansion)
            );
        }
        let _ = writeln!(out, "      *) __agentty_args+=(\"$__agentty_word\");;");
        let _ = writeln!(out, "    esac");
        let _ = writeln!(out, "  done");
        let _ = writeln!(out, "  command {command} \"${{__agentty_guide[@]}}\" \"${{__agentty_args[@]}}\"");
        let _ = writeln!(out, "}}");
    }
    out
}

/// PowerShell equivalent: a function per command that expands exact-match arguments (split on
/// whitespace) and calls the real program (applications only, so the function doesn't recurse).
/// `claude` and `codex` first ask for a working tree.
pub fn powershell_script(aliases: &[CommandAlias]) -> String {
    let quote = |text: &str| format!("'{}'", text.replace('\'', "''"));
    let list = |patterns: &str| patterns.split('|').filter(|p| !p.contains('*')).map(quote).collect::<Vec<_>>().join(", ");
    let mut out = String::from("# Generated by Agentty: reserved words and working trees. Edit words in Settings.\n");
    let _ = writeln!(out, "function global:__agentty_own_tree([string]$__agentty_name, [object[]]$__agentty_rest) {{");
    let _ = writeln!(out, "  if (-not $env:AGENTTY_SHELL_API -or -not $env:AGENTTY_BIN -or -not $env:AGENTTY_SOCKET) {{ return }}");
    let _ = writeln!(out, "  if (-not (Get-Command $__agentty_name -CommandType Application -ErrorAction SilentlyContinue)) {{ return }}");
    let _ = writeln!(out, "  if ($__agentty_rest.Count -gt 0 -and \"$($__agentty_rest[0])\" -in @({})) {{ return }}", list(STAY_FIRST));
    let _ = writeln!(out, "  foreach ($__agentty_word in $__agentty_rest) {{");
    let _ = writeln!(out, "    if (\"$__agentty_word\" -in @({}) -or \"$__agentty_word\" -like '--resume=*') {{ return }}", list(STAY_ANY));
    let _ = writeln!(out, "  }}");
    let _ = writeln!(out, "  $__agentty_tree = & $env:AGENTTY_BIN worktree-for $__agentty_name | Select-Object -First 1");
    let _ = writeln!(out, "  if ($__agentty_tree -and (Test-Path -LiteralPath \"$__agentty_tree\" -PathType Container)) {{");
    let _ = writeln!(out, "    Set-Location -LiteralPath \"$__agentty_tree\"");
    let _ = writeln!(out, "  }}");
    let _ = writeln!(out, "}}");
    for (command, entries) in wrapped(aliases) {
        let _ = writeln!(out, "function global:{command} {{");
        if AGENTS.contains(&command.as_str()) {
            let _ = writeln!(out, "  __agentty_own_tree {} $args", quote(&command));
        }
        let _ = writeln!(out, "  $__agentty_guide = @()");
        if command == "claude" {
            let _ = writeln!(
                out,
                "  if ($env:AGENTTY_GUIDE_FILE -and (Test-Path -LiteralPath $env:AGENTTY_GUIDE_FILE) -and $env:AGENTTY_PLUGIN_DIR -and -not ($args.Count -gt 0 -and \"$($args[0])\" -in @({})) -and -not ($args | Where-Object {{ \"$_\" -like '--append-system-prompt*' -or \"$_\" -like '--system-prompt*' -or \"$_\" -like '--plugin-dir*' }})) {{",
                list(STAY_FIRST)
            );
            let _ = writeln!(
                out,
                "    $__agentty_guide = @('--append-system-prompt-file', $env:AGENTTY_GUIDE_FILE, '--plugin-dir', $env:AGENTTY_PLUGIN_DIR)"
            );
            let _ = writeln!(out, "  }}");
        }
        if entries.is_empty() {
            let _ = writeln!(out, "  $__agentty_args = $args");
        } else {
            let _ = writeln!(out, "  $__agentty_args = foreach ($__agentty_word in $args) {{");
            let _ = writeln!(out, "    switch -CaseSensitive ($__agentty_word) {{");
            for alias in entries {
                let words: Vec<String> = alias.expansion.split_whitespace().map(quote).collect();
                let _ = writeln!(out, "      {} {{ {} }}", quote(&alias.keyword), words.join(", "));
            }
            let _ = writeln!(out, "      default {{ $__agentty_word }}");
            let _ = writeln!(out, "    }}");
            let _ = writeln!(out, "  }}");
        }
        let _ = writeln!(
            out,
            "  $__agentty_program = Get-Command {} -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1",
            quote(&command)
        );
        let _ = writeln!(
            out,
            "  if (-not $__agentty_program) {{ Write-Error \"{command}: the term '{command}' is not recognized as a program\"; return }}"
        );
        let _ = writeln!(out, "  & $__agentty_program.Source @__agentty_guide @__agentty_args");
        let _ = writeln!(out, "}}");
    }
    out
}

const ZSHENV: &str = r#"# Agentty shell integration (generated). Restores the user's ZDOTDIR and startup files.
if [[ -n "$AGENTTY_USER_ZDOTDIR" ]]; then
  ZDOTDIR="$AGENTTY_USER_ZDOTDIR"
else
  unset ZDOTDIR
fi
unset AGENTTY_USER_ZDOTDIR
[[ -f "${ZDOTDIR:-$HOME}/.zshenv" ]] && source "${ZDOTDIR:-$HOME}/.zshenv"
if [[ -o interactive && -n "$AGENTTY_SHELL_DIR" ]]; then
  _agentty_load_aliases() { [[ -r "$AGENTTY_SHELL_DIR/aliases.zsh" ]] && source "$AGENTTY_SHELL_DIR/aliases.zsh" }
  # Before each prompt and right before each command, so words added in Settings apply at once.
  autoload -Uz add-zsh-hook 2>/dev/null && { add-zsh-hook precmd _agentty_load_aliases; add-zsh-hook preexec _agentty_load_aliases; }
fi
"#;

const BASHRC: &str = r#"# Agentty shell integration (generated).
[ -f /etc/profile ] && . /etc/profile
if [ -f "$HOME/.bash_profile" ]; then . "$HOME/.bash_profile"
elif [ -f "$HOME/.bash_login" ]; then . "$HOME/.bash_login"
elif [ -f "$HOME/.profile" ]; then . "$HOME/.profile"
fi
[ -f "$HOME/.bashrc" ] && . "$HOME/.bashrc"
_agentty_load_aliases() { [ -r "$AGENTTY_SHELL_DIR/aliases.bash" ] && . "$AGENTTY_SHELL_DIR/aliases.bash"; }
_agentty_load_aliases
# Words added in Settings apply from the next prompt.
PROMPT_COMMAND="_agentty_load_aliases${PROMPT_COMMAND:+;$PROMPT_COMMAND}"
"#;

/// Writes the integration files. Called at startup and whenever aliases change.
pub fn write_files(aliases: &[CommandAlias]) -> Result<()> {
    let dir = integration_dir();
    std::fs::create_dir_all(dir.join("zsh"))?;
    write_if_changed(&dir.join("zsh").join(".zshenv"), ZSHENV)?;
    write_if_changed(&dir.join("rc.bash"), BASHRC)?;
    write_if_changed(&dir.join("aliases.zsh"), &zsh_script(aliases))?;
    write_if_changed(&dir.join("aliases.bash"), &bash_script(aliases))?;
    if cfg!(windows) {
        write_if_changed(&dir.join("aliases.ps1"), &powershell_script(aliases))?;
    }
    Ok(())
}

fn write_if_changed(path: &Path, content: &str) -> Result<()> {
    if std::fs::read_to_string(path).ok().as_deref() != Some(content) {
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, content)?;
        std::fs::rename(tmp, path)?;
    }
    Ok(())
}

pub enum ShellFlavor {
    Zsh,
    Bash,
    Other,
}

pub fn flavor(shell: &str) -> ShellFlavor {
    match Path::new(shell).file_name().and_then(|n| n.to_str()) {
        Some("zsh") => ShellFlavor::Zsh,
        Some("bash") => ShellFlavor::Bash,
        _ => ShellFlavor::Other,
    }
}

/// Environment for a pane's shell.
/// What the shell wrappers may ask of the pane's Agentty (`$AGENTTY_SHELL_API`).
pub const SHELL_API: &str = "1";

pub fn environment(shell: &str) -> Vec<(String, String)> {
    let dir = integration_dir();
    let mut env = vec![
        ("AGENTTY_SHELL_DIR".to_string(), dir.display().to_string()),
        // The pane's Agentty answers `agentty worktree-for`: the wrappers only ask when this is set, so
        // wrappers written by a newer version never start an older Agentty that lacks the command.
        ("AGENTTY_SHELL_API".to_string(), SHELL_API.to_string()),
    ];
    if let ShellFlavor::Zsh = flavor(shell) {
        if let Ok(user) = std::env::var("ZDOTDIR") {
            env.push(("AGENTTY_USER_ZDOTDIR".into(), user));
        }
        env.push(("ZDOTDIR".into(), dir.join("zsh").display().to_string()));
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::process::Command;

    fn alias(keyword: &str, expansion: &str, command: &str) -> CommandAlias {
        CommandAlias { keyword: keyword.into(), expansion: expansion.into(), command: command.into() }
    }

    #[cfg(unix)]
    fn fake_command(dir: &Path) -> PathBuf {
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let script = bin.join("agenttyprobe");
        std::fs::write(&script, "#!/bin/sh\nfor a in \"$@\"; do printf '[%s]' \"$a\"; done\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        bin
    }

    #[test]
    fn rejects_unsafe_names() {
        let aliases = [alias("ok", "--x", "claude"), alias("bad word", "--x", "claude"), alias("k", "--x", "rm;ls")];
        let groups = grouped(&aliases);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups["claude"].len(), 1);
    }

    #[test]
    fn powershell_wrapper_lists_expansions() {
        let script =
            powershell_script(&[alias("zzzz", "--dangerously-skip-permissions --verbose", "claude"), alias("q", "--name it's", "codex")]);
        assert!(script.contains("function global:claude {"));
        assert!(script.contains("'zzzz' { '--dangerously-skip-permissions', '--verbose' }"));
        assert!(script.contains("'q' { '--name', 'it''s' }"));
        assert!(script.contains("Get-Command 'codex' -CommandType Application"));
    }

    /// The reserved-word functions in real PowerShell (skipped without `pwsh`).
    #[test]
    #[cfg(unix)]
    fn powershell_wrapper_expands_exact_words() {
        let Some(pwsh) = crate::launch::tests::test_pwsh() else { return };
        let dir = std::env::temp_dir().join(format!("agentty-pwsh-alias-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        crate::launch::tests::fake_program(&dir, "agenttyprobe");
        let aliases =
            [alias("zzzz", "--dangerously-skip-permissions --verbose", "agenttyprobe"), alias("q", "--name it's", "agenttyprobe")];
        let script = format!("{}\nagenttyprobe x zzzz y q ZZZZ", powershell_script(&aliases));
        let output = Command::new(pwsh)
            .args(["-NoProfile", "-NonInteractive", "-EncodedCommand", &crate::launch::encode_powershell(&script)])
            .env("PATH", format!("{}:{}", dir.display(), std::env::var("PATH").unwrap_or_default()))
            .output()
            .unwrap();
        let out = String::from_utf8_lossy(&output.stdout);
        assert!(
            out.starts_with("[x][--dangerously-skip-permissions][--verbose][y][--name][it's][ZZZZ]"),
            "{out}{}",
            String::from_utf8_lossy(&output.stderr)
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    #[cfg(unix)]
    fn zsh_wrapper_expands_exact_words() {
        let dir = std::env::temp_dir().join(format!("agentty-zsh-{}", std::process::id()));
        let bin = fake_command(&dir);
        let script = dir.join("aliases.zsh");
        let aliases = [alias("zzzz", "--dangerously-skip-permissions", "agenttyprobe"), alias("two", "--a 'b c'", "agenttyprobe")];
        std::fs::write(&script, zsh_script(&aliases)).unwrap();
        let output = Command::new("zsh")
            .args(["-f", "-c", &format!("source {}; agenttyprobe x zzzz two zzzzz", script.display())])
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout), "[x][--dangerously-skip-permissions][--a][b c][zzzzz]");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    #[cfg(unix)]
    fn bash_wrapper_expands_exact_words() {
        let dir = std::env::temp_dir().join(format!("agentty-bash-{}", std::process::id()));
        let bin = fake_command(&dir);
        let script = dir.join("aliases.bash");
        std::fs::write(&script, bash_script(&[alias("zzzz", "--dangerously-skip-permissions --verbose", "agenttyprobe")])).unwrap();
        let output = Command::new("bash")
            .args(["--noprofile", "--norc", "-c", &format!("source {}; agenttyprobe zzzz y", script.display())])
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout), "[--dangerously-skip-permissions][--verbose][y]");
        std::fs::remove_dir_all(dir).ok();
    }

    /// `claude` typed into a pane: the wrapper moves to the tree `agentty worktree-for` names, but
    /// not for `--version` or a resumed session, and not when Agentty answers nothing.
    #[test]
    #[cfg(unix)]
    fn agent_wrappers_move_to_the_tree_agentty_made() {
        use std::os::unix::fs::PermissionsExt;
        for (shell, script) in [("zsh", zsh_script(&[])), ("bash", bash_script(&[]))] {
            let dir = std::env::temp_dir().join(format!("agentty-tree-wrapper-{shell}-{}", std::process::id()));
            let (bin, tree) = (dir.join("bin"), dir.join("tree"));
            std::fs::create_dir_all(&bin).unwrap();
            std::fs::create_dir_all(&tree).unwrap();
            let executable = |path: &Path, body: &str| {
                std::fs::write(path, body).unwrap();
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
            };
            // The agent prints where it started; Agentty answers with the tree (or nothing).
            executable(&bin.join("claude"), "#!/bin/sh\npwd -P\n");
            let answer = format!("#!/bin/sh\n[ \"$1\" = worktree-for ] && [ -z \"$STAY\" ] && echo {}\n", tree.display());
            executable(&dir.join("agentty"), &answer);
            std::fs::write(dir.join("wrappers"), &script).unwrap();
            let run = |args: &str, stay: bool| {
                let flag = if shell == "zsh" { "-f" } else { "--norc" };
                let mut command = Command::new(shell);
                command
                    .args([flag, "-c", &format!("source {}; cd /; claude {args}", dir.join("wrappers").display())])
                    .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
                    .env("AGENTTY_BIN", dir.join("agentty"))
                    .env("AGENTTY_SHELL_API", SHELL_API)
                    .env("AGENTTY_SOCKET", "placeholder");
                if stay {
                    command.env("STAY", "1");
                }
                let output = command.output().unwrap();
                String::from_utf8_lossy(&output.stdout).trim().to_string()
            };
            let tree_real = tree.canonicalize().unwrap().display().to_string();
            assert_eq!(run("fix it", false), tree_real, "{shell}");
            assert_eq!(run("--version", false), "/", "{shell}");
            assert_eq!(run("--resume abc", false), "/", "{shell}");
            assert_eq!(run("", true), "/", "{shell}");
            std::fs::remove_dir_all(dir).ok();
        }
    }

    /// Typed `claude` gets Agentty's guide and skills when the pane has them.
    #[test]
    #[cfg(unix)]
    fn claude_wrapper_passes_the_guide() {
        use std::os::unix::fs::PermissionsExt;
        for (shell, script) in [("zsh", zsh_script(&[])), ("bash", bash_script(&[]))] {
            let dir = std::env::temp_dir().join(format!("agentty-guide-wrapper-{shell}-{}", std::process::id()));
            let bin = dir.join("bin");
            std::fs::create_dir_all(&bin).unwrap();
            std::fs::create_dir_all(dir.join("plugin")).unwrap();
            std::fs::write(dir.join("guide.md"), "guide").unwrap();
            let claude = bin.join("claude");
            std::fs::write(&claude, "#!/bin/sh\nfor a in \"$@\"; do printf '[%s]' \"$a\"; done\n").unwrap();
            std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
            std::fs::write(dir.join("wrappers"), &script).unwrap();
            let run = |args: &str| {
                let flag = if shell == "zsh" { "-f" } else { "--norc" };
                let output = Command::new(shell)
                    .args([flag, "-c", &format!("source {}; claude {args}", dir.join("wrappers").display())])
                    .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
                    // Never the Agentty this test runs in: its binary would be started for real.
                    .env_remove("AGENTTY_BIN")
                    .env_remove("AGENTTY_SOCKET")
                    .env_remove("AGENTTY_SHELL_API")
                    .env("AGENTTY_GUIDE_FILE", dir.join("guide.md"))
                    .env("AGENTTY_PLUGIN_DIR", dir.join("plugin"))
                    .output()
                    .unwrap();
                String::from_utf8_lossy(&output.stdout).to_string()
            };
            let (guide, plugin) = (dir.join("guide.md").display().to_string(), dir.join("plugin").display().to_string());
            assert_eq!(run("hi"), format!("[--append-system-prompt-file][{guide}][--plugin-dir][{plugin}][hi]"), "{shell}");
            assert_eq!(run("--version"), "[--version]", "{shell}");
            assert_eq!(run("mcp list"), "[mcp][list]", "{shell}");
            assert_eq!(run("--append-system-prompt mine"), "[--append-system-prompt][mine]", "{shell}");
            std::fs::remove_dir_all(dir).ok();
        }
    }

    #[test]
    fn powershell_wrappers_ask_for_a_tree() {
        let script = powershell_script(&[]);
        assert!(script.contains("function global:claude {") && script.contains("function global:codex {"));
        assert!(script.contains("__agentty_own_tree 'claude' $args"));
        assert!(script.contains("worktree-for $__agentty_name"));
        // Wildcards are matched with -like, not listed as words.
        assert!(script.contains("'--resume')") && !script.contains("'--resume=*',"));
    }
}
