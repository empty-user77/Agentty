use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

pub fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

pub fn mtime_ms(path: &Path) -> u64 {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Collects `*.jsonl` files under `dir`, descending at most `depth` levels.
pub fn jsonl_files(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() {
            if depth > 0 {
                jsonl_files(&path, depth - 1, out);
            }
        } else if path.extension().is_some_and(|e| e == "jsonl") {
            out.push(path);
        }
    }
}

/// Newest first, truncated to `limit`.
pub fn newest(mut files: Vec<PathBuf>, limit: usize) -> Vec<(PathBuf, u64)> {
    let mut with_time: Vec<_> = files
        .drain(..)
        .map(|p| {
            let t = mtime_ms(&p);
            (p, t)
        })
        .collect();
    with_time.sort_by_key(|a| std::cmp::Reverse(a.1));
    with_time.truncate(limit);
    with_time
}

pub fn truncate_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

pub fn one_line(s: &str, max: usize) -> String {
    let flat: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_chars(&flat, max)
}

/// Agentty's data directory: `$AGENTTY_DATA_DIR` or `~/.agentty`.
pub fn data_dir() -> PathBuf {
    std::env::var_os("AGENTTY_DATA_DIR").map(PathBuf::from).unwrap_or_else(|| home().join(".agentty"))
}

/// Lines from the last `bytes` of a file, newest first (the partial first line is dropped).
pub fn tail_lines_rev(path: &Path, bytes: u64) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) = fs::File::open(path) else { return Vec::new() };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(bytes);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let mut buffer = Vec::new();
    if file.read_to_end(&mut buffer).is_err() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&buffer);
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    if start > 0 && !lines.is_empty() {
        lines.remove(0);
    }
    lines.reverse();
    lines
}

/// Per-file values keyed by path, with the modification time they were computed at.
pub type MtimeCache<T> = std::sync::Mutex<std::collections::HashMap<PathBuf, (u64, T)>>;

/// Remembers a value per file until the file changes (by modification time).
pub fn cached_by_mtime<T: Clone + Send + 'static>(cache: &MtimeCache<T>, path: &Path, mtime: u64, compute: impl FnOnce() -> T) -> T {
    if let Some((at, value)) = cache.lock().ok().and_then(|c| c.get(path).cloned()) {
        if at == mtime {
            return value;
        }
    }
    let value = compute();
    if let Ok(mut c) = cache.lock() {
        c.insert(path.to_path_buf(), (mtime, value.clone()));
    }
    value
}

/// Creates `dir` (and any missing parents) so only the user can enter it: `0700` on Unix from the
/// moment each folder is made, not chmodded afterwards. An existing `dir` that group or others can
/// reach is tightened to `0700` (the system refuses this for a folder the user doesn't own, so only
/// Agentty's own folders change); parents that already exist are left as they are.
pub fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(dir)?.permissions().mode();
        if mode & 0o077 != 0 {
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}

/// Makes the data directory and the folders in it that hold conversation text or recordings
/// private, at startup: created `0700`, and tightened if an older version (or `create_dir_all`)
/// left them open to group or others. Folders that don't exist yet are created when first used.
pub fn secure_data_dir() {
    let dir = data_dir();
    if let Err(err) = create_private_dir(&dir) {
        eprintln!("agentty: could not make {} private: {err}", dir.display());
        return;
    }
    for sub in ["handoffs", "voice"] {
        let path = dir.join(sub);
        if path.is_dir() {
            if let Err(err) = create_private_dir(&path) {
                eprintln!("agentty: could not make {} private: {err}", path.display());
            }
        }
    }
}

