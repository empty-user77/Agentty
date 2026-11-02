//! Monitoring → Trash: "Delete Workspace" moves a workspace here instead of removing it. It keeps
//! the saved form of the workspace (`~/.agentty/trash.json`, shared by the windows), and restoring
//! brings it back asleep. What deleting would destroy waits for the permanent delete: the worktrees
//! ticked to go with it and its sync data. That delete is asked for here, or comes by itself once
//! the retention period (Settings, or this page) has passed. Agents' own transcripts are never
//! touched: they stay where the agent keeps them, as they always have.

use super::persist::{LayoutState, NodeSnapshot, PaneSnapshot, TabSnapshot, WorkspaceSnapshot};
use super::tree_manager::{badge, check_box, confirm_dialog};
use super::{Workbench, Workspace};
use crate::i18n::{t, tf};
use crate::launch::PaneKind;
use crate::settings::{settings, update_settings};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, IconSize, TypeScale};
use crate::usage_view::card;
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, SharedString, Window};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const DAY_SECS: i64 = 24 * 60 * 60;
/// How long a workspace stays in the trash unless set otherwise.
pub const DEFAULT_RETENTION_DAYS: u32 = 7;
/// The periods the − and + buttons step through.
const RETENTION_CHOICES: [u32; 9] = [1, 3, 7, 14, 30, 60, 90, 180, 365];

/// A workspace in the trash.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashedWorkspace {
    pub trash_id: u64,
    /// The name it had in the list.
    pub title: String,
    /// The folder it worked in.
    pub folder: PathBuf,
    /// Seconds since the epoch.
    pub deleted_at: i64,
    /// When it goes by itself under the period set when it was deleted (see `expiry`).
    pub expires_at: i64,
    pub snapshot: WorkspaceSnapshot,
    /// Group ids are given out again after a restart: it comes back to its group by name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_name: Option<String>,
    /// Linked working trees to remove with it, when it is deleted for good.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trees: Vec<PathBuf>,
    /// Its id in the sync repository: restoring syncs on under it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync_id: Option<String>,
    /// "Also delete sync data" was ticked: the data leaves with the permanent delete.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub purge_sync: bool,
    /// Its orchestrations: their list leaves out what has no workspace, and restoring adds them back.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub orchestrations: Vec<super::orchestration::Record>,
    /// Board tickets it worked on, bound to it again when it comes back.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tickets: Vec<u64>,
}

impl TrashedWorkspace {
    /// When it goes by itself. A longer period set later keeps it longer; a shorter one only applies
    /// to what is deleted from then on, so changing the setting never deletes anything by surprise.
    pub fn expiry(&self, retention_days: u32) -> i64 {
        self.expires_at.max(self.deleted_at + i64::from(retention_days) * DAY_SECS)
    }

    /// The agent sessions of its tabs (open and set aside), once each.
    pub fn sessions(&self) -> Vec<(PaneKind, String)> {
        fn walk(node: &NodeSnapshot, out: &mut Vec<(PaneKind, String)>) {
            match node {
                NodeSnapshot::Pane(PaneSnapshot { kind, session_id: Some(id), .. }) => {
                    if !out.iter().any(|(_, known)| known == id) {
                        out.push((*kind, id.clone()));
                    }
                }
                NodeSnapshot::Pane(_) => {}
                NodeSnapshot::Split { children, .. } => children.iter().for_each(|child| walk(child, out)),
            }
        }
        let mut sessions = Vec::new();
        for tab in self.snapshot.tabs.iter().chain(&self.snapshot.parked) {
            walk(&tab.layout, &mut sessions);
        }
        sessions
    }
}

/// `~/.agentty/trash.json`.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashFile {
    pub workspaces: Vec<TrashedWorkspace>,
    /// Sync data of workspaces deleted for good, while sync can't take it (off, paused): the delete
    /// was asked for, and a week may have passed, so it waits here rather than in memory.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_sync_purges: Vec<PendingSyncPurge>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingSyncPurge {
    pub plugin: Option<String>,
    pub sync_id: String,
}

impl TrashFile {
    pub fn path() -> PathBuf {
        agentty_bridge::fsutil::data_dir().join("trash.json")
    }

    /// What to show: an unreadable file shows as empty.
    pub fn load(path: &Path) -> Self {
        Self::read(path).unwrap_or_default()
    }

