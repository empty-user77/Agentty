//! Saves and restores the workspace layout (`~/.agentty/workspaces.json`).
//! Agent panes are restored by resuming their session when a transcript exists.

use super::panes::{Axis, PaneNode};
use crate::launch::{home_dir, LaunchSpec, PaneKind};
use agentty_bridge::model::Agent;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaneSnapshot {
    pub kind: PaneKind,
    pub cwd: PathBuf,
    pub title: String,
    #[serde(default)]
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum NodeSnapshot {
    Pane(PaneSnapshot),
    Split { axis: Axis, sizes: Vec<f32>, children: Vec<NodeSnapshot> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TabSnapshot {
    pub layout: NodeSnapshot,
    #[serde(default)]
    pub active_pane: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSnapshot {
    pub id: u64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub group: Option<u64>,
    pub cwd: PathBuf,
    pub tabs: Vec<TabSnapshot>,
    #[serde(default)]
    pub active_tab: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupSnapshot {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub collapsed: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LayoutState {
    pub groups: Vec<GroupSnapshot>,
    pub workspaces: Vec<WorkspaceSnapshot>,
    #[serde(default)]
    pub active_workspace: usize,
}

impl LayoutState {
    /// Window 0 keeps `workspaces.json`; further windows use `workspaces-window<n>.json`.
    fn path(slot: usize) -> PathBuf {
        let dir = agentty_bridge::fsutil::data_dir();
        if slot == 0 {
            dir.join("workspaces.json")
        } else {
            dir.join(format!("workspaces-window{slot}.json"))
        }
    }

    pub fn load(slot: usize) -> Self {
        std::fs::read(Self::path(slot)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    /// Extra windows saved at the last quit, to reopen them.
    pub fn saved_window_slots() -> Vec<usize> {
        let Ok(entries) = std::fs::read_dir(agentty_bridge::fsutil::data_dir()) else { return Vec::new() };
        let mut slots: Vec<usize> = entries
            .flatten()
            .filter_map(|e| e.file_name().to_str()?.strip_prefix("workspaces-window")?.strip_suffix(".json")?.parse().ok())
            .filter(|slot| *slot > 0)
            .collect();
        slots.sort_unstable();
        slots
    }

    /// Forgets a window that was closed on purpose.
    pub fn remove(slot: usize) {
        if slot > 0 {
            let _ = std::fs::remove_file(Self::path(slot));
        }
    }

    pub fn save(&self, slot: usize) -> anyhow::Result<()> {
        let path = Self::path(slot);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }
}

impl NodeSnapshot {
    pub fn from_tree(tree: &PaneNode<PaneSnapshot>) -> Self {
        match tree {
            PaneNode::Leaf(pane) => NodeSnapshot::Pane(pane.clone()),
            PaneNode::Split { axis, children, sizes } => {
                NodeSnapshot::Split { axis: *axis, sizes: sizes.clone(), children: children.iter().map(Self::from_tree).collect() }
            }
        }
    }

    pub fn to_tree(&self) -> Option<PaneNode<PaneSnapshot>> {
        match self {
            NodeSnapshot::Pane(pane) => Some(PaneNode::Leaf(pane.clone())),
            NodeSnapshot::Split { axis, sizes, children } => {
                let children: Vec<_> = children.iter().filter_map(Self::to_tree).collect();
                match children.len() {
                    0 => None,
                    1 => children.into_iter().next(),
                    n => {
                        let sizes = if sizes.len() == n { sizes.clone() } else { vec![1.0 / n as f32; n] };
                        Some(PaneNode::Split { axis: *axis, children, sizes })
                    }
                }
            }
        }
    }
}

impl PaneSnapshot {
    /// How to bring this pane back: resume the agent session if its transcript still exists.
    pub fn launch_spec(&self) -> LaunchSpec {
        let cwd = if self.cwd.is_dir() { self.cwd.clone() } else { home_dir() };
        let resumable = match (self.kind, &self.session_id) {
            (PaneKind::Claude, Some(id)) => agentty_bridge::claude::exists(id).then(|| (Agent::Claude, id.clone())),
            (PaneKind::Codex, Some(id)) => agentty_bridge::codex::find(id).is_ok().then(|| (Agent::Codex, id.clone())),
            _ => None,
        };
        match resumable {
            Some((agent, id)) => LaunchSpec::resume(agent, id, self.title.clone(), cwd),
            None => {
                let mut spec = LaunchSpec::new(self.kind, cwd);
                if self.kind != PaneKind::Shell {
                    spec.title = self.title.clone();
                }
                spec
            }
        }
    }
}

impl PartialEq for PaneSnapshot {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind && self.cwd == other.cwd && self.title == other.title && self.session_id == other.session_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_roundtrip() {
        let pane = |title: &str| PaneSnapshot { kind: PaneKind::Shell, cwd: "/tmp".into(), title: title.into(), session_id: None };
        let mut tree = PaneNode::Leaf(pane("a"));
        tree.split(&pane("a"), pane("b"), Axis::Vertical);
        let json = serde_json::to_string(&NodeSnapshot::from_tree(&tree)).unwrap();
        let restored = serde_json::from_str::<NodeSnapshot>(&json).unwrap().to_tree().unwrap();
        let titles: Vec<_> = restored.leaves().into_iter().map(|p| p.title).collect();
        assert_eq!(titles, ["a", "b"]);
    }
}
