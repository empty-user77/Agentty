//! Sync into a SQLite file on this computer instead of a git repository: no account, no remote,
//! nothing to set up. The file is the store, as the remote is for git, and `<data dir>/sync/repo`
//! stays the working tree every other part of the sync reads and writes; a sync makes the tree the
//! file's content first and stores what changed after, like a fetch, reset and commit.
//!
//! The file holds every path of the tree with its bytes (`files`), so the file alone is the whole
//! backup: with the tree deleted, the next sync writes it out again, and a copy of the file brought
//! to another computer (as its own sync file) brings every session back for "Continue".
//!
//! Paths read from the file are written into the tree only when they stay inside it (no `..`, no
//! root): the file may come from elsewhere. It is created `0600`; the data it holds is what a git
//! repository would hold, transcripts with secrets masked.

use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::{BTreeMap, HashMap};
use std::path::{Component, Path, PathBuf};

/// The file's own format, kept in `meta`.
const SCHEMA_VERSION: &str = "1";

/// The sync file and the working tree that mirrors it.
pub struct SqliteStore {
    pub dir: PathBuf,
    pub file: PathBuf,
}

/// A stored path's size and modification time as last seen in the tree: what has not changed since
/// is neither read nor written again (as git's index does).
#[derive(Clone, Copy, PartialEq, Eq)]
struct Seen {
    size: u64,
    mtime_ns: i64,
}

impl SqliteStore {
    pub fn new(dir: PathBuf, file: PathBuf) -> Self {
        Self { dir, file }
    }

