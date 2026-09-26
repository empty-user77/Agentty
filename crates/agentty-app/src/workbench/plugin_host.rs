//! What plugins can do in a window: read where the user is, send prompts and keystrokes, read
//! sessions, show notifications and panels; and `agentty://` links from other apps.

use super::{LaunchTarget, Page, Pane, Workbench, Workspace};
use crate::i18n::{t, tf};
use crate::launch::{LaunchSpec, PaneKind};
use crate::plugins::{self, PluginCall};
use crate::terminal::AgentStatus;
use agentty_bridge::model::{Agent, Role};
use agentty_bridge::plugins::link::{self, Link};
use agentty_bridge::plugins::ui::UiEvent;
use agentty_bridge::plugins::{codes, PromptRequest, PromptTarget};
use gpui::{AppContext, Context, Window};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;

/// Longest single turn returned by `session/get`.
/// Characters of a `ui/notify` message shown — a toast, not a page.
const MAX_NOTIFY_CHARS: usize = 300;
/// How long after the user used a plugin it may type into a terminal it did not open.
const PLUGIN_GESTURE_WINDOW: std::time::Duration = std::time::Duration::from_secs(10);

const TURN_TEXT_LIMIT: usize = 20_000;
/// Panes plugins may be told about at once.
const MAX_WATCHED_PANES: usize = 32;
/// How often a watched pane is looked at when nothing is being drawn.
const PANE_POLL: std::time::Duration = std::time::Duration::from_millis(400);

fn status_id(pane: &Pane, cx: &gpui::App) -> &'static str {
    let view = pane.read(cx);
    if !view.is_running() {
        return "exited";
    }
    if !view.is_agent() {
        return "shell";
    }
    match view.status {
        AgentStatus::Idle => "idle",
        AgentStatus::Working => "working",
        AgentStatus::Thinking => "thinking",
        AgentStatus::Finished(_) => "finished",
        AgentStatus::Permission(_) => "permission",
        AgentStatus::Question(_) => "question",
        AgentStatus::Interrupted => "interrupted",
    }
}

/// What of the context a plugin is allowed to see, from the permissions in its manifest.
#[derive(Clone, Copy, Default)]
pub struct ContextScope {
    /// `workspace.read`: folders and titles.
    pub places: bool,
    /// `session.read`: session ids.
    pub sessions: bool,
}

impl ContextScope {
    fn of(plugin: &str, cx: &gpui::App) -> Self {
        let manifest = plugins::plugin(cx, plugin).and_then(|p| p.manifest.as_ref());
        Self {
            places: manifest.is_some_and(|m| m.has_permission("workspace.read")),
            sessions: manifest.is_some_and(|m| m.has_permission("session.read")),
        }
    }

    /// Everything, for Agentty's own use (the "Send to…" dialog, the flow page).
    fn full() -> Self {
        Self { places: true, sessions: true }
    }
}

fn pane_json(pane: &Pane, scope: ContextScope, cx: &gpui::App) -> Value {
    let view = pane.read(cx);
    let mut value = json!({
        "id": view.pane_id,
        "kind": crate::brand::kind_id(view.agent_kind().unwrap_or(PaneKind::Shell)),
        "tool": view.tool_id(),
        "status": status_id(pane, cx),
        "running": view.is_running(),
    });
    // The folder and what the agent called the task say a lot about the user's work.
    if scope.places {
        value["title"] = json!(view.display_title());
        value["cwd"] = json!(view.display_cwd());
    }
    if scope.sessions {
        value["sessionId"] = json!(view.session_id_live.clone().or_else(|| view.spec.session_id.clone()));
    }
    value
}

/// `claude`, `codex`, `shell`/`terminal` → pane kind (Claude Code by default).
fn kind_named(name: Option<&str>) -> PaneKind {
    match name {
        Some("codex") => PaneKind::Codex,
        Some("shell" | "terminal") => PaneKind::Shell,
        _ => PaneKind::Claude,
    }
}

/// What a watched pane is owed: nothing, the status it has moved to, or one last word that it is
/// gone. A status a plugin has already been told is not repeated — a pane says `working` on every
/// look while an agent works, and a plugin is not owed that fifty times.
#[derive(Debug, PartialEq, Eq)]
enum PaneNews {
    Nothing,
    Changed(&'static str),
    Closed,
}

fn pane_news(status: Option<&'static str>, last: &str) -> PaneNews {
    match status {
        Some(status) if status == last => PaneNews::Nothing,
        Some(status) => PaneNews::Changed(status),
        None if last == "closed" => PaneNews::Nothing,
        None => PaneNews::Closed,
    }
}

impl Workbench {
    fn workspace_json(&self, index: usize, with_panes: bool, scope: ContextScope, cx: &gpui::App) -> Value {
        let ws: &Workspace = &self.workspaces[index];
        let mut value = json!({ "id": ws.id, "active": index == self.active_workspace });
        if scope.places {
            value["name"] = json!(self.workspace_title(ws, cx));
            value["cwd"] = json!(ws.cwd);
        }
        if with_panes {
            value["panes"] = ws.tabs.iter().flat_map(|t| t.root.leaves()).map(|p| pane_json(&p, scope, cx)).collect();
        }
        value
    }

    /// Where the user is (or `pane`), as `plugin` is allowed to see it.
    pub fn plugin_context(&self, plugin: &str, pane: Option<&Pane>, cx: &gpui::App) -> Value {
        self.scoped_context(ContextScope::of(plugin, cx), pane, cx)
    }

