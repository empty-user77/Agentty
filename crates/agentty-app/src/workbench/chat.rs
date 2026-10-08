//! Chat workspaces: the user talks to one lead agent (Claude Code) in a chat; the lead plans the
//! work and hands pieces to workers with `agentty tasks`. The workers run in their own terminals,
//! laid out in a grid below the chat, each in its own git worktree, so the user sees the whole team
//! at work while the conversation stays in one place.
//!
//! The chat is a view over a real Claude Code session: the lead runs in an ordinary terminal pane
//! (the tab's first pane), what is written in the chat is typed into it, and the conversation is
//! read back from its transcript. Its terminal is one click away, and comes up by itself while the
//! lead waits on something only its screen can answer (a tool approval, a question, first-run setup).
//!
//! The loop: workers start at once in a chat tab (the user agreed to that by opening one), at most
//! [`MAX_WORKERS`] at a time, a finished one making room for the next. When a worker ends a turn,
//! its last reply goes to the lead as a message starting with [`REPORT_MARK`]; the lead reviews it
//! and reports in the chat, or sends the worker a follow-up (`agentty tasks send`).

use super::chat_team::{folder_brief, review_prompt, ChatRoles, RoleAgent, REVIEW_PREFIX};
use super::panes::{Axis, PaneNode};
use super::{status_label_sized, Pane, Tab, Workbench};
use crate::agent_signal::{browser_reply, TasksCtlRequest, TasksRequest};
use crate::i18n::{t, tf};
use crate::launch::{LaunchChoice, LaunchSpec, PaneKind};
use crate::terminal::AgentStatus;
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, TypeScale};
use agentty_bridge::model::{Role, Turn};
use gpui::{
    div, list, prelude::*, px, AnyElement, App, ClickEvent, Context, Entity, EntityId, ListAlignment, ListState, SharedString,
    Subscription, Task, Window,
};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

/// Workers on screen at once, in a grid of two columns.
pub const MAX_WORKERS: usize = 4;
/// Marks the lead's instructions in the first message, which the chat leaves out.
const BRIEF_START: &str = "<!-- agentty:chat-lead -->";
const BRIEF_END: &str = "<!-- /agentty:chat-lead -->";
/// Starts every message Agentty sends the lead about its workers.
const REPORT_MARK: &str = "[Agentty]";
/// How often the lead's transcript is checked for news.
const WATCH_INTERVAL: Duration = Duration::from_millis(700);
/// How much of a worker's last reply goes into its report.
const REPORT_REPLY_LIMIT: usize = 4_000;
/// The chat's share of the height once workers are below it.
const DEFAULT_TOP: f32 = 0.6;
/// The chat never gets less height than this while workers share the tab with it.
pub const CHAT_MIN_HEIGHT: f32 = 360.;
/// Tool steps of one reply shown before the rest fold into a count.
const TOOL_STEPS_SHOWN: usize = 3;

/// What the lead is told before the user's first message. English, like every prompt Agentty
/// ships; the last line names the language to talk in.
const BRIEF: &str = "\
You are the lead agent of an Agentty chat workspace. The user talks to you in this chat. You plan the work and hand \
pieces of it to worker agents; each worker runs in a terminal of its own, which the user watches in a grid below the chat.

{folder}

