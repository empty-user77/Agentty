//! Chat view's look: a pane's session as a chat app shows it. A bar with the conversation's name
//! and the pane's tools, the conversation in a readable column (replies with their steps folded
//! into cards, the agent's to-do list, its questions as buttons), the composer with what it runs
//! on under it, and a panel beside it with where it works, its context, plan usage and connectors.

use super::*;
use crate::workbench::{Page, SidePanel};
use agentty_bridge::chat_log::{Block, Todo, TodoStatus, ToolStep};
use agentty_bridge::extensions::{McpHealth, McpStatus};
use gpui::{HighlightStyle, MouseButton, MouseDownEvent, PathPromptOptions, StyledText};

/// How often the panel reads the agent's MCP servers again.
pub(super) const MCP_EVERY: Duration = Duration::from_secs(300);
/// The panel beside the conversation comes up from this pane width.
const SIDE_PANEL_FROM: f32 = 980.;
const SIDE_PANEL_WIDTH: f32 = 300.;
/// Steps of a card shown before the earlier ones fold away.
const STEPS_SHOWN: usize = 4;
/// Sessions read for the start screen, and how many of those in this folder it shows.
const RECENT_SCANNED: usize = 200;
const RECENT_SHOWN: usize = 4;
/// Height of the conversation's bar.
const BAR_HEIGHT: f32 = 38.;

/// The permission mode Claude Code shows under its input box (Shift+Tab moves to the next one).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode {
    Ask,
    Edits,
    Plan,
    Auto,
    Bypass,
}

impl Mode {
    fn of(lines: &[String]) -> Mode {
        let screen = lines.iter().rev().take(6).map(|l| l.to_lowercase()).collect::<Vec<_>>().join("\n");
        if screen.contains("bypass permissions on") {
            Mode::Bypass
        } else if screen.contains("accept edits on") {
            Mode::Edits
        } else if screen.contains("plan mode on") {
            Mode::Plan
        } else if screen.contains("auto mode on") {
            Mode::Auto
        } else {
            Mode::Ask
        }
    }

    fn label(self) -> &'static str {
        match self {
            Mode::Ask => "chat.mode_ask",
            Mode::Edits => "chat.mode_edits",
            Mode::Plan => "chat.mode_plan",
            Mode::Auto => "chat.mode_auto",
            Mode::Bypass => "chat.mode_bypass",
        }
    }
}

/// The reasoning effort Claude Code shows over its input box (`◐ medium · /effort`).
fn effort_of(lines: &[String]) -> Option<&'static str> {
    let line = lines.iter().rev().take(12).find(|l| l.contains("/effort"))?.to_lowercase();
    ["xhigh", "max", "high", "medium", "low"].into_iter().find(|level| line.contains(level)).map(|level| match level {
        "xhigh" => "chat.effort_xhigh",
        "max" => "chat.effort_max",
        "high" => "chat.effort_high",
        "medium" => "chat.effort_medium",
        _ => "chat.effort_low",
    })
}

/// What the screen and the pane say about the agent, read once per frame.
struct Live {
    tool: &'static str,
    agent: agentty_bridge::model::Agent,
    title: String,
    working: bool,
    activity: Option<String>,
    elapsed: Option<String>,
    model: Option<String>,
    context: Option<(f64, u64, u64)>,
    choices: Option<(Vec<String>, usize)>,
    question: Option<String>,
    mode: Option<Mode>,
    effort: Option<&'static str>,
    folder: PathBuf,
    branch: Option<String>,
    worktree: bool,
    width: f32,
}

impl Workbench {
    /// Whether `pane` is drawn as chat view's chat right now (not its terminal).
    pub(in crate::workbench) fn plain_chat_in_front(&self, pane: &Pane, cx: &App) -> bool {
        self.chats.get(&pane.entity_id()).is_some_and(|state| state.plain && self.chat_in_front(state, cx))
    }

    fn live(&self, state: &ChatState, cx: &App) -> Live {
        let pane = &state.lead;
        let view = pane.read(cx);
        let agent = chat_lead_agent(pane, cx);
        let lines = view.screen_lines(40);
        let waits = waits_on_user(view);
        let title = state
            .shown
            .iter()
            .find_map(|item| match item {
                ChatItem::User(text) => Some(text.lines().next().unwrap_or_default().chars().take(80).collect::<String>()),
                _ => None,
            })
            .unwrap_or_else(|| view.display_title());
        Live {
            tool: view.tool_id(),
            agent,
            title,
            working: view.status.in_turn(),
            activity: view.last_tool.as_ref().map(|(tool, target)| tool_text(tool, target.as_deref(), cx)),
            elapsed: view.working_since.map(|since| crate::workbench::layout::format_elapsed(since.elapsed().as_secs())),
            model: self.pane_model_text(pane, cx),
            context: view.stats.as_ref().and_then(|stats| stats.context_percent().map(|p| (p, stats.context_used, stats.context_window))),
            choices: waits.then(|| screen_choices(&lines)).flatten(),
            question: match &view.status {
                AgentStatus::Permission(Some(what)) | AgentStatus::Question(Some(what)) => Some(what.clone()),
                _ => None,
            }
            .or_else(|| crate::terminal::classify_screen(&lines).permission),
            mode: (agent == agentty_bridge::model::Agent::Claude).then(|| Mode::of(&lines)),
            effort: effort_of(&lines),
            folder: view.display_cwd(),
            branch: view.git_branch.clone(),
            worktree: view.worktree.is_some(),
            width: self.pane_bounds.borrow().get(&pane.entity_id()).map(|b| f32::from(b.size.width)).unwrap_or(f32::MAX),
        }
    }

