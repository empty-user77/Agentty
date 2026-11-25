//! Shortcuts the user can change.
//!
//! - [`catalog`]: the commands and Agentty's own keys for them.
//! - [`presets`]: other sets of keys to start from (VS Code).
//! - [`chord`]: one key press: written down, compared, shown, judged.
//! - this file: what the settings say ([`ShortcutSettings`]), what that adds up to
//!   ([`resolve`]), and the changes the settings screen makes ([`assign`], [`reset`], …).
//!
//! The rest of the app touches this in two places: `main.rs` binds [`bind`]'s keys with every
//! other binding, and a label that shows a shortcut asks [`hint`] for it. With Agentty's preset
//! and nothing changed, the bindings are the ones `main.rs` used to list itself (a test below
//! holds them to that).

pub mod catalog;
pub mod chord;
mod presets;

pub use catalog::{Command, COMMANDS, GROUPS};
pub use presets::Preset;

use gpui::{App, Global, KeyBinding};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// The `shortcuts` part of the settings file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ShortcutSettings {
    pub preset: Preset,
    /// The user's own keys, by system (`macos`, `windows`, `linux`) and command id: the chord as
    /// it was pressed there, or nothing for "no shortcut". Kept per system because a pressed
    /// chord is that keyboard's (⌘ on one, Ctrl+Shift on another), and settings travel.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub custom: BTreeMap<String, BTreeMap<String, String>>,
}

impl ShortcutSettings {
    fn mine(&self) -> Option<&BTreeMap<String, String>> {
        self.custom.get(std::env::consts::OS)
    }

    fn mine_mut(&mut self) -> &mut BTreeMap<String, String> {
        self.custom.entry(std::env::consts::OS.to_string()).or_default()
    }

    /// The user's own chord for a command on this system (`Some("")`: no shortcut). A chord that
    /// is not one (a settings file edited by hand) counts as not set: binding it would panic.
    pub fn custom_for(&self, id: &str) -> Option<&str> {
        self.mine()?.get(id).map(String::as_str).filter(|chord| chord.is_empty() || is_chord(chord))
    }

    /// Whether anything was changed on this system.
    pub fn changed(&self) -> bool {
        self.mine().is_some_and(|mine| !mine.is_empty())
    }

    fn forget(&mut self, id: &str) {
        self.mine_mut().remove(id);
        self.custom.retain(|_, keys| !keys.is_empty());
    }
}

/// Whether gpui reads the text as one key press with its modifiers.
fn is_chord(text: &str) -> bool {
    text.len() <= 64 && !text.contains(char::is_whitespace) && gpui::Keystroke::parse(keep(text)).is_ok()
}

/// The keys a command answers to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Keys {
    /// The preset's, written for macOS.
    Written(Vec<&'static str>),
    /// The user's, as pressed on this system.
    Pressed(String),
    None,
}

pub struct Resolved {
    pub command: &'static Command,
    pub keys: Keys,
}

/// What a binding answers to on this system: itself, and for a written Shift + symbol the
/// character gpui reports instead (`cmd-shift-[` is pressed as `cmd-{`).
fn answers_to(binding: &KeyBinding) -> Vec<String> {
    let Some(chord) = chord::of_binding(binding) else { return Vec::new() };
    let mut all = vec![chord];
    if let [keystroke] = binding.keystrokes() {
        let keystroke = keystroke.inner();
        if let Some(typed) = crate::keymap::us_shifted(&keystroke.key).filter(|_| keystroke.modifiers.shift) {
            all.push(chord::written(&gpui::Modifiers { shift: false, ..keystroke.modifiers }, typed));
        }
    }
    all
}

