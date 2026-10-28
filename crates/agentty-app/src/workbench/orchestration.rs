//! Orchestrations kept across restarts: which Claude Code session leads each one, what it is for and
//! how far it got. One place decides its stage — from the orchestrator's state and its subagents'
//! transcripts — for the strip on its pane, the Orchestration page and the page's list alike. Work a
//! restart (or a crash) cut off shows as interrupted, with Resume; Stop ends the whole team; Delete
//! takes it off the list, and closes its terminal if asked.

use super::{Pane, Workbench};
use crate::i18n::{t, tf};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, TypeScale};
use gpui::{div, prelude::*, AnyElement, ClickEvent, Context, SharedString, Window};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

/// The page's colours: cyan for what runs, green for what is done, amber for what was cut off.
pub const RUNNING: u32 = 0x22d3ee;
pub const DONE: u32 = 0x34d399;
pub const CUT: u32 = 0xf5a524;

/// What the orchestrator is asked when it carries on after being cut off or stopped.
const RESUME: &str = "Resume this orchestration where it left off. It was cut off (Agentty or this session was closed) \
or stopped, so the agents that were running then are not running any more and their work may be missing or half \
done. Check each deliverable of the plan against what is in the folder, start again only the agents whose work is \
missing or incomplete (wave by wave, as planned, each wave's agents in one message), then have the reviewer check \
and finish with the report. Do not redo finished work.";

/// What the orchestrator is told when the user stops the whole team.
const STOP: &str = "Stop this orchestration now: stop every agent of it that is still running (TaskStop for each \
background agent), start nothing new, and reply with one short line saying what was stopped.";

/// How far an orchestration got, as last seen. Kept on disk, so a restart knows what was cut off.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// Its first turn: reading and planning.
    #[default]
    Planning,
    /// The plan is shown; it waits for the go.
    Ready,
    Running,
    Done,
    /// Cut off by a restart or a crash: its agents died with the process.
    Interrupted,
    /// Stopped by the user.
    Stopped,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub id: u64,
    /// The orchestrator's session: how its pane is found again after a restart. Empty until the
    /// lead's agent names it (Codex does after its first turn); `lead_pane` finds it until then.
    pub session: String,
    /// The agent that leads: Claude Code, or Codex where Claude Code is not installed.
    #[serde(default = "claude")]
    pub lead: agentty_bridge::model::Agent,
    /// The lead's pane while its session is not known yet.
    #[serde(skip)]
    pub lead_pane: Option<u64>,
    pub goal: String,
    pub folder: PathBuf,
    pub started_ms: u64,
    #[serde(default)]
    pub stage: Stage,
    /// When the agent process last seen running it started: another process means the first one
    /// ended, and work it was doing ended with it.
    #[serde(default)]
    pub seen_since: Option<u64>,
    /// Until when it was just told to start, resume or stop and has not picked that up yet: its
    /// pane still shows the state before (or its session is still coming up), which says nothing.
    #[serde(skip)]
    pub settling_until: u64,
    /// The plan the orchestrator handed over with `agentty plan`: every agent, by wave, before any
    /// of them starts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plan: Vec<PlanAgent>,
}

fn claude() -> agentty_bridge::model::Agent {
    agentty_bridge::model::Agent::Claude
}

/// One agent of an orchestration's plan. Its subagent is found by name (the Agent tool's
/// description).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlanAgent {
    pub name: String,
    /// 1 for the first wave.
    pub wave: u32,
    /// What it does, in a few words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Who does the work: claude, or another AI agent (codex, gemini, agy).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The names of the agents whose results it needs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
}

/// Bounds on a plan: it comes from an agent and is drawn on the page.
const PLAN_AGENTS: usize = 40;
const PLAN_WAVES: u32 = 20;
const PLAN_TEXT: usize = 200;

/// A plan as `agentty plan` sends it: `{"agents": [...]}` or the bare list. Names are unique, waves
/// run from 1, and a dependency names another agent of an earlier wave.
pub fn parse_plan(value: &serde_json::Value) -> Result<Vec<PlanAgent>, String> {
    let list = value.get("agents").unwrap_or(value);
    let agents: Vec<PlanAgent> = serde_json::from_value(list.clone()).map_err(|e| format!("not a plan: {e}"))?;
    if agents.is_empty() {
        return Err("the plan has no agents".into());
    }
    if agents.len() > PLAN_AGENTS {
        return Err(format!("at most {PLAN_AGENTS} agents"));
    }
    let mut names = std::collections::HashSet::new();
    for agent in &agents {
        let texts = [Some(&agent.name), agent.role.as_ref(), agent.agent.as_ref(), agent.model.as_ref()];
        if agent.name.trim().is_empty() || texts.into_iter().flatten().chain(&agent.depends_on).any(|t| t.chars().count() > PLAN_TEXT) {
            return Err(format!("agent {:?}: a name is needed, and every text is at most {PLAN_TEXT} characters", agent.name));
        }
        if !(1..=PLAN_WAVES).contains(&agent.wave) {
            return Err(format!("agent {:?}: wave runs from 1 to {PLAN_WAVES}", agent.name));
        }
        if !names.insert(agent.name.as_str()) {
            return Err(format!("agent {:?} is in the plan twice", agent.name));
        }
    }
    for agent in &agents {
        for needed in &agent.depends_on {
            match agents.iter().find(|a| &a.name == needed) {
                Some(other) if other.wave < agent.wave => {}
                Some(_) => return Err(format!("agent {:?} depends on {needed:?}, which is not in an earlier wave", agent.name)),
                None => return Err(format!("agent {:?} depends on {needed:?}, which is not in the plan", agent.name)),
            }
        }
    }
    Ok(agents)
}

