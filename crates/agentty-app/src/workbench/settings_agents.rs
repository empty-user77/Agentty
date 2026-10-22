//! Settings → Agents: the agent CLIs on this machine, which of them the launcher, the start page and
//! the palette offer, how much they may do without asking, and Claude Code's own features (agent
//! teams, thinking, updates) switched in its user settings.

use super::settings_page::{note_for_search, row_with_hint, section_card, section_with_hint, toggle};
use super::Workbench;
use crate::i18n::{t, tf};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{action_button, action_button_with_icon, icon, IconSize, TypeScale};
use agentty_bridge::claude_features::ClaudeFeature;
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, Div, SharedString};

/// An agent CLI the page lists: Claude Code and Codex first, then the others Agentty knows.
struct AgentRow {
    id: &'static str,
    name: &'static str,
    binary: &'static str,
    maker: &'static str,
    /// Where to read how to install it (the start page's install hint), when Agentty knows.
    install_url: Option<&'static str>,
}

fn agent_rows() -> Vec<AgentRow> {
    let mut rows = vec![
        AgentRow {
            id: "claude",
            name: "Claude Code",
            binary: "claude",
            maker: "Anthropic",
            install_url: Some(super::chrome::CLAUDE_INSTALL_URL),
        },
        AgentRow { id: "codex", name: "Codex", binary: "codex", maker: "OpenAI", install_url: Some(super::chrome::CODEX_INSTALL_URL) },
    ];
    rows.extend(crate::agents::OTHER_AGENTS.iter().map(|a| AgentRow {
        id: a.id,
        name: a.name,
        binary: a.binary,
        maker: a.maker,
        install_url: None,
    }));
    rows
}

/// The id of an agent row from the debug driver's argument.
pub(super) fn agent_row_id(argument: &str) -> Option<&'static str> {
    agent_rows().into_iter().map(|row| row.id).find(|id| *id == argument.trim())
}

fn feature_label(feature: ClaudeFeature) -> (&'static str, &'static str) {
    match feature {
        ClaudeFeature::AgentTeams => ("agents.claude_teams", "agents.claude_teams_hint"),
        ClaudeFeature::AlwaysThinking => ("agents.claude_thinking", "agents.claude_thinking_hint"),
        ClaudeFeature::AutoUpdates => ("agents.claude_updates", "agents.claude_updates_hint"),
    }
}

/// A small rounded label with a coloured outline: "Connected", "3 found", "Not installed".
pub(super) fn pill(text: impl Into<SharedString>, color: u32) -> Div {
    div()
        .flex_shrink_0()
        .px_2()
        .py_0p5()
        .rounded_full()
        .border_1()
        .border_color(hex_alpha(color, 0.45))
        .bg(hex_alpha(color, 0.1))
        .t_caption()
        .text_color(hex(color))
        .child(text.into())
}

/// Two-way switch drawn as a segmented control ("On | Off"), for a row where both words matter.
pub(super) fn on_off(
    id: &'static str,
    key: &str,
    on: bool,
    set: impl Fn(bool, &mut gpui::Window, &mut gpui::App) + Clone + 'static,
    cx: &gpui::App,
) -> Div {
    let option = |value: bool, label: &'static str| {
        let set = set.clone();
        div()
            .id(SharedString::from(format!("{id}-{key}-{value}")))
            .px_2p5()
            .py_0p5()
            .rounded_md()
            .t_small()
            .cursor_pointer()
            .when(on == value, |d| d.bg(hex(Chrome::SELECTED)).text_color(hex(Chrome::BRIGHT)))
            .when(on != value, |d| d.text_color(hex(Chrome::MUTED)).hover(|s| s.text_color(hex(Chrome::BRIGHT))))
            .child(label)
            .on_click(move |_: &ClickEvent, window, cx| set(value, window, cx))
    };
    div()
        .flex_shrink_0()
        .flex()
        .p_0p5()
        .gap_0p5()
        .rounded_lg()
        .border_1()
        .border_color(hex(Chrome::BORDER))
        .bg(hex(Chrome::EDITOR))
        .child(option(true, t(cx, "agents.enabled")))
        .child(option(false, t(cx, "agents.disabled")))
}

