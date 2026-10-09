//! Parallel tasks an agent asks for (`agentty tasks`): nothing starts until the user says so in a
//! dialog. Then every task gets a working tree of its own (a new branch from the project's default
//! branch, like a second session of a project) and a pane split off the asking agent's — from three
//! tasks on a tab of its own, so the asking agent keeps its room — where Claude Code or Codex starts
//! with the task's prompt. The asking agent hears which tasks started and where.
//!
//! Requests from several agents wait in line; one dialog shows all the tasks of one request.

use super::panes::Axis;
use super::{Pane, Workbench};
use crate::agent_signal::{browser_reply, TasksRequest};
use crate::i18n::{t, tf};
use crate::launch::LaunchSpec;
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, tilde, TypeScale};
use agentty_bridge::model::Agent;
use gpui::{div, prelude::*, px, AnyElement, AppContext, ClickEvent, Context, SharedString, Window};
use std::path::PathBuf;

pub(super) fn agent_of(name: Option<&str>) -> Agent {
    if name == Some("codex") {
        Agent::Codex
    } else {
        Agent::Claude
    }
}

/// Up to two tasks split off the asking agent's pane; from three on, each gets a tab of its own.
fn opens_as_tabs(count: usize) -> bool {
    count >= 3
}

impl Workbench {
    /// An agent asked to start tasks: queue the question (turned off in Settings: refuse at once).
    pub fn ask_to_start_tasks(&mut self, mut request: TasksRequest, cx: &mut Context<Self>) {
        if !crate::settings::settings(cx).agent_tasks {
            let _ = request.reply.send(browser_reply(Err("starting parallel tasks is turned off in Agentty's settings".into())));
            return;
        }
        request.cwd = self.tasks_project(&request, cx);
        // A chat's lead starts its workers without asking: opening the chat was the user's yes.
        let Some(request) = self.start_chat_tasks(request, cx) else { return };
        // A board ticket's agent splits its work without asking: moving the ticket to "instructed"
        // was the user's yes. The tasks start from the ticket's branch.
        let board_base = self.pane_by_id(request.pane, cx).and_then(|pane| self.board_task_base(&pane));
        let split = crate::settings::settings(cx).board.split;
        if board_base.is_some() && split == crate::settings::BoardSplit::Off {
            let _ = request
                .reply
                .send(browser_reply(Err("splitting board tickets is turned off in Agentty's settings: do the work yourself".into())));
            return;
        }
        if let Some(base) = board_base.filter(|_| split == crate::settings::BoardSplit::Auto) {
            let handle = self.window_handle;
            cx.spawn(async move |this, cx| {
                let _ = cx.update_window(handle, |_, window, cx| {
                    let _ = this.update(cx, |this, cx| this.start_tasks(request, base, window, cx));
                });
            })
            .detach();
            return;
        }
        self.task_requests.push_back(request);
        cx.notify();
    }

    /// The folder the tasks' working trees are made from. The command runs wherever the agent's shell
    /// is — often a scratch folder outside the project — and a folder outside git would start every
    /// task right there, without a tree of its own. Then the asking agent's own folder is the project:
    /// where the pane works now or, since that is read from whatever runs in front (the very shell
    /// that sent the command, still in the scratch folder), the folder the pane was started in.
    fn tasks_project(&self, request: &TasksRequest, cx: &gpui::App) -> PathBuf {
        let in_git = |dir: &std::path::Path| agentty_bridge::worktree::tree_root(dir).is_some();
        if in_git(&request.cwd) {
            return request.cwd.clone();
        }
        let Some(pane) = self.pane_by_id(request.pane, cx) else { return request.cwd.clone() };
        let pane = pane.read(cx);
        [pane.display_cwd(), pane.spec.cwd.clone()].into_iter().find(|dir| in_git(dir)).unwrap_or_else(|| request.cwd.clone())
    }

    fn decline_tasks(&mut self, cx: &mut Context<Self>) {
        if let Some(request) = self.task_requests.pop_front() {
            let _ = request.reply.send(browser_reply(Err("the user declined the tasks".into())));
        }
        cx.notify();
    }

    fn pane_by_id(&self, pane_id: u64, cx: &gpui::App) -> Option<Pane> {
        self.all_panes().into_iter().find(|p| p.read(cx).pane_id == pane_id)
    }

