//! Moving a workspace, or a whole group, to another Agentty window.
//!
//! What runs keeps running: the terminals are handed over as they are (their processes, their
//! agent sessions, the account each one started on), only the window drawing them changes. What
//! belongs to the window that drew them stays there (split sizes, the in-app browser's pages, an
//! orchestration's links), and the workspace takes the other window's group of the same name.

use super::{Pane, Workbench, Workspace};
use crate::i18n::{t, tf};
use gpui::{App, Context, EntityId, Window};
use std::collections::HashMap;

/// Another open window a workspace can go to, as its menu lists it.
#[derive(Clone)]
pub(crate) struct WindowTarget {
    pub slot: usize,
    pub name: String,
}

/// A workspace on its way from one window to another.
pub(crate) struct MovedWorkspace {
    workspace: Workspace,
    /// Its group, by name and colour: groups have ids of their own in each window.
    group: Option<(String, Option<u32>)>,
    /// Panes shown as a chat or as their terminal by choice.
    chat_modes: Vec<(EntityId, bool)>,
    /// Chat tabs: their lead and how the chat was set up.
    chats: Vec<(Pane, super::chat_team::ChatRoles)>,
    tickets: Vec<u64>,
    /// Its id in the sync repository, when both windows sync to the same one.
    sync_id: Option<String>,
}

/// Moves workspaces `ids` of window `from` to window `to` (both by slot). Runs outside any window
/// update, so both can be changed; nothing happens when either window is gone. `group`: the group
/// that was moved as a whole, which goes from `from` with its last workspace.
pub(crate) fn move_workspaces(from: usize, ids: Vec<u64>, group: Option<u64>, to: usize, cx: &mut App) {
    let windows = crate::workbenches(cx);
    let find = |slot: usize| windows.iter().find(|w| w.read(cx).is_ok_and(|wb| wb.slot == slot && !wb.closed)).copied();
    let (Some(source), Some(target)) = (find(from), find(to)) else { return };
    let Ok(profile) = target.read(cx).map(|wb| wb.sync_profile().clone()) else { return };
    let Ok(moved) = source.update(cx, |wb, window, cx| wb.take_workspaces(&ids, group, &profile, window, cx)) else { return };
    if moved.is_empty() {
        return;
    }
    let received = target.update(cx, |wb, window, cx| {
        wb.receive_workspaces(moved, window, cx);
        window.activate_window();
    });
    if received.is_err() {
        // The window went away meanwhile; nothing is lost, the terminals are dropped with it.
        eprintln!("agentty: window {to} closed while workspaces were moved to it");
    }
    cx.activate(true);
}

impl Workbench {
    /// The other windows open now, for the menus that send a workspace to one.
    pub(super) fn refresh_window_targets(&mut self, cx: &App) {
        let me = self.window_id;
        self.window_targets = crate::workbenches(cx)
            .into_iter()
            .filter(|handle| handle.window_id() != me)
            .filter_map(|handle| {
                let wb = handle.read(cx).ok().filter(|wb| !wb.closed)?;
                Some(WindowTarget { slot: wb.slot, name: wb.window_name(cx) })
            })
            .collect();
    }

    /// How this window is named in another window's menu: its number, then its title without the
    /// app's name (two windows may show workspaces of the same name).
    fn window_name(&self, cx: &App) -> String {
        let number = tf(cx, "window.numbered", &[("n", &(self.slot + 1).to_string())]);
        match self.native_title.strip_suffix(" — Agentty").map(str::trim) {
            Some(name) if !name.is_empty() => format!("{number} · {name}"),
            _ => number,
        }
    }

    /// Sends group `group`, with all its workspaces, to window `slot`.
    pub(super) fn send_group_to_window(&mut self, group: u64, slot: usize, cx: &mut Context<Self>) {
        let ids = self.workspaces.iter().filter(|w| w.group == Some(group)).map(|w| w.id).collect();
        self.send_to_window_as(ids, Some(group), slot, cx);
    }

    /// Sends workspaces to window `slot`.
    pub(super) fn send_to_window(&mut self, ids: Vec<u64>, slot: usize, cx: &mut Context<Self>) {
        self.send_to_window_as(ids, None, slot, cx);
    }

    /// Sends workspaces (of `group` when it goes as a whole) to window `slot`. A plugin's workspace
    /// stays: its panel and browser pages run in this window.
    fn send_to_window_as(&mut self, ids: Vec<u64>, group: Option<u64>, slot: usize, cx: &mut Context<Self>) {
        self.workspace_menu = None;
        self.workspace_submenu = None;
        self.group_menu = None;
        let movable: Vec<u64> =
            ids.iter().copied().filter(|id| self.workspaces.iter().any(|ws| ws.id == *id && ws.plugin.is_none())).collect();
        if movable.len() < ids.len() {
            self.show_toast(t(cx, "window.move_plugin_stays"), cx);
        }
        if movable.is_empty() {
            cx.notify();
            return;
        }
        let from = self.slot;
        cx.defer(move |cx| move_workspaces(from, movable, group, slot, cx));
        cx.notify();
    }

