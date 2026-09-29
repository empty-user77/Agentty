//! Agent groups (`agentty_bridge::agent_groups`): a team of agents with a role each, started together
//! in one tab of a project. Every member starts with its role and the others' as its first message,
//! wears its name and role on its pane, and is linked to the other members from the start — Claude
//! Code members through Claude Code's own session messaging, the others with live context links.
//! Work that belongs to another member is handed to it with `agentty group send`.

use super::panes::Axis;
use super::{Pane, Workbench};
use crate::agent_signal::{browser_reply, GroupRequest};
use crate::i18n::{t, tf};
use crate::launch::{LaunchSpec, PaneKind};
use agentty_bridge::agent_groups::AgentGroup;
use agentty_bridge::model::Agent;
use gpui::{AppContext, Context, Window};
use std::path::PathBuf;
use std::time::Duration;

/// A group started in this window.
pub struct GroupRun {
    pub id: u64,
    pub group: AgentGroup,
    /// (pane id, member index) of every member that was started, the main one first.
    pub members: Vec<(u64, usize)>,
    /// The project the group works in.
    pub cwd: PathBuf,
    /// The language the members talk to the user in.
    pub language: &'static str,
}

/// How member `index` of `group` is started: its agent, with its role as the first message.
fn member_spec(group: &AgentGroup, index: usize, cwd: &std::path::Path, language: &str) -> LaunchSpec {
    let member = &group.agents[index];
    let agent = if member.agent == "codex" { Agent::Codex } else { Agent::Claude };
    LaunchSpec::with_prompt(agent, group.role_prompt(index, language), member.name.clone(), cwd.to_path_buf())
}

/// How long the group waits for its Claude Code members to register their sessions before it links
/// the members it has.
const LINK_WAIT: Duration = Duration::from_secs(90);
const LINK_POLL: Duration = Duration::from_secs(2);
/// How long the other members wait for the user to trust the project folder in the main pane.
const TRUST_WAIT: Duration = Duration::from_secs(15 * 60);
/// Longest request one member hands another.
const MAX_HANDOVER: usize = 20_000;

impl Workbench {
    /// The group and member a pane belongs to.
    pub(super) fn group_member_of(&self, pane_id: u64) -> Option<(&GroupRun, usize)> {
        self.agent_groups.iter().find_map(|run| run.members.iter().find(|(p, _)| *p == pane_id).map(|(_, index)| (run, *index)))
    }