    fn scoped_context(&self, scope: ContextScope, pane: Option<&Pane>, cx: &gpui::App) -> Value {
        let pane = pane.cloned().or_else(|| self.active_pane());
        let index =
            pane.as_ref().and_then(|p| self.locate(p)).map(|(w, _)| w).or((!self.workspaces.is_empty()).then_some(self.active_workspace));
        json!({
            "workspace": index.map(|i| self.workspace_json(i, false, scope, cx)),
            "pane": pane.map(|p| pane_json(&p, scope, cx)),
            "language": crate::settings::settings(cx).language.resolved().code(),
        })
    }

    /// Remembers that `plugin` started `pane`, so `pane/status` reaches it as that pane works.
    pub(super) fn watch_pane_for_plugin(&mut self, pane: u64, plugin: &str, cx: &mut Context<Self>) {
        self.watch_pane_for(pane, plugin, cx);
    }

    /// Remembers that `plugin` started `pane`, so `pane/status` reaches it as that pane works.
    fn watch_pane_for(&mut self, pane: u64, plugin: &str, cx: &mut Context<Self>) {
        // Only a plugin allowed to see agent status is told about one.
        if !ContextScope::of(plugin, cx).places {
            return;
        }
        // At most a handful: a plugin that opens sessions endlessly is not owed a list of them.
        if self.plugin_panes.len() >= MAX_WATCHED_PANES {
            return;
        }
        self.plugin_panes.insert(pane, (plugin.to_string(), ""));
        self.poll_pane_status(cx);
    }

