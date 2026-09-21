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
const TURN_TEXT_LIMIT: usize = 20_000;

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
            "language": crate::settings::settings(cx).language.code(),
        })
    }

    /// Sends the context to running plugins when it changed (focused pane, its status or folder).
    pub(super) fn broadcast_plugin_context(&mut self, window: &Window, cx: &mut Context<Self>) {
        let running = plugins::running_plugins(cx);
        if !window.is_window_active() || running.is_empty() {
            return;
        }
        // Compared on everything, sent per plugin with only what it may see.
        let key = self.scoped_context(ContextScope::full(), None, cx).to_string();
        if key != self.plugin_context_key {
            self.plugin_context_key = key;
            for id in running {
                let context = self.plugin_context(&id, None, cx);
                plugins::send_if_running(&id, "context/changed", json!({ "context": context }), cx);
            }
        }
    }

    pub(super) fn run_plugin_command(&mut self, plugin: &str, command: &str, pane: Option<Pane>, cx: &mut Context<Self>) {
        let context = self.plugin_context(plugin, pane.as_ref(), cx);
        if !plugins::notify_plugin(plugin, "command/execute", json!({ "command": command, "args": {}, "context": context }), cx) {
            self.show_toast(tf(cx, "plugins.not_enabled", &[("name", plugin)]), cx);
        }
    }

    pub(super) fn send_plugin_event(&mut self, plugin: &str, event: UiEvent, cx: &mut Context<Self>) {
        let mut params = serde_json::to_value(event).unwrap_or_default();
        params["context"] = self.plugin_context(plugin, None, cx);
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
                let message: String = params["message"].as_str().unwrap_or_default().chars().take(300).collect();
                let text = format!("{}: {message}", call.plugin_name);
                if params["kind"].as_str() == Some("error") {
                    self.set_status(text.clone(), cx);
                }
                self.show_toast(text, cx);
                call.reply(Ok(Value::Null), cx);
            }
            "ui/showPanel" => {
                self.open_plugin_panel(&call.plugin, cx);
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
                    if request.target == PromptTarget::Ask {
                        self.open_prompt_dialog(request, window, cx);
                        call.reply(Ok(json!({ "status": "asked" })), cx);
                    } else {
                        let result = self.deliver_prompt(request, window, cx);
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
                    Some(pane) if !text.is_empty() => {
                        let id = pane.read(cx).pane_id;
                        type_into(&pane, text, submit, true, cx);
                        call.reply(Ok(json!({ "paneId": id })), cx);
                    }
                    Some(_) => call.reply(Err((codes::INVALID_PARAMS, "text is empty".into())), cx),
                    None => call.reply(Err((codes::INVALID_PARAMS, "no such pane".into())), cx),
                }
            }
            "session/get" => self.plugin_session(call, cx),
            other => call.reply(Err((codes::METHOD_NOT_FOUND, format!("unknown method {other}"))), cx),
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
                self.launch_with_prompt(kind, text, request.title.clone(), cwd, request.submit, target, window, cx)?
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
                        self.launch_with_prompt(kind, text, request.title.clone(), cwd, request.submit, LaunchTarget::NewTab, window, cx)?
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

    #[allow(clippy::too_many_arguments)]
    fn launch_with_prompt(
        &mut self,
        kind: PaneKind,
        text: String,
        title: Option<String>,
        cwd: PathBuf,
        submit: bool,
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