/// Every command with the keys the settings give it. The user's own keys come first: a preset's
/// key that one of them took is left out, so a chord never means two things in one context.
pub fn resolve(settings: &ShortcutSettings) -> Vec<Resolved> {
    let commands: Vec<&'static Command> = COMMANDS.iter().filter(|command| command.available()).collect();
    let mut taken: Vec<(Option<&str>, String)> = Vec::new();
    for command in &commands {
        if let Some(chord) = settings.custom_for(command.id).filter(|chord| !chord.is_empty()) {
            taken.push((command.context, chord.to_string()));
        }
    }
    commands
        .into_iter()
        .map(|command| match settings.custom_for(command.id) {
            Some("") => Resolved { command, keys: Keys::None },
            Some(chord) => Resolved { command, keys: Keys::Pressed(chord.to_string()) },
            None => {
                let free: Vec<&'static str> = settings
                    .preset
                    .keys(command.id)
                    .unwrap_or(command.keys)
                    .iter()
                    .copied()
                    .filter(|keys| {
                        let answers = answers_to(&command.written(keys));
                        !taken.iter().any(|(context, chord)| *context == command.context && answers.contains(chord))
                    })
                    .collect();
                let keys = if free.is_empty() { Keys::None } else { Keys::Written(free) };
                Resolved { command, keys }
            }
        })
        .collect()
}

/// A string kept for the life of the app, each distinct one once: gpui's bindings and tooltips
/// take `&'static str`. Only chords and labels go in, so it stays as small as the keyboard.
fn keep(text: &str) -> &'static str {
    static KEPT: std::sync::Mutex<Vec<&'static str>> = std::sync::Mutex::new(Vec::new());
    let mut kept = KEPT.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(known) = kept.iter().find(|known| **known == text) {
        return known;
    }
    let leaked: &'static str = Box::leak(text.to_string().into_boxed_str());
    kept.push(leaked);
    leaked
}

fn bindings_of(resolved: &[Resolved]) -> Vec<KeyBinding> {
    let mut bindings = Vec::new();
    for entry in resolved {
        match &entry.keys {
            Keys::Written(keys) => bindings.extend(keys.iter().map(|keys| entry.command.written(keys))),
            Keys::Pressed(chord) => bindings.push(entry.command.pressed(keep(chord))),
            Keys::None => {}
        }
    }
    bindings
}

/// The labels of the bindings in force, read by everything that shows a shortcut. Rebuilt when
/// the keys are bound, never while drawing.
#[derive(Default)]
struct Labels {
    /// The first key of each command with macOS's glyphs (what `keymap::display` takes), or as
    /// pressed here.
    hints: HashMap<&'static str, &'static str>,
    /// Every key of each command, as this system writes shortcuts.
    shown: HashMap<&'static str, Vec<String>>,
}

impl Global for Labels {}

fn labels_of(resolved: &[Resolved]) -> Labels {
    let mut labels = Labels::default();
    for entry in resolved {
        let (hint, shown) = match &entry.keys {
            Keys::Written(keys) => {
                let glyphs: Vec<String> = keys.iter().map(|keys| chord::glyphs(keys)).collect();
                let shown = glyphs.iter().map(|label| crate::keymap::display(label).into_owned()).collect();
                (glyphs.first().cloned(), shown)
            }
            Keys::Pressed(chord) => {
                let label = chord::pressed_label(chord);
                (Some(label.clone()), vec![label])
            }
            Keys::None => (None, Vec::new()),
        };
        if let Some(hint) = hint {
            labels.hints.insert(entry.command.id, keep(&hint));
        }
        labels.shown.insert(entry.command.id, shown);
    }
    labels
}

/// The changeable shortcuts as the settings have them, for `main.rs` to bind with the rest. Also
/// notes their labels for [`hint`] and [`shown`].
pub fn bind(cx: &mut App) -> Vec<KeyBinding> {
    let resolved = resolve(&crate::settings::settings(cx).shortcuts);
    cx.set_global(labels_of(&resolved));
    bindings_of(&resolved)
}

/// A command's shortcut for a tooltip or a menu row (`ui::Tooltip::text` and the like show it
/// through `keymap::display`). `None` when it has no shortcut.
pub fn hint(id: &str, cx: &App) -> Option<&'static str> {
    match cx.try_global::<Labels>() {
        Some(labels) => labels.hints.get(id).copied(),
        // Before the first binding (a test): Agentty's own key.
        None => catalog::command(id).and_then(|command| command.keys.first()).map(|keys| keep(&chord::glyphs(keys))),
    }
}

/// Every key of a command, as this system writes shortcuts. Empty: no shortcut.
pub fn shown(id: &str, cx: &App) -> Vec<String> {
    cx.try_global::<Labels>().and_then(|labels| labels.shown.get(id).cloned()).unwrap_or_default()
}

