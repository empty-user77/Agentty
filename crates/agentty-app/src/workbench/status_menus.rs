//! Status bar shortcuts for the active tab's agent: loaded skills, subagents and MCP servers (with
//! live connection state), each an icon that opens a menu upwards.

use super::{Page, Workbench};
use crate::i18n::{t, tf};
use crate::launch::PaneKind;
use crate::settings::AdvisorChoice;
use crate::theme::{hex, Chrome};
use crate::ui::{icon, popover, IconSize, TypeScale};
use agentty_bridge::context::ContextSnapshot;
use agentty_bridge::extensions::{discover, parse_claude_mcp_list, parse_codex_mcp_list, Extension, ExtensionKind, McpHealth, McpStatus};
use agentty_bridge::model::Agent;
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, SharedString};
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StatusMenu {
    Context,
    Skills,
    Agents,
    Mcp,
    Advisor,
}

type Key = (Agent, PathBuf);

fn status_menu_key(menu: StatusMenu) -> &'static str {
    match menu {
        StatusMenu::Context => "status-context",
        StatusMenu::Skills => "status-skills",
        StatusMenu::Agents => "status-agents",
        StatusMenu::Mcp => "status-mcp",
        StatusMenu::Advisor => "status-advisor",
    }
}
/// Result of `mcp list`: `None` while it runs.
type McpCheck = Option<Result<Vec<McpStatus>, String>>;

#[derive(Default)]
pub struct AgentInventory {
    items: Option<(Key, Vec<Extension>)>,
    mcp: Option<(Key, Instant, McpCheck)>,
    /// Context of a session (agent, session id): `None` while it loads or when it can't be read.
    pub(super) context: Option<((Agent, String), bool, Option<ContextSnapshot>)>,
}

const MCP_TTL: Duration = Duration::from_secs(60);
const MAX_ROWS: usize = 40;

/// Runs `<agent> mcp list` in `cwd` through the user's interactive login shell (full output).
fn mcp_list(agent: Agent, cwd: PathBuf) -> Result<Vec<McpStatus>, String> {
    let program = if agent == Agent::Claude { "claude" } else { "codex" };
    let output = std::process::Command::new(crate::launch::LaunchSpec::shell_program())
        .args(["-l", "-i", "-c", &format!("{program} mcp list")])
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(if agent == Agent::Claude { parse_claude_mcp_list(&text) } else { parse_codex_mcp_list(&text) })
}

impl Workbench {
    fn active_agent(&self, cx: &gpui::App) -> Option<Key> {
        let pane = self.active_pane()?;
        let view = pane.read(cx);
        let agent = view.agent_kind().and_then(PaneKind::agent)?;
        Some((agent, view.display_cwd()))
    }

    pub(super) fn toggle_status_menu(&mut self, menu: StatusMenu, cx: &mut Context<Self>) {
        // The click that closed the menu from outside must not reopen it.
        if self.just_dismissed(status_menu_key(menu)) {
            return;
        }
        if self.status_menu == Some(menu) {
            self.status_menu = None;
            return cx.notify();
        }
        self.status_menu = Some(menu);
        if menu == StatusMenu::Advisor {
            return cx.notify();
        }
        if menu == StatusMenu::Context {
            self.load_context(cx);
            return cx.notify();
        }
        let Some(key) = self.active_agent(cx) else { return cx.notify() };
        if self.inventory.items.as_ref().is_none_or(|(k, _)| *k != key) {
            let (agent, cwd) = key.clone();
            let task = cx.background_spawn(async move { discover(agent, Some(&cwd)) });
            let task_key = key.clone();
            cx.spawn(async move |this, cx| {
                let items = task.await;
                let _ = this.update(cx, |this, cx| {
                    this.inventory.items = Some((task_key, items));
                    cx.notify();
                });
            })
            .detach();
        }
        let stale = self.inventory.mcp.as_ref().is_none_or(|(k, at, _)| *k != key || at.elapsed() > MCP_TTL);
        if menu == StatusMenu::Mcp && stale {
            self.inventory.mcp = Some((key.clone(), Instant::now(), None));
            let (agent, cwd) = key.clone();
            let task = cx.background_spawn(async move { mcp_list(agent, cwd) });
            cx.spawn(async move |this, cx| {
                let result = task.await;
                let _ = this.update(cx, |this, cx| {
                    if let Some((k, _, slot)) = this.inventory.mcp.as_mut().filter(|(k, _, _)| *k == key) {
                        let _ = k;
                        *slot = Some(result);
                    }
                    cx.notify();
                });
            })
            .detach();
        }
        cx.notify();
    }