    /// A pane chat view draws as a chat: its conversation in place of its terminal. None when the
    /// pane is not one, or its terminal is in front.
    pub(in crate::workbench) fn render_plain_chat(&self, pane: &Pane, cx: &mut Context<Self>) -> Option<AnyElement> {
        let state = self.chats.get(&pane.entity_id()).filter(|state| state.plain)?;
        if !self.chat_in_front(state, cx) {
            return None;
        }
        let id = pane.entity_id();
        let live = self.live(state, cx);
        let side = state.side_open && live.width >= SIDE_PANEL_FROM;

        let messages: AnyElement =
            if state.shown.is_empty() { self.render_chat_welcome(state, &live, cx) } else { message_list(state, cx) };
        let approval = live.choices.clone().map(|choices| self.render_chat_question(id, choices, live.question.clone(), cx));
        let activity = (live.working && approval.is_none()).then(|| {
            let mut text = live.activity.clone().unwrap_or_else(|| t(cx, "chat.working").to_string());
            if let Some(elapsed) = &live.elapsed {
                text = format!("{text}  {elapsed}");
            }
            div().flex_shrink_0().px_4().pb_1().child(column(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(crate::ui::spinner(13., hex(Chrome::ORANGE)))
                    .child(div().t_small().truncate().text_color(hex(Chrome::MUTED)).child(text)),
            ))
        });
        let composer = self.render_chat_composer(state, &live, cx);
        let target = pane.clone();
        let main = div()
            .relative()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .child(messages)
            .children(approval)
            .children(activity)
            .children(composer)
            .children(state.model_menu.then(|| self.render_model_menu(id, &live, cx)));
        Some(
            div()
                .id(SharedString::from(format!("plain-chat-{}", id.as_u64())))
                .size_full()
                .flex()
                .flex_col()
                .bg(hex(Chrome::EDITOR))
                .relative()
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _: &MouseDownEvent, _, cx| this.mark_active(&target, cx)))
                .child(self.render_chat_bar(pane, state, &live, cx))
                .children(state.bar_menu.then(|| self.render_bar_menu(pane, cx)))
                .child(div().flex_1().min_h_0().flex().child(main).when(side, |d| d.child(self.render_chat_panel(state, &live, cx))))
                .into_any_element(),
        )
    }

    /// The conversation's bar: its name (the pane's menu, with Rename), the panel beside it, and a
    /// menu with the rest. The browser and files buttons stay in the window's own bar above.
    fn render_chat_bar(&self, pane: &Pane, state: &ChatState, live: &Live, cx: &mut Context<Self>) -> impl IntoElement {
        let id = pane.entity_id();
        let menu_pane = pane.clone();
        let button = |key: &'static str, glyph: &'static str, active: bool, tip: String| {
            div()
                .id(key)
                .flex_shrink_0()
                .size(px(28.))
                .rounded_md()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .when(active, |d| d.bg(hex(Chrome::SELECTED)))
                .hover(|s| s.bg(hex(Chrome::HOVER)))
                .tooltip(crate::ui::Tooltip::text(tip, None))
                .child(icon(glyph, 15., hex(if active { Chrome::BRIGHT } else { Chrome::MUTED })))
        };
        div()
            .flex_shrink_0()
            .h(px(BAR_HEIGHT))
            .px_3()
            .flex()
            .items_center()
            .gap_1()
            .border_b_1()
            .border_color(hex(Chrome::BORDER))
            .child(
                div()
                    .id("chat-title")
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .child(
                        div()
                            .truncate()
                            .t_body()
                            .font_weight(crate::theme::EMPHASIS)
                            .text_color(hex(Chrome::BRIGHT))
                            .child(live.title.clone()),
                    )
                    .child(icon("chevron-down", 13., hex(Chrome::MUTED)))
                    .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                        this.open_pane_menu(menu_pane.clone(), event.position(), cx);
                    })),
            )
            .child(div().flex_1())
            // Only where the panel fits: a narrow pane never shows it, so the switch would do nothing.
            .children((live.width >= SIDE_PANEL_FROM).then(|| {
                button("chat-bar-panel", "list", state.side_open, t(cx, "chat.side_panel").to_string()).on_click(cx.listener(
                    move |this, _: &ClickEvent, _, cx| {
                        if let Some(state) = this.chats.get_mut(&id) {
                            state.side_open = !state.side_open;
                            cx.notify();
                        }
                    },
                ))
            }))
            .child(button("chat-bar-more", "ellipsis", state.bar_menu, t(cx, "chat.more").to_string()).on_click(cx.listener(
                move |this, _: &ClickEvent, _, cx| {
                    // Opens only: a click on it while open already closed the menu (outside it).
                    if let Some(state) = this.chats.get_mut(&id) {
                        state.bar_menu = true;
                        cx.notify();
                    }
                },
            )))
    }

    /// The bar's `⋯` menu: the terminal, Git, and the pane's own menu.
    fn render_bar_menu(&self, pane: &Pane, cx: &mut Context<Self>) -> AnyElement {
        let id = pane.entity_id();
        let pane_menu = pane.clone();
        let close = move |this: &mut Self, cx: &mut Context<Self>| {
            if let Some(state) = this.chats.get_mut(&id) {
                state.bar_menu = false;
            }
            cx.notify();
        };
        let row = |key: &'static str, glyph: &'static str, label: String| {
            div()
                .id(key)
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .py_1p5()
                .rounded_md()
                .cursor_pointer()
                .t_body()
                .text_color(hex(Chrome::BRIGHT))
                .hover(|s| s.bg(hex(Chrome::HOVER)))
                .child(icon(glyph, 14., hex(Chrome::MUTED)))
                .child(label)
        };
        div()
            .id("chat-bar-menu")
            .absolute()
            .top(px(BAR_HEIGHT - 2.))
            .right(px(12.))
            .w(px(200.))
            .p_1()
            .rounded_lg()
            .border_1()
            .border_color(hex(Chrome::BORDER))
            .bg(hex(Chrome::PANEL))
            .shadow_lg()
            .flex()
            .flex_col()
            .on_mouse_down_out(cx.listener(move |this, _: &MouseDownEvent, _, cx| close(this, cx)))
            .child(row("chat-menu-terminal", "square-terminal", t(cx, "chat.show_terminal").to_string()).on_click(cx.listener(
                move |this, _: &ClickEvent, window, cx| {
                    close(this, cx);
                    this.toggle_chat_terminal(id, window, cx);
                },
            )))
            .child(row("chat-menu-git", "git-branch", t(cx, "page.git").to_string()).on_click(cx.listener(
                move |this, _: &ClickEvent, _, cx| {
                    close(this, cx);
                    this.open_page(Page::Git, cx);
                },
            )))
            .child(row("chat-menu-pane", "ellipsis", t(cx, "chat.pane_menu").to_string()).on_click(cx.listener(
                move |this, event: &ClickEvent, _, cx| {
                    close(this, cx);
                    this.open_pane_menu(pane_menu.clone(), event.position(), cx);
                },
            )))
            .into_any_element()
    }

    /// Before the first message, as a chat app's start screen: what runs where (agent, model,
    /// folder, branch), a few ways to begin that fill the composer without sending, and the
    /// agent's earlier sessions in this folder to pick up again.
    fn render_chat_welcome(&self, state: &ChatState, live: &Live, cx: &mut Context<Self>) -> AnyElement {
        let id = state.lead.entity_id();
        let wide = live.width >= 760.;
        // Where it runs, one line; the folder and branch open what they name.
        let fact = |key: &'static str, glyph: &'static str, text: String| {
            div()
                .id(key)
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap_1()
                .px_2()
                .py_0p5()
                .rounded_md()
                .t_small()
                .text_color(hex(Chrome::MUTED))
                .child(icon(glyph, 12., hex(Chrome::MUTED)))
                .child(div().truncate().max_w(px(220.)).child(text))
        };
        let agent_name = match live.agent {
            agentty_bridge::model::Agent::Codex => "Codex",
            _ => "Claude Code",
        };
        let facts = div()
            .flex()
            .flex_wrap()
            .justify_center()
            .gap_1()
            .child(fact(
                "chat-fact-agent",
                "bot",
                match &live.model {
                    Some(model) => format!("{agent_name} · {model}"),
                    None => agent_name.to_string(),
                },
            ))
            .child(
                fact("chat-fact-folder", "folder", folder_name(&live.folder))
                    .cursor_pointer()
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .tooltip(crate::ui::Tooltip::text(crate::ui::tilde(&live.folder), None))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.open_files_panel(None, cx))),
            )
            .children(live.branch.clone().map(|branch| {
                fact("chat-fact-branch", if live.worktree { "git-fork" } else { "git-branch" }, branch)
                    .cursor_pointer()
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.open_page(Page::Git, cx)))
            }));

        // Ways to begin: each fills the composer, to be edited and sent by the user.
        let starters: [(&'static str, &'static str); 4] = [
            ("book-open", "chat.start_explain"),
            ("git-pull-request", "chat.start_review"),
            ("wrench", "chat.start_tests"),
            ("play", "chat.start_run"),
        ];
        let starter = |index: usize, glyph: &'static str, key: &'static str, cx: &mut Context<Self>| {
            let text = t(cx, key).to_string();
            let fill = text.clone();
            div()
                .id(SharedString::from(format!("chat-start-{index}")))
                .flex_1()
                .min_w_0()
                .flex()
                .items_start()
                .gap_2p5()
                .px_3()
                .py_2p5()
                .rounded_xl()
                .border_1()
                .border_color(hex(Chrome::BORDER))
                .cursor_pointer()
                .hover(|s| s.bg(hex(Chrome::HOVER)).border_color(hex(Chrome::SELECTED)))
                .child(div().pt(px(2.)).child(icon(glyph, 15., hex(Chrome::ORANGE))))
                .child(div().flex_1().min_w_0().t_body().text_color(hex(Chrome::FOREGROUND)).child(text))
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.fill_chat_composer(id, &fill, window, cx)))
        };
        let mut grid = div().w_full().flex().flex_col().gap_2();
        if wide {
            for (row, pair) in starters.chunks(2).enumerate() {
                let mut line = div().w_full().flex().gap_2();
                for (n, (glyph, key)) in pair.iter().enumerate() {
                    line = line.child(starter(row * 2 + n, glyph, key, cx));
                }
                grid = grid.child(line);
            }
        } else {
            for (index, (glyph, key)) in starters.iter().enumerate() {
                grid = grid.child(starter(index, glyph, key, cx));
            }
        }

        // Earlier sessions here: picked up where they were left, in a workspace of their own.
        let now = crate::ui::now_ms();
        let recent = state.recent.as_ref().filter(|sessions| !sessions.is_empty()).map(|sessions| {
            let rows = sessions.iter().enumerate().map(|(index, session)| {
                let picked = session.clone();
                let title = Some(session.title.trim())
                    .filter(|t| !t.is_empty())
                    .map(str::to_string)
                    .or_else(|| session.last_prompt.clone())
                    .unwrap_or_else(|| session.id.chars().take(8).collect());
                div()
                    .id(SharedString::from(format!("chat-recent-{index}")))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .child(icon("history", 14., hex(Chrome::MUTED)))
                    .child(div().flex_1().min_w_0().truncate().t_body().text_color(hex(Chrome::FOREGROUND)).child(title))
                    .child(
                        div()
                            .flex_shrink_0()
                            .t_caption()
                            .text_color(hex(Chrome::MUTED))
                            .child(crate::ui::relative_time(now, session.updated_at)),
                    )
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.resume_session(&picked, window, cx)))
            });
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .px_2()
                        .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "chat.recent_here")))
                        .child(
                            div()
                                .id("chat-recent-all")
                                .t_small()
                                .text_color(hex(Chrome::MUTED))
                                .cursor_pointer()
                                .hover(|s| s.text_color(hex(Chrome::BRIGHT)))
                                .child(t(cx, "chat.recent_all"))
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.show_panel(SidePanel::Sessions, cx))),
                        ),
                )
                .children(rows)
        });

        div()
            .id("chat-welcome")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .items_center()
            .px_6()
            .py_8()
            .child(div().flex_1().min_h(px(16.)))
            .child(
                div()
                    .w_full()
                    .max_w(px(640.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_6()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap_2()
                            .child(crate::brand::avatar(live.tool, 44.))
                            .child(div().pt_1().t_display().text_color(hex(Chrome::BRIGHT)).child(t(cx, "chat.plain_empty")))
                            .child(facts),
                    )
                    .child(grid)
                    .children(recent),
            )
            .child(div().flex_1().min_h(px(16.)))
            .into_any_element()
    }

    /// Puts `text` in the composer, to be edited and sent by the user.
    fn fill_chat_composer(&mut self, id: EntityId, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some((input, _)) = self.chats.get(&id).and_then(|state| state.input.as_ref()) else { return };
        let input = input.clone();
        input.update(cx, |input, cx| input.set_text(text.to_string(), cx));
        window.focus(&gpui::Focusable::focus_handle(&input, cx));
    }

    /// Reads the agent's earlier sessions in this folder for the start screen, once, while the
    /// chat is still empty.
    pub(super) fn refresh_chat_recent(&mut self, id: EntityId, cx: &mut Context<Self>) {
        let Some(state) = self.chats.get_mut(&id) else { return };
        if !state.plain || state.recent.is_some() || state.recent_loading || !state.shown.is_empty() {
            return;
        }
        state.recent_loading = true;
        let view = state.lead.read(cx);
        let (agent, folder, current) = (chat_lead_agent(&state.lead, cx), view.display_cwd(), view.current_session());
        let task = cx.background_spawn(async move {
            let folder = folder.to_string_lossy().into_owned();
            agentty_bridge::list(Some(agent), RECENT_SCANNED)
                .into_iter()
                .filter(|s| s.cwd.as_deref() == Some(folder.as_str()) && Some(&s.id) != current.as_ref())
                .take(RECENT_SHOWN)
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |this, cx| {
            let sessions = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(state) = this.chats.get_mut(&id) {
                    state.recent = Some(sessions);
                    state.recent_loading = false;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The agent's approval or question, its options as buttons.
    fn render_chat_question(
        &self,
        id: EntityId,
        (choices, current): (Vec<String>, usize),
        question: Option<String>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let options = choices.into_iter().enumerate().map(|(index, label)| {
            div()
                .id(SharedString::from(format!("chat-choice-{index}")))
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .py_1p5()
                .rounded_md()
                .cursor_pointer()
                .t_body()
                .text_color(hex(Chrome::BRIGHT))
                .when(index == current, |d| d.bg(hex(Chrome::SELECTED)))
                .hover(|s| s.bg(hex(Chrome::HOVER)))
                .child(
                    div()
                        .flex_shrink_0()
                        .size(px(20.))
                        .rounded_md()
                        .border_1()
                        .border_color(hex(Chrome::BORDER))
                        .flex()
                        .items_center()
                        .justify_center()
                        .t_caption()
                        .text_color(hex(Chrome::MUTED))
                        .child((index + 1).to_string()),
                )
                .child(div().min_w_0().child(label))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.answer_chat_choice(id, index, cx)))
        });
        div()
            .flex_shrink_0()
            .px_4()
            .pb_2()
            .child(column(
                div()
                    .p_3()
                    .rounded_xl()
                    .border_1()
                    .border_color(hex_alpha(Chrome::ATTENTION, 0.5))
                    .bg(hex(Chrome::PANEL))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap_2()
                            .pb_1()
                            .child(icon("message-circle-question", 15., hex(Chrome::ATTENTION)))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .t_body()
                                    .font_weight(crate::theme::EMPHASIS)
                                    .text_color(hex(Chrome::BRIGHT))
                                    .child(question.unwrap_or_else(|| t(cx, "chat.agent_asks").to_string())),
                            ),
                    )
                    .children(options),
            ))
            .into_any_element()
    }

    /// The composer, and under it where the agent works, its model, permission mode and context.
    fn render_chat_composer(&self, state: &ChatState, live: &Live, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (input, _) = state.input.as_ref()?;
        let id = state.lead.entity_id();
        let round = |d: gpui::Stateful<gpui::Div>| {
            d.flex_shrink_0().size(px(30.)).rounded_full().flex().items_center().justify_center().cursor_pointer()
        };
        let attach = round(div().id("chat-attach"))
            .hover(|s| s.bg(hex(Chrome::HOVER)))
            .tooltip(crate::ui::Tooltip::text(t(cx, "chat.attach"), None))
            .child(icon("plus", 17., hex(Chrome::FOREGROUND)))
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.attach_to_chat(id, window, cx)));
        let mic = super::super::voice_input_available().then(|| {
            round(div().id("chat-mic"))
                .hover(|s| s.bg(hex(Chrome::HOVER)))
                .tooltip(crate::ui::Tooltip::text(t(cx, "shortcuts.voice"), None))
                .child(icon("mic", 16., hex(Chrome::FOREGROUND)))
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    if let Some(pane) = this.chats.get(&id).map(|state| state.lead.clone()) {
                        this.focus_pane(&pane, window, cx);
                    }
                    this.toggle_mic(window, cx);
                }))
        });
        let action = if live.working {
            round(div().id("chat-stop"))
                .hover(|s| s.bg(hex(Chrome::HOVER)))
                .tooltip(crate::ui::Tooltip::text(t(cx, "chat.stop"), Some("Esc")))
                .child(div().size(px(12.)).rounded_sm().bg(hex(Chrome::BRIGHT)))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.interrupt_chat(id, cx)))
        } else {
            round(div().id("chat-send"))
                .bg(hex(Chrome::BRIGHT))
                .hover(|s| s.opacity(0.85))
                .child(icon("arrow-up", 16., hex(Chrome::EDITOR)))
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.send_chat_message(id, window, cx)))
        };
        let field = div()
            .pl_1p5()
            .pr_1p5()
            .py_1p5()
            .rounded_2xl()
            .border_1()
            .border_color(hex(Chrome::BORDER))
            .bg(hex(Chrome::PANEL))
            .flex()
            .items_end()
            .gap_1()
            .min_h(px(46.))
            .child(attach)
            .child(div().flex_1().min_w_0().pt(px(5.)).pb(px(5.)).px_1().t_body().text_color(hex(Chrome::BRIGHT)).child(input.clone()))
            .children(mic)
            .child(action);
        let chip = |key: &'static str| {
            div()
                .id(key)
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap_1()
                .px_1p5()
                .py_0p5()
                .rounded_md()
                .cursor_pointer()
                .hover(|s| s.bg(hex(Chrome::HOVER)).text_color(hex(Chrome::BRIGHT)))
        };
        let folder = folder_name(&live.folder);
        let footer = div()
            .flex()
            .items_center()
            .gap_1()
            .px_1()
            .t_small()
            .text_color(hex(Chrome::MUTED))
            .child(chip("chat-footer-computer").child(icon("laptop", 13., hex(Chrome::MUTED))).child(t(cx, "chat.this_computer")).on_click(
                cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if let Some(state) = this.chats.get_mut(&id) {
                        state.side_open = true;
                        cx.notify();
                    }
                }),
            ))
            .child(
                chip("chat-footer-folder")
                    .min_w_0()
                    .child(icon("folder", 13., hex(Chrome::MUTED)))
                    .child(div().truncate().max_w(px(180.)).child(folder))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.open_files_panel(None, cx))),
            )
            .child(div().flex_1())
            .children(live.model.clone().map(|model| {
                chip("chat-footer-model")
                    .text_color(hex(Chrome::FOREGROUND))
                    .child(model)
                    .children(live.effort.map(|key| div().text_color(hex(Chrome::MUTED)).child(t(cx, key))))
                    .child(icon("chevron-down", 12., hex(Chrome::MUTED)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        if let Some(state) = this.chats.get_mut(&id) {
                            state.model_menu = true;
                            cx.notify();
                        }
                    }))
            }))
            .children(live.mode.map(|mode| {
                chip("chat-footer-mode")
                    .text_color(hex(if mode == Mode::Ask { Chrome::FOREGROUND } else { Chrome::ORANGE }))
                    .tooltip(crate::ui::Tooltip::text(t(cx, "chat.mode_hint"), Some("⇧Tab")))
                    .child(t(cx, mode.label()))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.cycle_chat_mode(id, cx)))
            }))
            .children(live.context.map(|(percent, _, _)| {
                div().flex_shrink_0().flex().items_center().gap_1p5().px_1p5().child(meter(percent, 28.)).child(format!("{percent:.0}%"))
            }));
        Some(
            div()
                .flex_shrink_0()
                .px_4()
                .pt_1()
                .pb_3()
                .child(column(div().flex().flex_col().gap_1p5().child(field).child(footer)))
                .into_any_element(),
        )
    }

    /// The models the agent can switch to, above the composer's model chip.
    fn render_model_menu(&self, id: EntityId, live: &Live, cx: &mut Context<Self>) -> AnyElement {
        let options: Vec<(&'static str, String)> = match live.agent {
            agentty_bridge::model::Agent::Claude => vec![
                ("default", t(cx, "chat.model_default").to_string()),
                ("opus", "Opus".into()),
                ("sonnet", "Sonnet".into()),
                ("haiku", "Haiku".into()),
            ],
            // Codex chooses in a picker of its own, in the terminal.
            _ => vec![("", t(cx, "chat.model_pick_in_terminal").to_string())],
        };
        let rows = options.into_iter().enumerate().map(|(index, (model, label))| {
            div()
                .id(SharedString::from(format!("chat-model-{index}")))
                .px_3()
                .py_1p5()
                .rounded_md()
                .cursor_pointer()
                .t_body()
                .text_color(hex(Chrome::BRIGHT))
                .hover(|s| s.bg(hex(Chrome::HOVER)))
                .child(label)
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.choose_chat_model(id, model, window, cx)))
        });
        div()
            .absolute()
            .bottom(px(46.))
            .right(px(24.))
            .w(px(220.))
            .p_1()
            .rounded_lg()
            .border_1()
            .border_color(hex(Chrome::BORDER))
            .bg(hex(Chrome::PANEL))
            .shadow_lg()
            .flex()
            .flex_col()
            .on_mouse_down_out(cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                if let Some(state) = this.chats.get_mut(&id) {
                    state.model_menu = false;
                }
                cx.notify();
            }))
            .children(live.model.clone().map(|model| div().px_3().py_1().t_caption().text_color(hex(Chrome::MUTED)).child(model)))
            .children(rows)
            .into_any_element()
    }

    /// The panel beside the conversation: where the agent works, its context, plan usage and
    /// connectors.
    fn render_chat_panel(&self, state: &ChatState, live: &Live, cx: &mut Context<Self>) -> AnyElement {
        let card = || div().p_3().rounded_xl().border_1().border_color(hex(Chrome::BORDER)).flex().flex_col().gap_2();
        let heading = |text: String| div().t_small().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(text);
        let row = |glyph: &'static str, text: String| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .min_w_0()
                .t_small()
                .text_color(hex(Chrome::FOREGROUND))
                .child(icon(glyph, 14., hex(Chrome::MUTED)))
                .child(div().min_w_0().truncate().child(text))
        };
        let remote = crate::settings::settings(cx).remote.clone();
        let environment = card()
            .child(heading(t(cx, "chat.panel_environment").to_string()))
            .child(row("laptop", t(cx, "chat.this_computer").to_string()))
            .child(
                div()
                    .id("chat-panel-folder")
                    .cursor_pointer()
                    .rounded_md()
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .tooltip(crate::ui::Tooltip::text(crate::ui::tilde(&live.folder), None))
                    .child(row("folder", folder_name(&live.folder)))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.open_files_panel(None, cx))),
            )
            .children(live.branch.clone().map(|branch| row(if live.worktree { "git-fork" } else { "git-branch" }, branch)))
            .when(remote.feature, |d| {
                d.child(
                    div()
                        .id("chat-panel-remote")
                        .flex()
                        .items_center()
                        .justify_between()
                        .cursor_pointer()
                        .rounded_md()
                        .hover(|s| s.bg(hex(Chrome::HOVER)))
                        .child(row("smartphone", t(cx, "chat.remote_access").to_string()))
                        .child(switch(remote.enabled))
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.open_page(Page::Remote, cx))),
                )
            });
        let context = live.context.map(|(percent, used, window)| {
            card()
                .child(
                    div().flex().items_center().justify_between().child(heading(t(cx, "chat.panel_context").to_string())).child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .t_small()
                            .text_color(hex(Chrome::MUTED))
                            .child(format!("{percent:.0}%"))
                            .child(meter(percent, 40.)),
                    ),
                )
                .child(div().t_caption().text_color(hex(Chrome::MUTED)).child(tf(
                    cx,
                    "chat.tokens",
                    &[("used", &agentty_bridge::short_tokens(used)), ("window", &agentty_bridge::short_tokens(window))],
                )))
        });
        let usage = (!state.usage.is_empty()).then(|| {
            card().child(heading(t(cx, "chat.panel_usage").to_string())).children(state.usage.iter().map(|(_, limit)| {
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .t_small()
                    .text_color(hex(Chrome::FOREGROUND))
                    .child(div().truncate().child(t(cx, limit.label)))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_color(hex(Chrome::MUTED))
                            .child(format!("{:.0}%", limit.used_percent))
                            .child(meter(limit.used_percent, 40.)),
                    )
            }))
        });
        let servers = state.mcp.as_ref().and_then(|(_, servers)| servers.as_ref());
        let summary = match servers {
            None => t(cx, "chat.checking").to_string(),
            Some(Err(_)) => "—".to_string(),
            Some(Ok(list)) if list.is_empty() => t(cx, "chat.connectors_none").to_string(),
            Some(Ok(list)) => {
                let broken = list.iter().filter(|s| matches!(s.health, McpHealth::Failed | McpHealth::NeedsAuth)).count();
                if broken > 0 {
                    tf(cx, "chat.connectors_reconnect", &[("n", &broken.to_string())])
                } else {
                    list.len().to_string()
                }
            }
        };
        let open = state.open.borrow().contains("panel:connectors");
        let id = state.lead.entity_id();
        let connectors = card()
            .child(
                div()
                    .id("chat-panel-connectors")
                    .flex()
                    .items_center()
                    .justify_between()
                    .cursor_pointer()
                    .child(div().flex().items_center().gap_1().child(heading(t(cx, "chat.panel_connectors").to_string())).child(icon(
                        if open { "chevron-down" } else { "chevron-right" },
                        12.,
                        hex(Chrome::MUTED),
                    )))
                    .child(div().t_small().text_color(hex(Chrome::MUTED)).child(summary))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        if let Some(state) = this.chats.get(&id) {
                            let mut set = state.open.borrow_mut();
                            if !set.remove("panel:connectors") {
                                set.insert("panel:connectors".into());
                            }
                        }
                        cx.notify();
                    })),
            )
            .when(open, |d| d.children(servers.and_then(|s| s.as_ref().ok()).into_iter().flatten().map(server_row)));
        div()
            .id("chat-panel")
            .flex_shrink_0()
            .w(px(SIDE_PANEL_WIDTH))
            .h_full()
            .overflow_y_scroll()
            .p_3()
            .pl_0()
            .flex()
            .flex_col()
            .gap_3()
            .child(environment)
            .children(context)
            .children(usage)
            .child(connectors)
            .into_any_element()
    }

    /// Reads the agent's MCP servers for the panel, in the background, at most every [`MCP_EVERY`].
    pub(super) fn refresh_chat_mcp(&mut self, id: EntityId, cx: &mut Context<Self>) {
        let Some(state) = self.chats.get_mut(&id) else { return };
        if !state.plain || !state.side_open || state.mcp.as_ref().is_some_and(|(at, _)| at.elapsed() < MCP_EVERY) {
            return;
        }
        let (agent, cwd) = (chat_lead_agent(&state.lead, cx), state.lead.read(cx).display_cwd());
        state.mcp = Some((Instant::now(), None));
        let task = cx.background_spawn(async move { crate::workbench::status_menus::mcp_list(agent, cwd) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(state) = this.chats.get_mut(&id) {
                    state.mcp = Some((Instant::now(), Some(result)));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Over the terminal of a pane chat view shows as a chat: the way back to the chat, or why the
    /// terminal is up (the agent asks something only its screen can answer).
    pub(in crate::workbench) fn render_plain_chat_return(&self, pane: &Pane, cx: &mut Context<Self>) -> Option<AnyElement> {
        let state = self.chats.get(&pane.entity_id()).filter(|state| state.plain)?;
        let id = pane.entity_id();
        let pill = div().absolute().top_1().right_3().px_2().py_1().rounded_md().flex().items_center().gap_1().t_caption();
        if Self::terminal_forced(state, cx) {
            let reason = t(cx, if state.setup_screen { "chat.setup_screen_plain" } else { "chat.answer_in_terminal" });
            return Some(
                pill.bg(hex_alpha(Chrome::ATTENTION, 0.9))
                    .text_color(hex(Chrome::BRIGHT))
                    .child(icon("message-circle-question", 12., hex(Chrome::BRIGHT)))
                    .child(reason)
                    .into_any_element(),
            );
        }
        Some(
            pill.id("chat-back")
                .cursor_pointer()
                .border_1()
                .border_color(hex(Chrome::BORDER))
                .bg(hex(Chrome::PANEL))
                .text_color(hex(Chrome::FOREGROUND))
                .hover(|s| s.bg(hex(Chrome::HOVER)))
                .child(icon("message-square", 12., hex(Chrome::FOREGROUND)))
                .child(t(cx, "chat.show_chat"))
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.toggle_chat_terminal(id, window, cx)))
                .into_any_element(),
        )
    }

    /// Picks option `target` of the selection on the agent's screen, as read now.
    fn answer_chat_choice(&mut self, id: EntityId, target: usize, cx: &mut Context<Self>) {
        let Some(state) = self.chats.get(&id) else { return };
        let lead = state.lead.clone();
        let Some((choices, current)) = screen_choices(&lead.read(cx).screen_lines(40)) else { return };
        if target >= choices.len() {
            return;
        }
        lead.update(cx, |view, _| view.write(choice_keys(current, target)));
        cx.notify();
    }

    /// Stops the agent's turn, as Esc in its terminal does.
    fn interrupt_chat(&mut self, id: EntityId, cx: &mut Context<Self>) {
        let Some(state) = self.chats.get(&id) else { return };
        state.lead.update(cx, |view, _| view.write(b"\x1b".to_vec()));
    }

    /// The next permission mode, as Shift+Tab in Claude Code.
    fn cycle_chat_mode(&mut self, id: EntityId, cx: &mut Context<Self>) {
        let Some(state) = self.chats.get(&id) else { return };
        state.lead.update(cx, |view, _| view.write(b"\x1b[Z".to_vec()));
        cx.notify();
    }

    /// Switches the agent's model (`/model <name>`); Codex picks in its own terminal picker.
    fn choose_chat_model(&mut self, id: EntityId, model: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.chats.get_mut(&id) else { return };
        state.model_menu = false;
        let lead = state.lead.clone();
        if model.is_empty() {
            state.show_terminal = true;
            lead.update(cx, |view, cx| view.submit_prompt("/model".into(), cx));
            self.focus_pane(&lead, window, cx);
        } else {
            lead.update(cx, |view, cx| view.submit_prompt(format!("/model {model}"), cx));
        }
        cx.notify();
    }

    /// Files or folders chosen with the composer's `+`: their paths go into the message.
    fn attach_to_chat(&mut self, id: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions { files: true, directories: true, multiple: true, prompt: None });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let _ = this.update_in(cx, |this, window, cx| {
                let Some((input, _)) = this.chats.get(&id).and_then(|state| state.input.as_ref()) else { return };
                let input = input.clone();
                let quoted: Vec<String> = paths.iter().map(|p| crate::launch::shell_quote(&p.display().to_string())).collect();
                input.update(cx, |input, cx| {
                    let mut text = input.text().to_string();
                    if !text.is_empty() && !text.ends_with(' ') {
                        text.push(' ');
                    }
                    text.push_str(&quoted.join(" "));
                    text.push(' ');
                    input.set_text(text, cx);
                });
                window.focus(&gpui::Focusable::focus_handle(&input, cx));
            });
        })
        .detach();
    }
}

