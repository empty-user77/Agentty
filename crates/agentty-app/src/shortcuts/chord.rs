//! One key press with its modifiers: how a pressed one is written down, compared, shown and
//! judged fit to be a shortcut.

use gpui::{KeyBinding, Keystroke, Modifiers};

/// The keys of a press as `KeyBinding` reads them back (`ctrl-alt-shift-cmd-t`): the same order
/// `keymap` writes, so a pressed chord and a written one compare as text.
pub fn written(modifiers: &Modifiers, key: &str) -> String {
    let mut out = String::new();
    for (on, name) in [
        (modifiers.control, "ctrl-"),
        (modifiers.alt, "alt-"),
        (modifiers.shift, "shift-"),
        (modifiers.platform, "cmd-"),
        (modifiers.function, "fn-"),
    ] {
        if on {
            out.push_str(name);
        }
    }
    out.push_str(key);
    out
}

/// What a binding answers to, for telling two bindings on the same keys apart from two that only
/// look alike (`cmd-shift-[` as written, `cmd-{` as gpui reports it).
pub fn of_binding(binding: &KeyBinding) -> Option<String> {
    let [keystroke] = binding.keystrokes() else { return None };
    let keystroke = keystroke.inner();
    Some(written(&keystroke.modifiers, &keystroke.key))
}

/// Why a press cannot be a shortcut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// A key with nothing held (or Shift alone) types text.
    NeedsModifier,
    /// Ctrl with a letter is the shell's (^C, ^D, ^R…).
    TerminalKey,
    /// Agentty or the system uses it for something that cannot move.
    Reserved,
}

impl Refusal {
    pub fn message(self) -> &'static str {
        match self {
            Self::NeedsModifier => "keys.refused.modifier",
            Self::TerminalKey => "keys.refused.terminal",
            Self::Reserved => "keys.refused.reserved",
        }
    }
}

/// Keys that keep their meaning, written for macOS: copy, paste and the rest of text editing,
/// the terminal's own, the numbered tabs and workspaces, font size, find, quit and what macOS
/// itself answers to. A shortcut on one of them would either never fire or take the key away
/// from typing.
pub const RESERVED: &[&str] = &[
    "cmd-c",
    "cmd-v",
    "cmd-x",
    "cmd-a",
    "cmd-z",
    "cmd-shift-z",
    "cmd-k",
    "cmd-f",
    "cmd-s",
    "cmd-q",
    "cmd-h",
    "alt-cmd-h",
    "cmd-m",
    "cmd-=",
    "cmd-+",
    "cmd--",
    "cmd-0",
    "cmd-1",
    "cmd-2",
    "cmd-3",
    "cmd-4",
    "cmd-5",
    "cmd-6",
    "cmd-7",
    "cmd-8",
    "cmd-9",
    "ctrl-1",
    "ctrl-2",
    "ctrl-3",
    "ctrl-4",
    "ctrl-5",
    "ctrl-6",
    "ctrl-7",
    "ctrl-8",
    "ctrl-9",
    "cmd-tab",
    "cmd-space",
    "cmd-`",
];

/// The reserved keys as this system has them (`cmd-c` is `ctrl-shift-c` on Windows and Linux).
pub fn reserved_here() -> Vec<String> {
    RESERVED.iter().map(|keys| crate::keymap::binding(keys, None).into_owned()).collect()
}

/// Whether a pressed chord may be a shortcut. `reserved` is [`reserved_here`].
pub fn judge(modifiers: &Modifiers, key: &str, reserved: &[String]) -> Result<(), Refusal> {
    let function_key = key.len() > 1 && key.starts_with('f') && key[1..].chars().all(|c| c.is_ascii_digit());
    if !(modifiers.control || modifiers.alt || modifiers.platform || function_key) {
        return Err(Refusal::NeedsModifier);
    }
    // Ctrl alone with a letter or one of the punctuation keys that make a control character.
    let only_ctrl = modifiers.control && !modifiers.alt && !modifiers.platform && !modifiers.shift;
    let control_character = key.len() == 1 && key.chars().all(|c| c.is_ascii_alphabetic() || "[]\\^_@ ".contains(c));
    if only_ctrl && (control_character || key == "space") {
        return Err(Refusal::TerminalKey);
    }
    if reserved.contains(&written(modifiers, key)) {
        return Err(Refusal::Reserved);
    }
    Ok(())
}

/// A pressed keystroke as a chord, or `None` for a press that is only a modifier going down.
pub fn of_keystroke(keystroke: &Keystroke) -> Option<String> {
    let key = keystroke.key.as_str();
    if key.is_empty() || matches!(key, "shift" | "control" | "alt" | "platform" | "function" | "cmd" | "ctrl" | "fn" | "capslock") {
        return None;
    }
    Some(written(&keystroke.modifiers, key))
}

/// Splits `alt-cmd-c` / `cmd--` into the modifiers held and the key.
pub fn parse(keys: &str) -> (Modifiers, &str) {
    let mut modifiers = Modifiers::default();
    let mut rest = keys;
    while let Some((modifier, tail)) = rest.split_once('-') {
        if tail.is_empty() {
            break; // `cmd--`: the key is `-`
        }
        match modifier {
            "ctrl" => modifiers.control = true,
            "alt" => modifiers.alt = true,
            "shift" => modifiers.shift = true,
            "cmd" => modifiers.platform = true,
            "fn" => modifiers.function = true,
            _ => break,
        }
        rest = tail;
    }
    (modifiers, rest)
}