How to work:
- Answer questions and make small changes yourself.
- For anything bigger, make a short plan, then start workers with `agentty tasks` (`agentty tasks --plan <plan.json>` \
with a JSON array of {\"title\", \"prompt\", \"agent\"}, or `agentty tasks --title <title> --prompt-file <file>`). Here \
they start at once, without asking the user again. At most 4 run at a time, reviewers included; a finished one makes \
room for a new one. Each worker gets its own git worktree on a new branch. Leave \"agent\" out to use the agent the \
user chose for this chat's workers.
- Every worker prompt must stand on its own: the goal, the files involved, the constraints, how to verify, and to commit \
its work on its branch when done. Workers do not push or open pull requests unless the user asked for that.
- After starting workers, tell the user in a few lines who does what, then end your turn. Do not wait, poll or sleep.
- When a worker ends a turn, Agentty sends you a message that starts with \"[Agentty]\", with the worker's title, \
branch, folder and last reply (and whether reviews are on). Review its work (read the diff in its folder), then merge \
it, report to the user, or give the worker a follow-up with `agentty tasks send --to \"<title>\" --prompt \"<text>\"`.
- `agentty tasks status` lists your workers; `agentty tasks result --to \"<title>\"` prints a worker's last reply again; \
`agentty tasks stop --to \"<title>\"` closes a worker you no longer need (its branch stays); \
`agentty tasks review --worker \"<title>\"` starts a reviewer that reads the worker's work without editing it and \
reports back like a worker.
- Keep replies in the chat short: what happened, what comes next, what you need from the user.

Talk to the user in {language}.";

/// One entry of the chat.
#[derive(Debug, Clone, PartialEq)]
pub enum ChatItem {
    User(String),
    Lead(String),
    /// A worker's report to the lead (sent by Agentty).
    Report(String),
}

pub struct ChatState {
    pub lead: Pane,
    /// The agents its workers and reviewers run as.
    roles: ChatRoles,
    input: Option<(Entity<TextInput>, Subscription)>,
    /// From the transcript.
    items: Vec<ChatItem>,
    /// Messages sent from the chat that the transcript does not show yet.
    echo: Vec<String>,
    /// `items` and then `echo`, as the list shows them.
    shown: Rc<Vec<ChatItem>>,
    list: ListState,
    /// The list follows the newest entry: the user has not scrolled up to read something older.
    follow: Rc<std::cell::Cell<bool>>,
    /// The lead has had its instructions (they are in its transcript, or were just sent).
    briefed: bool,
    /// The user asked to see the lead's terminal.
    show_terminal: bool,
    /// The lead's screen shows something the chat can't answer (folder trust, sign-in).
    setup_screen: bool,
    /// Reports waiting while the lead waits on the user (an approval, a question).
    pending: Vec<String>,
    transcript: Option<(String, PathBuf)>,
    seen_len: u64,
    loading: bool,
    _watch: Task<()>,
}

/// The lead's first message: its instructions (with where it works, see [`folder_brief`]), then what
/// the user wrote.
fn briefed_prompt(text: &str, language: &str, folder: &str) -> String {
    format!("{BRIEF_START}\n{}\n{BRIEF_END}\n\n{text}", BRIEF.replace("{language}", language).replace("{folder}", folder))
}

/// Waits (a few seconds at most) until `path` stops growing: the turn's last lines reach the
/// transcript a moment after the hook that says it ended.
fn settle(path: &std::path::Path) {
    let len = || std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let mut last = len();
    for _ in 0..10 {
        std::thread::sleep(Duration::from_millis(400));
        let now = len();
        if now == last {
            return;
        }
        last = now;
    }
}

/// The last thing `agent` said in session `session`, whole and without its tool steps; else
/// `fallback` (the first line its hook reported). Reads a transcript: call it off the main thread.
fn last_reply(agent: Option<agentty_bridge::model::Agent>, session: Option<String>, fallback: Option<String>) -> Option<String> {
    let from_transcript = agent.zip(session).and_then(|(agent, id)| {
        let path = agentty_bridge::transcript_path(agent, &id)?;
        settle(&path);
        let (_, turns) = agentty_bridge::load(agent, &id).ok()?;
        let last = turns.into_iter().rev().find(|turn| turn.role == Role::Assistant)?;
        let text: Vec<&str> = last.text.lines().filter(|line| !line.starts_with("[tool: ")).collect();
        Some(text.join("\n").trim().to_string()).filter(|text| !text.is_empty())
    });
    from_transcript.or(fallback)
}

/// Text typed in as a paste, without the `<pasted_content id="…">` tags Claude Code keeps around it.
fn without_paste_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<pasted_content").into_iter().chain(rest.find("</pasted_content")).min() {
        out.push_str(&rest[..start]);
        match rest[start..].find('>') {
            Some(end) => rest = &rest[start + end + 1..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The chat as the transcript tells it: what the user wrote (without the lead's instructions),
/// what the lead answered, and the workers' reports. Also whether the lead was briefed.
pub fn chat_items(turns: &[Turn]) -> (Vec<ChatItem>, bool) {
    let mut briefed = false;
    let mut items = Vec::new();
    for turn in turns {
        match turn.role {
            Role::User => {
                let unwrapped = without_paste_tags(&turn.text);
                let mut text = unwrapped.trim();
                if let Some((_, rest)) = text.split_once(BRIEF_END) {
                    briefed = true;
                    text = rest.trim();
                } else if text.contains(BRIEF_START) {
                    briefed = true;
                    continue;
                }
                if text.is_empty() {
                    continue;
                }
                items.push(match text.strip_prefix(REPORT_MARK) {
                    Some(report) => ChatItem::Report(report.trim().to_string()),
                    None => ChatItem::User(text.to_string()),
                });
            }
            Role::Assistant => items.push(ChatItem::Lead(turn.text.clone())),
        }
    }
    (items, briefed)
}

/// The tab's layout: the lead on top, its workers below in rows of two.
pub fn chat_tree<T: Clone + PartialEq>(lead: T, workers: Vec<T>, top: f32) -> PaneNode<T> {
    if workers.is_empty() {
        return PaneNode::Leaf(lead);
    }
    let mut rows: Vec<PaneNode<T>> = workers
        .chunks(2)
        .map(|pair| match pair {
            [one] => PaneNode::Leaf(one.clone()),
            _ => PaneNode::Split {
                axis: Axis::Horizontal,
                children: pair.iter().cloned().map(PaneNode::Leaf).collect(),
                sizes: vec![1. / pair.len() as f32; pair.len()],
            },
        })
        .collect();
    let grid = if rows.len() == 1 {
        rows.remove(0)
    } else {
        let share = 1. / rows.len() as f32;
        PaneNode::Split { axis: Axis::Vertical, sizes: vec![share; rows.len()], children: rows }
    };
    let top = top.clamp(0.2, 0.85);
    PaneNode::Split { axis: Axis::Vertical, children: vec![PaneNode::Leaf(lead), grid], sizes: vec![top, 1. - top] }
}

/// The chat's share of the height as the user left it (they may have dragged the divider).
fn top_share<T: Clone + PartialEq>(root: &PaneNode<T>, lead: &T) -> f32 {
    match root {
        PaneNode::Split { axis: Axis::Vertical, children, sizes } if matches!(children.first(), Some(PaneNode::Leaf(l)) if l == lead) => {
            let total: f32 = sizes.iter().sum();
            if total > 0. {
                sizes[0] / total
            } else {
                DEFAULT_TOP
            }
        }
        _ => DEFAULT_TOP,
    }
}

/// The chat entries that changed: the list re-measures only those.
fn splice_list(list: &ListState, old: &[ChatItem], new: &[ChatItem]) {
    let common = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    if common == old.len() && common == new.len() {
        return;
    }
    list.splice(common..old.len(), new.len() - common);
}

/// A first-run screen of Claude Code that only its terminal can answer.
fn is_setup_screen(lines: &[String]) -> bool {
    let screen = lines.join("\n").to_lowercase();
    ["trust the files", "trust this folder", "select login method", "choose the text style"].iter().any(|s| screen.contains(s))
}

/// Whether `view` waits on the user: its status says so, or its screen shows a selection (an
/// approval, a question, a first-run question such as Codex's "Trust this folder?" in a new
/// worktree) the hooks have not reported. Text typed in then, with its Enter, would pick whatever
/// option is highlighted.
fn waits_on_user(view: &crate::terminal::TerminalView) -> bool {
    if view.status.needs_user() {
        return true;
    }
    let lines = view.screen_lines(40);
    let screen = crate::terminal::classify_screen(&lines);
    screen.permission.is_some() || screen.question || is_setup_screen(&lines)
}

fn status_word(status: &AgentStatus, running: bool) -> &'static str {
    if !running {
        return "exited";
    }
    match status {
        AgentStatus::Idle => "idle",
        AgentStatus::Working => "working",
        AgentStatus::Thinking => "thinking",
        AgentStatus::Finished(_) => "finished",
        AgentStatus::Permission(_) => "waiting for the user's approval",
        AgentStatus::Question(_) => "waiting for the user's answer",
        AgentStatus::Interrupted => "interrupted",
    }
}

fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(limit).collect();
    cut.push_str("\n[…]");
    cut
}

impl Workbench {
    /// A new workspace in `cwd` whose first tab is a chat. In a git repository the lead works in a
    /// worktree of its own, on a new branch: the chat's integration branch, which its workers start
    /// from and are merged back into, away from the project folder until the user asks for the
    /// result. A repository without a commit has nothing to branch from: the lead works in it.
    pub(super) fn create_chat_workspace(&mut self, cwd: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        crate::metrics::track(cx, "feature_used", serde_json::json!({ "feature": "chat_workspace" }));
        let tree = agentty_bridge::worktree::tree_root(&cwd).and_then(|_| agentty_bridge::worktree::create(&cwd, "chat").ok());
        let folder = tree.as_ref().map(|t| t.path.clone()).unwrap_or_else(|| cwd.clone());
        if tree.is_some() {
            self.refresh_files_panel(cx);
        }
        self.create_workspace(LaunchChoice::Chat.spec(folder), window, cx);
        // The workspace is the project's, wherever its lead works.
        if let Some(ws) = self.workspaces.last_mut() {
            ws.cwd = cwd;
        }
        let lead = self.workspaces.last().and_then(|ws| ws.tabs.first()).map(|tab| tab.active.clone());
        if let Some(lead) = lead {
            self.register_chat(&lead, ChatRoles::default(), cx);
            // The composer is made at the next frame; the keyboard goes there then.
            self.refocus = true;
        }
        self.persist(cx);
        cx.notify();
    }

    /// Makes `lead` the lead of a chat (its tab's first pane).
    pub(super) fn register_chat(&mut self, lead: &Pane, roles: ChatRoles, cx: &mut Context<Self>) {
        let id = lead.entity_id();
        if self.chats.contains_key(&id) {
            return;
        }
        let watch = cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(WATCH_INTERVAL).await;
            match this.update(cx, |this, cx| this.poll_chat(id, cx)) {
                Ok(true) => {}
                _ => break,
            }
        });
        let list = ListState::new(0, ListAlignment::Bottom, px(600.));
        let follow = Rc::new(std::cell::Cell::new(true));
        // At the bottom the list sticks to it; scrolled up, it stays where the user is reading.
        let following = follow.clone();
        list.set_scroll_handler(move |event, _, _| following.set(!event.is_scrolled));
        self.chats.insert(
            id,
            ChatState {
                lead: lead.clone(),
                roles,
                input: None,
                items: Vec::new(),
                echo: Vec::new(),
                shown: Rc::new(Vec::new()),
                list,
                follow,
                briefed: false,
                show_terminal: false,
                setup_screen: false,
                pending: Vec::new(),
                transcript: None,
                seen_len: 0,
                loading: false,
                _watch: watch,
            },
        );
    }

    /// The choices of the chat led by `lead`.
    pub(super) fn chat_roles(&self, lead: &Pane) -> Option<ChatRoles> {
        self.chats.get(&lead.entity_id()).map(|state| state.roles)
    }

    fn set_chat_roles(&mut self, id: EntityId, change: impl FnOnce(ChatRoles, bool) -> ChatRoles, cx: &mut Context<Self>) {
        let codex = self.is_installed("codex");
        let Some(state) = self.chats.get_mut(&id) else { return };
        state.roles = change(state.roles, codex);
        self.persist(cx);
        cx.notify();
    }

    /// The branch the lead works on, and whether it is the chat's own (integration) branch — the
    /// lead in a linked worktree — rather than the project folder's.
    fn lead_branch(lead: &Pane, cx: &App) -> (Option<String>, bool) {
        let view = lead.read(cx);
        (view.git_branch.clone(), view.worktree.is_some())
    }

    /// Whether `ws` holds a chat: a live chat tab, or one in its saved layout while it sleeps.
    pub(super) fn is_chat_workspace(&self, ws: &super::Workspace) -> bool {
        ws.tabs.iter().any(|tab| self.chat_lead_of_tab(tab).is_some())
            || ws.dormant.as_ref().is_some_and(|dormant| dormant.tabs.iter().any(|tab| tab.chat))
    }

    /// The lead of `tab` when it is a chat tab.
    pub(super) fn chat_lead_of_tab(&self, tab: &Tab) -> Option<Pane> {
        tab.root.leaves().into_iter().next().filter(|lead| self.chats.contains_key(&lead.entity_id()))
    }

    /// The lead of the chat tab `pane` is in (the pane itself when it is the lead).
    fn chat_lead_of(&self, pane: &Pane) -> Option<Pane> {
        let (w, t) = self.locate(pane)?;
        self.chat_lead_of_tab(&self.workspaces[w].tabs[t])
    }

    /// The title a chat's lead gave `pane`, when `pane` is one of its workers.
    pub(super) fn chat_worker_title(&self, pane: &Pane, cx: &App) -> Option<String> {
        self.chat_lead_of(pane).filter(|lead| lead != pane).map(|_| pane.read(cx).spec.title.clone())
    }

    /// The workers of the chat led by `lead`: the other panes of its tab.
    fn chat_workers(&self, lead: &Pane) -> Vec<Pane> {
        let Some((w, t)) = self.locate(lead) else { return Vec::new() };
        self.workspaces[w].tabs[t].root.leaves().into_iter().filter(|p| p != lead).collect()
    }

    /// Whether the chat is on screen for `lead` (and not its terminal).
    fn chat_in_front(&self, state: &ChatState, cx: &App) -> bool {
        !(state.show_terminal || state.setup_screen || state.lead.read(cx).status.needs_user())
    }

    /// The composer to put the keyboard in instead of `pane`, when `pane` is a lead shown as a chat.
    pub(super) fn chat_input_for(&self, pane: &Pane, cx: &App) -> Option<Entity<TextInput>> {
        let state = self.chats.get(&pane.entity_id())?;
        if !self.chat_in_front(state, cx) {
            return None;
        }
        state.input.as_ref().map(|(input, _)| input.clone())
    }

    /// Forgets chats whose lead is gone, and gives new ones their composer.
    pub(super) fn prepare_chats(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.chats.is_empty() {
            return;
        }
        let alive: std::collections::HashSet<EntityId> = self.all_panes().iter().map(|p| p.entity_id()).collect();
        self.chats.retain(|id, _| alive.contains(id));
        let missing: Vec<EntityId> = self.chats.iter().filter(|(_, s)| s.input.is_none()).map(|(id, _)| *id).collect();
        for id in missing {
            let input = cx.new(|cx| TextInput::localized("", "chat.placeholder", window, cx).composer(2));
            let subscription = cx.subscribe_in(&input, window, move |this, _, event: &TextInputEvent, window, cx| {
                if let TextInputEvent::Confirmed = event {
                    this.send_chat_message(id, window, cx);
                }
            });
            if let Some(state) = self.chats.get_mut(&id) {
                state.input = Some((input, subscription));
            }
        }
    }

    /// Reads the lead's transcript again when it grew. False once the chat is gone (the watch ends).
    fn poll_chat(&mut self, id: EntityId, cx: &mut Context<Self>) -> bool {
        let Some(state) = self.chats.get_mut(&id) else { return false };
        let view = state.lead.read(cx);
        // Until the lead has its instructions, its first-run screens may be in the way.
        let setup = !state.briefed && is_setup_screen(&view.screen_lines(40));
        let session = view.current_session();
        if setup != state.setup_screen {
            state.setup_screen = setup;
            cx.notify();
        }
        if state.loading {
            return true;
        }
        let Some(session) = session else { return true };
        let known = state.transcript.as_ref().filter(|(s, _)| *s == session).map(|(_, path)| path.clone());
        let seen = if known.is_some() { state.seen_len } else { u64::MAX };
        state.loading = true;
        let task = cx.background_spawn(async move {
            let path = match known {
                Some(path) => path,
                None => agentty_bridge::claude::find(&session).ok()?,
            };
            let len = std::fs::metadata(&path).ok()?.len();
            if len == seen {
                return Some((session, path, len, None));
            }
            let turns = agentty_bridge::claude::transcript(&path).ok().map(|(_, turns)| turns);
            Some((session, path, len, turns))
        });
        cx.spawn(async move |this, cx| {
            let loaded = task.await;
            let _ = this.update(cx, |this, cx| this.apply_chat_load(id, loaded, cx));
        })
        .detach();
        true
    }

    #[allow(clippy::type_complexity)]
    fn apply_chat_load(&mut self, id: EntityId, loaded: Option<(String, PathBuf, u64, Option<Vec<Turn>>)>, cx: &mut Context<Self>) {
        let Some(state) = self.chats.get_mut(&id) else { return };
        state.loading = false;
        let Some((session, path, len, turns)) = loaded else { return };
        state.transcript = Some((session, path));
        state.seen_len = len;
        let Some(turns) = turns else { return };
        let (items, briefed) = chat_items(&turns);
        state.briefed |= briefed;
        // What the transcript shows now no longer needs its echo.
        state.echo.retain(|sent| !items.iter().rev().take(8).any(|item| matches!(item, ChatItem::User(text) if text == sent)));
        state.items = items;
        Self::show_chat_items(state);
        cx.notify();
    }

    fn show_chat_items(state: &mut ChatState) {
        let mut shown = state.items.clone();
        shown.extend(state.echo.iter().cloned().map(ChatItem::User));
        if state.follow.get() {
            // Measured again from the end, so a reply that keeps growing stays in view.
            if shown != *state.shown {
                state.list.reset(shown.len());
            }
        } else {
            splice_list(&state.list, &state.shown, &shown);
        }
        state.shown = Rc::new(shown);
    }

    /// Sends what was written in the chat to the lead (with its instructions, the first time).
    fn send_chat_message(&mut self, id: EntityId, _window: &mut Window, cx: &mut Context<Self>) {
        let Some((input, _)) = self.chats.get(&id).and_then(|state| state.input.as_ref()) else { return };
        let input = input.clone();
        let text = input.read(cx).text().trim().to_string();
        if text.is_empty() {
            return;
        }
        input.update(cx, |input, cx| input.set_text("", cx));
        self.say_to_lead(id, text, cx);
    }

    /// Types what the user wrote into the lead (with its instructions in front, the first time) and
    /// shows it in the chat until the transcript does.
    fn say_to_lead(&mut self, id: EntityId, text: String, cx: &mut Context<Self>) {
        let language = agentty_bridge::idea::language_name(crate::settings::settings(cx).language.resolved().code());
        let Some(state) = self.chats.get_mut(&id) else { return };
        // A slash command is for Claude Code itself, not a message to brief the lead with.
        let prompt = if state.briefed || text.starts_with('/') {
            text.clone()
        } else {
            briefed_prompt(&text, language, &Self::lead_folder_brief(&state.lead, cx))
        };
        state.briefed |= !text.starts_with('/');
        state.echo.push(text);
        // What the user just wrote is what they look at: back to the newest entry.
        state.follow.set(true);
        Self::show_chat_items(state);
        // Never into a selection on the lead's screen: it goes in once that is answered.
        if waits_on_user(state.lead.read(cx)) || !state.pending.is_empty() {
            state.pending.push(prompt);
        } else {
            state.lead.update(cx, |view, cx| view.submit_prompt(prompt, cx));
        }
        cx.notify();
    }

    /// Where the lead works, for its instructions. Asks git once (the first message only).
    fn lead_folder_brief(lead: &Pane, cx: &App) -> String {
        let view = lead.read(cx);
        let cwd = view.display_cwd();
        let linked = agentty_bridge::worktree::tree_root(&cwd).is_some_and(|root| agentty_bridge::worktree::is_linked(&root));
        if !linked {
            return folder_brief(None);
        }
        let branch = view.git_branch.clone().or_else(|| agentty_bridge::git::head(&cwd).and_then(|(branch, _)| branch));
        let project = agentty_bridge::worktree::main_tree(&cwd);
        match (branch, project) {
            (Some(branch), Some(project)) => folder_brief(Some((&branch, &project))),
            _ => folder_brief(None),
        }
    }

    /// A message for the lead from Agentty. Claude Code takes messages typed during a turn and reads
    /// them when it is done, so it goes in now — unless the lead waits on the user (an approval, a
    /// question): typed into that selection it would answer it. Then it waits until that is over.
    fn deliver_to_lead(&mut self, id: EntityId, text: String, cx: &mut Context<Self>) {
        let Some(state) = self.chats.get_mut(&id) else { return };
        if waits_on_user(state.lead.read(cx)) || !state.pending.is_empty() {
            state.pending.push(text);
            return;
        }
        state.lead.update(cx, |view, cx| view.submit_prompt(text, cx));
    }

    /// The lead no longer waits on the user: what waited for it goes in, as one message.
    pub(super) fn flush_chat_pending(&mut self, pane: &Pane, cx: &mut Context<Self>) {
        let Some(state) = self.chats.get_mut(&pane.entity_id()) else { return };
        if state.pending.is_empty() || waits_on_user(state.lead.read(cx)) {
            return;
        }
        let text = std::mem::take(&mut state.pending).join("\n\n");
        state.lead.update(cx, |view, cx| view.submit_prompt(text, cx));
    }

    /// A pane ended a turn: a lead takes what waited for it; a worker reports to its lead.
    pub(super) fn chat_turn_finished(&mut self, pane: &Pane, message: Option<String>, cx: &mut Context<Self>) {
        if self.chats.contains_key(&pane.entity_id()) {
            return self.flush_chat_pending(pane, cx);
        }
        let Some(lead) = self.chat_lead_of(pane) else { return };
        let view = pane.read(cx);
        let title = view.spec.title.clone();
        let branch = view.git_branch.clone();
        let folder = view.display_cwd();
        let (agent, session) = (view.agent_kind().and_then(PaneKind::agent), view.current_session());
        let lead_id = lead.entity_id();
        let reviewer = title.starts_with(REVIEW_PREFIX);
        // Reviews are asked for in the report itself, so a choice changed mid-chat counts at once.
        let hint = if reviewer { None } else { self.chat_roles(&lead).and_then(|roles| roles.review_hint(&title)) };
        let task = cx.background_spawn(async move { last_reply(agent, session, message) });
        cx.spawn(async move |this, cx| {
            let reply = task.await.unwrap_or_else(|| "(no reply text found)".into());
            let who = if reviewer { "Reviewer" } else { "Worker" };
            let mut report = format!("{REPORT_MARK} {who} \"{title}\" finished its turn.\n");
            if let Some(branch) = branch {
                report.push_str(&format!("Branch: {branch}\n"));
            }
            report.push_str(&format!("Folder: {}\nIts last reply:\n{}", folder.display(), truncate(&reply, REPORT_REPLY_LIMIT)));
            if let Some(hint) = hint {
                report.push_str(&format!("\n\n{hint}"));
            }
            let _ = this.update(cx, |this, cx| this.deliver_to_lead(lead_id, report, cx));
        })
        .detach();
    }

    /// The lead of the chat whose lead pane is `pane_id`.
    fn chat_by_pane_id(&self, pane_id: u64, cx: &App) -> Option<EntityId> {
        self.chats.iter().find(|(_, s)| s.lead.read(cx).pane_id == pane_id).map(|(id, _)| *id)
    }

    /// `agentty tasks` from a chat's lead: the workers start at once, in the grid below the chat.
    /// Returns the request when its asker is not a lead.
    pub(super) fn start_chat_tasks(&mut self, request: TasksRequest, cx: &mut Context<Self>) -> Option<TasksRequest> {
        let Some(id) = self.chat_by_pane_id(request.pane, cx) else { return Some(request) };
        let (cwd, tasks) = (request.cwd.clone(), request.tasks.clone());
        let (lead, roles) = match self.chats.get(&id) {
            Some(state) => (state.lead.clone(), state.roles),
            None => return Some(request),
        };
        // On the chat's own branch, workers build on what was merged into it so far.
        let base = match Self::lead_branch(&lead, cx) {
            (Some(branch), true) => Some(branch),
            _ => None,
        };
        cx.spawn(async move |this, cx| {
            let mut started: Vec<serde_json::Value> = Vec::new();
            let mut problems: Vec<String> = Vec::new();
            for task in tasks {
                // No tree is made for a task that would find no room.
                if !this.read_with(cx, |this, cx| this.chat_has_room(id, None, cx)).unwrap_or(false) {
                    problems.push(format!("{}: all {MAX_WORKERS} workers are busy; start it when one of them has finished", task.title));
                    continue;
                }
                let in_git = agentty_bridge::worktree::tree_root(&cwd).is_some();
                let (source, label, base) = (cwd.clone(), task.title.clone(), base.clone());
                let tree = if in_git {
                    let made = cx.background_spawn(async move {
                        match base {
                            Some(base) => agentty_bridge::worktree::create_from(&source, &label, &base),
                            None => agentty_bridge::worktree::create(&source, &label),
                        }
                    });
                    match made.await {
                        Ok(tree) => Some(tree),
                        Err(err) => {
                            problems.push(format!("{}: {err:#}", task.title));
                            continue;
                        }
                    }
                } else {
                    None
                };
                let folder = tree.as_ref().map(|t| t.path.clone()).unwrap_or_else(|| cwd.clone());
                let branch = tree.as_ref().and_then(|t| t.branch.clone());
                let agent = task.agent.as_deref().and_then(RoleAgent::from_label).unwrap_or(roles.worker);
                let spec = LaunchSpec::with_prompt(agent.agent(), task.prompt.clone(), task.title.clone(), folder.clone());
                match this.update(cx, |this, cx| this.add_chat_worker(id, spec, None, cx)) {
                    Ok(Ok(replaced)) => {
                        started.push(serde_json::json!({ "title": task.title, "branch": branch, "folder": folder, "replaced": replaced }));
                        if tree.is_none() {
                            problems.push(format!(
                                "{}: {} is not in a git repository, so the worker shares that folder with you",
                                task.title,
                                folder.display()
                            ));
                        }
                    }
                    Ok(Err(problem)) => problems.push(format!("{}: {problem}", task.title)),
                    Err(_) => problems.push(format!("{}: Agentty's window is gone", task.title)),
                }
            }
            let answer = if started.is_empty() {
                browser_reply(Err(if problems.is_empty() { "nothing was started".into() } else { problems.join("; ") }))
            } else {
                browser_reply(Ok(serde_json::json!({ "started": started, "problems": problems }).to_string()))
            };
            let _ = request.reply.send(answer);
            let _ = this.update(cx, |this, cx| {
                this.refresh_files_panel(cx);
                cx.notify();
            });
        })
        .detach();
        None
    }

    /// A worker that is done (not in a turn, not waiting on the user) can make room for a new one —
    /// never `keep` (the worker a new reviewer is about to look at).
    fn idle_worker(&self, lead: &Pane, keep: Option<&Pane>, cx: &App) -> Option<Pane> {
        self.chat_workers(lead)
            .into_iter()
            .filter(|p| Some(p) != keep)
            .filter(|p| {
                let view = p.read(cx);
                !view.status.in_turn() && !waits_on_user(view)
            })
            .min_by_key(|p| p.read(cx).launched_at_ms)
    }

    fn chat_has_room(&self, id: EntityId, keep: Option<&Pane>, cx: &App) -> bool {
        let Some(state) = self.chats.get(&id) else { return false };
        self.chat_workers(&state.lead).len() < MAX_WORKERS || self.idle_worker(&state.lead, keep, cx).is_some()
    }

    /// Adds a worker running `spec` to the grid of chat `id`; with every place taken, the worker
    /// that finished longest ago closes for it (its branch and session stay). Returns the title of
    /// the worker it replaced.
    fn add_chat_worker(
        &mut self,
        id: EntityId,
        spec: LaunchSpec,
        keep: Option<&Pane>,
        cx: &mut Context<Self>,
    ) -> Result<Option<String>, String> {
        let lead = self.chats.get(&id).map(|s| s.lead.clone()).ok_or("the chat was closed")?;
        let mut workers = self.chat_workers(&lead);
        let mut replaced = None;
        if workers.len() >= MAX_WORKERS {
            let old = self.idle_worker(&lead, keep, cx).ok_or(format!("all {MAX_WORKERS} workers are busy"))?;
            replaced = Some(old.read(cx).spec.title.clone());
            self.remove_pane(&old, cx);
            workers.retain(|p| *p != old);
        }
        let (w, t) = self.locate(&lead).ok_or("the chat was closed")?;
        let pane = self.spawn_pane(spec, cx);
        workers.push(pane);
        let tab = &mut self.workspaces[w].tabs[t];
        let top = top_share(&tab.root, &lead);
        tab.root = chat_tree(lead, workers, top);
        self.persist(cx);
        cx.notify();
        Ok(replaced)
    }

    /// `agentty tasks status|send|result|stop|review` from a chat's lead.
    pub fn answer_tasks_ctl(&mut self, request: TasksCtlRequest, cx: &mut Context<Self>) {
        if request.ctl.action == "result" {
            return self.answer_result(request, cx);
        }
        let answer = self.tasks_ctl(&request, cx);
        let _ = request.reply.send(browser_reply(answer));
    }

    /// The lead's chat and the worker `to` names in it.
    fn lead_and_worker(&self, request: &TasksCtlRequest, cx: &App) -> Result<(EntityId, Pane, Pane), String> {
        let id = self
            .chat_by_pane_id(request.pane, cx)
            .ok_or("only the lead agent of an Agentty chat has workers (open a chat workspace to use this)")?;
        let lead = self.chats.get(&id).map(|s| s.lead.clone()).ok_or("the chat was closed")?;
        let workers = self.chat_workers(&lead);
        let to = request.ctl.to.as_str();
        let worker = workers.iter().find(|p| p.read(cx).spec.title.eq_ignore_ascii_case(to)).cloned().ok_or_else(|| {
            let names: Vec<String> = workers.iter().map(|p| format!("\"{}\"", p.read(cx).spec.title)).collect();
            format!("no worker \"{to}\" (workers: {})", if names.is_empty() { "none".into() } else { names.join(", ") })
        })?;
        Ok((id, lead, worker))
    }

    /// `agentty tasks result`: the worker's last reply, read from its transcript in the background.
    fn answer_result(&mut self, request: TasksCtlRequest, cx: &mut Context<Self>) {
        let (_, _, worker) = match self.lead_and_worker(&request, cx) {
            Ok(found) => found,
            Err(problem) => {
                let _ = request.reply.send(browser_reply(Err(problem)));
                return;
            }
        };
        let view = worker.read(cx);
        let title = view.spec.title.clone();
        let (agent, session) = (view.agent_kind().and_then(PaneKind::agent), view.current_session());
        let fallback = match &view.status {
            AgentStatus::Finished(Some(message)) => Some(message.clone()),
            _ => None,
        };
        let status = status_word(&view.status, view.is_running());
        cx.background_spawn(async move {
            let reply = last_reply(agent, session, fallback);
            let answer = serde_json::json!({ "title": title, "status": status, "reply": reply });
            let _ = request.reply.send(browser_reply(Ok(answer.to_string())));
        })
        .detach();
    }

    fn tasks_ctl(&mut self, request: &TasksCtlRequest, cx: &mut Context<Self>) -> Result<String, String> {
        if request.ctl.action == "status" {
            let id = self
                .chat_by_pane_id(request.pane, cx)
                .ok_or("only the lead agent of an Agentty chat has workers (open a chat workspace to use this)")?;
            let lead = self.chats.get(&id).map(|s| s.lead.clone()).ok_or("the chat was closed")?;
            let roles = self.chat_roles(&lead).unwrap_or_default();
            let list: Vec<serde_json::Value> = self
                .chat_workers(&lead)
                .iter()
                .map(|pane| {
                    let view = pane.read(cx);
                    serde_json::json!({
                        "title": view.spec.title,
                        "role": if view.spec.title.starts_with(REVIEW_PREFIX) { "reviewer" } else { "worker" },
                        "agent": crate::brand::kind_id(view.spec.kind),
                        "status": status_word(&view.status, view.is_running()),
                        "branch": view.git_branch,
                        "folder": view.display_cwd(),
                    })
                })
                .collect();
            let (branch, integration) = Self::lead_branch(&lead, cx);
            return Ok(serde_json::json!({
                "workers": list,
                "max": MAX_WORKERS,
                "workerAgent": roles.worker.label(),
                "reviewerAgent": roles.reviewer.map(RoleAgent::label),
                "leadBranch": branch,
                "integrationBranch": integration,
            })
            .to_string());
        }
        let (id, lead, worker) = self.lead_and_worker(request, cx)?;
        let to = worker.read(cx).spec.title.clone();
        match request.ctl.action.as_str() {
            "send" => {
                if !worker.read(cx).is_running() {
                    return Err(format!("worker \"{to}\" is no longer running"));
                }
                // Typed into an approval or a question, the message's Enter would answer it.
                if waits_on_user(worker.read(cx)) {
                    return Err(format!(
                        "worker \"{to}\" is waiting for the user (an approval or a question in its terminal); tell the user, and send this once it is answered"
                    ));
                }
                let text = request.ctl.prompt.trim().to_string();
                worker.update(cx, |view, cx| view.submit_prompt(text, cx));
                Ok(serde_json::json!({ "sent": to }).to_string())
            }
            "stop" => {
                self.remove_pane(&worker, cx);
                self.persist(cx);
                cx.notify();
                Ok(serde_json::json!({ "stopped": to, "note": "its branch and session stay" }).to_string())
            }
            "review" => {
                if to.starts_with(REVIEW_PREFIX) {
                    return Err(format!("\"{to}\" is a reviewer; review the worker it looked at instead"));
                }
                if !self.chat_has_room(id, Some(&worker), cx) {
                    return Err(format!("all {MAX_WORKERS} places are busy; start the review when one of them has finished"));
                }
                let roles = self.chat_roles(&lead).unwrap_or_default();
                // Asked for by name, a review runs even with reviews off: the choice decides the agent.
                let agent = request.ctl.agent.as_deref().and_then(RoleAgent::from_label).or(roles.reviewer).unwrap_or_default();
                let view = worker.read(cx);
                let (folder, branch) = (view.display_cwd(), view.git_branch.clone());
                let base = match Self::lead_branch(&lead, cx) {
                    (Some(branch), _) => branch,
                    _ => agentty_bridge::worktree::base_ref(&folder),
                };
                let mut spec = LaunchSpec::with_prompt(
                    agent.agent(),
                    review_prompt(&to, branch.as_deref(), &base, &request.ctl.prompt),
                    format!("{REVIEW_PREFIX}{to}"),
                    folder.clone(),
                );
                spec.read_only = true;
                let replaced = self.add_chat_worker(id, spec, Some(&worker), cx)?;
                Ok(serde_json::json!({
                    "reviewer": format!("{REVIEW_PREFIX}{to}"),
                    "agent": agent.label(),
                    "folder": folder,
                    "replaced": replaced,
                })
                .to_string())
            }
            other => Err(format!("unknown action {other}")),
        }
    }

    /// The lead's place in its tab, drawn as the chat (or as its terminal, with the chat's bar on top).
    pub(super) fn render_chat(&self, pane: &Pane, split: bool, active: bool, cx: &mut Context<Self>) -> Option<AnyElement> {
        let state = self.chats.get(&pane.entity_id())?;
        let id = pane.entity_id();
        let view = pane.read(cx);
        let forced = state.setup_screen || view.status.needs_user();
        let terminal = state.show_terminal || forced;
        let (status, color) = status_label_sized(view, false, cx);
        let working = view.status.in_turn();
        let last_tool = view.last_tool.as_ref().map(|(tool, target)| crate::terminal::tool_label(tool, target.as_deref()));
        let workers = self.chat_workers(pane);

        let mut chips = div().flex().items_center().gap_1().min_w_0().overflow_hidden();
        for (index, worker) in workers.iter().enumerate() {
            let wview = worker.read(cx);
            let (_, dot) = status_label_sized(wview, true, cx);
            let title = wview.spec.title.clone();
            let target = worker.clone();
            chips = chips.child(
                div()
                    .id(SharedString::from(format!("chat-worker-{index}")))
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_1p5()
                    .py_0p5()
                    .rounded_md()
                    .max_w(px(160.))
                    .cursor_pointer()
                    .bg(hex_alpha(dot, if waits_on_user(wview) { 0.25 } else { 0.1 }))
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .child(div().flex_shrink_0().size(px(6.)).rounded_full().bg(hex(dot)))
                    .child(div().t_caption().truncate().text_color(hex(Chrome::FOREGROUND)).child(title))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.go_to_pane(&target, window, cx))),
            );
        }
        let toggle_label = t(cx, if terminal { "chat.show_chat" } else { "chat.show_terminal" });
        // The chat's own branch, when the lead works on one: where its workers' work comes together.
        let integration = match Self::lead_branch(pane, cx) {
            (Some(branch), true) => Some(branch),
            _ => None,
        };
        let roles = state.roles;
        let reviewer = match roles.reviewer {
            Some(agent) => agent.name().to_string(),
            None => t(cx, "chat.role_off").to_string(),
        };
        let role_button = |id: &'static str, label: String| {
            div()
                .id(id)
                .flex_shrink_0()
                .px_1p5()
                .py_0p5()
                .rounded_md()
                .cursor_pointer()
                .t_caption()
                .text_color(hex(Chrome::FOREGROUND))
                .hover(|s| s.bg(hex(Chrome::HOVER)))
                .child(label)
        };
        let header = div()
            .flex_shrink_0()
            .h(px(32.))
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(hex(Chrome::BORDER))
            .bg(hex(Chrome::PANEL))
            .child(icon("message-square", 14., hex(Chrome::BRIGHT)))
            .child(
                div()
                    .flex_shrink_0()
                    .t_small()
                    .font_weight(crate::theme::EMPHASIS)
                    .text_color(hex(Chrome::BRIGHT))
                    .child(t(cx, "chat.title")),
            )
            .child(div().flex_shrink_0().t_caption().text_color(hex(color)).child(status))
            .when_some(integration, |d, branch| {
                d.child(
                    div()
                        .id("chat-branch")
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap_1()
                        .t_caption()
                        .text_color(hex(Chrome::MUTED))
                        .tooltip(crate::ui::Tooltip::text(t(cx, "chat.branch_hint"), None))
                        .child(icon("git-branch", 11., hex(Chrome::MUTED)))
                        .child(div().truncate().max_w(px(220.)).child(branch)),
                )
            })
            .child(div().flex_1())
            .child(chips)
            .child(div().flex_shrink_0().t_caption().text_color(hex(Chrome::MUTED)).child(tf(
                cx,
                "chat.workers",
                &[("n", &workers.len().to_string()), ("max", &MAX_WORKERS.to_string())],
            )))
            .child(
                role_button("chat-role-worker", tf(cx, "chat.role_worker", &[("agent", roles.worker.name())]))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.set_chat_roles(id, ChatRoles::next_worker, cx))),
            )
            .child(
                role_button("chat-role-reviewer", tf(cx, "chat.role_reviewer", &[("agent", &reviewer)]))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.set_chat_roles(id, ChatRoles::next_reviewer, cx))),
            )
            .when(!forced, |d| {
                d.child(
                    div()
                        .id("chat-toggle")
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_2()
                        .py_0p5()
                        .rounded_md()
                        .cursor_pointer()
                        .border_1()
                        .border_color(hex(Chrome::BORDER))
                        .t_caption()
                        .text_color(hex(Chrome::FOREGROUND))
                        .hover(|s| s.bg(hex(Chrome::HOVER)))
                        .child(icon(if terminal { "message-square" } else { "square-terminal" }, 12., hex(Chrome::FOREGROUND)))
                        .child(toggle_label)
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.toggle_chat_terminal(id, window, cx))),
                )
            });

        let body: AnyElement = if terminal {
            let reason = forced.then(|| {
                div()
                    .flex_shrink_0()
                    .px_3()
                    .py_1()
                    .t_small()
                    .bg(hex_alpha(Chrome::ATTENTION, 0.15))
                    .text_color(hex(Chrome::BRIGHT))
                    .child(t(cx, if state.setup_screen { "chat.setup_screen" } else { "chat.lead_waits" }))
            });
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .children(reason)
                .child(div().flex_1().min_h_0().child(self.render_pane(pane, split, active, cx)))
                .into_any_element()
        } else {
            self.render_chat_body(state, working, last_tool, &workers, cx)
        };
        Some(div().size_full().flex().flex_col().bg(hex(Chrome::EDITOR)).child(header).child(body).into_any_element())
    }

    fn render_chat_body(
        &self,
        state: &ChatState,
        working: bool,
        last_tool: Option<String>,
        workers: &[Pane],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let messages: AnyElement = if state.shown.is_empty() {
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .px_6()
                .child(icon("users", 28., hex(Chrome::MUTED)))
                .child(div().t_title().text_color(hex(Chrome::BRIGHT)).child(t(cx, "chat.empty_title")))
                .child(div().max_w(px(520.)).t_body().text_center().text_color(hex(Chrome::MUTED)).child(tf(
                    cx,
                    "chat.empty_body",
                    &[("max", &MAX_WORKERS.to_string())],
                )))
                .into_any_element()
        } else {
            let items = state.shown.clone();
            let rows = list(state.list.clone(), move |index, _, cx| match items.get(index) {
                Some(item) => div().w_full().px_4().pt_3().child(render_item(index, item, cx)).into_any_element(),
                None => div().into_any_element(),
            })
            .size_full()
            .pb_3();
            div()
                .relative()
                .group(crate::ui::SCROLL_GROUP)
                .flex_1()
                .min_h_0()
                .child(rows)
                .child(crate::ui::list_scrollbar(state.list.clone()))
                .into_any_element()
        };
        // A worker waiting on the user is said here too: the chat is where the user looks.
        let waiting: Vec<AnyElement> = workers
            .iter()
            .filter(|p| waits_on_user(p.read(cx)))
            .enumerate()
            .map(|(index, worker)| {
                let title = worker.read(cx).spec.title.clone();
                let target = worker.clone();
                div()
                    .id(SharedString::from(format!("chat-waiting-{index}")))
                    .mx_4()
                    .mt_2()
                    .px_3()
                    .py_1p5()
                    .rounded_md()
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .gap_2()
                    .bg(hex_alpha(Chrome::ATTENTION, 0.15))
                    .hover(|s| s.bg(hex_alpha(Chrome::ATTENTION, 0.25)))
                    .child(icon("bell", 12., hex(Chrome::ATTENTION)))
                    .child(div().t_small().text_color(hex(Chrome::BRIGHT)).child(tf(cx, "chat.worker_waits", &[("title", &title)])))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.go_to_pane(&target, window, cx)))
                    .into_any_element()
            })
            .collect();
        let activity = working.then(|| {
            let text = match last_tool {
                Some(tool) => format!("{} · {tool}", t(cx, "chat.lead_working")),
                None => t(cx, "chat.lead_working").to_string(),
            };
            div()
                .flex_shrink_0()
                .px_4()
                .pt_1()
                .flex()
                .items_center()
                .gap_2()
                .child(crate::ui::spinner(12., hex(Chrome::ORANGE)))
                .child(div().t_caption().truncate().text_color(hex(Chrome::MUTED)).child(text))
        });
        let composer = state.input.as_ref().map(|(input, _)| {
            let id = state.lead.entity_id();
            div()
                .flex_shrink_0()
                .m_3()
                .px_3()
                .py_2()
                .rounded_lg()
                .border_1()
                .border_color(hex(Chrome::BORDER))
                .bg(hex(Chrome::PANEL))
                .flex()
                .items_end()
                .gap_2()
                .child(div().flex_1().min_w_0().t_body().text_color(hex(Chrome::BRIGHT)).child(input.clone()))
                .child(
                    div()
                        .id("chat-send")
                        .flex_shrink_0()
                        .size(px(28.))
                        .rounded_md()
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .bg(hex(Chrome::ACCENT))
                        .hover(|s| s.opacity(0.85))
                        .child(icon("send", 14., hex(Chrome::BRIGHT)))
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.send_chat_message(id, window, cx))),
                )
        });
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(messages)
            .children(waiting)
            .children(activity)
            .children(composer)
            .into_any_element()
    }

    fn toggle_chat_terminal(&mut self, id: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.chats.get_mut(&id) else { return };
        state.show_terminal = !state.show_terminal;
        let lead = state.lead.clone();
        self.mark_active(&lead, cx);
        self.focus_pane(&lead, window, cx);
        cx.notify();
    }

    /// Debug driver: `chat <folder>`, `chat send:<text>`, `chat terminal`, `chat state`.
    pub(super) fn debug_chat(&mut self, argument: &str, window: &mut Window, cx: &mut Context<Self>) {
        let front = self.active_pane().and_then(|pane| self.chat_lead_of(&pane)).map(|lead| lead.entity_id());
        match argument.split_once(':').unwrap_or((argument, "")) {
            ("send", text) => {
                let Some(id) = front else { return eprintln!("agentty: no chat in front") };
                self.say_to_lead(id, text.trim().to_string(), cx);
            }
            ("terminal", _) => {
                if let Some(id) = front {
                    self.toggle_chat_terminal(id, window, cx);
                }
            }
            ("worker", _) | ("reviewer", _) => {
                let Some(id) = front else { return eprintln!("agentty: no chat in front") };
                let next = if argument == "worker" { ChatRoles::next_worker } else { ChatRoles::next_reviewer };
                self.set_chat_roles(id, next, cx);
            }
            ("state", _) => {
                let Some(state) = front.and_then(|id| self.chats.get(&id)) else { return eprintln!("agentty: no chat in front") };
                let workers: Vec<String> = self
                    .chat_workers(&state.lead)
                    .iter()
                    .map(|p| {
                        let view = p.read(cx);
                        format!("{} [{}]", view.spec.title, status_word(&view.status, view.is_running()))
                    })
                    .collect();
                eprintln!(
                    "agentty: chat lead={} roles={:?} briefed={} setup={} terminal={} pending={} items={:?} workers={workers:?}",
                    status_word(&state.lead.read(cx).status, state.lead.read(cx).is_running()),
                    state.roles,
                    state.briefed,
                    state.setup_screen,
                    state.show_terminal,
                    state.pending.len(),
                    state.shown,
                );
            }
            (folder, _) => self.create_chat_workspace(PathBuf::from(folder), window, cx),
        }
    }
}

