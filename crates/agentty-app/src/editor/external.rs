//! "Open in editor": hands the file to VS Code, Cursor or the system's text editor.
//!
//! The file is opened as a document, never run: VS Code and Cursor are told which file and line
//! to show, macOS opens it with the default *text* editor (`open -t`), Windows with Notepad, and
//! Linux with an installed text editor. `xdg-open` is never used: it picks the app registered for
//! the file's type, which for a script (or, with Wine, a `.js` or `.bat`) can be one that runs it.

use crate::settings::ExternalEditor;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Eq)]
pub enum ExternalError {
    /// The chosen editor is not installed.
    NotInstalled(&'static str),
    Failed(String),
}

/// Opens `file` at `line` / `column` (1-based) with the preferred editor.
pub fn open(file: &Path, line: usize, column: usize, preference: ExternalEditor, project: &Path) -> Result<(), ExternalError> {
    let (program, args) = command(file, line, column, preference, project)?;
    agentty_bridge::process::command(&program).args(&args).spawn().map(|_| ()).map_err(|e| ExternalError::Failed(e.to_string()))
}

/// The program and arguments that open `file` for `preference`.
pub fn command(
    file: &Path,
    line: usize,
    column: usize,
    preference: ExternalEditor,
    project: &Path,
) -> Result<(PathBuf, Vec<OsString>), ExternalError> {
    let goto = || {
        let mut target = file.as_os_str().to_os_string();
        target.push(format!(":{line}:{column}"));
        vec![OsString::from("--goto"), target]
    };
    match preference {
        ExternalEditor::VsCode => editor_cli("code", project).map(|p| (p, goto())).ok_or(ExternalError::NotInstalled("Visual Studio Code")),
        ExternalEditor::Cursor => editor_cli("cursor", project).map(|p| (p, goto())).ok_or(ExternalError::NotInstalled("Cursor")),
        ExternalEditor::Auto => match editor_cli("code", project) {
            Some(code) => Ok((code, goto())),
            None => system(file),
        },
        ExternalEditor::System => system(file),
    }
}

/// The command-line launcher of an editor (`code`, `cursor`): on `PATH`, or inside the app where
/// the installer puts it without always linking it onto `PATH`. Never one inside the project.
fn editor_cli(name: &str, project: &Path) -> Option<PathBuf> {
    super::format::find_program(name, project).or_else(|| {
        let app = match name {
            "code" => "Visual Studio Code",
            _ => "Cursor",
        };
        let candidates: Vec<PathBuf> = if cfg!(target_os = "macos") {
            let home = dirs::home_dir().unwrap_or_default();
            [PathBuf::from("/Applications"), home.join("Applications")]
                .into_iter()
                .map(|dir| dir.join(format!("{app}.app/Contents/Resources/app/bin/{name}")))
                .collect()
        } else if cfg!(windows) {
            let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_default();
            let folder = if name == "code" { "Microsoft VS Code" } else { "cursor" };
            vec![local.join("Programs").join(folder).join("bin").join(format!("{name}.cmd"))]
        } else {
            vec![PathBuf::from(format!("/usr/share/{name}/bin/{name}")), PathBuf::from(format!("/snap/bin/{name}"))]
        };
        candidates.into_iter().find(|p| p.is_absolute() && p.is_file())
    })
}

fn system(file: &Path) -> Result<(PathBuf, Vec<OsString>), ExternalError> {
    if cfg!(target_os = "macos") {
        return Ok((PathBuf::from("/usr/bin/open"), vec!["-t".into(), file.as_os_str().to_os_string()]));
    }
    if cfg!(windows) {
        let root = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        return Ok((root.join("notepad.exe"), vec![file.as_os_str().to_os_string()]));
    }
    // Text editors of the common desktops, first found wins.
    const EDITORS: &[&str] =
        &["gnome-text-editor", "gedit", "kate", "kwrite", "xed", "pluma", "mousepad", "featherpad", "leafpad", "geany"];
    EDITORS
        .iter()
        .find_map(|name| agentty_bridge::process::which(name))
        .map(|editor| (editor, vec![file.as_os_str().to_os_string()]))
        .ok_or(ExternalError::NotInstalled("a text editor"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_editor_opens_files_as_text() {
        let file = std::env::temp_dir().join(format!("agentty-external-{}.txt", std::process::id()));
        std::fs::write(&file, "x").unwrap();
        let project = std::env::temp_dir();
        match command(&file, 3, 1, ExternalEditor::System, &project) {
            Ok((program, args)) => {
                if cfg!(target_os = "macos") {
                    assert_eq!(program, PathBuf::from("/usr/bin/open"));
                    assert_eq!(args[0], "-t", "open -t: the text editor, never the file's own app");
                } else if cfg!(windows) {
                    assert!(program.ends_with("notepad.exe"));
                }
                assert_eq!(args.last().unwrap(), file.as_os_str());
            }
            Err(err) => assert!(cfg!(target_os = "linux") && err == ExternalError::NotInstalled("a text editor")),
        }
        std::fs::remove_file(file).ok();
    }

    #[test]
    fn editors_are_told_the_line() {
        let project = std::env::temp_dir();
        if let Ok((_, args)) = command(Path::new("/p/src/main.rs"), 12, 5, ExternalEditor::VsCode, &project) {
            assert_eq!(args, [OsString::from("--goto"), OsString::from("/p/src/main.rs:12:5")]);
        }
    }
}
