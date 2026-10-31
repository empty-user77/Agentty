//! Chat view's look: a pane's session as a chat app shows it, under the same bar as its terminal.
//! The conversation in a readable column (replies with their steps folded into cards, the agent's
//! to-do list, its questions as buttons), and the composer with what it runs on under it.

use super::*;
use crate::workbench::{Page, SidePanel};
use agentty_bridge::chat_log::{Block, Todo, TodoStatus, ToolStep};
use gpui::{HighlightStyle, MouseButton, MouseDownEvent, PathPromptOptions};

/// Steps of a card shown before the earlier ones fold away.
const STEPS_SHOWN: usize = 4;
/// Sessions read for the start screen, and how many of those in this folder it shows.
const RECENT_SCANNED: usize = 200;
const RECENT_SHOWN: usize = 4;

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
    working: bool,
    /// The last turn was stopped before it ended.
    interrupted: bool,
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
        self.chats.get(&pane.entity_id()).is_some_and(|state| state.plain && state.first_load.is_none() && self.chat_in_front(state, cx))
    }

    fn live(&self, state: &ChatState, cx: &App) -> Live {
        let pane = &state.lead;
        let view = pane.read(cx);
        let agent = chat_lead_agent(pane, cx);
        let lines = view.screen_lines(40);
        let waits = waits_on_user(view);
        Live {
            tool: view.tool_id(),
            agent,
            working: view.status.in_turn(),
            interrupted: matches!(view.status, AgentStatus::Interrupted),
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
        let state = self.chats.get(&pane.entity_id()).filter(|state| state.plain && state.first_load.is_none())?;
        if !self.chat_in_front(state, cx) {
            return None;
        }
        let id = pane.entity_id();
        let live = self.live(state, cx);

        let messages: AnyElement =
            if state.shown.is_empty() { self.render_chat_welcome(state, &live, cx) } else { message_list(state, cx) };
        let approval = live.choices.clone().map(|choices| self.render_chat_question(id, choices, live.question.clone(), cx));
        let interrupted = (!live.working && live.interrupted).then(|| {
            div().flex_shrink_0().px_4().pb_1().child(column(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .t_small()
                    .text_color(hex(Chrome::MUTED))
                    .child(icon("circle-pause", 13., hex(Chrome::MUTED)))
                    .child(t(cx, "chat.interrupted")),
            ))
        });
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
            .children(interrupted)
            .children(activity)
            .children(composer);
        Some(
            div()
                .id(SharedString::from(format!("plain-chat-{}", id.as_u64())))
                .size_full()
                .flex()
                .flex_col()
                .bg(hex(Chrome::EDITOR))
                .relative()
                // A click on the chat reads what it finished, as one on its terminal does. A press on
                // its text starts a selection and takes the keyboard there (⌘C); elsewhere it drops one.
                .key_context("ChatReading")
                .track_focus(&state.reading)
                .on_action(cx.listener(|this, _: &crate::workbench::CopyChatSelection, _, cx| {
                    if let Some(text) = super::selectable::selected_text() {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
                        this.show_toast(t(cx, "chat.copied"), cx);
                    }
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(move |this, event: &gpui::MouseUpEvent, _, cx| this.selection_ended(id, event.position, cx)),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| this.selection_menu(id, event.position, cx)),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                        this.mark_active(&target, cx);
                        target.update(cx, |view, cx| view.acknowledge(cx));
                        this.chat_select = None;
                        if super::selectable::take_press() {
                            if let Some(state) = this.chats.get(&target.entity_id()) {
                                window.focus(&state.reading);
                            }
                        }
                        cx.notify();
                    }),
                )
                // The side panel goes by the pane's width as last laid out: when a panel docked
                // beside it opens or closes, draw again at once instead of showing the old layout
                // squeezed (or stretched) until something else redraws.
                .child({
                    let (widths, drawn_at) = (self.pane_bounds.clone(), live.width);
                    gpui::canvas(
                        |_, _, _| {},
                        move |_, _, window, _| {
                            let now = widths.borrow().get(&id).map(|b| f32::from(b.size.width));
                            if now.is_some_and(|now| (now - drawn_at).abs() > 1.) {
                                window.refresh();
                            }
                        },
                    )
                    .absolute()
                    .size_0()
                })
                .child(div().flex_1().min_h_0().flex().child(main))
                .into_any_element(),
        )
    }

    /// Before the first message, as a chat app's start screen: what runs where (agent, model,
    /// folder, branch), a few ways to begin that fill the composer without sending, and the
    /// agent's earlier sessions in this folder to pick up again.
    fn render_chat_welcome(&self, state: &ChatState, live: &Live, cx: &mut Context<Self>) -> AnyElement {
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
                    .children(recent),
            )
            .child(div().flex_1().min_h(px(16.)))
            .into_any_element()
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
        let row = div()
            .flex()
            .items_end()
            .gap_1()
            .child(attach)
            .child(div().flex_1().min_w_0().pt(px(5.)).pb(px(5.)).px_1().t_body().text_color(hex(Chrome::BRIGHT)).child(input.clone()))
            .children(mic)
            .child(action);
        let field = div()
            .pl_1p5()
            .pr_1p5()
            .py_1p5()
            .rounded_2xl()
            .border_1()
            .border_color(hex(Chrome::BORDER))
            .bg(hex(Chrome::PANEL))
            .flex()
            .flex_col()
            .gap_1()
            .min_h(px(46.))
            .children(self.render_quote(state, cx))
            .children(self.render_attachments(state, cx))
            .child(row);
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
        let footer = div()
            .flex()
            .items_center()
            .gap_1()
            .px_1()
            .t_small()
            .text_color(hex(Chrome::MUTED))
            .child(div().flex_1())
            .children(live.model.clone().map(|model| {
                chip("chat-footer-model")
                    .text_color(hex(Chrome::FOREGROUND))
                    .child(model)
                    .children(live.effort.map(|key| div().text_color(hex(Chrome::MUTED)).child(t(cx, key))))
                    .child(icon("chevron-down", 12., hex(Chrome::MUTED)))
                    .when(state.model_menu, |d| d.bg(hex(Chrome::SELECTED)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        // The click that closed the menu from outside doesn't open it again.
                        if this.just_dismissed("chat-model-menu") {
                            return;
                        }
                        if let Some(state) = this.chats.get_mut(&id) {
                            state.model_menu = !state.model_menu;
                            cx.notify();
                        }
                    }))
                    .relative()
                    .children(state.model_menu.then(|| self.render_model_menu(id, live, cx)))
            }))
            .children(live.mode.map(|mode| {
                chip("chat-footer-mode")
                    .text_color(hex(if mode == Mode::Ask { Chrome::FOREGROUND } else { Chrome::ORANGE }))
                    .tooltip(crate::ui::Tooltip::text(t(cx, "chat.mode_hint"), Some("⇧Tab")))
                    .child(t(cx, mode.label()))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.cycle_chat_mode(id, cx)))
            }))
            .children(live.context.map(|(percent, _, _)| {
                chip("chat-footer-context")
                    .gap_1p5()
                    .child(meter(percent, 28.))
                    .child(format!("{percent:.0}%"))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.toggle_chat_context(id, cx)))
                    .relative()
                    .children(self.render_chat_context(state, cx))
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

    /// Opens or closes the context-memory menu the terminal's bar has, over the composer's meter.
    fn toggle_chat_context(&mut self, id: EntityId, cx: &mut Context<Self>) {
        let Some(state) = self.chats.get(&id) else { return };
        let pane = state.lead.clone();
        self.toggle_pane_menu(super::super::status_menus::StatusMenu::Context, pane, cx);
    }

    /// The context-memory menu over the composer's meter, while it is open for this chat.
    fn render_chat_context(&self, state: &ChatState, cx: &mut Context<Self>) -> Option<AnyElement> {
        let open = self.status_menu == Some(super::super::status_menus::StatusMenu::Context) && self.menu_pane_is(&state.lead);
        if !open {
            return None;
        }
        let agent = chat_lead_agent(&state.lead, cx);
        let menu = self.render_context_menu(&state.lead, agent, cx);
        Some(
            div()
                .absolute()
                .right_0()
                .bottom_full()
                .child(
                    gpui::deferred(
                        gpui::anchored()
                            .anchor(gpui::Corner::BottomRight)
                            .snap_to_window_with_margin(px(8.))
                            .child(div().mb_1().child(menu)),
                    )
                    .with_priority(3),
                )
                .into_any_element(),
        )
    }

    /// The status bar's MCP menu: a connector to reconnect, in Claude Code's own `/mcp` (signing in
    /// may open the browser), in the pane in front's terminal (a chat shows its terminal for it).
    pub(in crate::workbench) fn reconnect_mcp_in_front(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pane) = self.active_pane().filter(|pane| pane.read(cx).agent_kind() == Some(crate::launch::PaneKind::Claude)) else {
            return self.show_toast(t(cx, "status.mcp_reconnect_none"), cx);
        };
        if let Some(state) = self.chats.get_mut(&pane.entity_id()).filter(|state| state.plain) {
            state.show_terminal = true;
        }
        pane.update(cx, |view, cx| view.submit_prompt("/mcp".into(), cx));
        self.focus_pane(&pane, window, cx);
        cx.notify();
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
        let menu = div()
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
                this.note_dismissed("chat-model-menu");
                if let Some(state) = this.chats.get_mut(&id) {
                    state.model_menu = false;
                }
                cx.notify();
            }))
            .children(live.model.clone().map(|model| div().px_3().py_1().t_caption().text_color(hex(Chrome::MUTED)).child(model)))
            .children(rows);
        // Over the chip, from its left edge (the window's edge moves it in if need be).
        div()
            .absolute()
            .left_0()
            .bottom_full()
            .child(
                gpui::deferred(
                    gpui::anchored().anchor(gpui::Corner::BottomLeft).snap_to_window_with_margin(px(8.)).child(div().mb_1().child(menu)),
                )
                .with_priority(3),
            )
            .into_any_element()
    }

    /// Over the terminal of a pane chat view shows as a chat: the way back to the chat, or why the
    /// terminal is up (the agent asks something only its screen can answer).
    pub(in crate::workbench) fn render_plain_chat_return(&self, pane: &Pane, cx: &mut Context<Self>) -> Option<AnyElement> {
        let state = self.chats.get(&pane.entity_id()).filter(|state| state.plain && state.first_load.is_none())?;
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
            let _ = this.update_in(cx, |this, window, cx| this.attach_paths(id, &paths, window, cx));
        })
        .detach();
    }

    /// Attaches `paths` to the message being written (shown over the input, sent with it).
    pub(in crate::workbench) fn attach_paths(&mut self, id: EntityId, paths: &[PathBuf], window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.chats.get_mut(&id) else { return };
        for path in paths {
            if !state.attachments.contains(path) {
                state.attachments.push(path.clone());
            }
        }
        if let Some((input, _)) = state.input.as_ref() {
            window.focus(&gpui::Focusable::focus_handle(input, cx));
        }
        cx.notify();
    }

    /// What the next message replies to, over the input, with a button that drops it.
    fn render_quote(&self, state: &ChatState, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let quote = state.quote.as_ref()?;
        let id = state.lead.entity_id();
        let first = quote.lines().find(|line| !line.trim().is_empty()).unwrap_or_default().to_string();
        Some(
            div()
                .mx_1p5()
                .mt_1()
                .pl_2()
                .border_l_2()
                .border_color(hex(Chrome::BORDER))
                .flex()
                .items_center()
                .gap_2()
                .child(icon("undo-2", 13., hex(Chrome::MUTED)))
                .child(div().flex_1().min_w_0().truncate().t_small().text_color(hex(Chrome::MUTED)).child(first))
                .child(
                    div()
                        .id("chat-quote-remove")
                        .size(px(20.))
                        .rounded_md()
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|s| s.bg(hex(Chrome::HOVER)))
                        .tooltip(crate::ui::Tooltip::text(t(cx, "chat.remove_attachment"), None))
                        .child(icon("x", 12., hex(Chrome::MUTED)))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            if let Some(state) = this.chats.get_mut(&id) {
                                state.quote = None;
                            }
                            cx.notify();
                        })),
                ),
        )
    }

    /// The files attached to the next message: pictures as thumbnails, other files by name, each
    /// with a button that takes it off again.
    fn render_attachments(&self, state: &ChatState, cx: &mut Context<Self>) -> Option<gpui::Div> {
        if state.attachments.is_empty() {
            return None;
        }
        let id = state.lead.entity_id();
        let items = state.attachments.iter().enumerate().map(|(index, path)| {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string());
            let group = SharedString::from(format!("chat-attachment-{index}"));
            let remove = div()
                .id(("chat-attachment-remove", index))
                .absolute()
                .top(px(-6.))
                .right(px(-6.))
                .size(px(18.))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .bg(hex(Chrome::OVERLAY))
                .border_1()
                .border_color(hex(Chrome::BORDER))
                .invisible()
                .group_hover(group.clone(), |s| s.visible())
                .tooltip(crate::ui::Tooltip::text(t(cx, "chat.remove_attachment"), None))
                .child(icon("x", 11., hex(Chrome::FOREGROUND)))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if let Some(state) = this.chats.get_mut(&id) {
                        if index < state.attachments.len() {
                            state.attachments.remove(index);
                        }
                    }
                    cx.notify();
                }));
            let tile = div()
                .id(("chat-attachment", index))
                .group(group)
                .relative()
                .flex_shrink_0()
                .rounded_lg()
                .border_1()
                .border_color(hex(Chrome::BORDER));
            let tile = if is_picture(path) {
                tile.size(px(72.)).tooltip(crate::ui::Tooltip::text(name, None)).child(
                    div()
                        .size_full()
                        .rounded_lg()
                        .overflow_hidden()
                        .child(gpui::img(path.clone()).size_full().object_fit(gpui::ObjectFit::Cover)),
                )
            } else {
                tile.h(px(40.))
                    .max_w(px(220.))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .bg(hex(Chrome::EDITOR))
                    .t_small()
                    .text_color(hex(Chrome::FOREGROUND))
                    .tooltip(crate::ui::Tooltip::text(crate::ui::tilde(path), None))
                    .child(icon(if path.is_dir() { "folder" } else { "file-text" }, 14., hex(Chrome::MUTED)))
                    .child(div().min_w_0().truncate().child(name))
            };
            tile.child(remove)
        });
        Some(div().flex().flex_wrap().items_end().gap_2().px_1p5().pt_1p5().pb_1().children(items))
    }
}