    /// Reads the trash to change it. No file is an empty trash; a file that can't be read right now
    /// is an error, since saving would replace what it holds. A file or an entry that was read but
    /// not understood (from a newer version, or damaged) is left out, its copy kept aside.
    pub fn read(path: &Path) -> std::io::Result<Self> {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(err) => return Err(err),
        };
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Raw {
            workspaces: Vec<serde_json::Value>,
            #[serde(default)]
            pending_sync_purges: Vec<PendingSyncPurge>,
        }
        let raw = serde_json::from_slice::<Raw>(&bytes);
        let damaged = raw.is_err();
        let (raw, pending_sync_purges) = raw.map_or_else(|_| Default::default(), |raw| (raw.workspaces, raw.pending_sync_purges));
        let total = raw.len();
        let workspaces: Vec<TrashedWorkspace> = raw.into_iter().filter_map(|value| serde_json::from_value(value).ok()).collect();
        if damaged || workspaces.len() != total {
            Self::keep_aside(path, &bytes);
        }
        Ok(Self { workspaces, pending_sync_purges })
    }

    /// A copy named after its content: reading the same file again makes no second one, and a
    /// later damaged file does not replace an earlier copy.
    fn keep_aside(path: &Path, bytes: &[u8]) {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        bytes.hash(&mut hasher);
        let copy = path.with_extension(format!("json.unreadable-{:016x}", hasher.finish()));
        if !copy.exists() {
            let _ = agentty_bridge::fsutil::write_private(&copy, bytes);
        }
    }

    /// Private (0600) and written whole: it holds folders, titles and session ids.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        agentty_bridge::fsutil::write_private_durable(path, &serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }

    /// An id no entry has, not reused soon after one left (what the page remembers goes by it).
    fn new_id(&self, now: i64) -> u64 {
        let newest = self.workspaces.iter().map(|w| w.trash_id).max().unwrap_or(0);
        (newest + 1).max(u64::try_from(now).unwrap_or(0).saturating_mul(1000))
    }

    /// Takes the entries `which` picks out of the file.
    fn take(&mut self, which: impl Fn(&TrashedWorkspace) -> bool) -> Vec<TrashedWorkspace> {
        let (taken, kept) = std::mem::take(&mut self.workspaces).into_iter().partition(|w| which(w));
        self.workspaces = kept;
        taken
    }
}

/// Of the entries asked to come back, those that can't: a plugin has one workspace, so an entry
/// waits while its plugin has one open, or while an earlier entry of the same plugin comes back.
fn waiting_for_their_plugin(items: &[TrashedWorkspace], ids: &[u64], mut open_plugins: HashSet<String>) -> Vec<u64> {
    items
        .iter()
        .filter(|item| ids.contains(&item.trash_id))
        .filter(|item| item.snapshot.plugin.as_ref().is_some_and(|plugin| !open_plugins.insert(plugin.clone())))
        .map(|item| item.trash_id)
        .collect()
}

/// The period the − or + button leads to, also from a value set by hand in the settings file.
fn stepped_retention(days: u32, up: bool) -> u32 {
    if up {
        RETENTION_CHOICES.iter().copied().find(|c| *c > days).unwrap_or(days)
    } else {
        RETENTION_CHOICES.iter().rev().copied().find(|c| *c < days).unwrap_or(days)
    }
}

fn retention(cx: &gpui::App) -> u32 {
    settings(cx).trash_retention_days.max(1)
}

fn now_secs() -> i64 {
    chrono::Local::now().timestamp()
}

fn date_text(secs: i64) -> String {
    chrono::DateTime::from_timestamp(secs, 0)
        .map(|at| at.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "—".into())
}

/// Whole days until `expiry`, rounded up: what is left of today still counts as a day.
fn days_left(expiry: i64, now: i64) -> i64 {
    ((expiry - now).max(0) + DAY_SECS - 1) / DAY_SECS
}

/// Where a session's transcript is, or its id when it is not on this computer any more.
fn session_location(kind: PaneKind, id: &str) -> String {
    let path = match kind {
        PaneKind::Claude => agentty_bridge::claude::find(id).ok(),
        PaneKind::Codex => agentty_bridge::codex::find(id).ok(),
        PaneKind::Shell => None,
    };
    path.map_or_else(|| id.to_string(), |path| crate::ui::tilde(&path))
}

/// The − and + of the retention period (this page and Settings → General).
pub(super) fn retention_stepper(cx: &mut Context<Workbench>) -> gpui::Div {
    let days = retention(cx);
    let step = |id: &'static str, label: &'static str, up: bool, cx: &mut Context<Workbench>| {
        crate::ui::action_button(
            id,
            label,
            cx.listener(move |_, _: &ClickEvent, _, cx| {
                update_settings(cx, move |s| s.trash_retention_days = stepped_retention(s.trash_retention_days.max(1), up));
            }),
        )
    };
    div()
        .flex()
        .items_center()
        .gap_1()
        .child(step("trash-retention-dec", "−", false, cx))
        .child(div().w(px(56.)).flex().justify_center().t_body().text_color(hex(Chrome::BRIGHT)).child(tf(
            cx,
            "trash.days",
            &[("n", &days.to_string())],
        )))
        .child(step("trash-retention-inc", "+", true, cx))
}

/// What the page keeps between frames.
#[derive(Default)]
pub struct TrashPage {
    scroll: gpui::ScrollHandle,
    items: Vec<TrashedWorkspace>,
    /// The file as it was when read: another window's change shows without asking.
    stamp: Option<SystemTime>,
    loaded: bool,
    selected: HashSet<u64>,
    /// Waiting for "Delete for good?": which entries.
    confirm: Option<Vec<u64>>,
    /// Transcript locations by entry, looked up in the background (it means reading folders).
    sessions: HashMap<u64, Vec<String>>,
}

