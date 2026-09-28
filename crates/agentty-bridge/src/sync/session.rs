//! A session's transcript in the sync repository: appended in chunks, rebuilt on another computer.
//!
//! A transcript only grows. Each sync writes what was added since the last one as a new chunk,
//! cut after the last complete line (a line being written right now waits for the next sync), so a
//! chunk never changes once pushed and the repository's history grows only by what is new.

use super::mask::mask;
use super::model::{now_stamp, Chunk, SessionManifest, Sidecar, FORMAT_VERSION, MANIFEST_FILE};
use anyhow::{ensure, Context, Result};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Largest chunk written at once. GitHub refuses files over 100 MB and warns over 50 MB.
const MAX_CHUNK_BYTES: u64 = 8 * 1024 * 1024;
/// Side files larger than this are left out (large tool outputs; the transcript has the gist).
const MAX_SIDECAR_BYTES: u64 = 16 * 1024 * 1024;
/// Bytes hashed before the end of what was uploaded, to notice a rewritten transcript.
const TAIL_BYTES: u64 = 4096;
const SIDECAR_DIR: &str = "sidecar";

/// What the caller knows about the session besides its files.
pub struct SessionFacts<'a> {
    pub agent: crate::model::Agent,
    pub id: &'a str,
    pub title: &'a str,
    pub device: &'a str,
    pub cwd: &'a str,
    pub project: super::model::ProjectRef,
    /// The transcript relative to the agent's session folder.
    pub source: String,
    pub parent: Option<super::model::ParentRef>,
}

/// Writes what is new in `transcript` (and its side folder) under `dir`, the session's folder in
/// the clone. Returns the manifest, and whether anything was written.
pub fn upload(dir: &Path, transcript: &Path, side: Option<&Path>, facts: &SessionFacts) -> Result<(SessionManifest, bool)> {
    let previous = read_manifest(dir).ok().filter(|m| m.agent == facts.agent && m.id == facts.id);
    let len = fs::metadata(transcript).with_context(|| format!("{} is gone", transcript.display()))?.len();
    let mut file = fs::File::open(transcript)?;

    // Keep the chunks already pushed only if the file still starts with what they hold.
    let mut chunks = previous.as_ref().map(|m| m.chunks.clone()).unwrap_or_default();
    let mut uploaded = chunks.last().map_or(0, |c| c.end);
    let intact =
        previous.as_ref().is_some_and(|m| uploaded <= len && tail_hash(&mut file, uploaded).ok().as_deref() == Some(m.tail_hash.as_str()));
    let mut changed = false;
    if !intact && uploaded > 0 {
        for chunk in &chunks {
            let _ = fs::remove_file(dir.join(&chunk.file));
        }
        chunks.clear();
        uploaded = 0;
        changed = true;
    }

    // New complete lines, in pieces no larger than MAX_CHUNK_BYTES.
    let mut next = chunks.len() + 1;
    let mut at = uploaded;
    while at < len {
        let want = (len - at).min(MAX_CHUNK_BYTES);
        let mut buffer = vec![0u8; want as usize];
        file.seek(SeekFrom::Start(at))?;
        file.read_exact(&mut buffer)?;
        let Some(last_newline) = buffer.iter().rposition(|&b| b == b'\n') else {
            if want == MAX_CHUNK_BYTES {
                // One line longer than a whole chunk: take it as it is rather than stall forever.
                write_chunk(dir, next, &buffer)?;
                chunks.push(Chunk { file: chunk_name(next), start: at, end: at + want });
                at += want;
                next += 1;
                changed = true;
                continue;
            }
            break;
        };
        buffer.truncate(last_newline + 1);
        let end = at + buffer.len() as u64;
        write_chunk(dir, next, &buffer)?;
        chunks.push(Chunk { file: chunk_name(next), start: at, end });
        at = end;
        next += 1;
        changed = true;
    }

    let mut sidecars = previous.as_ref().map(|m| m.sidecars.clone()).unwrap_or_default();
    if let Some(side) = side.filter(|s| s.is_dir()) {
        changed |= upload_sidecars(dir, side, &mut sidecars)?;
    }

    let meta_changed = previous
        .as_ref()
        .is_none_or(|m| m.title != facts.title || m.project != facts.project || m.parent != facts.parent || m.source != facts.source);
    let manifest = SessionManifest {
        version: FORMAT_VERSION,
        agent: facts.agent,
        id: facts.id.to_string(),
        title: facts.title.to_string(),
        device: facts.device.to_string(),
        cwd: facts.cwd.to_string(),
        project: facts.project.clone(),
        source: facts.source.clone(),
        tail_hash: tail_hash(&mut file, at)?,
        chunks,
        sidecars,
        parent: facts.parent.clone(),
        updated_at: if changed || meta_changed {
            now_stamp()
        } else {
            previous.as_ref().map(|m| m.updated_at.clone()).unwrap_or_else(now_stamp)
        },
    };
    if changed || meta_changed {
        write_json(&dir.join(MANIFEST_FILE), &manifest)?;
    }
    Ok((manifest, changed || meta_changed))
}