    /// Takes workspaces out of this window, their terminals still running, for `profile`'s window.
    fn take_workspaces(
        &mut self,
        ids: &[u64],
        group: Option<u64>,
        profile: &agentty_bridge::sync::SyncProfile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<MovedWorkspace> {
        let active_id = self.workspaces.get(self.active_workspace).map(|w| w.id);
        let same_repository = same_sync_repository(self.sync_profile(), profile);
        let mut moved = Vec::new();
        for &id in ids {
            let Some(index) = self.workspaces.iter().position(|w| w.id == id && w.plugin.is_none()) else { continue };
            let ws = &self.workspaces[index];
            let panes: Vec<Pane> = ws.tabs.iter().flat_map(|t| t.root.leaves()).collect();
            let chats = ws
                .tabs
                .iter()
                .filter_map(|tab| self.chat_lead_of_tab(tab))
                .map(|lead| {
                    let roles = self.chat_roles(&lead).unwrap_or_default();
                    (lead, roles)
                })
                .collect();
            let chat_modes = panes.iter().filter_map(|p| self.chat_modes.get(&p.entity_id()).map(|on| (p.entity_id(), *on))).collect();
            let group = ws.group.and_then(|g| self.groups.iter().find(|x| x.id == g)).map(|g| (g.name.clone(), g.color));
            let tickets = self.board_tickets_of(id);
            // Another account's window syncs to another repository: there it starts afresh, and
            // here it ends as a removed one would.
            let sync_id = if same_repository {
                let sync_id = self.workspace_sync_id(id);
                self.forget_sync_of(id);
                sync_id
            } else {
                self.sync_workspace_removed(id, cx);
                None
            };
            for pane in &panes {
                self.let_go_of_pane(pane, cx);
            }
            let Some(index) = self.workspaces.iter().position(|w| w.id == id) else { continue };
            let workspace = self.workspaces.remove(index);
            self.board_workspace_closed(id);
            moved.push(MovedWorkspace { workspace, group, chat_modes, chats, tickets, sync_id });
        }
        if moved.is_empty() {
            return moved;
        }
        // A group sent as a whole goes along, unless something stayed in it (a plugin's workspace).
        if let Some(group) = group.filter(|g| !self.workspaces.iter().any(|w| w.group == Some(*g))) {
            self.groups.retain(|g| g.id != group);
        }
        // Whatever was in front stays in front; when it was one that left, the one before it.
        match active_id.and_then(|id| self.workspaces.iter().position(|w| w.id == id)) {
            Some(index) => self.active_workspace = index,
            None => {
                self.active_workspace = self.active_workspace.min(self.workspaces.len().saturating_sub(1));
                if !self.workspaces.is_empty() {
                    self.select_workspace(self.active_workspace, cx);
                }
            }
        }
        self.persist(cx);
        self.focus_active(window, cx);
        cx.notify();
        moved
    }

    /// Drops what this window keeps about a pane that goes on in another window. Unlike closing it,
    /// nothing is stopped: its servers and its process keep running.
    fn let_go_of_pane(&mut self, pane: &Pane, cx: &mut Context<Self>) {
        let entity = pane.entity_id();
        let pane_id = pane.read(cx).pane_id;
        self.pane_subscriptions.remove(&entity);
        self.pane_bounds.borrow_mut().remove(&entity);
        self.footer_modes.borrow_mut().remove(&entity);
        if self.zoomed.as_ref() == Some(pane) {
            self.zoomed = None;
        }
        if self.status_menu_pane.as_ref() == Some(pane) {
            self.status_menu_pane = None;
        }
        if self.agent_panel.as_ref().is_some_and(|p| p.pane == entity) {
            self.agent_panel = None;
        }
        if self.popped_out.as_ref().is_some_and(|p| p.pane == entity) {
            self.close_popped_out(cx);
        }
        if self.connect_pick == Some(pane_id) {
            self.connect_pick = None;
        }
        self.chats.remove(&entity);
        self.chat_modes.remove(&entity);
        self.plugin_panes.remove(&pane_id);
        self.plugin_launched.remove(&pane_id);
        self.servers.listeners.remove(&pane_id);
        self.notices.retain(|n| n.pane_id != pane_id);
        self.flow_forget_pane(pane_id, cx);
    }

    /// Takes in workspaces from another window and brings the first awake one to the front: a
    /// sleeping one stays asleep, as it was in the window it came from.
    fn receive_workspaces(&mut self, moved: Vec<MovedWorkspace>, window: &mut Window, cx: &mut Context<Self>) {
        self.welcome = false;
        // Ids are each window's own: every workspace gets one here, and a parent that came along
        // is found again under its new id.
        let renumbered: HashMap<u64, u64> = moved.iter().map(|m| (m.workspace.id, self.next_id())).collect();
        let first = self.workspaces.len();
        for item in moved {
            let MovedWorkspace { mut workspace, group, chat_modes, chats, tickets, sync_id } = item;
            let id = renumbered[&workspace.id];
            workspace.id = id;
            workspace.group = group.map(|(name, color)| self.group_named(name, color));
            workspace.marks.parent = moved_parent(workspace.marks.parent, &renumbered);
            if let Some(dormant) = workspace.dormant.as_mut() {
                dormant.id = id;
                dormant.group = workspace.group;
                dormant.marks.parent = workspace.marks.parent;
            }
            for pane in workspace.tabs.iter().flat_map(|t| t.root.leaves()) {
                self.subscribe_pane(&pane, cx);
            }
            self.chat_modes.extend(chat_modes);
            self.workspaces.push(workspace);
            for (lead, roles) in chats {
                self.register_chat(&lead, roles, cx);
            }
            self.board_rebind_tickets(&tickets, id);
            if let Some(sync_id) = sync_id {
                self.sync_workspace_restored(id, sync_id, cx);
            }
        }
        if let Some(index) = (first..self.workspaces.len()).find(|&i| self.workspaces[i].dormant.is_none()) {
            self.activate_workspace(index, window, cx);
        }
        self.persist(cx);
        cx.notify();
    }

    /// The group called `name` here, made (with `color`) when there is none.
    fn group_named(&mut self, name: String, color: Option<u32>) -> u64 {
        let next_id = &mut self.next_id;
        group_named_in(&mut self.groups, name, color, || {
            let id = *next_id;
            *next_id += 1;
            id
        })
    }
}

/// Whether two windows sync to the same repository, so a workspace keeps its sync id between them.
fn same_sync_repository(from: &agentty_bridge::sync::SyncProfile, to: &agentty_bridge::sync::SyncProfile) -> bool {
    from == to
}

/// A moved workspace's parent in its new window: the parent's new id when it came along, none
/// when it stayed behind (ids are each window's own, so the old one would name a stranger).
fn moved_parent(parent: Option<u64>, renumbered: &HashMap<u64, u64>) -> Option<u64> {
    parent.and_then(|parent| renumbered.get(&parent).copied())
}

/// The id of the group called `name` in `groups`, made with `color` and the id `next` gives when
/// there is none.
fn group_named_in(groups: &mut Vec<super::Group>, name: String, color: Option<u32>, next: impl FnOnce() -> u64) -> u64 {
    if let Some(group) = groups.iter().find(|g| g.name == name) {
        return group.id;
    }
    let id = next();
    groups.push(super::Group { id, name, collapsed: false, color });
    id
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentty_bridge::agent_auth::AuthAgent;
    use agentty_bridge::sync::SyncProfile;

    #[test]
    fn a_workspace_keeps_its_sync_id_only_within_one_account() {
        let own = SyncProfile::default();
        let first = SyncProfile::of_account(AuthAgent::Claude, "example-account-1");
        let second = SyncProfile::of_account(AuthAgent::Claude, "example-account-2");
        let codex = SyncProfile::of_account(AuthAgent::Codex, "example-account-1");
        // Two windows on the same login (a second window of the main account) share a repository.
        assert!(same_sync_repository(&own, &SyncProfile::default()));
        assert!(same_sync_repository(&first, &SyncProfile::of_account(AuthAgent::Claude, "example-account-1")));
        // Another account's window has a repository of its own.
        assert!(!same_sync_repository(&own, &first));
        assert!(!same_sync_repository(&first, &second));
        assert!(!same_sync_repository(&first, &codex));
    }

    #[test]
    fn a_parent_is_found_again_only_when_it_came_along() {
        let renumbered = HashMap::from([(10, 3), (11, 4)]);
        assert_eq!(moved_parent(Some(10), &renumbered), Some(3));
        // Its parent stayed in the other window: no parent here, not whichever workspace has id 12.
        assert_eq!(moved_parent(Some(12), &renumbered), None);
        assert_eq!(moved_parent(None, &renumbered), None);
    }

    #[test]
    fn a_group_is_joined_by_name_or_made_with_its_colour() {
        let mut groups = vec![super::super::Group { id: 7, name: "Team".into(), collapsed: true, color: None }];
        // One of the same name is joined, whatever its colour; no new id is taken.
        assert_eq!(group_named_in(&mut groups, "Team".into(), Some(0xff0000), || panic!("no id needed")), 7);
        assert_eq!(groups.len(), 1);
        // Another is made, with the colour it had.
        assert_eq!(group_named_in(&mut groups, "Review".into(), Some(0x00ff00), || 9), 9);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[1].name, "Review");
        assert_eq!(groups[1].color, Some(0x00ff00));
        assert!(!groups[1].collapsed);
    }
}