    /// Opens the file, made (private) with its tables the first time.
    fn open(&self) -> Result<Connection> {
        if let Some(parent) = self.file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if !self.file.exists() {
            // Made private before SQLite writes anything into it.
            crate::fsutil::write_private(&self.file, b"")?;
        }
        let db = Connection::open(&self.file).with_context(|| format!("couldn't open {}", self.file.display()))?;
        db.busy_timeout(std::time::Duration::from_secs(10))?;
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS files (
                 path TEXT PRIMARY KEY,
                 size INTEGER NOT NULL,
                 mtime_ns INTEGER NOT NULL,
                 content BLOB NOT NULL
             );",
        )?;
        let version: Option<String> = db.query_row("SELECT value FROM meta WHERE key = 'schema'", [], |r| r.get(0)).optional()?;
        match version.as_deref() {
            None => {
                db.execute("INSERT INTO meta (key, value) VALUES ('schema', ?1)", [SCHEMA_VERSION])?;
                db.execute("INSERT OR IGNORE INTO meta (key, value) VALUES ('kind', 'agentty-sync')", [])?;
            }
            Some(SCHEMA_VERSION) => {}
            Some(other) => bail!("{} was made by a newer Agentty (format {other})", self.file.display()),
        }
        Ok(db)
    }

    /// The working tree is there (the file is made when first opened).
    pub fn is_cloned(&self) -> bool {
        self.dir.is_dir()
    }

    /// Makes the working tree, private to the user, filled from the file.
    pub fn ensure_clone(&self) -> Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        if let Some(parent) = self.dir.parent() {
            super::repo::private_dir(parent);
        }
        super::repo::private_dir(&self.dir);
        self.open()?;
        self.reset_to_remote()
    }

    /// Nothing to fetch: the file is on this computer.
    pub fn fetch(&self) -> Result<()> {
        Ok(())
    }

    /// Makes the working tree exactly the file's content: missing or changed files written from
    /// it, files it doesn't hold removed. A file with nothing stored yet (new, or deleted by
    /// the user) takes the tree as it is instead of emptying it; the next commit stores it.
    pub fn reset_to_remote(&self) -> Result<()> {
        let mut db = self.open()?;
        let stored = stored(&db)?;
        if stored.is_empty() {
            return Ok(());
        }
        let on_disk = tree_files(&self.dir);
        for (path, _) in on_disk.iter().filter(|(path, _)| !stored.contains_key(*path)) {
            let _ = std::fs::remove_file(self.dir.join(path));
        }
        let tx = db.transaction()?;
        for (path, seen) in &stored {
            if on_disk.get(path) == Some(seen) {
                continue;
            }
            let target = inside(&self.dir, path)?;
            let content: Vec<u8> = tx.query_row("SELECT content FROM files WHERE path = ?1", [path], |r| r.get(0))?;
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            crate::fsutil::write_private(&target, &content)?;
            if let Some(now) = seen_at(&target) {
                tx.execute("UPDATE files SET size = ?2, mtime_ns = ?3 WHERE path = ?1", params![path, now.size as i64, now.mtime_ns])?;
            }
        }
        tx.commit()?;
        remove_empty_dirs(&self.dir);
        Ok(())
    }

    /// Stores what changed in the tree since the last time, in one transaction. `false` when nothing
    /// did.
    pub fn commit_all(&self) -> Result<bool> {
        let mut db = self.open()?;
        let stored = stored(&db)?;
        let on_disk = tree_files(&self.dir);
        let tx = db.transaction()?;
        let mut changed = false;
        for (path, seen) in &on_disk {
            if stored.get(path) == Some(seen) {
                continue;
            }
            let content = std::fs::read(self.dir.join(path))?;
            tx.execute(
                "INSERT INTO files (path, size, mtime_ns, content) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(path) DO UPDATE SET size = excluded.size, mtime_ns = excluded.mtime_ns, content = excluded.content",
                params![path, seen.size as i64, seen.mtime_ns, content],
            )?;
            changed = true;
        }
        for path in stored.keys().filter(|path| !on_disk.contains_key(*path)) {
            tx.execute("DELETE FROM files WHERE path = ?1", [path])?;
            changed = true;
        }
        if changed {
            tx.execute(
                "INSERT INTO meta (key, value) VALUES ('updatedAt', ?1) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [super::model::now_stamp()],
            )?;
        }
        tx.commit()?;
        Ok(changed)
    }

    /// Files in the working tree other than a README: nothing a sync wrote.
    pub fn is_empty_or_readme_only(&self) -> bool {
        let Ok(entries) = std::fs::read_dir(&self.dir) else { return true };
        entries.flatten().all(|e| super::repo::setup_file(&e.file_name().to_string_lossy()))
    }

    /// The file's size on disk.
    pub fn size(&self) -> u64 {
        std::fs::metadata(&self.file).map(|m| m.len()).unwrap_or(0)
    }
}

/// Every stored path with what was last seen of it.
fn stored(db: &Connection) -> Result<BTreeMap<String, Seen>> {
    let mut statement = db.prepare("SELECT path, size, mtime_ns FROM files")?;
    let rows =
        statement.query_map([], |r| Ok((r.get::<_, String>(0)?, Seen { size: r.get::<_, i64>(1)?.max(0) as u64, mtime_ns: r.get(2)? })))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Every file under `dir` by its path relative to it (`/` between parts, on every platform).
fn tree_files(dir: &Path) -> HashMap<String, Seen> {
    let mut out = HashMap::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(folder) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() {
                pending.push(path);
            } else if kind.is_file() {
                let (Ok(relative), Some(seen)) = (path.strip_prefix(dir), seen_at(&path)) else { continue };
                let parts: Vec<String> = relative.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
                out.insert(parts.join("/"), seen);
            }
        }
    }
    out
}

fn seen_at(path: &Path) -> Option<Seen> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime_ns = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX));
    Some(Seen { size: meta.len(), mtime_ns })
}

/// `relative` under `dir`, refused when it would land anywhere else.
fn inside(dir: &Path, relative: &str) -> Result<PathBuf> {
    let path = Path::new(relative);
    if relative.is_empty() || !path.components().all(|c| matches!(c, Component::Normal(_))) {
        bail!("refused a stored path outside the sync folder: {relative}");
    }
    Ok(dir.join(path))
}

