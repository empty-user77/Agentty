//! Keyboard shortcuts per platform. Bindings and the shortcut labels shown in the UI are written
//! once, macOS style (`cmd-t`, `⌘T`). On macOS they are used as written. On Windows and Linux
//! the ⌘ key doesn't exist (GPUI's `cmd` would be the Windows / Super key), and a terminal must
//! leave Ctrl+letter to the shell, so they are translated the way Windows Terminal and GNOME
//! Terminal do it:
//!
//! | macOS          | Windows / Linux      |
//! |----------------|----------------------|
//! | `cmd-X`        | `ctrl-shift-X`       |
//! | `cmd-shift-X`  | `ctrl-alt-shift-X`   |
//! | `alt-cmd-X`    | `ctrl-alt-X`         |
//! | `ctrl-cmd-X`   | `ctrl-alt-shift-X`   |
//! | `cmd-1`…`9`    | `alt-1`…`9`          |
//! | `cmd-=` `-` `0`| `ctrl-=` `-` `0`     |
//!
//! Text fields (not terminals) use plain Ctrl (`cmd-c` → `ctrl-c`), and ⌘-click becomes
//! Ctrl-click.

use std::borrow::Cow;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Chord {
    ctrl: bool,
    alt: bool,
    shift: bool,
    cmd: bool,
}

/// Contexts whose bindings are plain text editing (Ctrl+C copies there, there is no shell).
const TEXT_CONTEXTS: &[&str] = &["TextInput", "CodeEditor"];

/// Splits `alt-cmd-c` / `cmd--` into modifiers and key.
fn parse(keys: &str) -> (Chord, &str) {
    let mut chord = Chord::default();
    let mut rest = keys;
    while let Some((modifier, tail)) = rest.split_once('-') {
        if tail.is_empty() {
            break; // `cmd--`: the key is `-`
        }
        match modifier {
            "ctrl" => chord.ctrl = true,
            "alt" => chord.alt = true,
            "shift" => chord.shift = true,
            "cmd" => chord.cmd = true,
            _ => break,
        }
        rest = tail;
    }
    (chord, rest)
}

fn format(chord: Chord, key: &str) -> String {
    let mut out = String::new();
    for (on, name) in [(chord.ctrl, "ctrl-"), (chord.alt, "alt-"), (chord.shift, "shift-"), (chord.cmd, "cmd-")] {
        if on {
            out.push_str(name);
        }
    }
    out.push_str(key);
    out
}

/// The Windows / Linux chord for a macOS chord.
fn translate(chord: Chord, key: &str, text_field: bool) -> Chord {
    if !chord.cmd {
        return chord;
    }
    let plain = Chord { cmd: false, ..chord };
    if text_field {
        return Chord { ctrl: true, ..plain };
    }
    let only_cmd = plain == Chord::default();
    if only_cmd && matches!(key, "=" | "+" | "-" | "0") {
        return Chord { ctrl: true, ..Chord::default() };
    }
    if only_cmd && key.len() == 1 && key.chars().all(|c| c.is_ascii_digit()) {
        return Chord { alt: true, ..Chord::default() };
    }
    match (chord.ctrl, chord.alt, chord.shift) {
        (false, false, false) => Chord { ctrl: true, shift: true, ..Chord::default() },
        (false, false, true) => Chord { ctrl: true, alt: true, shift: true, cmd: false },
        (false, true, false) => Chord { ctrl: true, alt: true, ..Chord::default() },
        (true, false, false) => Chord { ctrl: true, alt: true, shift: true, cmd: false },
        _ => Chord { ctrl: true, shift: true, ..plain },
    }
}

/// The keystroke string to bind on this platform for a binding written for macOS.
pub fn binding<'a>(keys: &'a str, context: Option<&str>) -> Cow<'a, str> {
    if cfg!(target_os = "macos") {
        return Cow::Borrowed(keys);
    }
    translate_binding(keys, context)
}

fn translate_binding<'a>(keys: &'a str, context: Option<&str>) -> Cow<'a, str> {
    let (chord, key) = parse(keys);
    if !chord.cmd {
        return Cow::Borrowed(keys);
    }
    let text_field = context.is_some_and(|c| TEXT_CONTEXTS.contains(&c));
    Cow::Owned(format(translate(chord, key, text_field), key))
}

/// Whether a click with these modifiers opens links (⌘-click on macOS, Ctrl-click elsewhere).
pub fn link_modifier(modifiers: &gpui::Modifiers) -> bool {
    if cfg!(target_os = "macos") {
        modifiers.platform
    } else {
        modifiers.control
    }
}