/// The team's subagents now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Team {
    pub running: usize,
    pub done: usize,
    /// Not finished, and their process is gone.
    pub cut_off: usize,
    /// Started subagents (running, done or cut off).
    pub all: usize,
    /// Agents of its plan that have not started yet (not in `all`).
    pub pending: usize,
}

/// The orchestrator's pane as seen now.
#[derive(Clone, Copy, Debug, Default)]
pub struct Seen {
    /// Its Claude Code runs (the pane is there and the agent in it).
    pub alive: bool,
    /// When that process started.
    pub since: Option<u64>,
    pub in_turn: bool,
    /// Its last turn is over (and nothing happened since).
    pub turn_over: bool,
    pub team: Team,
    /// It was just told something and has not started on it yet.
    pub settling: bool,
}

/// The stage after seeing the pane: (stage, the process seen). Stopped stays until the user resumes;
/// a new process while it planned or ran means it was cut off.
pub fn next_stage(stage: Stage, seen_since: Option<u64>, seen: &Seen) -> (Stage, Option<u64>) {
    let unfinished = matches!(stage, Stage::Planning | Stage::Running);
    if seen.settling && !seen.in_turn {
        return (stage, if seen.alive { seen.since } else { seen_since });
    }
    if !seen.alive {
        return (if unfinished { Stage::Interrupted } else { stage }, seen_since);
    }
    let mut stage = stage;
    // Another process than the one last seen, and nobody restarted it on purpose: cut off.
    if !seen.settling && seen.since != seen_since && seen_since.is_some() && unfinished {
        stage = Stage::Interrupted;
    }
    let seen_since = seen.since;
    if stage == Stage::Stopped {
        return (stage, seen_since);
    }
    let team = seen.team;
    let stage = if seen.in_turn {
        // Starting the first wave is running; a turn before any agent (or changing the plan) is planning.
        if team.all > 0 || stage == Stage::Running {
            Stage::Running
        } else {
            Stage::Planning
        }
    } else if team.running > 0 {
        Stage::Running
    } else if seen.turn_over {
        // Its turn is over with nothing running: whatever was cut off before, it has dealt with.
        if team.all == 0 {
            Stage::Ready
        } else {
            Stage::Done
        }
    } else if stage == Stage::Running && team.all > 0 && team.cut_off == 0 && team.pending == 0 {
        // Resting in the same process with its whole team done (the last agent's report came in
        // after its turn, so the pane says idle rather than finished): done.
        Stage::Done
    } else if team.cut_off > 0 && unfinished {
        // Cut off while it ran; a finished one keeps its result.
        Stage::Interrupted
    } else {
        // Resting with nothing new to tell (after a restart, say): as it was.
        stage
    };
    (stage, seen_since)
}

/// The stage in a word or two (the list on the page).
pub fn stage_key(stage: Stage) -> &'static str {
    match stage {
        Stage::Planning => "orch.stage_planning",
        Stage::Ready => "orch.plan_ready",
        Stage::Running => "orch.stage_running_short",
        Stage::Done => "orch.stage_done_short",
        Stage::Interrupted => "orch.stage_interrupted_short",
        Stage::Stopped => "orch.stage_stopped_short",
    }
}

fn records_path() -> PathBuf {
    agentty_bridge::fsutil::data_dir().join("orchestrations.json")
}

pub fn load_records() -> Vec<Record> {
    std::fs::read(records_path()).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
}

/// Goals are the user's words: the file is private.
fn save_records(records: &[Record]) {
    if let Ok(bytes) = serde_json::to_vec_pretty(records) {
        if let Err(err) = agentty_bridge::fsutil::write_private(&records_path(), &bytes) {
            eprintln!("agentty: orchestrations could not be saved: {err}");
        }
    }
}

impl Workbench {
    pub(super) fn orchestrations(&mut self) -> &mut Vec<super::orchestration::Record> {
        if !self.session_board.records_loaded {
            self.session_board.records_loaded = true;
            self.session_board.records = load_records();
        }
        &mut self.session_board.records
    }

    /// A new orchestration, led by the agent in `pane_id`.
    pub(super) fn add_orchestration(&mut self, pane_id: u64, goal: String, folder: PathBuf, cx: &mut Context<Self>) -> Option<u64> {
        let pane = self.all_panes().into_iter().find(|p| p.read(cx).pane_id == pane_id)?;
        let lead = pane.read(cx).spec.kind.agent()?;
        let session = pane.read(cx).current_session().unwrap_or_default();
        let now = crate::ui::now_ms();
        let records = self.orchestrations();
        let id = records.iter().map(|r| r.id + 1).max().unwrap_or(1).max(now);
        records.push(Record {
            id,
            session,
            lead,
            lead_pane: Some(pane_id),
            goal,
            folder,
            started_ms: now,
            stage: Stage::Planning,
            seen_since: None,
            settling_until: 0,
            plan: Vec::new(),
        });
        save_records(records);
        Some(id)
    }

