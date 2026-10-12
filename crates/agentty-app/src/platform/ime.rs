//! Korean (and other) input on Windows, where gpui 0.2.2 loses some of what the IME needs. Both
//! are fixed in gpui after 0.2.2 (zed-industries/zed#41259, #46545), not yet on crates.io:
//!
//! - It never passes a key it does not know (한/영, 한자, Japanese conversion keys) to
//!   `TranslateMessage`, and an IMM32 IME ("previous version" of the Microsoft IME, and others)
//!   only switches mode and composes from there: Korean could not be typed at all. A message hook
//!   on the UI thread translates exactly those keys ([`install`]).
//! - When one message both finishes a syllable and starts the next (한 + ㄴ), gpui hands the new
//!   syllable over first and the finished one after it, which drops the new one from the marked
//!   text: it is not shown, and Enter or Backspace then reaches the terminal ahead of it. Input
//!   handlers ask the IME what is still being composed after taking a finished syllable
//!   ([`pending_composition`]).

/// Starts translating the keys gpui drops (see the module). Call once, on the UI thread.
pub fn install() {
    #[cfg(windows)]
    windows_ime::install();
}

/// What the IME of `window` is still composing, after a finished syllable was handed over: the
/// input handler shows it as marked text again. `None` elsewhere than on Windows.
pub fn pending_composition(window: &gpui::Window) -> Option<String> {
    #[cfg(windows)]
    return windows_ime::pending_composition(window);
    #[cfg(not(windows))]
    {
        let _ = window;
        None
    }
}

#[cfg(windows)]
mod windows_ime {
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Input::Ime::{ImmGetCompositionStringW, ImmGetContext, ImmGetVirtualKey, ImmReleaseContext, GCS_COMPSTR};
    use windows::Win32::UI::Input::KeyboardAndMouse::{MapVirtualKeyW, MAPVK_VK_TO_CHAR, VK_PROCESSKEY};
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, SetWindowsHookExW, TranslateMessage, MSG, PM_REMOVE, WH_GETMESSAGE, WM_KEYDOWN, WM_SYSKEYDOWN,
    };

    pub fn install() {
        // SAFETY: a hook procedure for this thread only; it stays for the life of the process.
        let _ = unsafe { SetWindowsHookExW(WH_GETMESSAGE, Some(on_message), None, GetCurrentThreadId()) };
    }

    /// Runs as `GetMessageW` hands over each message, before gpui dispatches it.
    unsafe extern "system" fn on_message(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        // Only messages taken off the queue: a peek would see them again.
        if code >= 0 && wparam.0 == PM_REMOVE.0 as usize && lparam.0 != 0 {
            // SAFETY: for WH_GETMESSAGE, `lparam` points at the message being retrieved.
            let msg = unsafe { &*(lparam.0 as *const MSG) };
            if (msg.message == WM_KEYDOWN || msg.message == WM_SYSKEYDOWN) && dropped_by_gpui(msg) {
                // SAFETY: a key message of this thread, as a message loop would translate it.
                let _ = unsafe { TranslateMessage(msg) };
            }
        }
        // SAFETY: passing the hook's arguments on unchanged.
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }

    /// Whether gpui 0.2.2 drops this key without `TranslateMessage`: a key the IME took
    /// (`VK_PROCESSKEY`) whose own key gpui has no name for, or one of the IME's own keys.
    fn dropped_by_gpui(msg: &MSG) -> bool {
        let key = (msg.wParam.0 & 0xFFFF) as u16;
        if key == VK_PROCESSKEY.0 {
            // SAFETY: a plain query about the key message being handled.
            let own = unsafe { ImmGetVirtualKey(msg.hwnd) } as u16;
            return !gpui_names(own);
        }
        super::is_ime_key(key)
    }

    /// gpui's key names: its fixed list, or a key that types a character.
    fn gpui_names(key: u16) -> bool {
        // SAFETY: a plain query.
        super::gpui_fixed_name(key) || unsafe { MapVirtualKeyW(key as u32, MAPVK_VK_TO_CHAR) } != 0
    }

    pub fn pending_composition(window: &gpui::Window) -> Option<String> {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let hwnd = match HasWindowHandle::window_handle(window).ok()?.as_raw() {
            RawWindowHandle::Win32(handle) => HWND(handle.hwnd.get() as *mut _),
            _ => return None,
        };
        // SAFETY: the window's own input context, released before returning; the buffer is as
        // long as the IME said the string is (in bytes).
        unsafe {
            let context = ImmGetContext(hwnd);
            if context.is_invalid() {
                return None;
            }
            let bytes = ImmGetCompositionStringW(context, GCS_COMPSTR, None, 0);
            let text = if bytes > 0 {
                let mut buffer = vec![0u16; bytes as usize / 2];
                let read = ImmGetCompositionStringW(context, GCS_COMPSTR, Some(buffer.as_mut_ptr().cast()), bytes as u32);
                if read > 0 {
                    buffer.truncate(read as usize / 2);
                    String::from_utf16_lossy(&buffer)
                } else {
                    String::new()
                }
            } else {
                String::new()
            };
            let _ = ImmReleaseContext(hwnd, context);
            (!text.is_empty()).then_some(text)
        }
    }
}

/// The IME's own keys: 한/영 (`VK_HANGUL`, also `VK_KANA`), `VK_IME_ON`, `VK_JUNJA`, `VK_FINAL`,
/// 한자 (`VK_HANJA`, also `VK_KANJI`), `VK_IME_OFF`, and Japanese conversion keys (`VK_CONVERT`,
/// `VK_NONCONVERT`, `VK_ACCEPT`, `VK_MODECHANGE`). gpui 0.2.2 has no name for any of them.
#[cfg_attr(not(windows), allow(dead_code))]
fn is_ime_key(key: u16) -> bool {
    matches!(key, 0x15..=0x1A | 0x1C..=0x1F)
}

/// The keys gpui 0.2.2 names from a fixed list (`parse_immutable`): space, backspace, enter, tab,
/// arrows, home / end, page up / down, browser back / forward, escape, insert, delete, menu, F1–F24.
#[cfg_attr(not(windows), allow(dead_code))]
fn gpui_fixed_name(key: u16) -> bool {
    matches!(key, 0x08 | 0x09 | 0x0D | 0x1B | 0x20..=0x28 | 0x2D | 0x2E | 0x5D | 0x70..=0x87 | 0xA6 | 0xA7)
}

#[cfg(test)]
mod tests {
    use super::{gpui_fixed_name, is_ime_key};

    #[test]
    fn ime_keys_are_told_from_named_keys() {
        // 한/영, 한자, IME on/off, Japanese conversion keys.
        for key in [0x15, 0x16, 0x19, 0x1A, 0x1C, 0x1D, 0x1F] {
            assert!(is_ime_key(key) && !gpui_fixed_name(key), "{key:#x}");
        }
        // Escape sits between them and is a key gpui names.
        assert!(!is_ime_key(0x1B) && gpui_fixed_name(0x1B));
        // Enter, backspace, arrows, F1 and F24 are named; letters and digits are not on the list
        // (they type a character instead).
        for key in [0x0D, 0x08, 0x25, 0x28, 0x70, 0x87] {
            assert!(gpui_fixed_name(key) && !is_ime_key(key), "{key:#x}");
        }
        for key in [0x41, 0x5A, 0x30] {
            assert!(!gpui_fixed_name(key) && !is_ime_key(key), "{key:#x}");
        }
    }
}