/// The name a folder goes by: its last component.
fn folder_name(path: &std::path::Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| crate::ui::tilde(path))
}

/// A short bar filled to `percent`.
fn meter(percent: f64, width: f32) -> gpui::Div {
    let color = match percent {
        p if p >= 90. => Chrome::ERROR,
        p if p >= 70. => Chrome::ORANGE,
        _ => Chrome::ATTENTION,
    };
    let fill = (percent.clamp(0., 100.) / 100.) as f32 * width;
    div()
        .w(px(width))
        .h(px(5.))
        .rounded_full()
        .bg(hex(Chrome::SELECTED))
        .child(div().h_full().w(px(fill.max(3.))).rounded_full().bg(hex(color)))
}

/// An on/off switch, as it reads (the setting itself is changed where it lives).
fn switch(on: bool) -> gpui::Div {
    div()
        .flex_shrink_0()
        .w(px(30.))
        .h(px(17.))
        .p(px(2.))
        .rounded_full()
        .flex()
        .when(on, |d| d.justify_end())
        .bg(hex(if on { Chrome::ACCENT } else { Chrome::SELECTED }))
        .child(div().size(px(13.)).rounded_full().bg(hex(Chrome::BRIGHT)))
}

fn server_row(server: &McpStatus) -> gpui::Div {
    let color = match server.health {
        McpHealth::Connected | McpHealth::Enabled => Chrome::SUCCESS,
        McpHealth::Failed => Chrome::ERROR,
        McpHealth::NeedsAuth => Chrome::WARNING,
        McpHealth::Disabled | McpHealth::Unknown => Chrome::MUTED,
    };
    div()
        .flex()
        .items_center()
        .gap_2()
        .t_small()
        .text_color(hex(Chrome::FOREGROUND))
        .child(div().flex_shrink_0().size(px(7.)).rounded_full().bg(hex(color)))
        .child(div().truncate().child(server.name.clone()))
}

