//! Parallel tasks an agent asks for (`agentty tasks`): nothing starts until the user says so in a
//! dialog. Then every task gets a working tree of its own (a new branch from the project's default
//! branch, like a second session of a project) and a pane split off the asking agent's, where
//! Claude Code or Codex starts with the task's prompt. The asking agent hears which tasks started
//! and where.
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

/// Lines of each prompt shown in the dialog.
const PREVIEW_LINES: usize = 4;

fn agent_of(name: Option<&str>) -> Agent {
    if name == Some("codex") {
        Agent::Codex
    } else {
        Agent::Claude
    }
}

impl Workbench {
    /// An agent asked to start tasks: queue the question (turned off in Settings: refuse at once).
    pub fn ask_to_start_tasks(&mut self, request: TasksRequest, cx: &mut Context<Self>) {
        if !crate::settings::settings(cx).agent_tasks {
            let _ = request.reply.send(browser_reply(Err("starting parallel tasks is turned off in Agentty's settings".into())));
            return;
        }
        self.task_requests.push_back(request);
        cx.notify();
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
    /// to the right of the asking agent, the next ones below it — and the answer to the agent.
    fn start_tasks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(request) = self.task_requests.pop_front() else { return };
        let Some(caller) = self.pane_by_id(request.pane, cx) else {
            let _ = request.reply.send(browser_reply(Err("the asking pane is gone".into())));
            return cx.notify();
        };
        let handle = window.window_handle();
        let (cwd, tasks) = (request.cwd.clone(), request.tasks.clone());
        cx.spawn(async move |this, cx| {
            let mut started: Vec<serde_json::Value> = Vec::new();
            let mut problems: Vec<String> = Vec::new();
            let mut previous: Option<Pane> = None;
            for task in tasks {
                // A folder outside git has no working trees: the task starts there, next to the asking agent.
                let in_git = agentty_bridge::worktree::tree_root(&cwd).is_some();
                let (source, label) = (cwd.clone(), task.title.clone());
                let tree = if in_git {
                    match cx.background_spawn(async move { agentty_bridge::worktree::create(&source, &label) }).await {
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
                        this.update(cx, |this, cx| this.split_pane_with(&anchor, spec, axis, window, cx)).ok().flatten()
                    })
                    .ok()
                    .flatten();
                match opened {
                    Some(pane) => {
                        previous = Some(pane);
                        started.push(serde_json::json!({ "title": task.title, "branch": branch, "folder": folder }));
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
            let preview: String = task.prompt.lines().filter(|l| !l.trim().is_empty()).take(PREVIEW_LINES).collect::<Vec<_>>().join("\n");
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
                                .child(
                                    button("tasks-start", t(cx, "tasks.start").to_string(), true)
                                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.start_tasks(window, cx))),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}
