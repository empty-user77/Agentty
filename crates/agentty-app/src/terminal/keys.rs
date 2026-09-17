//! Translates GPUI keystrokes into the byte sequences a terminal application expects.
//! Printable text is not handled here; it arrives through the platform input handler (IME aware).

use alacritty_terminal::term::TermMode;
use gpui::Keystroke;

pub fn to_escape(keystroke: &Keystroke, mode: TermMode, option_as_meta: bool) -> Option<Vec<u8>> {
    let m = keystroke.modifiers;
    if m.platform {
        return None; // Cmd shortcuts belong to the app.
    }
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
            if m.alt {
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
        _ if m.control && key.chars().count() == 1 => control_byte(key.chars().next()?)?,
        _ if m.alt && option_as_meta && key.chars().count() == 1 => {
            let mut bytes = vec![0x1b];
            let ch = if m.shift { key.to_uppercase() } else { key.to_string() };
            bytes.extend_from_slice(ch.as_bytes());
            bytes
        }
        _ => return None,
    };
    Some(bytes)
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
        '_' | '-' | '7' => 0x1f,
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
        assert_eq!(to_escape(&ks("ctrl-c"), TermMode::empty(), false), Some(vec![3]));
    }

    #[test]
    fn arrows_follow_cursor_mode() {
        assert_eq!(to_escape(&ks("up"), TermMode::empty(), false), Some(b"\x1b[A".to_vec()));
        assert_eq!(to_escape(&ks("up"), TermMode::APP_CURSOR, false), Some(b"\x1bOA".to_vec()));
        assert_eq!(to_escape(&ks("shift-up"), TermMode::empty(), false), Some(b"\x1b[1;2A".to_vec()));
    }

    #[test]
    fn printable_and_cmd_are_ignored() {
        assert_eq!(to_escape(&ks("a"), TermMode::empty(), false), None);
        assert_eq!(to_escape(&ks("cmd-c"), TermMode::empty(), false), None);
    }
}