/// The logo of an agent on a small square, as the rows of this page and the integrations show it.
pub(super) fn logo_tile(art: impl IntoElement, dim: bool) -> Div {
    div()
        .flex_shrink_0()
        .size(px(34.))
        .rounded_lg()
        .border_1()
        .border_color(hex(Chrome::BORDER))
        .bg(hex(Chrome::EDITOR))
        .flex()
        .items_center()
        .justify_center()
        .when(dim, |d| d.opacity(0.55))
        .child(art)
}

impl Workbench {
    fn load_claude_features(&mut self, cx: &mut Context<Self>) {
        if self.settings_ui.claude_features_busy {
            return;
        }
        self.settings_ui.claude_features_busy = true;
        let task = cx.background_spawn(async { agentty_bridge::claude_features::read().map_err(|e| format!("{e:#}")) });
        cx.spawn(async move |this, cx| {
            let read = task.await;
            let _ = this.update(cx, |this, cx| {
                this.settings_ui.claude_features_busy = false;
                this.settings_ui.claude_features = Some(read);
                // Its switches are searchable once they are known.
                this.settings_ui.reindex();
                cx.notify();
            });
        })
        .detach();
    }

    fn set_claude_feature(&mut self, feature: ClaudeFeature, on: bool, cx: &mut Context<Self>) {
        if self.settings_ui.claude_features_busy {
            return;
        }
        self.settings_ui.claude_features_busy = true;
        let task = cx.background_spawn(async move {
            agentty_bridge::claude_features::set(feature, on)?;
            agentty_bridge::claude_features::read()
        });
        cx.spawn(async move |this, cx| {
            let result = task.await.map_err(|e| format!("{e:#}"));
            let _ = this.update(cx, |this, cx| {
                this.settings_ui.claude_features_busy = false;
                match result {
                    Ok(read) => {
                        this.settings_ui.claude_features = Some(Ok(read));
                        this.set_status(t(cx, "agents.claude_saved"), cx);
                    }
                    Err(err) => this.settings_ui.claude_features = Some(Err(err)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Looks for agent CLIs again now (the button; otherwise at most once a minute).
    fn redetect_agents(&mut self, cx: &mut Context<Self>) {
        self.installed_at = None;
        self.settings_ui.reindex();
        self.detect_agents(cx);
        self.settings_ui.claude_features = None;
        cx.notify();
    }

    pub(super) fn render_agents_settings(&mut self, cx: &mut Context<Self>) -> Div {
        self.detect_agents(cx);
        if self.settings_ui.claude_features.is_none() {
            self.load_claude_features(cx);
        }
        let prefs = crate::settings::settings(cx).clone();
        let installed = self.installed.clone();
        let rows = agent_rows();
        let (found, missing): (Vec<&AgentRow>, Vec<&AgentRow>) =
            rows.iter().partition(|row| installed.as_ref().is_some_and(|i| i.has(row.binary)));

        let permissions = section_with_hint(t(cx, "agents.permissions"), t(cx, "agents.permissions_hint")).child(row_with_hint(
            t(cx, "settings.always_bypass"),
            t(cx, "settings.always_bypass_hint"),
            toggle("always-bypass", prefs.always_bypass, |s| s.always_bypass = !s.always_bypass, cx),
        ));

        note_for_search(t(cx, "agents.installed"), t(cx, "agents.installed_hint"));
        let header = div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_0p5()
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
                                    .child(t(cx, "agents.installed")),
                            )
                            .when(installed.is_some(), |d| {
                                d.child(pill(tf(cx, "agents.found", &[("n", &found.len().to_string())]), Chrome::MUTED))
                            }),
                    )
                    .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "agents.installed_hint"))),
            )
            .child(action_button_with_icon(
                "agents-refresh",
                "refresh-cw",
                t(cx, "agents.refresh"),
                cx.listener(|this, _: &ClickEvent, _, cx| this.redetect_agents(cx)),
            ));
        let mut list = section_card().child(header);
        if installed.is_none() {
            list = list
                .child(div().pt_3().border_t_1().border_color(hex(Chrome::BORDER)).child(crate::ui::loading_row(t(cx, "agents.looking"))));
        } else if found.is_empty() {
            list = list.child(
                div()
                    .pt_3()
                    .border_t_1()
                    .border_color(hex(Chrome::BORDER))
                    .t_small()
                    .text_color(hex(Chrome::MUTED))
                    .child(t(cx, "agents.none")),
            );
        }
        for row in &found {
            list = list.child(self.render_agent_row(row, &prefs, cx));
        }

        let mut page = div().flex().flex_col().child(permissions).child(list);
        if installed.is_some() && !missing.is_empty() {
            note_for_search(t(cx, "agents.available"), t(cx, "agents.available_hint"));
            let mut available = section_card().child(
                div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
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
                                    .child(t(cx, "agents.available")),
                            )
                            .child(pill(tf(cx, "agents.count", &[("n", &missing.len().to_string())]), Chrome::MUTED)),
                    )
                    .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "agents.available_hint"))),
            );
            for row in &missing {
                let (id, name, url) = (row.id, row.name, row.install_url);
                note_for_search(name, "");
                available = available.child(
                    div()
                        .pt_3()
                        .border_t_1()
                        .border_color(hex(Chrome::BORDER))
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(logo_tile(crate::brand::avatar_colored(row.id, 18.), true))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(div().t_body().text_color(hex(Chrome::FOREGROUND)).child(row.name))
                                .child(div().t_small().font_family(crate::settings::BUNDLED_FONT).text_color(hex(Chrome::MUTED)).child(
                                    if row.maker.is_empty() { row.binary.to_string() } else { format!("{} · {}", row.binary, row.maker) },
                                )),
                        )
                        .children(url.map(|url| {
                            action_button(
                                SharedString::from(format!("agent-install-{id}")),
                                t(cx, "agents.install_guide"),
                                cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    this.install_hint = Some((id, name, url));
                                    cx.notify();
                                }),
                            )
                        })),
                );
            }
            page = page.child(available);
        }
        page
    }

    fn render_agent_row(&self, row: &AgentRow, prefs: &crate::settings::Settings, cx: &mut Context<Self>) -> Div {
        let id = row.id;
        note_for_search(row.name, "");
        let enabled = prefs.agent_enabled(id);
        let version = self.installed.as_ref().and_then(|i| i.version(row.binary)).map(str::to_string);
        let has_details = matches!(id, "claude" | "codex");
        let open = has_details && (self.settings_ui.agents_open.contains(id) || super::settings_page::collecting());
        let command = match (id, prefs.always_bypass) {
            ("claude", true) => "claude --dangerously-skip-permissions".to_string(), // audit: ok — shown on the page only, what the always-bypass opt-in launches
            ("codex", true) => "codex --dangerously-bypass-approvals-and-sandbox".to_string(), // audit: ok — shown on the page only
            _ => row.binary.to_string(),
        };
        let header = div()
            .id(SharedString::from(format!("agent-row-{id}")))
            .flex()
            .items_center()
            .gap_3()
            .when(has_details, |d| {
                d.cursor_pointer().on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if !this.settings_ui.agents_open.remove(id) {
                        this.settings_ui.agents_open.insert(id);
                    }
                    this.settings_ui.reindex();
                    cx.notify();
                }))
            })
            .child(logo_tile(crate::brand::avatar_colored(id, 18.), !enabled))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().t_body().text_color(hex(if enabled { Chrome::BRIGHT } else { Chrome::MUTED })).child(row.name))
                            .children(version.map(|v| div().t_caption().text_color(hex(Chrome::MUTED)).child(format!("v{v}")))),
                    )
                    .child(
                        div().truncate().t_small().font_family(crate::settings::BUNDLED_FONT).text_color(hex(Chrome::MUTED)).child(command),
                    ),
            )
            .child(
                // Its own click target: choosing on/off must not open or close the row too.
                div().on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation()).child(on_off(
                    "agent-enabled",
                    id,
                    enabled,
                    move |on, _, cx| crate::settings::update_settings(cx, move |s| s.set_agent_enabled(id, on)),
                    cx,
                )),
            )
            .child(div().w(px(16.)).flex_shrink_0().when(has_details, |d| {
                d.child(icon(if open { "chevron-up" } else { "chevron-down" }, IconSize::INLINE, hex(Chrome::MUTED)))
            }));
        let mut item = div()
            .pt_3()
            .border_t_1()
            .border_color(hex(Chrome::BORDER))
            .flex()
            .flex_col()
            .gap_3()
            .child(super::settings_page::highlighted(row.name, header));
        if open {
            item = item.child(super::settings_page::in_card(id, || match id {
                "claude" => self.render_claude_details(cx),
                _ => render_codex_details(cx),
            }));
        }
        item
    }

    fn render_claude_details(&self, cx: &mut Context<Self>) -> Div {
        let path = agentty_bridge::claude_features::settings_path();
        let mut details = details_box().child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "agents.claude_features_hint")));
        match &self.settings_ui.claude_features {
            None => details = details.child(crate::ui::loading_row(t(cx, "agents.reading"))),
            Some(Err(err)) => details = details.child(div().t_small().text_color(hex(Chrome::ERROR)).child(err.clone())),
            Some(Ok(features)) => {
                for (feature, on) in features.clone() {
                    let (label, hint) = feature_label(feature);
                    note_for_search(t(cx, label), t(cx, hint));
                    details = details.child(super::settings_page::highlighted(
                        t(cx, label),
                        div()
                            .flex()
                            .items_center()
                            .gap_4()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap_0p5()
                                    .child(div().t_body().text_color(hex(Chrome::FOREGROUND)).child(t(cx, label)))
                                    .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, hint))),
                            )
                            .child(super::settings_page::switch(
                                SharedString::from(format!("claude-feature-{feature:?}")),
                                on,
                                cx.listener(move |this, _: &ClickEvent, _, cx| this.set_claude_feature(feature, !on, cx)),
                            )),
                    ));
                }
            }
        }
        details.child(file_line(path, "agents-claude-open", cx))
    }
}

fn details_box() -> Div {
    div().ml(px(46.)).p_3().flex().flex_col().gap_3().rounded_lg().border_1().border_color(hex(Chrome::BORDER)).bg(hex(Chrome::EDITOR))
}

/// "Settings file: ~/.claude/settings.json  [Open]".
fn file_line(path: std::path::PathBuf, id: &'static str, cx: &mut Context<Workbench>) -> AnyElement {
    let shown = crate::ui::tilde(&path);
    div()
        .flex()
        .items_center()
        .gap_2()
        .child(div().flex_1().min_w_0().truncate().t_small().text_color(hex(Chrome::MUTED)).child(tf(
            cx,
            "agents.config_file",
            &[("path", &shown)],
        )))
        .child(action_button(id, t(cx, "agents.open_file"), move |_: &ClickEvent, _, cx| cx.open_with_system(&path)))
        .into_any_element()
}

fn render_codex_details(cx: &mut Context<Workbench>) -> Div {
    let path = agentty_bridge::fsutil::home().join(".codex").join("config.toml");
    details_box().child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "agents.codex_hint"))).child(file_line(
        path,
        "agents-codex-open",
        cx,
    ))
}
