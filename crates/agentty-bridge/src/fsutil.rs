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
/// reach is tightened to `0700` (see [`tighten_dir`]); parents that already exist are left as they
/// are. For Agentty's own folders only: other folders go through [`create_new_dirs`].
pub fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    create_new_dirs(dir)?;
    tighten_dir(dir);
    Ok(())
}

/// Creates the missing folders of `dir`, each `0700` on Unix from the moment it is made. Folders
/// that already exist (`dir` included) keep their permissions.
fn create_new_dirs(dir: &Path) -> std::io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)
}

/// Narrows `dir` to `0700` when group or others can reach it — only a real folder (a symlink is
/// never followed) the user owns. A tightening the system refuses (an SMB or exFAT volume) is
/// logged, never an error: the files in it are still written `0600`.
#[cfg(unix)]
fn tighten_dir(dir: &Path) {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
    // Opened without following a symlink, then checked and changed through that handle, so the
    // folder can't be swapped for a link between the check and the chmod.
    let Ok(handle) = fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY).open(dir) else { return };
    let Ok(meta) = handle.metadata() else { return };
    if !meta.is_dir() || meta.uid() != unsafe { libc::getuid() } || meta.mode() & 0o077 == 0 {
        return;
    }
    if let Err(err) = handle.set_permissions(fs::Permissions::from_mode(0o700)) {
        eprintln!("agentty: could not make {} private: {err}", dir.display());
    }
}

#[cfg(not(unix))]
fn tighten_dir(_dir: &Path) {}

/// Readies the folder a private file goes into. Agentty's own folders (under `own`, the data
/// directory) are made private; any other folder — a backup's destination, an agent's session
/// folder — only gets its missing parts created, and is never narrowed: the file itself is `0600`.
fn prepare_dir(dir: &Path, own: &Path) -> std::io::Result<()> {
    // Compared by path components: a `..` could step out of `own` while still starting with it.
    let inside = dir.starts_with(own) && !dir.components().any(|c| c == std::path::Component::ParentDir);
    if inside {
        create_private_dir(dir)
    } else {
        create_new_dirs(dir)
    }
}

/// Makes the data directory and the folders in it that hold conversation text or recordings
/// private, at startup: created `0700`, and tightened if an older version (or `create_dir_all`)
/// left them open to group or others — unless the data directory is a symlink or someone else's
/// folder (`AGENTTY_DATA_DIR` on a shared folder). Folders that don't exist yet are created when
/// first used. Also clears temporary files an interrupted save left behind.
pub fn secure_data_dir() {
    let dir = data_dir();
    if let Err(err) = create_new_dirs(&dir) {
        eprintln!("agentty: could not create {}: {err}", dir.display());
        return;
    }
    tighten_dir(&dir);
    for sub in ["handoffs", "voice", "limits"] {
        tighten_dir(&dir.join(sub));
    }
    std::thread::spawn(move || sweep_stale_temp_files(&dir, 2, STALE_TEMP_AGE));
}

/// How old a temporary file of [`write_private`] must be before it counts as left behind: a save
/// takes milliseconds, so anything this old belongs to a save that never finished.
const STALE_TEMP_AGE: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// Whether `name` is a temporary file [`write_private`] makes: `.<name>.<pid>.<n>.tmp`.
fn is_private_temp_name(name: &str) -> bool {
    let Some(inner) = name.strip_prefix('.').and_then(|n| n.strip_suffix(".tmp")) else { return false };
    let mut parts = inner.rsplitn(3, '.');
    let digits = |p: Option<&str>| p.is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    digits(parts.next()) && digits(parts.next()) && parts.next().is_some_and(|n| !n.is_empty())
}

/// Removes temporary files of [`write_private`] older than `age` in `dir` and its folders down to
/// `depth` levels: only plain files named as it names them; symlinks are never followed.
fn sweep_stale_temp_files(dir: &Path, depth: usize, age: std::time::Duration) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() {
            if depth > 0 {
                sweep_stale_temp_files(&entry.path(), depth - 1, age);
            }
            continue;
        }
        if !ft.is_file() || !entry.file_name().to_str().is_some_and(is_private_temp_name) {
            continue;
        }
        let old = entry.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|e| e >= age);
        if old {
            let _ = fs::remove_file(entry.path());
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
/// Missing parent folders are created private (`0700`), and Agentty's own folders are tightened.
pub fn create_private(path: &Path) -> std::io::Result<fs::File> {
    if let Some(parent) = path.parent() {
        prepare_dir(parent, &data_dir())?;
    }
    let _ = fs::remove_file(path);
    private_options().open(path)
}

/// Writes a file only the user can read: created `0600` on Unix under a temporary name in the same
/// folder before any content is written, then renamed over `path` — so a reader never sees half of
/// it, and a file an older version left world-readable is replaced by a private one. Missing parent
/// folders are created private (`0700`); an existing one is tightened only when it is Agentty's own
/// (in the data directory), never a folder the user picked. Not flushed to disk: cheap enough for
/// the UI thread.
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_private_with(path, bytes, false)
}

/// [`write_private`], flushed to disk before it replaces `path`, so a power cut leaves the old file
/// or the new one, never an empty one. The flush is slow on macOS (`F_FULLFSYNC`): for files whose
/// loss costs the user (the window layout), not for every small save.
pub fn write_private_durable(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_private_with(path, bytes, true)
}