    /// Keeps `pane/status` arriving when nothing is being drawn.
    ///
    /// A pane's status is read while the window is drawn, and a window nobody is looking at — one
    /// behind another, one on a locked screen — is not drawn at all. A plugin waiting for the
    /// agent it set to work would then be waiting for the user to come back, which is the one
    /// thing an AgentOS must not need. So while any pane is watched, Agentty looks anyway.
    fn poll_pane_status(&mut self, cx: &mut Context<Self>) {
        if self.plugin_pane_poll {
            return;
        }
        self.plugin_pane_poll = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(PANE_POLL).await;
                let watching = this.update(cx, |this, cx| {
                    this.broadcast_pane_status(cx);
                    !this.plugin_panes.is_empty()
                });
                if !matches!(watching, Ok(true)) {
                    break;
                }
            }
            let _ = this.update(cx, |this, _| this.plugin_pane_poll = false);
        })
        .detach();
    }

    /// Tells each plugin how the panes it started are getting on, when that changed. This is what
    /// a step of an AgentOS waits for: the agent it set to work has finished, or is asking.
    pub(super) fn broadcast_pane_status(&mut self, cx: &mut Context<Self>) {
        if self.plugin_panes.is_empty() {
            return;
        }
        let panes = self.all_panes();
        let mut changed: Vec<(String, Value)> = Vec::new();
        let mut gone: Vec<u64> = Vec::new();
        for (pane_id, (plugin, last)) in self.plugin_panes.iter_mut() {
            let pane = panes.iter().find(|pane| pane.read(cx).pane_id == *pane_id);
            let status = pane.map(|pane| status_id(pane, cx));
            match pane_news(status, last) {
                // The pane was closed: said once, because the pane stops being watched here.
                PaneNews::Closed => {
                    changed.push((plugin.clone(), json!({ "paneId": pane_id, "status": "closed", "running": false })));
                    gone.push(*pane_id);
                    continue;
                }
                PaneNews::Nothing => {
                    if pane.is_none() {
                        gone.push(*pane_id);
                    }
                    continue;
                }
                PaneNews::Changed(status) => *last = status,
            }
            let Some(pane) = pane else { continue };
            let status = *last;
            let view = pane.read(cx);
            changed.push((
                plugin.clone(),
                json!({
                    "paneId": pane_id,
                    "status": status,
                    "running": view.is_running(),
                    "agent": crate::brand::kind_id(view.agent_kind().unwrap_or(PaneKind::Shell)),
                    "title": view.display_title(),
                    "cwd": view.display_cwd(),
                }),
            ));
        }
        for pane in gone {
            self.plugin_panes.remove(&pane);
        }
        for (plugin, params) in changed {
            plugins::send_if_running(&plugin, "pane/status", params, cx);
        }
    }

    /// Sends the context to running plugins when it changed (focused pane, its status or folder).
    pub(super) fn broadcast_plugin_context(&mut self, window: &Window, cx: &mut Context<Self>) {
        let running = plugins::running_plugins(cx);
        if !window.is_window_active() || running.is_empty() {
            return;
        }
        // Compared on everything, sent per plugin with only what it may see. Compared as it is
        // rather than written out to a string: this runs on every frame, and almost every one of
        // them finds nothing changed.
        let key = self.scoped_context(ContextScope::full(), None, cx);
        if key != self.plugin_context_key {
            self.plugin_context_key = key;
            for id in running {
                let context = self.plugin_context(&id, None, cx);
                plugins::send_if_running(&id, "context/changed", json!({ "context": context }), cx);
            }
        }
    }

    pub(super) fn run_plugin_command(&mut self, plugin: &str, command: &str, pane: Option<Pane>, cx: &mut Context<Self>) {
        self.plugin_gesture.insert(plugin.to_string(), std::time::Instant::now());
        let context = self.plugin_context(plugin, pane.as_ref(), cx);
        if !plugins::notify_plugin(plugin, "command/execute", json!({ "command": command, "args": {}, "context": context }), cx) {
            self.show_toast(tf(cx, "plugins.not_enabled", &[("name", plugin)]), cx);
        }
    }

    pub(super) fn send_plugin_event(&mut self, plugin: &str, event: UiEvent, cx: &mut Context<Self>) {
        // The user did something in the plugin's panel (typing in a field is not asking for anything).
        if event.event != "change" {
            self.plugin_gesture.insert(plugin.to_string(), std::time::Instant::now());
        }
        let mut params = serde_json::to_value(event).unwrap_or_default();
        params["context"] = self.plugin_context(plugin, None, cx);
        // In the plugin's workspace, which automation the panel belonged to.
        if let Some(instance) = self.active_instance(plugin, cx) {
            params["instance"] = Value::String(instance);
        }
        plugins::notify_plugin(plugin, "ui/event", params, cx);
    }

    pub(super) fn open_plugin_panel(&mut self, plugin: &str, cx: &mut Context<Self>) {
        if self.plugin_panel.as_deref() == Some(plugin) && self.page.is_none() {
            return;
        }
        if let Some(previous) = self.plugin_panel.take() {
            plugins::notify_plugin(&previous, "panel/close", json!({ "context": self.plugin_context(&previous, None, cx) }), cx);
            self.close_plugin_window(&previous, cx);
        }
        // A panel that fills the area takes the place of a page, so an open page steps aside.
        self.page = None;
        self.plugin_mode_menu = false;
        self.plugin_panel = Some(plugin.to_string());
        plugins::notify_plugin(plugin, "panel/open", json!({ "context": self.plugin_context(plugin, None, cx) }), cx);
        // Already in a window of its own, behind something: bring it forward rather than do
        // nothing visible. A window it does not have yet is opened by the render that follows.
        self.activate_plugin_window(plugin, cx);
        cx.notify();
    }

    pub(super) fn toggle_plugin_panel(&mut self, plugin: &str, cx: &mut Context<Self>) {
        if self.plugin_panel.as_deref() == Some(plugin) && self.page.is_none() {
            self.close_plugin_panel(cx);
        } else {
            self.open_plugin_panel(plugin, cx);
        }
    }

    /// Drops a panel whose plugin is being disabled, removed, or is already gone — so without
    /// telling the plugin, which is no longer there to hear it. The window a panel in `window`
    /// mode opened goes with it: left behind it stays on screen with nothing to draw, and its
    /// handle would be reused the next time that plugin's panel opened.
    pub(super) fn drop_plugin_panel(&mut self, plugin: &str, cx: &mut Context<Self>) {
        if self.plugin_panel.as_deref() == Some(plugin) {
            self.plugin_panel = None;
            self.plugin_mode_menu = false;
        }
        self.close_plugin_window(plugin, cx);
    }

    pub(super) fn close_plugin_panel(&mut self, cx: &mut Context<Self>) {
        self.plugin_mode_menu = false;
        if let Some(previous) = self.plugin_panel.take() {
            plugins::notify_plugin(&previous, "panel/close", json!({ "context": self.plugin_context(&previous, None, cx) }), cx);
            self.close_plugin_window(&previous, cx);
        }
        cx.notify();
    }

    /// A plugin call that needs this window.
    pub fn plugin_call(&mut self, call: PluginCall, window: &mut Window, cx: &mut Context<Self>) {
        let params = call.params.clone();
        match call.method.as_str() {
            "ui/notify" => {
                let message: String = params["message"].as_str().unwrap_or_default().chars().take(MAX_NOTIFY_CHARS).collect();
                let text = format!("{}: {message}", call.plugin_name);
                if params["kind"].as_str() == Some("error") {
                    self.set_status(text.clone(), cx);
                }
                self.show_toast(text, cx);
                call.reply(Ok(Value::Null), cx);
            }
            "ui/showPanel" => {
                self.show_plugin(&call.plugin, window, cx);
                call.reply(Ok(Value::Null), cx);
            }
            "context/get" => call.reply(Ok(self.plugin_context(&call.plugin, None, cx)), cx),
            "workspace/list" => {
                // Reached only with `workspace.read`, so folders and titles are included.
                let scope = ContextScope::of(&call.plugin, cx);
                let list: Vec<Value> = (0..self.workspaces.len()).map(|i| self.workspace_json(i, true, scope, cx)).collect();
                call.reply(Ok(Value::Array(list)), cx);
            }
            "prompt/inject" => match serde_json::from_value::<PromptRequest>(params) {
                Ok(mut request) => {
                    request.source = Some(call.plugin_name.clone());
                    request.plugin = Some(call.plugin.clone());
                    // Into a terminal the plugin may not type into on its own: the user decides,
                    // in the same dialog a link's prompt goes through (never sent by itself).
                    if let Some(pane) = self.prompt_target_pane(&request, cx) {
                        let allowed = match (&pane, request.target) {
                            // A workspace of the plugin's own may get a new agent tab.
                            (_, PromptTarget::Workspace)
                                if request
                                    .workspace_id
                                    .and_then(|id| self.workspaces.iter().find(|w| w.id == id))
                                    .is_some_and(|w| w.plugin.as_deref() == Some(call.plugin.as_str())) =>
                            {
                                true
                            }
                            (Some(pane), _) => self.plugin_may_type(&call.plugin, pane, cx).is_ok(),
                            (None, _) => false,
                        };
                        if !allowed {
                            request.target = PromptTarget::Ask;
                            request.submit = false;
                        }
                    }
                    if request.target == PromptTarget::Ask {
                        self.open_prompt_dialog(request, window, cx);
                        call.reply(Ok(json!({ "status": "asked" })), cx);
                    } else {
                        let request_target = request.target;
                        let result = self.deliver_prompt(request, window, cx);
                        // The pane this plugin set to work: it hears how that pane gets on.
                        if let Ok(pane) = &result {
                            self.watch_pane_for(*pane, &call.plugin, cx);
                            if matches!(
                                request_target,
                                PromptTarget::NewTab | PromptTarget::NewWorkspace | PromptTarget::Split | PromptTarget::Own
                            ) {
                                self.plugin_launched.insert(*pane, call.plugin.clone());
                            }
                        }
                        call.reply(
                            result.map(|pane| json!({ "status": "sent", "paneId": pane })).map_err(|e| (codes::INVALID_PARAMS, e)),
                            cx,
                        );
                    }
                }
                Err(err) => call.reply(Err((codes::INVALID_PARAMS, format!("invalid prompt request: {err}"))), cx),
            },
            "terminal/send" => {
                let text = params["text"].as_str().unwrap_or_default().to_string();
                let submit = params["submit"].as_bool().unwrap_or(false);
                let pane = match params["paneId"].as_u64() {
                    Some(id) => self.all_panes().into_iter().find(|p| p.read(cx).pane_id == id),
                    None => self.active_pane(),
                };
                match pane {
                    Some(pane) if !text.is_empty() => match self.plugin_may_type(&call.plugin, &pane, cx) {
                        Ok(()) => {
                            let id = pane.read(cx).pane_id;
                            type_into(&pane, text, submit, true, cx);
                            call.reply(Ok(json!({ "paneId": id })), cx);
                        }
                        Err(why) => call.reply(Err((codes::PERMISSION_DENIED, why)), cx),
                    },
                    Some(_) => call.reply(Err((codes::INVALID_PARAMS, "text is empty".into())), cx),
                    None => call.reply(Err((codes::INVALID_PARAMS, "no such pane".into())), cx),
                }
            }
            // A terminal the plugin opened (`prompt/inject` into a new tab, split or its own
            // workspace), closed once its agent's work is read: an automation that asks an agent
            // every hour would otherwise leave a terminal behind every hour. Nothing else — not the
            // user's terminals, not another plugin's.
            "terminal/close" => {
                let id = call.params["paneId"].as_u64().unwrap_or_default();
                let pane = self.all_panes().into_iter().find(|p| p.read(cx).pane_id == id);
                match pane {
                    Some(pane) if self.plugin_launched.get(&id) == Some(&call.plugin) => {
                        self.plugin_launched.remove(&id);
                        self.perform_close(super::confirm::CloseTarget::Pane(pane), window, cx);
                        call.reply(Ok(Value::Null), cx);
                    }
                    Some(_) => call.reply(Err((codes::PERMISSION_DENIED, "a plugin closes only terminals it opened".into())), cx),
                    None => call.reply(Err((codes::INVALID_PARAMS, "no such pane".into())), cx),
                }
            }
            // The agents `prompt/inject` can start on this computer (installed), for a plugin to
            // offer the choice, or to take the only one.
            "agent/list" => match self.installed.clone() {
                Some(installed) => call.reply(Ok(agent_list(&installed)), cx),
                // Asked before the first look at what is installed ended (right after Agentty
                // started): look now and answer then, not with a guess.
                None => {
                    let task = cx.background_spawn(async { crate::agents::detect() });
                    cx.spawn(async move |this, cx| {
                        let installed = task.await;
                        let _ = this.update(cx, |this, cx| {
                            call.reply(Ok(agent_list(&installed)), cx);
                            this.installed = Some(installed);
                            cx.notify();
                        });
                    })
                    .detach();
                }
            },
            "media/htmlToVideo" => self.plugin_html_video(call, window, cx),
            "session/get" => self.plugin_session(call, cx),
            "workspace/instances" => {
                let list = self.plugin_instances(&call.plugin, cx);
                call.reply(Ok(Value::Array(list)), cx);
            }
            "workspace/setInstanceTitle" => {
                let (instance, title) = (call.params["instance"].as_str().unwrap_or_default(), call.params["title"].as_str());
                let result = self.set_instance_title(&call.plugin, instance, title, cx);
                call.reply(result.map(|()| Value::Null).map_err(|e| (codes::INVALID_PARAMS, e)), cx);
            }
            "workspace/closeInstance" => {
                let instance = call.params["instance"].as_str().unwrap_or_default().to_string();
                let result = self.close_instance(&call.plugin, &instance, window, cx);
                call.reply(result.map(|()| Value::Null).map_err(|e| (codes::INVALID_PARAMS, e)), cx);
            }
            browser if browser.starts_with("browser/") => self.plugin_browser_call(call, window, cx),
            other => call.reply(Err((codes::METHOD_NOT_FOUND, format!("unknown method {other}"))), cx),
        }
    }

    /// Whether `plugin` may type into `pane`. Its own terminals (ones it launched, or in its own
    /// workspace) always; another one — the user's own agents and shells — only right after the
    /// user used the plugin (a click in its panel, one of its commands), and never while that pane
    /// waits for the user to approve or answer something: a plugin must not answer for the user.
    fn plugin_may_type(&self, plugin: &str, pane: &Pane, cx: &gpui::App) -> Result<(), String> {
        let id = pane.read(cx).pane_id;
        let own_workspace = self.locate(pane).is_some_and(|(w, _)| self.workspaces[w].plugin.as_deref() == Some(plugin));
        if own_workspace || self.plugin_launched.get(&id).is_some_and(|p| p == plugin) {
            return Ok(());
        }
        if pane.read(cx).status.needs_user() {
            return Err("that terminal is waiting for the user's answer".into());
        }
        let recent = self.plugin_gesture.get(plugin).is_some_and(|at| at.elapsed() < PLUGIN_GESTURE_WINDOW);
        if recent {
            Ok(())
        } else {
            Err("a plugin types into a terminal it did not open only right after the user used it".into())
        }
    }

    /// The existing terminal a prompt request would type into, if it would type into one.
    fn prompt_target_pane(&self, request: &PromptRequest, cx: &gpui::App) -> Option<Option<Pane>> {
        match request.target {
            PromptTarget::Active => Some(self.active_pane()),
            PromptTarget::Pane => Some(request.pane_id.and_then(|id| self.all_panes().into_iter().find(|p| p.read(cx).pane_id == id))),
            PromptTarget::Workspace => {
                let ws = request.workspace_id.and_then(|id| self.workspaces.iter().find(|w| w.id == id));
                Some(ws.and_then(|ws| ws.tabs.get(ws.active_tab)).map(|t| t.active.clone()))
            }
            _ => None,
        }
    }

    fn plugin_session(&mut self, call: PluginCall, cx: &mut Context<Self>) {
        let pane = match call.params["paneId"].as_u64() {
            Some(id) => self.all_panes().into_iter().find(|p| p.read(cx).pane_id == id),
            None => self.active_pane(),
        };
        let Some(pane) = pane else { return call.reply(Err((codes::INVALID_PARAMS, "no such pane".into())), cx) };
        let view = pane.read(cx);
        let Some(agent) = view.agent_kind().and_then(PaneKind::agent) else {
            return call.reply(Err((codes::INVALID_PARAMS, "the pane is not running Claude Code or Codex".into())), cx);
        };
        let session_id = view.session_id_live.clone().or_else(|| view.spec.session_id.clone());
        let (cwd, launched) = (view.spec.cwd.clone(), view.launched_at_ms);
        let (pane_id, title, status) = (view.pane_id, view.display_title(), status_id(&pane, cx));
        let max_turns = call.params["maxTurns"].as_u64().unwrap_or(200).clamp(1, 2000) as usize;
        let task = cx.background_spawn(async move {
            let id = match (agent, session_id) {
                (_, Some(id)) => id,
                (Agent::Codex, None) => agentty_bridge::codex::find_recent(&cwd, launched).ok_or("no Codex session found yet")?,
                _ => return Err("no session yet".to_string()),
            };
            let (cwd, turns) = agentty_bridge::load(agent, &id).map_err(|e| format!("{e:#}"))?;
            Ok::<_, String>((id, cwd, turns))
        });
        cx.spawn(async move |_, cx| {
            let result = task.await;
            let _ = cx.update(|cx| match result {
                Ok((id, session_cwd, turns)) => {
                    let skip = turns.len().saturating_sub(max_turns);
                    let list: Vec<Value> = turns[skip..]
                        .iter()
                        .map(|turn| {
                            let text = agentty_bridge::fsutil::truncate_chars(&turn.text, TURN_TEXT_LIMIT);
                            json!({ "role": if turn.role == Role::User { "user" } else { "assistant" }, "text": text })
                        })
                        .collect();
                    call.reply(
                        Ok(json!({
                            "paneId": pane_id,
                            "agent": agent.id(),
                            "sessionId": id,
                            "title": title,
                            "cwd": session_cwd,
                            "status": status,
                            "turnCount": turns.len(),
                            "turns": list,
                        })),
                        cx,
                    );
                }
                Err(error) => call.reply(Err((codes::UNAVAILABLE, error)), cx),
            });
        })
        .detach();
    }

    /// Sends a prompt where `request.target` says (not `Ask`); returns the pane it went to.
    pub(super) fn deliver_prompt(&mut self, request: PromptRequest, window: &mut Window, cx: &mut Context<Self>) -> Result<u64, String> {
        let text = agentty_bridge::plugins::spill_long_prompt(&request.text, request.title.as_deref()).map_err(|e| e.to_string())?;
        if text.trim().is_empty() {
            return Err("the prompt is empty".into());
        }
        let kind = kind_named(request.agent.as_deref());
        let restricted = request.restricted();
        let model = request.model();
        if restricted {
            // Files only means the plugin's own files: a folder elsewhere (the home folder, say)
            // would put everything in it within the agent's reach.
            let plugin = request.plugin.as_deref().ok_or("files-only agents are started by plugins")?;
            let root = agentty_bridge::plugins::store::plugin_data_dir(plugin);
            let inside = request
                .cwd
                .as_ref()
                .and_then(|cwd| cwd.canonicalize().ok())
                .zip(root.canonicalize().ok())
                .is_some_and(|(cwd, root)| cwd.starts_with(root));
            if !inside {
                return Err("a files-only agent works in a folder of the plugin's own data".into());
            }
        }
        let find = |this: &Self, id: u64, cx: &gpui::App| this.all_panes().into_iter().find(|p| p.read(cx).pane_id == id);
        let pane = match request.target {
            PromptTarget::Ask => return Err("ask is handled by the dialog".into()),
            PromptTarget::Active => {
                let pane = self.active_pane().ok_or("no terminal is focused")?;
                type_into(&pane, text, request.submit, false, cx);
                pane
            }
            PromptTarget::Pane => {
                let pane = request.pane_id.and_then(|id| find(self, id, cx)).ok_or("no such pane")?;
                type_into(&pane, text, request.submit, false, cx);
                pane
            }
            PromptTarget::NewWorkspace | PromptTarget::NewTab | PromptTarget::Split => {
                let cwd = request.cwd.clone().filter(|p| p.is_dir()).unwrap_or_else(|| self.default_cwd(cx));
                let target = match request.target {
                    PromptTarget::NewTab => LaunchTarget::NewTab,
                    PromptTarget::Split => LaunchTarget::SplitRight,
                    _ => LaunchTarget::NewWorkspace,
                };
                self.launch_with_prompt(kind, text, request.title.clone(), cwd, request.submit, (restricted, model), target, window, cx)?
            }
            PromptTarget::Own if request.instance.is_some() => {
                let plugin = request.plugin.clone().ok_or("only a plugin has a workspace of its own")?;
                let instance = request.instance.clone().unwrap_or_default();
                let (spec, later) = self.prompt_spec(kind, text, request.cwd.clone(), request.submit, (restricted, model), cx);
                let pane = self.open_in_instance(&plugin, &instance, spec, cx)?;
                type_later(&pane, later, cx);
                // The job opens beside the automation's terminals; the user stays where they are.
                cx.notify();
                return Ok(pane.read(cx).pane_id);
            }
            PromptTarget::Own => {
                let plugin = request.plugin.clone().ok_or("only a plugin has a workspace of its own")?;
                if self.plugin_workspace(&plugin).is_none() {
                    self.create_plugin_workspace(&plugin, window, cx);
                }
                let index = self.plugin_workspace(&plugin).ok_or("the plugin's workspace could not be made")?;
                // The job's tab opens where the plugin works, and that is where the user sees it.
                if index != self.active_workspace {
                    let current = self.workspaces.get(self.active_workspace).filter(|ws| ws.plugin.is_none()).map(|ws| ws.id);
                    if current.is_some() {
                        self.before_plugin_workspace = current;
                    }
                    self.activate_workspace(index, window, cx);
                }
                let cwd = request.cwd.clone().filter(|p| p.is_dir()).unwrap_or_else(|| self.workspaces[index].cwd.clone());
                self.launch_with_prompt(
                    kind,
                    text,
                    request.title.clone(),
                    cwd,
                    request.submit,
                    (restricted, model),
                    LaunchTarget::NewTab,
                    window,
                    cx,
                )?
            }
            PromptTarget::Workspace => {
                let index =
                    request.workspace_id.and_then(|id| self.workspaces.iter().position(|w| w.id == id)).ok_or("no such workspace")?;
                self.activate_workspace(index, window, cx);
                let ws = &self.workspaces[index];
                let mut panes: Vec<Pane> = ws.tabs.get(ws.active_tab).map(|t| vec![t.active.clone()]).unwrap_or_default();
                panes.extend(ws.tabs.iter().flat_map(|t| t.root.leaves()));
                let idle = panes.into_iter().find(|p| {
                    let view = p.read(cx);
                    view.agent_kind() == Some(kind) && view.is_running() && !view.is_busy()
                });
                match idle {
                    Some(pane) if kind != PaneKind::Shell => {
                        type_into(&pane, text, request.submit, false, cx);
                        self.mark_active(&pane, cx);
                        pane
                    }
                    _ => {
                        let cwd = self.workspaces[index].cwd.clone();
                        self.launch_with_prompt(
                            kind,
                            text,
                            request.title.clone(),
                            cwd,
                            request.submit,
                            (restricted, model),
                            LaunchTarget::NewTab,
                            window,
                            cx,
                        )?
                    }
                }
            }
        };
        self.page = None;
        self.mark_active(&pane, cx);
        self.focus_pane(&pane, window, cx);
        cx.notify();
        Ok(pane.read(cx).pane_id)
    }

    /// How a prompt starts: an agent with it as the first message, or a program it is typed into
    /// once ready (never with Enter in a shell).
    pub(super) fn prompt_spec(
        &mut self,
        kind: PaneKind,
        text: String,
        cwd: Option<PathBuf>,
        submit: bool,
        (restricted, model): (bool, Option<String>),
        cx: &mut Context<Self>,
    ) -> (LaunchSpec, Option<(String, u64)>) {
        let cwd = cwd.filter(|p| p.is_dir()).unwrap_or_else(|| self.default_cwd(cx));
        let (mut spec, later) = match (kind.agent(), submit) {
            (Some(agent), true) => {
                let mut spec = LaunchSpec::with_prompt(agent, text, String::new(), cwd);
                spec.title = LaunchSpec::new(kind, PathBuf::new()).title;
                (spec, None)
            }
            (agent, _) => (LaunchSpec::new(kind, cwd), Some((text, if agent.is_some() { 4000 } else { 1200 }))),
        };
        spec.restricted = restricted;
        spec.model = model.filter(|_| kind.agent().is_some());
        (spec, later)
    }

    #[allow(clippy::too_many_arguments)]
    fn launch_with_prompt(
        &mut self,
        kind: PaneKind,
        text: String,
        title: Option<String>,
        cwd: PathBuf,
        submit: bool,
        (restricted, model): (bool, Option<String>),
        target: LaunchTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Pane, String> {
        // A new agent in a project another agent works in gets a working tree of its own, like one
        // opened from the + menu.
        let cwd = if kind.agent().is_some() { self.own_tree_now(kind, &cwd, cx).unwrap_or(cwd) } else { cwd };
        let (spec, later) = match (kind.agent(), submit) {
            // The agent starts with the prompt as its first message.
            (Some(agent), true) => {
                let mut spec = LaunchSpec::with_prompt(agent, text, String::new(), cwd);
                spec.title = LaunchSpec::new(kind, PathBuf::new()).title;
                (spec, None)
            }
            // Typed once the program is ready, without pressing Enter (never Enter in a shell).
            (agent, _) => (LaunchSpec::new(kind, cwd), Some((text, if agent.is_some() { 4000 } else { 1200 }))),
        };
        let mut spec = spec;
        spec.restricted = restricted;
        spec.model = model.filter(|_| kind.agent().is_some());
        match target {
            LaunchTarget::NewWorkspace => {
                self.create_workspace(spec, window, cx);
                if let (Some(ws), Some(title)) = (self.workspaces.last_mut(), title.filter(|t| !t.trim().is_empty())) {
                    ws.name = Some(title.chars().take(60).collect());
                }
                self.persist(cx);
            }
            LaunchTarget::NewTab => self.open_tab(spec, window, cx),
            LaunchTarget::SplitRight => self.split_with(spec, super::Axis::Horizontal, window, cx),
            LaunchTarget::SplitDown => self.split_with(spec, super::Axis::Vertical, window, cx),
        }
        let pane = self.active_pane().ok_or("could not open a terminal")?;
        if let Some((text, delay)) = later {
            let typed = pane.clone();
            cx.spawn(async move |_, cx| {
                cx.background_executor().timer(Duration::from_millis(delay)).await;
                let _ = cx.update(|cx| typed.update(cx, |view, _| view.insert_text(&text)));
            })
            .detach();
        }
        Ok(pane)
    }

    /// Handles an `agentty://` link opened by another app.
    pub fn open_agentty_link(&mut self, url: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.mini.is_some() {
            self.exit_mini(None, window, cx);
        }
        if std::env::var("AGENTTY_BACKGROUND").as_deref() != Ok("1") {
            window.activate_window();
            cx.activate(true);
        }
        match link::parse(url) {
            Err(error) => self.set_status(tf(cx, "plugins.link_failed", &[("name", &error)]), cx),
            Ok(Link::Store { plugin }) => self.open_plugins_page(plugin, cx),
            Ok(Link::Prompt(mut request)) => {
                request.target = PromptTarget::Ask;
                // Any website can open a link: the text is typed in for the user to read and send
                // themselves, unless they tick "send right away" in the dialog.
                request.submit = false;
                request.source.get_or_insert_with(|| t(cx, "plugins.link_source").to_string());
                self.open_prompt_dialog(request, window, cx);
            }
            Ok(Link::Plugin { id, path, query }) => {
                let installed = plugins::plugin(cx, &id).cloned();
                match installed {
                    Some(plugin) if plugin.active() => {
                        let context = self.plugin_context(&id, None, cx);
                        plugins::open_link(&id, &path, &query, url, context, cx);
                    }
                    Some(_) => {
                        self.open_plugins_page(Some(id.clone()), cx);
                        self.plugins_page.message = Some((tf(cx, "plugins.link_disabled", &[("name", &id)]), true));
                    }
                    None if agentty_bridge::plugins::store::builtin(&id).is_some() => {
                        self.open_plugins_page(Some(id.clone()), cx);
                        self.plugins_page.pending_link = Some((id, url.to_string()));
                    }
                    None => self.set_status(tf(cx, "plugins.link_unknown", &[("name", &id)]), cx),
                }
            }
        }
        cx.notify();
    }

    pub(super) fn open_plugins_page(&mut self, focus: Option<String>, cx: &mut Context<Self>) {
        if self.page != Some(Page::Plugins) {
            self.open_page(Page::Plugins, cx);
        }
        // Arriving at a named plugin, it is the one the page opens on.
        if focus.is_some() {
            self.plugins_page.selected = focus.clone();
        }
        self.plugins_page.focus = focus;
        cx.notify();
    }
}