impl Workbench {
    /// A workspace as it is saved, asleep or awake.
    pub(super) fn workspace_snapshot(&self, ws: &Workspace, cx: &gpui::App) -> WorkspaceSnapshot {
        if let Some(dormant) = &ws.dormant {
            // Live tabs beside a snapshot are saved with it, never dropped.
            let mut tabs: Vec<TabSnapshot> = if ws.asleep_on_close && !ws.tabs.is_empty() { Vec::new() } else { dormant.tabs.clone() };
            tabs.extend(ws.tabs.iter().map(|tab| self.snapshot_tab(tab, cx)));
            return WorkspaceSnapshot {
                name: ws.name.clone(),
                group: ws.group,
                closed_tabs: ws.closed_tabs.clone(),
                color: None,
                color_value: ws.color,
                remote_hidden: ws.remote_hidden,
                marks: ws.marks.clone(),
                parked: ws.parked.clone(),
                tabs,
                ..dormant.clone()
            };
        }
        self.live_snapshot(ws, cx)
    }

    /// "Delete Workspace": into the trash, its terminals closed. `trees` and `purge_sync` wait for
    /// the permanent delete. If the trash can't be written the workspace stays where it is.
    pub(super) fn move_workspace_to_trash(
        &mut self,
        id: u64,
        trees: Vec<PathBuf>,
        purge_sync: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ws) = self.workspaces.iter().find(|w| w.id == id) else { return };
        let (path, now, days) = (TrashFile::path(), now_secs(), retention(cx));
        let mut file = match TrashFile::read(&path) {
            Ok(file) => file,
            Err(err) => return self.show_toast_for(tf(cx, "trash.move_failed", &[("error", &err.to_string())]), 8000, cx),
        };
        let title = self.workspace_title(ws, cx);
        let tickets = self.board_tickets_of(id);
        let orchestrations =
            self.workspaces.iter().position(|w| w.id == id).map(|index| self.orchestrations_in(index, cx)).unwrap_or_default();
        let Some(ws) = self.workspaces.iter().find(|w| w.id == id) else { return };
        file.workspaces.push(TrashedWorkspace {
            trash_id: file.new_id(now),
            title: title.clone(),
            folder: self.workspace_folder(ws, cx),
            deleted_at: now,
            expires_at: now + i64::from(days) * DAY_SECS,
            snapshot: self.workspace_snapshot(ws, cx),
            group_name: ws.group.and_then(|g| self.groups.iter().find(|group| group.id == g)).map(|g| g.name.clone()),
            trees,
            sync_id: self.workspace_sync_id(id),
            purge_sync,
            orchestrations,
            tickets,
        });
        if let Err(err) = file.save(&path) {
            return self.show_toast_for(tf(cx, "trash.move_failed", &[("error", &format!("{err:#}"))]), 8000, cx);
        }
        self.close_workspace(id, window, cx);
        self.trash.loaded = false;
        self.show_toast(tf(cx, "trash.moved", &[("name", &title), ("n", &days.to_string())]), cx);
    }

    /// Opens the page (from the workspace menu; a second click does not close it).
    pub(super) fn open_trash(&mut self, cx: &mut Context<Self>) {
        self.workspace_menu = None;
        if self.page != Some(super::Page::Trash) {
            self.open_page(super::Page::Trash, cx);
        }
        cx.notify();
    }

    /// Reads the file when it changed, and looks up the transcripts of entries new to the page.
    fn refresh_trash(&mut self, cx: &mut Context<Self>) {
        let path = TrashFile::path();
        let stamp = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        if self.trash.loaded && self.trash.stamp == stamp {
            return;
        }
        let mut items = TrashFile::load(&path).workspaces;
        items.sort_by_key(|item| std::cmp::Reverse(item.deleted_at));
        let page = &mut self.trash;
        page.loaded = true;
        page.stamp = stamp;
        let listed: HashSet<u64> = items.iter().map(|item| item.trash_id).collect();
        page.selected.retain(|id| listed.contains(id));
        page.sessions.retain(|id, _| listed.contains(id));
        let unknown: Vec<(u64, Vec<(PaneKind, String)>)> =
            items.iter().filter(|item| !page.sessions.contains_key(&item.trash_id)).map(|item| (item.trash_id, item.sessions())).collect();
        page.items = items;
        if unknown.is_empty() {
            return;
        }
        for (id, sessions) in &unknown {
            page.sessions.insert(*id, sessions.iter().map(|(_, session)| session.clone()).collect());
        }
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_spawn(async move {
                    unknown
                        .into_iter()
                        .map(|(id, sessions)| (id, sessions.iter().map(|(kind, session)| session_location(*kind, session)).collect()))
                        .collect::<Vec<(u64, Vec<String>)>>()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                for (id, locations) in found {
                    if let Some(known) = this.trash.sessions.get_mut(&id) {
                        *known = locations;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Brings entries back into this window, asleep, as they were saved.
    fn restore_trashed(&mut self, ids: &[u64], cx: &mut Context<Self>) {
        let path = TrashFile::path();
        let mut file = match TrashFile::read(&path) {
            Ok(file) => file,
            Err(err) => return self.show_toast_for(tf(cx, "trash.move_failed", &[("error", &err.to_string())]), 8000, cx),
        };
        let open_plugins: HashSet<String> = self.workspaces.iter().filter_map(|ws| ws.plugin.clone()).collect();
        let waiting = waiting_for_their_plugin(&file.workspaces, ids, open_plugins);
        let restored = file.take(|item| ids.contains(&item.trash_id) && !waiting.contains(&item.trash_id));
        let none_before = self.workspaces.is_empty();
        let count = restored.len();
        for item in restored {
            let mut snapshot = item.snapshot;
            // Its own id when free: workspaces listed under it find it again, and so does its parent
            // link (a parent not in this window is drawn as none).
            let id = if self.workspaces.iter().any(|ws| ws.id == snapshot.id) { self.next_id() } else { snapshot.id };
            self.next_id = self.next_id.max(id + 1);
            let parked = super::persist::take_old_todo_tabs(&mut snapshot);
            snapshot.id = id;
            snapshot.group = item.group_name.and_then(|name| self.groups.iter().find(|g| g.name == name)).map(|g| g.id);
            snapshot.marks.asleep = true;
            self.workspaces.push(Workspace {
                id,
                name: snapshot.name.clone(),
                group: snapshot.group,
                cwd: snapshot.cwd.clone(),
                tabs: Vec::new(),
                active_tab: 0,
                closed_tabs: snapshot.closed_tabs.clone(),
                color: super::stored_color(snapshot.color_value, snapshot.color),
                plugin: snapshot.plugin.clone(),
                remote_hidden: snapshot.remote_hidden,
                marks: snapshot.marks.clone(),
                parked,
                dormant: Some(snapshot),
                asleep_on_close: false,
            });
            if let Some(sync_id) = item.sync_id {
                self.sync_workspace_restored(id, sync_id, cx);
            }
            self.bring_back_orchestrations(item.orchestrations);
            self.board_rebind_tickets(&item.tickets, id);
        }
        // It was the only one: what is in front must be awake once the page closes.
        if none_before && count > 0 {
            self.select_workspace_for_page(0, cx);
        }
        self.trash.loaded = false;
        // Saved in the list before it leaves the trash: a crash between the two, or a failed save,
        // leaves it in both places, which a delete can tidy, and never in neither.
        self.persist(cx);
        if let Err(err) = file.save(&path) {
            return self.show_toast_for(tf(cx, "trash.move_failed", &[("error", &format!("{err:#}"))]), 8000, cx);
        }
        let skipped = ids.iter().filter(|id| waiting.contains(id)).count();
        let key = if skipped > 0 { "trash.restored_some" } else { "trash.restored" };
        self.show_toast(tf(cx, key, &[("n", &count.to_string()), ("skipped", &skipped.to_string())]), cx);
        cx.notify();
    }

    /// Working trees something still works in: panes of this window, and every workspace saved in
    /// any window (a sleeping one has no panes, but it comes back to its folder).
    /// So do `waiting`: entries left in the trash, which come back to theirs if restored.
    fn trees_still_used(&self, waiting: &[TrashedWorkspace], cx: &gpui::App) -> HashSet<PathBuf> {
        fn panes(node: &NodeSnapshot, out: &mut Vec<PathBuf>) {
            match node {
                NodeSnapshot::Pane(pane) => out.push(pane.cwd.clone()),
                NodeSnapshot::Split { children, .. } => children.iter().for_each(|child| panes(child, out)),
            }
        }
        let mut folders: Vec<PathBuf> = Vec::new();
        let mut note = |ws: &WorkspaceSnapshot| {
            folders.push(ws.cwd.clone());
            ws.tabs.iter().chain(&ws.parked).for_each(|tab| panes(&tab.layout, &mut folders));
        };
        self.workspaces.iter().filter_map(|ws| ws.dormant.as_ref()).for_each(&mut note);
        waiting.iter().map(|item| &item.snapshot).for_each(&mut note);
        for slot in LayoutState::saved_window_slots().into_iter().filter(|slot| *slot != self.slot) {
            LayoutState::load(slot).workspaces.iter().for_each(&mut note);
        }
        folders.extend(self.workspaces.iter().map(|ws| ws.cwd.clone()));
        let mut used = self.trees_in_use(cx);
        used.extend(
            folders.iter().filter_map(|folder| agentty_bridge::worktree::tree_root(folder)).map(|root| root.canonicalize().unwrap_or(root)),
        );
        used
    }

    /// Deletes entries for good: out of the file first, then their worktrees (git keeps what would
    /// be lost, and one something still works in stays) and their sync data if that was asked for.
    fn purge_trashed(&mut self, which: impl Fn(&TrashedWorkspace) -> bool, cx: &mut Context<Self>) -> usize {
        let path = TrashFile::path();
        let mut file = match TrashFile::read(&path) {
            Ok(file) => file,
            Err(err) => {
                self.show_toast_for(tf(cx, "trash.move_failed", &[("error", &err.to_string())]), 8000, cx);
                return 0;
            }
        };
        let gone = file.take(which);
        if gone.is_empty() {
            return 0;
        }
        for item in &gone {
            if let (true, Some(sync_id)) = (item.purge_sync, &item.sync_id) {
                file.pending_sync_purges.push(PendingSyncPurge { plugin: item.snapshot.plugin.clone(), sync_id: sync_id.clone() });
            }
        }
        if let Err(err) = file.save(&path) {
            self.show_toast_for(tf(cx, "trash.move_failed", &[("error", &format!("{err:#}"))]), 8000, cx);
            return 0;
        }
        let mut trees: Vec<PathBuf> = gone.iter().flat_map(|item| item.trees.iter().cloned()).collect();
        trees.sort();
        trees.dedup();
        // Removed meanwhile (Monitoring → Worktrees, or by hand): nothing left to do for it.
        trees.retain(|tree| tree.is_dir());
        if !trees.is_empty() {
            let used = self.trees_still_used(&file.workspaces, cx);
            trees.retain(|tree| !used.contains(&tree.canonicalize().unwrap_or_else(|_| tree.clone())));
        }
        self.hand_over_sync_purges(cx);
        self.trash.loaded = false;
        let count = gone.len();
        self.show_toast(tf(cx, "trash.purged", &[("n", &count.to_string())]), cx);
        if !trees.is_empty() {
            // Its toast says what went and what stayed, in place of the one above.
            self.remove_trees_after_close(trees, cx);
        }
        cx.notify();
        count
    }

    /// Sync data waiting to leave the repository goes to the sync, once it is able to take it.
    fn hand_over_sync_purges(&mut self, cx: &mut Context<Self>) {
        if !self.sync_ready() {
            return;
        }
        let path = TrashFile::path();
        let Ok(mut file) = TrashFile::read(&path) else { return };
        if file.pending_sync_purges.is_empty() {
            return;
        }
        let purges = std::mem::take(&mut file.pending_sync_purges);
        if file.save(&path).is_ok() {
            for purge in purges {
                self.sync_purge_trashed(purge.plugin, purge.sync_id, cx);
            }
        }
    }

    /// Checks for entries past their date soon after the start, then every hour, in every window:
    /// the file decides, so a second window finds them gone.
    pub(super) fn start_trash_expiry(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(60)).await;
            loop {
                let open = this.update(cx, |this, cx| {
                    let (now, days) = (now_secs(), retention(cx));
                    this.purge_trashed(|item| item.expiry(days) <= now, cx);
                    this.hand_over_sync_purges(cx);
                });
                if open.is_err() {
                    break;
                }
                cx.background_executor().timer(Duration::from_secs(60 * 60)).await;
            }
        })
        .detach();
    }

    /// The page as the debug probe reports it.
    pub(super) fn trash_debug_state(&self) -> serde_json::Value {
        serde_json::json!({
            "items": self.trash.items.iter().map(|item| &item.title).collect::<Vec<_>>(),
            "selected": self.trash.selected.len(),
            "confirm": self.trash.confirm.as_ref().map(Vec::len),
            "sessions": self.trash.sessions.values().flatten().collect::<Vec<_>>(),
        })
    }

    /// `trash-restore <n>` / `trash-purge <n>` (debug driver), the n-th entry on the page.
    pub(super) fn debug_trash(&mut self, command: &str, argument: &str, cx: &mut Context<Self>) {
        self.refresh_trash(cx);
        let Some(id) = argument.trim().parse::<usize>().ok().and_then(|i| self.trash.items.get(i)).map(|item| item.trash_id) else {
            return;
        };
        match command {
            "trash-restore" => self.restore_trashed(&[id], cx),
            _ => {
                self.purge_trashed(|item| item.trash_id == id, cx);
            }
        }
    }

    pub(super) fn render_trash_page(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        self.refresh_trash(cx);
        let scroll = self.trash.scroll.clone();
        let total = self.trash.items.len();
        let selected = self.trash.selected.len();
        let all: Vec<u64> = self.trash.items.iter().map(|item| item.trash_id).collect();
        let every_one = total > 0 && selected == total;

        let header = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_3()
            .child(div().t_heading().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(t(cx, "page.trash")))
            .child(div().flex_1())
            .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "trash.retention")))
            .child(retention_stepper(cx));

        let toolbar = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(crate::ui::action_button(
                "trash-select-all",
                t(cx, if every_one { "trees.clear_selection" } else { "trash.select_all" }),
                cx.listener({
                    let all = all.clone();
                    move |this, _: &ClickEvent, _, cx| {
                        if this.trash.selected.len() == all.len() {
                            this.trash.selected.clear();
                        } else {
                            this.trash.selected = all.iter().copied().collect();
                        }
                        cx.notify();
                    }
                }),
            ))
            .child(div().flex_1())
            .child(
                Self::trash_button(
                    "trash-restore-selected",
                    "rotate-cw",
                    tf(cx, "trash.restore_selected", &[("n", &selected.to_string())]),
                    false,
                    selected > 0,
                )
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    let ids: Vec<u64> = this.trash.selected.iter().copied().collect();
                    if !ids.is_empty() {
                        this.restore_trashed(&ids, cx);
                    }
                })),
            )
            .child(
                Self::trash_button(
                    "trash-delete-selected",
                    "trash-2",
                    tf(cx, "trash.delete_selected", &[("n", &selected.to_string())]),
                    true,
                    selected > 0,
                )
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    let ids: Vec<u64> = this.trash.selected.iter().copied().collect();
                    if !ids.is_empty() {
                        this.trash.confirm = Some(ids);
                        cx.notify();
                    }
                })),
            )
            .child(Self::trash_button("trash-delete-all", "trash-2", t(cx, "trash.delete_all").to_string(), true, total > 0).on_click(
                cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if !all.is_empty() {
                        this.trash.confirm = Some(all.clone());
                        cx.notify();
                    }
                }),
            ));

        let mut list = card().flex().flex_col().overflow_hidden();
        if total == 0 {
            list = list.child(crate::ui::hint(t(cx, "trash.empty")).px_4().py_4());
        }
        let items = self.trash.items.clone();
        for item in &items {
            list = list.child(self.render_trash_row(item, cx));
        }

        let confirm = self.render_trash_confirm(cx);
        div()
            .size_full()
            .relative()
            .child(
                div()
                    .id("trash-page")
                    .size_full()
                    .bg(hex(Chrome::EDITOR))
                    .overflow_y_scroll()
                    .track_scroll(&scroll)
                    .child(div().px_6().py_5().flex().flex_col().gap_4().child(header).child(toolbar).child(list)),
            )
            .group(crate::ui::SCROLL_GROUP)
            .child(crate::ui::scrollbar(scroll))
            .children(confirm)
    }

    /// A toolbar button, greyed out while it has nothing to act on.
    fn trash_button(id: &'static str, glyph: &'static str, label: String, danger: bool, enabled: bool) -> gpui::Stateful<gpui::Div> {
        let color = if enabled { Chrome::BRIGHT } else { Chrome::MUTED };
        div()
            .id(id)
            .flex_shrink_0()
            .px_2p5()
            .py_1()
            .rounded_md()
            .t_small()
            .flex()
            .items_center()
            .gap_1p5()
            .bg(hex(if danger && enabled { Chrome::ERROR } else { 0x2d2d30 }))
            .text_color(hex(color))
            .when(enabled, |d| d.cursor_pointer().hover(|s| s.opacity(0.85)))
            .child(icon(glyph, IconSize::INLINE, hex(color)))
            .child(label)
    }

    fn render_trash_row(&self, item: &TrashedWorkspace, cx: &mut Context<Self>) -> AnyElement {
        let id = item.trash_id;
        let checked = self.trash.selected.contains(&id);
        let expiry = item.expiry(retention(cx));
        let left = days_left(expiry, now_secs());
        let detail = |label: &str, value: String| {
            div()
                .flex()
                .items_baseline()
                .gap_2()
                .min_w_0()
                .child(div().w(px(112.)).flex_shrink_0().t_caption().text_color(hex(Chrome::MUTED)).child(label.to_string()))
                .child(div().flex_1().min_w_0().t_small().truncate().text_color(hex(Chrome::FOREGROUND)).child(value))
        };
        let mut details = div()
            .flex()
            .flex_col()
            .gap_0p5()
            .min_w_0()
            .child(detail(t(cx, "trash.deleted_at"), date_text(item.deleted_at)))
            .child(detail(t(cx, "trash.expires_at"), date_text(expiry)))
            .child(detail(t(cx, "trash.workspace_path"), crate::ui::tilde(&item.folder)));
        for location in self.trash.sessions.get(&id).into_iter().flatten() {
            details = details.child(detail(t(cx, "trash.session_path"), location.clone()));
        }
        for tree in &item.trees {
            details = details.child(detail(t(cx, "trash.tree_to_remove"), crate::ui::tilde(tree)));
        }
        div()
            .id(SharedString::from(format!("trash-row-{id}")))
            .px_4()
            .py_3()
            .flex()
            .items_start()
            .gap_3()
            .border_b_1()
            .border_color(hex(0x2a2a2a))
            .cursor_pointer()
            .when(checked, |d| d.bg(hex_alpha(Chrome::ACCENT, 0.10)))
            .hover(|s| s.bg(hex(Chrome::HOVER)))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                if !this.trash.selected.remove(&id) {
                    this.trash.selected.insert(id);
                }
                cx.notify();
            }))
            .child(check_box(checked, true).mt(px(3.)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .min_w_0()
                            .child(div().t_body().truncate().text_color(hex(Chrome::BRIGHT)).child(item.title.clone()))
                            .child(badge(
                                tf(cx, "trash.days_left", &[("n", &left.to_string())]),
                                if left <= 1 { Chrome::WARNING } else { Chrome::MUTED },
                            )),
                    )
                    .child(details),
            )
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap_1()
                    .child(crate::ui::action_button(
                        SharedString::from(format!("trash-restore-{id}")),
                        t(cx, "trash.restore"),
                        cx.listener(move |this, _: &ClickEvent, _, cx| {
                            cx.stop_propagation();
                            this.restore_trashed(&[id], cx);
                        }),
                    ))
                    .child(
                        crate::ui::icon_only(
                            SharedString::from(format!("trash-delete-{id}")),
                            "trash-2",
                            cx.listener(move |this, _: &ClickEvent, _, cx| {
                                cx.stop_propagation();
                                this.trash.confirm = Some(vec![id]);
                                cx.notify();
                            }),
                        )
                        .tooltip(crate::ui::Tooltip::text(t(cx, "trash.delete_now"), None)),
                    ),
            )
            .into_any_element()
    }

    fn render_trash_confirm(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let ids = self.trash.confirm.as_ref()?;
        let chosen: Vec<&TrashedWorkspace> = self.trash.items.iter().filter(|item| ids.contains(&item.trash_id)).collect();
        let names = chosen.iter().map(|item| item.title.as_str()).collect::<Vec<_>>().join(", ");
        let trees = chosen.iter().map(|item| item.trees.len()).sum::<usize>();
        let mut body = vec![
            div()
                .t_body()
                .text_color(hex(Chrome::FOREGROUND))
                .child(tf(cx, "trash.confirm_body", &[("n", &chosen.len().to_string())]))
                .into_any_element(),
            div().t_caption().text_color(hex(Chrome::MUTED)).max_h(px(72.)).overflow_hidden().child(names).into_any_element(),
        ];
        if trees > 0 {
            body.push(
                div()
                    .t_caption()
                    .text_color(hex(Chrome::WARNING))
                    .child(tf(cx, "trash.confirm_trees", &[("n", &trees.to_string())]))
                    .into_any_element(),
            );
        }
        body.push(div().t_caption().text_color(hex(Chrome::MUTED)).child(t(cx, "trash.confirm_sessions")).into_any_element());
        Some(confirm_dialog(
            "trash-confirm",
            t(cx, "trash.confirm_title").to_string(),
            body,
            t(cx, "trash.confirm_ok").to_string(),
            cx.listener(|this, _: &ClickEvent, _, cx| {
                this.trash.confirm = None;
                cx.notify();
            }),
            cx.listener(|this, _: &ClickEvent, _, cx| {
                if let Some(ids) = this.trash.confirm.take() {
                    this.purge_trashed(|item| ids.contains(&item.trash_id), cx);
                }
            }),
            t(cx, "confirm.cancel").to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(kind: PaneKind, session: Option<&str>) -> NodeSnapshot {
        NodeSnapshot::Pane(PaneSnapshot {
            kind,
            cwd: PathBuf::from("/tmp/project"),
            title: String::new(),
            session_id: session.map(str::to_string),
            tool: None,
            chat_view: None,
        })
    }

    fn entry(trash_id: u64, layout: NodeSnapshot) -> TrashedWorkspace {
        let snapshot: WorkspaceSnapshot = serde_json::from_value(serde_json::json!({
            "id": 1, "cwd": "/tmp/project", "tabs": [{ "layout": serde_json::to_value(&layout).unwrap() }],
        }))
        .unwrap();
        TrashedWorkspace {
            trash_id,
            title: "project".into(),
            folder: "/tmp/project".into(),
            deleted_at: 1_000_000,
            expires_at: 1_000_000 + 7 * DAY_SECS,
            snapshot,
            group_name: None,
            trees: Vec::new(),
            sync_id: None,
            purge_sync: false,
            orchestrations: Vec::new(),
            tickets: Vec::new(),
        }
    }

    /// The one copy kept aside next to `path`.
    fn aside(path: &Path) -> Vec<u8> {
        let copy = std::fs::read_dir(path.parent().unwrap()).unwrap().flatten().find(|e| e.file_name() != "trash.json").unwrap();
        std::fs::read(copy.path()).unwrap()
    }

    fn temp_file(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("agentty-trash-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("trash.json")
    }

    #[test]
    fn a_longer_period_extends_and_a_shorter_one_deletes_nothing_early() {
        let item = entry(1, pane(PaneKind::Shell, None));
        assert_eq!(item.expiry(7), item.deleted_at + 7 * DAY_SECS);
        assert_eq!(item.expiry(30), item.deleted_at + 30 * DAY_SECS);
        assert_eq!(item.expiry(1), item.deleted_at + 7 * DAY_SECS);
    }

    #[test]
    fn the_stepper_walks_the_choices_from_any_value() {
        assert_eq!(stepped_retention(7, true), 14);
        assert_eq!(stepped_retention(7, false), 3);
        assert_eq!(stepped_retention(10, true), 14);
        assert_eq!(stepped_retention(10, false), 7);
        assert_eq!(stepped_retention(1, false), 1);
        assert_eq!(stepped_retention(365, true), 365);
    }

    #[test]
    fn what_is_left_of_a_day_counts_as_a_day() {
        assert_eq!(days_left(1000 + 7 * DAY_SECS, 1000), 7);
        assert_eq!(days_left(1000 + 1, 1000), 1);
        assert_eq!(days_left(1000, 1000), 0);
        assert_eq!(days_left(500, 1000), 0);
    }

    #[test]
    fn sessions_come_from_every_split_once() {
        let layout = NodeSnapshot::Split {
            axis: crate::workbench::panes::Axis::Horizontal,
            sizes: vec![0.5, 0.5],
            children: vec![
                pane(PaneKind::Claude, Some("a")),
                pane(PaneKind::Shell, None),
                pane(PaneKind::Claude, Some("a")),
                pane(PaneKind::Codex, Some("b")),
            ],
        };
        let sessions = entry(1, layout).sessions();
        assert_eq!(sessions, vec![(PaneKind::Claude, "a".to_string()), (PaneKind::Codex, "b".to_string())]);
    }

    #[test]
    fn the_file_keeps_entries_and_restores_them_whole() {
        let path = temp_file("round-trip");
        let mut item = entry(5, pane(PaneKind::Claude, Some("a")));
        item.trees = vec!["/tmp/tree".into()];
        item.sync_id = Some("sync".into());
        item.purge_sync = true;
        TrashFile { workspaces: vec![item], ..Default::default() }.save(&path).unwrap();
        let read = TrashFile::load(&path);
        assert_eq!(read.workspaces.len(), 1);
        let read = &read.workspaces[0];
        assert_eq!((read.trash_id, read.purge_sync), (5, true));
        assert_eq!(read.trees, [PathBuf::from("/tmp/tree")]);
        assert_eq!(read.sessions(), vec![(PaneKind::Claude, "a".to_string())]);
    }

    #[test]
    fn take_removes_only_what_it_picks() {
        let mut file = TrashFile {
            workspaces: vec![entry(1, pane(PaneKind::Shell, None)), entry(2, pane(PaneKind::Shell, None))],
            ..Default::default()
        };
        let taken = file.take(|item| item.trash_id == 1);
        assert_eq!(taken.iter().map(|i| i.trash_id).collect::<Vec<_>>(), [1]);
        assert_eq!(file.workspaces.iter().map(|i| i.trash_id).collect::<Vec<_>>(), [2]);
    }

    #[test]
    fn an_unreadable_file_or_entry_is_copied_aside_before_it_can_be_overwritten() {
        let path = temp_file("damaged");
        std::fs::write(&path, b"{ not json").unwrap();
        assert!(TrashFile::load(&path).workspaces.is_empty());
        assert_eq!(aside(&path), b"{ not json");

        let path = temp_file("one-bad-entry");
        let good = serde_json::to_value(entry(1, pane(PaneKind::Shell, None))).unwrap();
        let bytes = serde_json::to_vec(&serde_json::json!({ "workspaces": [good, { "from": "the future" }] })).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        assert_eq!(TrashFile::load(&path).workspaces.len(), 1);
        assert_eq!(aside(&path), bytes);
    }

    #[test]
    fn a_plugin_gets_one_workspace_back() {
        let of = |id, plugin: Option<&str>| {
            let mut item = entry(id, pane(PaneKind::Shell, None));
            item.snapshot.plugin = plugin.map(str::to_string);
            item
        };
        let items = [of(1, Some("a")), of(2, Some("a")), of(3, Some("b")), of(4, None), of(5, None)];
        let open = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<HashSet<_>>();
        assert_eq!(waiting_for_their_plugin(&items, &[1, 2, 3, 4, 5], open(&[])), [2], "two of one plugin: the first comes back");
        assert_eq!(waiting_for_their_plugin(&items, &[1, 3], open(&["a"])), [1], "its plugin has one open");
        assert_eq!(waiting_for_their_plugin(&items, &[3, 4, 5], open(&["a"])), Vec::<u64>::new(), "not asked for: not counted");
    }

    #[test]
    #[cfg(unix)]
    fn a_file_that_cannot_be_read_now_is_an_error_not_an_empty_trash() {
        // A folder in the file's place reads as an error for every user, root included.
        let path = temp_file("no-read");
        std::fs::create_dir_all(&path).unwrap();
        assert!(TrashFile::read(&path).is_err());
        assert!(TrashFile::load(&path).workspaces.is_empty());
    }

    #[test]
    fn a_damaged_file_is_copied_once_and_earlier_copies_stay() {
        let path = temp_file("copies");
        let copies = || std::fs::read_dir(path.parent().unwrap()).unwrap().flatten().filter(|e| e.file_name() != "trash.json").count();
        std::fs::write(&path, b"{ one").unwrap();
        TrashFile::load(&path);
        TrashFile::load(&path);
        assert_eq!(copies(), 1, "the same file again makes no second copy");
        std::fs::write(&path, b"{ two").unwrap();
        TrashFile::load(&path);
        assert_eq!(copies(), 2, "another damaged file does not replace the first copy");
    }

    #[test]
    fn sync_purges_waiting_survive_a_read_and_a_save() {
        let path = temp_file("pending");
        let purge = PendingSyncPurge { plugin: None, sync_id: "abc".into() };
        TrashFile { pending_sync_purges: vec![purge.clone()], ..Default::default() }.save(&path).unwrap();
        let read = TrashFile::read(&path).unwrap();
        assert_eq!(read.pending_sync_purges, [purge]);
        read.save(&path).unwrap();
        assert_eq!(TrashFile::read(&path).unwrap().pending_sync_purges.len(), 1);
    }

    #[test]
    fn a_missing_file_is_an_empty_trash() {
        let path = temp_file("missing");
        assert!(TrashFile::load(&path).workspaces.is_empty());
        assert_eq!(std::fs::read_dir(path.parent().unwrap()).unwrap().count(), 0, "no copy of nothing");
    }
}
