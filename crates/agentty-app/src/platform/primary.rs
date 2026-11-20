//! Linux's primary selection: text that was just selected with the pointer is there without a
//! copy, and a middle click pastes it. Other systems have no such thing: nothing is kept, and
//! nothing is read.

use gpui::App;

/// Notes `text` as what the pointer selected last.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub fn set(text: String, cx: &App) {
    if !text.is_empty() {
        cx.write_to_primary(gpui::ClipboardItem::new_string(text));
    }
}

#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
pub fn set(_text: String, _cx: &App) {}

/// What a middle click pastes; `None` where the system has no primary selection.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub fn get(cx: &App) -> Option<String> {
    cx.read_from_primary().and_then(|item| item.text()).filter(|text| !text.is_empty())
}

#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
pub fn get(_cx: &App) -> Option<String> {
    None
}