/// Types a prompt into a pane that has just started, once its program is ready.
fn type_later(pane: &Pane, later: Option<(String, u64)>, cx: &mut Context<Workbench>) {
    let Some((text, delay)) = later else { return };
    let typed = pane.clone();
    cx.spawn(async move |_, cx| {
        cx.background_executor().timer(Duration::from_millis(delay)).await;
        let _ = cx.update(|cx| typed.update(cx, |view, _| view.insert_text(&text)));
    })
    .detach();
}

/// Types into a pane: agents get Enter after the paste when `submit`; shells only when the plugin
/// explicitly typed into that terminal (`terminal/send`).
fn type_into(pane: &Pane, text: String, submit: bool, allow_shell_enter: bool, cx: &mut gpui::App) {
    pane.update(cx, |view, cx| {
        let agent = view.is_agent();
        if submit && (agent || allow_shell_enter) {
            view.submit_prompt(text, cx);
        } else {
            view.insert_text(&text);
        }
    });
}

/// Limits of `media/htmlToVideo`: seconds, frames a second, pixels a side, bytes of HTML.
const HTML_VIDEO_SECONDS: f64 = 60.;
const HTML_VIDEO_FPS: u32 = 30;
const HTML_VIDEO_SIDE: u32 = 1920;
const HTML_VIDEO_BYTES: usize = 2 * 1024 * 1024;

