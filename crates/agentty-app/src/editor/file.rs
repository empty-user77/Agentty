//! Reading a file for the editor and writing it back.
//!
//! Files are only ever read and written, never run. Saving goes through a temporary file next to
//! the original that is renamed over it, so a crash or a full disk never leaves half a file; the
//! original's permissions are kept. A link is written through to the file it points at, and a
//! file whose real location is outside the project opens read-only until the user allows edits.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Files larger than this open read-only (and without colors).
pub const EDITABLE_LIMIT: u64 = 2 * 1024 * 1024;
/// Files larger than this are not opened at all.
pub const OPEN_LIMIT: u64 = 20 * 1024 * 1024;
/// A line this long (minified code) makes the file read-only: it could not be shown usefully.
pub const LONG_LINE: usize = 20_000;

/// What the file looked like on disk, to notice changes made by others (agents, git).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub len: u64,
    pub modified: Option<SystemTime>,
}

impl Stamp {
    pub fn of(path: &Path) -> Option<Self> {
        let meta = std::fs::metadata(path).ok()?;
        Some(Self { len: meta.len(), modified: meta.modified().ok() })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadOnly {
    /// Bigger than [`EDITABLE_LIMIT`].
    Large,
    /// Has a line longer than [`LONG_LINE`].
    LongLines,
    /// Not valid UTF-8: shown with replacement characters, and saving would change it.
    NotUtf8,
    /// The real file is outside the project (reached through a link).
    OutsideProject,
    /// The file system does not let us write it.
    NoPermission,
}

#[derive(Debug)]
pub enum Opened {
    Text(TextFile),
    /// Contains NUL bytes: an image, an archive, a compiled file…
    Binary,
    /// Bigger than [`OPEN_LIMIT`] (its size).
    TooLarge(u64),
    Failed(String),
}

#[derive(Debug)]
pub struct TextFile {
    pub text: String,
    pub format: Format,
    pub read_only: Option<ReadOnly>,
    /// Where the bytes really are (differs from the opened path for a link).
    pub target: PathBuf,
    pub stamp: Option<Stamp>,
}

/// How the file stored its text, kept when it is written back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Format {
    pub crlf: bool,
    pub bom: bool,
}

impl Format {
    /// Text (with `\n` line breaks) as the bytes to write.
    pub fn encode(self, text: &str) -> Vec<u8> {
        let mut out = Vec::with_capacity(text.len() + 3);
        if self.bom {
            out.extend_from_slice(b"\xEF\xBB\xBF");
        }
        if self.crlf {
            out.extend_from_slice(text.replace('\n', "\r\n").as_bytes());
        } else {
            out.extend_from_slice(text.as_bytes());
        }
        out
    }
}

/// The real location of `path` and whether it is outside `project` (both resolved through links).
pub fn resolve(path: &Path, project: &Path) -> (PathBuf, bool) {
    // A file that is gone (deleted, to be written again) resolves through its folder.
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| {
        match (path.parent().and_then(|p| std::fs::canonicalize(p).ok()), path.file_name()) {
            (Some(dir), Some(name)) => dir.join(name),
            _ => path.to_path_buf(),
        }
    });
    let root = std::fs::canonicalize(project).unwrap_or_else(|_| project.to_path_buf());
    let outside = !target.starts_with(&root);
    (target, outside)
}