/// A reply: its words, its to-do list, and its tool steps folded into cards.
pub(super) fn render_reply(index: usize, blocks: &Arc<Vec<Block>>, rows: &Rows, cx: &App) -> AnyElement {
    let group = SharedString::from(format!("chat-reply-{index}"));
    let mut out = div().w_full().flex().flex_col().gap_2p5();
    let mut steps: Vec<&ToolStep> = Vec::new();
    for block in blocks.iter() {
        if let Block::Tool(step) = block {
            steps.push(step);
            continue;
        }
        if !steps.is_empty() {
            out = out.child(steps_card(index, &std::mem::take(&mut steps), rows, cx));
        }
        out = match block {
            Block::Text(text) => out.child(markdown(text, index)),
            Block::Todos(todos) => out.child(todo_card(todos)),
            Block::Tool(_) => out,
        };
    }
    if !steps.is_empty() {
        out = out.child(steps_card(index, &steps, rows, cx));
    }
    let copied = reply_text(blocks);
    out.group(group.clone())
        .when(!copied.trim().is_empty(), |d| {
            d.child(
                div()
                    .id(SharedString::from(format!("chat-copy-{index}")))
                    .size(px(24.))
                    .rounded_md()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .invisible()
                    .group_hover(group, |s| s.visible())
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .child(icon("copy", 13., hex(Chrome::MUTED)))
                    .on_click(move |_, _, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string(copied.trim().to_string()))),
            )
        })
        .into_any_element()
}