    /// Starts `group` in a new tab of the active workspace: one pane per member, in the project's
    /// folder, each with its role as its first message. The main member (the first) starts first;
    /// in a folder Claude Code has not been told to trust yet, the others wait until the user
    /// answered its question there, so it is asked once and not in every pane.
    pub(super) fn start_agent_group(&mut self, group: AgentGroup, window: &mut Window, cx: &mut Context<Self>) {
        let cwd: PathBuf = self.workspaces.get(self.active_workspace).map(|ws| ws.cwd.clone()).unwrap_or_else(crate::launch::home_dir);
        let repo = agentty_bridge::agent_groups::repo_slug(&cwd);
        if !group.scope.allows(repo.as_deref()) {
            self.set_status(tf(cx, "groups.wrong_repo", &[("name", &group.name)]), cx);
            return;
        }
        let language = agentty_bridge::idea::language_name(crate::settings::settings(cx).language.code());
        self.open_tab(member_spec(&group, 0, &cwd, language), window, cx);
        let Some(main) = self.active_pane() else { return };
        self.next_group_run += 1;
        let id = self.next_group_run;
        let main_id = main.read(cx).pane_id;
        // Claude Code may still ask whether it can trust the folder. A session registers itself only
        // once that is answered, so the others start when the main one has registered.
        let may_ask =
            group.agents.len() > 1 && group.agents[0].agent == "claude" && !agentty_bridge::claude_trust::is_trusted_exactly(&cwd);
        self.set_status(tf(cx, "groups.started", &[("name", &group.name), ("n", &group.agents.len().to_string())]), cx);
        self.agent_groups.push(GroupRun { id, group, members: vec![(main_id, 0)], cwd, language });
        if !may_ask {
            self.start_other_members(id, window, cx);
            return cx.notify();
        }
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let asked = std::time::Instant::now();
            let mut told = false;
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                // `Some(true)`: the main session runs, go on; `Some(false)`: give up; `None`: wait.
                let state = this
                    .update(cx, |this, cx| {
                        // Gone, or the agent quit (the user said no to the folder): nobody else starts.
                        let main_runs = this.pane_by_id(main_id, cx).is_some_and(|p| p.read(cx).agent_kind().is_some());
                        if !this.agent_groups.iter().any(|r| r.id == id) || !main_runs || asked.elapsed() >= TRUST_WAIT {
                            return Some(false);
                        }
                        if this.peer_session_of(main_id, cx).is_some() {
                            return Some(true);
                        }
                        // Still not running after a few seconds: it is waiting for the answer.
                        if !told && asked.elapsed() >= Duration::from_secs(4) {
                            told = true;
                            this.set_status(t(cx, "groups.waiting_trust").to_string(), cx);
                        }
                        None
                    })
                    .unwrap_or(Some(false));
                match state {
                    Some(true) => break,
                    Some(false) => return,
                    None => {}
                }
            }
            let _ = cx.update_window(handle, |_, window, cx| {
                let _ = this.update(cx, |this, cx| this.start_other_members(id, window, cx));
            });
        })
        .detach();
        cx.notify();
    }

    /// Opens the members after the main one beside it, puts the main one in focus view and links
    /// the group once everyone has started.
    fn start_other_members(&mut self, run_id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(run) = self.agent_groups.iter().find(|r| r.id == run_id) else { return };
        let (group, cwd, language) = (run.group.clone(), run.cwd.clone(), run.language);
        let Some(main) = run.members.first().and_then(|(pane, _)| self.pane_by_id(*pane, cx)) else { return };
        // A grid: the second member to the right of the main one, the next ones below them in turn.
        let mut panes: Vec<Pane> = vec![main.clone()];
        for index in 1..group.agents.len() {
            let (anchor, axis) = match index {
                1 => (panes[0].clone(), Axis::Horizontal),
                2 => (panes[0].clone(), Axis::Vertical),
                3 => (panes[1].clone(), Axis::Vertical),
                4 => (panes[2].clone(), Axis::Horizontal),
                _ => (panes[3].clone(), Axis::Horizontal),
            };
            match self.split_pane_with(&anchor, member_spec(&group, index, &cwd, language), axis, window, cx) {
                Some(pane) => panes.push(pane),
                None => break,
            }
        }
        let started: Vec<(u64, usize)> = panes.iter().enumerate().skip(1).map(|(index, p)| (p.read(cx).pane_id, index)).collect();
        if let Some(run) = self.agent_groups.iter_mut().find(|r| r.id == run_id) {
            run.members.extend(started);
        }
        // The main member is where the user works: in focus view, the others working beside it.
        if panes.len() > 1 && self.zoomed.as_ref() != Some(&main) {
            self.toggle_zoom(&main, window, cx);
        } else {
            self.focus_pane(&main, window, cx);
        }
        self.link_group_when_ready(run_id, cx);
        cx.notify();
    }

    /// Waits until the Claude Code members registered their sessions (they do once they start),
    /// then links every member with every other one.
    fn link_group_when_ready(&mut self, run_id: u64, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let started = std::time::Instant::now();
            loop {
                cx.background_executor().timer(LINK_POLL).await;
                let ready = this
                    .update(cx, |this, cx| {
                        let Some(run) = this.agent_groups.iter().find(|r| r.id == run_id) else { return true };
                        let waiting = run.members.iter().any(|(pane, _)| {
                            this.pane_by_id(*pane, cx).is_some_and(|p| p.read(cx).spec.kind == PaneKind::Claude)
                                && this.peer_session_of(*pane, cx).is_none()
                        });
                        !waiting || started.elapsed() >= LINK_WAIT
                    })
                    .unwrap_or(true);
                if ready {
                    break;
                }
            }
            let _ = this.update(cx, |this, cx| this.link_group(run_id, cx));
        })
        .detach();
    }

    /// Links the members of a group, every one with every other one: Claude Code sessions message
    /// each other directly (they are told each other's session names), the rest get live context
    /// links both ways.
    fn link_group(&mut self, run_id: u64, cx: &mut Context<Self>) {
        let Some(run) = self.agent_groups.iter().find(|r| r.id == run_id) else { return };
        let members: Vec<(u64, String)> = run
            .members
            .iter()
            .filter(|(pane, _)| self.pane_by_id(*pane, cx).is_some())
            .map(|(pane, index)| (*pane, run.group.agents[*index].name.clone()))
            .collect();
        let group_name = run.group.name.clone();
        let peers: Vec<(u64, String, Option<agentty_bridge::claude::PeerSession>)> =
            members.into_iter().map(|(pane, name)| (pane, name, self.peer_session_of(pane, cx))).collect();
        for (i, (a, _, peer_a)) in peers.iter().enumerate() {
            for (b, _, peer_b) in peers.iter().skip(i + 1) {
                match (peer_a, peer_b) {
                    (Some(_), Some(_)) => {
                        self.flow_link_direct(*a, *b);
                        self.flow_link_direct(*b, *a);
                    }
                    _ => {
                        self.connect_panes(*a, *b, true, cx);
                        self.connect_panes(*b, *a, true, cx);
                    }
                }
            }
        }
        // One message per Claude Code member with the session names of the others it can message.
        for (pane, _, peer) in &peers {
            if peer.is_none() {
                continue;
            }
            let others: Vec<String> = peers
                .iter()
                .filter(|(other, _, p)| other != pane && p.is_some())
                .map(|(_, name, p)| format!("- \"{name}\": Claude Code session \"{}\"", p.as_ref().map(|p| p.name.as_str()).unwrap_or("")))
                .collect();
            if others.is_empty() {
                continue;
            }
            self.deliver_to_pane(
                *pane,
                format!(
                    "Agentty linked the members of the agent group \"{group_name}\" so you share your work directly \
                     with Claude Code's session messaging:\n{}\n\n\
                     When you finish something another member builds on, or need something from one, tell it with \
                     SendMessage (use ListAgents if a name is not found); its replies arrive as messages. Keep handing \
                     new requests of mine over with `agentty group send`, so I see them in that member's pane. \
                     Don't answer this message; wait for my request.",
                    others.join("\n")
                ),
                cx,
            );
        }
        cx.notify();
    }

    /// `agentty group …` from a member of a group: the members (`list`), or a request handed to one
    /// (`send`).
    pub fn answer_group_request(&mut self, request: GroupRequest, cx: &mut Context<Self>) {
        let answer = self.group_request(&request, cx);
        let _ = request.reply.send(browser_reply(answer));
        cx.notify();
    }

    fn group_request(&mut self, request: &GroupRequest, cx: &mut Context<Self>) -> Result<String, String> {
        let Some((run, me)) = self.group_member_of(request.pane) else {
            return Err("this terminal is not a member of an agent group".into());
        };
        let group = run.group.clone();
        let members = run.members.clone();
        let from = group.agents[me].name.clone();
        match request.args["action"].as_str() {
            Some("list") => {
                let list: Vec<serde_json::Value> = group
                    .agents
                    .iter()
                    .enumerate()
                    .map(|(index, agent)| {
                        let pane = members.iter().find(|(_, i)| *i == index).and_then(|(p, _)| self.pane_by_id(*p, cx));
                        let status = match &pane {
                            None => "closed",
                            Some(p) if p.read(cx).status.in_turn() => "working",
                            Some(_) => "idle",
                        };
                        serde_json::json!({
                            "name": agent.name, "role": agent.role, "agent": agent.agent,
                            "status": status, "you": index == me,
                        })
                    })
                    .collect();
                Ok(serde_json::json!({ "group": group.name, "members": list }).to_string())
            }
            Some("send") => {
                let to = request.args["to"].as_str().unwrap_or_default();
                let message = request.args["message"].as_str().unwrap_or_default().trim();
                if message.is_empty() || message.chars().count() > MAX_HANDOVER {
                    return Err(format!("the request must be 1–{MAX_HANDOVER} characters"));
                }
                let Some(target) = group.agents.iter().position(|a| a.name.eq_ignore_ascii_case(to.trim())) else {
                    let names: Vec<&str> = group.agents.iter().map(|a| a.name.as_str()).collect();
                    return Err(format!("no member named \"{to}\"; the members are {}", names.join(", ")));
                };
                if target == me {
                    return Err("that is you; do the work yourself".into());
                }
                let Some(pane) = members.iter().find(|(_, i)| *i == target).map(|(p, _)| *p).filter(|p| self.pane_by_id(*p, cx).is_some())
                else {
                    return Err(format!("\"{}\" is closed", group.agents[target].name));
                };
                let busy = self.pane_by_id(pane, cx).is_some_and(|p| p.read(cx).status.in_turn());
                self.deliver_to_pane(
                    pane,
                    format!(
                        "Message from \"{from}\", a member of your agent group:\n\n{message}\n\n\
                         If it is a request in your role, it is yours now: do it, and when \"{from}\" waits for the \
                         result, tell it with `agentty group send \"{from}\" \"…\"`. If it is news or an answer, use it \
                         and reply only when something is needed from you."
                    ),
                    cx,
                );
                let name = group.agents[target].name.clone();
                self.set_status(tf(cx, "groups.handed", &[("from", &from), ("to", &name)]), cx);
                Ok(serde_json::json!({ "sent": name, "queued": busy }).to_string())
            }
            _ => Err("unknown group action (use list or send)".into()),
        }
    }

    /// Installs a group package the user picked, and says which.
    pub(super) fn import_agent_group(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions { files: true, directories: false, multiple: false, prompt: None });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let result = agentty_bridge::agent_groups::install_file(&path);
            let _ = this.update(cx, |this, cx| {
                let message = match result {
                    Ok(group) => tf(cx, "groups.imported", &[("name", &group.name)]),
                    Err(err) => tf(cx, "groups.import_failed", &[("error", &format!("{err:#}"))]),
                };
                this.set_status(message, cx);
            });
        })
        .detach();
    }

    /// The folder with the installed groups (made, with the sample group, when missing).
    pub(super) fn open_agent_groups_folder(&mut self, cx: &mut Context<Self>) {
        agentty_bridge::agent_groups::ensure_sample();
        let dir = agentty_bridge::agent_groups::groups_dir();
        let _ = std::fs::create_dir_all(&dir);
        crate::platform::reveal(&dir);
        self.set_status(t(cx, "groups.folder_opened").to_string(), cx);
    }
}