    pub(super) fn record_of_pane(&self, pane: &Pane, cx: &gpui::App) -> Option<&Record> {
        if !crate::settings::settings(cx).orchestration {
            return None;
        }
        let view = pane.read(cx);
        if !view.is_agent() {
            return None;
        }
        let (current, started) = (view.current_session(), view.spec.session_id.clone());
        self.session_board.records.iter().find(|r| {
            (!r.session.is_empty() && (Some(&r.session) == current.as_ref() || Some(&r.session) == started.as_ref()))
                || (r.session.is_empty() && r.lead_pane == Some(view.pane_id))
        })
    }

    /// `agentty plan` from a pane: the plan of the orchestration that pane leads (no other).
    pub fn answer_plan(&mut self, request: crate::agent_signal::PlanRequest, cx: &mut Context<Self>) {
        if let Some(step) = request.plan.get("step").and_then(|s| s.as_str()).map(str::to_string) {
            let result = self.plan_step(request.pane, &step, request.plan.get("name").and_then(|n| n.as_str()).unwrap_or(""), cx);
            let _ = request.reply.send(crate::agent_signal::browser_reply(result));
            return cx.notify();
        }
        let result = (|| {
            let plan = parse_plan(&request.plan)?;
            let pane = self.all_panes().into_iter().find(|p| p.read(cx).pane_id == request.pane).ok_or("the asking pane is gone")?;
            let id = self
                .record_of_pane(&pane, cx)
                .map(|r| r.id)
                .ok_or("this agent does not lead an orchestration (agentty plan is for one started from Agentty's Orchestrator)")?;
            let waves = plan.iter().map(|a| a.wave).max().unwrap_or(0);
            let agents = plan.len();
            let records = self.orchestrations();
            if let Some(record) = records.iter_mut().find(|r| r.id == id) {
                record.plan = plan;
            }
            save_records(records);
            Ok::<_, String>(serde_json::json!({ "agents": agents, "waves": waves }).to_string())
        })();
        let _ = request.reply.send(crate::agent_signal::browser_reply(result));
        cx.notify();
    }

    /// `agentty plan --start <name>` / `--done <name>`: an agent of the plan started or finished.
    /// A lead without subagents of its own (Codex runs its agents as processes) says so itself;
    /// the run is kept with the pane like a subagent its hooks report.
    fn plan_step(&mut self, pane_id: u64, step: &str, name: &str, cx: &mut Context<Self>) -> Result<String, String> {
        let pane = self.all_panes().into_iter().find(|p| p.read(cx).pane_id == pane_id).ok_or("the asking pane is gone")?;
        let record = self
            .record_of_pane(&pane, cx)
            .ok_or("this agent does not lead an orchestration (agentty plan is for one started from Agentty's Orchestrator)")?;
        if !record.plan.iter().any(|a| a.name == name) {
            return Err(format!("no agent named \"{name}\" in the plan: send the plan first (agentty plan)"));
        }
        let name = name.to_string();
        let now = std::time::Instant::now();
        match step {
            "start" => pane.update(cx, |view, _| {
                // Started again: the earlier run under its name is over (it failed, or was given up).
                for run in view.subagents.iter_mut().filter(|r| r.task.as_deref() == Some(name.as_str()) && r.finished.is_none()) {
                    run.finished = Some(now);
                }
                let id = format!("plan:{name}:{}", view.subagents.len());
                view.subagents.push(crate::terminal::SubagentRun {
                    id,
                    kind: "plan".into(),
                    task: Some(name.clone()),
                    started: now,
                    finished: None,
                    last_tool: None,
                    result: None,
                    last_active: now,
                });
            }),
            "done" => pane.update(cx, |view, _| {
                for run in view.subagents.iter_mut().filter(|r| r.task.as_deref() == Some(name.as_str()) && r.finished.is_none()) {
                    run.finished = Some(now);
                }
            }),
            _ => return Err(format!("unknown step {step}: use --start or --done")),
        }
        Ok(serde_json::json!({ "agent": name, "step": step }).to_string())
    }

    /// The colour of the orchestration mark for workspace `index`, when an orchestration is led
    /// from it (its pane, or — asleep — its saved session).
    pub(super) fn orchestration_mark(&self, index: usize, cx: &gpui::App) -> Option<u32> {
        if !crate::settings::settings(cx).orchestration {
            return None;
        }
        let ws = self.workspaces.get(index)?;
        let record = self.session_board.records.iter().find(|record| {
            let live = ws.tabs.iter().flat_map(|t| t.root.leaves()).any(|pane| {
                let view = pane.read(cx);
                view.current_session().as_deref() == Some(record.session.as_str())
                    || view.spec.session_id.as_deref() == Some(record.session.as_str())
            });
            live || self.sleeping_workspace_of(record) == Some(index)
        })?;
        let pane = self.pane_of_record(record, cx);
        let team = self.team_of(&record.session, pane.as_ref(), cx);
        Some(self.stage_words(record.stage, team, cx).1)
    }

    /// The asleep workspace (not opened since the app started) that holds the session.
    fn sleeping_workspace_of(&self, record: &Record) -> Option<usize> {
        self.workspaces.iter().position(|ws| {
            ws.dormant.as_ref().is_some_and(|snapshot| {
                snapshot.tabs.iter().any(|tab| {
                    tab.layout
                        .to_tree()
                        .is_some_and(|tree| tree.leaves().iter().any(|p| p.session_id.as_deref() == Some(record.session.as_str())))
                })
            })
        })
    }

