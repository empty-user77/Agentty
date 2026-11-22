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
    // Wayland hands copied files over as text (their addresses or paths, a line each).
    if let Some(copied) = item.text() {
        let files = files_in_text(&copied);
        if !files.is_empty() {
            return files.iter().map(|path| terminal_safe(path)).collect();
        }
    }
    // Text beside a picture of it (a spreadsheet's cells) is pasted as the text.
    if text {
        return Vec::new();
    }
    image.and_then(|image| write_image(image.format, &image.bytes)).into_iter().collect()
}

/// The files a file manager's copy names when it reaches us as text: every line a `file://`
/// address (Dolphin, Nautilus) of a file that exists. Anything else — a sentence, a path someone
/// typed, an address of something that is not there — is text, and empty comes back.
fn files_in_text(text: &str) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let Some(path) = line.strip_prefix("file://").map(percent_decoded) else { return Vec::new() };
        // `file://host/path` names another machine's file.
        let path = PathBuf::from(path);
        if !path.is_absolute() || !path.exists() {
            return Vec::new();
        }
        files.push(path);
    }
    files
}

/// `%20` and the like in a file address, as the bytes they stand for.
fn percent_decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let hex = (bytes[at] == b'%').then(|| text.get(at + 1..at + 3)).flatten().and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match hex {
            Some(byte) => {
                out.push(byte);
                at += 3;
            }
            None => {
                out.push(bytes[at]);
                at += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Whether `paths` is a copied image that was written out for this paste (not files the user
/// copied): where text is on the clipboard beside it, a place that takes text pastes the text.
pub fn is_written_image(paths: &[PathBuf]) -> bool {
    let folder = drop_dir();
    !paths.is_empty()
        && paths.iter().all(|path| {
            path.parent() == Some(folder.as_path()) && path.file_name().is_some_and(|name| name.to_string_lossy().starts_with("pasted-"))
        })
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
    write_private(&path, bytes).ok()?;
    Some(path)
}

/// A pasted image may be a screenshot of anything: readable by this user only.
fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)?.write_all(bytes)
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))] // the macOS drop hook queues here
pub(crate) fn push(drop: Dropped) {
    if let Ok(mut drops) = DROPS.lock() {
        drops.push(drop);
    }
}

/// Where dropped and pasted files are written. macOS and Windows give each user a temporary
/// folder of their own; Linux's `/tmp` is everyone's, so there the folder carries the user's id,
/// is open to that user only, and is used only when it is really theirs.
pub(crate) fn drop_dir() -> PathBuf {
    #[cfg(unix)]
    {
        // SAFETY: `getuid` has no preconditions and cannot fail.
        let uid = unsafe { libc::getuid() };
        let shared = std::env::temp_dir().join(format!("agentty-drops-{uid}"));
        if private_dir(&shared, uid) {
            return shared;
        }
        // Another user's folder (or a link) took the name: Agentty's own data folder instead.
        let own = agentty_bridge::fsutil::data_dir().join("drops");
        let _ = private_dir(&own, uid);
        own
    }
    #[cfg(not(unix))]
    {
        let dir = std::env::temp_dir().join("agentty-drops");
        let _ = std::fs::create_dir_all(&dir);
        dir
    }
}

/// Makes `dir` (`0700`) if it is not there, and says whether it is a real folder of `uid`'s that
/// nobody else can open. One with looser permissions is closed first.
#[cfg(unix)]
fn private_dir(dir: &std::path::Path, uid: u32) -> bool {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    let _ = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir);
    // Not followed: a link planted under this name points somewhere else.
    let Ok(meta) = std::fs::symlink_metadata(dir) else { return false };
    if !meta.is_dir() || meta.uid() != uid {
        return false;
    }
    meta.mode() & 0o077 == 0 || std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).is_ok()
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
    fn copied_files_are_read_from_their_addresses() {
        let dir = std::env::temp_dir().join(format!("agentty-uri-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("my report.txt");
        std::fs::write(&file, b"x").unwrap();
        let address = format!("file://{}", file.display().to_string().replace(' ', "%20"));
        assert_eq!(files_in_text(&format!("{address}\r\n")), vec![file.clone()]);
        assert_eq!(files_in_text(&format!("{address}\n{address}\n")).len(), 2);
        // Text, a typed path, a file that is not there, a mix: all text.
        assert!(files_in_text("see file:///etc/hosts for it").is_empty());
        assert!(files_in_text(&file.display().to_string()).is_empty());
        assert!(files_in_text("file:///no/such/file-agentty").is_empty());
        assert!(files_in_text(&format!("{address}\nand a note")).is_empty());
        assert!(files_in_text("").is_empty());
        assert_eq!(percent_decoded("a%20b%2Fc%zz%4"), "a b/c%zz%4");
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
        // An image written for a paste is told from files the user copied.
        let written = write_image(gpui::ImageFormat::Png, b"x").unwrap();
        assert!(is_written_image(std::slice::from_ref(&written)));
        assert!(!is_written_image(&[written.clone(), PathBuf::from("/tmp/report.pdf")]));
        assert!(!is_written_image(&[]));
        std::fs::remove_file(written).ok();
    }

    /// A screenshot pasted on a shared Linux machine is the user's alone: the folder is theirs and
    /// closed, the file too, and a folder of the same name that is not theirs is never used.
    #[cfg(unix)]
    #[test]
    fn pasted_files_are_private() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let written = write_image(gpui::ImageFormat::Png, b"private").unwrap();
        let folder = written.parent().unwrap();
        // SAFETY: no preconditions.
        let uid = unsafe { libc::getuid() };
        let meta = std::fs::symlink_metadata(folder).unwrap();
        assert!(meta.is_dir() && meta.uid() == uid);
        assert_eq!(meta.mode() & 0o777, 0o700);
        assert_eq!(std::fs::metadata(&written).unwrap().mode() & 0o777, 0o600);
        std::fs::remove_file(written).ok();

        let dir = std::env::temp_dir().join(format!("agentty-private-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A folder left open is closed.
        let open = dir.join("open");
        std::fs::create_dir(&open).unwrap();
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(private_dir(&open, uid));
        assert_eq!(std::fs::metadata(&open).unwrap().mode() & 0o777, 0o700);
        // A link under the name, a file under the name and someone else's folder are refused.
        let link = dir.join("link");
        std::os::unix::fs::symlink(&open, &link).unwrap();
        assert!(!private_dir(&link, uid));
        let file = dir.join("file");
        std::fs::write(&file, b"x").unwrap();
        assert!(!private_dir(&file, uid));
        assert!(!private_dir(&open, uid.wrapping_add(1)));
        std::fs::remove_dir_all(dir).ok();
    }
}
