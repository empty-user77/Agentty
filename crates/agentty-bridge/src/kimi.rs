//! Reader for Kimi CLI sessions: `~/.kimi/sessions/<md5 of work dir>/<session id>/context.jsonl`
//! (older ones: `<session id>.jsonl`), with work dirs listed in `~/.kimi/kimi.json`.
//! `KIMI_SHARE_DIR` moves the whole folder.

use crate::fsutil::{self, one_line, truncate_chars};
use crate::model::{Agent, Role, SessionInfo, Turn};
use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Where this agent keeps its sessions, for the guard on deleting one. `KIMI_SHARE_DIR` moves
/// it, so this is read rather than assumed.
pub fn session_root() -> PathBuf {
    share_dir()
}

fn share_dir() -> PathBuf {
    std::env::var_os("KIMI_SHARE_DIR").map(PathBuf::from).unwrap_or_else(|| fsutil::home().join(".kimi"))
}

/// Session folder name (md5 of the path) → work dir.
fn work_dirs() -> HashMap<String, String> {
    let Ok(bytes) = std::fs::read(share_dir().join("kimi.json")) else { return HashMap::new() };
    let Ok(json) = serde_json::from_slice::<Value>(&bytes) else { return HashMap::new() };
    json["work_dirs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|w| {
            let path = w["path"].as_str()?;
            let hash = md5_hex(path.as_bytes());
            let name = match w["kaos"].as_str() {
                Some(kaos) if kaos != "local" => format!("{kaos}_{hash}"),
                _ => hash,
            };
            Some((name, path.to_string()))
        })
        .collect()
}

/// The session folder Kimi CLI keeps for a work dir (the md5 of its path).
pub fn work_dir_folder(cwd: &Path) -> String {
    md5_hex(cwd.to_string_lossy().as_bytes())
}

/// Whether Kimi CLI knows `cwd` as a work dir (listed in `kimi.json`).
pub fn knows_work_dir(cwd: &Path) -> bool {
    work_dirs().values().any(|p| Path::new(p) == cwd)
}

/// Id of the newest session in `cwd` written after `since_ms` (Kimi runs as a command in a pane,
/// so its session is found from its files).
pub fn find_recent(cwd: &Path, since_ms: u64) -> Option<String> {
    find_recent_in(&share_dir().join("sessions").join(work_dir_folder(cwd)), since_ms)
}

fn find_recent_in(folder: &Path, since_ms: u64) -> Option<String> {
    let mut found: Vec<(u64, String)> = std::fs::read_dir(folder)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            let (id, file) = match name.strip_suffix(".jsonl") {
                Some(id) => (id.to_string(), entry.path()),
                None => (name, entry.path().join("context.jsonl")),
            };
            let at = fsutil::mtime_ms(&file);
            (file.is_file() && at >= since_ms).then_some((at, id))
        })
        .collect();
    found.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    found.into_iter().next().map(|(_, id)| id)
}

/// (session id, context file) for every session.
fn sessions() -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let Ok(dirs) = std::fs::read_dir(share_dir().join("sessions")) else { return out };
    for dir in dirs.flatten() {
        let Ok(entries) = std::fs::read_dir(dir.path()) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if path.join("context.jsonl").is_file() {
                out.push((name, path.join("context.jsonl")));
            } else if let Some(id) = name.strip_suffix(".jsonl") {
                out.push((id.to_string(), path));
            }
        }
    }
    out
}

static SUMMARIES: std::sync::LazyLock<fsutil::MtimeCache<Option<SessionInfo>>> = std::sync::LazyLock::new(Default::default);

pub fn list(limit: usize) -> Vec<SessionInfo> {
    let dirs = work_dirs();
    let ids: HashMap<PathBuf, String> = sessions().into_iter().map(|(id, path)| (path, id)).collect();
    fsutil::newest(ids.keys().cloned().collect(), limit.saturating_mul(2))
        .into_iter()
        .filter_map(|(path, updated_at)| {
            let id = ids.get(&path)?.clone();
            fsutil::cached_by_mtime(&SUMMARIES, &path, updated_at, || summarize(&id, &path, updated_at, &dirs).ok().flatten())
        })
        .take(limit)
        .collect()
}

fn text_of(content: &Value) -> Option<String> {
    let text = match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => {
            parts.iter().filter(|p| p["type"] == "text").filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n")
        }
        _ => return None,
    };
    (!text.trim().is_empty()).then_some(text)
}

