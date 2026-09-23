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
    /// What was actually running in the pane ("claude", "codex", "shell", …), which is not always
    /// what it was started as. Kept so a folded-away workspace shows the logo it had open.
    #[serde(default)]
    pub tool: Option<String>,
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
    /// Tabs closed in this workspace, newest first ("recently closed tabs").
    #[serde(default)]
    pub closed_tabs: Vec<TabSnapshot>,
    /// Legacy: an index into the old fixed accent list. Read, never written.
    #[serde(default)]
    pub color: Option<usize>,
    /// The colour the card is filled with, as `0xRRGGBB`.
    #[serde(default)]
    pub color_value: Option<u32>,
    /// The branch the workspace was last on. The folder is read again in the background, so this
    /// is only what the card shows until that answer arrives.
    #[serde(default)]
    pub branch: Option<String>,
    /// When one of its panes last said something, so a closed workspace keeps its "5분 전".
    #[serde(default)]
    pub last_activity_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupSnapshot {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub collapsed: bool,
    /// Legacy accent index, read for layouts written before colours were free-form.
    #[serde(default)]
    pub color: Option<usize>,
    #[serde(default)]
    pub color_value: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LayoutState {
    pub groups: Vec<GroupSnapshot>,
    pub workspaces: Vec<WorkspaceSnapshot>,
    #[serde(default)]
    pub active_workspace: usize,
    #[serde(default)]
    pub ungrouped_collapsed: bool,
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

    /// Every extra window with a saved layout (open at the last quit or recently closed).
    fn all_window_slots() -> Vec<usize> {
        let Ok(entries) = std::fs::read_dir(agentty_bridge::fsutil::data_dir()) else { return Vec::new() };
        let mut slots: Vec<usize> = entries
            .flatten()
            .filter_map(|e| e.file_name().to_str()?.strip_prefix("workspaces-window")?.strip_suffix(".json")?.parse().ok())
            .filter(|slot| *slot > 0)
            .collect();
        slots.sort_unstable();
        slots
    }

    /// Extra windows that were open at the last quit, to reopen them.
    pub fn saved_window_slots() -> Vec<usize> {
        let closed = ClosedWindows::load();
        Self::all_window_slots().into_iter().filter(|slot| !closed.contains(*slot)).collect()
    }

    /// First slot no saved window uses.
    pub fn next_free_slot() -> usize {
        Self::all_window_slots().last().map_or(1, |slot| slot + 1)
    }

    fn remove(slot: usize) {
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

/// A window closed on purpose, kept so it can be reopened (Dock menu, History menu).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClosedWindow {
    pub slot: usize,
    pub title: String,
    pub closed_at_ms: u64,
}

/// `~/.agentty/closed-windows.json`, most recently closed first.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ClosedWindows {
    pub windows: Vec<ClosedWindow>,
}

impl ClosedWindows {
    const LIMIT: usize = 10;

    fn path() -> PathBuf {
        agentty_bridge::fsutil::data_dir().join("closed-windows.json")
    }

    pub fn load() -> Self {
        std::fs::read(Self::path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    fn save(&self) {
        let path = Self::path();
        let tmp = path.with_extension("json.tmp");
        if let Ok(bytes) = serde_json::to_vec_pretty(self) {
            if std::fs::write(&tmp, bytes).is_ok() {
                let _ = std::fs::rename(tmp, path);
            }
        }
    }

    pub fn contains(&self, slot: usize) -> bool {
        self.windows.iter().any(|w| w.slot == slot)
    }

    /// Adds `window` at the front; windows pushed past the limit are forgotten with their layout.
    fn push(&mut self, window: ClosedWindow) -> Vec<usize> {
        self.windows.retain(|w| w.slot != window.slot);
        self.windows.insert(0, window);
        self.windows.split_off(self.windows.len().min(Self::LIMIT)).into_iter().map(|w| w.slot).collect()
    }

    /// Remembers a closed window (its layout is already saved); empty windows are just deleted.
    pub fn remember(slot: usize, title: String, has_workspaces: bool) {
        if slot == 0 {
            return;
        }
        if !has_workspaces {
            LayoutState::remove(slot);
            return;
        }
        let mut closed = Self::load();
        let dropped = closed.push(ClosedWindow { slot, title, closed_at_ms: crate::ui::now_ms() });
        dropped.into_iter().for_each(LayoutState::remove);
        closed.save();
    }

    /// Marks a window as open again.
    pub fn reopen(slot: usize) {
        let mut closed = Self::load();
        closed.windows.retain(|w| w.slot != slot);
        closed.save();
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

/// What a pane is saved as: its kind and the session to resume. `running` is the agent in the
/// pane now, `live` the session it is in, `launched` the one the pane was started with.
///
/// Claude or Codex typed into a shell comes back as that agent, resumed, once its session is known
/// (the shell is where quitting it leads anyway). `/clear` and `/resume` move an agent to another
/// session than it was started with: the one running now is the one to come back to.
pub fn saved_session(
    spec: PaneKind,
    running: Option<PaneKind>,
    live: Option<String>,
    launched: Option<String>,
) -> (PaneKind, Option<String>) {
    let kind = match spec {
        PaneKind::Shell => running.filter(|_| live.is_some()).unwrap_or(PaneKind::Shell),
        kind => kind,
    };
    let live = live.filter(|_| running == Some(kind));
    match kind {
        PaneKind::Shell => (kind, None),
        _ => (kind, live.or(launched)),
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
    fn panes_are_saved_with_the_session_running_now() {
        let id = |s: &str| Some(s.to_string());
        use PaneKind::{Claude, Codex, Shell};
        // A plain shell, and a shell whose agent's session is not known yet.
        assert_eq!(saved_session(Shell, None, None, None), (Shell, None));
        assert_eq!(saved_session(Shell, Some(Claude), None, None), (Shell, None));
        // Claude typed into a shell comes back as Claude in that session.
        assert_eq!(saved_session(Shell, Some(Claude), id("typed"), None), (Claude, id("typed")));
        assert_eq!(saved_session(Shell, Some(Codex), id("typed"), None), (Codex, id("typed")));
        // ... and as a shell once it quit, even though the pane remembers the session.
        assert_eq!(saved_session(Shell, None, id("typed"), None), (Shell, None));
        // A launched Claude after `/clear` resumes the new session.
        assert_eq!(saved_session(Claude, Some(Claude), id("after-clear"), id("launched")), (Claude, id("after-clear")));
        assert_eq!(saved_session(Claude, Some(Claude), None, id("launched")), (Claude, id("launched")));
        // Claude quit and Codex was typed in its shell: the Claude session is still what comes back.
        assert_eq!(saved_session(Claude, Some(Codex), id("codex"), id("launched")), (Claude, id("launched")));
        assert_eq!(saved_session(Claude, None, id("old"), id("launched")), (Claude, id("launched")));
    }

    #[test]
    fn closed_windows_keep_the_most_recent() {
        let mut closed = ClosedWindows::default();
        for slot in 1..=12 {
            let dropped = closed.push(ClosedWindow { slot, title: format!("w{slot}"), closed_at_ms: slot as u64 });
            assert_eq!(dropped, if slot > 10 { vec![slot - 10] } else { vec![] });
        }
        assert_eq!(closed.windows.first().map(|w| w.slot), Some(12));
        closed.push(ClosedWindow { slot: 5, title: "again".into(), closed_at_ms: 99 });
        assert_eq!(closed.windows.len(), 10);
        assert_eq!(closed.windows[0].title, "again");
        assert!(closed.contains(5) && !closed.contains(1));
    }

    /// A layout written before a workspace remembered its last state still loads, and what is
    /// written now comes back whole.
    #[test]
    fn last_state_survives_the_round_trip() {
        let old = r#"{"id":1,"cwd":"/tmp","tabs":[]}"#;
        let loaded: WorkspaceSnapshot = serde_json::from_str(old).unwrap();
        assert_eq!(loaded.branch, None);
        assert_eq!(loaded.last_activity_ms, None);
        let saved = WorkspaceSnapshot {
            branch: Some("feat/cards".into()),
            last_activity_ms: Some(1_700_000_000_000),
            tabs: vec![TabSnapshot {
                layout: NodeSnapshot::Pane(PaneSnapshot {
                    kind: PaneKind::Shell,
                    cwd: "/tmp".into(),
                    title: "t".into(),
                    session_id: None,
                    tool: Some("claude".into()),
                }),
                active_pane: 0,
            }],
            ..loaded
        };
        let back: WorkspaceSnapshot = serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
        assert_eq!(back.branch.as_deref(), Some("feat/cards"));
        assert_eq!(back.last_activity_ms, Some(1_700_000_000_000));
        let NodeSnapshot::Pane(pane) = &back.tabs[0].layout else { panic!("one pane") };
        assert_eq!(pane.tool.as_deref(), Some("claude"));
    }

    #[test]
    fn layout_roundtrip() {
        let pane =
            |title: &str| PaneSnapshot { kind: PaneKind::Shell, cwd: "/tmp".into(), title: title.into(), session_id: None, tool: None };
        let mut tree = PaneNode::Leaf(pane("a"));
        tree.split(&pane("a"), pane("b"), Axis::Vertical);
        let json = serde_json::to_string(&NodeSnapshot::from_tree(&tree)).unwrap();
        let restored = serde_json::from_str::<NodeSnapshot>(&json).unwrap().to_tree().unwrap();
        let titles: Vec<_> = restored.leaves().into_iter().map(|p| p.title).collect();
        assert_eq!(titles, ["a", "b"]);
    }
}