/// A sentence that names a shortcut by Agentty's own key ("Press ⌘N to create one"), with the
/// key the command has now. A command left without a shortcut keeps the sentence as written.
pub fn in_text(text: &str, cx: &App) -> String {
    let mut text = text.to_string();
    for id in ["chat_view", "new_workspace", "jump_unread", "voice"] {
        let (Some(command), Some(now)) = (catalog::command(id), shown(id, cx).into_iter().next()) else { continue };
        let written = crate::keymap::display(&chord::glyphs(command.keys[0])).into_owned();
        if written != now && text.contains(&written) {
            text = text.replace(&written, &now);
        }
    }
    text
}

/// The other commands that answer to `chord` (as pressed here) where `id` would: giving `id`
/// the chord takes it from them.
pub fn conflicts(id: &str, chord: &str, settings: &ShortcutSettings) -> Vec<&'static Command> {
    let Some(target) = catalog::command(id) else { return Vec::new() };
    resolve(settings)
        .into_iter()
        .filter(|entry| entry.command.id != id && entry.command.context == target.context)
        .filter(|entry| match &entry.keys {
            Keys::Written(keys) => keys.iter().any(|keys| answers_to(&entry.command.written(keys)).iter().any(|a| a == chord)),
            Keys::Pressed(pressed) => pressed == chord,
            Keys::None => false,
        })
        .map(|entry| entry.command)
        .collect()
}

/// Gives `id` the chord (as pressed here; empty for no shortcut). A command of the user's own
/// that had it is left without one; a preset's key simply stops being that command's.
pub fn assign_in(settings: &mut ShortcutSettings, id: &str, chord: &str) {
    if !chord.is_empty() {
        let Some(target) = catalog::command(id) else { return };
        let losers: Vec<&'static str> = COMMANDS
            .iter()
            .filter(|other| other.id != id && other.context == target.context && settings.custom_for(other.id) == Some(chord))
            .map(|other| other.id)
            .collect();
        for loser in losers {
            settings.mine_mut().insert(loser.to_string(), String::new());
        }
    }
    settings.mine_mut().insert(id.to_string(), chord.to_string());
}

fn change(cx: &mut App, change: impl FnOnce(&mut ShortcutSettings)) {
    crate::settings::update_settings(cx, |settings| change(&mut settings.shortcuts));
    crate::rebind_keys(cx);
}

pub fn assign(id: &str, chord: &str, cx: &mut App) {
    change(cx, |settings| assign_in(settings, id, chord));
}

/// Back to the preset's keys for one command.
pub fn reset(id: &str, cx: &mut App) {
    change(cx, |settings| settings.forget(id));
}

/// Whether anything differs from a fresh install on this system: another preset, or a key changed.
pub fn is_default(settings: &ShortcutSettings) -> bool {
    settings.preset == Preset::default() && !settings.changed()
}

/// Everything as a fresh install has it: Agentty's preset, and none of this system's own keys.
pub fn reset_all(cx: &mut App) {
    change(cx, |settings| {
        settings.preset = Preset::default();
        settings.custom.remove(std::env::consts::OS);
    });
}