impl Workbench {
    /// `media/htmlToVideo` (`files`): `{ from, to, seconds, width?, height?, fps? }`. Loads the HTML
    /// at `from` (the plugin's folder) in a page of its own with the virtual clock
    /// ([`crate::html_recorder`]), and records `seconds` of it into the MP4 at `to`, a `__seek` and
    /// a snapshot a frame. Fails on a script error, and if the page tries to go anywhere else.
    pub(super) fn plugin_html_video(&mut self, call: PluginCall, window: &mut Window, cx: &mut Context<Self>) {
        use agentty_bridge::plugins::files;
        let bad = |message: String| (codes::INVALID_PARAMS, message);
        let params = &call.params;
        let resolve = |key: &str| files::resolve(&call.plugin, params[key].as_str().unwrap_or_default()).map_err(|e| bad(format!("{e:#}")));
        let parsed = (|| {
            let (from, to) = (resolve("from")?, resolve("to")?);
            let html = std::fs::read_to_string(&from).map_err(|e| bad(format!("{}: {e}", from.display())))?;
            if html.len() > HTML_VIDEO_BYTES {
                return Err(bad("the page is larger than 2 MB".into()));
            }
            let seconds = params["seconds"].as_f64().unwrap_or(15.).clamp(1., HTML_VIDEO_SECONDS);
            let side = |key: &str, default: u32| params[key].as_u64().map_or(default, |v| (v as u32).clamp(64, HTML_VIDEO_SIDE)) & !1;
            let fps = params["fps"].as_u64().map_or(30, |v| (v as u32).clamp(10, HTML_VIDEO_FPS));
            Ok((html, to, seconds, side("width", 1920), side("height", 1080), fps))
        })();
        let (html, to, seconds, width, height, fps) = match parsed {
            Ok(parsed) => parsed,
            Err(err) => return call.reply(Err(err), cx),
        };
        let Some(recorder) = crate::html_recorder::Recorder::start(window, width, height, &html) else {
            return call.reply(Err((codes::UNAVAILABLE, "recording a page is not available here".into())), cx);
        };
        let count = (seconds * fps as f64).round() as usize;
        cx.spawn(async move |_, cx| {
            use futures::channel::oneshot;
            let result: Result<Value, String> = async {
                // Loaded (at most 20 s), and a moment for its fonts and first layout.
                for _ in 0..200 {
                    cx.background_executor().timer(std::time::Duration::from_millis(100)).await;
                    if !recorder.loading() {
                        break;
                    }
                }
                cx.background_executor().timer(std::time::Duration::from_millis(400)).await;
                let eval = |script: String| {
                    let (send, answer) = oneshot::channel();
                    recorder.eval(&script, move |result| {
                        let _ = send.send(result);
                    });
                    answer
                };
                let mut writer = crate::platform::video::Mp4Writer::new(&to, width, height, fps).map_err(|e| format!("{e:#}"))?;
                let (mut first, mut moved) = (None::<u64>, false);
                for index in 0..count {
                    let ms = index as f64 * 1000. / fps as f64;
                    let errors = eval(format!("__seek({ms})")).await.map_err(|_| "the page stopped".to_string())??;
                    if errors != "0" {
                        let list =
                            eval("JSON.stringify(__agenttyErrors.slice(0, 3))".into()).await.ok().and_then(Result::ok).unwrap_or_default();
                        return Err(format!("the page has script errors: {list}"));
                    }
                    // It is the page it was given, or nothing is recorded.
                    if index % 30 == 0 && !matches!(recorder.address().as_str(), "" | "about:blank") {
                        return Err("the page tried to go to another address".into());
                    }
                    let (send, answer) = oneshot::channel();
                    recorder.frame(move |pixels| {
                        let _ = send.send(pixels);
                    });
                    let pixels = answer.await.ok().flatten().ok_or("a frame could not be taken")?;
                    // Whether anything moves at all: a still page is not a video.
                    let digest = pixels.iter().step_by(997).fold(0u64, |h, b| h.wrapping_mul(31).wrapping_add(*b as u64));
                    match first {
                        None => first = Some(digest),
                        Some(d) if d != digest => moved = true,
                        _ => {}
                    }
                    writer.append(&pixels).map_err(|e| format!("{e:#}"))?;
                }
                writer.finish().map_err(|e| format!("{e:#}"))?;
                if !moved {
                    return Err("nothing on the page moved".into());
                }
                let size = std::fs::metadata(&to).map(|m| m.len()).unwrap_or(0);
                Ok(json!({ "width": width, "height": height, "seconds": seconds, "frames": count, "size": size }))
            }
            .await;
            drop(recorder);
            eprintln!("html-video: {}", result.as_ref().map(Value::to_string).unwrap_or_else(|e| e.clone()));
            let _ = cx.update(|cx| call.reply(result.map_err(|e| (codes::INVALID_PARAMS, e)), cx));
        })
        .detach();
    }
}

