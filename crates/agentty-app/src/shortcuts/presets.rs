//! Sets of shortcuts to start from. A preset only says where it differs from Agentty's own keys
//! (`catalog`), in the same macOS-written form.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", from = "String")]
pub enum Preset {
    #[default]
    Agentty,
    VsCode,
}

/// A preset this version does not know (a newer one's) reads as Agentty's: the settings file
/// must load either way.
impl From<String> for Preset {
    fn from(name: String) -> Self {
        match name.as_str() {
            "vscode" => Self::VsCode,
            _ => Self::Agentty,
        }
    }
}

impl Preset {
    pub const ALL: [Self; 2] = [Self::Agentty, Self::VsCode];

    /// Its name: a product's name, the same in every language.
    pub fn name(self) -> &'static str {
        match self {
            Self::Agentty => "Agentty",
            Self::VsCode => "VS Code",
        }
    }

    /// Its keys for a command, when they are not Agentty's.
    pub fn keys(self, id: &str) -> Option<&'static [&'static str]> {
        match self {
            Self::Agentty => None,
            Self::VsCode => VS_CODE.iter().find(|(command, _)| *command == id).map(|(_, keys)| *keys),
        }
    }
}

/// Visual Studio Code's keys where it has the same thing: a terminal is a tab here, a split is a
/// split, the Explorer is the files panel and Source Control the Git page. Everything VS Code
/// shares with Agentty already (⇧⌘P, ⌘P, ⌘B, ⌘W, ⌘,, ⇧⌘X, ⇧⌘N, ⇧⌘] / ⇧⌘[, ⌃Tab) stays as it is.
const VS_CODE: &[(&str, &[&str])] = &[
    // Terminal: Create New Terminal (⌃⇧`). ⌘T stays beside it.
    ("new_terminal", NEW_TERMINAL),
    // Split Editor / Split Terminal.
    ("split_right", &["cmd-\\"]),
    ("split_down", &["alt-cmd-\\"]),
    // Focus Next / Previous Pane (terminal), which is also Next / Previous Editor.
    ("next_pane", &["alt-cmd-right"]),
    ("previous_pane", &["alt-cmd-left"]),
    // View: Show Explorer. The workspace list takes the key the files panel had.
    ("files", &["cmd-shift-e"]),
    ("show_workspaces", &["cmd-alt-b"]),
    // View: Show Source Control.
    ("git", &["ctrl-shift-g"]),
];

/// gpui on Linux reports Shift with a symbol as the character it types and drops the Shift
/// (`keymap::as_typed` does this for the translated ⌘ shortcuts; this one has no ⌘ to translate).
const NEW_TERMINAL: &[&str] =
    if cfg!(any(target_os = "linux", target_os = "freebsd")) { &["ctrl-~", "cmd-t"] } else { &["ctrl-shift-`", "cmd-t"] };

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shortcuts::catalog;

    /// A preset names commands that exist, and gives no key to two commands of one context.
    #[test]
    fn a_preset_is_whole() {
        for preset in Preset::ALL {
            let mut taken: Vec<(Option<&str>, &str, &str)> = Vec::new();
            for (id, _) in VS_CODE {
                assert!(catalog::command(id).is_some(), "{id} is not a command");
            }
            for command in catalog::COMMANDS {
                for keys in preset.keys(command.id).unwrap_or(command.keys) {
                    if let Some((_, _, other)) = taken.iter().find(|(context, k, _)| *context == command.context && k == keys) {
                        panic!("{}: {keys} is given to {other} and {}", preset.name(), command.id);
                    }
                    taken.push((command.context, keys, command.id));
                }
            }
        }
    }

    #[test]
    fn the_setting_reads_as_a_word() {
        assert_eq!(serde_json::to_string(&Preset::VsCode).unwrap(), "\"vscode\"");
        assert_eq!(serde_json::from_str::<Preset>("\"agentty\"").unwrap(), Preset::Agentty);
    }
}