pub fn set_preset(preset: Preset, cx: &mut App) {
    change(cx, |settings| settings.preset = preset);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workbench::*;
    use crate::{git_view, key, NewWindow};

    fn summary(bindings: &[KeyBinding]) -> Vec<(String, String, Option<String>)> {
        let mut all: Vec<_> = bindings
            .iter()
            .map(|b| (chord::of_binding(b).unwrap_or_default(), b.action().name().to_string(), b.predicate().map(|p| format!("{p:?}"))))
            .collect();
        all.sort();
        all
    }

    /// With Agentty's preset and nothing changed, the bindings are exactly the ones `main.rs`
    /// listed before shortcuts could be changed. This list is that one, copied; it is not to be
    /// "kept in step" with the catalog: a difference means a default shortcut changed.
    #[test]
    fn the_default_bindings_are_what_they_always_were() {
        let before = [
            key("cmd-shift-n", NewWindow, None),
            key("cmd-t", NewTerminalTab, None),
            key("alt-cmd-c", NewClaudeTab, None),
            key("alt-cmd-x", NewCodexTab, None),
            key("cmd-n", NewTerminalTab, None),
            key("cmd-n", NewWorkspace, Some("Sidebar")),
            key("cmd-w", ClosePane, None),
            key("cmd-shift-w", CloseTab, None),
            key("cmd-d", SplitRight, None),
            key("cmd-shift-d", SplitDown, None),
            key("cmd-]", NextPane, None),
            key("cmd-[", PreviousPane, None),
            key("cmd-shift-]", NextTab, None),
            key("cmd-shift-[", PreviousTab, None),
            key("ctrl-tab", NextTab, None),
            key("ctrl-shift-tab", PreviousTab, None),
            key("cmd-alt-down", NextWorkspace, None),
            key("cmd-alt-up", PreviousWorkspace, None),
            key("cmd-b", ToggleSidebar, None),
            key("cmd-shift-e", ShowWorkspaces, None),
            key("cmd-shift-s", ShowSessions, None),
            key("cmd-shift-f", OpenFlow, None),
            key("cmd-alt-u", OpenUsage, None),
            key("cmd-,", OpenSettings, None),
            key("cmd-shift-x", OpenExtensions, None),
            key("cmd-shift-g", OpenGit, None),
            key("ctrl-cmd-m", ToggleMini, None),
            key("cmd-shift-b", ToggleBrowser, None),
            key("cmd-shift-r", HardReloadBrowser, None),
            key("cmd-alt-b", ToggleFiles, None),
            key("cmd-p", GoToFile, None),
            key("cmd-shift-u", JumpToUnread, None),
            key("cmd-shift-p", OpenPalette, None),
            key("cmd-shift-enter", ToggleZoom, None),
            key("cmd-shift-j", ToggleChatView, None),
            key("cmd-shift-o", SearchSessions, None),
            key("cmd-enter", git_view::CommitChanges, Some("GitView")),
            key("cmd-p", git_view::RemoteAction, Some("GitView")),
            key("cmd-shift-t", git_view::Fetch, Some("GitView")),
            key("cmd-r", git_view::Refresh, Some("GitView")),
        ];
        let mut before = before.to_vec();
        if voice_input_available() {
            before.push(key("cmd-shift-m", ToggleVoiceInput, None));
        }
        let now = bindings_of(&resolve(&ShortcutSettings::default()));
        assert_eq!(summary(&now), summary(&before));
        // The menu bar shows an action's first binding: ⌘T for a new tab, ⇧⌘] for the next tab.
        let first = |name: &str| now.iter().find(|b| b.action().name() == name).and_then(chord::of_binding);
        assert_eq!(first(gpui::Action::name(&NewTerminalTab)), chord::of_binding(&key("cmd-t", NewTerminalTab, None)));
        assert_eq!(first(gpui::Action::name(&NextTab)), chord::of_binding(&key("cmd-shift-]", NextTab, None)));
    }

    fn keys_of(settings: &ShortcutSettings, id: &str) -> Keys {
        resolve(settings).into_iter().find(|entry| entry.command.id == id).map(|entry| entry.keys).unwrap_or(Keys::None)
    }

    /// The chord this system binds for a key written for macOS.
    fn here(keys: &'static str) -> String {
        chord::of_binding(&key(keys, NewWindow, None)).unwrap()
    }

    #[test]
    fn a_chord_given_to_one_command_leaves_the_other() {
        let mut settings = ShortcutSettings::default();
        let taken = here("cmd-d");
        // Split Right has ⌘D: giving it to the palette says so first, then takes it.
        let losing: Vec<&str> = conflicts("palette", &taken, &settings).iter().map(|c| c.id).collect();
        assert_eq!(losing, ["split_right"]);
        assign_in(&mut settings, "palette", &taken);
        assert_eq!(keys_of(&settings, "palette"), Keys::Pressed(taken.clone()));
        assert_eq!(keys_of(&settings, "split_right"), Keys::None);
        assert!(conflicts("palette", &taken, &settings).is_empty());
        // One of two keys taken: the command keeps the other.
        assign_in(&mut settings, "flow", &here("ctrl-tab"));
        assert_eq!(keys_of(&settings, "next_tab"), Keys::Written(vec!["cmd-shift-]"]));
        // Taken again by a third: the second, the user's own, is left with none.
        assign_in(&mut settings, "git", &taken);
        assert_eq!(keys_of(&settings, "palette"), Keys::None);
        assert_eq!(settings.custom_for("palette"), Some(""));
        // Back to the preset: everything it gave returns.
        settings.forget("git");
        settings.forget("palette");
        settings.forget("flow");
        assert_eq!(settings, ShortcutSettings::default());
        assert_eq!(keys_of(&settings, "split_right"), Keys::Written(vec!["cmd-d"]));
    }

    /// The same chord in another context is another shortcut: ⌘N in the workspace list, ⌘P on
    /// the Git page.
    #[test]
    fn contexts_keep_their_own_keys() {
        let settings = ShortcutSettings::default();
        let ids = |id: &str, keys: &'static str| conflicts(id, &here(keys), &settings).iter().map(|c| c.id).collect::<Vec<_>>();
        assert_eq!(ids("split_right", "cmd-n"), ["new_tab_here"]);
        assert_eq!(ids("git_refresh", "cmd-p"), ["git_push"]);
        assert_eq!(ids("split_right", "cmd-p"), ["go_to_file"]);
    }

    /// A written Shift + symbol and the character gpui reports for it are one chord.
    #[test]
    fn a_shifted_symbol_is_found_as_it_is_pressed() {
        if !cfg!(target_os = "macos") {
            return;
        }
        let settings = ShortcutSettings::default();
        let ids: Vec<&str> = conflicts("palette", "cmd-}", &settings).iter().map(|c| c.id).collect();
        assert_eq!(ids, ["next_tab"]);
    }

    #[test]
    fn a_preset_changes_only_what_it_names() {
        let settings = ShortcutSettings { preset: Preset::VsCode, ..Default::default() };
        assert_eq!(keys_of(&settings, "split_right"), Keys::Written(vec!["cmd-\\"]));
        let Keys::Written(new_terminal) = keys_of(&settings, "new_terminal") else { panic!("new_terminal has keys") };
        assert_eq!(new_terminal.last(), Some(&"cmd-t"));
        assert_eq!(keys_of(&settings, "palette"), Keys::Written(vec!["cmd-shift-p"]));
        // The user's own key stays through a change of preset.
        let mut settings = settings;
        assign_in(&mut settings, "split_right", &here("cmd-d"));
        settings.preset = Preset::Agentty;
        assert_eq!(keys_of(&settings, "split_right"), Keys::Pressed(here("cmd-d")));
    }

    #[test]
    fn the_settings_file_keeps_only_what_changed() {
        assert_eq!(serde_json::to_string(&ShortcutSettings::default()).unwrap(), r#"{"preset":"agentty"}"#);
        let mut settings = ShortcutSettings::default();
        assign_in(&mut settings, "palette", "cmd-shift-k");
        let back: ShortcutSettings = serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        assert_eq!(back, settings);
        assert!(back.changed());
        // A chord nothing can press is left out instead of stopping the app at start.
        let broken: ShortcutSettings = serde_json::from_str(&format!(
            r#"{{"custom":{{"{}":{{"palette":"not a key","git":"cmd-shift-nothing-k","flow":"cmd-shift-k"}}}}}}"#,
            std::env::consts::OS
        ))
        .unwrap();
        assert_eq!(broken.custom_for("palette"), None);
        assert_eq!(broken.custom_for("git"), None);
        assert_eq!(keys_of(&broken, "flow"), Keys::Pressed("cmd-shift-k".into()));
        assert_eq!(bindings_of(&resolve(&broken)).len(), bindings_of(&resolve(&ShortcutSettings::default())).len());
        // A preset this version does not know reads as Agentty's, not as a broken settings file.
        let unknown: ShortcutSettings = serde_json::from_str(r#"{"preset":"emacs"}"#).unwrap();
        assert_eq!(unknown.preset, Preset::Agentty);
    }

    /// Every command has a name and a group the settings screen lists, and an id of its own.
    #[test]
    fn the_catalog_is_whole() {
        for (i, command) in COMMANDS.iter().enumerate() {
            assert!(GROUPS.contains(&command.group), "{}", command.id);
            assert!(!command.keys.is_empty(), "{}", command.id);
            assert!(COMMANDS[..i].iter().all(|other| other.id != command.id), "{} twice", command.id);
            assert!(crate::i18n::lookup(command.label).is_some(), "{} has no name", command.label);
        }
    }
}
