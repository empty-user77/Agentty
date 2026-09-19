//! Child processes Agentty starts in the background (git, sqlite3, agent CLIs, …).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A `Command` that never opens a console window (Windows starts console programs from a GUI
/// app in a new, visible console otherwise). Identical to `Command::new` elsewhere.
pub fn command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    hide_window(&mut command);
    command
}

pub fn hide_window(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// Extensions tried for a bare program name: `PATHEXT` on Windows (`.exe`, `.cmd`, …).
fn extensions() -> Vec<String> {
    if cfg!(windows) {
        let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        let mut list: Vec<String> = pathext.split(';').filter(|e| !e.is_empty()).map(|e| e.to_ascii_lowercase()).collect();
        // PowerShell scripts can't be started directly; npm installs a `.cmd` next to them.
        list.retain(|e| e != ".ps1");
        list
    } else {
        vec![String::new()]
    }
}

/// Finds `name` in the directories of `path_env` (a `PATH` value), like `command -v`.
///
/// Only absolute directories are searched: an empty or relative entry (`PATH=a;;b`, `.`) would
/// resolve against the current directory, where a planted `claude.cmd` could hijack the lookup.
/// A `name` with a directory part must be an absolute path for the same reason.
pub fn which_in(name: &str, path_env: &OsStr) -> Option<PathBuf> {
    let candidate = Path::new(name);
    if name.is_empty() {
        return None;
    }
    if candidate.components().count() > 1 {
        return (candidate.is_absolute() && is_program(candidate)).then(|| candidate.to_path_buf());
    }
    let has_extension = cfg!(windows) && candidate.extension().is_some();
    for dir in search_dirs(path_env) {
        if has_extension && is_program(&dir.join(name)) {
            return Some(dir.join(name));
        }
        for extension in extensions() {
            let path = dir.join(format!("{name}{extension}"));
            if is_program(&path) {
                return Some(path);
            }
        }
    }
    None
}

/// The directories of a `PATH` value that are searched: absolute ones only.
fn search_dirs(path_env: &OsStr) -> impl Iterator<Item = PathBuf> + '_ {
    std::env::split_paths(path_env).filter(|dir| dir.is_absolute())
}

/// `name` on the current `PATH` ([`current_path`]).
pub fn which(name: &str) -> Option<PathBuf> {
    which_in(name, &current_path())
}

/// The `PATH` for programs Agentty starts. On Windows a GUI app keeps the `PATH` it was started
/// with, so a tool installed while Agentty runs (Git, Node.js, Claude Code, …) would stay
/// invisible until a restart; the user and machine values from the registry — what a new login
/// gets — are appended to this process's `PATH` (whose entries keep their order and priority),
/// then the folders installers put tools in without always adding them to `PATH`
/// ([`tool_dirs`]). Elsewhere this is `$PATH`.
pub fn current_path() -> std::ffi::OsString {
    let process = std::env::var_os("PATH").unwrap_or_default();
    #[cfg(windows)]
    {
        let registry = [registry::machine_path(), registry::user_path()];
        let mut extra: Vec<PathBuf> =
            registry.iter().flatten().flat_map(|value| std::env::split_paths(value).collect::<Vec<_>>()).collect();
        extra.extend(tool_dirs());
        merge_paths(&process, &extra)
    }
    #[cfg(not(windows))]
    {
        process
    }
}

/// Folders agent CLIs and their runtimes install into, whether or not the installer put them on
/// `PATH` (or the change reached Agentty yet): Claude Code's native installer (`~/.local/bin`),
/// npm's global folder, Node.js, PowerShell 7, Homebrew, … Only folders that exist, and only
/// absolute ones under the home folder or the system's program folders — never the current
/// directory. They are searched after everything on `PATH`.
pub fn tool_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(home) = &home {
        dirs.push(home.join(".local").join("bin"));
    }
    if cfg!(windows) {
        let var = |name: &str| std::env::var_os(name).map(PathBuf::from);
        dirs.extend(var("APPDATA").map(|d| d.join("npm")));
        for base in ["ProgramFiles", "ProgramW6432"] {
            if let Some(dir) = var(base) {
                dirs.push(dir.join("nodejs"));
                dirs.push(dir.join("PowerShell").join("7"));
                dirs.push(dir.join("Git").join("cmd"));
            }
        }
        dirs.extend(var("LOCALAPPDATA").map(|d| d.join("Microsoft").join("WindowsApps")));
    } else {
        if let Some(home) = &home {
            for sub in [".claude/local", ".npm-global/bin", ".volta/bin", ".bun/bin", ".cargo/bin"] {
                dirs.push(home.join(sub));
            }
        }
        // Homebrew (Apple silicon, Intel). Linux's Homebrew reaches PATH through the login shell.
        for dir in ["/opt/homebrew/bin", "/usr/local/bin"] {
            dirs.push(PathBuf::from(dir));
        }
    }
    dirs.retain(|dir| dir.is_absolute() && dir.is_dir());
    dirs.dedup();
    dirs
}