fn write_private_with(path: &Path, bytes: &[u8], durable: bool) -> std::io::Result<()> {
    write_private_in(path, bytes, durable, &data_dir())
}

/// [`write_private_with`], with `own` as Agentty's own folder (the data directory outside tests).
fn write_private_in(path: &Path, bytes: &[u8], durable: bool, own: &Path) -> std::io::Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    // Unique per write, so two threads (or two Agentty processes on one folder) saving the same
    // file never share a temporary file.
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    prepare_dir(dir, own)?;
    let name = path.file_name().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "no file name"))?;
    let mut tmp_name = std::ffi::OsString::from(".");
    tmp_name.push(name);
    tmp_name.push(format!(".{}.{}.tmp", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let tmp = dir.join(tmp_name);
    let written = (|| {
        let mut file = private_options().open(&tmp)?;
        std::io::Write::write_all(&mut file, bytes)?;
        if durable {
            file.sync_all()?;
        }
        Ok(())
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
        write_private_durable(&path, b"new").unwrap();
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

    #[cfg(unix)]
    #[test]
    fn a_folder_outside_the_data_dir_is_never_narrowed() {
        use std::os::unix::fs::PermissionsExt;
        let root = scratch("outside");
        let own = root.join("data");
        let desktop = root.join("Desktop");
        fs::create_dir_all(&desktop).unwrap();
        fs::set_permissions(&desktop, fs::Permissions::from_mode(0o755)).unwrap();
        write_private_in(&desktop.join("backup.agentty"), b"x", false, &own).unwrap();
        assert_eq!(mode(&desktop), 0o755);
        assert_eq!(mode(&desktop.join("backup.agentty")), 0o600);
        // A folder that doesn't exist yet is still made private.
        write_private_in(&desktop.join("new").join("a.json"), b"x", false, &own).unwrap();
        assert_eq!(mode(&desktop.join("new")), 0o700);
        assert_eq!(mode(&desktop), 0o755);
        // Nor through a `..` that only looks like it is inside.
        write_private_in(&own.join("..").join("Desktop").join("b.json"), b"x", false, &own).unwrap();
        assert_eq!(mode(&desktop), 0o755);
        fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_inside_the_data_dir_is_tightened() {
        use std::os::unix::fs::PermissionsExt;
        let root = scratch("inside");
        let own = root.join("data");
        let handoffs = own.join("handoffs");
        fs::create_dir_all(&handoffs).unwrap();
        fs::set_permissions(&handoffs, fs::Permissions::from_mode(0o755)).unwrap();
        write_private_in(&handoffs.join("h.md"), b"x", false, &own).unwrap();
        assert_eq!(mode(&handoffs), 0o700);
        fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_folder_is_not_followed_when_tightening() {
        use std::os::unix::fs::PermissionsExt;
        let root = scratch("symlink");
        let shared = root.join("shared");
        fs::create_dir_all(&shared).unwrap();
        fs::set_permissions(&shared, fs::Permissions::from_mode(0o755)).unwrap();
        let own = root.join("data");
        std::os::unix::fs::symlink(&shared, &own).unwrap();
        write_private_in(&own.join("settings.json"), b"x", false, &own).unwrap();
        assert_eq!(mode(&shared), 0o755);
        assert_eq!(mode(&shared.join("settings.json")), 0o600);
        create_private_dir(&own).unwrap();
        assert_eq!(mode(&shared), 0o755);
        fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn someone_elses_folder_is_left_alone_without_an_error() {
        // `/tmp` (resolved past the macOS symlink) belongs to root and is open to everyone.
        let tmp = fs::canonicalize("/tmp").unwrap();
        if unsafe { libc::getuid() } == 0 {
            return;
        }
        let before = mode(&tmp);
        create_private_dir(&tmp).unwrap();
        assert_eq!(mode(&tmp), before);
    }

    #[test]
    fn recognizes_only_its_own_temporary_names() {
        assert!(is_private_temp_name(".settings.json.123.0.tmp"));
        assert!(is_private_temp_name(".a.b.c.42.7.tmp"));
        assert!(!is_private_temp_name("settings.json.123.0.tmp"));
        assert!(!is_private_temp_name(".settings.json.tmp"));
        assert!(!is_private_temp_name(".123.0.tmp"));
        assert!(!is_private_temp_name(".notes.x.0.tmp"));
        assert!(!is_private_temp_name(".settings.json.123.0.tmp.bak"));
    }

    #[test]
    fn stale_temporary_files_are_swept() {
        let root = scratch("sweep");
        let nested = root.join("workspaces");
        fs::create_dir_all(&nested).unwrap();
        let stale = [root.join(".settings.json.1.0.tmp"), nested.join(".board.json.2.5.tmp")];
        let kept = [root.join("settings.json"), root.join("user.tmp"), root.join(".x.tmp")];
        for path in stale.iter().chain(&kept) {
            fs::write(path, b"x").unwrap();
        }
        // Young files stay: a save in another process may still be writing one.
        sweep_stale_temp_files(&root, 2, std::time::Duration::from_secs(3600));
        assert!(stale.iter().all(|p| p.exists()));
        sweep_stale_temp_files(&root, 2, std::time::Duration::ZERO);
        assert!(stale.iter().all(|p| !p.exists()));
        assert!(kept.iter().all(|p| p.exists()));
        fs::remove_dir_all(&root).unwrap();
    }
}