    /// The user said yes: working trees first (in the background), then one pane per task — the first
    /// to the right of the asking agent, the next ones below it (a tab each from three tasks on) — and
    /// the answer to the agent.
    fn start_tasks(&mut self, request: TasksRequest, base: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(caller) = self.pane_by_id(request.pane, cx) else {
            let _ = request.reply.send(browser_reply(Err("the asking pane is gone".into())));
            return cx.notify();
        };
        let handle = window.window_handle();
        let (cwd, tasks) = (request.cwd.clone(), request.tasks.clone());
        let as_tabs = opens_as_tabs(tasks.len());
        cx.spawn(async move |this, cx| {
            let mut started: Vec<serde_json::Value> = Vec::new();
            let mut problems: Vec<String> = Vec::new();
            let mut previous: Option<Pane> = None;
            for task in tasks {
                // A folder outside git has no working trees: the task starts there, next to the asking agent.
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
                let folder: PathBuf = tree.as_ref().map(|t| t.path.clone()).unwrap_or_else(|| cwd.clone());
                let branch = tree.as_ref().and_then(|t| t.branch.clone());
                let spec =
                    LaunchSpec::with_prompt(agent_of(task.agent.as_deref()), task.prompt.clone(), task.title.clone(), folder.clone());
                let anchor = previous.clone().unwrap_or_else(|| caller.clone());
                let axis = if previous.is_some() { Axis::Vertical } else { Axis::Horizontal };
                let opened = cx
                    .update_window(handle, |_, window, cx| {
                        this.update(cx, |this, cx| {
                            if as_tabs {
                                this.tab_beside(&caller, spec, cx)
                            } else {
                                this.split_pane_with(&anchor, spec, axis, window, cx)
                            }
                        })
                        .ok()
                        .flatten()
                    })
                    .ok()
                    .flatten();
                match opened {
                    Some(pane) => {
                        let _ = this.update(cx, |this, cx| this.board_adopt(&pane, cx));
                        previous = Some(pane);
                        started.push(serde_json::json!({ "title": task.title, "branch": branch, "folder": folder }));
                        if tree.is_none() {
                            // Said out loud: the asking agent must not take the shared folder for a tree of its own.
                            problems.push(format!(
                                "{}: {} is not in a git repository, so the task started there without a worktree of its own",
                                task.title,
                                folder.display()
                            ));
                        }
                    }
                    None => problems.push(format!("{}: the asking agent's tab is gone", task.title)),
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
        cx.notify();
    }

    /// Adds a tab running `spec` to `anchor`'s workspace, behind the tab on screen: many tasks at
    /// once would squeeze the asking agent into a sliver if they were all split off its pane.
    fn tab_beside(&mut self, anchor: &Pane, spec: LaunchSpec, cx: &mut Context<Self>) -> Option<Pane> {
        let (w, _) = self.locate(anchor)?;
        self.wake_for_new_tab(w, cx);
        let pane = self.spawn_pane(spec, cx);
        self.workspaces[w].tabs.push(super::Tab { root: super::panes::PaneNode::Leaf(pane.clone()), active: pane.clone(), instance: None });
        self.persist(cx);
        cx.notify();
        Some(pane)
    }

    /// Splits `anchor` (wherever it is) with a new pane running `spec`, and shows it.
    pub(super) fn split_pane_with(
        &mut self,
        anchor: &Pane,
        spec: LaunchSpec,
        axis: Axis,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Pane> {
        let (w, t) = self.locate(anchor)?;
        let pane = self.spawn_pane(spec, cx);
        let ws = &mut self.workspaces[w];
        let tab = &mut ws.tabs[t];
        tab.root.split(anchor, pane.clone(), axis);
        tab.active = pane.clone();
        ws.active_tab = t;
        self.active_workspace = w;
        self.page = None;
        self.welcome = false;
        self.focus_pane(&pane, window, cx);
        self.persist(cx);
        cx.notify();
        Some(pane)
    }

    pub(super) fn render_tasks_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let request = self.task_requests.front()?;
        let waiting = self.task_requests.len() - 1;
        let caller = self.pane_by_id(request.pane, cx);
        let who = caller.as_ref().map(|p| p.read(cx).display_title()).unwrap_or_default();
        let in_git = agentty_bridge::worktree::tree_root(&request.cwd).is_some();
        let mut list = div().id("tasks-dialog-list").max_h(px(340.)).overflow_y_scroll().flex().flex_col().gap_2();
        for (index, task) in request.tasks.iter().enumerate() {
            let agent = if task.agent.as_deref() == Some("codex") { "Codex" } else { "Claude Code" };
            // The whole prompt, not a preview: what is approved here runs as a new agent session, and
            // an instruction below the first few lines must be as visible as the ones above it.
            let preview: String = task.prompt.lines().filter(|l| !l.trim().is_empty()).collect::<Vec<_>>().join("\n");
            list = list.child(
                div()
                    .id(SharedString::from(format!("tasks-dialog-task-{index}")))
                    .p_2()
                    .rounded_md()
                    .bg(hex(0x1a1a1a))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .t_body()
                                    .font_weight(crate::theme::EMPHASIS)
                                    .text_color(hex(Chrome::BRIGHT))
                                    .child(task.title.clone()),
                            )
                            .child(div().t_small().text_color(hex(Chrome::MUTED)).child(agent)),
                    )
                    .child(div().t_small().text_color(hex(Chrome::FOREGROUND)).child(preview)),
            );
        }
        let button = |id: &'static str, label: String, primary: bool| {
            div()
                .id(id)
                .px_3()
                .py_1p5()
                .rounded_md()
                .t_body()
                .cursor_pointer()
                .bg(if primary { hex(Chrome::ACCENT) } else { hex(0x2d2d30) })
                .text_color(hex(Chrome::BRIGHT))
                .hover(|s| s.opacity(0.85))
                .child(label)
        };
        Some(
            div()
                .id("tasks-dialog-overlay")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(hex_alpha(0x000000, 0.45))
                .occlude()
                .child(
                    div()
                        .id("tasks-dialog")
                        .w(px(620.))
                        .max_h(gpui::relative(0.9))
                        .p_5()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .rounded_xl()
                        .bg(hex(Chrome::OVERLAY))
                        .border_1()
                        .border_color(hex(Chrome::OVERLAY_BORDER))
                        .shadow_lg()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(icon("columns-2", crate::ui::IconSize::BUTTON, hex(Chrome::BRIGHT)))
                                .child(div().t_title().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(tf(
                                    cx,
                                    "tasks.title",
                                    &[("n", &request.tasks.len().to_string())],
                                )))
                                .child(div().flex_1())
                                .when(waiting > 0, |d| {
                                    d.child(div().t_small().text_color(hex(Chrome::MUTED)).child(tf(
                                        cx,
                                        "tasks.waiting",
                                        &[("n", &waiting.to_string())],
                                    )))
                                }),
                        )
                        .child(div().t_small().text_color(hex(Chrome::MUTED)).child(tf(
                            cx,
                            "tasks.from",
                            &[("name", &who), ("folder", &tilde(&request.cwd))],
                        )))
                        .child(list)
                        .child(
                            div()
                                .t_small()
                                .text_color(hex(Chrome::MUTED))
                                .child(t(cx, if in_git { "tasks.how_trees" } else { "tasks.how_here" })),
                        )
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .child(
                                    button("tasks-decline", t(cx, "tasks.decline").to_string(), false)
                                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.decline_tasks(cx))),
                                )
                                .child(button("tasks-start", t(cx, "tasks.start").to_string(), true).on_click(cx.listener(
                                    |this, _: &ClickEvent, window, cx| {
                                        if let Some(request) = this.task_requests.pop_front() {
                                            // A board ticket's tasks start from its branch, asked or not.
                                            let base =
                                                this.pane_by_id(request.pane, cx).and_then(|pane| this.board_task_base(&pane)).flatten();
                                            this.start_tasks(request, base, window, cx);
                                        }
                                    },
                                ))),
                        ),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::opens_as_tabs;

    #[test]
    fn two_tasks_split_and_three_or_more_open_as_tabs() {
        assert!(!opens_as_tabs(1));
        assert!(!opens_as_tabs(2));
        assert!(opens_as_tabs(3));
        assert!(opens_as_tabs(6));
    }
}
