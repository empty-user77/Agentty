//! What the keyboard layout in use types, read from the system's own layout data, so that a key
//! that types a character on that layout is text, not a shortcut or Meta.
//!
//! The bindings and the terminal's Meta key are written for a US layout. On most other layouts
//! Option (macOS) or AltGr (Windows) is how characters are typed: `[ ] { } @ | \ ~` on German,
//! French, Spanish, Italian, Portuguese and the Nordic layouts, `ą ę ł ś ż` on Polish Pro, `ş ğ ı`
//! on Turkish Q. Every keystroke is checked against the active layout, and the answer is
//! refreshed when the layout changes (`refresh`, called from `main`):
//!
//! - **Option on macOS** ([`option_types_text`]): a character is typed when it is ASCII
//!   (punctuation and digits code needs) or a letter of the layout's language; everything else
//!   Option makes (`∫ ƒ π` on a US layout) stays free for Meta.
//! - **⌘ shortcuts on macOS** ([`cmd_key`]): a key a layout cannot type without Option (`[` on
//!   German) moves where macOS's own menus move it (`⌘Ö`), from the system tables gpui carries.
//! - **Ctrl+Alt shortcuts on Windows** ([`altgr_types_text`]): Windows reports AltGr as Ctrl+Alt
//!   on every layout gpui does not list as an AltGr layout; a shortcut on a key whose AltGr types a
//!   character there (Croatian AltGr+B is `{`) is left out, or it would take the character.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use gpui::{App, Keystroke};

#[derive(Default)]
struct Layout {
    /// Characters of the layout's language that are not ASCII (`ą`, `ş`, `ä`), both cases.
    letters: HashSet<char>,
    /// The layout `letters` were read for. The system announces a switch before every part of
    /// it reports the new layout, so the letters are checked against the layout in use when
    /// they are needed.
    letters_for: Option<String>,
    /// macOS: the key a ⌘ shortcut moves to on this layout (`[` → `ö` on German).
    cmd_keys: HashMap<char, char>,
}

thread_local! {
    static LAYOUT: RefCell<Layout> = RefCell::default();
}

/// Reads the active layout again: at startup and whenever the user switches layouts.
pub fn refresh(cx: &App) {
    let cmd_keys = match cx.keyboard_mapper().get_key_equivalents() {
        Some(table) if cfg!(target_os = "macos") => table.iter().map(|(from, to)| (*from, *to)).collect(),
        _ => HashMap::new(),
    };
    let letters_for = platform::layout_id();
    let letters = platform::language_letters();
    LAYOUT.with(|layout| *layout.borrow_mut() = Layout { letters, letters_for, cmd_keys });
}

/// macOS: whether Option with this key types a character the layout's user needs, which then
/// goes in as text even while Option is Meta.
pub fn option_types_text(keystroke: &Keystroke) -> bool {
    LAYOUT.with(|layout| {
        option_text(keystroke, &|ch| {
            let mut layout = layout.borrow_mut();
            let id = platform::layout_id();
            if layout.letters_for != id {
                layout.letters = platform::language_letters();
                layout.letters_for = id;
            }
            layout.letters.contains(&ch)
        })
    })
}

fn option_text(keystroke: &Keystroke, language_letter: &dyn Fn(char) -> bool) -> bool {
    let m = keystroke.modifiers;
    if !m.alt || m.control || m.platform || m.function {
        return false;
    }
    let Some(text) = keystroke.key_char.as_deref() else { return false };
    let mut chars = text.chars();
    let (Some(ch), None) = (chars.next(), chars.next()) else { return false };
    // A key that types itself with Option (US ⌥`) is a Meta chord, not a character.
    if ch.to_lowercase().eq(keystroke.key.chars()) {
        return false;
    }
    ch.is_ascii_graphic() || language_letter(ch)
}

/// macOS: the key a ⌘ shortcut on `key` takes on the active layout, when macOS moves it there.
pub fn cmd_key(key: char) -> Option<char> {
    LAYOUT.with(|layout| layout.borrow().cmd_keys.get(&key).copied())
}

/// Windows: whether Ctrl+Alt (with Shift when `shift`) on the key named `key` types a character
/// on the active layout. Always false elsewhere: Linux reports AltGr as its own modifier, and
/// macOS has none.
pub fn altgr_types_text(key: &str, shift: bool) -> bool {
    platform::altgr_types_text(key, shift)
}

#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)] // objc 0.2 macros check a `cargo-clippy` cfg
mod platform {
    use std::collections::HashSet;
    use std::ffi::c_void;