pub fn open(path: &Path, project: &Path) -> Opened {
    let (target, outside) = resolve(path, project);
    let meta = match std::fs::metadata(&target) {
        Ok(meta) => meta,
        Err(err) => return Opened::Failed(err.to_string()),
    };
    if !meta.is_file() {
        return Opened::Failed("not a file".into());
    }
    if meta.len() > OPEN_LIMIT {
        return Opened::TooLarge(meta.len());
    }
    let bytes = match std::fs::read(&target) {
        Ok(bytes) => bytes,
        Err(err) => return Opened::Failed(err.to_string()),
    };
    if bytes[..bytes.len().min(8192)].contains(&0) {
        return Opened::Binary;
    }
    let (body, bom) = match bytes.strip_prefix(b"\xEF\xBB\xBF") {
        Some(rest) => (rest, true),
        None => (bytes.as_slice(), false),
    };
    let (text, utf8) = match std::str::from_utf8(body) {
        Ok(text) => (text.to_string(), true),
        Err(_) => (String::from_utf8_lossy(body).into_owned(), false),
    };
    // Mixed line endings: the more common one is kept.
    let crlf = text.matches("\r\n").count() * 2 > text.matches('\n').count();
    let text = if crlf { text.replace("\r\n", "\n") } else { text };
    let read_only = if !utf8 {
        Some(ReadOnly::NotUtf8)
    } else if outside {
        Some(ReadOnly::OutsideProject)
    } else if meta.len() > EDITABLE_LIMIT {
        Some(ReadOnly::Large)
    } else if text.split('\n').any(|line| line.len() > LONG_LINE) {
        Some(ReadOnly::LongLines)
    } else if meta.permissions().readonly() {
        Some(ReadOnly::NoPermission)
    } else {
        None
    };
    Opened::Text(TextFile { text, format: Format { crlf, bom }, read_only, target: target.clone(), stamp: Stamp::of(&target) })
}

