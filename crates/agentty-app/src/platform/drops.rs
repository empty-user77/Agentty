//! Files dropped on a window, queued for the app loop, which hands them to the pane under the
//! pointer (see `Workbench::drop_files_at`). Shared by every platform's drop handling.

use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Debug, Clone)]
pub struct Dropped {
    pub window_number: isize,
    /// Window coordinates, top-left origin.
    pub position: (f32, f32),
    pub paths: Vec<PathBuf>,
}

static DROPS: Mutex<Vec<Dropped>> = Mutex::new(Vec::new());

pub fn drain() -> Vec<Dropped> {
    let drops = DROPS.lock().map(|mut d| std::mem::take(&mut *d)).unwrap_or_default();
    drops.into_iter().map(|d| Dropped { paths: d.paths.into_iter().map(|p| terminal_safe(&p)).collect(), ..d }).collect()
}

/// Spaces that look like spaces but that shells and terminals measure differently, e.g. the
/// narrow no-break space macOS puts before "PM" in screenshot names.
fn odd_space(c: char) -> bool {
    matches!(c, '\u{00a0}' | '\u{2000}'..='\u{200b}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}')
}

/// A path that types cleanly into a shell. Screenshot thumbnails live in a temporary folder
/// that macOS empties shortly after, and their names contain odd spaces that break line editing;
/// such files are copied to Agentty's drop folder under a clean name.
pub fn terminal_safe(path: &std::path::Path) -> PathBuf {
    let text = path.to_string_lossy();
    let temporary = text.contains("/TemporaryItems/");
    if !temporary && !text.chars().any(odd_space) {
        return path.to_path_buf();
    }
    let Some(name) = path.file_name().map(|n| n.to_string_lossy().chars().map(|c| if odd_space(c) { ' ' } else { c }).collect::<String>())
    else {
        return path.to_path_buf();
    };
    let target = drop_dir().join(name);
    let copied = if path.is_dir() { false } else { std::fs::copy(path, &target).is_ok() };
    if copied {
        target
    } else {
        path.to_path_buf()
    }
}

/// Files a paste brings: copied files, and a copied image (a screenshot, an image copied in a
/// browser) written out as a file — an agent reads a path, not the clipboard's image. Empty when
/// the clipboard holds text, which pastes as text.
pub fn pasted_paths(cx: &gpui::App) -> Vec<PathBuf> {
    let paths = crate::file_drop::clipboard_paths();
    if !paths.is_empty() || cfg!(any(target_os = "macos", windows)) {
        return paths;
    }
    // Linux has no reader of its own: what GPUI's clipboard carries (files, images) is used.
    let Some(item) = cx.read_from_clipboard() else { return Vec::new() };
    let mut files = Vec::new();
    let mut image = None;
    let mut text = false;
    for entry in item.entries() {
        match entry {
            gpui::ClipboardEntry::ExternalPaths(paths) => files.extend(paths.paths().iter().map(|p| terminal_safe(p))),
            gpui::ClipboardEntry::Image(copied) if image.is_none() => image = Some(copied),
            gpui::ClipboardEntry::String(copied) => text |= !copied.text().trim().is_empty(),
            _ => {}
        }
    }
    if !files.is_empty() {
        return files;
    }
    // Text beside a picture of it (a spreadsheet's cells) is pasted as the text.
    if text {
        return Vec::new();
    }
    image.and_then(|image| write_image(image.format, &image.bytes)).into_iter().collect()
}

/// A copied image as a file in the drop folder; `None` for a format agents do not read.
fn write_image(format: gpui::ImageFormat, bytes: &[u8]) -> Option<PathBuf> {
    let extension = match format {
        gpui::ImageFormat::Png => "png",
        gpui::ImageFormat::Jpeg => "jpg",
        gpui::ImageFormat::Webp => "webp",
        gpui::ImageFormat::Gif => "gif",
        gpui::ImageFormat::Bmp => "bmp",
        _ => return None,
    };
    if bytes.is_empty() {
        return None;
    }
    let millis = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let path = drop_dir().join(format!("pasted-{millis}.{extension}"));
    std::fs::write(&path, bytes).ok()?;
    Some(path)
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))] // the macOS drop hook queues here
pub(crate) fn push(drop: Dropped) {
    if let Ok(mut drops) = DROPS.lock() {
        drops.push(drop);
    }
}

pub(crate) fn drop_dir() -> PathBuf {
    let dir = std::env::temp_dir().join("agentty-drops");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn odd_spaces_are_replaced_in_copies() {
        let dir = std::env::temp_dir().join(format!("agentty-drop-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("스크린샷 2026-09-17 \u{202f}오후 7.56.13.png");
        std::fs::write(&source, b"png").unwrap();
        let safe = terminal_safe(&source);
        assert!(!safe.to_string_lossy().chars().any(odd_space));
        assert_eq!(std::fs::read(&safe).unwrap(), b"png");
        let plain = dir.join("plain.txt");
        std::fs::write(&plain, b"x").unwrap();
        assert_eq!(terminal_safe(&plain), plain);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_copied_image_becomes_a_file() {
        let path = write_image(gpui::ImageFormat::Png, b"not really a png").unwrap();
        assert_eq!(path.extension().and_then(|e| e.to_str()), Some("png"));
        assert_eq!(std::fs::read(&path).unwrap(), b"not really a png");
        std::fs::remove_file(path).ok();
        // Nothing to write, and a format agents do not read.
        assert_eq!(write_image(gpui::ImageFormat::Png, b""), None);
        assert_eq!(write_image(gpui::ImageFormat::Svg, b"<svg/>"), None);
    }
}
