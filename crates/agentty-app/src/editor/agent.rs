//! The editor and the agent working on the same code.
//!
//! - **Linked session**: the terminal session the editor talks to (picked by the workbench, shown
//!   beside the editor). Its name — the agent and four characters of its session, `Claude-3f9a` —
//!   marks what it does here.
//! - **Where the agent edits**: just before an agent's edit (Claude Code's hooks name the file and
//!   the text being replaced) a caret with its name shows at that spot; when the file changes on
//!   disk the changed lines light up and fade, with the name beside them. Agents without such hooks
//!   are known by their changes on disk while the linked session works.
//! - **Selection bubble**: select code and a small bar offers *Ask AI* (an instruction about
//!   exactly those lines, sent to the linked session), *Copy*, *Go to definition* and *Find usages*.

use super::buffer::{Pos, Selection};
use super::{CodeEditor, Content, EditorEvent};
use crate::i18n::t;
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, IconSize, TypeScale};
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, ClipboardItem, Context, Entity, Subscription, Window};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long changed lines stay lit after an agent's edit (they fade over this time).
pub const FADE: Duration = Duration::from_millis(3200);
/// How long the agent's name stays at the place it edited.
pub const LINGER: Duration = Duration::from_secs(8);
/// A change read from disk before the agent's hook said it wrote it is claimed within this time.
const CLAIM_WINDOW: Duration = Duration::from_secs(4);
/// An announced edit that never reported back stops being shown after this.
const PENDING_LIMIT: Duration = Duration::from_secs(90);

/// The session the editor is linked to, as the workbench describes it.
#[derive(Clone, Debug, PartialEq)]
pub struct LinkedAgent {
    pub pane_id: u64,
    /// `Claude-3f9a`.
    pub label: String,
    /// Brand id for the avatar (`claude`, `codex`, …).
    pub brand: &'static str,
    pub color: u32,
    pub working: bool,
    /// It waits for the user: a permission to grant or a question to answer.
    pub waiting: bool,
}

/// An agent at work in a file.
#[derive(Clone, Debug)]
pub struct Presence {
    pub pane_id: u64,
    pub label: String,
    pub color: u32,
    pub path: PathBuf,
    /// Where its caret is.
    pub at: Pos,
    /// Lines it changed (first, last).
    pub lines: Option<(usize, usize)>,
    /// The separate runs of changed lines inside them, lit while they fade.
    pub parts: Vec<(usize, usize)>,
    /// Announced, not written yet.
    pub pending: bool,
    /// A line of the text being replaced, to find the spot in a file that was not open.
    pub anchor: Option<String>,
    pub since: Instant,
}

impl Presence {
    /// How lit the changed lines are, from 1 (just changed) to 0.
    pub fn glow(&self) -> f32 {
        if self.pending || self.lines.is_none() {
            return 0.;
        }
        1. - (self.since.elapsed().as_secs_f32() / FADE.as_secs_f32()).min(1.)
    }

    fn expired(&self) -> bool {
        let limit = if self.pending { PENDING_LIMIT } else { LINGER };
        self.since.elapsed() > limit
    }
}

/// An instruction sent about some lines, until the agent changes them (or a while passes).
#[derive(Clone, Debug)]
pub struct Asked {
    pub path: PathBuf,
    pub lines: (usize, usize),
    pub pane_id: Option<u64>,
    pub label: String,
    pub color: u32,
    pub since: Instant,
}

/// How long the lines of an instruction stay marked without an answer.
const ASKED_LIMIT: Duration = Duration::from_secs(180);

gpui::actions!(code_editor, [AskOnly]);

pub fn bind_keys(cx: &mut gpui::App) {
    cx.bind_keys([crate::key("alt-enter", AskOnly, Some(ASK))]);
}

/// The instruction box's key context.
const ASK: &str = "EditorAsk";

/// An instruction being typed about the selected lines.
pub struct AskBox {
    pub input: Entity<TextInput>,
    path: PathBuf,
    project: PathBuf,
    /// Lines (from 0) the instruction is about.
    lines: (usize, usize),
    _subscription: Subscription,
}

/// Where a line of text (trimmed) first appears in the buffer.
pub fn find_anchor(buffer: &super::buffer::Buffer, anchor: &str) -> Option<Pos> {
    let anchor = anchor.trim();
    if anchor.is_empty() {
        return None;
    }
    (0..buffer.line_count()).find_map(|i| buffer.line(i).find(anchor).map(|col| Pos::new(i, col)))
}

