//! Translates GPUI keystrokes into the byte sequences a terminal application expects.
//! Printable text is not handled here; it arrives through the platform input handler (IME aware).

use alacritty_terminal::term::TermMode;
use gpui::Keystroke;

/// `agent`: Claude Code or Codex runs in the foreground, whose prompts read some keys differently
/// from a shell.
pub fn to_escape(keystroke: &Keystroke, mode: TermMode, option_as_meta: bool, agent: bool) -> Option<Vec<u8>> {
    let m = keystroke.modifiers;
    if m.platform {
        // The Mac's line-editing shortcuts, sent the way iTerm2 and Ghostty send them: ⌘⌫ deletes
        // to the start of the line (^U), ⌘⌦ to its end (^K), ⌘← / ⌘→ go to its start and end
        // (^A / ^E). Every other Cmd shortcut belongs to the app.
        if cfg!(target_os = "macos") && !m.control && !m.alt && !m.shift {
            match keystroke.key.as_str() {
                "backspace" => return Some(vec![0x15]),
                "delete" => return Some(vec![0x0b]),
                "left" => return Some(vec![0x01]),
                "right" => return Some(vec![0x05]),
                _ => {}
            }
        }
        return None;
    }
    // Alt is Meta on Windows and Linux; on the Mac, Option types characters (é, ∫) unless the
    // setting makes it Meta.
    let meta = option_as_meta || !cfg!(target_os = "macos");
    let app_cursor = mode.contains(TermMode::APP_CURSOR);
    let key = keystroke.key.as_str();

    // xterm modifier parameter: 1 + shift(1) + alt(2) + ctrl(4)
    let modifier_param = 1 + m.shift as u8 + 2 * m.alt as u8 + 4 * m.control as u8;
    let csi = |final_byte: char| -> Vec<u8> {
        if modifier_param > 1 {
            format!("\x1b[1;{modifier_param}{final_byte}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{final_byte}").into_bytes()
        } else {
            format!("\x1b[{final_byte}").into_bytes()
        }
    };
    let tilde = |code: u8| -> Vec<u8> {
        if modifier_param > 1 {
            format!("\x1b[{code};{modifier_param}~").into_bytes()
        } else {
            format!("\x1b[{code}~").into_bytes()
        }
    };

    let bytes = match key {
        "enter" => {
            // Shift+Enter inserts a newline in agent prompts, like Option+Enter (Meta-Return).
            if m.alt || m.shift {
                b"\x1b\r".to_vec()
            } else {
                b"\r".to_vec()
            }
        }
        "tab" => {
            if m.shift {
                b"\x1b[Z".to_vec()
            } else {
                b"\t".to_vec()
            }
        }
        "escape" => b"\x1b".to_vec(),
        "backspace" => match (m.control, m.alt) {
            // Ctrl+Backspace deletes a word. ^H does that in PowerShell (ConPTY reads it as
            // Ctrl+Backspace), but the agents read it as one Backspace; ^W is their word delete.
            (true, _) if agent => b"\x17".to_vec(),
            (true, _) => b"\x08".to_vec(),
            (false, true) => b"\x1b\x7f".to_vec(),
            _ => b"\x7f".to_vec(),
        },
        "space" if m.control => b"\x00".to_vec(),
        "up" => csi('A'),
        "down" => csi('B'),
        "right" => csi('C'),
        "left" => csi('D'),
        "home" => csi('H'),
        "end" => csi('F'),
        "insert" => tilde(2),
        // ⌥⌦ / Alt+Delete deletes the next word (Meta-d), as iTerm2 sends it.
        "delete" if m.alt && !m.control && !m.shift => b"\x1bd".to_vec(),
        "delete" => tilde(3),
        "pageup" => tilde(5),
        "pagedown" => tilde(6),
        "f1" => csi_ss3('P', modifier_param),
        "f2" => csi_ss3('Q', modifier_param),
        "f3" => csi_ss3('R', modifier_param),
        "f4" => csi_ss3('S', modifier_param),
        "f5" => tilde(15),
        "f6" => tilde(17),
        "f7" => tilde(18),
        "f8" => tilde(19),
        "f9" => tilde(20),
        "f10" => tilde(21),
        "f11" => tilde(23),
        "f12" => tilde(24),
        _ if m.control && key.chars().count() == 1 => {
            let byte = control_byte(key.chars().next()?)?;
            // Ctrl+Alt+letter is Meta-Ctrl: Escape first.
            if m.alt && meta {
                [vec![0x1b], byte].concat()
            } else {
                byte
            }
        }
        _ if m.alt && meta && key.chars().count() == 1 => {
            let mut bytes = vec![0x1b];
            let ch = if m.shift { key.to_uppercase() } else { key.to_string() };
            bytes.extend_from_slice(ch.as_bytes());
            bytes
        }
        _ => return None,
    };
    Some(bytes)
}