/// What a step is called in its card: what the agent said it is for, else the tool and its target.
fn step_label(step: &ToolStep, cx: &App) -> String {
    if let Some(description) = &step.description {
        return description.clone();
    }
    tool_text(&step.name, step.detail.as_deref(), cx)
}

/// A tool and what it ran on, as a chat app says it: "Created server.js", "Ran npm test".
fn tool_text(tool: &str, target: Option<&str>, cx: &App) -> String {
    let target = target.map(|t| short_target(t.lines().next().unwrap_or_default()));
    let verb = match tool {
        "Write" | "NotebookWrite" => Some("chat.tool_write"),
        "Edit" | "MultiEdit" | "NotebookEdit" | "apply_patch" => Some("chat.tool_edit"),
        "Read" | "view_image" => Some("chat.tool_read"),
        "Grep" | "Glob" | "LS" | "ToolSearch" => Some("chat.tool_search"),
        "Bash" | "shell" | "exec_command" | "local_shell" => Some("chat.tool_run"),
        "WebFetch" | "WebSearch" | "web_search" => Some("chat.tool_fetch"),
        _ => None,
    };
    match (verb, target) {
        (Some(verb), target) => tf(cx, verb, &[("target", target.as_deref().unwrap_or_default())]).trim().to_string(),
        (None, target) => crate::terminal::tool_label(tool, target.as_deref()),
    }
}

