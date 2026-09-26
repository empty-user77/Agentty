//! What the sync repository holds, file by file. Everything is plain JSON / JSONL so the repository
//! reads on GitHub; each file says which device wrote it.
//!
//! ```text
//! .agentty-sync.json                               Marker
//! devices/<device-id>.json                         DeviceInfo — written by that device only
//! workspace/<sync-id>/sync_metadata.json           WorkspaceMetadata — one section per device
//! workspace/<sync-id>/devices/<device-id>/<agent>/<session-id>/
//!     session.json                                 SessionManifest
//!     0001.jsonl, 0002.jsonl, …                    the transcript, in append-only chunks
//!     sidecar/…                                    subagent transcripts and tool results
//! plugin/<plugin-id>/<sync-id>/…                   the same, for a plugin's workspace
//! ```

use crate::model::Agent;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const FORMAT_VERSION: u32 = 1;
pub const MARKER_FILE: &str = ".agentty-sync.json";
pub const METADATA_FILE: &str = "sync_metadata.json";
pub const MANIFEST_FILE: &str = "session.json";

/// Marks a repository as an Agentty sync repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Marker {
    pub kind: String,
    pub version: u32,
    pub created_at: String,
    pub created_by: String,
}

impl Marker {
    pub const KIND: &'static str = "agentty-sync";
}

/// `devices/<id>.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub os: String,
    pub app_version: String,
    pub last_sync_at: String,
}

/// What became of a workspace on a device.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkspaceState {
    #[default]
    Open,
    /// Every tab closed; the workspace is still in the list.
    Closed,
    /// Removed on that device; its sessions stay here.
    Deleted,
}

/// The project a workspace or session works in, so another computer can find its own copy.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRef {
    /// Browser URL of `origin`, without credentials.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_url: Option<String>,
    /// Folder inside the repository (`""` at its root, `/`-separated).
    #[serde(default)]
    pub path_in_repo: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
}

/// A session continued from another one (another device's, or an earlier point of its own).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParentRef {
    pub device: String,
    pub session: String,
    /// The message it branched after (Claude Code's `uuid`), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// One session in a device's section of the workspace metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEntry {
    pub agent: Agent,
    pub id: String,
    pub title: String,
    /// When the transcript last changed.
    pub updated_at: String,
    /// Open in a tab on that device when it last synced.
    pub open: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<ParentRef>,
}

/// A device's view of a workspace. Only that device writes it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSection {
    pub device_name: String,
    pub synced_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<u32>,
    /// The folder on that device (a hint only: other computers find theirs through `project`).
    pub cwd: String,
    #[serde(default)]
    pub project: ProjectRef,
    #[serde(default)]
    pub state: WorkspaceState,
    /// When `state` last changed.
    #[serde(default)]
    pub state_at: String,
    #[serde(default)]
    pub sessions: Vec<SessionEntry>,
}

/// `workspace/<sync-id>/sync_metadata.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceMetadata {
    pub version: u32,
    pub sync_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin: Option<String>,
    /// By device id.
    #[serde(default)]
    pub devices: BTreeMap<String, DeviceSection>,
}

impl WorkspaceMetadata {
    /// The section written most recently: the workspace's current name, colour and layout.
    pub fn latest(&self) -> Option<(&String, &DeviceSection)> {
        self.devices.iter().max_by(|a, b| a.1.synced_at.cmp(&b.1.synced_at))
    }

    /// Every session of every device, newest first.
    pub fn sessions(&self) -> Vec<(&String, &DeviceSection, &SessionEntry)> {
        let mut all: Vec<_> =
            self.devices.iter().flat_map(|(id, section)| section.sessions.iter().map(move |s| (id, section, s))).collect();
        all.sort_by(|a, b| b.2.updated_at.cmp(&a.2.updated_at));
        all
    }
}

/// A session as the tree of branches sees it.
#[derive(Debug, Clone, Copy)]
pub struct TreeNode<'a> {
    pub device: &'a str,
    pub id: &'a str,
    pub parent: Option<&'a ParentRef>,
    pub updated_at: &'a str,
}

/// Sessions as a tree of branches: each continued session under the one it came from. Returns
/// (index into `nodes`, depth), a branch right after its parent; newest first at every level,
/// judged by the newest session in each subtree.
pub fn tree_order(nodes: &[TreeNode]) -> Vec<(usize, usize)> {
    let parent_of = |i: usize| {
        nodes[i].parent.and_then(|p| (0..nodes.len()).find(|&j| j != i && nodes[j].device == p.device && nodes[j].id == p.session))
    };
    let parents: Vec<Option<usize>> = (0..nodes.len()).map(parent_of).collect();
    let children = |of: Option<usize>| -> Vec<usize> { (0..nodes.len()).filter(|&i| parents[i] == of).collect() };
    // Newest activity anywhere below a session (a loop in bad data stops at the depth limit).
    fn newest(i: usize, nodes: &[TreeNode], parents: &[Option<usize>], depth: usize) -> String {
        let own = nodes[i].updated_at.to_string();
        if depth > 32 {
            return own;
        }
        (0..nodes.len())
            .filter(|&c| parents[c] == Some(i))
            .map(|c| newest(c, nodes, parents, depth + 1))
            .chain(std::iter::once(own))
            .max()
            .unwrap_or_default()
    }
    let mut out = Vec::new();
    let mut seen = vec![false; nodes.len()];
    let mut stack: Vec<(usize, usize)> = Vec::new();
    let mut roots = children(None);
    // A parent that points in a circle has no root: those come out as roots of their own.
    roots.extend((0..nodes.len()).filter(|&i| parents[i].is_some() && !has_root(i, &parents)));
    roots.sort_by_key(|&i| std::cmp::Reverse(newest(i, nodes, &parents, 0)));
    for root in roots.into_iter().rev() {
        stack.push((root, 0));
    }
    while let Some((i, depth)) = stack.pop() {
        if std::mem::replace(&mut seen[i], true) {
            continue;
        }
        out.push((i, depth));
        let mut kids = children(Some(i));
        kids.sort_by_key(|&c| std::cmp::Reverse(newest(c, nodes, &parents, 0)));
        for kid in kids.into_iter().rev() {
            stack.push((kid, depth + 1));
        }
    }
    out
}

