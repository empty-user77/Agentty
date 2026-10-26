//! Agentty as the app that opens spreadsheets, CSV files and Word documents from Finder. The
//! bundle declares those types (`packaging/macos/Info.plist.in`, as an alternate handler); this
//! asks Launch Services which app opens a type now and, when the user says so, makes it Agentty.
//!
//! Only a real app bundle answers: a development build run from `target/` is not something Finder
//! can open files with, so it never asks and never claims a type.

use std::path::Path;

/// The file types Agentty offers to open, by extension, with their uniform type identifiers.
pub const TYPES: [(&str, &str); 5] = [
    ("csv", "public.comma-separated-values-text"),
    ("tsv", "public.tab-separated-values-text"),
    ("xlsx", "org.openxmlformats.spreadsheetml.sheet"),
    ("xls", "com.microsoft.excel.xls"),
    ("docx", "org.openxmlformats.wordprocessingml.document"),
];

fn extension(path: &Path) -> Option<String> {
    path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase())
}

fn content_type(path: &Path) -> Option<&'static str> {
    let extension = extension(path)?;
    TYPES.iter().find(|(e, _)| *e == extension).map(|(_, uti)| *uti)
}

/// A file of a type Agentty can be the default app for.
pub fn supported(path: &Path) -> bool {
    content_type(path).is_some()
}

/// Whether Agentty opens this file's type from Finder; `None` when that can't be told (not macOS,
/// or not running from an app bundle).
pub fn is_default(path: &Path) -> Option<bool> {
    platform::is_default(content_type(path)?)
}

/// Makes Agentty the app that opens this file's type.
pub fn make_default(path: &Path) -> anyhow::Result<()> {
    let uti = content_type(path).ok_or_else(|| anyhow::anyhow!("not a file type Agentty opens"))?;
    platform::make_default(uti)
}

/// Whether to offer making Agentty the default app for this file: a type it supports, that
/// another app opens now, and the user did not turn the offer down for.
pub fn should_ask(path: &Path, cx: &gpui::App) -> bool {
    let Some(extension) = extension(path) else { return false };
    supported(path) && !crate::settings::settings(cx).default_app_dismissed.contains(&extension) && is_default(path) == Some(false)
}

/// Not asked again for this file's type.
pub fn dismiss(path: &Path, cx: &mut gpui::App) {
    let Some(extension) = extension(path).filter(|_| supported(path)) else { return };
    crate::settings::update_settings(cx, |s| {
        if !s.default_app_dismissed.contains(&extension) {
            s.default_app_dismissed.push(extension);
        }
    });
}

#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)] // objc 0.2 macros check a `cargo-clippy` cfg
mod platform {
    use core_foundation::base::TCFType;
    use core_foundation::string::{CFString, CFStringRef};
    use objc::{msg_send, sel, sel_impl};

    /// Viewer, editor, shell: every way of opening a file.
    const ROLES_ALL: u32 = 0xFFFF_FFFF;

    #[link(name = "CoreServices", kind = "framework")]
    extern "C" {
        fn LSCopyDefaultRoleHandlerForContentType(content_type: CFStringRef, role: u32) -> CFStringRef;
        fn LSSetDefaultRoleHandlerForContentType(content_type: CFStringRef, role: u32, handler: CFStringRef) -> i32;
    }

    /// This app's bundle identifier, when it runs from an app bundle.
    fn bundle_id() -> Option<String> {
        type Id = *mut objc::runtime::Object;
        unsafe {
            let bundle: Id = msg_send![objc::class!(NSBundle), mainBundle];
            if bundle.is_null() {
                return None;
            }
            let path: Id = msg_send![bundle, bundlePath];
            let identifier: Id = msg_send![bundle, bundleIdentifier];
            let text = |s: Id| -> Option<String> {
                if s.is_null() {
                    return None;
                }
                let utf8: *const std::os::raw::c_char = msg_send![s, UTF8String];
                (!utf8.is_null()).then(|| std::ffi::CStr::from_ptr(utf8).to_string_lossy().into_owned())
            };
            text(path).filter(|p| p.ends_with(".app"))?;
            text(identifier)
        }
    }

    pub fn is_default(uti: &str) -> Option<bool> {
        let ours = bundle_id()?;
        let uti = CFString::new(uti);
        // SAFETY: a valid CFString in; the result follows the create rule (owned, or null).
        let handler = unsafe { LSCopyDefaultRoleHandlerForContentType(uti.as_concrete_TypeRef(), ROLES_ALL) };
        if handler.is_null() {
            return Some(false);
        }
        let handler = unsafe { CFString::wrap_under_create_rule(handler) };
        Some(handler.to_string().eq_ignore_ascii_case(&ours))
    }

    pub fn make_default(uti: &str) -> anyhow::Result<()> {
        let ours = bundle_id().ok_or_else(|| anyhow::anyhow!("not running from the Agentty app"))?;
        let (uti, handler) = (CFString::new(uti), CFString::new(&ours));
        // SAFETY: two valid CFStrings, borrowed for the call.
        let status = unsafe { LSSetDefaultRoleHandlerForContentType(uti.as_concrete_TypeRef(), ROLES_ALL, handler.as_concrete_TypeRef()) };
        anyhow::ensure!(status == 0, "Launch Services refused (status {status})");
        Ok(())
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    pub fn is_default(_: &str) -> Option<bool> {
        None
    }

    pub fn make_default(_: &str) -> anyhow::Result<()> {
        anyhow::bail!("choosing the default app is only supported on macOS")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_its_types_by_extension() {
        assert!(supported(Path::new("/tmp/a.CSV")));
        assert!(supported(Path::new("report.xlsx")));
        assert!(supported(Path::new("letter.docx")));
        assert!(!supported(Path::new("notes.md")));
        assert!(!supported(Path::new("Makefile")));
        assert_eq!(content_type(Path::new("a.tsv")), Some("public.tab-separated-values-text"));
    }

    #[test]
    fn a_development_build_is_never_the_default() {
        // Tests run from `target/`, not from an app bundle.
        assert_eq!(is_default(Path::new("a.csv")), None);
        assert!(make_default(Path::new("a.csv")).is_err());
    }
}