/// A shortcut label written with macOS glyphs (`⇧⌘]  /  ⇧⌘[`, `⌘ Click`) as shown on this
/// platform (`Ctrl+Alt+Shift+]  /  …`, `Ctrl+Click`).
pub fn display(text: &str) -> Cow<'_, str> {
    if cfg!(target_os = "macos") || !text.contains(['⌘', '⌥', '⌃', '⇧']) {
        return Cow::Borrowed(text);
    }
    Cow::Owned(translate_display(text, false))
}

/// Like [`display`], for a shortcut of a text editing context (⌘S → Ctrl+S, not Ctrl+Shift+S).
pub fn display_in_text(text: &str) -> Cow<'_, str> {
    if cfg!(target_os = "macos") || !text.contains(['⌘', '⌥', '⌃', '⇧']) {
        return Cow::Borrowed(text);
    }
    Cow::Owned(translate_display(text, true))
}

fn translate_display(text: &str, text_field: bool) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let mut chord = Chord::default();
        let start = i;
        while i < chars.len() {
            match chars[i] {
                '⌃' => chord.ctrl = true,
                '⌥' => chord.alt = true,
                '⇧' => chord.shift = true,
                '⌘' => chord.cmd = true,
                _ => break,
            }
            i += 1;
        }
        if i == start {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        // `⌘ Click` / `⌘-click` / `⌘+클릭`: a modifier-click.
        let mut j = i;
        if j < chars.len() && matches!(chars[j], ' ' | '-' | '+') && chars.get(j + 1).is_some_and(|c| c.is_alphabetic()) {
            let word: String = chars[j + 1..].iter().take_while(|c| c.is_alphabetic()).collect();
            if !word.chars().all(|c| c.is_ascii_uppercase()) || word.len() > 1 {
                out.push_str(if chord.cmd { "Ctrl" } else { "" });
                out.push('+');
                i = j + 1;
                continue;
            }
        }
        let key: String = if j < chars.len() && chars[j].is_ascii_alphanumeric() {
            let run: String = chars[j..].iter().take_while(|c| c.is_ascii_alphanumeric()).collect();
            // `⌘T` is one letter; `Tab` / `Click` are names.
            let first_upper_single = run.len() > 1 && run.chars().all(|c| c.is_ascii_uppercase());
            if first_upper_single {
                run[..1].to_string()
            } else {
                run
            }
        } else if j < chars.len() {
            chars[j].to_string()
        } else {
            String::new()
        };
        j += key.chars().count();
        let key_label = match key.as_str() {
            "↩" => "Enter".to_string(),
            "↓" => "Down".to_string(),
            "↑" => "Up".to_string(),
            "←" => "Left".to_string(),
            "→" => "Right".to_string(),
            other if other.chars().count() == 1 => other.to_uppercase(),
            other => other.to_string(),
        };
        let translated = translate(chord, &key.to_lowercase(), text_field);
        let mut parts: Vec<&str> = Vec::new();
        for (on, name) in [(translated.ctrl, "Ctrl"), (translated.alt, "Alt"), (translated.shift, "Shift"), (translated.cmd, "Super")] {
            if on {
                parts.push(name);
            }
        }
        out.push_str(&parts.join("+"));
        if !key_label.is_empty() {
            out.push('+');
            out.push_str(&key_label);
        }
        i = j;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_bindings() {
        let t = |keys: &str| translate_binding(keys, None).into_owned();
        assert_eq!(t("cmd-t"), "ctrl-shift-t");
        assert_eq!(t("cmd-shift-d"), "ctrl-alt-shift-d");
        assert_eq!(t("alt-cmd-c"), "ctrl-alt-c");
        assert_eq!(t("ctrl-cmd-m"), "ctrl-alt-shift-m");
        assert_eq!(t("cmd-1"), "alt-1");
        assert_eq!(t("cmd--"), "ctrl--");
        assert_eq!(t("cmd-="), "ctrl-=");
        assert_eq!(t("ctrl-tab"), "ctrl-tab");
        assert_eq!(translate_binding("cmd-c", Some("TextInput")), "ctrl-c");
        assert_eq!(parse("cmd--"), (Chord { cmd: true, ..Chord::default() }, "-"));
    }

    #[test]
    fn translated_app_bindings_do_not_collide() {
        // Every chord bound in main.rs, grouped by context.
        let bindings: &[(&str, Option<&str>)] = &[
            ("cmd-q", None),
            ("cmd-shift-n", None),
            ("cmd-t", None),
            ("alt-cmd-c", None),
            ("alt-cmd-x", None),
            ("cmd-n", None),
            ("cmd-w", None),
            ("cmd-shift-w", None),
            ("cmd-d", None),
            ("cmd-shift-d", None),
            ("cmd-]", None),
            ("cmd-[", None),
            ("cmd-shift-]", None),
            ("cmd-shift-[", None),
            ("ctrl-tab", None),
            ("ctrl-shift-tab", None),
            ("cmd-alt-down", None),
            ("cmd-alt-up", None),
            ("cmd-b", None),
            ("cmd-shift-e", None),
            ("cmd-shift-s", None),
            ("cmd-shift-f", None),
            ("cmd-alt-u", None),
            ("cmd-,", None),
            ("cmd-shift-x", None),
            ("cmd-shift-g", None),
            ("ctrl-cmd-m", None),
            ("cmd-shift-b", None),
            ("cmd-alt-b", None),
            ("cmd-f", None),
            ("cmd-=", None),
            ("cmd-+", None),
            ("cmd--", None),
            ("cmd-0", None),
            ("cmd-shift-u", None),
            ("cmd-shift-p", None),
            ("cmd-shift-enter", None),
            ("cmd-shift-o", None),
            ("ctrl-1", None),
            ("ctrl-9", None),
            ("cmd-1", None),
            ("cmd-9", None),
            ("cmd-enter", Some("GitView")),
            ("cmd-p", Some("GitView")),
            ("cmd-shift-t", Some("GitView")),
            ("cmd-r", Some("GitView")),
            ("cmd-c", Some("Terminal")),
            ("cmd-v", Some("Terminal")),
            ("cmd-k", Some("Terminal")),
            ("cmd-a", Some("Terminal")),
        ];
        let translated: Vec<(String, Option<&str>)> =
            bindings.iter().map(|(keys, context)| (translate_binding(keys, *context).into_owned(), *context)).collect();
        for (i, a) in translated.iter().enumerate() {
            for (j, b) in translated.iter().enumerate() {
                // A context binding shadows a global one only inside that context, which is fine;
                // two bindings in the same scope must differ.
                if i < j && a.1 == b.1 {
                    assert_ne!(a.0, b.0, "{} and {} collide", bindings[i].0, bindings[j].0);
                }
            }
        }
        // Terminal copy/paste use the conventional Ctrl+Shift+C / V.
        assert_eq!(translate_binding("cmd-c", Some("Terminal")), "ctrl-shift-c");
    }

    #[test]
    fn displays_shortcuts() {
        assert_eq!(translate_display("⌘T", false), "Ctrl+Shift+T");
        // The file editor is a text field: plain Ctrl.
        assert_eq!(translate_display("⌘S", true), "Ctrl+S");
        assert_eq!(translate_display("⇧⌘Z", true), "Ctrl+Shift+Z");
        assert_eq!(translate_display("⇧⌘]  /  ⇧⌘[  ·  ⌃Tab", false), "Ctrl+Alt+Shift+]  /  Ctrl+Alt+Shift+[  ·  Ctrl+Tab");
        assert_eq!(translate_display("⌥⌘↓", false), "Ctrl+Alt+Down");
        assert_eq!(translate_display("⌃1 … ⌃9", false), "Ctrl+1 … Ctrl+9");
        assert_eq!(translate_display("⌘1 … ⌘9", false), "Alt+1 … Alt+9");
        assert_eq!(translate_display("⌘=  /  ⌘-  /  ⌘0", false), "Ctrl+=  /  Ctrl+-  /  Ctrl+0");
        assert_eq!(translate_display("⇧⌘↩", false), "Ctrl+Alt+Shift+Enter");
        assert_eq!(translate_display("⌘ Click", false), "Ctrl+Click");
        assert_eq!(translate_display("Open links (⌘-click in terminals) in", false), "Open links (Ctrl+click in terminals) in");
        assert_eq!(translate_display("⌘ 클릭하여 링크 열기", false), "Ctrl+클릭하여 링크 열기");
        assert_eq!(translate_display("No workspaces. Press ⌘N to create one.", false), "No workspaces. Press Ctrl+Shift+N to create one.");
        assert_eq!(translate_display("⌘,", false), "Ctrl+Shift+,");
    }
}