/// One entry of the chat.
fn render_item(index: usize, item: &ChatItem, cx: &App) -> AnyElement {
    let row = div().id(SharedString::from(format!("chat-item-{index}"))).w_full().flex();
    match item {
        ChatItem::User(text) => row
            .justify_end()
            .child(
                div()
                    .max_w(gpui::relative(0.8))
                    .px_3()
                    .py_2()
                    .rounded_lg()
                    .bg(hex_alpha(Chrome::ACCENT, 0.18))
                    .border_1()
                    .border_color(hex_alpha(Chrome::ACCENT, 0.35))
                    .child(text_block(text, Chrome::BRIGHT)),
            )
            .into_any_element(),
        ChatItem::Lead(text) => row
            .gap_2()
            .child(crate::brand::avatar("claude", 20.))
            .child(div().flex_1().min_w_0().pt_0p5().child(lead_block(text, cx)))
            .into_any_element(),
        ChatItem::Report(text) => {
            let mut lines = text.lines();
            let first = lines.next().unwrap_or_default().to_string();
            let rest: Vec<&str> = lines.filter(|l| !l.trim().is_empty()).take(6).collect();
            row.child(
                div()
                    .w_full()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(hex(Chrome::PANEL))
                    .border_l_2()
                    .border_color(hex(Chrome::SUCCESS))
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .child(icon("bot", 12., hex(Chrome::SUCCESS)))
                            .child(div().t_small().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::FOREGROUND)).child(first)),
                    )
                    .children(
                        rest.into_iter().map(|line| div().t_caption().truncate().text_color(hex(Chrome::MUTED)).child(line.to_string())),
                    ),
            )
            .into_any_element()
        }
    }
}