/// What is sent to the agent: the file, the lines and the instruction, in English so any agent
/// reads it the same way (the instruction itself stays in the user's words).
pub fn ask_prompt(rel: &str, lines: (usize, usize), instruction: &str) -> String {
    let range = if lines.0 == lines.1 { format!("line {}", lines.0 + 1) } else { format!("lines {}-{}", lines.0 + 1, lines.1 + 1) };
    format!("In {rel}, {range}: {}", instruction.trim())
}

/// The short id of a session for its label: four characters of its id (or of the pane's number).
pub fn short_id(session: Option<&str>, pane_id: u64) -> String {
    match session.map(|s| s.chars().filter(char::is_ascii_alphanumeric).take(4).collect::<String>()).filter(|s| s.len() == 4) {
        Some(id) => id.to_lowercase(),
        None => format!("{:04x}", pane_id.wrapping_mul(0x9e37_79b9) & 0xffff),
    }
}

/// Lines that differ between `old` and `new` text, as (first, last) lines of `new`; `None` when
/// nothing changed. A deletion is the line where the text went.
pub fn changed_lines(old: &str, new: &str) -> Option<(usize, usize)> {
    if old == new {
        return None;
    }
    let (a, b): (Vec<&str>, Vec<&str>) = (old.lines().collect(), new.lines().collect());
    let first = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let tail = a[first..].iter().rev().zip(b[first..].iter().rev()).take_while(|(x, y)| x == y).count();
    let last = (b.len() - tail).max(first + 1) - 1;
    Some((first.min(b.len().saturating_sub(1)), last.min(b.len().saturating_sub(1))))
}

impl CodeEditor {
    /// The workbench tells which session the editor is linked to (and whether it works).
    pub fn set_linked(&mut self, linked: Option<LinkedAgent>, cx: &mut Context<Self>) {
        if self.linked != linked {
            self.linked = linked;
            cx.notify();
        }
    }

    /// An agent is about to change `path`: its caret goes where `anchor` (a line of the text it
    /// replaces) is, or to the top for a whole new file.
    pub fn agent_will_edit(&mut self, agent: &LinkedAgent, path: &Path, anchor: Option<&str>, cx: &mut Context<Self>) {
        self.remember_agent(agent);
        let at = self
            .docs
            .iter()
            .find(|d| d.path == path || d.target == path)
            .and_then(|doc| find_anchor(&doc.buffer, anchor?))
            .unwrap_or_default();
        self.presence.retain(|p| p.pane_id != agent.pane_id);
        self.presence.push(Presence {
            pane_id: agent.pane_id,
            label: agent.label.clone(),
            color: agent.color,
            path: path.to_path_buf(),
            at,
            lines: None,
            parts: Vec::new(),
            pending: true,
            anchor: anchor.map(str::to_string),
            since: Instant::now(),
        });
        // Following: the file the agent is about to change comes to the front (opened if need be),
        // at the spot, without taking the keyboard — and not while the user types in the editor.
        // Only while the editor is on screen: following never swaps the terminals for it.
        if self.follow_agent && !self.typing() && !self.preview.covered {
            match self.docs.iter().position(|d| d.path == path || d.target == path) {
                Some(index) => {
                    self.active = index;
                    if let Some(doc) = self.docs.get_mut(index) {
                        doc.scroll_to_line = Some(at.line);
                    }
                    self.tabs_changed(cx);
                }
                None => {
                    let project = self.project_root().unwrap_or_default();
                    self.quiet_open = Some(path.to_path_buf());
                    self.pending_goto = Some((path.to_path_buf(), Pos::default(), 0, anchor.map(str::to_string)));
                    self.pending_column = None;
                    cx.emit(EditorEvent::OpenFile(path.to_path_buf(), project));
                }
            }
        }
        self.keep_presence_fresh(cx);
        cx.notify();
    }

    /// The agent wrote `path`: it is read again now rather than at the next check.
    pub fn agent_did_edit(&mut self, agent: &LinkedAgent, path: &Path, cx: &mut Context<Self>) {
        self.remember_agent(agent);
        // The change was read from disk a moment before this report came: it is this agent's.
        self.unclaimed.retain(|(_, _, at)| at.elapsed() < CLAIM_WINDOW);
        if let Some(index) = self.unclaimed.iter().position(|(p, _, _)| p == path) {
            let (_, before, _) = self.unclaimed.remove(index);
            self.claim_change(agent, path, &before, cx);
            return;
        }
        // A file not open here: the agent's mark stays at the spot it announced, as written.
        if !self.docs.iter().any(|d| d.path == path || d.target == path) {
            for presence in self.presence.iter_mut().filter(|p| p.pane_id == agent.pane_id && p.path == path) {
                presence.pending = false;
                presence.since = Instant::now();
            }
            return cx.notify();
        }
        self.expected_edits.retain(|(p, _, at)| p != path && at.elapsed() < Duration::from_secs(10));
        self.expected_edits.push((path.to_path_buf(), agent.clone(), Instant::now()));
        self.check_disk(cx);
    }