/// Folders left empty by removed files (the tree itself stays).
fn remove_empty_dirs(dir: &Path) {
    fn walk(folder: &Path) -> bool {
        let Ok(entries) = std::fs::read_dir(folder) else { return false };
        let mut empty = true;
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_type().is_ok_and(|k| k.is_dir()) && walk(&path) {
                let _ = std::fs::remove_dir(&path);
            } else {
                empty = false;
            }
        }
        empty
    }
    walk(dir);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("agentty-sqlite-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn stores_the_tree_and_brings_it_back() {
        let base = temp("roundtrip");
        let store = SqliteStore::new(base.join("tree"), base.join("sessions.sqlite"));
        store.ensure_clone().unwrap();
        assert!(store.is_empty_or_readme_only());
        std::fs::create_dir_all(store.dir.join("workspace/a")).unwrap();
        std::fs::write(store.dir.join("workspace/a/0001.jsonl"), b"{\"x\":1}\n\0binary").unwrap();
        std::fs::write(store.dir.join("README.md"), "hi").unwrap();
        assert!(store.commit_all().unwrap());
        assert!(!store.commit_all().unwrap(), "nothing changed");

        // The tree gone: the file writes it out again.
        std::fs::remove_dir_all(&store.dir).unwrap();
        store.ensure_clone().unwrap();
        assert_eq!(std::fs::read(store.dir.join("workspace/a/0001.jsonl")).unwrap(), b"{\"x\":1}\n\0binary");
        assert!(!store.commit_all().unwrap(), "written back as stored");

        // A file the sync left behind without storing it goes; a changed one is put back.
        std::fs::write(store.dir.join("stray.json"), "{}").unwrap();
        std::fs::write(store.dir.join("README.md"), "changed").unwrap();
        store.reset_to_remote().unwrap();
        assert!(!store.dir.join("stray.json").exists());
        assert_eq!(std::fs::read_to_string(store.dir.join("README.md")).unwrap(), "hi");

        // Removed from the tree and committed: gone from the file, its folder too after a reset.
        std::fs::remove_file(store.dir.join("workspace/a/0001.jsonl")).unwrap();
        assert!(store.commit_all().unwrap());
        store.reset_to_remote().unwrap();
        assert!(!store.dir.join("workspace").exists());
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn a_new_file_keeps_the_tree_it_finds() {
        let base = temp("adopt");
        let store = SqliteStore::new(base.join("tree"), base.join("sessions.sqlite"));
        std::fs::create_dir_all(&store.dir).unwrap();
        std::fs::write(store.dir.join("kept.json"), "{}").unwrap();
        store.ensure_clone().unwrap();
        assert!(store.dir.join("kept.json").exists());
        assert!(store.commit_all().unwrap());
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn refuses_stored_paths_outside_the_tree() {
        let base = temp("escape");
        let store = SqliteStore::new(base.join("tree"), base.join("sessions.sqlite"));
        store.ensure_clone().unwrap();
        let db = store.open().unwrap();
        db.execute("INSERT INTO files (path, size, mtime_ns, content) VALUES ('../outside.txt', 1, 0, x'41')", []).unwrap();
        drop(db);
        assert!(store.reset_to_remote().is_err());
        assert!(!base.join("outside.txt").exists());
        assert!(inside(&store.dir, "/etc/passwd").is_err());
        assert!(inside(&store.dir, "a/../../b").is_err());
        assert!(inside(&store.dir, "a/b.json").is_ok());
        std::fs::remove_dir_all(base).ok();
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let base = temp("private");
        let store = SqliteStore::new(base.join("tree"), base.join("sessions.sqlite"));
        store.ensure_clone().unwrap();
        assert_eq!(std::fs::metadata(&store.file).unwrap().permissions().mode() & 0o777, 0o600);
        std::fs::remove_dir_all(base).ok();
    }
}
