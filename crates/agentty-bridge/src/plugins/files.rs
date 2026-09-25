//! `files/*`: a folder of the plugin's own, to keep files in (`files` permission).
//!
//! A WebAssembly plugin has no file system; one that collects pictures and videos, or hands an
//! agent a file to work on, needs one. It gets exactly one folder, `plugin-data/<id>/files`, and
//! every path it names is a path inside it: relative, made of plain names, never through a link.
//! What it keeps there is the user's alone (`0700` folder, `0600` files).

use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

/// Bytes one `files/write` carries (a message is capped at 16 MB, and this is base64 in it).
pub const MAX_WRITE: usize = 8 * 1024 * 1024;
/// Bytes one `files/read` returns.
pub const MAX_READ: usize = 4 * 1024 * 1024;
/// A file the plugin keeps, however it got there.
pub const MAX_FILE: u64 = 1024 * 1024 * 1024;
const MAX_PATH: usize = 512;
const MAX_NAME: usize = 128;
const MAX_DEPTH: usize = 16;
const MAX_LIST: usize = 5000;

/// The plugin's folder, made if it is not there yet.
pub fn root(plugin: &str) -> Result<PathBuf> {
    let root = super::store::plugin_data_dir(plugin).join("files");
    if !root.is_dir() {
        std::fs::create_dir_all(&root).context("could not make the plugin's folder")?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700));
    }
    Ok(root)
}

/// `relative` as a path inside the plugin's folder. `""` or `"."` is the folder itself.
pub fn resolve(plugin: &str, relative: &str) -> Result<PathBuf> {
    let root = root(plugin)?;
    let joined = inside(&root, relative)?;
    // No link on the way: a plugin that put one there (it cannot make one, but a file could come
    // from anywhere) must not reach through it.
    let mut at = root.clone();
    for part in joined.strip_prefix(&root).unwrap_or(Path::new("")).components() {
        at.push(part);
        if std::fs::symlink_metadata(&at).is_ok_and(|m| m.file_type().is_symlink()) {
            bail!("{relative} goes through a link");
        }
    }
    Ok(joined)
}

/// The checks on a path's text, apart from the disk.
fn inside(root: &Path, relative: &str) -> Result<PathBuf> {
    if relative.len() > MAX_PATH || relative.contains('\0') || relative.contains('\\') {
        bail!("that is not a path in the plugin's folder");
    }
    let path = Path::new(relative);
    let mut out = root.to_path_buf();
    let mut depth = 0;
    for part in path.components() {
        match part {
            Component::Normal(name) => {
                let name = name.to_str().context("a path is text")?;
                if name.len() > MAX_NAME || name.chars().any(char::is_control) {
                    bail!("{relative}: a name is too long or has control characters");
                }
                depth += 1;
                out.push(name);
            }
            Component::CurDir => {}
            _ => bail!("{relative} is not a path inside the plugin's folder"),
        }
    }
    if depth > MAX_DEPTH {
        bail!("{relative} is more than {MAX_DEPTH} folders deep");
    }
    Ok(out)
}

fn parent_made(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

fn private_options() -> std::fs::OpenOptions {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut options = std::fs::OpenOptions::new();
        options.mode(0o600);
        options
    }
    #[cfg(not(unix))]
    std::fs::OpenOptions::new()
}