/// A sent message's files, and where its words start.
type Sent = (Rc<Vec<PathBuf>>, usize);

/// The files a sent message carries: the existing paths it starts with (as the composer puts
/// them, quoted where needed), and the words after them.
pub(super) fn sent_attachments(text: &str) -> (Rc<Vec<PathBuf>>, &str) {
    // Kept per message: a bubble is drawn again on every frame of a reply streaming in, and each
    // path is looked up on disk.
    thread_local! {
        static SEEN: std::cell::RefCell<std::collections::HashMap<u64, Sent>> =
            std::cell::RefCell::new(std::collections::HashMap::new());
    }
    let key = fxhash(text);
    if let Some((files, at)) = SEEN.with(|seen| seen.borrow().get(&key).cloned()) {
        return (files, text.get(at..).unwrap_or(text));
    }
    let (files, words) = find_sent_attachments(text);
    let at = text.len() - words.len();
    let files = Rc::new(files);
    SEEN.with(|seen| {
        let mut seen = seen.borrow_mut();
        // A long conversation: start over rather than grow without end.
        if seen.len() > 1000 {
            seen.clear();
        }
        seen.insert(key, (files.clone(), at));
    });
    (files, words)
}

fn find_sent_attachments(text: &str) -> (Vec<PathBuf>, &str) {
    let mut files = Vec::new();
    let mut rest = text.trim_start();
    loop {
        let (token, after) = if let Some(quoted) = rest.strip_prefix('\'') {
            match quoted.find('\'') {
                Some(end) => (&quoted[..end], &quoted[end + 1..]),
                None => break,
            }
        } else if rest.starts_with('/') {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            (&rest[..end], &rest[end..])
        } else {
            break;
        };
        let path = PathBuf::from(token);
        if !token.starts_with('/') || !path.exists() {
            break;
        }
        files.push(path);
        rest = after.trim_start();
    }
    if files.is_empty() {
        (files, text)
    } else {
        (files, rest)
    }
}