    /// The workspace to close with the orchestration: its own, holding nothing but its session.
    fn own_workspace_of(&self, record: &Record, cx: &gpui::App) -> Option<u64> {
        if let Some(pane) = self.pane_of_record(record, cx) {
            let (w, _) = self.locate(&pane)?;
            let ws = &self.workspaces[w];
            let leaves: Vec<Pane> = ws.tabs.iter().flat_map(|t| t.root.leaves()).collect();
            return (leaves == [pane] && ws.parked.is_empty()).then_some(ws.id);
        }
        let ws = &self.workspaces[self.sleeping_workspace_of(record)?];
        let snapshot = ws.dormant.as_ref()?;
        let panes: usize = snapshot.tabs.iter().filter_map(|t| t.layout.to_tree()).map(|t| t.leaves().len()).sum();
        (panes == 1 && ws.parked.is_empty()).then_some(ws.id)
    }

    pub(super) fn pane_of_record(&self, record: &Record, cx: &gpui::App) -> Option<Pane> {
        self.all_panes().into_iter().find(|p| {
            let view = p.read(cx);
            if record.session.is_empty() {
                return record.lead_pane == Some(view.pane_id);
            }
            view.current_session().as_deref() == Some(record.session.as_str())
                || view.spec.session_id.as_deref() == Some(record.session.as_str())
        })
    }

    /// Brings every orchestration's stage up to date with its pane and its team; saves on a change.
    pub(super) fn sync_orchestrations(&mut self, cx: &gpui::App) {
        self.orchestrations();
        let now = crate::ui::now_ms();
        let mut changed = false;
        // A lead that had no session yet (Codex names it after its first turn) has one now.
        for index in 0..self.session_board.records.len() {
            let record = &self.session_board.records[index];
            if !record.session.is_empty() {
                continue;
            }
            let found = self.pane_of_record(record, cx).and_then(|pane| pane.read(cx).current_session());
            if let Some(session) = found {
                self.session_board.records[index].session = session;
                changed = true;
            }
        }
        for index in 0..self.session_board.records.len() {
            let record = self.session_board.records[index].clone();
            let settling = now < record.settling_until;
            let pane = self.pane_of_record(&record, cx);
            let seen = match &pane {
                Some(pane) => {
                    let view = pane.read(cx);
                    Seen {
                        alive: view.live_agent.is_some(),
                        since: view.agent_since_ms,
                        in_turn: view.status.in_turn(),
                        turn_over: matches!(view.status, crate::terminal::AgentStatus::Finished(_)),
                        team: self.team_of(&record.session, Some(pane), cx),
                        settling,
                    }
                }
                None => Seen { settling, ..Seen::default() },
            };
            // Right after launch the agent is not seen yet: nothing to judge until it is.
            if pane.is_some() && !seen.alive && record.seen_since.is_none() {
                continue;
            }
            let (stage, seen_since) = next_stage(record.stage, record.seen_since, &seen);
            if settling && seen.in_turn {
                // It picked it up.
                self.session_board.records[index].settling_until = 0;
            }
            if stage != record.stage || seen_since != record.seen_since {
                let record = &mut self.session_board.records[index];
                record.stage = stage;
                record.seen_since = seen_since;
                changed = true;
            }
        }
        if changed {
            save_records(&self.session_board.records);
        }
    }

    /// Sets the stage the user just moved it to, and gives the agent a minute to pick that up.
    fn set_stage(&mut self, id: u64, stage: Stage) {
        if let Some(record) = self.orchestrations().iter_mut().find(|r| r.id == id) {
            record.stage = stage;
            record.settling_until = crate::ui::now_ms() + 60_000;
        }
        save_records(&self.session_board.records);
    }

    /// The stage in words, with its colour — the same on the pane's strip and on the page.
    pub(super) fn stage_words(&self, stage: Stage, team: Team, cx: &gpui::App) -> (String, u32) {
        let (done, all) = (team.done.to_string(), (team.all + team.pending).to_string());
        match stage {
            Stage::Planning => (t(cx, "orch.stage_planning").to_string(), RUNNING),
            Stage::Ready => (t(cx, "orch.plan_ready_hint_pane").to_string(), RUNNING),
            Stage::Running if team.running > 0 => {
                (tf(cx, "orch.stage_running", &[("running", &team.running.to_string()), ("done", &done), ("all", &all)]), RUNNING)
            }
            Stage::Running if team.all + team.pending == 0 => (t(cx, "orch.stage_running_short").to_string(), RUNNING),
            Stage::Running => (tf(cx, "orch.stage_running_lead", &[("done", &done), ("all", &all)]), RUNNING),
            // Nothing known of its team (its terminal is closed): the stage alone.
            Stage::Done if team.all == 0 => (t(cx, "orch.stage_done_short").to_string(), DONE),
            Stage::Stopped if team.all == 0 => (t(cx, "orch.stage_stopped_short").to_string(), Chrome::MUTED),
            // Done: agents a crash cut off were run again, so they are not the team's any more.
            Stage::Done => {
                (tf(cx, "orch.stage_done", &[("done", &done), ("all", &(team.all - team.cut_off + team.pending).to_string())]), DONE)
            }
            Stage::Interrupted if team.cut_off > 0 => {
                (tf(cx, "orch.stage_interrupted", &[("cut", &team.cut_off.to_string()), ("done", &done), ("all", &all)]), CUT)
            }
            Stage::Interrupted => (t(cx, "orch.stage_interrupted_plain").to_string(), CUT),
            Stage::Stopped => (tf(cx, "orch.stage_stopped", &[("done", &done), ("all", &all)]), Chrome::MUTED),
        }
    }