/// `base` followed by the entries of `extra` it doesn't have yet (case-insensitive on Windows,
/// trailing separators ignored). Empty entries are dropped.
pub fn merge_paths(base: &OsStr, extra: &[PathBuf]) -> std::ffi::OsString {
    let key = |p: &Path| {
        let text = p.to_string_lossy();
        let text = text.trim_end_matches(['/', '\\']);
        if cfg!(windows) {
            text.to_lowercase()
        } else {
            text.to_string()
        }
    };
    let mut entries: Vec<PathBuf> = std::env::split_paths(base).filter(|p| !p.as_os_str().is_empty()).collect();
    for entry in extra {
        if !entry.as_os_str().is_empty() && !entries.iter().any(|e| key(e) == key(entry)) {
            entries.push(entry.clone());
        }
    }
    std::env::join_paths(entries).unwrap_or_else(|_| base.to_os_string())
}

/// Git for Windows' `bash.exe`, which Claude Code needs on Windows to run hooks (and so to report
/// pane status): `$CLAUDE_CODE_GIT_BASH_PATH` when set, else found from `git` on PATH or the
/// usual install folders. WSL's `System32\bash.exe` is never used — it can't run Windows
/// programs the way hooks need.
#[cfg(windows)]
pub fn git_bash() -> Option<PathBuf> {
    if let Some(configured) = std::env::var_os("CLAUDE_CODE_GIT_BASH_PATH").map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(configured);
    }
    let mut roots: Vec<PathBuf> = which("git").map(|git| git_roots(&git)).unwrap_or_default();
    for base in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
        if let Some(dir) = std::env::var_os(base) {
            roots.push(PathBuf::from(dir).join("Git"));
        }
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        roots.push(PathBuf::from(local).join("Programs").join("Git"));
    }
    roots.into_iter().map(|root| root.join("bin").join("bash.exe")).find(|bash| bash.is_file())
}

/// Install folders a `git.exe` may belong to: `<root>\cmd\git.exe`, `<root>\bin\git.exe` and
/// `<root>\mingw64\bin\git.exe` (the layouts of Git for Windows).
#[cfg_attr(not(windows), allow(dead_code))]
fn git_roots(git: &Path) -> Vec<PathBuf> {
    let Some(dir) = git.parent() else { return Vec::new() };
    let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let mut roots = Vec::new();
    if matches!(name(dir).as_str(), "cmd" | "bin") {
        roots.extend(dir.parent().map(Path::to_path_buf));
    }
    if name(dir) == "bin" && dir.parent().is_some_and(|p| name(p).starts_with("mingw")) {
        roots.extend(dir.parent().and_then(Path::parent).map(Path::to_path_buf));
    }
    // WSL's bash lives in System32; a git found there is never Git for Windows.
    roots.retain(|root| !root.to_string_lossy().to_ascii_lowercase().contains("system32"));
    roots
}

/// `%NAME%` replaced by `lookup(NAME)`, like Windows expands REG_EXPAND_SZ values; unknown names
/// and a lone `%` stay as they are.
#[cfg_attr(not(windows), allow(dead_code))]
fn expand_variables(text: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) if end > 0 => {
                let name = &after[..end];
                match lookup(name) {
                    Some(value) => out.push_str(&value),
                    None => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[end + 1..];
            }
            _ => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(windows)]
mod registry {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_NOEXPAND, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ,
    };

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// A string value, `%VARIABLES%` expanded. RegGetValueW refuses `RRF_RT_REG_EXPAND_SZ`
    /// without `RRF_NOEXPAND` (ERROR_INVALID_PARAMETER), and `Path` usually is REG_EXPAND_SZ: read
    /// it as it is stored and expand it here.
    fn read(root: HKEY, key: &str, value: &str) -> Option<std::ffi::OsString> {
        read_raw(root, key, value).map(|raw| super::expand_variables(&raw.to_string_lossy(), |name| std::env::var(name).ok()).into())
    }

    fn read_raw(root: HKEY, key: &str, value: &str) -> Option<std::ffi::OsString> {
        use std::os::windows::ffi::OsStringExt;
        let (key, value) = (wide(key), wide(value));
        let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND;
        let mut bytes: u32 = 0;
        // SAFETY: the first call only asks for the size; the second fills a buffer of that size.
        unsafe {
            if RegGetValueW(root, key.as_ptr(), value.as_ptr(), flags, std::ptr::null_mut(), std::ptr::null_mut(), &mut bytes)
                != ERROR_SUCCESS
            {
                return None;
            }
            let mut buffer = vec![0u16; (bytes as usize).div_ceil(2) + 1];
            bytes = (buffer.len() * 2) as u32;
            if RegGetValueW(root, key.as_ptr(), value.as_ptr(), flags, std::ptr::null_mut(), buffer.as_mut_ptr().cast(), &mut bytes)
                != ERROR_SUCCESS
            {
                return None;
            }
            let len = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
            Some(std::ffi::OsString::from_wide(&buffer[..len]))
        }
    }

    pub fn user_path() -> Option<std::ffi::OsString> {
        read(HKEY_CURRENT_USER, "Environment", "Path")
    }

    pub fn machine_path() -> Option<std::ffi::OsString> {
        read(HKEY_LOCAL_MACHINE, r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment", "Path")
    }
}

