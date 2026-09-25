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
    /// The automation the tab is, in a plugin's workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<TabInstance>,
    pub layout: NodeSnapshot,
    #[serde(default)]
    pub active_pane: usize,
    /// The pane in focus view (⇧⌘↩), by its place among the tab's panes.
    #[serde(default)]
    pub zoomed_pane: Option<usize>,
}

/// A tab of a plugin's workspace as one automation: an id the plugin knows it by, and the name it
/// gave it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TabInstance {
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
}

/// What was open around the terminals: the sidebar and the panels docked beside them. Their
/// widths are settings; this is whether they were showing, and what.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelState {
    #[serde(default)]
    pub sidebar_hidden: bool,
    #[serde(default)]
    pub files_open: bool,
    /// The working tree the files panel was pinned to, if it was.
    #[serde(default)]
    pub files_pinned: Option<PathBuf>,
    /// The in-app browser's tabs by address, and the one in front; empty when it was closed.
    #[serde(default)]
    pub browser_tabs: Vec<String>,
    #[serde(default)]
    pub browser_active: usize,
    /// The plugin whose panel was open.
    #[serde(default)]
    pub plugin: Option<String>,
    #[serde(default)]
    pub docker_open: bool,
    #[serde(default)]
    pub database_open: bool,
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
    /// The plugin the workspace belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin: Option<String>,
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

/// Where the window was and how big, in screen points. `maximized` / `fullscreen` keep the size
/// it goes back to when left.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowState {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    #[serde(default)]
    pub maximized: bool,
    #[serde(default)]
    pub fullscreen: bool,
}

impl WindowState {
    pub fn from_bounds(bounds: gpui::WindowBounds) -> Self {
        let (rect, maximized, fullscreen) = match bounds {
            gpui::WindowBounds::Windowed(rect) => (rect, false, false),
            gpui::WindowBounds::Maximized(rect) => (rect, true, false),
            gpui::WindowBounds::Fullscreen(rect) => (rect, false, true),
        };
        Self {
            x: f32::from(rect.origin.x),
            y: f32::from(rect.origin.y),
            width: f32::from(rect.size.width),
            height: f32::from(rect.size.height),
            maximized,
            fullscreen,
        }
    }

    /// The bounds to open with, when enough of the window lands on one of `screens` to grab its
    /// title bar (a monitor that was unplugged, a resolution that shrank: the default instead).
    /// Never smaller than `min`.
    pub fn to_bounds(self, screens: &[gpui::Bounds<gpui::Pixels>], min: gpui::Size<gpui::Pixels>) -> Option<gpui::WindowBounds> {
        use gpui::{point, px, size, Bounds};
        if !(self.x.is_finite() && self.y.is_finite() && self.width.is_finite() && self.height.is_finite()) {
            return None;
        }
        let rect = Bounds::new(
            point(px(self.x), px(self.y)),
            size(px(self.width.max(f32::from(min.width))), px(self.height.max(f32::from(min.height)))),
        );
        // The strip along the top where the title bar is: 120 × 30 of it must be on a screen.
        let reachable = screens.iter().any(|screen| {
            let left = rect.origin.x.max(screen.origin.x);
            let right = (rect.origin.x + rect.size.width).min(screen.origin.x + screen.size.width);
            let top = rect.origin.y.max(screen.origin.y);
            let bottom = (rect.origin.y + px(30.)).min(screen.origin.y + screen.size.height);
            right - left >= px(120.) && bottom - top >= px(30.)
        });
        reachable.then_some(if self.fullscreen {
            gpui::WindowBounds::Fullscreen(rect)
        } else if self.maximized {
            gpui::WindowBounds::Maximized(rect)
        } else {
            gpui::WindowBounds::Windowed(rect)
        })
    }
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
    /// Plugins' workspaces made before the Plugins group were moved into it (once: where the user
    /// puts them afterwards stays theirs).
    #[serde(default)]
    pub plugins_grouped: bool,
    /// The window's place and size at the last save.
    #[serde(default)]
    pub window: Option<WindowState>,
    #[serde(default)]
    pub panels: PanelState,
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

    /// Reads the layout. A workspace that can't be read (a file from a newer version, one damaged
    /// on disk) is left out rather than taking every other one with it, and the file is copied
    /// aside first: the next save would otherwise write over the only copy of what was lost.
    pub fn load(slot: usize) -> Self {
        let path = Self::path(slot);
        let Ok(bytes) = std::fs::read(&path) else { return Self::default() };
        let (state, complete) = Self::parse(&bytes);
        if !complete {
            let backup = path.with_extension(format!("json.unreadable-{}", crate::ui::now_ms()));
            match std::fs::copy(&path, &backup) {
                Ok(_) => eprintln!("agentty: part of {} could not be read; kept a copy at {}", path.display(), backup.display()),
                Err(err) => eprintln!("agentty: part of {} could not be read, and no copy could be kept: {err}", path.display()),
            }
        }
        state
    }