/// What a sent message carried: pictures as thumbnails, other files by name.
pub(super) fn sent_tiles(files: &[PathBuf]) -> gpui::Div {
    div().flex().flex_wrap().justify_end().gap_1p5().children(files.iter().map(|path| {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string());
        let tile = div().flex_shrink_0().rounded_lg().border_1().border_color(hex(Chrome::BORDER)).overflow_hidden();
        if is_picture(path) {
            tile.size(px(96.)).child(gpui::img(path.clone()).size_full().object_fit(gpui::ObjectFit::Cover))
        } else {
            tile.h(px(36.))
                .max_w(px(240.))
                .px_2()
                .flex()
                .items_center()
                .gap_1p5()
                .bg(hex(Chrome::PANEL))
                .t_small()
                .text_color(hex(Chrome::FOREGROUND))
                .child(icon(if path.is_dir() { "folder" } else { "file-text" }, 14., hex(Chrome::MUTED)))
                .child(div().min_w_0().truncate().child(name))
        }
    }))
}

/// Whether a file is shown as a picture among the attachments.
fn is_picture(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp"))
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

/// Copies `text` and says so in the window's toast.
fn copy_text(text: String, window: &mut Window, cx: &mut App) {
    cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
    if let Some(Some(workbench)) = window.root::<Workbench>() {
        workbench.update(cx, |workbench, cx| workbench.show_toast(t(cx, "chat.copied"), cx));
    }
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
                    .on_click(move |_, window, cx| copy_text(copied.trim().to_string(), window, cx)),
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
        "Bash" | "shell" | "exec_command" | "exec" | "local_shell" => Some("chat.tool_run"),
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
                .on_click(move |event, window, cx| toggle.toggle(index, &key, event.position().y, window, cx)),
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
            .on_click(move |event, window, cx| toggle.toggle(index, &key, event.position().y, window, cx));
        let body = open.then(|| {
            div()
                .px_3()
                .pb_3()
                .flex()
                .flex_col()
                .gap_2()
                // A file edit shows what changed; its file is already in the label.
                .children(step.detail.as_ref().filter(|_| step.diff.is_none()).map(|detail| {
                    let command = step.name == "Bash" || step.name.contains("shell") || step.name.contains("exec");
                    if command {
                        code_box(&format!("$ {detail}"), None, Some("bash"))
                    } else {
                        code_box(detail, None, None)
                    }
                }))
                .children(step.diff.as_ref().map(|diff| diff_box(diff)))
                .children(step.output.as_ref().filter(|o| !o.trim().is_empty()).map(|output| {
                    // A file read shows the file in its own colors, its line numbers dimmed.
                    let file = (!step.failed && is_file_read(&step.name)).then_some(step.detail.as_deref()).flatten();
                    div()
                        .px_1()
                        .font_family("JetBrains Mono")
                        .t_small()
                        .text_color(hex(if step.failed { Chrome::ERROR } else { Chrome::MUTED }))
                        .flex()
                        .flex_col()
                        .children(match file.and_then(|path| numbered_file(output, std::path::Path::new(path))) {
                            Some(lines) => lines,
                            None => output.lines().map(|line| div().child(plain_line(line))).collect(),
                        })
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

/// Whether a tool reads a file (its output is the file, with line numbers).
fn is_file_read(name: &str) -> bool {
    matches!(name, "Read" | "read_file" | "view")
}

/// A file as a read tool shows it (`   12→text` or `   12\ttext`): the numbers dimmed, the text in
/// the file's colors. None when the file has no grammar or the output isn't numbered.
fn numbered_file(output: &str, path: &std::path::Path) -> Option<Vec<gpui::Div>> {
    let split = |line: &str| -> Option<(usize, usize)> {
        let digits_end = line.len() - line.trim_start().len();
        let rest = &line[digits_end..];
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        let after = &rest[digits..];
        let sep = if after.starts_with('→') {
            '→'.len_utf8()
        } else if after.starts_with('\t') {
            1
        } else {
            return None;
        };
        (digits > 0).then_some((digits_end + digits, sep))
    };
    let lines: Vec<&str> = output.lines().collect();
    let parts: Vec<Option<(usize, usize)>> = lines.iter().map(|line| split(line)).collect();
    // Numbered, but for a line or two (a clipped read ends with `…`).
    if parts.iter().filter(|part| part.is_none()).count() > 2 {
        return None;
    }
    let text: String =
        lines.iter().zip(&parts).filter_map(|(line, part)| part.map(|(at, sep)| &line[at + sep..])).collect::<Vec<_>>().join("\n");
    let colors = cached_colors(&text, None, Some(path))?;
    let mut colors = colors.iter();
    Some(
        lines
            .iter()
            .zip(&parts)
            .map(|(line, part)| match (part, part.and_then(|_| colors.next())) {
                (Some((at, sep)), Some(spans)) => div()
                    .flex()
                    .child(div().flex_shrink_0().pr_3().text_color(hex(Chrome::MUTED)).child(line[..*at].to_string()))
                    .child(colored_line(&line[at + sep..], spans)),
                _ => div().child(plain_line(line)),
            })
            .collect(),
    )
}

/// A line of code or output with no colors of its own, selectable as the rest of the chat.
fn plain_line(line: &str) -> gpui::AnyElement {
    super::selectable::selectable(if line.is_empty() { " ".to_string() } else { line.to_string() }, Vec::new())
}

/// One line in its grammar's colors.
fn colored_line(text: &str, spans: &[crate::editor::highlight::Span]) -> gpui::AnyElement {
    if text.is_empty() {
        return super::selectable::selectable(" ".to_string(), Vec::new());
    }
    let mut at = 0;
    let mut runs = Vec::new();
    for span in spans {
        let end = (at + span.len).min(text.len());
        if end > at && text.is_char_boundary(at) && text.is_char_boundary(end) {
            let mut style = HighlightStyle { color: Some(crate::theme::raw(span.color)), ..Default::default() };
            if span.bold {
                style.font_weight = Some(gpui::FontWeight::BOLD);
            }
            if span.italic {
                style.font_style = Some(gpui::FontStyle::Italic);
            }
            runs.push((at..end, style));
        }
        at = end;
    }
    super::selectable::selectable(text.to_string(), runs)
}

/// A snippet's colors, a span list per line.
type Colors = Rc<Vec<Vec<crate::editor::highlight::Span>>>;

/// [`crate::editor::highlight::snippet`], kept: a chat draws the same blocks every frame.
fn cached_colors(text: &str, token: Option<&str>, path: Option<&std::path::Path>) -> Option<Colors> {
    thread_local! {
        static CACHE: std::cell::RefCell<std::collections::HashMap<u64, Option<Colors>>> =
            std::cell::RefCell::new(std::collections::HashMap::new());
    }
    crate::editor::highlight::syntaxes()?;
    let key =
        fxhash(&format!("{}\u{1}{}\u{1}{text}", token.unwrap_or_default(), path.map(|p| p.display().to_string()).unwrap_or_default()));
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(found) = cache.get(&key) {
            return found.clone();
        }
        // Long conversations hold many blocks: start over rather than grow without end.
        if cache.len() > 400 {
            cache.clear();
        }
        let colors = crate::editor::highlight::snippet(text, token, path).map(Rc::new);
        cache.insert(key, colors.clone());
        colors
    })
}

/// A code block: its language and a copy button over it, the code in its grammar's colors when
/// `grammar` (or the language) names one.
fn code_box(code: &str, language: Option<&str>, grammar: Option<&str>) -> gpui::Div {
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
                        .on_click(move |_, window, cx| copy_text(copied.clone(), window, cx)),
                ),
        )
        .child(div().px_3().pb_2().font_family("JetBrains Mono").t_small().text_color(hex(Chrome::FOREGROUND)).flex().flex_col().children(
            {
                let shown: String = code.lines().take(400).collect::<Vec<_>>().join("\n");
                let colors = cached_colors(&shown, grammar.or(language), None);
                shown
                    .lines()
                    .enumerate()
                    .map(|(n, line)| match colors.as_ref().and_then(|c| c.get(n)) {
                        Some(spans) => div().child(colored_line(line, spans)),
                        None => div().child(plain_line(line)),
                    })
                    .collect::<Vec<_>>()
            },
        ))
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
            out = out.child(code_box(&code.join("\n"), (!language.is_empty()).then_some(language), None));
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

/// Running text with `code` and **bold** set apart, selectable, its addresses opening.
fn inline(text: &str) -> AnyElement {
    let (plain, spans) = inline_spans(text);
    let highlights = spans.into_iter().map(|(range, code)| {
        let style = if code {
            HighlightStyle { color: Some(hex(Chrome::ORANGE)), background_color: Some(hex(Chrome::PANEL)), ..Default::default() }
        } else {
            HighlightStyle { font_weight: Some(gpui::FontWeight::SEMIBOLD), color: Some(hex(Chrome::BRIGHT)), ..Default::default() }
        };
        (range, style)
    });
    super::selectable::selectable(plain, highlights.collect::<Vec<_>>())
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
    fn sent_files_lead_the_message() {
        let dir = std::env::temp_dir().join(format!("agentty-sent-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (plain, spaced) = (dir.join("shot.png"), dir.join("my notes.txt"));
        std::fs::write(&plain, b"x").unwrap();
        std::fs::write(&spaced, b"x").unwrap();
        let text = format!("{} '{}' what is in these?", plain.display(), spaced.display());
        let (files, words) = sent_attachments(&text);
        assert_eq!(*files, vec![plain.clone(), spaced.clone()]);
        assert_eq!(words, "what is in these?");
        // A path that isn't there is part of what was written.
        let (files, words) = sent_attachments("/no/such/file explain");
        assert!(files.is_empty());
        assert_eq!(words, "/no/such/file explain");
        let _ = std::fs::remove_dir_all(&dir);
    }

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
