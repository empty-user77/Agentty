//! "Open in": the editors and IDEs installed on this computer that a workspace's folder can be
//! handed to (VS Code, Cursor, Zed, the JetBrains IDEs, Xcode …), plus the file manager.
//!
//! Found by looking, never by running anything: on macOS an app bundle in `/Applications` or
//! `~/Applications` (where JetBrains Toolbox puts its IDEs), elsewhere the IDE's launcher on
//! `PATH`. The folder goes to the app as one argument, never through a shell.

use std::path::{Path, PathBuf};

/// One app a folder can be opened in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenApp {
    /// Stable id, for element ids.
    pub id: &'static str,
    pub name: &'static str,
    target: Target,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    /// A macOS app bundle, opened with `open -a`.
    Bundle(PathBuf),
    /// A launcher program, given the folder as its argument.
    Program(PathBuf),
    /// Finder, Explorer or the desktop's file manager.
    FileManager,
}

struct Known {
    id: &'static str,
    name: &'static str,
    /// Bundle names on macOS, the first one found wins (Community / Ultimate editions).
    bundles: &'static [&'static str],
    /// Launchers on `PATH` on Windows and Linux.
    programs: &'static [&'static str],
}

const KNOWN: &[Known] = &[
    Known { id: "vscode", name: "VS Code", bundles: &["Visual Studio Code"], programs: &["code"] },
    Known { id: "cursor", name: "Cursor", bundles: &["Cursor"], programs: &["cursor"] },
    Known { id: "windsurf", name: "Windsurf", bundles: &["Windsurf"], programs: &["windsurf"] },
    Known { id: "zed", name: "Zed", bundles: &["Zed"], programs: &["zed", "zeditor"] },
    Known {
        id: "intellij",
        name: "IntelliJ IDEA",
        bundles: &["IntelliJ IDEA", "IntelliJ IDEA Ultimate", "IntelliJ IDEA CE"],
        programs: &["idea", "idea64", "intellij-idea-ultimate", "intellij-idea-community"],
    },
    Known { id: "webstorm", name: "WebStorm", bundles: &["WebStorm"], programs: &["webstorm", "webstorm64"] },
    Known {
        id: "pycharm",
        name: "PyCharm",
        bundles: &["PyCharm", "PyCharm Professional Edition", "PyCharm CE"],
        programs: &["pycharm", "pycharm64", "charm"],
    },
    Known { id: "goland", name: "GoLand", bundles: &["GoLand"], programs: &["goland", "goland64"] },
    Known { id: "rustrover", name: "RustRover", bundles: &["RustRover"], programs: &["rustrover", "rustrover64"] },
    Known { id: "clion", name: "CLion", bundles: &["CLion"], programs: &["clion", "clion64"] },
    Known { id: "phpstorm", name: "PhpStorm", bundles: &["PhpStorm"], programs: &["phpstorm", "phpstorm64"] },
    Known { id: "rider", name: "Rider", bundles: &["Rider"], programs: &["rider", "rider64"] },
    Known { id: "android-studio", name: "Android Studio", bundles: &["Android Studio"], programs: &["studio", "studio64"] },
    Known { id: "fleet", name: "Fleet", bundles: &["Fleet"], programs: &["fleet"] },
    Known { id: "sublime", name: "Sublime Text", bundles: &["Sublime Text"], programs: &["subl"] },
    Known { id: "xcode", name: "Xcode", bundles: &["Xcode"], programs: &[] },
];

impl OpenApp {
    /// Opens `folder` in this app.
    pub fn open(&self, folder: &Path) -> std::io::Result<()> {
        match &self.target {
            Target::Bundle(bundle) => std::process::Command::new("/usr/bin/open").arg("-a").arg(bundle).arg(folder).spawn().map(|_| ()),
            Target::Program(program) => agentty_bridge::process::command(program).arg(folder).spawn().map(|_| ()),
            Target::FileManager => {
                crate::platform::open_folder(folder);
                Ok(())
            }
        }
    }
}

/// The apps installed here, in [`KNOWN`]'s order, then the file manager.
pub fn installed() -> Vec<OpenApp> {
    let home = dirs::home_dir().unwrap_or_default();
    let mut apps = find(&[PathBuf::from("/Applications"), home.join("Applications")], agentty_bridge::process::which);
    apps.push(OpenApp { id: "files", name: file_manager_name(), target: Target::FileManager });
    apps
}

fn file_manager_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "Finder"
    } else if cfg!(windows) {
        "File Explorer"
    } else {
        "Files"
    }
}

/// [`installed`] without the file manager, looking in `app_dirs` on macOS and with `which`
/// elsewhere (split out for the tests).
fn find(app_dirs: &[PathBuf], which: impl Fn(&str) -> Option<PathBuf>) -> Vec<OpenApp> {
    KNOWN
        .iter()
        .filter_map(|known| {
            let target = if cfg!(target_os = "macos") {
                known
                    .bundles
                    .iter()
                    .flat_map(|bundle| app_dirs.iter().map(move |dir| dir.join(format!("{bundle}.app"))))
                    .find(|path| path.is_dir())
                    .map(Target::Bundle)
            } else {
                known.programs.iter().find_map(|program| which(program)).map(Target::Program)
            }?;
            Some(OpenApp { id: known.id, name: known.name, target })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_installed_apps_are_offered() {
        let dir = std::env::temp_dir().join(format!("agentty-open-in-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("Zed.app")).unwrap();
        std::fs::create_dir_all(dir.join("IntelliJ IDEA CE.app")).unwrap();
        let found =
            find(std::slice::from_ref(&dir), |name| (name == "code" || name == "idea").then(|| PathBuf::from(format!("/bin/{name}"))));
        let ids: Vec<_> = found.iter().map(|a| a.id).collect();
        if cfg!(target_os = "macos") {
            assert_eq!(ids, ["zed", "intellij"], "the bundles that are there, the Community edition counting as IntelliJ");
            assert_eq!(found[1].target, Target::Bundle(dir.join("IntelliJ IDEA CE.app")));
        } else {
            assert_eq!(ids, ["vscode", "intellij"]);
            assert_eq!(found[0].target, Target::Program(PathBuf::from("/bin/code")));
        }
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn the_file_manager_is_always_there() {
        assert_eq!(installed().last().map(|a| a.id), Some("files"));
    }
}