    use cocoa::base::{id, nil};
    use cocoa::foundation::NSString;
    use objc::{class, msg_send, sel, sel_impl};

    #[link(name = "Carbon", kind = "framework")]
    extern "C" {
        fn TISCopyCurrentKeyboardLayoutInputSource() -> *mut c_void;
        fn TISGetInputSourceProperty(source: *mut c_void, key: *const c_void) -> *mut c_void;
        static kTISPropertyInputSourceLanguages: *const c_void;
        static kTISPropertyInputSourceID: *const c_void;
    }

    #[link(name = "Foundation", kind = "framework")]
    extern "C" {
        static NSLocaleExemplarCharacterSet: id;
        static NSLocaleQuotationBeginDelimiterKey: id;
        static NSLocaleQuotationEndDelimiterKey: id;
        static NSLocaleAlternateQuotationBeginDelimiterKey: id;
        static NSLocaleAlternateQuotationEndDelimiterKey: id;
    }

    /// The active keyboard layout's identifier (`com.apple.keylayout.PolishPro`).
    pub fn layout_id() -> Option<String> {
        // SAFETY: as in `layout_language`; the identifier is a CFString owned by the source.
        unsafe {
            let source = TISCopyCurrentKeyboardLayoutInputSource();
            if source.is_null() {
                return None;
            }
            let id = to_string(TISGetInputSourceProperty(source, kTISPropertyInputSourceID) as id);
            core_foundation::base::CFRelease(source as _);
            id
        }
    }

    /// The language the active keyboard layout is for, as the system names it (`pl`, `de`). An
    /// input method (Korean, Japanese, Chinese) reports the layout under it, ABC as a rule.
    fn layout_language() -> Option<String> {
        // SAFETY: TIS returns a retained input source (released below) whose properties it
        // owns; the language list is a CFArray of CFStrings, toll-free bridged to NSArray.
        unsafe {
            let source = TISCopyCurrentKeyboardLayoutInputSource();
            if source.is_null() {
                return None;
            }
            let languages = TISGetInputSourceProperty(source, kTISPropertyInputSourceLanguages) as id;
            let count: usize = if languages.is_null() { 0 } else { msg_send![languages, count] };
            let language = if count == 0 {
                None
            } else {
                let first: id = msg_send![languages, objectAtIndex: 0usize];
                to_string(first)
            };
            core_foundation::base::CFRelease(source as _);
            language
        }
    }

    unsafe fn to_string(text: id) -> Option<String> {
        if text.is_null() {
            return None;
        }
        let utf8: *const std::os::raw::c_char = msg_send![text, UTF8String];
        (!utf8.is_null()).then(|| std::ffi::CStr::from_ptr(utf8).to_string_lossy().into_owned())
    }

    /// The letters the layout's language writes beyond ASCII, both cases, and its quotation marks.
    pub fn language_letters() -> HashSet<char> {
        let mut letters = HashSet::new();
        let Some(language) = layout_language() else { return letters };
        // SAFETY: plain Foundation calls on the main thread; the locale and the objects read from
        // it are autoreleased.
        unsafe {
            let identifier = NSString::alloc(nil).init_str(&language);
            let locale: id = msg_send![class!(NSLocale), localeWithLocaleIdentifier: identifier];
            let _: () = msg_send![identifier, release];
            if locale.is_null() {
                return letters;
            }
            let set: id = msg_send![locale, objectForKey: NSLocaleExemplarCharacterSet];
            if !set.is_null() {
                for unit in 0x80u16..0x3000 {
                    let member: bool = msg_send![set, characterIsMember: unit];
                    if let Some(ch) = member.then(|| char::from_u32(unit as u32)).flatten() {
                        letters.extend(ch.to_lowercase().chain(ch.to_uppercase()));
                    }
                }
            }
            for key in [
                NSLocaleQuotationBeginDelimiterKey,
                NSLocaleQuotationEndDelimiterKey,
                NSLocaleAlternateQuotationBeginDelimiterKey,
                NSLocaleAlternateQuotationEndDelimiterKey,
            ] {
                let mark: id = msg_send![locale, objectForKey: key];
                letters.extend(to_string(mark).unwrap_or_default().chars().filter(|ch| !ch.is_ascii()));
            }
        }
        letters
    }

    pub fn altgr_types_text(_key: &str, _shift: bool) -> bool {
        false
    }
}