/// `agent/list`: the agents `prompt/inject` can start here, with the models each was seen using
/// (its default first).
fn agent_list(installed: &crate::agents::Installed) -> Value {
    let models = |id: &str| -> Vec<Value> {
        match id {
            "claude" => installed.claude_models.iter().map(|(m, label)| json!({ "id": m, "label": label })).collect(),
            _ => installed.codex_models.iter().map(|m| json!({ "id": m, "label": agentty_bridge::pretty_model(m) })).collect(),
        }
    };
    Value::Array(
        [("claude", "Claude Code"), ("codex", "Codex")]
            .iter()
            .filter(|(binary, _)| installed.has(binary))
            .map(|(id, name)| json!({ "id": id, "name": name, "version": installed.version(id), "models": models(id) }))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::{pane_news, PaneNews};

    #[test]
    fn a_plugin_hears_a_status_once_and_not_on_every_look() {
        // The first look after a prompt was sent.
        assert_eq!(pane_news(Some("working"), ""), PaneNews::Changed("working"));
        // Every look while the agent works.
        assert_eq!(pane_news(Some("working"), "working"), PaneNews::Nothing);
        // The one a step is waiting for.
        assert_eq!(pane_news(Some("finished"), "working"), PaneNews::Changed("finished"));
        // And back to work when the plugin sends the next prompt.
        assert_eq!(pane_news(Some("working"), "finished"), PaneNews::Changed("working"));
    }

    #[test]
    fn a_pane_that_is_gone_is_said_once() {
        assert_eq!(pane_news(None, "working"), PaneNews::Closed);
        // A pane opened and closed between two looks is still reported: "it is gone" is the
        // answer a plugin waiting on it needs.
        assert_eq!(pane_news(None, ""), PaneNews::Closed);
        // And never twice.
        assert_eq!(pane_news(None, "closed"), PaneNews::Nothing);
    }
}
