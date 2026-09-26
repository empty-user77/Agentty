//! Session sync in the app: when workspaces sync, the icon left of the notifications in the title
//! bar and its popover, and Settings → Sync. The work itself is `agentty_bridge::sync`.
//!
//! A workspace syncs only while none of its agents is in a turn, and never continuously:
//! - a turn finished (debounced, so panes finishing together sync once);
//! - every five minutes while nothing happens;
//! - "Sync now";
//! - a tab closed or the workspace removed (its final state is recorded).

use super::settings_page::{row_with_hint, section};
use super::{PaneKind, Workbench, Workspace};
use crate::i18n::{t, tf};
use crate::launch::LaunchSpec;
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{action_button, chip, icon, now_ms, popover, IconSize, TypeScale};
use agentty_bridge::model::Agent;
use agentty_bridge::sync::model::{ParentRef, ProjectRef, TreeNode, WorkspaceState};
use agentty_bridge::sync::repo::{GithubRepo, Remote};
use agentty_bridge::sync::restore::{ContextRequest, RestoreOutcome, RestoreRequest};
use agentty_bridge::sync::{self as bridge, Connected, Overview, SessionInput, SyncConfig, SyncError, SyncRequest, WorkspaceInput};
use gpui::{div, prelude::*, px, ClickEvent, Context, Div, Entity, SharedString, Subscription, Task, Window};
use std::collections::HashSet;
use std::time::Duration;

/// Turns that finish close together sync once.
const DEBOUNCE: Duration = Duration::from_secs(20);
/// How long nothing must happen before the periodic sync, and how often it runs.
const IDLE_MS: u64 = 5 * 60 * 1000;
const TICK: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Status {
    /// No repository connected.
    Off,
    Paused,
    /// Nothing synced yet in this run.
    Ready,
    Synced,
    Syncing,
    /// The repository became public: sync stopped until it is private again.
    Public,
    Error(String),
}

/// Which way the repository is chosen in settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Method {
    #[default]
    Github,
    Url,
}

pub(super) struct SyncUi {
    config: SyncConfig,
    status: Status,
    running: bool,
    /// A trigger arrived while a sync was running: run again right after.
    again: Option<Scope>,
    /// Workspaces skipped because an agent was working; they sync on the next check.
    pending: HashSet<u64>,
    pub(super) popover_open: bool,
    overview: Overview,
    last_auto_ms: u64,
    debounce: Option<Task<()>>,
    /// Workspaces waiting to be synced by the debounced trigger (`None`: all).
    soon: Option<Scope>,
    /// Final states of removed workspaces, sent with the next sync.
    removed: Vec<WorkspaceInput>,
    purge: Vec<(Option<String>, String)>,
    // Settings → Sync.
    method: Method,
    gh_login: Option<Option<String>>,
    repos: Vec<GithubRepo>,
    looking_up: bool,
    url_input: Option<Entity<TextInput>>,
    new_repo_input: Option<Entity<TextInput>>,
    device_input: Option<(Entity<TextInput>, Subscription)>,
    connecting: bool,
    message: Option<(String, bool)>,
    started: bool,
    /// Synced sessions that have a transcript on this computer (`None` until looked up).
    local_ids: Option<HashSet<String>>,
}

impl Default for SyncUi {
    fn default() -> Self {
        let config = bridge::load_config();
        let status = if !config.connected() {
            Status::Off
        } else if config.paused {
            Status::Paused
        } else {
            Status::Ready
        };
        Self {
            overview: if config.connected() { bridge::overview() } else { Overview::default() },
            config,
            status,
            running: false,
            again: None,
            pending: HashSet::new(),
            popover_open: false,
            last_auto_ms: now_ms(),
            debounce: None,
            soon: None,
            removed: Vec::new(),
            purge: Vec::new(),
            method: Method::default(),
            gh_login: None,
            repos: Vec::new(),
            looking_up: false,
            url_input: None,
            new_repo_input: None,
            device_input: None,
            connecting: false,
            message: None,
            started: false,
            local_ids: None,
        }
    }
}

/// A session in the sync repository, as the lists show it.
#[derive(Debug, Clone)]
pub struct SyncedSession {
    plugin: Option<String>,
    sync_id: String,
    device: String,
    device_name: String,
    agent: Agent,
    id: String,
    title: String,
    updated_at: String,
    /// Open in a tab on that device when it last synced.
    open: bool,
    /// The session it was continued from.
    parent: Option<ParentRef>,
    workspace_name: Option<String>,
    /// The workspace's folder and project on that device.
    cwd: String,
    project: ProjectRef,
}

/// Which workspaces a sync covers.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Scope {
    All,
    Some(HashSet<u64>),
}

impl Scope {
    fn merge(self, other: Scope) -> Scope {
        match (self, other) {
            (Scope::Some(mut a), Scope::Some(b)) => {
                a.extend(b);
                Scope::Some(a)
            }
            _ => Scope::All,
        }
    }

    fn covers(&self, id: u64) -> bool {
        match self {
            Scope::All => true,
            Scope::Some(ids) => ids.contains(&id),
        }
    }
}

impl SyncUi {
    fn active(&self) -> bool {
        self.config.connected() && !self.config.paused && self.status != Status::Public
    }

    pub(super) fn debug_state(&self) -> serde_json::Value {
        serde_json::json!({
            "status": format!("{:?}", self.status),
            "running": self.running,
            "pending": self.pending.len(),
            "connected": self.config.remote.as_ref().map(|r| r.label()),
            "workspaces": self.overview.workspaces.len(),
            "devices": self.overview.devices.len(),
            "connecting": self.connecting,
            "message": self.message.as_ref().map(|(text, error)| serde_json::json!({ "text": text, "error": error })),
        })
    }
}

impl Workbench {
    fn sync_key(&self, id: u64) -> String {
        format!("{}:{id}", self.slot)
    }