    /// `[skills] [agents] [MCP]` icons for the status bar; empty when the active tab isn't an agent.
    pub(super) fn render_status_icons(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let key = self.active_agent(cx)?;
        let button = |id: &'static str, glyph: &'static str, menu: StatusMenu, cx: &mut Context<Self>| {
            let open = self.status_menu == Some(menu);
            let label = match menu {
                StatusMenu::Context => t(cx, "context.title"),
                StatusMenu::Skills => t(cx, "status.skills"),
                StatusMenu::Agents => t(cx, "status.agents"),
                StatusMenu::Mcp => t(cx, "status.mcp"),
                StatusMenu::Advisor => t(cx, "advisor.title"),
            };
            div()
                .id(id)
                .tooltip(crate::ui::Tooltip::text(label, None))
                .h_full()
                .px_1p5()
                .flex()
                .items_center()
                .cursor_pointer()
                .when(open, |d| d.bg(hex(Chrome::SELECTED)))
                .hover(|s| s.bg(hex(Chrome::HOVER)))
                .child(icon(glyph, 13., hex(if open { Chrome::BRIGHT } else { Chrome::MUTED })))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.toggle_status_menu(menu, cx)))
        };
        let menu = self.status_menu.filter(|m| *m != StatusMenu::Advisor).map(|menu| self.render_status_menu(menu, &key, cx));
        Some(
            div()
                .relative()
                .h_full()
                .flex()
                .items_center()
                .child(button("status-context", "brain", StatusMenu::Context, cx))
                .child(button("status-skills", "sparkles", StatusMenu::Skills, cx))
                .child(button("status-agents", "bot", StatusMenu::Agents, cx))
                .child(button("status-mcp", "blocks", StatusMenu::Mcp, cx))
                .children(menu)
                .into_any_element(),
        )
    }

    fn render_status_menu(&self, menu: StatusMenu, key: &Key, cx: &mut Context<Self>) -> AnyElement {
        let agent = key.0;
        if menu == StatusMenu::Context {
            return self.render_context_menu(agent, cx);
        }
        let title = match menu {
            StatusMenu::Context | StatusMenu::Advisor => t(cx, "context.title"),
            StatusMenu::Skills => t(cx, "status.skills"),
            StatusMenu::Agents => t(cx, "status.agents"),
            StatusMenu::Mcp => t(cx, "status.mcp"),
        };
        let mut list = div().id("status-menu-list").flex().flex_col().max_h(px(360.)).overflow_y_scroll();
        let items = self.inventory.items.as_ref().filter(|(k, _)| k == key).map(|(_, items)| items);
        let row = |id: SharedString, dot: Option<u32>, name: String, meta: String| {
            div()
                .id(id)
                .px_2()
                .py_1()
                .rounded_md()
                .flex()
                .items_center()
                .gap_2()
                .t_small()
                .cursor_pointer()
                .hover(|s| s.bg(hex(Chrome::HOVER)))
                .when_some(dot, |d, color| d.child(div().flex_shrink_0().size(px(7.)).rounded_full().bg(hex(color))))
                .child(div().flex_shrink_0().max_w(px(170.)).truncate().text_color(hex(Chrome::BRIGHT)).child(name))
                .child(div().flex_1().min_w_0().truncate().text_color(hex(Chrome::MUTED)).child(meta))
        };
        match menu {
            StatusMenu::Context | StatusMenu::Advisor => {}
            StatusMenu::Skills | StatusMenu::Agents => {
                let kind = if menu == StatusMenu::Skills { ExtensionKind::Skill } else { ExtensionKind::Agent };
                match items {
                    None => list = list.child(crate::ui::loading_row(t(cx, "ext.loading"))),
                    Some(items) => {
                        let matching: Vec<&Extension> = items.iter().filter(|e| e.kind == kind).collect();
                        if matching.is_empty() {
                            list = list.child(crate::ui::hint(t(cx, "status.none")));
                        }
                        for (index, item) in matching.into_iter().take(MAX_ROWS).enumerate() {
                            let invocation = item.invocation.clone();
                            list = list.child(
                                row(SharedString::from(format!("status-item-{index}")), None, item.name.clone(), item.description.clone())
                                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                        this.status_menu = None;
                                        if let Some(text) = invocation.clone() {
                                            this.use_in_agent(agent, text, false, window, cx);
                                        }
                                        cx.notify();
                                    })),
                            );
                        }
                    }
                }
            }
            StatusMenu::Mcp => match self.inventory.mcp.as_ref().filter(|(k, _, _)| k == key).and_then(|(_, _, r)| r.as_ref()) {
                None => list = list.child(crate::ui::loading_row(t(cx, "status.mcp_checking"))),
                Some(Err(error)) => list = list.child(crate::ui::hint(error.clone())),
                Some(Ok(servers)) if servers.is_empty() => list = list.child(crate::ui::hint(t(cx, "status.none"))),
                Some(Ok(servers)) => {
                    for (index, server) in servers.iter().take(MAX_ROWS).enumerate() {
                        let (color, label) = match server.health {
                            McpHealth::Connected => (Chrome::SUCCESS, t(cx, "status.mcp_connected")),
                            McpHealth::Failed => (Chrome::ERROR, t(cx, "status.mcp_failed")),
                            McpHealth::NeedsAuth => (Chrome::WARNING, t(cx, "status.mcp_auth")),
                            McpHealth::Enabled => (Chrome::SUCCESS, t(cx, "status.mcp_enabled")),
                            McpHealth::Disabled => (Chrome::MUTED, t(cx, "status.mcp_disabled")),
                            McpHealth::Unknown => (Chrome::MUTED, "?"),
                        };
                        list = list.child(row(
                            SharedString::from(format!("status-mcp-{index}")),
                            Some(color),
                            server.name.clone(),
                            label.into(),
                        ));
                    }
                }
            },
        }
        let count = match menu {
            StatusMenu::Mcp => self.inventory.mcp.as_ref().and_then(|(_, _, r)| r.as_ref()).and_then(|r| r.as_ref().ok()).map(|s| s.len()),
            _ => items.map(|items| {
                items
                    .iter()
                    .filter(|e| e.kind == if menu == StatusMenu::Skills { ExtensionKind::Skill } else { ExtensionKind::Agent })
                    .count()
            }),
        };
        let header = div()
            .px_2()
            .pt_1()
            .pb_1p5()
            .flex()
            .items_center()
            .gap_2()
            .t_caption()
            .text_color(hex(Chrome::MUTED))
            .child(div().flex_1().child(format!("{} · {title}", agent.display_name())))
            .children(count.map(|n| n.to_string()));
        let footer = div()
            .id("status-menu-manage")
            .mt_1()
            .px_2()
            .py_1()
            .rounded_md()
            .flex()
            .items_center()
            .gap_1p5()
            .t_small()
            .text_color(hex(Chrome::BLUE))
            .cursor_pointer()
            .hover(|s| s.bg(hex(Chrome::HOVER)))
            .child(icon("blocks", IconSize::INLINE, hex(Chrome::BLUE)))
            .child(t(cx, "status.manage"))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.status_menu = None;
                this.page = Some(Page::Extensions);
                cx.notify();
            }));
        let panel = popover()
            .w(px(380.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                if let Some(menu) = this.status_menu.take() {
                    this.note_dismissed(status_menu_key(menu));
                }
                cx.notify();
            }))
            .child(header)
            .child(list)
            .child(div().h(px(1.)).bg(hex(Chrome::OVERLAY_BORDER)))
            .child(footer);
        let _ = tf;
        div()
            .absolute()
            .bottom(px(24.))
            .right_0()
            .child(gpui::deferred(crate::ui::fade_in("status-menu-fade", panel)).with_priority(3))
            .into_any_element()
    }
}

