//! What the keyboard layout in use types, read from the system's own layout data, so that a key
//! that types a character on that layout is text, not a shortcut or Meta.
//!
//! The bindings and the terminal's Meta key are written for a US layout. On most other layouts
//! Option (macOS) or AltGr (Windows) is how characters are typed: `[ ] { } @ | \ ~` on German,
//! French, Spanish, Italian, Portuguese and the Nordic layouts, `ą ę ł ś ż` on Polish Pro, `ş ğ ı`
//! on Turkish Q. What a layout types is read once per layout ([`refresh`], from `main` at start
//! and on every switch), and every key is checked against it:
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
use std::rc::Rc;

use gpui::{App, Keystroke};

#[derive(Default)]
struct Layout {
    /// The layout gpui reported when this was read, so a repeated notification reads nothing.
    id: String,
    /// Characters of the layout's language that are not ASCII (`ą`, `ş`, `ä`), both cases.
    letters: Rc<HashSet<char>>,
    /// What the layout types without Option (with or without Shift): Option making one of these
    /// (US ⌥⇧` is a backtick) gives nothing new, so it stays Meta.
    plain: HashSet<char>,
    /// macOS: the key a ⌘ shortcut moves to on this layout (`[` → `ö` on German).
    cmd_keys: HashMap<char, char>,
}

thread_local! {
    static LAYOUT: RefCell<Layout> = RefCell::default();
    /// Letters per language: switching back and forth (ABC and Korean, say) reads each once.
    static LETTERS: RefCell<HashMap<String, Rc<HashSet<char>>>> = RefCell::default();
}

/// Reads the layout gpui reports, at start and when the user switches layouts. False when it is
/// the layout already read, so the caller can skip binding everything again.
pub fn refresh(cx: &App) -> bool {
    let id = cx.keyboard_layout().id();
    if LAYOUT.with(|layout| layout.borrow().id == id) {
        return false;
    }
    let cmd_keys = match cx.keyboard_mapper().get_key_equivalents() {
        Some(table) if cfg!(target_os = "macos") => table.iter().map(|(from, to)| (*from, *to)).collect(),
        _ => HashMap::new(),
    };
    let language = platform::layout_language().unwrap_or_default();
    let letters = LETTERS
        .with(|cache| cache.borrow_mut().entry(language.clone()).or_insert_with(|| Rc::new(platform::language_letters(&language))).clone());
    let plain = platform::plain_chars();
    LAYOUT.with(|layout| *layout.borrow_mut() = Layout { id: id.to_string(), letters, plain, cmd_keys });
    true
}

/// macOS: whether gpui read the layout before the system finished switching to it. gpui reads
/// the layout when AppKit posts its input-source notification, which can come early (seen going
/// from German to Korean) and then not again, leaving the old layout's shortcuts bound.
pub fn read_too_early(cx: &App) -> bool {
    platform::layout_id().is_some_and(|id| id != cx.keyboard_layout().id())
}

/// Has gpui read the layout again, by posting the notification it reads the layout on.
pub fn read_again() {
    platform::post_layout_notification();
}

/// macOS: whether Option with this key types a character the layout's user needs, which then
/// goes in as text even while Option is Meta.
pub fn option_types_text(keystroke: &Keystroke) -> bool {
    LAYOUT.with(|layout| {
        let layout = layout.borrow();
        option_text(keystroke, &|ch| layout.plain.contains(&ch), &|ch| layout.letters.contains(&ch))
    })
}

/// Text when Option makes a character the layout types only with Option (`typed_plain` says
/// which it types without) that is ASCII or a letter of its language.
fn option_text(keystroke: &Keystroke, typed_plain: &dyn Fn(char) -> bool, language_letter: &dyn Fn(char) -> bool) -> bool {
    let m = keystroke.modifiers;
    if !m.alt || m.control || m.platform || m.function {
        return false;
    }
    let Some(text) = keystroke.key_char.as_deref() else { return false };
    let mut chars = text.chars();
    let (Some(ch), None) = (chars.next(), chars.next()) else { return false };
    // A key that types itself with Option (US ⌥`), or anything the layout types without Option,
    // is a Meta chord: Option adds nothing there.
    if ch.to_lowercase().eq(keystroke.key.chars()) || typed_plain(ch) {
        return false;
    }
    ch.is_ascii_graphic() || language_letter(ch)
}

/// macOS: the key a ⌘ shortcut on `key` takes on the active layout, when macOS moves it there.
pub fn cmd_key(key: char) -> Option<char> {
    LAYOUT.with(|layout| layout.borrow().cmd_keys.get(&key).copied())
}

/// macOS: whether the active layout moves any ⌘ shortcut (none on a US layout).
pub fn moves_cmd_keys() -> bool {
    LAYOUT.with(|layout| !layout.borrow().cmd_keys.is_empty())
}

/// Windows: whether Ctrl+Alt (with Shift when `shift`) on the key named `key` types a character
/// on the active layout. Always false elsewhere: Linux reports AltGr as its own modifier, and
/// macOS has none.
pub fn altgr_types_text(key: &str, shift: bool) -> bool {
    platform::altgr_types_text(key, shift)
}