/// A file as its name, a command or anything else as its first 90 characters.
fn short_target(target: &str) -> String {
    let target = target.trim();
    if target.starts_with('/') && !target.contains(' ') {
        if let Some(name) = std::path::Path::new(target).file_name() {
            return name.to_string_lossy().into_owned();
        }
    }
    target.chars().take(90).collect()
}

/// A run of tool steps: each one a row that opens to what it ran and what came back. A long run
/// shows its last steps, the earlier ones behind a row of their own.
fn steps_card(index: usize, steps: &[&ToolStep], rows: &Rows, cx: &App) -> AnyElement {
    let group_key = format!("group:{}", steps.first().map_or("", |s| s.id.as_str()));
    let hidden = if rows.is_open(&group_key) { 0 } else { steps.len().saturating_sub(STEPS_SHOWN) };
    let mut card = div().w_full().rounded_xl().border_1().border_color(hex(Chrome::BORDER)).flex().flex_col().overflow_hidden();
    if hidden > 0 {
        let toggle = rows.clone();
        let key = group_key.clone();
        card = card.child(
            div()
                .id(SharedString::from(format!("chat-steps-more-{index}-{group_key}")))
                .px_3()
                .py_2()
                .cursor_pointer()
                .t_small()
                .text_color(hex(Chrome::MUTED))
                .hover(|s| s.bg(hex(Chrome::HOVER)))
                .child(tf(cx, "chat.steps_more", &[("n", &hidden.to_string())]))
                .on_click(move |_, _, cx| toggle.toggle(index, &key, cx)),
        );
    }
    for (n, step) in steps.iter().enumerate().skip(hidden) {
        let open = rows.is_open(&step.id);
        let (glyph, color) = match (&step.output, step.failed) {
            (_, true) => ("circle-x", Chrome::ERROR),
            (Some(_), false) => ("circle-check", Chrome::MUTED),
            (None, false) => ("loader-circle", Chrome::ORANGE),
        };
        let toggle = rows.clone();
        let key = step.id.clone();
        let header = div()
            .id(SharedString::from(format!("chat-step-{index}-{n}-{}", step.id)))
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .cursor_pointer()
            .hover(|s| s.bg(hex(Chrome::HOVER)))
            .child(icon(glyph, 14., hex(color)))
            .child(div().flex_1().min_w_0().truncate().t_small().text_color(hex(Chrome::FOREGROUND)).child(step_label(step, cx)))
            .child(icon(if open { "chevron-down" } else { "chevron-right" }, 12., hex(Chrome::MUTED)))
            .on_click(move |_, _, cx| toggle.toggle(index, &key, cx));
        let body = open.then(|| {
            div()
                .px_3()
                .pb_3()
                .flex()
                .flex_col()
                .gap_2()
                // A file edit shows what changed; its file is already in the label.
                .children(step.detail.as_ref().filter(|_| step.diff.is_none()).map(|detail| {
                    let shown = if step.name == "Bash" || step.name.contains("shell") || step.name.contains("exec") {
                        format!("$ {detail}")
                    } else {
                        detail.clone()
                    };
                    code_box(&shown, None)
                }))
                .children(step.diff.as_ref().map(|diff| diff_box(diff)))
                .children(step.output.as_ref().filter(|o| !o.trim().is_empty()).map(|output| {
                    div()
                        .px_1()
                        .font_family("JetBrains Mono")
                        .t_small()
                        .text_color(hex(if step.failed { Chrome::ERROR } else { Chrome::MUTED }))
                        .flex()
                        .flex_col()
                        .children(output.lines().map(|line| div().child(if line.is_empty() { " ".to_string() } else { line.to_string() })))
                }))
        });
        card = card.child(div().when(n > hidden, |d| d.border_t_1().border_color(hex(Chrome::BORDER))).child(header).children(body));
    }
    card.into_any_element()
}