/// Writes the transcript of the session stored in `dir` to `transcript`, and its side files into
/// `side`. `rewrite` changes each line on the way (a different working directory on this computer).
pub fn rebuild(dir: &Path, transcript: &Path, side: Option<&Path>, rewrite: &dyn Fn(&str) -> String) -> Result<SessionManifest> {
    let manifest = read_manifest(dir)?;
    let out = transcript_bytes(dir, &manifest, rewrite)?;
    if let Some(parent) = transcript.parent() {
        fs::create_dir_all(parent)?;
    }
    crate::fsutil::write_private(transcript, &out)?;
    if let Some(side) = side {
        write_sidecars(dir, &manifest, side)?;
    }
    Ok(manifest)
}

/// The transcript stored in `dir`, its chunks joined, each line passed through `rewrite`.
pub fn transcript_bytes(dir: &Path, manifest: &SessionManifest, rewrite: &dyn Fn(&str) -> String) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for chunk in &manifest.chunks {
        ensure!(safe_name(&chunk.file), "bad chunk name in {}", dir.display());
        let bytes = fs::read(dir.join(&chunk.file)).with_context(|| format!("chunk {} is missing", chunk.file))?;
        for line in String::from_utf8_lossy(&bytes).split_inclusive('\n') {
            out.extend_from_slice(rewrite(line).as_bytes());
        }
    }
    Ok(out)
}

/// Copies the session's side files into `side`.
pub fn write_sidecars(dir: &Path, manifest: &SessionManifest, side: &Path) -> Result<()> {
    for sidecar in &manifest.sidecars {
        let Some(relative) = safe_relative(&sidecar.path) else { continue };
        let from = dir.join(SIDECAR_DIR).join(&relative);
        let Ok(bytes) = fs::read(&from) else { continue };
        crate::fsutil::write_private(&side.join(&relative), &bytes)?;
    }
    Ok(())
}

pub fn read_manifest(dir: &Path) -> Result<SessionManifest> {
    let text = fs::read_to_string(dir.join(MANIFEST_FILE))?;
    Ok(serde_json::from_str(&text)?)
}

fn upload_sidecars(dir: &Path, side: &Path, known: &mut Vec<Sidecar>) -> Result<bool> {
    let mut files = Vec::new();
    collect_files(side, side, 3, &mut files);
    let mut changed = false;
    for (path, relative) in files {
        let Ok(meta) = fs::metadata(&path) else { continue };
        if meta.len() > MAX_SIDECAR_BYTES {
            continue;
        }
        let modified_ms = crate::fsutil::mtime_ms(&path);
        if known.iter().any(|s| s.path == relative && s.size == meta.len() && s.modified_ms == modified_ms) {
            continue;
        }
        let Ok(bytes) = fs::read(&path) else { continue };
        // Text is masked like the transcript; anything else is copied as it is.
        let bytes = match String::from_utf8(bytes) {
            Ok(text) => mask(&text).into_bytes(),
            Err(err) => err.into_bytes(),
        };
        let target = dir.join(SIDECAR_DIR).join(&relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&target, bytes)?;
        known.retain(|s| s.path != relative);
        known.push(Sidecar { path: relative, size: meta.len(), modified_ms });
        changed = true;
    }
    known.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(changed)
}

fn collect_files(root: &Path, dir: &Path, depth: usize, out: &mut Vec<(PathBuf, String)>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else { continue };
        // Links are never followed: a side folder is the agent's own files, nothing it points to.
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            if depth > 0 {
                collect_files(root, &path, depth - 1, out);
            }
        } else if kind.is_file() {
            let Ok(relative) = path.strip_prefix(root) else { continue };
            let relative: Vec<String> = relative.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect();
            out.push((path, relative.join("/")));
        }
    }
}

/// A `/`-separated relative path from a manifest, refused if it could leave its folder.
pub fn safe_relative(path: &str) -> Option<PathBuf> {
    let parts: Vec<&str> = path.split('/').collect();
    parts.iter().all(|p| safe_name(p)).then(|| parts.iter().collect())
}

fn safe_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', ':', '\0'])
}

fn chunk_name(n: usize) -> String {
    format!("{n:04}.jsonl")
}

fn write_chunk(dir: &Path, n: usize, bytes: &[u8]) -> Result<()> {
    fs::create_dir_all(dir)?;
    let text = mask(&String::from_utf8_lossy(bytes));
    fs::write(dir.join(chunk_name(n)), text)?;
    Ok(())
}