/// Writes `bytes` to `target` (a real file, not a link) without ever leaving it half-written:
/// a temporary file in the same folder, flushed to disk, given the original's permissions, then
/// renamed over it. A file with other hard links is rewritten in place instead, so the links
/// keep seeing it.
pub fn save_atomic(target: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(target).ok();
    if meta.as_ref().is_some_and(|m| m.file_type().is_symlink()) {
        return Err(std::io::Error::other("refusing to replace a link; save to the file it points at"));
    }
    if meta.as_ref().is_some_and(hard_linked) {
        // Other links share this inode, so the file has to keep it and a rename is out. Writing in
        // place truncates first, so the old content is read aside and put back when the write fails
        // partway (a full disk, an I/O error): a save still never leaves half a file.
        let previous = std::fs::read(target)?;
        let open_in_place = || {
            let mut options = std::fs::OpenOptions::new();
            options.write(true).truncate(true);
            // Never through a link put there since the check above.
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::custom_flags(&mut options, libc::O_NOFOLLOW);
            options.open(target)
        };
        let mut file = open_in_place()?;
        let result = file.write_all(bytes).and_then(|()| file.sync_all());
        if result.is_err() {
            if let Ok(mut file) = open_in_place() {
                let _ = file.write_all(&previous).and_then(|()| file.sync_all());
            }
        }
        return result;
    }
    let dir = target.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = target.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let temp = dir.join(format!(".{name}.agentty-{}.tmp", uuid::Uuid::new_v4().simple()));
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        // Born with the original's permissions: a private file is never readable by others meanwhile.
        #[cfg(unix)]
        if let Some(meta) = &meta {
            use std::os::unix::fs::PermissionsExt;
            std::os::unix::fs::OpenOptionsExt::mode(&mut options, meta.permissions().mode() & 0o7777);
        }
        let mut file = options.open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        if let Some(meta) = &meta {
            std::fs::set_permissions(&temp, meta.permissions())?;
        }
        std::fs::rename(&temp, target)?;
        // The rename itself is only durable once the folder is flushed too.
        #[cfg(unix)]
        if let Ok(dir) = std::fs::File::open(dir) {
            let _ = dir.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

#[cfg(unix)]
fn hard_linked(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    meta.nlink() > 1
}

#[cfg(not(unix))]
fn hard_linked(_: &std::fs::Metadata) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("agentty-editor-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn text(opened: Opened) -> TextFile {
        match opened {
            Opened::Text(file) => file,
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[test]
    fn saving_replaces_the_file_and_leaves_no_temporary_behind() {
        let dir = temp_dir("save");
        let file = dir.join("a.txt");
        std::fs::write(&file, "old").unwrap();
        save_atomic(&file, b"new").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "new");
        let names: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(names, ["a.txt"]);
        // A new file can be written too.
        save_atomic(&dir.join("b.txt"), b"b").unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("b.txt")).unwrap(), "b");
        std::fs::remove_dir_all(dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn saving_keeps_the_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("mode");
        let file = dir.join("run.sh");
        std::fs::write(&file, "echo").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o750)).unwrap();
        save_atomic(&file, b"echo hi").unwrap();
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o750);
        let secret = dir.join("secret.env.example");
        std::fs::write(&secret, "a").unwrap();
        std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o600)).unwrap();
        save_atomic(&secret, b"b").unwrap();
        assert_eq!(std::fs::metadata(&secret).unwrap().permissions().mode() & 0o777, 0o600);
        std::fs::remove_dir_all(dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn links_are_written_through_and_outside_targets_are_read_only() {
        let dir = temp_dir("links");
        let project = dir.join("project");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("real.txt"), "inside").unwrap();
        std::fs::write(dir.join("secret.txt"), "outside").unwrap();
        std::os::unix::fs::symlink(project.join("real.txt"), project.join("link.txt")).unwrap();
        std::os::unix::fs::symlink(dir.join("secret.txt"), project.join("escape.txt")).unwrap();

        let inside = text(open(&project.join("link.txt"), &project));
        assert_eq!(inside.read_only, None);
        assert!(inside.target.ends_with("real.txt"));
        // Saving to the target keeps the link a link.
        save_atomic(&inside.target, b"changed").unwrap();
        assert!(std::fs::symlink_metadata(project.join("link.txt")).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_to_string(project.join("link.txt")).unwrap(), "changed");
        // The link itself is never replaced by a plain file.
        assert!(save_atomic(&project.join("link.txt"), b"x").is_err());

        let outside = text(open(&project.join("escape.txt"), &project));
        assert_eq!(outside.read_only, Some(ReadOnly::OutsideProject));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn line_endings_and_byte_order_mark_survive_a_round_trip() {
        let dir = temp_dir("format");
        let file = dir.join("win.txt");
        std::fs::write(&file, b"\xEF\xBB\xBFa\r\nb\r\n").unwrap();
        let opened = text(open(&file, &dir));
        assert_eq!(opened.text, "a\nb\n");
        assert_eq!(opened.format, Format { crlf: true, bom: true });
        save_atomic(&file, &opened.format.encode("a\nb\nc\n")).unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"\xEF\xBB\xBFa\r\nb\r\nc\r\n");
        // Mostly LF with one CRLF stays LF.
        std::fs::write(&file, "a\nb\nc\r\nd\n").unwrap();
        assert_eq!(text(open(&file, &dir)).format, Format { crlf: false, bom: false });
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn binary_large_and_odd_files() {
        let dir = temp_dir("kinds");
        std::fs::write(dir.join("img.png"), b"\x89PNG\x00\x01").unwrap();
        assert!(matches!(open(&dir.join("img.png"), &dir), Opened::Binary));
        std::fs::write(dir.join("latin1.txt"), b"caf\xe9").unwrap();
        assert_eq!(text(open(&dir.join("latin1.txt"), &dir)).read_only, Some(ReadOnly::NotUtf8));
        std::fs::write(dir.join("min.js"), "x".repeat(LONG_LINE + 1)).unwrap();
        assert_eq!(text(open(&dir.join("min.js"), &dir)).read_only, Some(ReadOnly::LongLines));
        let big = "a\n".repeat(EDITABLE_LIMIT as usize / 2 + 1);
        std::fs::write(dir.join("big.log"), big).unwrap();
        assert_eq!(text(open(&dir.join("big.log"), &dir)).read_only, Some(ReadOnly::Large));
        assert!(matches!(open(&dir.join("missing.txt"), &dir), Opened::Failed(_)));
        std::fs::remove_dir_all(dir).ok();
    }
}