/// Windows: a character typed with AltGr (`@` is AltGr+2 on a Swiss or German keyboard), as
/// ConPTY's win32-input-mode key presses with Right Alt and Left Ctrl held, the way Windows
/// Terminal sends it. Written as plain text, ConPTY makes the key event up from the layout with
/// Ctrl and Alt and no AltGr, and the agents' prompts read AltGr+2 as Ctrl+Alt+2 and drop it.
/// `None` when the character is not typed with AltGr or AltGr is not held now.
#[cfg(windows)]
pub fn altgr_text(text: &str) -> Option<Vec<u8>> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, MapVirtualKeyW, VkKeyScanW, MAPVK_VK_TO_VSC, VK_LCONTROL, VK_RMENU};
    let mut chars = text.chars();
    let (ch, None) = (chars.next()?, chars.next()) else { return None };
    let unit = u16::try_from(u32::from(ch)).ok()?;
    // SAFETY: keyboard state and layout queries on the thread that received the text.
    let held = |vk: u16| unsafe { GetKeyState(vk as i32) } < 0;
    if !(held(VK_RMENU.0) && held(VK_LCONTROL.0)) {
        return None;
    }
    let (vk, shift) = altgr_key(unsafe { VkKeyScanW(unit) })?;
    let scan = unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) } as u16;
    Some(win32_key_press(vk, scan, unit, shift))
}

/// The key of a `VkKeyScanW` answer and whether Shift is part of it, when the character takes
/// Ctrl+Alt (AltGr) on the layout.
#[cfg_attr(not(windows), allow(dead_code))]
fn altgr_key(scan: i16) -> Option<(u16, bool)> {
    if scan == -1 {
        return None;
    }
    let (vk, shifts) = ((scan as u16) & 0xff, ((scan as u16) >> 8) & 0xff);
    // Shift 1, Ctrl 2, Alt 4.
    (shifts & 6 == 6).then_some((vk, shifts & 1 == 1))
}

/// A key pressed and released in win32-input-mode (`CSI Vk;Sc;Uc;Kd;Cs;Rc _`), with AltGr held.
#[cfg_attr(not(windows), allow(dead_code))]
fn win32_key_press(vk: u16, scan: u16, unit: u16, shift: bool) -> Vec<u8> {
    const RIGHT_ALT_PRESSED: u32 = 0x0001;
    const LEFT_CTRL_PRESSED: u32 = 0x0008;
    const SHIFT_PRESSED: u32 = 0x0010;
    let state = RIGHT_ALT_PRESSED | LEFT_CTRL_PRESSED | if shift { SHIFT_PRESSED } else { 0 };
    format!("\x1b[{vk};{scan};{unit};1;{state};1_\x1b[{vk};{scan};{unit};0;{state};1_").into_bytes()
}

fn csi_ss3(final_byte: char, modifier_param: u8) -> Vec<u8> {
    if modifier_param > 1 {
        format!("\x1b[1;{modifier_param}{final_byte}").into_bytes()
    } else {
        format!("\x1bO{final_byte}").into_bytes()
    }
}