impl Workbench {
    /// Restarts the active Claude tab with `advisor`, resuming its conversation.
    pub(super) fn set_pane_advisor(&mut self, advisor: AdvisorChoice, cx: &mut Context<Self>) {
        self.status_menu = None;
        let Some(pane) = self.active_pane() else { return cx.notify() };
        let view = pane.read(cx);
        if view.advisor().is_none() {
            self.set_status(t(cx, "advisor.not_claude"), cx);
        } else if view.advisor() == Some(advisor) {
            cx.notify();
        } else if view.is_busy() {
            self.set_status(t(cx, "advisor.busy"), cx);
        } else if pane.update(cx, |view, cx| view.restart_with_advisor(advisor, cx)) {
            self.set_status(tf(cx, "advisor.restarted", &[("advisor", t(cx, advisor.label_key()))]), cx);
        }
        cx.notify();
    }

    /// "Advisor: Opus" in the status bar for a Claude tab; opens a menu to switch it.
    pub(super) fn render_advisor_chip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let current = self.active_pane()?.read(cx).advisor()?;
        let open = self.status_menu == Some(StatusMenu::Advisor);
        let active = current.model().is_some();
        let chip = div()
            .id("status-advisor")
            .tooltip(crate::ui::Tooltip::text(t(cx, "advisor.tooltip"), None))
            .h_full()
            .px_1p5()
            .flex()
            .items_center()
            .gap_1()
            .cursor_pointer()
            .when(open, |d| d.bg(hex(Chrome::SELECTED)))
            .hover(|s| s.bg(hex(Chrome::HOVER)))
            .child(icon("message-circle-question", 13., hex(if active { Chrome::BLUE } else { Chrome::MUTED })))
            .child(div().text_color(hex(if active { Chrome::FOREGROUND } else { Chrome::MUTED })).child(format!(
                "{} · {}",
                t(cx, "advisor.title"),
                t(cx, current.label_key())
            )))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.toggle_status_menu(StatusMenu::Advisor, cx)));
        let menu = open.then(|| {
            let mut list = div().flex().flex_col();
            for choice in AdvisorChoice::ALL {
                let selected = choice == current;
                list = list.child(
                    div()
                        .id(SharedString::from(format!("advisor-choice-{choice:?}")))
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .flex()
                        .items_center()
                        .gap_2()
                        .t_small()
                        .cursor_pointer()
                        .hover(|s| s.bg(hex(Chrome::HOVER)))
                        .child(div().w(px(14.)).flex_shrink_0().when(selected, |d| d.child(icon("check", 12., hex(Chrome::BLUE)))))
                        .child(div().flex_1().text_color(hex(Chrome::BRIGHT)).child(t(cx, choice.label_key())))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.set_pane_advisor(choice, cx))),
                );
            }
            let panel = popover()
                .w(px(300.))
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    if this.status_menu == Some(StatusMenu::Advisor) {
                        this.status_menu = None;
                        this.note_dismissed(status_menu_key(StatusMenu::Advisor));
                    }
                    cx.notify();
                }))
                .child(div().px_2().pt_1().pb_1p5().t_caption().text_color(hex(Chrome::MUTED)).child(t(cx, "advisor.menu_title")))
                .child(list)
                .child(div().h(px(1.)).my_1().bg(hex(Chrome::OVERLAY_BORDER)))
                .child(div().px_2().pb_1().t_caption().text_color(hex(Chrome::MUTED)).child(t(cx, "advisor.menu_hint")));
            div().absolute().bottom(px(24.)).left_0().child(gpui::deferred(crate::ui::fade_in("advisor-menu-fade", panel)).with_priority(3))
        });
        Some(div().relative().h_full().flex().items_center().child(chip).children(menu).into_any_element())
    }
}