#[cfg(windows)]
mod platform {
    use std::collections::HashSet;

    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetKeyboardLayout, MapVirtualKeyExW, ToUnicodeEx, VkKeyScanExW, MAPVK_VK_TO_VSC, VK_CONTROL, VK_LCONTROL, VK_MENU, VK_RMENU,
        VK_SHIFT,
    };

    pub fn language_letters() -> HashSet<char> {
        HashSet::new()
    }

    pub fn layout_id() -> Option<String> {
        None
    }

    pub fn altgr_types_text(key: &str, shift: bool) -> bool {
        let mut chars = key.chars();
        let (Some(ch), None) = (chars.next(), chars.next()) else { return false };
        // SAFETY: layout queries for this thread's layout; `ToUnicodeEx` with flag 4 leaves the
        // keyboard state (dead keys) as it was.
        unsafe {
            let layout = GetKeyboardLayout(0);
            let vk = if ch.is_ascii_alphanumeric() {
                ch.to_ascii_uppercase() as u32
            } else {
                match VkKeyScanExW(ch as u16, layout) {
                    -1 => return false,
                    scan => (scan as u16 & 0xff) as u32,
                }
            };
            let scan = MapVirtualKeyExW(vk, MAPVK_VK_TO_VSC, Some(layout));
            let mut state = [0u8; 256];
            for held in [VK_CONTROL, VK_LCONTROL, VK_MENU, VK_RMENU] {
                state[held.0 as usize] = 0x80;
            }
            if shift {
                state[VK_SHIFT.0 as usize] = 0x80;
            }
            let mut text = [0u16; 8];
            let written = ToUnicodeEx(vk, scan, &state, &mut text, 4, Some(layout));
            // Negative: a dead key, which composes a character with the next key.
            written < 0
                || (written > 0
                    && String::from_utf16_lossy(&text[..written as usize]).chars().any(|ch| !ch.is_control() && !ch.is_whitespace()))
        }
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod platform {
    use std::collections::HashSet;

    pub fn language_letters() -> HashSet<char> {
        HashSet::new()
    }

    pub fn layout_id() -> Option<String> {
        None
    }

    pub fn altgr_types_text(_key: &str, _shift: bool) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::Modifiers;

    /// A keystroke the way gpui reports it on macOS: `key` from the layout without modifiers,
    /// `key_char` from the layout with them.
    fn option(key: &str, typed: &str, shift: bool) -> Keystroke {
        Keystroke { modifiers: Modifiers { alt: true, shift, ..Modifiers::default() }, key: key.into(), key_char: Some(typed.into()) }
    }

    /// The characters below come from the layouts macOS ships (UCKeyTranslate on each layout's
    /// own data), with the letters macOS's locale data gives for the layout's language.
    #[test]
    fn option_types_what_the_layout_needs_and_is_meta_otherwise() {
        let none = |_: char| false;
        let polish = |ch: char| "ąęłśżźćńóĄĘŁŚŻŹĆŃÓ".contains(ch);
        let turkish = |ch: char| "şğıçöüŞĞİÇÖÜ".contains(ch);
        // Portuguese (#272), German, French, Spanish, Swedish: code characters.
        for (key, typed) in [("8", "["), ("9", "]"), ("l", "@"), ("5", "["), ("7", "|"), ("n", "~"), ("¥", "\\")] {
            assert!(option_text(&option(key, typed, false), &none), "⌥{key} → {typed}");
        }
        assert!(option_text(&option("8", "{", true), &none));
        // Polish Pro and Turkish Q: the language's own letters.
        assert!(option_text(&option("a", "ą", false), &polish));
        assert!(option_text(&option("l", "Ł", true), &polish));
        assert!(option_text(&option("s", "ş", false), &turkish));
        // US / ABC: Option's symbols are not anyone's text, so Option stays Meta (word moves,
        // Claude Code's ⌥P and ⌥T).
        for (key, typed) in [("b", "∫"), ("f", "ƒ"), ("p", "π"), ("t", "†"), ("a", "å"), ("c", "ç"), ("8", "•")] {
            assert!(!option_text(&option(key, typed, false), &none), "⌥{key} → {typed}");
        }
        // US ⌥` types the key itself (a dead key), and a Polish ⌥b is no Polish letter.
        assert!(!option_text(&option("`", "`", false), &none));
        assert!(!option_text(&option("b", "∫", false), &polish));
        // Not Option alone: Ctrl+Option and ⌘ chords, and a key with no character.
        let mut chord = option("8", "[", false);
        chord.modifiers.control = true;
        assert!(!option_text(&chord, &none));
        chord.modifiers = Modifiers { alt: true, platform: true, ..Modifiers::default() };
        assert!(!option_text(&chord, &none));
        assert!(!option_text(&Keystroke { key_char: None, ..option("left", "", false) }, &none));
    }
}