fn private_options() -> fs::OpenOptions {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

/// A new file only the user can read (`0600` on Unix from the start), replacing one left there.
/// Missing parent folders are created private (`0700`).
pub fn create_private(path: &Path) -> std::io::Result<fs::File> {
    if let Some(parent) = path.parent() {
        create_private_dir(parent)?;
    }
    let _ = fs::remove_file(path);
    private_options().open(path)
}

/// Writes a file only the user can read: created `0600` on Unix under a temporary name in the same
/// folder before any content is written, flushed to disk, then renamed over `path` — so a reader
/// never sees half of it, a power cut leaves the old file or the new one, and a file an older
/// version left world-readable is replaced by a private one. Missing parent folders are created
/// private (`0700`).
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    // Unique per write, so two threads (or two Agentty processes on one folder) saving the same
    // file never share a temporary file.
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    create_private_dir(dir)?;
    let name = path.file_name().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "no file name"))?;
    let mut tmp_name = std::ffi::OsString::from(".");
    tmp_name.push(name);
    tmp_name.push(format!(".{}.{}.tmp", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let tmp = dir.join(tmp_name);
    let written = (|| {
        let mut file = private_options().open(&tmp)?;
        std::io::Write::write_all(&mut file, bytes)?;
        file.sync_all()
    })();
    if let Err(err) = written {
        let _ = fs::remove_file(&tmp);
        return Err(err);
    }
    // Windows: a virus scanner or the search indexer can hold the file for a moment.
    let mut attempt = 0;
    loop {
        match fs::rename(&tmp, path) {
            Ok(()) => return Ok(()),
            Err(_) if cfg!(windows) && attempt < 3 => {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(50 * attempt));
            }
            Err(err) => {
                let _ = fs::remove_file(&tmp);
                return Err(err);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("agentty-fsutil-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn write_private_creates_the_file_and_its_folder_private() {
        let root = scratch("create");
        let path = root.join("nested").join("settings.json");
        write_private(&path, b"{\"a\":1}").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"{\"a\":1}");
        #[cfg(unix)]
        {
            assert_eq!(mode(&path), 0o600);
            assert_eq!(mode(&root.join("nested")), 0o700);
            assert_eq!(mode(&root), 0o700);
        }
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn write_private_replaces_an_open_file_atomically() {
        let root = scratch("replace");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("workspaces.json");
        fs::write(&path, b"old").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        }
        // A reader holding the old file keeps seeing the old content: the new one is a different
        // file renamed into place, not the old one truncated and rewritten.
        let held = fs::File::open(&path).unwrap();
        write_private(&path, b"new").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new");
        let mut old = String::new();
        std::io::Read::read_to_string(&mut &held, &mut old).unwrap();
        assert_eq!(old, "old");
        #[cfg(unix)]
        assert_eq!(mode(&path), 0o600);
        // No temporary file is left behind.
        let names: Vec<_> = fs::read_dir(&root).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(names, vec![std::ffi::OsString::from("workspaces.json")]);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn write_private_failure_leaves_the_old_file() {
        let root = scratch("fail");
        // The target is a non-empty folder, so the rename fails after the temporary file is written.
        let path = root.join("taken");
        fs::create_dir_all(path.join("inside")).unwrap();
        assert!(write_private(&path, b"x").is_err());
        assert!(path.join("inside").is_dir());
        let names: Vec<_> = fs::read_dir(&root).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(names, vec![std::ffi::OsString::from("taken")]);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn concurrent_private_writes_of_one_file_all_land() {
        let root = scratch("concurrent");
        let path = root.join("board.json");
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || write_private(&path, format!("{i}").as_bytes()))
            })
            .collect();
        for thread in threads {
            thread.join().unwrap().unwrap();
        }
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.parse::<u32>().unwrap() < 8);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn create_private_is_private_from_the_start() {
        let root = scratch("create-private");
        let path = root.join("a.log");
        let file = create_private(&path).unwrap();
        drop(file);
        #[cfg(unix)]
        {
            assert_eq!(mode(&path), 0o600);
            assert_eq!(mode(&root), 0o700);
        }
        fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn create_private_dir_tightens_an_open_folder() {
        use std::os::unix::fs::PermissionsExt;
        let root = scratch("tighten");
        fs::create_dir_all(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
        create_private_dir(&root).unwrap();
        assert_eq!(mode(&root), 0o700);
        // Already private: left alone (and no error).
        create_private_dir(&root).unwrap();
        assert_eq!(mode(&root), 0o700);
        fs::remove_dir_all(&root).unwrap();
    }
}
