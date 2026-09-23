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

/// Writes a file only the user can read: created `0600` on Unix before any content is written,
/// then moved into place so a reader never sees half of it.
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp)?;
    std::io::Write::write_all(&mut file, bytes)?;
    drop(file);
    std::fs::rename(tmp, path)
}