    /// Starts the periodic check (once per window).
    pub(super) fn start_sync(&mut self, cx: &mut Context<Self>) {
        if std::mem::replace(&mut self.sync.started, true) {
            return;
        }
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(TICK).await;
            if this.update(cx, |this, cx| this.sync_tick(cx)).is_err() {
                break;
            }
        })
        .detach();
    }

    fn sync_tick(&mut self, cx: &mut Context<Self>) {
        if !self.sync.active() || self.sync.running {
            return;
        }
        let now = now_ms();
        let quiet_since = self.all_panes().iter().map(|p| p.read(cx).last_activity_ms).max().unwrap_or(0);
        if now.saturating_sub(quiet_since) >= IDLE_MS && now.saturating_sub(self.sync.last_auto_ms) >= IDLE_MS {
            self.sync.last_auto_ms = now;
            return self.run_sync(Scope::All, cx);
        }
        // Workspaces that were busy at their trigger go as soon as they are idle.
        if !self.sync.pending.is_empty() {
            let ready: HashSet<u64> = self.sync.pending.iter().copied().filter(|id| !self.workspace_busy(*id, cx)).collect();
            if !ready.is_empty() {
                self.run_sync(Scope::Some(ready), cx);
            }
        }
    }

    /// A turn finished in `pane_id`: its workspace syncs in a moment.
    pub(super) fn sync_after_turn(&mut self, pane_id: u64, cx: &mut Context<Self>) {
        let Some(ws) =
            self.workspaces.iter().find(|ws| ws.tabs.iter().any(|t| t.root.leaves().iter().any(|p| p.read(cx).pane_id == pane_id)))
        else {
            return;
        };
        let id = ws.id;
        self.sync_soon(Scope::Some(HashSet::from([id])), cx);
    }

    /// A tab closed: the workspace records which sessions were closed.
    pub(super) fn sync_after_close(&mut self, workspace_id: u64, cx: &mut Context<Self>) {
        if self.sync.config.workspaces.contains_key(&self.sync_key(workspace_id)) {
            self.sync_soon(Scope::Some(HashSet::from([workspace_id])), cx);
        }
    }

    fn sync_soon(&mut self, scope: Scope, cx: &mut Context<Self>) {
        if !self.sync.active() {
            return;
        }
        self.sync.soon = Some(match self.sync.soon.take() {
            Some(previous) => previous.merge(scope),
            None => scope,
        });
        self.sync.debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DEBOUNCE).await;
            let _ = this.update(cx, |this, cx| {
                if let Some(scope) = this.sync.soon.take() {
                    this.run_sync(scope, cx);
                }
            });
        }));
    }

    /// "Sync now".
    pub(super) fn sync_now(&mut self, cx: &mut Context<Self>) {
        if !self.sync.config.connected() {
            return;
        }
        if self.sync.status == Status::Public {
            // Asked again by the user: check whether it is private now.
            self.sync.status = Status::Ready;
        }
        self.run_sync(Scope::All, cx);
    }

    fn workspace_busy(&self, id: u64, cx: &gpui::App) -> bool {
        self.workspaces
            .iter()
            .filter(|ws| ws.id == id)
            .flat_map(|ws| ws.tabs.iter().flat_map(|t| t.root.leaves()))
            .any(|p| p.read(cx).status.in_turn())
    }

    /// Sessions of a workspace: from its panes, or from its saved layout while it sleeps.
    fn sync_sessions(&self, ws: &Workspace, open: bool, cx: &gpui::App) -> Vec<SessionInput> {
        let panes: Vec<super::Pane> = ws.tabs.iter().flat_map(|t| t.root.leaves()).collect();
        let mut snapshots: Vec<super::persist::PaneSnapshot> = panes.iter().map(|pane| Self::snapshot_pane(pane, cx)).collect();
        if let Some(dormant) = &ws.dormant {
            snapshots.extend(dormant.tabs.iter().flat_map(|t| super::snapshot_panes(&t.layout)).cloned());
        }
        let mut sessions: Vec<SessionInput> = Vec::new();
        for pane in snapshots {
            let agent = match pane.kind {
                PaneKind::Claude => Agent::Claude,
                PaneKind::Codex => Agent::Codex,
                PaneKind::Shell => continue,
            };
            let Some(id) = pane.session_id.filter(|id| !id.is_empty()) else { continue };
            if sessions.iter().any(|s| s.id == id) {
                continue;
            }
            sessions.push(SessionInput { agent, id, title: pane.title, open, cwd: Some(pane.cwd), parent: None, started_ms: None });
        }
        // Gemini CLI and Kimi CLI run as commands in a shell pane: their session is found in their
        // files when the sync runs (the newest one in the pane's folder since it started).
        for pane in &panes {
            let view = pane.read(cx);
            let agent = match view.tool_id() {
                "gemini" => Agent::Gemini,
                "kimi" => Agent::Kimi,
                _ => continue,
            };
            sessions.push(SessionInput {
                agent,
                id: String::new(),
                title: view.display_title(),
                open,
                cwd: Some(view.display_cwd()),
                parent: None,
                started_ms: Some(view.launched_at_ms),
            });
        }
        sessions
    }

    fn sync_input(&self, ws: &Workspace, state: WorkspaceState, cx: &gpui::App) -> WorkspaceInput {
        let local_key = self.sync_key(ws.id);
        WorkspaceInput {
            sync_id: self.sync.config.workspaces.get(&local_key).cloned(),
            local_key,
            plugin: ws.plugin.clone(),
            name: Some(self.workspace_title(ws, cx)),
            group: ws.group.and_then(|g| self.groups.iter().find(|group| group.id == g)).map(|g| g.name.clone()),
            color: ws.color,
            cwd: ws.cwd.clone(),
            state,
            sessions: self.sync_sessions(ws, state != WorkspaceState::Deleted, cx),
        }
    }

    /// The workspace is about to be removed: its final state goes with the next sync, and with
    /// `purge` its data leaves the repository.
    pub(super) fn sync_workspace_removed(&mut self, id: u64, purge: bool, cx: &mut Context<Self>) {
        let key = self.sync_key(id);
        let Some(sync_id) = self.sync.config.workspaces.get(&key).cloned() else { return };
        let Some(ws) = self.workspaces.iter().find(|w| w.id == id) else { return };
        if purge {
            self.sync.purge.push((ws.plugin.clone(), sync_id));
        } else {
            let input = WorkspaceInput { sync_id: Some(sync_id), ..self.sync_input(ws, WorkspaceState::Deleted, cx) };
            self.sync.removed.push(input);
        }
        self.sync.pending.remove(&id);
        let _ = bridge::update_config(|c| {
            c.workspaces.remove(&key);
        });
        self.sync.config.workspaces.remove(&key);
        self.sync_soon(Scope::Some(HashSet::new()), cx);
    }

    /// Whether a workspace has data in the sync repository (the delete dialog asks about it).
    pub(super) fn workspace_synced(&self, id: u64) -> bool {
        self.sync.config.connected() && self.sync.config.workspaces.contains_key(&self.sync_key(id))
    }

    fn build_request(&mut self, scope: &Scope, cx: &gpui::App) -> SyncRequest {
        let mut request = SyncRequest { app_version: env!("CARGO_PKG_VERSION").to_string(), ..Default::default() };
        let mut chosen: Vec<&Workspace> = Vec::new();
        for ws in &self.workspaces {
            if !scope.covers(ws.id) {
                continue;
            }
            if self.workspace_busy(ws.id, cx) {
                self.sync.pending.insert(ws.id);
                continue;
            }
            let key = self.sync_key(ws.id);
            // Only workspaces with an agent session, or synced before (its sessions may all be closed now).
            if !self.sync.config.workspaces.contains_key(&key) && self.sync_sessions(ws, true, cx).is_empty() {
                continue;
            }
            chosen.push(ws);
        }
        // A workspace synced for the first time gets its id in the sync itself, which first looks
        // for the same project synced from another computer.
        for ws in chosen {
            self.sync.pending.remove(&ws.id);
            let open = !ws.tabs.is_empty() || ws.dormant.is_some();
            let state = if open { WorkspaceState::Open } else { WorkspaceState::Closed };
            request.workspaces.push(self.sync_input(ws, state, cx));
        }
        request.workspaces.append(&mut self.sync.removed);
        request.purge.append(&mut self.sync.purge);
        request
    }

    fn run_sync(&mut self, scope: Scope, cx: &mut Context<Self>) {
        if !self.sync.config.connected() || self.sync.config.paused {
            return;
        }
        if self.sync.running {
            self.sync.again = Some(match self.sync.again.take() {
                Some(previous) => previous.merge(scope),
                None => scope,
            });
            return;
        }
        let request = self.build_request(&scope, cx);
        // A scoped trigger whose workspaces are all busy waits for them; "all" still refreshes the list.
        if request.workspaces.is_empty() && request.purge.is_empty() && scope != Scope::All {
            return;
        }
        self.sync.running = true;
        self.sync.status = Status::Syncing;
        cx.notify();
        let retry =
            (request.workspaces.iter().filter(|w| w.state == WorkspaceState::Deleted).cloned().collect::<Vec<_>>(), request.purge.clone());
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { bridge::sync(&request) }).await;
            let _ = this.update(cx, |this, cx| {
                this.sync.running = false;
                match result {
                    Ok(outcome) => {
                        this.sync.overview = outcome.overview;
                        // With the ids handed out in this pass.
                        this.sync.config = bridge::load_config();
                        this.sync.status = Status::Synced;
                        if this.session_filter == super::SessionFilter::Synced {
                            this.refresh_synced_local(cx);
                        }
                    }
                    Err(err) => {
                        // What could not be recorded goes with the next attempt.
                        let (removed, purge) = retry;
                        this.sync.removed.extend(removed);
                        this.sync.purge.extend(purge);
                        this.sync.status = match err {
                            SyncError::Public => Status::Public,
                            SyncError::NotConnected => Status::Off,
                            SyncError::Busy => Status::Ready,
                            other => Status::Error(sync_error_text(&other, cx)),
                        };
                    }
                }
                if let Some(scope) = this.sync.again.take() {
                    this.run_sync(scope, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Looks up which synced sessions this computer has (the agents' folders), in the background.
    pub(super) fn refresh_synced_local(&mut self, cx: &mut Context<Self>) {
        let wanted: Vec<(Agent, String)> = self.synced_sessions(None).into_iter().map(|s| (s.agent, s.id)).collect();
        cx.spawn(async move |this, cx| {
            let found = cx.background_spawn(async move { bridge::present_locally(&wanted) }).await;
            let _ = this.update(cx, |this, cx| {
                this.sync.local_ids = Some(found);
                cx.notify();
            });
        })
        .detach();
    }

    /// Synced sessions this computer does not have, and that no pane here runs.
    fn missing_sessions(&self, cx: &gpui::App) -> Vec<SyncedSession> {
        let Some(local) = &self.sync.local_ids else { return Vec::new() };
        let mut seen = HashSet::new();
        self.synced_sessions(None)
            .into_iter()
            .filter(|s| !local.contains(&s.id) && self.pane_for_session(&s.id, cx).is_none())
            // One row per session: the newest copy, when several computers have it.
            .filter(|s| seen.insert(s.id.clone()))
            .take(100)
            .collect()
    }

    /// A newer continuation of session `id` on another computer: the same session gone further
    /// there, or a branch of it, changed after this computer's copy last did.
    pub(super) fn newer_elsewhere(&self, id: &str) -> Option<SyncedSession> {
        if !self.sync.config.connected() {
            return None;
        }
        let me = &self.sync.config.device_id;
        let all = self.synced_sessions(None);
        let mine = all.iter().find(|s| &s.device == me && s.id == id).map(|s| s.updated_at.clone())?;
        all.into_iter()
            .filter(|s| &s.device != me)
            .filter(|s| s.id == id || s.parent.as_ref().is_some_and(|p| p.session == id))
            .filter(|s| s.updated_at > mine)
            .max_by(|a, b| a.updated_at.cmp(&b.updated_at))
    }

    /// Asks, before a session is resumed, whether to take the newer copy from another computer.
    /// `false` when there is none (resume as usual).
    pub(super) fn ask_newer_elsewhere(&mut self, session: &agentty_bridge::model::SessionInfo, cx: &mut Context<Self>) -> bool {
        let Some(newer) = self.newer_elsewhere(&session.id) else { return false };
        let when = agentty_bridge::sync::model::stamp_to_ms(&newer.updated_at)
            .map(|ms| super::tree_manager::ago(cx, (ms / 1000) as i64))
            .unwrap_or_default();
        self.ask(
            super::ask::Ask {
                title: t(cx, "sync.newer_title").into(),
                body: Some(tf(cx, "sync.newer_body", &[("device", &newer.device_name), ("ago", &when)]).into()),
                choices: vec![
                    super::ask::AskChoice {
                        label: t(cx, "sync.newer_use_latest").into(),
                        action: super::ask::AskAction::ContinueSynced(Box::new(newer)),
                        primary: true,
                        danger: false,
                    },
                    super::ask::AskChoice {
                        label: t(cx, "sync.newer_use_local").into(),
                        action: super::ask::AskAction::Resume { session: session.clone(), compact: false },
                        primary: false,
                        danger: false,
                    },
                ],
                cancel: true,
            },
            cx,
        );
        true
    }

    /// Context merge: the synced session's conversation goes to the agent in front as a prompt
    /// that points at a handoff document. Only typed in, never sent: the user reads it and presses
    /// Enter.
    fn share_synced_context(&mut self, entry: SyncedSession, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pane) = self.active_pane() else { return };
        let (kind, busy) = {
            let view = pane.read(cx);
            (view.agent_kind(), view.status.in_turn())
        };
        let to = match kind {
            Some(PaneKind::Claude) => Agent::Claude,
            Some(PaneKind::Codex) => Agent::Codex,
            _ => return self.show_toast(t(cx, "sync.context_no_agent"), cx),
        };
        if busy {
            return self.show_toast(t(cx, "sync.context_busy"), cx);
        }
        self.sync.popover_open = false;
        let request = ContextRequest {
            plugin: entry.plugin,
            sync_id: entry.sync_id,
            device: entry.device,
            agent: entry.agent,
            session: entry.id,
            to,
        };
        cx.spawn_in(window, async move |this, cx| {
            let result = cx.background_spawn(async move { agentty_bridge::sync::restore::share_context(&request) }).await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(prompt) => {
                    pane.update(cx, |view, _| view.insert_text(&prompt));
                    this.focus_pane(&pane, window, cx);
                    this.show_toast_for(t(cx, "sync.context_ready"), 6000, cx);
                }
                Err(err) => {
                    let text = tf(cx, "sync.context_failed", &[("reason", &sync_error_text(&err, cx))]);
                    this.show_toast_for(text, 8000, cx);
                }
            });
        })
        .detach();
    }

    pub(super) fn sync_connected(&self) -> bool {
        self.sync.config.connected()
    }

    /// Every synced session of one workspace (or all), newest first.
    fn synced_sessions(&self, only: Option<&str>) -> Vec<SyncedSession> {
        let mut out = Vec::new();
        for meta in &self.sync.overview.workspaces {
            if only.is_some_and(|id| id != meta.sync_id) {
                continue;
            }
            let name = meta.latest().and_then(|(_, s)| s.name.clone());
            for (device, section, session) in meta.sessions() {
                out.push(SyncedSession {
                    plugin: meta.plugin.clone(),
                    sync_id: meta.sync_id.clone(),
                    device: device.clone(),
                    device_name: section.device_name.clone(),
                    agent: session.agent,
                    id: session.id.clone(),
                    title: session.title.clone(),
                    updated_at: session.updated_at.clone(),
                    open: session.open,
                    parent: session.parent.clone(),
                    workspace_name: name.clone(),
                    cwd: section.cwd.clone(),
                    project: section.project.clone(),
                });
            }
        }
        out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        out
    }

    /// The local workspace of this window synced under `sync_id`.
    fn workspace_for_sync_id(&self, sync_id: &str) -> Option<u64> {
        let prefix = format!("{}:", self.slot);
        self.sync
            .config
            .workspaces
            .iter()
            .filter(|(key, id)| key.starts_with(&prefix) && id.as_str() == sync_id)
            .filter_map(|(key, _)| key[prefix.len()..].parse::<u64>().ok())
            .find(|id| self.workspaces.iter().any(|w| w.id == *id))
    }

    /// Continues a synced session: brought onto this computer if needed, then resumed in a new tab
    /// of `target` — or of the workspace it belongs to here, or of a new one that syncs into it.
    pub(super) fn continue_synced(&mut self, entry: SyncedSession, target: Option<u64>, window: &mut Window, cx: &mut Context<Self>) {
        let mine = entry.device == self.sync.config.device_id;
        if mine && self.jump_to_session(&entry.id, window, cx) {
            self.sync.popover_open = false;
            return;
        }
        match entry.agent {
            // Amp keeps its threads on its own servers: any computer signed in resumes them as they are.
            Agent::Amp => {
                self.sync.popover_open = false;
                let cwd = target
                    .and_then(|id| self.workspaces.iter().find(|w| w.id == id))
                    .map(|w| w.cwd.clone())
                    .unwrap_or_else(crate::launch::home_dir);
                return self.open_tab(LaunchSpec::resume(entry.agent, entry.id, entry.title, cwd), window, cx);
            }
            agent if !bridge::syncs_files(agent) => {
                return self.show_toast(tf(cx, "sync.not_synced_agent", &[("agent", agent.display_name())]), cx);
            }
            _ => {}
        }
        let target = target.or_else(|| self.workspace_for_sync_id(&entry.sync_id));
        let workspace_cwd = match target.and_then(|id| self.workspaces.iter().find(|w| w.id == id)) {
            Some(ws) => ws.cwd.clone(),
            None => {
                let mut candidates = self.known_project_folders(cx);
                candidates.push(crate::launch::home_dir());
                agentty_bridge::sync::restore::project_folder(&entry.project, &entry.cwd, &candidates)
                    .unwrap_or_else(crate::launch::home_dir)
            }
        };
        let request = RestoreRequest {
            plugin: entry.plugin.clone(),
            sync_id: entry.sync_id.clone(),
            device: entry.device.clone(),
            agent: entry.agent,
            session: entry.id.clone(),
            workspace_cwd,
            keep_local: self.pane_for_session(&entry.id, cx).is_some(),
        };
        self.sync.popover_open = false;
        self.show_toast(t(cx, "sync.restoring"), cx);
        cx.spawn_in(window, async move |this, cx| {
            let result = cx.background_spawn(async move { agentty_bridge::sync::restore::restore(&request) }).await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(restored) => {
                    let spec = LaunchSpec::resume(restored.agent, restored.id.clone(), restored.title.clone(), restored.cwd.clone());
                    match target.and_then(|id| this.workspaces.iter().position(|w| w.id == id)) {
                        Some(index) => {
                            this.activate_workspace(index, window, cx);
                            this.open_tab(spec, window, cx);
                        }
                        None => {
                            this.create_workspace(spec, window, cx);
                            // The new workspace syncs into the one the session came from.
                            if let Some(ws) = this.workspaces.last_mut() {
                                ws.name = entry.workspace_name.clone().or(Some(restored.title.clone()));
                                let key = format!("{}:{}", this.slot, ws.id);
                                if entry.plugin.is_none() {
                                    let sync_id = entry.sync_id.clone();
                                    let _ = bridge::update_config(move |c| {
                                        c.workspaces.insert(key, sync_id);
                                    });
                                    this.sync.config = bridge::load_config();
                                }
                            }
                            this.persist(cx);
                        }
                    }
                    let note = match restored.outcome {
                        RestoreOutcome::Branched => t(cx, "sync.restored_branch"),
                        RestoreOutcome::Updated => t(cx, "sync.restored_updated"),
                        RestoreOutcome::Downloaded => t(cx, "sync.restored"),
                        RestoreOutcome::UpToDate => t(cx, "sync.restored_local"),
                    };
                    this.show_toast(note, cx);
                    this.refresh_sessions(cx);
                    this.refresh_synced_local(cx);
                }
                Err(err) => {
                    let text = tf(cx, "sync.restore_failed", &[("reason", &sync_error_text(&err, cx))]);
                    this.show_toast_for(text, 8000, cx);
                }
            });
        })
        .detach();
    }

    /// Session history → "Synced": sessions of the sync repository this computer does not have.
    pub(super) fn render_synced_sessions(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let missing = self.missing_sessions(cx);
        let mut list = div().id("synced-sessions").flex().flex_col().flex_1().min_h_0().overflow_y_scroll().px_2();
        if self.sync.local_ids.is_none() {
            list = list.child(crate::ui::hint(t(cx, "sync.looking_up_local")));
        } else if missing.is_empty() {
            list = list.child(crate::ui::hint(t(cx, "sync.no_missing")));
        }
        for (index, entry) in missing.into_iter().enumerate() {
            let when = agentty_bridge::sync::model::stamp_to_ms(&entry.updated_at)
                .map(|ms| super::tree_manager::ago(cx, (ms / 1000) as i64))
                .unwrap_or_default();
            let place = [entry.workspace_name.clone().unwrap_or_default(), entry.device_name.clone(), when]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" · ");
            let title = entry.title.clone();
            let agent = entry.agent;
            list = list.child(
                div()
                    .id(("synced-session", index))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .mb_1()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .tooltip(crate::ui::Tooltip::text(t(cx, "sync.continue_hint"), None))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.continue_synced(entry.clone(), None, window, cx)))
                    .child(crate::brand::avatar_colored(agent.id(), 16.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().t_body().truncate().text_color(hex(Chrome::FOREGROUND)).child(title))
                            .child(div().t_caption().truncate().text_color(hex(Chrome::MUTED)).child(place)),
                    )
                    .child(icon("cloud", IconSize::INLINE, hex(Chrome::MUTED))),
            );
        }
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(div().px_3().pb_2().t_caption().text_color(hex(Chrome::MUTED)).child(t(cx, "sync.missing_hint")))
            .child(list)
    }

    // -- title bar -------------------------------------------------------------------------------

    fn sync_look(&self) -> (&'static str, u32) {
        match &self.sync.status {
            Status::Off | Status::Paused => ("cloud-off", Chrome::MUTED),
            Status::Syncing => ("cloud-upload", Chrome::ACCENT),
            Status::Public => ("cloud-alert", Chrome::ERROR),
            Status::Error(_) => ("cloud-alert", Chrome::WARNING),
            Status::Ready | Status::Synced if !self.sync.pending.is_empty() => ("cloud-upload", Chrome::WARNING),
            Status::Ready => ("cloud", Chrome::MUTED),
            Status::Synced => ("cloud-check", Chrome::MUTED),
        }
    }

    fn sync_status_text(&self, cx: &gpui::App) -> String {
        match &self.sync.status {
            Status::Off => t(cx, "sync.status.off").to_string(),
            Status::Paused => t(cx, "sync.status.paused").to_string(),
            Status::Syncing => t(cx, "sync.status.syncing").to_string(),
            Status::Public => t(cx, "sync.status.public").to_string(),
            Status::Error(reason) => tf(cx, "sync.status.error", &[("reason", reason)]),
            Status::Ready | Status::Synced if !self.sync.pending.is_empty() => {
                tf(cx, "sync.status.pending", &[("n", &self.sync.pending.len().to_string())])
            }
            Status::Ready | Status::Synced => match self.last_sync_ago(cx) {
                Some(ago) => tf(cx, "sync.status.synced", &[("ago", &ago)]),
                None => t(cx, "sync.status.ready").to_string(),
            },
        }
    }

    fn last_sync_ago(&self, cx: &gpui::App) -> Option<String> {
        let at = self.sync.config.last_sync_at.as_deref().and_then(agentty_bridge::sync::model::stamp_to_ms)?;
        Some(super::tree_manager::ago(cx, (at / 1000) as i64))
    }

    /// The sync icon left of the notifications.
    pub(super) fn render_sync_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (glyph, color) = self.sync_look();
        let tooltip = self.sync_status_text(cx);
        let pending = self.sync.pending.len();
        div()
            .id("status-sync")
            .size(px(crate::ui::ICON_BUTTON))
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .flex_shrink_0()
            .cursor_pointer()
            .when(self.sync.popover_open, |d| d.bg(hex(Chrome::SELECTED)))
            .when(matches!(self.sync.status, Status::Public), |d| d.bg(hex_alpha(Chrome::ERROR, 0.18)))
            .hover(|s| s.bg(hex(Chrome::HOVER)))
            .tooltip(crate::ui::Tooltip::text(SharedString::from(tooltip), None))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                if this.just_dismissed("sync") {
                    return;
                }
                this.sync.popover_open = !this.sync.popover_open;
                this.notices_open = false;
                this.launcher_open = false;
                cx.notify();
            }))
            .child(icon(glyph, IconSize::BUTTON, hex(color)))
            .when(pending > 0 && !self.sync.running, |d| {
                d.child(div().ml_0p5().t_caption().text_color(hex(Chrome::WARNING)).child(pending.to_string()))
            })
    }

    /// This window's workspace in front, with its id in the repository.
    fn front_sync_workspace(&self) -> Option<(&Workspace, Option<String>)> {
        let ws = self.workspaces.get(self.active_workspace)?;
        Some((ws, self.sync.config.workspaces.get(&self.sync_key(ws.id)).cloned()))
    }

    pub(super) fn render_sync_popover(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let connected = self.sync.config.connected();
        let mut body = div().flex().flex_col().gap_2().p_3();

        let status_color = match self.sync.status {
            Status::Public => Chrome::ERROR,
            Status::Error(_) => Chrome::WARNING,
            _ => Chrome::FOREGROUND,
        };
        body = body.child(div().t_small().text_color(hex(status_color)).child(self.sync_status_text(cx)));
        if self.sync.status == Status::Public {
            body = body.child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "sync.public_hint")));
        }

        if connected {
            // The sessions of the workspace in front, from every device.
            if let Some((ws, sync_id)) = self.front_sync_workspace() {
                let title = tf(cx, "sync.this_workspace", &[("name", &self.workspace_title(ws, cx))]);
                let mut list = div().flex().flex_col().gap_1();
                let open_here: Vec<String> = self.sync_sessions(ws, true, cx).into_iter().map(|s| s.id).collect();
                let target = ws.id;
                let flat = sync_id.as_deref().map(|id| self.synced_sessions(Some(id))).unwrap_or_default();
                // Branches under the session they came from.
                let nodes: Vec<TreeNode> = flat
                    .iter()
                    .map(|s| TreeNode { device: &s.device, id: &s.id, parent: s.parent.as_ref(), updated_at: &s.updated_at })
                    .collect();
                let entries: Vec<(SyncedSession, usize)> =
                    agentty_bridge::sync::model::tree_order(&nodes).into_iter().map(|(i, depth)| (flat[i].clone(), depth)).collect();
                // A session running here that went further on another computer.
                let newer: Option<SyncedSession> = open_here.iter().find_map(|id| self.newer_elsewhere(id));
                if entries.is_empty() {
                    list = list.child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "sync.no_sessions")));
                }
                if let Some(newer) = newer {
                    let when = agentty_bridge::sync::model::stamp_to_ms(&newer.updated_at)
                        .map(|ms| super::tree_manager::ago(cx, (ms / 1000) as i64))
                        .unwrap_or_default();
                    let latest = newer.clone();
                    list = list.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_2()
                            .py_1p5()
                            .rounded_md()
                            .bg(hex_alpha(Chrome::WARNING, 0.12))
                            .child(icon("cloud-alert", IconSize::INLINE, hex(Chrome::WARNING)))
                            .child(div().flex_1().min_w_0().t_caption().text_color(hex(Chrome::FOREGROUND)).child(tf(
                                cx,
                                "sync.newer_here",
                                &[("device", &newer.device_name), ("ago", &when)],
                            )))
                            .child(action_button(
                                "sync-get-latest",
                                t(cx, "sync.get_latest"),
                                cx.listener(move |this, _: &ClickEvent, window, cx| {
                                    this.continue_synced(latest.clone(), Some(target), window, cx)
                                }),
                            )),
                    );
                }
                for (index, (entry, depth)) in entries.into_iter().take(16).enumerate() {
                    let here = entry.device == self.sync.config.device_id;
                    // Only this computer's copy runs here; the same session from another one is a different copy.
                    let in_view = here && open_here.contains(&entry.id);
                    let when = agentty_bridge::sync::model::stamp_to_ms(&entry.updated_at)
                        .map(|ms| super::tree_manager::ago(cx, (ms / 1000) as i64))
                        .unwrap_or_default();
                    let device_label = if here { t(cx, "sync.this_computer").to_string() } else { entry.device_name.clone() };
                    let (title, agent, open_there) = (entry.title.clone(), entry.agent, entry.open && !here);
                    let context = entry.clone();
                    list = list.child(
                        div()
                            .id(("sync-session", index))
                            .flex()
                            .items_center()
                            .gap_2()
                            .pr_2()
                            .pl(px(8. + 14. * depth.min(4) as f32))
                            .when(depth > 0, |d| d.child(div().t_caption().text_color(hex(Chrome::MUTED)).child("↳")))
                            .py_1()
                            .rounded_md()
                            .when(in_view, |d| d.bg(hex_alpha(Chrome::ACCENT, 0.15)))
                            .child(crate::brand::avatar(agent.id(), 14.))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(div().t_small().truncate().text_color(hex(Chrome::BRIGHT)).child(title))
                                    .child(
                                        div()
                                            .t_caption()
                                            .truncate()
                                            .text_color(hex(Chrome::MUTED))
                                            .child(format!("{device_label} · {when}")),
                                    ),
                            )
                            .when(in_view, |d| {
                                d.child(div().flex_shrink_0().t_caption().text_color(hex(Chrome::ACCENT)).child(t(cx, "sync.in_terminal")))
                            })
                            .when(!in_view, |d| {
                                d.when(open_there, |d| {
                                    d.child(
                                        div().flex_shrink_0().t_caption().text_color(hex(Chrome::MUTED)).child(t(cx, "sync.open_there")),
                                    )
                                })
                                .child(
                                    action_button(
                                        ("sync-context", index),
                                        t(cx, "sync.context"),
                                        cx.listener(move |this, _: &ClickEvent, window, cx| {
                                            this.share_synced_context(context.clone(), window, cx)
                                        }),
                                    )
                                    .tooltip(crate::ui::Tooltip::text(t(cx, "sync.context_hint"), None)),
                                )
                                .child(
                                    action_button(
                                        ("sync-continue", index),
                                        t(cx, "sync.continue"),
                                        cx.listener(move |this, _: &ClickEvent, window, cx| {
                                            this.continue_synced(entry.clone(), Some(target), window, cx)
                                        }),
                                    )
                                    .tooltip(crate::ui::Tooltip::text(t(cx, "sync.continue_hint"), None)),
                                )
                            }),
                    );
                }
                body = body.child(div().pt_1().t_caption().text_color(hex(Chrome::MUTED)).child(title)).child(list);
            }

            // Devices.
            if !self.sync.overview.devices.is_empty() {
                let mut devices = div().flex().flex_col().gap_0p5();
                for device in &self.sync.overview.devices {
                    let here = device.id == self.sync.config.device_id;
                    let when = agentty_bridge::sync::model::stamp_to_ms(&device.last_sync_at)
                        .map(|ms| super::tree_manager::ago(cx, (ms / 1000) as i64))
                        .unwrap_or_default();
                    devices = devices.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .t_small()
                            .child(icon(if here { "monitor" } else { "laptop" }, IconSize::INLINE, hex(Chrome::MUTED)))
                            .child(div().flex_1().min_w_0().truncate().child(if here {
                                format!("{} ({})", device.name, t(cx, "sync.this_computer"))
                            } else {
                                device.name.clone()
                            }))
                            .child(div().flex_shrink_0().text_color(hex(Chrome::MUTED)).child(when)),
                    );
                }
                body = body.child(div().pt_1().t_caption().text_color(hex(Chrome::MUTED)).child(t(cx, "sync.devices"))).child(devices);
            }
        } else {
            body = body.child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "sync.off_hint")));
        }

        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(hex(Chrome::OVERLAY_BORDER))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().t_body().font_weight(crate::theme::EMPHASIS).child(t(cx, "sync.title")))
                    .children(
                        self.sync
                            .config
                            .remote
                            .as_ref()
                            .map(|r| div().t_caption().truncate().text_color(hex(Chrome::MUTED)).child(r.label())),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .gap_1()
                    .when(connected, |d| {
                        d.child(
                            action_button("sync-now", t(cx, "sync.now"), cx.listener(|this, _: &ClickEvent, _, cx| this.sync_now(cx)))
                                .when(self.sync.running || self.sync.config.paused, |b| b.opacity(0.5)),
                        )
                    })
                    .child(action_button(
                        "sync-settings",
                        t(cx, "sync.settings"),
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.sync.popover_open = false;
                            this.settings_section = super::settings_page::SettingsSection::Sync;
                            this.page = Some(super::Page::Settings);
                            cx.notify();
                        }),
                    )),
            );

        popover()
            .id("sync-popover")
            .w(px(360.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                if std::mem::take(&mut this.sync.popover_open) {
                    this.note_dismissed("sync");
                }
                cx.notify();
            }))
            .child(header)
            .child(body)
    }

    // -- Settings → Sync -------------------------------------------------------------------------

    fn look_up_github(&mut self, cx: &mut Context<Self>) {
        if self.sync.looking_up || self.sync.gh_login.is_some() {
            return;
        }
        self.sync.looking_up = true;
        cx.spawn(async move |this, cx| {
            let (login, repos) = cx
                .background_spawn(async move {
                    let login = agentty_bridge::sync::repo::gh_login();
                    let repos = if login.is_some() { agentty_bridge::sync::repo::gh_repos().unwrap_or_default() } else { Vec::new() };
                    (login, repos)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.sync.looking_up = false;
                this.sync.gh_login = Some(login);
                this.sync.repos = repos;
                cx.notify();
            });
        })
        .detach();
    }

    fn connect_sync(&mut self, remote: Remote, cx: &mut Context<Self>) {
        if self.sync.connecting {
            return;
        }
        self.sync.connecting = true;
        self.sync.message = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { bridge::connect(remote) }).await;
            let _ = this.update(cx, |this, cx| {
                this.sync.connecting = false;
                match result {
                    Ok(outcome) => {
                        this.sync.config = bridge::load_config();
                        this.sync.status = Status::Ready;
                        this.sync.message = Some((
                            match outcome {
                                Connected::Created => t(cx, "sync.connected_new").to_string(),
                                Connected::Joined { devices } => tf(cx, "sync.connected_joined", &[("n", &devices.to_string())]),
                            },
                            false,
                        ));
                        this.sync_now(cx);
                    }
                    Err(err) => this.sync.message = Some((sync_error_text(&err, cx), true)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// A repository from the gh list: carried the way gh itself clones (its `git_protocol`).
    fn connect_github(&mut self, repo: String, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let ssh = cx.background_spawn(async move { agentty_bridge::sync::repo::gh_prefers_ssh() }).await;
            let _ = this.update(cx, |this, cx| this.connect_sync(Remote::Github { repo, ssh }, cx));
        })
        .detach();
    }

    fn set_transport(&mut self, ssh: bool, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { bridge::set_github_ssh(ssh) }).await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.sync.config = bridge::load_config();
                        this.sync.message = None;
                    }
                    Err(err) => this.sync.message = Some((format!("{err:#}"), true)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn create_sync_repo(&mut self, cx: &mut Context<Self>) {
        let Some(input) = self.sync.new_repo_input.clone() else { return };
        let name = input.read(cx).text().trim().to_string();
        if name.is_empty() || self.sync.connecting {
            return;
        }
        self.sync.connecting = true;
        self.sync.message = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let created = cx.background_spawn(async move { agentty_bridge::sync::repo::gh_create_private(&name) }).await;
            let _ = this.update(cx, |this, cx| {
                this.sync.connecting = false;
                match created {
                    Ok(repo) => this.connect_github(repo, cx),
                    Err(err) => {
                        this.sync.message = Some((format!("{err:#}"), true));
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    fn set_sync_paused(&mut self, paused: bool, cx: &mut Context<Self>) {
        let _ = bridge::update_config(|c| c.paused = paused);
        self.sync.config.paused = paused;
        self.sync.status = if paused { Status::Paused } else { Status::Ready };
        if !paused {
            self.sync_now(cx);
        }
        cx.notify();
    }

    fn disconnect_sync(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { bridge::disconnect() }).await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.sync.config = bridge::load_config();
                        this.sync.overview = Overview::default();
                        this.sync.status = Status::Off;
                        this.sync.pending.clear();
                        this.sync.message = Some((t(cx, "sync.disconnected").to_string(), false));
                    }
                    Err(err) => this.sync.message = Some((format!("{err:#}"), true)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn sync_text_input(
        slot: &mut Option<Entity<TextInput>>,
        placeholder: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TextInput> {
        slot.get_or_insert_with(|| cx.new(|cx| TextInput::new("", placeholder, window, cx))).clone()
    }

    fn device_name_input(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<TextInput> {
        if let Some((input, _)) = &self.sync.device_input {
            return input.clone();
        }
        let input = cx.new(|cx| TextInput::new(self.sync.config.device_name.clone(), "", window, cx));
        let subscription = cx.subscribe(&input, |this, input, event: &TextInputEvent, cx| {
            if matches!(event, TextInputEvent::Changed | TextInputEvent::Confirmed) {
                let name = input.read(cx).text().trim().to_string();
                if !name.is_empty() {
                    let saved = name.clone();
                    let _ = bridge::update_config(move |c| c.device_name = saved);
                    this.sync.config.device_name = name;
                }
            }
        });
        self.sync.device_input = Some((input.clone(), subscription));
        input
    }

    pub(super) fn render_sync_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let field = |input: Entity<TextInput>| {
            div()
                .flex_1()
                .min_w_0()
                .px_2()
                .py_1()
                .rounded_md()
                .border_1()
                .border_color(hex(Chrome::BORDER))
                .bg(hex(0x1a1a1a))
                .t_body()
                .child(input)
        };
        let message = self
            .sync
            .message
            .clone()
            .map(|(text, error)| div().t_small().text_color(hex(if error { Chrome::ERROR } else { Chrome::SUCCESS })).child(text));
        let intro = div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "sync.intro"));

        if let Some(remote) = self.sync.config.remote.clone() {
            let paused = self.sync.config.paused;
            let device = self.device_name_input(window, cx);
            let repository = section(t(cx, "sync.repository"))
                .child(intro)
                .child(row_with_hint(
                    &remote.label(),
                    &self.sync_status_text(cx),
                    div()
                        .flex()
                        .gap_1()
                        .child(action_button(
                            "sync-settings-now",
                            t(cx, "sync.now"),
                            cx.listener(|this, _: &ClickEvent, _, cx| this.sync_now(cx)),
                        ))
                        .child(action_button(
                            "sync-pause",
                            t(cx, if paused { "sync.resume" } else { "sync.pause" }),
                            cx.listener(move |this, _: &ClickEvent, _, cx| this.set_sync_paused(!paused, cx)),
                        ))
                        .child(action_button(
                            "sync-disconnect",
                            t(cx, "sync.disconnect"),
                            cx.listener(|this, _: &ClickEvent, _, cx| this.disconnect_sync(cx)),
                        )),
                ))
                .when(self.sync.status == Status::Public, |d| {
                    d.child(div().t_small().text_color(hex(Chrome::ERROR)).child(t(cx, "sync.public_hint")))
                })
                .children(message);
            // A GitHub repository can go through either program; any other URL is git's.
            let transport = match &remote {
                Remote::Github { ssh, .. } => {
                    let ssh = *ssh;
                    div()
                        .flex()
                        .gap_1()
                        .child(chip(
                            "sync-via-gh",
                            t(cx, "sync.via_gh"),
                            !ssh,
                            cx.listener(|this, _: &ClickEvent, _, cx| this.set_transport(false, cx)),
                        ))
                        .child(chip(
                            "sync-via-git",
                            t(cx, "sync.via_git"),
                            ssh,
                            cx.listener(|this, _: &ClickEvent, _, cx| this.set_transport(true, cx)),
                        ))
                }
                Remote::Git { .. } => div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "sync.via_git_url")),
            };
            let repository = repository.child(row_with_hint(t(cx, "sync.transport"), t(cx, "sync.transport_hint"), transport));
            let this_device = section(t(cx, "sync.device")).child(row_with_hint(
                t(cx, "sync.device_name"),
                t(cx, "sync.device_name_hint"),
                div().w(px(220.)).child(field(device)),
            ));
            let rules = section(t(cx, "sync.when"))
                .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "sync.when_body")))
                .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "sync.privacy_body")));
            return div().flex().flex_col().child(repository).child(this_device).child(rules);
        }

        // Not connected: pick or create a private repository.
        self.look_up_github(cx);
        let method = self.sync.method;
        let methods = div()
            .flex()
            .gap_1()
            .child(chip(
                "sync-method-gh",
                t(cx, "sync.method_github"),
                method == Method::Github,
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.sync.method = Method::Github;
                    cx.notify();
                }),
            ))
            .child(chip(
                "sync-method-url",
                t(cx, "sync.method_url"),
                method == Method::Url,
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.sync.method = Method::Url;
                    cx.notify();
                }),
            ));
        let mut setup = section(t(cx, "sync.setup")).child(intro).child(methods);
        let busy = self.sync.connecting;
        match method {
            Method::Github => match &self.sync.gh_login {
                None => setup = setup.child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "sync.looking_up"))),
                Some(None) => setup = setup.child(div().t_small().text_color(hex(Chrome::WARNING)).child(t(cx, "sync.gh_missing"))),
                Some(Some(login)) => {
                    setup =
                        setup.child(div().t_small().text_color(hex(Chrome::MUTED)).child(tf(cx, "sync.gh_account", &[("login", login)])));
                    let new_repo = Self::sync_text_input(&mut self.sync.new_repo_input, "agentty-sync", window, cx);
                    setup = setup.child(row_with_hint(
                        t(cx, "sync.create_repo"),
                        t(cx, "sync.create_repo_hint"),
                        div().flex().items_center().gap_1().child(div().w(px(180.)).child(field(new_repo))).child(
                            action_button(
                                "sync-create",
                                t(cx, "sync.create"),
                                cx.listener(|this, _: &ClickEvent, _, cx| this.create_sync_repo(cx)),
                            )
                            .when(busy, |b| b.opacity(0.5)),
                        ),
                    ));
                    let private: Vec<GithubRepo> = self.sync.repos.iter().filter(|r| r.is_private()).cloned().collect();
                    if !private.is_empty() {
                        let mut list = div().flex().flex_col().gap_0p5().max_h(px(240.)).id("sync-repos").overflow_y_scroll();
                        for (index, repo) in private.into_iter().enumerate() {
                            let name = repo.name_with_owner.clone();
                            list = list.child(
                                div()
                                    .id(("sync-repo", index))
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                                    .child(icon("lock", IconSize::INLINE, hex(Chrome::MUTED)))
                                    .child(div().flex_1().min_w_0().truncate().t_small().child(name.clone()))
                                    .child(div().flex_shrink_0().t_caption().text_color(hex(Chrome::ACCENT)).child(t(cx, "sync.use_repo")))
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                        this.connect_github(name.clone(), cx);
                                    })),
                            );
                        }
                        setup =
                            setup.child(div().pt_1().t_caption().text_color(hex(Chrome::MUTED)).child(t(cx, "sync.pick_repo"))).child(list);
                    }
                }
            },
            Method::Url => {
                let url = Self::sync_text_input(&mut self.sync.url_input, "git@github.com:you/agentty-sync.git", window, cx);
                setup = setup.child(row_with_hint(
                    t(cx, "sync.url"),
                    t(cx, "sync.url_hint"),
                    div().flex().items_center().gap_1().child(div().w(px(260.)).child(field(url.clone()))).child(
                        action_button(
                            "sync-connect-url",
                            t(cx, "sync.connect"),
                            cx.listener(move |this, _: &ClickEvent, _, cx| {
                                let text = url.read(cx).text().trim().to_string();
                                if text.is_empty() {
                                    return;
                                }
                                // Used as given, with the user's own git (SSH keys, credential helper).
                                this.connect_sync(Remote::Git { url: text }, cx);
                            }),
                        )
                        .when(busy, |b| b.opacity(0.5)),
                    ),
                ));
            }
        }
        if busy {
            setup = setup.child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "sync.connecting")));
        }
        let rules = section(t(cx, "sync.when"))
            .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "sync.when_body")))
            .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "sync.privacy_body")));
        div().flex().flex_col().child(setup.children(message)).child(rules)
    }

    /// `debug sync connect <url> | now | popover | turn <pane id> | state`.
    pub(super) fn debug_sync(&mut self, argument: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (command, rest) = argument.split_once(' ').unwrap_or((argument, ""));
        match command {
            // The n-th session of the workspace in front, continued as its button does.
            "continue" => {
                if let Some((ws, Some(sync_id))) = self.front_sync_workspace() {
                    let target = ws.id;
                    if let Some(entry) = self.synced_sessions(Some(&sync_id)).into_iter().nth(rest.trim().parse().unwrap_or(0)) {
                        self.continue_synced(entry, Some(target), window, cx);
                    }
                }
            }
            // The n-th session of Session history → Synced.
            "missing" => {
                let entry = self.missing_sessions(cx).into_iter().nth(rest.trim().parse().unwrap_or(0));
                if let Some(entry) = entry {
                    self.continue_synced(entry, None, window, cx);
                }
            }
            "context" => {
                if let Some((_, Some(sync_id))) = self.front_sync_workspace() {
                    if let Some(entry) = self.synced_sessions(Some(&sync_id)).into_iter().nth(rest.trim().parse().unwrap_or(0)) {
                        self.share_synced_context(entry, window, cx);
                    }
                }
            }
            "filter" => {
                self.session_filter = super::SessionFilter::Synced;
                self.refresh_synced_local(cx);
                self.show_panel(super::SidePanel::Sessions, cx);
            }
            "connect" => self.connect_sync(Remote::Git { url: rest.trim().to_string() }, cx),
            // `owner/name`, as picked from the gh list.
            "connect-gh" => self.connect_github(rest.trim().to_string(), cx),
            "transport" => self.set_transport(rest.trim() == "ssh", cx),
            "now" => self.sync_now(cx),
            "popover" => {
                self.sync.popover_open = !self.sync.popover_open;
                cx.notify();
            }
            "turn" => self.sync_after_turn(rest.trim().parse().unwrap_or(0), cx),
            _ => {}
        }
        eprintln!("sync: {}", self.sync.debug_state());
    }
}

fn sync_error_text(err: &SyncError, cx: &gpui::App) -> String {
    match err {
        SyncError::NotConnected => t(cx, "sync.status.off").to_string(),
        SyncError::Public => t(cx, "sync.status.public").to_string(),
        SyncError::Busy => t(cx, "sync.busy").to_string(),
        SyncError::NotSyncRepository => t(cx, "sync.not_sync_repo").to_string(),
        SyncError::Failed(reason) => reason.clone(),
    }
}