    /// A file open here changed on disk and was read again (it was `before`, with `selection`):
    /// who changed what. The user's cursor and view stay on the text they were on.
    pub(super) fn note_disk_change(&mut self, path: &Path, before: &str, selection: Selection, cx: &mut Context<Self>) {
        let line_height = self.layout.as_ref().map(|l| l.line_height);
        let Some(index) = self.docs.iter().position(|d| d.path == path) else { return };
        let doc = &mut self.docs[index];
        let Some(hunk) = super::review::hunk(before, &doc.buffer.text()) else { return };
        let lines = changed_lines(before, &doc.buffer.text()).unwrap_or((hunk.start, hunk.start));
        let (start, old_end) = (hunk.start, hunk.start + hunk.old.len());
        let grown = hunk.new.len() as isize - hunk.old.len() as isize;
        let parts: Vec<(usize, usize)> = super::review::split(&hunk).iter().map(|p| (p.start, p.start + p.new.len().max(1) - 1)).collect();
        let moved = |pos: Pos| {
            if pos.line >= old_end {
                Some(Pos::new((pos.line as isize + grown).max(0) as usize, pos.col))
            } else if pos.line < start {
                Some(pos)
            } else {
                None
            }
        };
        // Text above the cursor grew or shrank: the cursor goes with its line. A selection in the
        // changed lines no longer means what it did and becomes a cursor.
        doc.buffer.selection = match (moved(selection.anchor), moved(selection.head)) {
            (Some(anchor), Some(head)) => Selection { anchor: doc.buffer.clamp(anchor), head: doc.buffer.clamp(head) },
            _ => Selection::cursor(doc.buffer.clamp(Pos::new(start + hunk.new.len(), 0))),
        };
        // And the view stays on the same text when the change is above it.
        if let (Some(height), true) = (line_height, index == self.active) {
            let end_row = doc.folds.row_of((old_end as isize + grown).max(0) as usize) as isize - grown;
            if (end_row as f32) * height <= doc.scroll.y {
                doc.scroll.y = (doc.scroll.y + grown as f32 * height).max(0.);
            }
        }
        let expected = self.expected_edits.iter().position(|(p, _, at)| p == path && at.elapsed() < Duration::from_secs(10));
        // An agent that announced an edit of this file (and has not reported it yet) made it.
        let announced = self.presence.iter().rev().find(|p| p.pending && p.path == path).map(|p| p.pane_id);
        let agent = match (expected, announced) {
            (Some(index), _) => Some(self.expected_edits.remove(index).1),
            (None, Some(pane_id)) => self.agents_seen.iter().find(|a| a.pane_id == pane_id).cloned(),
            // No hook said so: the linked session, if it is at work.
            (None, None) => self.linked.clone().filter(|l| l.working),
        };
        if agent.is_none() {
            // Its hook may still be on the way: kept for a moment to be claimed.
            self.unclaimed.retain(|(p, _, at)| p != path && at.elapsed() < CLAIM_WINDOW);
            self.unclaimed.push((path.to_path_buf(), before.to_string(), Instant::now()));
        }
        let (pane_id, label, color) = match &agent {
            Some(agent) => (agent.pane_id, agent.label.clone(), agent.color),
            None => (0, String::new(), Chrome::ACCENT),
        };
        // An unclaimed change is recorded when (if) its agent claims it, so nothing moves twice.
        if agent.is_some() {
            self.record_change(path, before, agent.as_ref());
        }
        if self.asked.as_ref().is_some_and(|a| a.path == path && (a.pane_id.is_none() || a.pane_id == agent.as_ref().map(|g| g.pane_id))) {
            self.asked = None;
        }
        self.presence.retain(|p| !(p.pane_id == pane_id && (p.pending || p.path == path)));
        self.presence.push(Presence {
            pane_id,
            label,
            color,
            path: path.to_path_buf(),
            at: Pos::new(lines.1, 0),
            lines: Some(lines),
            parts,
            pending: false,
            anchor: None,
            since: Instant::now(),
        });
        // Following waits while the user types: it never pulls the file away under their keys.
        if self.follow_agent && agent.is_some() && !self.typing() && !self.preview.covered {
            if let Some(index) = self.docs.iter().position(|d| d.path == path) {
                self.active = index;
                if let Some(doc) = self.docs.get_mut(index) {
                    doc.scroll_to_line = Some(lines.0);
                }
                self.tabs_changed(cx);
            }
        }
        self.keep_presence_fresh(cx);
        cx.notify();
    }