/// macOS: what the key with the virtual key `code` types on the active layout, with Carbon
/// `modifiers` (⌘ 1, ⇧ 2, ⌥ 8, ⌃ 16). For the debug driver's key presses.
#[cfg(target_os = "macos")]
pub fn key_chars(code: u16, modifiers: u32) -> String {
    platform::key_chars(code, modifiers)
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
        fn LMGetKbdType() -> u8;
        #[allow(clippy::too_many_arguments)]
        fn UCKeyTranslate(
            layout: *const c_void,
            code: u16,
            action: u16,
            modifiers: u32,
            keyboard_type: u32,
            options: u32,
            dead_key_state: *mut u32,
            max_length: usize,
            length: *mut usize,
            text: *mut u16,
        ) -> i32;
        static kTISPropertyInputSourceLanguages: *const c_void;
        static kTISPropertyInputSourceID: *const c_void;
        static kTISPropertyUnicodeKeyLayoutData: *const c_void;
    }

    #[link(name = "Foundation", kind = "framework")]
    extern "C" {
        static NSLocaleExemplarCharacterSet: id;
    }

    /// Calls `read` with the active keyboard layout's input source. An input method (Korean,
    /// Japanese, Chinese) reports the layout under it, ABC as a rule.
    fn with_layout<T>(read: impl FnOnce(*mut c_void) -> Option<T>) -> Option<T> {
        // SAFETY: TIS returns a retained input source, released after `read`; the properties
        // `read` takes from it are owned by the source and not kept past it.
        unsafe {
            let source = TISCopyCurrentKeyboardLayoutInputSource();
            if source.is_null() {
                return None;
            }
            let value = read(source);
            core_foundation::base::CFRelease(source as _);
            value
        }
    }

    /// The active keyboard layout's identifier (`com.apple.keylayout.PolishPro`).
    pub fn layout_id() -> Option<String> {
        // SAFETY: the identifier is a CFString of the source, copied out while it lives.
        with_layout(|source| unsafe { to_string(TISGetInputSourceProperty(source, kTISPropertyInputSourceID) as id) })
    }

    /// The language the active keyboard layout is for, as the system names it (`pl`, `de`).
    pub fn layout_language() -> Option<String> {
        with_layout(|source| {
            // SAFETY: the language list is a CFArray of CFStrings of the source, toll-free
            // bridged to NSArray, read while the source lives.
            unsafe {
                let languages = TISGetInputSourceProperty(source, kTISPropertyInputSourceLanguages) as id;
                let count: usize = if languages.is_null() { 0 } else { msg_send![languages, count] };
                if count == 0 {
                    return None;
                }
                to_string(msg_send![languages, objectAtIndex: 0usize])
            }
        })
    }

    pub fn key_chars(code: u16, modifiers: u32) -> String {
        with_layout(|source| {
            // SAFETY: the layout data belongs to the source and is read while it lives.
            unsafe {
                let data = TISGetInputSourceProperty(source, kTISPropertyUnicodeKeyLayoutData) as core_foundation::data::CFDataRef;
                if data.is_null() {
                    return None;
                }
                let layout = core_foundation::data::CFDataGetBytePtr(data) as *const c_void;
                let (mut text, mut length, mut dead) = ([0u16; 8], 0usize, 0u32);
                // Option 1 (kUCKeyTranslateNoDeadKeysBit): a dead key gives its own character.
                UCKeyTranslate(layout, code, 0, modifiers, LMGetKbdType() as u32, 1, &mut dead, text.len(), &mut length, text.as_mut_ptr());
                Some(String::from_utf16_lossy(&text[..length]))
            }
        })
        .unwrap_or_default()
    }

    /// Every character the active layout types without Option, with or without Shift.
    pub fn plain_chars() -> HashSet<char> {
        // Key codes 0–127 cover every key that types; Carbon modifiers: none, then ⇧.
        (0u16..128).flat_map(|code| [key_chars(code, 0), key_chars(code, 2)]).flat_map(|text| text.chars().collect::<Vec<_>>()).collect()
    }

    unsafe fn to_string(text: id) -> Option<String> {
        if text.is_null() {
            return None;
        }
        let utf8: *const std::os::raw::c_char = msg_send![text, UTF8String];
        (!utf8.is_null()).then(|| std::ffi::CStr::from_ptr(utf8).to_string_lossy().into_owned())
    }

    /// The letters `language` writes beyond ASCII, both cases. Only letters: punctuation such as
    /// English quotation marks is what a US layout makes with ⌥[ and ⌥], keys that stay Meta.
    pub fn language_letters(language: &str) -> HashSet<char> {
        let mut letters = HashSet::new();
        if language.is_empty() {
            return letters;
        }
        // SAFETY: Foundation calls on the main thread, in a pool of their own so the locale and
        // the objects read from it are released here.
        unsafe {
            let pool: id = msg_send![class!(NSAutoreleasePool), new];
            let identifier = NSString::alloc(nil).init_str(language);
            let locale: id = msg_send![class!(NSLocale), localeWithLocaleIdentifier: identifier];
            let _: () = msg_send![identifier, release];
            if !locale.is_null() {
                let set: id = msg_send![locale, objectForKey: NSLocaleExemplarCharacterSet];
                if !set.is_null() {
                    // Up to CJK, whose characters an input method types rather than Option.
                    for unit in 0x80u16..0x3000 {
                        let member: bool = msg_send![set, characterIsMember: unit];
                        if let Some(ch) = member.then(|| char::from_u32(unit as u32)).flatten() {
                            letters.extend(ch.to_lowercase().chain(ch.to_uppercase()));
                        }
                    }
                }
            }
            let _: () = msg_send![pool, drain];
        }
        letters
    }

    /// Queues the notification gpui reads the keyboard layout on (gpui observes it with no
    /// sender, so one with none reaches it). Queued, not posted: gpui's observer takes the app,
    /// which a post from inside an update would find taken.
    pub fn post_layout_notification() {
        // NSPostASAP: delivered at the end of the current run loop pass.
        const POST_ASAP: usize = 2;
        // SAFETY: Foundation calls on the main thread; the notification is autoreleased and
        // retained by the queue until it is posted.
        unsafe {
            let name = NSString::alloc(nil).init_str("NSTextInputContextKeyboardSelectionDidChangeNotification");
            let notification: id = msg_send![class!(NSNotification), notificationWithName: name object: nil];
            let queue: id = msg_send![class!(NSNotificationQueue), defaultQueue];
            let _: () = msg_send![queue, enqueueNotification: notification postingStyle: POST_ASAP];
            let _: () = msg_send![name, release];
        }
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

    pub fn layout_id() -> Option<String> {
        None
    }

    pub fn layout_language() -> Option<String> {
        None
    }

    pub fn language_letters(_language: &str) -> HashSet<char> {
        HashSet::new()
    }

    pub fn plain_chars() -> HashSet<char> {
        HashSet::new()
    }

    pub fn post_layout_notification() {}

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

    pub fn layout_id() -> Option<String> {
        None
    }

    pub fn layout_language() -> Option<String> {
        None
    }

    pub fn language_letters(_language: &str) -> HashSet<char> {
        HashSet::new()
    }

    pub fn plain_chars() -> HashSet<char> {
        HashSet::new()
    }

    pub fn post_layout_notification() {}

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
        // What a layout types without Option: here the ASCII a non-US layout has on its plain
        // keys (none of `[ ] { } @ | \ ~` on Portuguese or German), or all of it on a US layout.
        let european = |ch: char| ch.is_ascii_alphanumeric();
        let us = |ch: char| ch.is_ascii_graphic();
        let polish = |ch: char| "ąęłśżźćńóĄĘŁŚŻŹĆŃÓ".contains(ch);
        let turkish = |ch: char| "şğıçöüŞĞİÇÖÜ".contains(ch);
        // Portuguese (#272), German, French, Spanish, Swedish: code characters.
        for (key, typed) in [("8", "["), ("9", "]"), ("l", "@"), ("5", "["), ("7", "|"), ("n", "~"), ("¥", "\\")] {
            assert!(option_text(&option(key, typed, false), &european, &none), "⌥{key} → {typed}");
        }
        assert!(option_text(&option("8", "{", true), &european, &none));
        // Polish Pro and Turkish Q: the language's own letters.
        assert!(option_text(&option("a", "ą", false), &us, &polish));
        assert!(option_text(&option("l", "Ł", true), &us, &polish));
        assert!(option_text(&option("s", "ş", false), &us, &turkish));
        // US / ABC: Option's symbols are not anyone's text, so Option stays Meta (word moves,
        // Claude Code's ⌥P and ⌥T).
        for (key, typed) in [("b", "∫"), ("f", "ƒ"), ("p", "π"), ("t", "†"), ("a", "å"), ("c", "ç"), ("8", "•")] {
            assert!(!option_text(&option(key, typed, false), &us, &none), "⌥{key} → {typed}");
        }
        // US ⌥` types the key itself (a dead key), ⌥⇧` a backtick the layout types anyway, and a
        // Polish ⌥b is no Polish letter.
        assert!(!option_text(&option("`", "`", false), &none, &none));
        assert!(!option_text(&option("~", "`", true), &us, &none));
        assert!(!option_text(&option("b", "∫", false), &us, &polish));
        // Not Option alone: Ctrl+Option and ⌘ chords, and a key with no character.
        let mut chord = option("8", "[", false);
        chord.modifiers.control = true;
        assert!(!option_text(&chord, &european, &none));
        chord.modifiers = Modifiers { alt: true, platform: true, ..Modifiers::default() };
        assert!(!option_text(&chord, &european, &none));
        assert!(!option_text(&Keystroke { key_char: None, ..option("left", "", false) }, &european, &none));
    }
}
