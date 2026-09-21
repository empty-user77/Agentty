//! Windows / Linux: files dropped on a window arrive through GPUI's own `ExternalPaths` drop
//! event (see `Workbench::render`), so there is no native hook to install; the shared queue and
//! path cleanup are the same as on macOS.

pub use crate::platform::drops::drain;

pub fn install(_ns_window: crate::native::Id) {}

/// Files on the clipboard for ⌘V. GPUI's clipboard carries text and images but no file list here,
/// so pasting stays text-only until the platform offers one.
pub fn clipboard_paths() -> Vec<std::path::PathBuf> {
    Vec::new()
}