fn messages(path: &Path) -> Result<Vec<Value>> {
    Ok(std::fs::read_to_string(path)?.lines().filter_map(|l| serde_json::from_str(l).ok()).collect())
}

fn summarize(id: &str, path: &Path, updated_at: u64, dirs: &HashMap<String, String>) -> Result<Option<SessionInfo>> {
    let messages = messages(path)?;
    let user = |m: &&Value| m["role"] == "user";
    let Some(first) = messages.iter().filter(user).find_map(|m| text_of(&m["content"])) else { return Ok(None) };
    // Hash folder: the parent of `<id>/context.jsonl`, or of `<id>.jsonl`.
    let hash_dir =
        if path.file_name().is_some_and(|n| n == "context.jsonl") { path.parent().and_then(Path::parent) } else { path.parent() };
    let cwd = hash_dir.and_then(|d| d.file_name()).and_then(|n| dirs.get(&*n.to_string_lossy()).cloned());
    let state_title = path
        .parent()
        .and_then(|d| std::fs::read(d.join("state.json")).ok())
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|s| s["custom_title"].as_str().filter(|t| !t.is_empty()).map(str::to_string));
    Ok(Some(SessionInfo {
        agent: Agent::Kimi,
        resume_args: Agent::Kimi.resume_args(id),
        title: state_title.unwrap_or_else(|| one_line(&first, 80)),
        cwd,
        updated_at,
        last_prompt: messages.iter().rev().filter(user).find_map(|m| text_of(&m["content"])).map(|t| one_line(&t, 160)),
        last_reply: messages
            .iter()
            .rev()
            .filter(|m| m["role"] == "assistant")
            .find_map(|m| text_of(&m["content"]))
            .map(|t| one_line(&t, 200)),
        path: path.to_path_buf(),
        id: id.to_string(),
    }))
}

pub fn find(id: &str) -> Result<PathBuf> {
    sessions().into_iter().find(|(sid, _)| sid == id).map(|(_, path)| path).with_context(|| format!("Kimi session {id} not found"))
}

pub fn transcript(path: &Path) -> Result<(Option<String>, Vec<Turn>)> {
    let turns = messages(path)?
        .iter()
        .filter_map(|m| {
            let role = match m["role"].as_str()? {
                "user" => Role::User,
                "assistant" => Role::Assistant,
                _ => return None,
            };
            Some(Turn { role, text: truncate_chars(&text_of(&m["content"])?, 8_000) })
        })
        .collect();
    Ok((None, turns))
}

/// MD5 (RFC 1321), only to find Kimi's per-folder session directories.
fn md5_hex(input: &[u8]) -> String {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23,
        4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    let k: Vec<u32> = (0..64).map(|i| ((i as f64 + 1.0).sin().abs() * 4_294_967_296.0) as u32).collect();
    let (mut a0, mut b0, mut c0, mut d0) = (0x67452301u32, 0xefcdab89u32, 0x98badcfeu32, 0x10325476u32);
    let mut message = input.to_vec();
    let bit_len = (input.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_le_bytes());
    for chunk in message.chunks(64) {
        let m: Vec<u32> = chunk.chunks(4).map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]])).collect();
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((b & c) | (!b & d), i),
                16..=31 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let f = f.wrapping_add(a).wrapping_add(k[i]).wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(S[i]));
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    [a0, b0, c0, d0].iter().flat_map(|v| v.to_le_bytes()).map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md5_matches_known_digests() {
        assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5_hex(b"The quick brown fox jumps over the lazy dog"), "9e107d9d372bb6826bd81d3542a419d6");
    }

    #[test]
    fn reads_string_and_part_contents() {
        assert_eq!(text_of(&serde_json::json!("hi")).as_deref(), Some("hi"));
        assert_eq!(text_of(&serde_json::json!([{ "type": "text", "text": "yo" }])).as_deref(), Some("yo"));
    }
}

#[cfg(test)]
mod find_recent_tests {
    use super::*;

    #[test]
    fn the_newest_session_of_the_work_dir_is_the_panes() {
        let folder = std::env::temp_dir().join(format!("agentty-kimi-recent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(folder.join("s-new")).unwrap();
        std::fs::write(folder.join("s-new").join("context.jsonl"), "{}\n").unwrap();
        assert_eq!(find_recent_in(&folder, 0).as_deref(), Some("s-new"));
        assert_eq!(find_recent_in(&folder, u64::MAX), None);
        assert_eq!(work_dir_folder(Path::new("/work/app")).len(), 32);
        let _ = std::fs::remove_dir_all(&folder);
    }
}