/// A file edit's lines, taken out in red and put in in green.
fn diff_box(diff: &str) -> gpui::Div {
    div()
        .w_full()
        .rounded_lg()
        .border_1()
        .border_color(hex(Chrome::BORDER))
        .bg(hex(Chrome::PANEL))
        .py_1()
        .font_family("JetBrains Mono")
        .t_small()
        .flex()
        .flex_col()
        .children(diff.lines().map(|line| {
            let (color, background) = match line.chars().next() {
                Some('-') => (Chrome::ERROR, Some(hex_alpha(Chrome::ERROR, 0.1))),
                Some('+') => (Chrome::SUCCESS, Some(hex_alpha(Chrome::SUCCESS, 0.1))),
                _ => (Chrome::MUTED, None),
            };
            div().px_3().text_color(hex(color)).when_some(background, |d, bg| d.bg(bg)).child(if line.is_empty() {
                " ".to_string()
            } else {
                line.to_string()
            })
        }))
}

/// The agent's to-do list, each item with where it stands.
fn todo_card(todos: &[Todo]) -> AnyElement {
    div()
        .w_full()
        .px_3()
        .py_2p5()
        .rounded_xl()
        .border_1()
        .border_color(hex(Chrome::BORDER))
        .flex()
        .flex_col()
        .gap_1p5()
        .children(todos.iter().map(|todo| {
            let (glyph, color, text) = match todo.status {
                TodoStatus::Completed => ("circle-check", Chrome::MUTED, Chrome::MUTED),
                TodoStatus::InProgress => ("loader-circle", Chrome::ORANGE, Chrome::BRIGHT),
                TodoStatus::Pending => ("circle-dot", Chrome::MUTED, Chrome::FOREGROUND),
            };
            let label = match (todo.status, &todo.active) {
                (TodoStatus::InProgress, Some(active)) => active.clone(),
                _ => todo.text.clone(),
            };
            div().flex().items_start().gap_2().child(div().pt(px(2.)).child(icon(glyph, 14., hex(color)))).child(
                div()
                    .min_w_0()
                    .t_body()
                    .text_color(hex(text))
                    .when(todo.status == TodoStatus::Completed, |d| d.line_through())
                    .child(label),
            )
        }))
        .into_any_element()
}

/// A block of code (or a command), with its language and a copy button.
fn code_box(code: &str, language: Option<&str>) -> gpui::Div {
    let copied = code.to_string();
    let id = SharedString::from(format!("chat-code-{:x}", fxhash(code)));
    div()
        .w_full()
        .rounded_lg()
        .border_1()
        .border_color(hex(Chrome::BORDER))
        .bg(hex(Chrome::PANEL))
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .pl_3()
                .pr_1()
                .pt_1()
                .t_caption()
                .text_color(hex(Chrome::MUTED))
                .child(language.unwrap_or_default().to_string())
                .child(
                    div()
                        .id(id)
                        .size(px(22.))
                        .rounded_md()
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|s| s.bg(hex(Chrome::HOVER)))
                        .child(icon("copy", 12., hex(Chrome::MUTED)))
                        .on_click(move |_, _, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string(copied.clone()))),
                ),
        )
        .child(
            div()
                .px_3()
                .pb_2()
                .font_family("JetBrains Mono")
                .t_small()
                .text_color(hex(Chrome::FOREGROUND))
                .flex()
                .flex_col()
                .children(code.lines().take(400).map(|line| div().child(if line.is_empty() { " ".to_string() } else { line.to_string() }))),
        )
}