    /// The buttons that move an orchestration on from its stage: Start, Resume or Stop all — the same
    /// on the strip and on the page. `place` keeps their ids apart.
    pub(super) fn stage_buttons(&self, id: u64, stage: Stage, place: &'static str, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let button = |key: &'static str, label: &'static str, primary: bool, tone: u32| {
            div()
                .id(SharedString::from(format!("{place}-{key}-{id}")))
                .flex_shrink_0()
                .px_2p5()
                .py_0p5()
                .rounded_md()
                .cursor_pointer()
                .t_small()
                .when(primary, |d| d.bg(hex(Chrome::ACCENT)).text_color(hex(Chrome::ON_ACCENT)))
                .when(!primary, |d| d.border_1().border_color(hex_alpha(tone, 0.6)).text_color(hex(tone)))
                .hover(|s| s.opacity(0.85))
                .child(label)
        };
        let mut buttons = Vec::new();
        match stage {
            Stage::Ready => buttons.push(
                button("start", t(cx, "orch.plan_start"), true, Chrome::BRIGHT)
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.start_orchestration(id, cx)))
                    .into_any_element(),
            ),
            Stage::Interrupted | Stage::Stopped => buttons.push(
                button("resume", t(cx, "orch.resume"), true, Chrome::BRIGHT)
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.resume_orchestration(id, window, cx)))
                    .into_any_element(),
            ),
            Stage::Planning | Stage::Running => buttons.push(
                button("stop", t(cx, "orch.stop"), false, Chrome::ERROR)
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.confirm_stop_orchestration(id, cx)))
                    .into_any_element(),
            ),
            Stage::Done => {}
        }
        buttons
    }

    /// The go the plan waits for, typed into the orchestrator's session.
    pub(super) fn start_orchestration(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(record) = self.orchestrations().iter().find(|r| r.id == id).cloned() else { return };
        let Some(pane) = self.pane_of_record(&record, cx) else { return };
        pane.update(cx, |view, cx| view.submit_prompt("Start.".to_string(), cx));
        self.set_stage(id, Stage::Running);
        self.show_toast(t(cx, "orch.plan_started"), cx);
        cx.notify();
    }

    /// Carries on after a restart, a crash or Stop: the session opened again where needed, then
    /// asked to finish what is left.
    pub(super) fn resume_orchestration(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(record) = self.orchestrations().iter().find(|r| r.id == id).cloned() else { return };
        let title: String = record.goal.lines().next().unwrap_or("").chars().take(40).collect();
        let spec = || {
            let folder = if record.folder.is_dir() { record.folder.clone() } else { crate::launch::home_dir() };
            crate::launch::LaunchSpec::resume(record.lead, record.session.clone(), title.clone(), folder)
        };
        let pane = match self.pane_of_record(&record, cx) {
            // Its agent quit: the session starts again in its place.
            Some(pane) if pane.read(cx).agent_quit() => self.replace_pane(&pane, spec(), window, cx),
            // Running, or still coming up.
            Some(pane) => Some(pane),
            None => match self.sleeping_workspace_of(&record) {
                // After a restart only the workspace in front starts: this one wakes up.
                Some(index) => {
                    self.activate_workspace(index, window, cx);
                    self.pane_of_record(&record, cx)
                }
                None => {
                    self.create_workspace(spec(), window, cx);
                    self.name_new_workspace(title.clone());
                    let group = self.orchestration_group(cx);
                    if let Some(ws) = self.workspaces.last_mut() {
                        ws.group = Some(group);
                    }
                    self.persist(cx);
                    self.active_pane()
                }
            },
        };
        let Some(pane) = pane else { return };
        self.submit_when_ready(pane, RESUME.to_string(), cx);
        self.set_stage(id, Stage::Running);
        self.show_toast(t(cx, "orch.resumed"), cx);
        cx.notify();
    }

    /// Opens a finished orchestration's session again (its terminal was closed), without asking it
    /// anything: to read it, or to ask more.
    pub(super) fn reopen_orchestration(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(record) = self.orchestrations().iter().find(|r| r.id == id).cloned() else { return };
        if let Some(pane) = self.pane_of_record(&record, cx) {
            return self.go_to_pane(&pane, window, cx);
        }
        if let Some(index) = self.sleeping_workspace_of(&record) {
            self.activate_workspace(index, window, cx);
            return cx.notify();
        }
        let title: String = record.goal.lines().next().unwrap_or("").chars().take(40).collect();
        let folder = if record.folder.is_dir() { record.folder.clone() } else { crate::launch::home_dir() };
        let spec = crate::launch::LaunchSpec::resume(record.lead, record.session.clone(), title.clone(), folder);
        self.create_workspace(spec, window, cx);
        self.name_new_workspace(title);
        let group = self.orchestration_group(cx);
        if let Some(ws) = self.workspaces.last_mut() {
            ws.group = Some(group);
        }
        self.persist(cx);
        cx.notify();
    }

    /// Types `text` into `pane` once its agent is up (at once if it already is).
    fn submit_when_ready(&mut self, pane: Pane, text: String, cx: &mut Context<Self>) {
        if pane.read(cx).live_agent.is_some() && !pane.read(cx).status.in_turn() {
            pane.update(cx, |view, cx| view.submit_prompt(text, cx));
            return;
        }
        cx.spawn(async move |_, cx| {
            for _ in 0..120 {
                cx.background_executor().timer(Duration::from_millis(500)).await;
                let ready = pane.read_with(cx, |view, _| view.live_agent.is_some() && !view.status.in_turn()).unwrap_or(false);
                if ready {
                    // The CLI draws its prompt a moment after it is seen.
                    cx.background_executor().timer(Duration::from_millis(1_500)).await;
                    let _ = pane.update(cx, |view, cx| view.submit_prompt(text, cx));
                    return;
                }
            }
        })
        .detach();
    }

    pub(super) fn confirm_stop_orchestration(&mut self, id: u64, cx: &mut Context<Self>) {
        self.ask(
            super::ask::Ask {
                title: t(cx, "orch.stop_title").into(),
                body: Some(t(cx, "orch.stop_body").into()),
                choices: vec![super::ask::AskChoice {
                    label: t(cx, "orch.stop").into(),
                    action: super::ask::AskAction::StopOrchestration(id),
                    primary: true,
                    danger: true,
                }],
                cancel: true,
            },
            cx,
        );
    }

    /// Stops the whole team: the orchestrator's turn (Esc), then every agent still running (it is
    /// told to stop them), and the parallel sessions it started.
    pub(super) fn stop_orchestration(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(record) = self.orchestrations().iter().find(|r| r.id == id).cloned() else { return };
        if let Some(pane) = self.pane_of_record(&record, cx) {
            let pane_id = pane.read(cx).pane_id;
            let (alive, in_turn) = (pane.read(cx).live_agent.is_some(), pane.read(cx).status.in_turn());
            let team = self.team_of(&record.session, Some(&pane), cx);
            if in_turn {
                pane.update(cx, |view, _| view.write(b"\x1b".to_vec()));
            }
            if alive && (in_turn || team.running > 0) {
                self.submit_when_ready(pane.clone(), STOP.to_string(), cx);
            }
            // Its `agentty tasks` sessions: their turns end too.
            let panes = self.all_panes();
            for run in self.session_board.runs.iter().filter(|r| r.caller == pane_id) {
                for task in &run.tasks {
                    if let Some(task_pane) = panes.iter().find(|p| p.read(cx).pane_id == task.pane) {
                        if task_pane.read(cx).status.in_turn() {
                            task_pane.update(cx, |view, _| view.write(b"\x1b".to_vec()));
                        }
                    }
                }
            }
        }
        self.set_stage(id, Stage::Stopped);
        self.show_toast(t(cx, "orch.stopped"), cx);
        cx.notify();
    }

    pub(super) fn confirm_delete_orchestration(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(record) = self.orchestrations().iter().find(|r| r.id == id).cloned() else { return };
        let mut choices = Vec::new();
        if self.pane_of_record(&record, cx).is_some() || self.own_workspace_of(&record, cx).is_some() {
            choices.push(super::ask::AskChoice {
                label: t(cx, "orch.delete_close").into(),
                action: super::ask::AskAction::DeleteOrchestration { id, close: true },
                primary: false,
                danger: true,
            });
        }
        choices.push(super::ask::AskChoice {
            label: t(cx, "orch.delete").into(),
            action: super::ask::AskAction::DeleteOrchestration { id, close: false },
            primary: true,
            danger: true,
        });
        self.ask(
            super::ask::Ask {
                title: t(cx, "orch.delete_title").into(),
                body: Some(t(cx, "orch.delete_body").into()),
                choices,
                cancel: true,
            },
            cx,
        );
    }

    /// Takes an orchestration off the list, stopping it first if it still runs; with `close`, its
    /// terminal closes too (its workspace, when that is all the workspace holds).
    pub(super) fn delete_orchestration(&mut self, id: u64, close: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(record) = self.orchestrations().iter().find(|r| r.id == id).cloned() else { return };
        let pane = self.pane_of_record(&record, cx);
        let own = self.own_workspace_of(&record, cx);
        if matches!(record.stage, Stage::Planning | Stage::Running) && !close {
            self.stop_orchestration(id, cx);
        }
        self.session_board.records.retain(|r| r.id != id);
        save_records(&self.session_board.records);
        if self.session_board.selected == Some(super::session_board::Thread::Orchestration(id)) {
            self.session_board.selected = None;
        }
        if close {
            // Its own workspace goes with it; a tab in another workspace closes alone.
            match (own, pane) {
                (Some(workspace), _) => self.perform_close(super::confirm::CloseTarget::Workspace(workspace), window, cx),
                (None, Some(pane)) => self.perform_close(super::confirm::CloseTarget::Tabs(vec![pane]), window, cx),
                (None, None) => {}
            }
        }
        self.show_toast(t(cx, "orch.deleted"), cx);
        cx.notify();
    }

    /// The strip across the top of an orchestrator's pane: its stage in the page's words, the way to
    /// its workflow, and what moves it on (Start, Resume, Stop all).
    pub(super) fn render_orchestration_strip(&self, pane: &Pane, cx: &mut Context<Self>) -> Option<AnyElement> {
        let record = self.record_of_pane(pane, cx)?.clone();
        let team = self.team_of(&record.session, Some(pane), cx);
        let (said, color) = self.stage_words(record.stage, team, cx);
        let ready = record.stage == Stage::Ready;
        let id = record.id;
        let tint = if record.stage == Stage::Interrupted { CUT } else { RUNNING };
        Some(
            div()
                .flex_shrink_0()
                .px_3()
                .py_1p5()
                .flex()
                .items_center()
                .gap_3()
                .border_b_1()
                .border_color(hex_alpha(tint, 0.45))
                .bg(hex_alpha(tint, if ready || record.stage == Stage::Interrupted { 0.14 } else { 0.07 }))
                .child(icon("orbit", 14., hex(tint)))
                .child(
                    div()
                        .flex_shrink_0()
                        .t_small()
                        .font_weight(crate::theme::EMPHASIS)
                        .text_color(hex(Chrome::BRIGHT))
                        .child(t(cx, if ready { "orch.plan_ready" } else { "orch.card" })),
                )
                .when(!ready, |d| d.child(div().flex_shrink_0().size(gpui::px(6.)).rounded_full().bg(hex(color))))
                .child(div().flex_1().min_w_0().truncate().t_small().text_color(hex(Chrome::FOREGROUND)).child(said))
                .child(
                    div()
                        .id(SharedString::from(format!("strip-workflow-{id}")))
                        .flex_shrink_0()
                        .px_2p5()
                        .py_0p5()
                        .rounded_md()
                        .cursor_pointer()
                        .t_small()
                        .border_1()
                        .border_color(hex_alpha(RUNNING, 0.5))
                        .text_color(hex(Chrome::BRIGHT))
                        .hover(|s| s.opacity(0.85))
                        .child(t(cx, "orch.open_page"))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.open_orchestration(id, cx))),
                )
                .children(self.stage_buttons(id, record.stage, "strip", cx))
                .into_any_element(),
        )
    }

    /// The Orchestration page, at this orchestration.
    pub(super) fn open_orchestration(&mut self, id: u64, cx: &mut Context<Self>) {
        self.session_board.selected = Some(super::session_board::Thread::Orchestration(id));
        self.session_board.tab = super::session_board::SessionTab::Workflow;
        self.open_page(super::Page::Flow, cx);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(in_turn: bool, turn_over: bool, team: (usize, usize, usize, usize)) -> Seen {
        let (running, done, cut_off, all) = team;
        Seen { alive: true, since: Some(100), in_turn, turn_over, team: Team { running, done, cut_off, all, pending: 0 }, settling: false }
    }

    #[test]
    fn it_goes_from_plan_to_done() {
        let (stage, since) = next_stage(Stage::Planning, None, &seen(true, false, (0, 0, 0, 0)));
        assert_eq!((stage, since), (Stage::Planning, Some(100)));
        assert_eq!(next_stage(stage, since, &seen(false, true, (0, 0, 0, 0))).0, Stage::Ready);
        // Changing the plan is planning again; the go starts the run.
        assert_eq!(next_stage(Stage::Ready, since, &seen(true, false, (0, 0, 0, 0))).0, Stage::Planning);
        assert_eq!(next_stage(Stage::Running, since, &seen(true, false, (0, 0, 0, 0))).0, Stage::Running);
        // The orchestrator rests while its agents work: still running.
        assert_eq!(next_stage(Stage::Running, since, &seen(false, true, (2, 1, 0, 3))).0, Stage::Running);
        assert_eq!(next_stage(Stage::Running, since, &seen(false, true, (0, 3, 0, 3))).0, Stage::Done);
        // A follow-up question runs it again.
        assert_eq!(next_stage(Stage::Done, since, &seen(true, false, (0, 3, 0, 3))).0, Stage::Running);
    }

    #[test]
    fn a_restart_while_it_ran_is_an_interruption_until_it_works_again() {
        let mut after = seen(false, false, (0, 1, 2, 3));
        after.since = Some(900);
        let (stage, since) = next_stage(Stage::Running, Some(100), &after);
        assert_eq!((stage, since), (Stage::Interrupted, Some(900)));
        // Seen again in the same process: still interrupted, even with no agent left to count.
        assert_eq!(next_stage(stage, since, &Seen { since: Some(900), ..seen(false, false, (0, 3, 0, 3)) }).0, Stage::Interrupted);
        // Resumed: running.
        assert_eq!(next_stage(stage, since, &Seen { since: Some(900), ..seen(true, false, (0, 1, 2, 3)) }).0, Stage::Running);
        // A plan waiting for the go, or a finished run, is not cut off by a restart.
        assert_eq!(next_stage(Stage::Ready, Some(100), &Seen { since: Some(900), ..seen(false, false, (0, 0, 0, 0)) }).0, Stage::Ready);
        assert_eq!(next_stage(Stage::Done, Some(100), &Seen { since: Some(900), ..seen(false, false, (0, 3, 0, 3)) }).0, Stage::Done);
        // Its terminal closed (or its agent quit) mid-run.
        assert_eq!(next_stage(Stage::Running, Some(100), &Seen::default()).0, Stage::Interrupted);
    }

    #[test]
    fn what_it_was_just_told_holds_until_it_starts_on_it() {
        let told = |s: Seen| Seen { settling: true, ..s };
        // Start pressed; the pane still shows the plan's finished turn.
        assert_eq!(next_stage(Stage::Running, Some(100), &told(seen(false, true, (0, 0, 0, 0)))).0, Stage::Running);
        // Resume: the session is still coming up (no agent yet), then up in a new process.
        assert_eq!(next_stage(Stage::Running, Some(100), &told(Seen::default())), (Stage::Running, Some(100)));
        let resumed = Seen { since: Some(900), ..told(seen(true, false, (0, 1, 2, 3))) };
        assert_eq!(next_stage(Stage::Running, Some(100), &resumed), (Stage::Running, Some(900)));
    }

    #[test]
    fn agents_cut_off_before_a_resume_do_not_keep_it_interrupted() {
        // Three cut off by a crash; after Resume the orchestrator reran them and finished its turn.
        let resumed = seen(false, true, (0, 3, 3, 6));
        assert_eq!(next_stage(Stage::Running, Some(100), &resumed).0, Stage::Done);
        // Right after the restart, resting, nothing done since: interrupted.
        assert_eq!(next_stage(Stage::Interrupted, Some(100), &seen(false, false, (0, 0, 3, 3))).0, Stage::Interrupted);
        // A finished one opened again later (a new process, resting) stays done.
        let reopened = Seen { since: Some(900), ..seen(false, false, (0, 4, 3, 7)) };
        assert_eq!(next_stage(Stage::Done, Some(100), &reopened).0, Stage::Done);
    }

    #[test]
    fn resting_after_its_report_with_the_team_done_is_done() {
        // Not in a turn, not "finished" (an agent's report came in last), nothing running.
        assert_eq!(next_stage(Stage::Running, Some(100), &seen(false, false, (0, 4, 0, 4))).0, Stage::Done);
        // Still running agents: running.
        assert_eq!(next_stage(Stage::Running, Some(100), &seen(false, false, (1, 3, 0, 4))).0, Stage::Running);
    }

    #[test]
    fn stopped_stays_stopped_while_it_winds_down() {
        assert_eq!(next_stage(Stage::Stopped, Some(100), &seen(true, false, (2, 1, 0, 3))).0, Stage::Stopped);
        assert_eq!(next_stage(Stage::Stopped, Some(100), &Seen::default()).0, Stage::Stopped);
    }

    #[test]
    fn records_read_back_as_written() {
        let record = Record {
            id: 7,
            session: "00000000-0000-0000-0000-000000000000".into(),
            lead: agentty_bridge::model::Agent::Codex,
            lead_pane: None,
            goal: "Compare note apps".into(),
            folder: PathBuf::from("/tmp/example"),
            started_ms: 1,
            stage: Stage::Interrupted,
            seen_since: Some(5),
            settling_until: 0,
            plan: parse_plan(&serde_json::json!([{ "name": "scan", "wave": 1, "model": "sonnet" }])).unwrap(),
        };
        let text = serde_json::to_string(std::slice::from_ref(&record)).unwrap();
        assert!(text.contains("\"interrupted\""));
        let back: Vec<Record> = serde_json::from_str(&text).unwrap();
        assert_eq!(back, std::slice::from_ref(&record));
        // Records written before plans existed read back with none.
        let mut old = serde_json::to_value(std::slice::from_ref(&record)).unwrap();
        old[0].as_object_mut().unwrap().remove("plan");
        old[0].as_object_mut().unwrap().remove("lead");
        let old: Vec<Record> = serde_json::from_value(old).unwrap();
        assert!(old[0].plan.is_empty());
        // and, from before a lead could be Codex, Claude Code as their lead.
        assert_eq!(old[0].lead, agentty_bridge::model::Agent::Claude);
    }

    #[test]
    fn a_plan_names_each_agent_once_by_wave_with_earlier_dependencies() {
        let plan = parse_plan(&serde_json::json!({ "agents": [
            { "name": "survey", "wave": 1, "role": "Read the current tools", "model": "opus" },
            { "name": "ideas", "wave": 1 },
            { "name": "architect", "wave": 2, "depends_on": ["survey", "ideas"] },
            { "name": "critic", "wave": 2, "agent": "codex", "depends_on": ["survey"] },
            { "name": "review", "wave": 3, "depends_on": ["architect", "critic"] },
        ]}))
        .unwrap();
        assert_eq!(plan.len(), 5);
        assert_eq!(plan[3].agent.as_deref(), Some("codex"));
        assert_eq!(plan[4].depends_on, ["architect", "critic"]);

        let bad = |value: serde_json::Value| parse_plan(&value).unwrap_err();
        assert!(bad(serde_json::json!([])).contains("no agents"));
        assert!(bad(serde_json::json!([{ "name": "a", "wave": 0 }])).contains("wave"));
        assert!(bad(serde_json::json!([{ "name": "a", "wave": 1 }, { "name": "a", "wave": 2 }])).contains("twice"));
        assert!(
            bad(serde_json::json!([{ "name": "a", "wave": 1, "depends_on": ["b"] }, { "name": "b", "wave": 1 }])).contains("earlier wave")
        );
        assert!(bad(serde_json::json!([{ "name": "a", "wave": 2, "depends_on": ["zz"] }])).contains("not in the plan"));
        assert!(bad(serde_json::json!([{ "name": " ", "wave": 1 }])).contains("name"));
        assert!(bad(serde_json::json!({ "agents": "x" })).contains("not a plan"));
    }

    #[test]
    fn a_running_team_with_agents_yet_to_start_is_not_done() {
        let mut team = Team { running: 0, done: 2, cut_off: 0, all: 2, pending: 3 };
        let seen = Seen { alive: true, since: Some(100), in_turn: false, turn_over: false, team, settling: false };
        assert_eq!(next_stage(Stage::Running, Some(100), &seen).0, Stage::Running);
        team.pending = 0;
        let seen = Seen { team, ..seen };
        assert_eq!(next_stage(Stage::Running, Some(100), &seen).0, Stage::Done);
    }
}