    /// The layout in `bytes`, and whether all of it could be read.
    fn parse(bytes: &[u8]) -> (Self, bool) {
        if let Ok(state) = serde_json::from_slice::<Self>(bytes) {
            return (state, true);
        }
        let Ok(serde_json::Value::Object(mut value)) = serde_json::from_slice::<serde_json::Value>(bytes) else {
            return (Self::default(), bytes.iter().all(u8::is_ascii_whitespace));
        };
        // One piece at a time: the workspaces and groups that read, and the rest as it can.
        let items = |value: Option<serde_json::Value>| match value {
            Some(serde_json::Value::Array(items)) => items,
            _ => Vec::new(),
        };
        let workspaces: Vec<WorkspaceSnapshot> =
            items(value.remove("workspaces")).into_iter().filter_map(|w| serde_json::from_value(w).ok()).collect();
        let groups: Vec<GroupSnapshot> = items(value.remove("groups")).into_iter().filter_map(|g| serde_json::from_value(g).ok()).collect();
        let field = |value: &serde_json::Map<String, serde_json::Value>, key: &str| value.get(key).cloned().unwrap_or_default();
        let state = Self {
            groups,
            workspaces,
            active_workspace: serde_json::from_value(field(&value, "activeWorkspace")).unwrap_or_default(),
            ungrouped_collapsed: serde_json::from_value(field(&value, "ungroupedCollapsed")).unwrap_or_default(),
            plugins_grouped: serde_json::from_value(field(&value, "pluginsGrouped")).unwrap_or_default(),
            window: serde_json::from_value(field(&value, "window")).unwrap_or_default(),
            panels: serde_json::from_value(field(&value, "panels")).unwrap_or_default(),
        };
        (state, false)
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
        // A name of this process's own, so a second Agentty on the same folder can't interleave
        // its writes with ours; flushed to disk before it replaces the file, so a power cut leaves
        // the old layout or the new one, never an empty file.
        let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
        {
            use std::io::Write as _;
            let mut file = std::fs::File::create(&tmp)?;
            file.write_all(&serde_json::to_vec_pretty(self)?)?;
            file.sync_all()?;
        }
        // Windows: a virus scanner or the search indexer can hold the file for a moment.
        let mut attempt = 0;
        loop {
            match std::fs::rename(&tmp, &path) {
                Ok(()) => return Ok(()),
                Err(_) if attempt < 3 => {
                    attempt += 1;
                    std::thread::sleep(std::time::Duration::from_millis(50 * attempt));
                }
                Err(err) => {
                    let _ = std::fs::remove_file(&tmp);
                    return Err(err.into());
                }
            }
        }
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
        let missing = (!self.cwd.is_dir()).then(|| self.cwd.clone());
        let cwd = if missing.is_none() { self.cwd.clone() } else { home_dir() };
        let resumable = match (self.kind, &self.session_id) {
            (PaneKind::Claude, Some(id)) => agentty_bridge::claude::exists(id).then(|| (Agent::Claude, id.clone())),
            (PaneKind::Codex, Some(id)) => agentty_bridge::codex::find(id).is_ok().then(|| (Agent::Codex, id.clone())),
            _ => None,
        };
        let mut spec = match resumable {
            Some((agent, id)) => LaunchSpec::resume(agent, id, self.title.clone(), cwd),
            None => {
                let mut spec = LaunchSpec::new(self.kind, cwd);
                if self.kind != PaneKind::Shell {
                    spec.title = self.title.clone();
                }
                spec
            }
        };
        spec.missing_cwd = missing;
        spec
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
    fn a_window_comes_back_where_it_was_while_that_is_on_screen() {
        use gpui::{point, px, size, Bounds};
        let screen = Bounds::new(point(px(0.), px(0.)), size(px(1512.), px(982.)));
        let second = Bounds::new(point(px(1512.), px(0.)), size(px(2560.), px(1440.)));
        let min = size(px(720.), px(440.));
        let saved = WindowState { x: 100., y: 50., width: 1200., height: 800., maximized: false, fullscreen: false };
        let json = serde_json::to_string(&saved).unwrap();
        let loaded: WindowState = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded, saved);
        assert_eq!(
            loaded.to_bounds(&[screen], min),
            Some(gpui::WindowBounds::Windowed(Bounds::new(point(px(100.), px(50.)), size(px(1200.), px(800.)))))
        );
        // On the second monitor: only while it is connected.
        let there = WindowState { x: 2000., y: 100., ..saved };
        assert!(there.to_bounds(&[screen, second], min).is_some());
        assert_eq!(there.to_bounds(&[screen], min), None);
        // Mostly off the edge, title bar still reachable: kept. Title bar above the screen: not.
        assert!(WindowState { x: 1300., ..saved }.to_bounds(&[screen], min).is_some());
        assert_eq!(WindowState { y: -200., ..saved }.to_bounds(&[screen], min), None);
        // Maximized keeps its restore size; a size under the minimum grows to it.
        assert!(matches!(WindowState { maximized: true, ..saved }.to_bounds(&[screen], min), Some(gpui::WindowBounds::Maximized(_))));
        let tiny = WindowState { width: 100., height: 50., ..saved }.to_bounds(&[screen], min).unwrap();
        assert!(matches!(tiny, gpui::WindowBounds::Windowed(b) if b.size == min));
        // Written before windows were remembered.
        let old: LayoutState = serde_json::from_str(r#"{"groups":[],"workspaces":[]}"#).unwrap();
        assert_eq!(old.window, None);
    }

    #[test]
    fn open_panels_survive_the_round_trip() {
        let panels = PanelState {
            sidebar_hidden: true,
            files_open: true,
            files_pinned: Some("/tmp/tree".into()),
            browser_tabs: vec!["http://localhost:3000/".into(), "https://example.com/".into()],
            browser_active: 1,
            plugin: Some("launch".into()),
            docker_open: false,
            database_open: true,
        };
        let state = LayoutState { panels: panels.clone(), ..LayoutState::default() };
        let back: LayoutState = serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
        assert_eq!(back.panels, panels);
        // A layout from before: nothing open, sidebar showing.
        let old: LayoutState = serde_json::from_str(r#"{"groups":[],"workspaces":[]}"#).unwrap();
        assert_eq!(old.panels, PanelState::default());
        assert!(!old.panels.sidebar_hidden);
        // A tab written before focus view was saved.
        let tab: TabSnapshot =
            serde_json::from_str(r#"{"layout":{"type":"pane","kind":"shell","cwd":"/tmp","title":"t"},"activePane":0}"#).unwrap();
        assert_eq!(tab.zoomed_pane, None);
    }

    #[test]
    fn one_unreadable_workspace_does_not_take_the_others() {
        let good = r#"{"id":1,"cwd":"/tmp","tabs":[{"layout":{"type":"pane","kind":"shell","cwd":"/tmp","title":"t"}}]}"#;
        // A pane kind this version doesn't know (written by a newer one).
        let newer = r#"{"id":2,"cwd":"/tmp","tabs":[{"layout":{"type":"pane","kind":"gemini","cwd":"/tmp","title":"g"}}]}"#;
        let file = format!(
            r#"{{"groups":[{{"id":9,"name":"g"}}],"workspaces":[{good},{newer}],"activeWorkspace":1,"panels":{{"sidebarHidden":true}}}}"#
        );
        let (state, complete) = LayoutState::parse(file.as_bytes());
        assert!(!complete);
        assert_eq!(state.workspaces.iter().map(|w| w.id).collect::<Vec<_>>(), [1]);
        assert_eq!(state.groups.len(), 1);
        assert_eq!(state.active_workspace, 1);
        assert!(state.panels.sidebar_hidden);
        // A file that read whole says so; an empty one is nothing to keep.
        let (state, complete) = LayoutState::parse(format!(r#"{{"groups":[],"workspaces":[{good}]}}"#).as_bytes());
        assert!(complete && state.workspaces.len() == 1);
        assert!(!LayoutState::parse(b"{\"groups\":[").1);
        assert!(LayoutState::parse(b"").1);
    }

    #[test]
    fn a_missing_folder_is_remembered_not_replaced() {
        let gone = PathBuf::from("/nonexistent/agentty-test-folder");
        let pane = PaneSnapshot { kind: PaneKind::Shell, cwd: gone.clone(), title: "t".into(), session_id: None, tool: None };
        let spec = pane.launch_spec();
        assert_eq!(spec.cwd, home_dir());
        assert_eq!(spec.missing_cwd, Some(gone));
        let here = PaneSnapshot { cwd: std::env::temp_dir(), ..pane };
        assert_eq!(here.launch_spec().missing_cwd, None);
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
                instance: Some(TabInstance { id: "a1".into(), title: Some("Brand B".into()) }),
                layout: NodeSnapshot::Pane(PaneSnapshot {
                    kind: PaneKind::Shell,
                    cwd: "/tmp".into(),
                    title: "t".into(),
                    session_id: None,
                    tool: Some("claude".into()),
                }),
                active_pane: 0,
                zoomed_pane: None,
            }],
            ..loaded
        };
        let back: WorkspaceSnapshot = serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
        assert_eq!(back.branch.as_deref(), Some("feat/cards"));
        assert_eq!(back.last_activity_ms, Some(1_700_000_000_000));
        let NodeSnapshot::Pane(pane) = &back.tabs[0].layout else { panic!("one pane") };
        assert_eq!(pane.tool.as_deref(), Some("claude"));
        // A plugin workspace's tab stays the same automation across a restart.
        assert_eq!(back.tabs[0].instance.as_ref().map(|i| i.id.as_str()), Some("a1"));
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