/// The lead's reply: its text, with tool steps as short muted lines (a long run of them folded).
fn lead_block(text: &str, cx: &App) -> AnyElement {
    let mut out = div().flex().flex_col().gap_1();
    let mut prose = String::new();
    let mut steps: Vec<String> = Vec::new();
    let flush_steps = |out: gpui::Div, steps: &mut Vec<String>| {
        if steps.is_empty() {
            return out;
        }
        let hidden = steps.len().saturating_sub(TOOL_STEPS_SHOWN);
        let mut block = div().flex().flex_col();
        for step in steps.iter().take(TOOL_STEPS_SHOWN) {
            block = block.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .t_caption()
                    .text_color(hex(Chrome::MUTED))
                    .child(icon("square-terminal", 11., hex(Chrome::MUTED)))
                    .child(div().truncate().child(step.clone())),
            );
        }
        if hidden > 0 {
            block = block.child(div().t_caption().text_color(hex(Chrome::MUTED)).child(tf(
                cx,
                "chat.more_steps",
                &[("n", &hidden.to_string())],
            )));
        }
        steps.clear();
        out.child(block)
    };
    for line in text.lines() {
        if let Some(step) = line.strip_prefix("[tool: ") {
            if !prose.trim().is_empty() {
                out = out.child(text_block(prose.trim_end(), Chrome::FOREGROUND));
            }
            prose.clear();
            steps.push(step.trim_end_matches(']').to_string());
        } else {
            out = flush_steps(out, &mut steps);
            prose.push_str(line);
            prose.push('\n');
        }
    }
    out = flush_steps(out, &mut steps);
    if !prose.trim().is_empty() {
        out = out.child(text_block(prose.trim_end(), Chrome::FOREGROUND));
    }
    out.into_any_element()
}