/// A key's name on a label: `↩` on macOS, `Enter` elsewhere; a letter in capitals.
fn key_label(key: &str, mac: bool) -> String {
    let named = match (key, mac) {
        ("enter", true) => "↩",
        ("enter", false) => "Enter",
        ("up", true) => "↑",
        ("down", true) => "↓",
        ("left", true) => "←",
        ("right", true) => "→",
        ("up", false) => "Up",
        ("down", false) => "Down",
        ("left", false) => "Left",
        ("right", false) => "Right",
        ("backspace", true) => "⌫",
        ("delete", true) => "⌦",
        ("escape", _) => "Esc",
        ("tab", _) => "Tab",
        ("space", _) => "Space",
        ("backspace", false) => "Backspace",
        ("delete", false) => "Delete",
        ("home", _) => "Home",
        ("end", _) => "End",
        ("pageup", _) => "PageUp",
        ("pagedown", _) => "PageDown",
        _ => "",
    };
    if !named.is_empty() {
        return named.to_string();
    }
    key.to_uppercase()
}

/// A chord with macOS's glyphs (`⇧⌘T`), the form every shortcut label in the app is written in:
/// `keymap::display` shows it as this system writes shortcuts.
pub fn glyphs(keys: &str) -> String {
    let (modifiers, key) = parse(keys);
    let mut out = String::new();
    for (on, glyph) in [(modifiers.control, '⌃'), (modifiers.alt, '⌥'), (modifiers.shift, '⇧'), (modifiers.platform, '⌘')] {
        if on {
            out.push(glyph);
        }
    }
    out.push_str(&key_label(key, true));
    out
}

/// A chord pressed on this system, as this system writes shortcuts: glyphs on macOS,
/// `Ctrl+Shift+T` elsewhere. Nothing is translated: these are the keys that were held.
pub fn pressed_label(keys: &str) -> String {
    if cfg!(target_os = "macos") {
        return glyphs(keys);
    }
    let (modifiers, key) = parse(keys);
    let mut parts: Vec<String> = Vec::new();
    for (on, name) in [(modifiers.control, "Ctrl"), (modifiers.alt, "Alt"), (modifiers.shift, "Shift"), (modifiers.platform, "Super")] {
        if on {
            parts.push(name.to_string());
        }
    }
    parts.push(key_label(key, false));
    parts.join("+")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held(ctrl: bool, alt: bool, shift: bool, cmd: bool) -> Modifiers {
        Modifiers { control: ctrl, alt, shift, platform: cmd, function: false }
    }

    #[test]
    fn a_chord_is_written_the_way_bindings_are() {
        assert_eq!(written(&held(true, true, true, true), "t"), "ctrl-alt-shift-cmd-t");
        assert_eq!(written(&held(false, false, false, true), "-"), "cmd--");
        let (modifiers, key) = parse("alt-cmd-c");
        assert_eq!((modifiers, key), (held(false, true, false, true), "c"));
        assert_eq!(parse("cmd--").1, "-");
        assert_eq!(parse("ctrl-shift-tab").1, "tab");
    }

    #[test]
    fn typing_keys_and_the_shells_keys_are_refused() {
        let none: Vec<String> = Vec::new();
        assert_eq!(judge(&held(false, false, false, false), "t", &none), Err(Refusal::NeedsModifier));
        assert_eq!(judge(&held(false, false, true, false), "t", &none), Err(Refusal::NeedsModifier));
        assert_eq!(judge(&held(true, false, false, false), "c", &none), Err(Refusal::TerminalKey));
        assert_eq!(judge(&held(true, false, false, false), "[", &none), Err(Refusal::TerminalKey));
        assert_eq!(judge(&held(true, false, false, false), "`", &none), Ok(()));
        assert_eq!(judge(&held(true, false, true, false), "g", &none), Ok(()));
        assert_eq!(judge(&held(false, false, false, false), "f5", &none), Ok(()));
        assert_eq!(judge(&held(false, false, false, true), "t", &none), Ok(()));
        let reserved = vec!["cmd-c".to_string()];
        assert_eq!(judge(&held(false, false, false, true), "c", &reserved), Err(Refusal::Reserved));
    }

    #[test]
    fn labels_use_the_systems_own_names() {
        assert_eq!(glyphs("cmd-shift-t"), "⇧⌘T");
        assert_eq!(glyphs("ctrl-alt-shift-cmd-enter"), "⌃⌥⇧⌘↩");
        assert_eq!(glyphs("cmd-alt-right"), "⌥⌘→");
        assert_eq!(glyphs("ctrl-`"), "⌃`");
        assert_eq!(glyphs("ctrl-tab"), "⌃Tab");
        assert_eq!(glyphs("cmd--"), "⌘-");
        if !cfg!(target_os = "macos") {
            assert_eq!(pressed_label("ctrl-shift-t"), "Ctrl+Shift+T");
            assert_eq!(pressed_label("ctrl-alt-enter"), "Ctrl+Alt+Enter");
        }
    }
}