    /// Repaints while something fades, and forgets what is over.
    fn keep_presence_fresh(&mut self, cx: &mut Context<Self>) {
        if self.presence_ticking {
            return;
        }
        self.presence_ticking = true;
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(80)).await;
            let more = this.update(cx, |this, cx| {
                this.presence.retain(|p| !p.expired());
                if this.asked.as_ref().is_some_and(|a| a.since.elapsed() > ASKED_LIMIT) {
                    this.asked = None;
                }
                cx.notify();
                let busy = !this.presence.is_empty() || this.asked.is_some();
                if !busy {
                    this.presence_ticking = false;
                }
                busy
            });
            if !matches!(more, Ok(true)) {
                break;
            }
        })
        .detach();
    }

    /// Shows where an agent works: its file, at its caret.
    pub fn jump_to_agent(&mut self, pane_id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(presence) = self.presence.iter().rev().find(|p| p.pane_id == pane_id).cloned() else { return };
        self.go_to_anchor(&presence.path, presence.at, presence.anchor, window, cx);
    }

    /// One chip per agent at work in any file (several sessions may edit at once): its name and
    /// file; a click shows that place.
    pub(super) fn render_agents_at_work(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut seen = Vec::new();
        let front = self.docs.get(self.active).map(|d| d.path.clone());
        let linked = self.linked.as_ref().map(|l| l.pane_id);
        self.presence
            .iter()
            .rev()
            .filter(|p| !p.label.is_empty())
            // The linked session at work in the file in front already shows: its chip and its caret.
            .filter(|p| !(Some(p.pane_id) == linked && Some(&p.path) == front.as_ref()))
            .filter(|p| {
                let new = !seen.contains(&p.pane_id);
                seen.push(p.pane_id);
                new
            })
            .take(4)
            .map(|p| {
                let (pane_id, color) = (p.pane_id, p.color);
                let file = p.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                div()
                    .id(("editor-agent-at-work", pane_id as usize))
                    .flex_shrink_0()
                    .h(px(22.))
                    .px_1p5()
                    .flex()
                    .items_center()
                    .gap_1()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(hex_alpha(color, 0.16))
                    .hover(|s| s.bg(hex_alpha(color, 0.3)))
                    .t_caption()
                    .text_color(hex(Chrome::BRIGHT))
                    .child(div().size(px(7.)).rounded_full().bg(hex(color)).when(p.pending, |d| d.opacity(0.6)))
                    .child(p.label.clone())
                    .child(div().text_color(hex(Chrome::MUTED)).max_w(px(120.)).truncate().child(file))
                    .tooltip(crate::ui::Tooltip::text(t(cx, "editor.agent_jump"), None))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.jump_to_agent(pane_id, window, cx)))
                    .into_any_element()
            })
            .collect()
    }

    /// Agents at work in the file in front.
    pub(super) fn presence_here(&self) -> impl Iterator<Item = &Presence> {
        let path = self.docs.get(self.active).map(|d| d.path.clone());
        self.presence.iter().filter(move |p| Some(&p.path) == path.as_ref())
    }

    /// A file just opened: agents announced there before it was open get their spot.
    pub(super) fn resolve_presence(&mut self, path: &Path) {
        let Some(doc) = self.docs.iter().find(|d| d.path == path) else { return };
        for presence in self.presence.iter_mut().filter(|p| p.path == path && p.at == Pos::default()) {
            if let Some(at) = presence.anchor.as_deref().and_then(|a| find_anchor(&doc.buffer, a)) {
                presence.at = at;
            }
        }
    }

    /// Agents that announced edits, to name a change read from disk before their report came.
    fn remember_agent(&mut self, agent: &LinkedAgent) {
        self.agents_seen.retain(|a| a.pane_id != agent.pane_id);
        self.agents_seen.push(agent.clone());
    }

    /// A change already read from disk (it was `before`) turns out to be `agent`'s: recorded for
    /// review and marked, as if the report had come first.
    fn claim_change(&mut self, agent: &LinkedAgent, path: &Path, before: &str, cx: &mut Context<Self>) {
        let Some(doc) = self.docs.iter().find(|d| d.path == path) else { return };
        let after = doc.buffer.text();
        let Some(hunk) = super::review::hunk(before, &after) else { return };
        let parts: Vec<(usize, usize)> = super::review::split(&hunk).iter().map(|p| (p.start, p.start + p.new.len().max(1) - 1)).collect();
        let last = hunk.start + hunk.new.len().max(1) - 1;
        self.record_change(path, before, Some(agent));
        self.presence.retain(|p| !(p.path == path && (p.pane_id == agent.pane_id || p.label.is_empty())));
        self.presence.push(Presence {
            pane_id: agent.pane_id,
            label: agent.label.clone(),
            color: agent.color,
            path: path.to_path_buf(),
            at: Pos::new(last, 0),
            lines: Some((hunk.start, last)),
            parts,
            pending: false,
            anchor: None,
            since: Instant::now(),
        });
        if self.asked.as_ref().is_some_and(|a| a.path == path) {
            self.asked = None;
        }
        self.keep_presence_fresh(cx);
        self.tabs_changed(cx);
        cx.notify();
    }

    /// The user typed in the editor a moment ago.
    fn typing(&self) -> bool {
        self.last_typed.is_some_and(|at| at.elapsed() < Duration::from_millis(1500))
    }

    pub fn toggle_follow(&mut self, cx: &mut Context<Self>) {
        self.follow_agent = !self.follow_agent;
        cx.notify();
    }

    /// The toolbar's chip for the linked session: who it is and whether it works (click: show or
    /// hide it beside the editor), and whether the editor follows its edits.
    pub(super) fn render_session_chip(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(linked) = self.linked.as_ref() else {
            return div()
                .id("editor-session")
                .flex_shrink_0()
                .h(px(24.))
                .px_2()
                .flex()
                .items_center()
                .gap_1p5()
                .rounded_md()
                .cursor_pointer()
                .t_small()
                .text_color(hex(Chrome::FOREGROUND))
                .hover(|s| s.bg(hex(Chrome::HOVER)).text_color(hex(Chrome::BRIGHT)))
                .child(icon("link", IconSize::INLINE, hex(Chrome::FOREGROUND)))
                .when(!self.layout.as_ref().is_some_and(|l| l.width() < 760.), |d| d.child(t(cx, "editor.connect_session")))
                .tooltip(crate::ui::Tooltip::text(t(cx, "editor.connect_session_tip"), None))
                .on_click(cx.listener(|_, _: &ClickEvent, _, cx| cx.emit(EditorEvent::ToggleSession)))
                .into_any_element();
        };
        let follow = self.follow_agent;
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap_0p5()
            .child(
                div()
                    .id("editor-session")
                    .h(px(24.))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .rounded_md()
                    .cursor_pointer()
                    .border_1()
                    .border_color(hex_alpha(linked.color, 0.45))
                    .bg(hex_alpha(linked.color, 0.10))
                    .hover(|s| s.bg(hex_alpha(linked.color, 0.2)))
                    .t_small()
                    .text_color(hex(Chrome::BRIGHT))
                    .child(crate::brand::avatar_working(linked.brand, 14., linked.working, linked.pane_id, linked.color))
                    .child(linked.label.clone())
                    // Waiting for the user while the code is in front: said here, a click shows it.
                    .when(linked.waiting, |d| {
                        d.border_color(hex(Chrome::WARNING))
                            .child(div().size(px(7.)).rounded_full().bg(hex(Chrome::WARNING)))
                            .child(div().t_caption().text_color(hex(Chrome::WARNING)).child(t(cx, "editor.session_waiting")))
                    })
                    .tooltip(crate::ui::Tooltip::text(t(cx, "editor.session_tip"), None))
                    .on_click(cx.listener(|_, _: &ClickEvent, _, cx| cx.emit(EditorEvent::ToggleSession))),
            )
            .child(
                crate::ui::icon_only(
                    "editor-follow",
                    if follow { "eye" } else { "eye-off" },
                    cx.listener(|this, _: &ClickEvent, _, cx| this.toggle_follow(cx)),
                )
                .when(follow, |d| d.bg(hex(Chrome::SELECTED)))
                .tooltip(crate::ui::Tooltip::text(t(cx, if follow { "editor.follow_on" } else { "editor.follow_off" }), None)),
            )
            .into_any_element()
    }

    /// The names of agents at work in the file in front, over their carets.
    pub(super) fn render_presence_labels(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        // Drawn over the text only: never over a list, a question or a dialog of the workbench.
        let clear = !self.preview.covered && self.picker.is_none() && self.confirm.is_none();
        let Some(layout) = self.layout.as_ref().filter(|_| clear) else { return Vec::new() };
        let Some(doc) = self.docs.get(self.active) else { return Vec::new() };
        let Some((first, last)) = layout.shown_lines() else { return Vec::new() };
        let (top, bottom) = layout.corners();
        let mut labels = Vec::new();
        // The instruction sent, waiting for its answer: a note at the end of its first line.
        if let Some(asked) = self.asked.as_ref().filter(|a| a.path == doc.path && a.lines.0 < doc.buffer.line_count()) {
            let end = doc.buffer.line(asked.lines.0).len();
            if let Some(at) = layout.point_at(Pos::new(asked.lines.0, end)) {
                let text = if asked.label.is_empty() {
                    t(cx, "editor.asked").to_string()
                } else {
                    crate::i18n::tf(cx, "editor.asked_to", &[("agent", &asked.label)])
                };
                let note = div()
                    .px_1p5()
                    .h(px(16.))
                    .flex()
                    .items_center()
                    .rounded_sm()
                    .border_1()
                    .border_color(hex_alpha(asked.color, 0.7))
                    .text_size(px(10.5))
                    .text_color(hex(asked.color))
                    .child(text);
                labels.push(
                    gpui::deferred(gpui::anchored().position(gpui::point(at.x + px(14.), at.y + px(2.))).child(note))
                        .with_priority(1)
                        .into_any_element(),
                );
            }
        }
        // Agents at work above or below what is on screen: pinned to that edge, a click shows them.
        for p in self.presence_here().filter(|p| !p.label.is_empty() && (p.at.line < first || p.at.line > last)) {
            let above = p.at.line < first;
            let (line, pane_id) = (p.at.line, p.pane_id);
            let tag = div()
                .id(("editor-agent-edge", pane_id as usize))
                .flex()
                .items_center()
                .gap_1()
                .px_1p5()
                .h(px(18.))
                .rounded_sm()
                .cursor_pointer()
                .bg(hex(p.color))
                .shadow_md()
                .text_size(px(10.5))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(hex(0x111111))
                .child(if above { "↑" } else { "↓" })
                .child(p.label.clone())
                .child(format!("L{}", line + 1))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if let Some(doc) = this.docs.get_mut(this.active) {
                        doc.scroll_to_line = Some(line);
                    }
                    cx.notify();
                }));
            let (corner, at) = if above { (gpui::Corner::TopRight, top) } else { (gpui::Corner::BottomRight, bottom) };
            labels.push(gpui::deferred(gpui::anchored().anchor(corner).position(at).child(tag)).with_priority(1).into_any_element());
        }
        labels.extend(
            self.presence_here()
                .filter(|p| !p.label.is_empty() && p.at.line < doc.buffer.line_count() && p.at.line >= first && p.at.line <= last)
                .filter_map(|p| {
                    // After the end of its line, so the name never covers code.
                    let end = doc.buffer.line(p.at.line).len();
                    let at = layout.point_at(Pos::new(p.at.line, end))?;
                    let at = gpui::point(at.x + px(14.), at.y + px(2.));
                    let tag = div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_1p5()
                        .h(px(16.))
                        .rounded_sm()
                        .bg(hex(p.color))
                        .text_size(px(10.5))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(hex(0x111111))
                        .child(p.label.clone())
                        .when(p.pending, |d| d.child("…"));
                    Some(gpui::deferred(gpui::anchored().position(at).child(tag)).with_priority(1).into_any_element())
                }),
        );
        labels
    }

    // -- the selection bubble and asking the agent --------------------------------------------------

    /// Lines the selection covers, when there is one worth asking about.
    fn selected_lines(&self) -> Option<(usize, usize)> {
        let doc = self.docs.get(self.active).filter(|d| matches!(d.content, Content::Text) && !d.previewing())?;
        if doc.buffer.selection.is_empty() {
            return None;
        }
        let (start, end) = doc.buffer.selection.range();
        // A selection ending at the very start of a line does not take that line.
        let last = if end.col == 0 && end.line > start.line { end.line - 1 } else { end.line };
        Some((start.line, last))
    }

    /// Opens the instruction box for the selected lines (or the cursor's line).
    pub fn open_ask(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.docs.get(self.active).filter(|d| matches!(d.content, Content::Text)) else { return };
        let lines = self.selected_lines().unwrap_or((doc.buffer.cursor().line, doc.buffer.cursor().line));
        let (path, project) = (doc.path.clone(), doc.project.clone());
        let input = cx.new(|cx| TextInput::localized("", "editor.ask_placeholder", window, cx));
        let subscription = cx.subscribe_in(&input, window, |this, _, event: &TextInputEvent, window, cx| match event {
            TextInputEvent::Confirmed => this.send_ask(window, cx),
            TextInputEvent::Cancelled => this.close_ask(window, cx),
            _ => {}
        });
        input.update(cx, |input, cx| input.focus_and_select_all(window, cx));
        self.ask = Some(AskBox { input, path, project, lines, _subscription: subscription });
        cx.notify();
    }

    pub fn close_ask(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.ask.take().is_some() {
            window.focus(&self.focus_handle);
            cx.notify();
        }
    }

    pub(super) fn debug_send_ask(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.send_ask(window, cx);
    }

    fn send_ask(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.send_ask_as(false, window, cx);
    }

    /// Sends the instruction; `only_answer` asks without letting the agent change files (⌥↩).
    fn send_ask_as(&mut self, only_answer: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ask) = self.ask.as_ref() else { return };
        let instruction = ask.input.read(cx).text().trim().to_string();
        if instruction.is_empty() {
            return;
        }
        let rel = ask.path.strip_prefix(&ask.project).unwrap_or(&ask.path).to_string_lossy().replace('\\', "/");
        let mut prompt = ask_prompt(&rel, ask.lines, &instruction);
        if only_answer {
            prompt.push_str(" (Just answer; do not change any files.)");
        }
        let (path, lines) = (ask.path.clone(), ask.lines);
        self.send_instruction(path, lines, prompt, !only_answer, cx);
        self.close_ask(window, cx);
    }

    /// Sends `prompt`, about `lines` of `path`, to the linked session (or lets the user pick one),
    /// marking the lines until the agent changes them when it is asked to (`mark`).
    pub(super) fn send_instruction(&mut self, path: PathBuf, lines: (usize, usize), prompt: String, mark: bool, cx: &mut Context<Self>) {
        if mark {
            let linked = self.linked.clone();
            self.asked = Some(Asked {
                path,
                lines,
                pane_id: linked.as_ref().map(|l| l.pane_id),
                label: linked.as_ref().map(|l| l.label.clone()).unwrap_or_default(),
                color: linked.as_ref().map_or(Chrome::ACCENT, |l| l.color),
                since: Instant::now(),
            });
            self.keep_presence_fresh(cx);
        }
        crate::metrics::track(cx, "feature_used", serde_json::json!({ "feature": "editor_ask_agent" }));
        if let Some(doc) = self.docs.get_mut(self.active) {
            let cursor = doc.buffer.cursor();
            doc.buffer.move_to(cursor, false);
        }
        cx.emit(EditorEvent::AskAgent(prompt));
    }

    /// The bar over a selection, or the instruction box, placed under the selection's last line.
    pub(super) fn render_selection_tools(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        // Something of the workbench drawn over the editor (a dialog, a menu) hides the bars too.
        if self.drag.is_some() || self.picker.is_some() || self.confirm.is_some() || self.preview.covered {
            return None;
        }
        let layout = self.layout.as_ref()?;
        let doc = self.docs.get(self.active)?;
        if let Some(ask) = self.ask.as_ref() {
            let at = layout.point_below(Pos::new(ask.lines.1, 0))?;
            let linked = self.linked.as_ref().map(|l| l.label.clone());
            let lines = if ask.lines.0 == ask.lines.1 {
                format!("L{}", ask.lines.0 + 1)
            } else {
                format!("L{}–{}", ask.lines.0 + 1, ask.lines.1 + 1)
            };
            let input = ask.input.clone();
            let panel = div()
                .id("editor-ask")
                .key_context(ASK)
                .on_action(cx.listener(|this, _: &AskOnly, window, cx| this.send_ask_as(true, window, cx)))
                .w(px(460.))
                .flex()
                .flex_col()
                .gap_1()
                .p_2()
                .rounded_lg()
                .bg(hex(Chrome::OVERLAY))
                .border_1()
                .border_color(hex_alpha(Chrome::ACCENT, 0.7))
                .shadow_lg()
                .occlude()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .t_caption()
                        .text_color(hex(Chrome::MUTED))
                        .child(icon("sparkles", IconSize::INLINE, hex(Chrome::ACCENT)))
                        .child(lines)
                        .child(div().flex_1())
                        .child(match linked {
                            Some(label) => crate::i18n::tf(cx, "editor.ask_to", &[("agent", &label)]),
                            None => t(cx, "editor.ask_no_session").to_string(),
                        }),
                )
                .child(
                    div()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .bg(hex(Chrome::EDITOR))
                        .border_1()
                        .border_color(hex(Chrome::BORDER))
                        .t_body()
                        .text_color(hex(Chrome::BRIGHT))
                        .child(input),
                )
                .child(div().t_caption().text_color(hex(Chrome::MUTED)).child(t(cx, "editor.ask_keys")));
            return Some(
                gpui::deferred(gpui::anchored().position(at).snap_to_window_with_margin(self.popup_margin()).child(panel))
                    .with_priority(2)
                    .into_any_element(),
            );
        }
        // Find's matches and a snippet's placeholders are selected to be typed over, not acted on.
        if self.find.is_some() || self.snippet.is_some() {
            return None;
        }
        let (start, end) = doc.buffer.selection.range();
        if start == end {
            return None;
        }
        let at = layout.point_below(Pos::new(end.line, end.col))?;
        let single = start.line == end.line;
        let button = |id: &'static str, glyph: &'static str, label: String| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_1()
                .px_2()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .hover(|s| s.bg(hex(Chrome::HOVER)))
                .t_small()
                .text_color(hex(Chrome::BRIGHT))
                .child(icon(glyph, IconSize::INLINE, hex(Chrome::FOREGROUND)))
                .child(label)
        };
        let bar = div()
            .id("editor-selection-bar")
            .flex()
            .items_center()
            .gap_0p5()
            .p_0p5()
            .rounded_lg()
            .bg(hex(Chrome::OVERLAY))
            .border_1()
            .border_color(hex(Chrome::OVERLAY_BORDER))
            .shadow_lg()
            .occlude()
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                button("editor-sel-ask", "sparkles", t(cx, "editor.ask_ai").to_string())
                    .text_color(hex(Chrome::ACCENT))
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.open_ask(window, cx))),
            )
            .child(button("editor-sel-copy", "copy", t(cx, "editor.copy").to_string()).on_click(cx.listener(
                |this, _: &ClickEvent, _, cx| {
                    if let Some(doc) = this.docs.get(this.active) {
                        cx.write_to_clipboard(ClipboardItem::new_string(doc.buffer.selected_text()));
                    }
                },
            )))
            .when(single, |d| {
                d.child(
                    button("editor-sel-definition", "arrow-up-right", t(cx, "nav.definition").to_string())
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.go_to_definition(window, cx))),
                )
                .child(
                    button("editor-sel-usages", "search", t(cx, "nav.find_usages").to_string())
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.find_usages(window, cx))),
                )
            });
        Some(
            gpui::deferred(gpui::anchored().position(at).snap_to_window_with_margin(self.popup_margin()).child(bar))
                .with_priority(2)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prompt_names_the_file_and_lines() {
        assert_eq!(ask_prompt("src/app.ts", (9, 19), " add error handling "), "In src/app.ts, lines 10-20: add error handling");
        assert_eq!(ask_prompt("main.go", (4, 4), "rename this"), "In main.go, line 5: rename this");
    }

    #[test]
    fn short_ids_are_four_characters() {
        assert_eq!(short_id(Some("3F9A-77c1-…"), 1), "3f9a");
        assert_eq!(short_id(Some("ab"), 7).len(), 4, "too short a session id: the pane's number");
        assert_eq!(short_id(None, 7), short_id(None, 7));
        assert_ne!(short_id(None, 7), short_id(None, 8));
    }

    #[test]
    fn changed_lines_cover_insertions_edits_and_deletions() {
        assert_eq!(changed_lines("a\nb\nc", "a\nb\nc"), None);
        assert_eq!(changed_lines("a\nb\nc", "a\nB\nc"), Some((1, 1)));
        assert_eq!(changed_lines("a\nc", "a\nb1\nb2\nc"), Some((1, 2)));
        assert_eq!(changed_lines("a\nb\nc", "a\nc"), Some((1, 1)), "a deletion: where the text went");
        assert_eq!(changed_lines("", "x\ny"), Some((0, 1)));
        assert_eq!(changed_lines("a\nb", "a\nb\nc"), Some((2, 2)));
    }
}