/// Text with a little Markdown: headings stand out, code blocks are monospaced.
fn text_block(text: &str, color: u32) -> gpui::Div {
    let mut out = div().flex().flex_col().gap_0p5().t_body().text_color(hex(color));
    let mut code = false;
    for line in text.lines().take(600) {
        if line.trim_start().starts_with("```") {
            code = !code;
            continue;
        }
        let line_div = if code {
            div().px_2().bg(hex(Chrome::PANEL)).font_family("JetBrains Mono").t_small().child(if line.is_empty() {
                " ".to_string()
            } else {
                line.to_string()
            })
        } else if let Some(heading) = line.strip_prefix('#') {
            div()
                .pt_1()
                .font_weight(crate::theme::EMPHASIS)
                .text_color(hex(Chrome::BRIGHT))
                .child(heading.trim_start_matches('#').trim().to_string())
        } else {
            div().child(if line.is_empty() { " ".to_string() } else { line.replace("**", "") })
        };
        out = out.child(line_div);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(role: Role, text: &str) -> Turn {
        Turn { role, text: text.into() }
    }

    #[test]
    fn the_brief_is_left_out_and_reports_stand_apart() {
        let first = briefed_prompt("Build a todo app", "Korean", "Your folder is the project folder itself.");
        assert!(first.contains("Talk to the user in Korean.") && first.contains("Your folder is the project folder itself."));
        assert!(!first.contains("{folder}") && !first.contains("{language}"));
        let turns = vec![
            turn(Role::User, &first),
            turn(Role::Assistant, "Plan: two workers."),
            turn(Role::User, "[Agentty] Worker \"API\" finished its turn.\nIts last reply:\nDone"),
            turn(Role::User, "thanks"),
        ];
        let (items, briefed) = chat_items(&turns);
        assert!(briefed);
        assert_eq!(
            items,
            vec![
                ChatItem::User("Build a todo app".into()),
                ChatItem::Lead("Plan: two workers.".into()),
                ChatItem::Report("Worker \"API\" finished its turn.\nIts last reply:\nDone".into()),
                ChatItem::User("thanks".into()),
            ]
        );
        let (_, briefed) = chat_items(&[turn(Role::User, "hello")]);
        assert!(!briefed);
        // Claude Code keeps a pasted message between tags.
        let pasted = format!("\n\n<pasted_content id=\"7\">\n{first}\n</pasted_content id=\"7\">\n");
        let report = "<pasted_content id=\"8\">[Agentty] Worker \"A\" finished its turn.</pasted_content id=\"8\">";
        let (items, briefed) = chat_items(&[turn(Role::User, &pasted), turn(Role::User, report)]);
        assert!(briefed);
        assert_eq!(items, vec![ChatItem::User("Build a todo app".into()), ChatItem::Report("Worker \"A\" finished its turn.".into())]);
    }

    #[test]
    fn workers_fill_rows_of_two_below_the_chat() {
        assert!(matches!(chat_tree(0, vec![], 0.5), PaneNode::Leaf(0)));
        let one = chat_tree(0, vec![1], 0.6);
        assert_eq!(one.leaves(), vec![0, 1]);
        let PaneNode::Split { axis: Axis::Vertical, sizes, .. } = &one else { panic!("not stacked") };
        assert!((sizes[0] - 0.6).abs() < 1e-6);
        let four = chat_tree(0, vec![1, 2, 3, 4], 0.5);
        assert_eq!(four.leaves(), vec![0, 1, 2, 3, 4]);
        let PaneNode::Split { children, .. } = &four else { panic!("not split") };
        let PaneNode::Split { axis: Axis::Vertical, children: rows, .. } = &children[1] else { panic!("no rows") };
        assert_eq!(rows.len(), 2);
        assert!(matches!(&rows[0], PaneNode::Split { axis: Axis::Horizontal, .. }));
        assert!((top_share(&four, &0) - 0.5).abs() < 1e-6);
        assert!((top_share(&PaneNode::Leaf(0), &0) - DEFAULT_TOP).abs() < 1e-6);
    }

    #[test]
    fn only_changed_entries_are_measured_again() {
        let list = ListState::new(2, ListAlignment::Bottom, px(100.));
        let old = vec![ChatItem::User("a".into()), ChatItem::Lead("b".into())];
        let new = vec![ChatItem::User("a".into()), ChatItem::Lead("b c".into()), ChatItem::User("d".into())];
        splice_list(&list, &old, &new);
        assert_eq!(list.item_count(), 3);
    }

    #[test]
    fn first_run_screens_need_the_terminal() {
        assert!(is_setup_screen(&["Do you trust the files in this folder?".into()]));
        assert!(is_setup_screen(&["Trust this folder? Codex can read, edit, and run files here".into()]));
        assert!(!is_setup_screen(&["> what should we build".into()]));
    }
}