fn control_byte(ch: char) -> Option<Vec<u8>> {
    let byte = match ch.to_ascii_lowercase() {
        c @ 'a'..='z' => c as u8 - b'a' + 1,
        '@' | '2' => 0x00,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' => 0x1e,
        // Ctrl+/ is undo in readline and the agents' prompts, like Ctrl+_.
        '_' | '-' | '/' | '7' => 0x1f,
        '8' | '?' => 0x7f,
        _ => return None,
    };
    Some(vec![byte])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ks(s: &str) -> Keystroke {
        Keystroke::parse(s).unwrap()
    }

    #[test]
    fn control_letters() {
        assert_eq!(to_escape(&ks("ctrl-c"), TermMode::empty(), false, false), Some(vec![3]));
    }

    #[test]
    fn arrows_follow_cursor_mode() {
        assert_eq!(to_escape(&ks("up"), TermMode::empty(), false, false), Some(b"\x1b[A".to_vec()));
        assert_eq!(to_escape(&ks("up"), TermMode::APP_CURSOR, false, false), Some(b"\x1bOA".to_vec()));
        assert_eq!(to_escape(&ks("shift-up"), TermMode::empty(), false, false), Some(b"\x1b[1;2A".to_vec()));
    }

    #[test]
    fn shift_enter_is_a_newline_not_submit() {
        assert_eq!(to_escape(&ks("enter"), TermMode::empty(), false, false), Some(b"\r".to_vec()));
        assert_eq!(to_escape(&ks("shift-enter"), TermMode::empty(), false, false), Some(b"\x1b\r".to_vec()));
        assert_eq!(to_escape(&ks("alt-enter"), TermMode::empty(), false, false), Some(b"\x1b\r".to_vec()));
    }

    #[test]
    fn altgr_characters_go_as_win32_key_presses() {
        // `VkKeyScanW('@')` on Swiss German: the 2 key (0x32) with Ctrl+Alt; `{` on US: Shift+[.
        assert_eq!(altgr_key(0x0632), Some((0x32, false)));
        assert_eq!(altgr_key(0x0732), Some((0x32, true)));
        assert_eq!(altgr_key(0x01db), None);
        assert_eq!(altgr_key(-1), None);
        // Down and up, Right Alt and Left Ctrl held, as Windows Terminal sends them.
        assert_eq!(win32_key_press(0x32, 3, '@' as u16, false), b"\x1b[50;3;64;1;9;1_\x1b[50;3;64;0;9;1_".to_vec());
        assert_eq!(win32_key_press(0x32, 3, '@' as u16, true), b"\x1b[50;3;64;1;25;1_\x1b[50;3;64;0;25;1_".to_vec());
    }

    #[test]
    fn option_edits_words() {
        assert_eq!(to_escape(&ks("alt-backspace"), TermMode::empty(), false, false), Some(b"\x1b\x7f".to_vec()));
        assert_eq!(to_escape(&ks("alt-left"), TermMode::empty(), false, false), Some(b"\x1b[1;3D".to_vec()));
        assert_eq!(to_escape(&ks("alt-right"), TermMode::APP_CURSOR, false, false), Some(b"\x1b[1;3C".to_vec()));
    }

    #[test]
    fn alt_is_meta_off_the_mac() {
        let meta = !cfg!(target_os = "macos");
        let expect = |bytes: &[u8]| meta.then(|| bytes.to_vec());
        assert_eq!(to_escape(&ks("alt-b"), TermMode::empty(), false, false), expect(b"\x1bb"));
        assert_eq!(to_escape(&ks("alt-shift-p"), TermMode::empty(), false, true), expect(b"\x1bP"));
        // The Mac's setting turns Option into Meta.
        assert_eq!(to_escape(&ks("alt-f"), TermMode::empty(), true, false), Some(b"\x1bf".to_vec()));
        assert_eq!(to_escape(&ks("ctrl-alt-h"), TermMode::empty(), true, false), Some(vec![0x1b, 0x08]));
        assert_eq!(to_escape(&ks("ctrl-h"), TermMode::empty(), true, false), Some(vec![0x08]));
    }

    #[test]
    fn word_and_undo_keys() {
        assert_eq!(to_escape(&ks("alt-delete"), TermMode::empty(), false, false), Some(b"\x1bd".to_vec()));
        assert_eq!(to_escape(&ks("delete"), TermMode::empty(), false, false), Some(b"\x1b[3~".to_vec()));
        assert_eq!(to_escape(&ks("ctrl-/"), TermMode::empty(), false, false), Some(vec![0x1f]));
        // Ctrl+Backspace: a word in the shell's terms (^H) and in the agents' (^W).
        assert_eq!(to_escape(&ks("ctrl-backspace"), TermMode::empty(), false, false), Some(vec![0x08]));
        assert_eq!(to_escape(&ks("ctrl-backspace"), TermMode::empty(), false, true), Some(vec![0x17]));
        assert_eq!(to_escape(&ks("backspace"), TermMode::empty(), false, true), Some(vec![0x7f]));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn cmd_edits_the_line() {
        assert_eq!(to_escape(&ks("cmd-delete"), TermMode::empty(), false, false), Some(vec![0x0b]));
        assert_eq!(to_escape(&ks("cmd-backspace"), TermMode::empty(), false, false), Some(vec![0x15]));
        assert_eq!(to_escape(&ks("cmd-left"), TermMode::empty(), false, false), Some(vec![0x01]));
        assert_eq!(to_escape(&ks("cmd-right"), TermMode::APP_CURSOR, false, false), Some(vec![0x05]));
        // Shortcuts with more modifiers stay the app's.
        assert_eq!(to_escape(&ks("cmd-shift-left"), TermMode::empty(), false, false), None);
        assert_eq!(to_escape(&ks("cmd-alt-left"), TermMode::empty(), false, false), None);
    }

    #[test]
    fn printable_and_cmd_are_ignored() {
        assert_eq!(to_escape(&ks("a"), TermMode::empty(), false, false), None);
        assert_eq!(to_escape(&ks("cmd-c"), TermMode::empty(), false, false), None);
    }
}