fn fxhash(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// Markdown as a chat shows it: headings, lists, quotes, code blocks, and `code` and **bold** in
/// running text.
fn markdown(text: &str, index: usize) -> AnyElement {
    let mut out = div().w_full().flex().flex_col().gap_1p5().t_body().text_color(hex(Chrome::FOREGROUND));
    let mut lines = text.lines().peekable();
    let mut paragraph: Vec<String> = Vec::new();
    let flush = |out: gpui::Div, paragraph: &mut Vec<String>| {
        if paragraph.is_empty() {
            return out;
        }
        let text = paragraph.join("\n");
        paragraph.clear();
        out.child(inline(&text))
    };
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        if let Some(fence) = trimmed.strip_prefix("```") {
            out = flush(out, &mut paragraph);
            let language = fence.trim();
            let mut code = Vec::new();
            for inner in lines.by_ref() {
                if inner.trim_start().starts_with("```") {
                    break;
                }
                code.push(inner);
            }
            out = out.child(code_box(&code.join("\n"), (!language.is_empty()).then_some(language)));
        } else if trimmed.is_empty() {
            out = flush(out, &mut paragraph);
        } else if let Some(heading) = trimmed.strip_prefix('#') {
            out = flush(out, &mut paragraph);
            let level = 1 + heading.chars().take_while(|c| *c == '#').count();
            let heading = heading.trim_start_matches('#').trim();
            out = out.child(
                div()
                    .pt_1()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(hex(Chrome::BRIGHT))
                    .when(level <= 2, |d| d.t_title())
                    .child(inline(heading)),
            );
        } else if let Some(item) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
            out = flush(out, &mut paragraph);
            let depth = (line.len() - trimmed.len()) / 2;
            // A task list item (`- [ ]`, `- [x]`) reads as the to-do card's rows do.
            let task = item
                .strip_prefix("[ ] ")
                .map(|rest| (false, rest))
                .or_else(|| item.strip_prefix("[x] ").or_else(|| item.strip_prefix("[X] ")).map(|rest| (true, rest)));
            if let Some((done, rest)) = task {
                out = out.child(
                    div()
                        .pl(px(4. + depth as f32 * 16.))
                        .flex()
                        .items_start()
                        .gap_2()
                        .child(div().pt(px(2.)).child(icon(if done { "circle-check" } else { "circle-dot" }, 14., hex(Chrome::MUTED))))
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .text_color(hex(if done { Chrome::MUTED } else { Chrome::FOREGROUND }))
                                .when(done, |d| d.line_through())
                                .child(inline(rest)),
                        ),
                );
                continue;
            }
            out = out.child(
                div()
                    .pl(px(4. + depth as f32 * 16.))
                    .flex()
                    .gap_2()
                    .child(div().flex_shrink_0().text_color(hex(Chrome::MUTED)).child("•"))
                    .child(div().min_w_0().flex_1().child(inline(item))),
            );
        } else if let Some((number, item)) = numbered(trimmed) {
            out = flush(out, &mut paragraph);
            out = out.child(
                div()
                    .pl_1()
                    .flex()
                    .gap_2()
                    .child(div().flex_shrink_0().text_color(hex(Chrome::MUTED)).child(format!("{number}.")))
                    .child(div().min_w_0().flex_1().child(inline(item))),
            );
        } else if let Some(quote) = trimmed.strip_prefix('>') {
            out = flush(out, &mut paragraph);
            out = out.child(
                div().pl_3().border_l_2().border_color(hex(Chrome::BORDER)).text_color(hex(Chrome::MUTED)).child(inline(quote.trim())),
            );
        } else if trimmed.starts_with('|') {
            // Tables keep their columns in a fixed-width font.
            out = flush(out, &mut paragraph);
            if !trimmed.trim_matches(|c| c == '|' || c == '-' || c == ':' || c == ' ').is_empty() {
                out = out.child(div().font_family("JetBrains Mono").t_small().child(trimmed.to_string()));
            }
        } else if trimmed.chars().all(|c| c == '-' || c == '*' || c == '_') && trimmed.len() >= 3 {
            out = flush(out, &mut paragraph);
            out = out.child(div().h(px(1.)).my_1().bg(hex(Chrome::BORDER)));
        } else {
            paragraph.push(line.to_string());
        }
    }
    out = flush(out, &mut paragraph);
    let _ = index;
    out.into_any_element()
}

fn numbered(line: &str) -> Option<(&str, &str)> {
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    let rest = line[digits..].strip_prefix(". ")?;
    (digits > 0 && digits <= 3).then(|| (&line[..digits], rest))
}

/// Running text with `code` and **bold** set apart.
fn inline(text: &str) -> StyledText {
    let (plain, spans) = inline_spans(text);
    let highlights = spans.into_iter().map(|(range, code)| {
        let style = if code {
            HighlightStyle { color: Some(hex(Chrome::ORANGE)), background_color: Some(hex(Chrome::PANEL)), ..Default::default() }
        } else {
            HighlightStyle { font_weight: Some(gpui::FontWeight::SEMIBOLD), color: Some(hex(Chrome::BRIGHT)), ..Default::default() }
        };
        (range, style)
    });
    StyledText::new(plain).with_highlights(highlights.collect::<Vec<_>>())
}

/// `text` without its `` ` `` and `**` marks, and the ranges they marked (true: code).
fn inline_spans(text: &str) -> (String, Vec<(std::ops::Range<usize>, bool)>) {
    let mut out = String::with_capacity(text.len());
    let mut spans = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let tick = rest.find('`');
        let bold = rest.find("**");
        match (tick, bold) {
            (Some(at), b) if b.is_none_or(|b| at < b) => {
                if let Some(end) = rest[at + 1..].find('`') {
                    out.push_str(&rest[..at]);
                    let start = out.len();
                    out.push_str(&rest[at + 1..at + 1 + end]);
                    spans.push((start..out.len(), true));
                    rest = &rest[at + 2 + end..];
                } else {
                    out.push_str(rest);
                    rest = "";
                }
            }
            (_, Some(at)) => {
                if let Some(end) = rest[at + 2..].find("**") {
                    out.push_str(&rest[..at]);
                    let start = out.len();
                    out.push_str(&rest[at + 2..at + 2 + end]);
                    spans.push((start..out.len(), false));
                    rest = &rest[at + 4 + end..];
                } else {
                    out.push_str(rest);
                    rest = "";
                }
            }
            _ => {
                out.push_str(rest);
                rest = "";
            }
        }
    }
    spans.retain(|(range, _)| !range.is_empty());
    (out, spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_marks_become_spans() {
        let (text, spans) = inline_spans("run `cargo test` then **ship** it");
        assert_eq!(text, "run cargo test then ship it");
        assert_eq!(spans, vec![(4..14, true), (20..24, false)]);
        let (text, spans) = inline_spans("a ` lone tick and ** lone stars");
        assert_eq!(text, "a ` lone tick and ** lone stars");
        assert!(spans.is_empty());
        assert_eq!(numbered("12. twelve"), Some(("12", "twelve")));
        assert_eq!(numbered("2024. not a list"), None);
    }

    #[test]
    fn files_go_by_their_name() {
        assert_eq!(short_target("/private/tmp/demo/server.js"), "server.js");
        assert_eq!(short_target("node server.js & sleep 1"), "node server.js & sleep 1");
    }

    #[test]
    fn the_mode_is_read_off_the_footer() {
        let lines = |l: &[&str]| l.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(Mode::of(&lines(&["> ", "  ⏵⏵ accept edits on (shift+tab to cycle)"])), Mode::Edits);
        assert_eq!(Mode::of(&lines(&["> ", "  ⏸ plan mode on (shift+tab to cycle)"])), Mode::Plan);
        assert_eq!(Mode::of(&lines(&["> ", "  ? for shortcuts"])), Mode::Ask);
    }
}