/// Writes `bytes` to the file, replacing it, or after what is there with `append`. Folders on the
/// way are made. Returns the file's size after.
pub fn write(plugin: &str, relative: &str, bytes: &[u8], append: bool) -> Result<u64> {
    if bytes.len() > MAX_WRITE {
        bail!("one write is at most {MAX_WRITE} bytes; append the rest");
    }
    let path = resolve(plugin, relative)?;
    if path.is_dir() {
        bail!("{relative} is a folder");
    }
    parent_made(&path)?;
    if append {
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if size + bytes.len() as u64 > MAX_FILE {
            bail!("a file is at most {MAX_FILE} bytes");
        }
        let mut file = private_options().append(true).create(true).open(&path)?;
        file.write_all(bytes)?;
        return Ok(size + bytes.len() as u64);
    }
    // Whole or not at all: written beside it, then moved into place.
    let tmp =
        path.with_file_name(format!(".{}.tmp-{}", path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default(), std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let mut file = private_options().write(true).create_new(true).open(&tmp)?;
    file.write_all(bytes)?;
    drop(file);
    std::fs::rename(&tmp, &path)?;
    Ok(bytes.len() as u64)
}

/// Up to `length` bytes of the file from `offset`, and the file's whole size.
pub fn read(plugin: &str, relative: &str, offset: u64, length: usize) -> Result<(Vec<u8>, u64)> {
    let path = resolve(plugin, relative)?;
    let mut file = std::fs::File::open(&path).with_context(|| format!("{relative} is not there"))?;
    let size = file.metadata()?.len();
    file.seek(SeekFrom::Start(offset.min(size)))?;
    let mut bytes = Vec::new();
    file.take(length.min(MAX_READ) as u64).read_to_end(&mut bytes)?;
    Ok((bytes, size))
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub name: String,
    pub dir: bool,
    pub size: u64,
    pub modified_ms: u64,
}

fn entry_of(name: String, meta: &std::fs::Metadata) -> Entry {
    let modified_ms = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as u64);
    Entry { name, dir: meta.is_dir(), size: if meta.is_dir() { 0 } else { meta.len() }, modified_ms }
}

/// What is in a folder (names in order). A folder not made yet is empty.
pub fn list(plugin: &str, relative: &str) -> Result<Vec<Entry>> {
    let path = resolve(plugin, relative)?;
    let Ok(read) = std::fs::read_dir(&path) else { return Ok(Vec::new()) };
    let mut entries = Vec::new();
    for item in read.flatten().take(MAX_LIST) {
        let name = item.file_name().to_string_lossy().into_owned();
        // Half-written files of `write` and `download` are not the plugin's to see.
        if name.starts_with('.') || name.ends_with("part") && name.contains(".part") {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(item.path()) else { continue };
        if meta.file_type().is_symlink() {
            continue;
        }
        entries.push(entry_of(name, &meta));
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(entries)
}

/// The file or folder, if it is there.
pub fn stat(plugin: &str, relative: &str) -> Result<Option<Entry>> {
    let path = resolve(plugin, relative)?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(std::fs::symlink_metadata(&path).ok().map(|meta| entry_of(name, &meta)))
}

/// Removes a file, or a folder with all it holds. The plugin's folder itself stays.
pub fn remove(plugin: &str, relative: &str) -> Result<bool> {
    let path = resolve(plugin, relative)?;
    if path == root(plugin)? {
        bail!("the plugin's folder itself is not removed");
    }
    match std::fs::symlink_metadata(&path) {
        Err(_) => Ok(false),
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(&path).map(|()| true).map_err(Into::into),
        Ok(_) => std::fs::remove_file(&path).map(|()| true).map_err(Into::into),
    }
}

/// Moves a file or folder to another name in the plugin's folder.
pub fn rename(plugin: &str, from: &str, to: &str) -> Result<()> {
    let (from_path, to_path) = (resolve(plugin, from)?, resolve(plugin, to)?);
    let root = root(plugin)?;
    if from_path == root || to_path == root {
        bail!("the plugin's folder itself is not moved");
    }
    parent_made(&to_path)?;
    std::fs::rename(&from_path, &to_path).with_context(|| format!("could not move {from} to {to}"))
}

/// Copies a file to another name in the plugin's folder (folders on the way are made).
pub fn copy(plugin: &str, from: &str, to: &str) -> Result<u64> {
    let (from_path, to_path) = (resolve(plugin, from)?, resolve(plugin, to)?);
    if !from_path.is_file() {
        bail!("{from} is not a file");
    }
    parent_made(&to_path)?;
    let mut source = std::fs::File::open(&from_path)?;
    let tmp = to_path.with_file_name(format!(
        ".{}.tmp-{}",
        to_path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default(),
        std::process::id()
    ));
    let _ = std::fs::remove_file(&tmp);
    let mut target = private_options().write(true).create_new(true).open(&tmp)?;
    let copied = std::io::copy(&mut source, &mut target)?;
    drop(target);
    std::fs::rename(&tmp, &to_path)?;
    Ok(copied)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_plugin(test: impl FnOnce(&str)) {
        super::super::store::tests::with_data_dir(|_| test("files-test"));
    }

    #[test]
    fn a_plugin_keeps_files_in_its_own_folder_only() {
        with_plugin(|id| {
            assert_eq!(write(id, "x/2026-09-24/a/post.json", b"{}", false).unwrap(), 2);
            assert_eq!(write(id, "x/2026-09-24/a/post.json", b"[1]", true).unwrap(), 5);
            assert_eq!(read(id, "x/2026-09-24/a/post.json", 0, 100).unwrap(), (b"{}[1]".to_vec(), 5));
            assert_eq!(read(id, "x/2026-09-24/a/post.json", 2, 2).unwrap().0, b"[1".to_vec());
            let names: Vec<String> = list(id, "x/2026-09-24/a").unwrap().into_iter().map(|e| e.name).collect();
            assert_eq!(names, ["post.json"]);
            assert!(stat(id, "x").unwrap().unwrap().dir);
            assert!(stat(id, "nothing").unwrap().is_none());
            assert_eq!(copy(id, "x/2026-09-24/a/post.json", "z/copy.json").unwrap(), 5);
            assert_eq!(read(id, "z/copy.json", 0, 10).unwrap().0, b"{}[1]".to_vec());
            assert!(copy(id, "../x", "a").is_err() && copy(id, "z/copy.json", "../a").is_err());
            rename(id, "x/2026-09-24/a/post.json", "y/post.json").unwrap();
            assert!(stat(id, "y/post.json").unwrap().is_some());
            assert!(remove(id, "x").unwrap());
            assert!(!remove(id, "x").unwrap());
            assert!(remove(id, "").is_err(), "the folder itself stays");

            for outside in ["../other/secret", "/etc/hosts", "a/../../x", "..", "a\\b", "a\0b"] {
                assert!(resolve(id, outside).is_err(), "{outside:?} was let through");
                assert!(write(id, outside, b"x", false).is_err(), "{outside:?} was written");
            }
            assert!(write(id, &"a/".repeat(20), b"x", false).is_err(), "too deep");
            assert!(write(id, "big", &vec![0; MAX_WRITE + 1], false).is_err());
        });
    }

    #[cfg(unix)]
    #[test]
    fn a_link_in_the_folder_is_not_followed_and_files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        with_plugin(|id| {
            let root = root(id).unwrap();
            std::os::unix::fs::symlink(std::env::temp_dir(), root.join("out")).unwrap();
            assert!(write(id, "out/stolen", b"x", false).is_err());
            assert!(read(id, "out", 0, 1).is_err());
            assert!(list(id, "").unwrap().iter().all(|e| e.name != "out"), "links are not listed");
            write(id, "mine.txt", b"x", false).unwrap();
            let mode = std::fs::metadata(root.join("mine.txt")).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
            let folder = std::fs::metadata(&root).unwrap().permissions().mode() & 0o777;
            assert_eq!(folder, 0o700);
        });
    }
}