#[cfg(unix)]
fn is_program(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_program(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_programs_on_a_path() {
        let dir = std::env::temp_dir().join(format!("agentty-which-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let name = if cfg!(windows) { "agenttyprobe.cmd" } else { "agenttyprobe" };
        let program = dir.join(name);
        std::fs::write(&program, "").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let path = std::env::join_paths([dir.clone()]).unwrap();
        assert_eq!(which_in("agenttyprobe", &path), Some(program));
        assert_eq!(which_in("agentty-missing", &path), None);
        assert_eq!(which_in("", &path), None);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn finds_git_for_windows_roots() {
        let root = PathBuf::from("Git");
        assert_eq!(git_roots(&root.join("cmd").join("git.exe")), vec![root.clone()]);
        assert_eq!(git_roots(&root.join("mingw64").join("bin").join("git.exe")), vec![root.join("mingw64"), root.clone()]);
        assert!(git_roots(&PathBuf::from("Windows").join("System32").join("git.exe")).is_empty());
        assert!(git_roots(&PathBuf::from("git.exe")).is_empty());
    }

    #[test]
    fn merges_new_path_entries_after_existing_ones() {
        let first = std::env::temp_dir().join("agentty-a");
        let second = std::env::temp_dir().join("agentty-b");
        let base = std::env::join_paths([first.clone(), PathBuf::new(), second.clone()]).unwrap();
        let third = std::env::temp_dir().join("agentty-c");
        let mut same_as_first = first.clone().into_os_string();
        same_as_first.push(std::path::MAIN_SEPARATOR_STR);
        let merged = merge_paths(&base, &[PathBuf::from(same_as_first), third.clone(), PathBuf::new()]);
        assert_eq!(std::env::split_paths(&merged).collect::<Vec<_>>(), vec![first, second, third]);
    }

    #[test]
    fn expands_registry_variables() {
        let lookup = |name: &str| match name {
            "USERPROFILE" => Some(r"C:\Users\example".to_string()),
            "SystemRoot" => Some(r"C:\Windows".to_string()),
            _ => None,
        };
        assert_eq!(
            expand_variables(r"%USERPROFILE%\AppData\Local\Microsoft\WindowsApps;%SystemRoot%\system32", lookup),
            r"C:\Users\example\AppData\Local\Microsoft\WindowsApps;C:\Windows\system32"
        );
        assert_eq!(expand_variables("%UNKNOWN%\\bin;100%;%%", lookup), "%UNKNOWN%\\bin;100%;%%");
        assert_eq!(expand_variables("plain", lookup), "plain");
    }

    /// The machine `Path` is always there; reading it must work (it is REG_EXPAND_SZ).
    #[test]
    #[cfg(windows)]
    fn reads_the_machine_path_from_the_registry() {
        let path = registry::machine_path().expect("machine Path");
        let text = path.to_string_lossy().to_lowercase();
        assert!(text.contains("system32") && !text.contains('%'), "{text}");
    }

    #[test]
    fn tool_dirs_are_absolute_and_exist() {
        for dir in tool_dirs() {
            assert!(dir.is_absolute() && dir.is_dir(), "{}", dir.display());
        }
    }

    /// Relative and empty PATH entries mean the current directory: never searched.
    #[test]
    fn ignores_relative_path_entries() {
        let separator = if cfg!(windows) { ";" } else { ":" };
        let absolute = std::env::temp_dir();
        let path = std::env::join_paths([std::path::PathBuf::from("bin"), "".into(), ".".into(), absolute.clone()]).unwrap();
        assert_eq!(search_dirs(&path).collect::<Vec<_>>(), vec![absolute]);
        let only_relative = std::ffi::OsString::from(format!("src{separator}{separator}."));
        assert_eq!(search_dirs(&only_relative).count(), 0);
        // A name with a directory part must be absolute.
        assert_eq!(which_in(&format!("src{}lib.rs", std::path::MAIN_SEPARATOR), &only_relative), None);
    }
}