fn tail_hash(file: &mut fs::File, end: u64) -> Result<String> {
    let start = end.saturating_sub(TAIL_BYTES);
    let mut buffer = vec![0u8; (end - start) as usize];
    file.seek(SeekFrom::Start(start))?;
    file.read_exact(&mut buffer)?;
    let digest = Sha256::digest([end.to_le_bytes().as_slice(), &buffer].concat());
    Ok(digest.iter().map(|b| format!("{b:02x}")).collect())
}

pub fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    fs::write(path, text)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Agent;
    use std::io::Write;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("agentty-sync-session-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn facts<'a>() -> SessionFacts<'a> {
        SessionFacts {
            agent: Agent::Claude,
            id: "s1",
            title: "Fix login",
            device: "dev-a",
            cwd: "/work/app",
            project: Default::default(),
            source: "-work-app/s1.jsonl".into(),
            parent: None,
        }
    }

    fn append(path: &Path, text: &str) {
        fs::OpenOptions::new().create(true).append(true).open(path).unwrap().write_all(text.as_bytes()).unwrap();
    }

    #[test]
    fn only_new_complete_lines_are_uploaded() {
        let root = temp("append");
        let transcript = root.join("s1.jsonl");
        let dir = root.join("repo/s1");
        append(&transcript, "{\"n\":1}\n{\"n\":2}\n{\"n\":");
        let (manifest, changed) = upload(&dir, &transcript, None, &facts()).unwrap();
        assert!(changed);
        assert_eq!(manifest.chunks.len(), 1);
        assert_eq!(fs::read_to_string(dir.join("0001.jsonl")).unwrap(), "{\"n\":1}\n{\"n\":2}\n");

        // Nothing new: nothing written.
        let (_, changed) = upload(&dir, &transcript, None, &facts()).unwrap();
        assert!(!changed);

        // The half line is finished and more follows: one new chunk.
        append(&transcript, "3}\n{\"n\":4}\n");
        let (manifest, changed) = upload(&dir, &transcript, None, &facts()).unwrap();
        assert!(changed);
        assert_eq!(manifest.chunks.len(), 2);
        assert_eq!(fs::read_to_string(dir.join("0002.jsonl")).unwrap(), "{\"n\":3}\n{\"n\":4}\n");

        // Rebuilt elsewhere, the transcript is the same.
        let copy = root.join("elsewhere/s1.jsonl");
        rebuild(&dir, &copy, None, &|line| line.to_string()).unwrap();
        assert_eq!(fs::read_to_string(&copy).unwrap(), "{\"n\":1}\n{\"n\":2}\n{\"n\":3}\n{\"n\":4}\n");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_rewritten_transcript_is_uploaded_again() {
        let root = temp("rewrite");
        let transcript = root.join("s1.jsonl");
        let dir = root.join("repo/s1");
        append(&transcript, "{\"a\":1}\n{\"a\":2}\n");
        upload(&dir, &transcript, None, &facts()).unwrap();
        fs::write(&transcript, "{\"b\":1}\n").unwrap();
        let (manifest, changed) = upload(&dir, &transcript, None, &facts()).unwrap();
        assert!(changed);
        assert_eq!(manifest.chunks.len(), 1);
        assert_eq!(fs::read_to_string(dir.join("0001.jsonl")).unwrap(), "{\"b\":1}\n");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn side_files_follow_and_come_back() {
        let root = temp("side");
        let transcript = root.join("s1.jsonl");
        let side = root.join("s1");
        fs::create_dir_all(side.join("subagents")).unwrap();
        fs::write(side.join("subagents/agent-x.jsonl"), "{\"sub\":1}\n").unwrap();
        append(&transcript, "{\"n\":1}\n");
        let dir = root.join("repo/s1");
        let (manifest, _) = upload(&dir, &transcript, Some(&side), &facts()).unwrap();
        assert_eq!(manifest.sidecars.len(), 1);
        assert_eq!(manifest.sidecars[0].path, "subagents/agent-x.jsonl");

        let copy_side = root.join("elsewhere/s1");
        rebuild(&dir, &root.join("elsewhere/s1.jsonl"), Some(&copy_side), &|l| l.to_string()).unwrap();
        assert_eq!(fs::read_to_string(copy_side.join("subagents/agent-x.jsonl")).unwrap(), "{\"sub\":1}\n");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn manifest_paths_cannot_leave_their_folder() {
        assert!(safe_relative("subagents/a.jsonl").is_some());
        assert!(safe_relative("../x").is_none());
        assert!(safe_relative("a/../../x").is_none());
        assert!(safe_relative("").is_none());
        assert!(!safe_name("..\\x"));
    }
}