fn has_root(i: usize, parents: &[Option<usize>]) -> bool {
    let mut at = i;
    for _ in 0..=parents.len() {
        match parents[at] {
            None => return true,
            Some(p) => at = p,
        }
    }
    false
}

/// One chunk of a transcript: bytes `start..end` of the file on the device that wrote it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Chunk {
    pub file: String,
    pub start: u64,
    pub end: u64,
}

/// A file from the session's side folder (Claude Code: `subagents/`, `tool-results/`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sidecar {
    /// `/`-separated, relative to the side folder.
    pub path: String,
    pub size: u64,
    pub modified_ms: u64,
}

/// `…/<session-id>/session.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionManifest {
    pub version: u32,
    pub agent: Agent,
    pub id: String,
    pub title: String,
    pub device: String,
    /// Working directory on that device.
    pub cwd: String,
    #[serde(default)]
    pub project: ProjectRef,
    /// Where the transcript sits under the agent's own session folder, `/`-separated.
    pub source: String,
    pub chunks: Vec<Chunk>,
    /// SHA-256 of the last bytes before the last chunk's end, to notice a transcript rewritten
    /// underneath (then it is uploaded again from the start).
    pub tail_hash: String,
    #[serde(default)]
    pub sidecars: Vec<Sidecar>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<ParentRef>,
    pub updated_at: String,
}

impl SessionManifest {
    pub fn uploaded_bytes(&self) -> u64 {
        self.chunks.last().map_or(0, |c| c.end)
    }
}

/// Now, as the timestamps in the repository are written (UTC, milliseconds). They sort as text.
pub fn now_stamp() -> String {
    stamp_ms(chrono::Utc::now().timestamp_millis().max(0) as u64)
}

pub fn stamp_ms(ms: u64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms as i64)
        .unwrap_or_default()
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub fn stamp_to_ms(stamp: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(stamp).ok().map(|t| t.timestamp_millis().max(0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps_sort_as_text_and_read_back() {
        let a = stamp_ms(1_700_000_000_000);
        let b = stamp_ms(1_700_000_000_001);
        assert!(a < b);
        assert_eq!(stamp_to_ms(&a), Some(1_700_000_000_000));
    }

    #[test]
    fn branches_sit_under_the_session_they_came_from() {
        let from_a = ParentRef { device: "a".into(), session: "s1".into(), message: None };
        let from_b = ParentRef { device: "b".into(), session: "s1".into(), message: None };
        let nodes = [
            TreeNode { device: "a", id: "s1", parent: None, updated_at: "1" },
            TreeNode { device: "c", id: "s9", parent: None, updated_at: "5" },
            TreeNode { device: "b", id: "s1", parent: Some(&from_a), updated_at: "3" },
            TreeNode { device: "c", id: "s1", parent: Some(&from_b), updated_at: "7" },
        ];
        // The tree whose newest session is newest comes first, each branch below its parent.
        assert_eq!(tree_order(&nodes), vec![(0, 0), (2, 1), (3, 2), (1, 0)]);
        // A loop in the data still lists every session once.
        let loop_a = ParentRef { device: "b".into(), session: "x".into(), message: None };
        let loop_b = ParentRef { device: "a".into(), session: "x".into(), message: None };
        let looped = [
            TreeNode { device: "a", id: "x", parent: Some(&loop_a), updated_at: "1" },
            TreeNode { device: "b", id: "x", parent: Some(&loop_b), updated_at: "2" },
        ];
        let order = tree_order(&looped);
        assert_eq!(order.len(), 2);
    }

    #[test]
    fn the_newest_section_names_the_workspace() {
        let mut meta = WorkspaceMetadata { version: FORMAT_VERSION, sync_id: "w".into(), ..Default::default() };
        meta.devices.insert("a".into(), DeviceSection { synced_at: stamp_ms(1), name: Some("old".into()), ..Default::default() });
        meta.devices.insert("b".into(), DeviceSection { synced_at: stamp_ms(2), name: Some("new".into()), ..Default::default() });
        assert_eq!(meta.latest().unwrap().1.name.as_deref(), Some("new"));
    }
}
