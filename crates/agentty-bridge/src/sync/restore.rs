//! Brings a synced session onto this computer so its agent can resume it.
//!
//! The transcript is rebuilt from its chunks under the agent's own session folder, with the working
//! directory changed to where the project is on this computer. A session this computer already has
//! is only replaced when the synced one continues it; when both went on separately, the synced one
//! comes back as a new session (a branch) and the local one is left alone.

use super::model::ParentRef;
use super::{clone_dir, load_config, session, update_config, workspace_dir, Locate, Lock, SyncConfig, SyncError};
use crate::model::Agent;
use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// Which synced session to bring over.
#[derive(Debug, Clone)]
pub struct RestoreRequest {
    pub plugin: Option<String>,
    pub sync_id: String,
    /// The device whose copy is taken.
    pub device: String,
    pub agent: Agent,
    pub session: String,
    /// The folder of the workspace it goes into: where to look for the project on this computer.
    pub workspace_cwd: PathBuf,
    /// A pane here runs this session: its transcript must not change under it, so anything new
    /// comes back as a branch.
    pub keep_local: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreOutcome {
    /// Not on this computer before.
    Downloaded,
    /// This computer already had all of it.
    UpToDate,
    /// The synced copy continued the local one, which now has the rest.
    Updated,
    /// Both went on separately: the synced one is a new session next to the local one.
    Branched,
}

#[derive(Debug, Clone)]
pub struct Restored {
    pub agent: Agent,
    /// The session to resume (a new id when it branched).
    pub id: String,
    pub title: String,
    /// Where it resumes.
    pub cwd: PathBuf,
    pub outcome: RestoreOutcome,
}

/// Restores a session from the clone. Blocking; waits for a running sync to finish first.
pub fn restore(request: &RestoreRequest) -> Result<Restored, SyncError> {
    let _lock = Lock::wait(std::time::Duration::from_secs(60))?;
    let config = load_config();
    let restored = restore_in(&clone_dir(), &config, request, &Locate::agents())?;
    // Continuing another computer's session: the next sync records where it came from.
    if let Some(origin) = origin_of(&config, request, &restored) {
        let key = origin_key(restored.agent, &restored.id);
        let _ = update_config(|c| {
            c.origins.insert(key, origin);
        });
    }
    Ok(restored)
}

pub(super) fn origin_key(agent: Agent, id: &str) -> String {
    format!("{}:{id}", agent.id())
}

fn origin_of(config: &SyncConfig, request: &RestoreRequest, restored: &Restored) -> Option<ParentRef> {
    let from_elsewhere = request.device != config.device_id || restored.id != request.session;
    (from_elsewhere && restored.outcome != RestoreOutcome::UpToDate).then(|| ParentRef {
        device: request.device.clone(),
        session: request.session.clone(),
        message: None,
    })
}

pub(super) fn restore_in(root: &Path, config: &SyncConfig, request: &RestoreRequest, locate: &Locate) -> Result<Restored> {
    let _ = config;
    let workspace = workspace_dir(root, request.plugin.as_deref(), &request.sync_id).context("bad workspace id")?;
    let dir = workspace.join("devices").join(&request.device).join(request.agent.id()).join(&request.session);
    anyhow::ensure!(super::safe_segment(&request.device) && super::safe_segment(&request.session), "bad session id");
    let manifest = session::read_manifest(&dir).context("this session is not in the sync repository (sync first)")?;
    let cwd = local_cwd(&manifest, &request.workspace_cwd);
    let (old, new) = (manifest.cwd.clone(), cwd.to_string_lossy().to_string());
    let synced = session::transcript_bytes(&dir, &manifest, &|line| rewrite_cwd(line, &old, &new))?;
    let root_of_agent = (locate.root)(request.agent);
    let restored =
        |id: &str, outcome| Restored { agent: request.agent, id: id.to_string(), title: manifest.title.clone(), cwd: cwd.clone(), outcome };

    match (locate.transcript)(request.agent, &request.session) {
        Ok(local_path) => {
            let local = fs::read(&local_path)?;
            if local.starts_with(&synced) {
                return Ok(restored(&request.session, RestoreOutcome::UpToDate));
            }
            if synced.starts_with(&local) && !request.keep_local {
                crate::fsutil::write_private(&local_path, &synced)?;
                if request.agent == Agent::Claude {
                    session::write_sidecars(&dir, &manifest, &local_path.with_extension(""))?;
                }
                return Ok(restored(&request.session, RestoreOutcome::Updated));
            }
            // Both went on: the synced one becomes a session of its own.
            let id = new_session_id();
            match request.agent {
                Agent::Claude => {
                    let branched = replace_session_id(&synced, &request.session, &id);
                    let path = root_of_agent.join(claude_folder(&cwd)).join(format!("{id}.jsonl"));
                    write_new(&path, &branched)?;
                    session::write_sidecars(&dir, &manifest, &path.with_extension(""))?;
                }
                Agent::Codex => {
                    // Next to the local rollout, named and headed with the new id.
                    let name =
                        local_path.file_name().map(|n| n.to_string_lossy().replace(&request.session, &id)).context("bad rollout name")?;
                    anyhow::ensure!(name.contains(&id), "unexpected Codex rollout name");
                    write_new(&local_path.with_file_name(name), &replace_codex_id(&synced, &request.session, &id))?;
                }
                other => bail!("{} sessions that went on separately on two computers cannot be branched", other.display_name()),
            }
            Ok(restored(&id, RestoreOutcome::Branched))
        }
        Err(_) => {
            let path = match request.agent {
                Agent::Claude => root_of_agent.join(claude_folder(&cwd)).join(format!("{}.jsonl", request.session)),
                Agent::Codex => root_of_agent.join(session::safe_relative(&manifest.source).context("bad transcript path")?),
                // Their folders are named after the project folder by the agent itself: the session
                // goes back only into the same folder, and only one the agent already knows.
                Agent::Gemini | Agent::Kimi => {
                    let relative = session::safe_relative(&manifest.source).context("bad transcript path")?;
                    anyhow::ensure!(
                        Path::new(&manifest.cwd) == cwd,
                        "{} sessions come back only in the same folder ({}); it is not on this computer",
                        request.agent.display_name(),
                        manifest.cwd
                    );
                    anyhow::ensure!(
                        (locate.knows_folder)(request.agent, &relative, &cwd),
                        "open {} once in {} on this computer first, so it knows the folder",
                        request.agent.display_name(),
                        cwd.display()
                    );
                    root_of_agent.join(relative)
                }
                other => {
                    bail!("{} sessions are not synced as files; resume them in {} directly", other.display_name(), other.display_name())
                }
            };
            write_new(&path, &synced)?;
            if request.agent == Agent::Claude {
                session::write_sidecars(&dir, &manifest, &path.with_extension(""))?;
            }
            Ok(restored(&request.session, RestoreOutcome::Downloaded))
        }
    }
}

/// Which synced session to share, and with which agent.
#[derive(Debug, Clone)]
pub struct ContextRequest {
    pub plugin: Option<String>,
    pub sync_id: String,
    pub device: String,
    pub agent: Agent,
    pub session: String,
    /// The agent that reads it.
    pub to: Agent,
}

/// Context merge: the synced session's conversation as a handoff document, and the prompt that
/// asks an agent to read it. Nothing is merged into any transcript: the prompt carries it over.
pub fn share_context(request: &ContextRequest) -> Result<String, SyncError> {
    let _lock = Lock::wait(std::time::Duration::from_secs(60))?;
    let (manifest, turns) = context_turns(&clone_dir(), &super::sync_dir().join("context"), request)?;
    let handoff =
        crate::handoff::create_share(request.agent, &request.session, &manifest.title, request.to, Some(manifest.cwd.clone()), &turns)?;
    Ok(handoff.prompt)
}

/// The synced session's conversation, read through its agent's own reader (`scratch` holds the
/// rebuilt transcript for the moment it is read).
pub(super) fn context_turns(
    root: &Path,
    scratch: &Path,
    request: &ContextRequest,
) -> Result<(super::model::SessionManifest, Vec<crate::model::Turn>)> {
    anyhow::ensure!(super::safe_segment(&request.device) && super::safe_segment(&request.session), "bad session id");
    let workspace = workspace_dir(root, request.plugin.as_deref(), &request.sync_id).context("bad workspace id")?;
    let dir = workspace.join("devices").join(&request.device).join(request.agent.id()).join(&request.session);
    let manifest = session::read_manifest(&dir).context("this session is not in the sync repository (sync first)")?;
    let bytes = session::transcript_bytes(&dir, &manifest, &|line| line.to_string())?;
    // The agents' readers take a file: a private one for the moment it is read.
    let scratch = scratch.join(format!("{}-{}.jsonl", request.agent.id(), request.session));
    write_new(&scratch, &bytes)?;
    let turns = match request.agent {
        Agent::Claude => crate::claude::transcript(&scratch),
        Agent::Codex => crate::codex::transcript(&scratch),
        Agent::Gemini => crate::gemini::transcript(&scratch),
        Agent::Kimi => crate::kimi::transcript(&scratch),
        other => Err(anyhow::anyhow!("{} sessions cannot be shared yet", other.display_name())),
    };
    let _ = fs::remove_file(&scratch);
    let (_, turns) = turns?;
    anyhow::ensure!(!turns.is_empty(), "the session has no conversation yet");
    Ok((manifest, turns))
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    crate::fsutil::write_private(path, bytes)?;
    Ok(())
}

/// Where the session works on this computer: the same folder of the same project when the
/// workspace's folder is a clone of it, the original folder when it exists here, else the
/// workspace's folder.
fn local_cwd(manifest: &super::model::SessionManifest, workspace_cwd: &Path) -> PathBuf {
    if let Some(remote) = &manifest.project.remote_url {
        if let Some(root) = crate::git::repo_root(workspace_cwd) {
            if crate::git::remote_web_url(&root).as_ref() == Some(remote) {
                let inside = session::safe_relative(&manifest.project.path_in_repo).map(|p| root.join(p)).unwrap_or_else(|| root.clone());
                return if inside.is_dir() { inside } else { root };
            }
        }
    }
    let original = PathBuf::from(&manifest.cwd);
    if original.is_absolute() && original.is_dir() {
        return original;
    }
    workspace_cwd.to_path_buf()
}

/// The folder on this computer for a workspace synced elsewhere: its folder when it exists here,
/// else one of `candidates` (folders this computer works in) that is a clone of the same project.
pub fn project_folder(project: &super::model::ProjectRef, original: &str, candidates: &[PathBuf]) -> Option<PathBuf> {
    let original = PathBuf::from(original);
    if original.is_absolute() && original.is_dir() {
        return Some(original);
    }
    let remote = project.remote_url.as_ref()?;
    let mut roots: Vec<PathBuf> = candidates.iter().filter_map(|c| crate::git::repo_root(c)).collect();
    roots.sort();
    roots.dedup();
    let root = roots.into_iter().find(|root| crate::git::remote_web_url(root).as_ref() == Some(remote))?;
    let inside = session::safe_relative(&project.path_in_repo).map(|p| root.join(p)).unwrap_or_else(|| root.clone());
    Some(if inside.is_dir() { inside } else { root })
}

/// Claude Code's folder name for a working directory (see `claude::project_dir_for`).
fn claude_folder(cwd: &Path) -> String {
    cwd.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

/// `"cwd":"<old>…"` becomes `"cwd":"<new>…"`, byte for byte elsewhere.
pub(super) fn rewrite_cwd(line: &str, old: &str, new: &str) -> String {
    if old.is_empty() || old == new || !line.contains("\"cwd\"") {
        return line.to_string();
    }
    let quoted = |s: &str| serde_json::to_string(s).unwrap_or_default();
    let (old_q, new_q) = (quoted(old), quoted(new));
    // Without the closing quote: also a folder below it.
    let (old_open, new_open) = (&old_q[..old_q.len() - 1], &new_q[..new_q.len() - 1]);
    let mut out = line.to_string();
    // Agents write compact JSON; a space after the colon is accepted too.
    for key in ["\"cwd\":", "\"cwd\": "] {
        out = out.replace(&format!("{key}{old_q}"), &format!("{key}{new_q}"));
        for separator in ["/", "\\\\"] {
            out = out.replace(&format!("{key}{old_open}{separator}"), &format!("{key}{new_open}{separator}"));
        }
    }
    out
}

/// A Codex rollout under a new id: the id in its `session_meta` line.
fn replace_codex_id(bytes: &[u8], old: &str, new: &str) -> Vec<u8> {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        if line.contains("\"session_meta\"") {
            let mut line = line.to_string();
            for key in ["\"id\":", "\"id\": "] {
                line = line.replace(&format!("{key}\"{old}\""), &format!("{key}\"{new}\""));
            }
            out.push_str(&line);
        } else {
            out.push_str(line);
        }
    }
    out.into_bytes()
}

fn replace_session_id(bytes: &[u8], old: &str, new: &str) -> Vec<u8> {
    let mut text = String::from_utf8_lossy(bytes).into_owned();
    // Agents write compact JSON; a space after the colon is accepted too.
    for key in ["\"sessionId\":", "\"sessionId\": "] {
        text = text.replace(&format!("{key}\"{old}\""), &format!("{key}\"{new}\""));
    }
    text.into_bytes()
}

/// A random UUID (version 4), the form agents give their sessions.
fn new_session_id() -> String {
    let hex = super::new_id();
    let mut chars: Vec<char> = hex.chars().collect();
    chars[12] = '4';
    chars[16] = ['8', '9', 'a', 'b'][chars[16].to_digit(16).unwrap_or(0) as usize % 4];
    let s: String = chars.into_iter().collect();
    format!("{}-{}-{}-{}-{}", &s[0..8], &s[8..12], &s[12..16], &s[16..20], &s[20..32])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn working_directories_move_to_this_computer() {
        let line = r#"{"cwd":"/Users/a/app","x":1}"#;
        assert_eq!(rewrite_cwd(line, "/Users/a/app", "/home/b/app"), r#"{"cwd":"/home/b/app","x":1}"#);
        let below = r#"{"cwd":"/Users/a/app/web"}"#;
        assert_eq!(rewrite_cwd(below, "/Users/a/app", "/home/b/app"), r#"{"cwd":"/home/b/app/web"}"#);
        // Another folder that only starts the same is not moved.
        let other = r#"{"cwd":"/Users/a/application"}"#;
        assert_eq!(rewrite_cwd(other, "/Users/a/app", "/home/b/app"), other);
        let windows = r#"{"cwd":"C:\\work\\app\\web"}"#;
        assert_eq!(rewrite_cwd(windows, r"C:\work\app", "/home/b/app"), r#"{"cwd":"/home/b/app\\web"}"#);
    }

    #[test]
    fn a_branch_gets_its_own_session_id_on_every_line() {
        let text = "{\"sessionId\":\"s1\",\"a\":1}\n{\"sessionId\": \"s1\"}\n{\"sessionId\":\"s10\"}\n";
        let out = String::from_utf8(replace_session_id(text.as_bytes(), "s1", "n")).unwrap();
        assert_eq!(out, "{\"sessionId\":\"n\",\"a\":1}\n{\"sessionId\": \"n\"}\n{\"sessionId\":\"s10\"}\n");
    }

    #[test]
    fn new_session_ids_look_like_the_agents_own() {
        let id = new_session_id();
        assert_eq!(id.len(), 36);
        assert_eq!(id.chars().nth(14), Some('4'));
        assert!(super::super::safe_segment(&id));
    }
}
