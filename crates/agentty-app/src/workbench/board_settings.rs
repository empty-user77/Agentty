//! Settings → Development board: turning the board on, how tickets run (which agent, a worktree or
//! the project folder, whether agents may split their work), and Jira Cloud for importing issues.

use super::settings_page::{row_with_hint, section, toggle};
use super::Workbench;
use crate::i18n::{t, tf};
use crate::settings::{update_settings, BoardAgent, BoardCleanup, BoardRunMode, BoardSplit};
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::{hex, Chrome};
use crate::ui::{action_button, chip, TypeScale};
use gpui::{div, prelude::*, px, AppContext, ClickEvent, Context, Entity, Focusable, Subscription, Window};

/// The Jira fields of the settings page, kept across renders.
pub struct JiraFields {
    site: Entity<TextInput>,
    email: Entity<TextInput>,
    jql: Entity<TextInput>,
    token: Entity<TextInput>,
    _subscriptions: Vec<Subscription>,
}

#[derive(Default)]
pub struct JiraUi {
    fields: Option<JiraFields>,
    /// Whether a token is saved; read once (off the UI thread) when the Jira settings first show.
    token_saved: Option<bool>,
    /// That read is under way.
    token_checking: bool,
    busy: bool,
    /// The last check or save: (went well, what to say).
    message: Option<(bool, String)>,
}

/// A text field box that focuses its field wherever it is clicked.
pub(super) fn field_box(input: &Entity<TextInput>) -> gpui::Div {
    let focus = input.clone();
    div()
        .flex_1()
        .min_w_0()
        .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
            window.focus(&focus.focus_handle(cx));
            // Otherwise the workbench behind, which tracks focus too, takes it back on the same click.
            window.prevent_default();
            cx.stop_propagation();
        })
        .cursor_text()
        .px_2()
        .py_1()
        .rounded_md()
        .border_1()
        .border_color(hex(Chrome::BORDER))
        .bg(hex(0x1a1a1a))
        .t_body()
        .text_color(hex(Chrome::BRIGHT))
        .child(input.clone())
}

impl Workbench {
    fn jira_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) -> &JiraFields {
        if self.board_jira.fields.is_none() {
            let jira = crate::settings::settings(cx).board.jira.clone();
            let text = |value: String, placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>| {
                cx.new(|cx| {
                    let mut input = TextInput::new("", placeholder, window, cx);
                    input.set_text(value, cx);
                    input
                })
            };
            let site = text(jira.site, "https://team.atlassian.net", window, cx);
            let email = text(jira.email, "you@example.com", window, cx);
            let jql = text(jira.jql, "project = AB AND statusCategory != Done ORDER BY updated DESC", window, cx);
            let token = cx.new(|cx| TextInput::localized("", "board.jira_token_placeholder", window, cx).masked());
            // Typed values are kept as they are typed; the token only with Save or Enter.
            let keep = |input: &Entity<TextInput>, put: fn(&mut crate::settings::JiraSettings, String), cx: &mut Context<Self>| {
                cx.subscribe(input, move |_, input, event: &TextInputEvent, cx| {
                    if matches!(event, TextInputEvent::Changed) {
                        let value = input.read(cx).text().trim().to_string();
                        update_settings(cx, move |s| put(&mut s.board.jira, value));
                    }
                })
            };
            let subscriptions = vec![
                keep(&site, |j, v| j.site = v, cx),
                keep(&email, |j, v| j.email = v, cx),
                keep(&jql, |j, v| j.jql = v, cx),
                cx.subscribe(&token, |this, _, event: &TextInputEvent, cx| {
                    if matches!(event, TextInputEvent::Confirmed) {
                        this.save_jira_token(cx);
                    }
                }),
            ];
            self.board_jira.fields = Some(JiraFields { site, email, jql, token, _subscriptions: subscriptions });
        }
        self.board_jira.fields.as_ref().expect("made above")
    }

    fn save_jira_token(&mut self, cx: &mut Context<Self>) {
        let Some(fields) = &self.board_jira.fields else { return };
        let token = fields.token.read(cx).text().trim().to_string();
        if token.is_empty() {
            return;
        }
        fields.token.update(cx, |input, cx| input.set_text("", cx));
        self.board_jira.busy = true;
        cx.spawn(async move |this, cx| {
            let saved = cx.background_spawn(async move { agentty_bridge::jira::store_token(&token) }).await;
            let _ = this.update(cx, |this, cx| {
                this.board_jira.busy = false;
                this.board_jira.message = Some(match saved {
                    Ok(()) => {
                        this.board_jira.token_saved = Some(true);
                        (true, t(cx, "board.jira_token_saved").to_string())
                    }
                    Err(err) => (false, format!("{err:#}")),
                });
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Deletes the token in the background: the Keychain may ask the user or be slow, and the
    /// window must not freeze meanwhile.
    fn delete_jira_token(&mut self, cx: &mut Context<Self>) {
        if self.board_jira.busy {
            return;
        }
        self.board_jira.busy = true;
        cx.spawn(async move |this, cx| {
            let deleted = cx.background_spawn(async move { agentty_bridge::jira::delete_token() }).await;
            let _ = this.update(cx, |this, cx| {
                this.board_jira.busy = false;
                match deleted {
                    Ok(()) => {
                        this.board_jira.token_saved = Some(false);
                        this.board_jira.message = None;
                    }
                    Err(err) => this.board_jira.message = Some((false, format!("{err:#}"))),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Looks up once, in the background, whether a token is saved.
    fn check_jira_token_saved(&mut self, cx: &mut Context<Self>) {
        if self.board_jira.token_saved.is_some() || self.board_jira.token_checking {
            return;
        }
        self.board_jira.token_checking = true;
        cx.spawn(async move |this, cx| {
            let saved = cx.background_spawn(async move { agentty_bridge::jira::has_token() }).await;
            let _ = this.update(cx, |this, cx| {
                this.board_jira.token_checking = false;
                // A save or delete that finished meanwhile knows better.
                this.board_jira.token_saved.get_or_insert(saved);
                cx.notify();
            });
        })
        .detach();
    }

    /// Runs the search once and says how many issues it found.
    fn check_jira(&mut self, cx: &mut Context<Self>) {
        let jira = crate::settings::settings(cx).board.jira.clone();
        self.board_jira.busy = true;
        cx.spawn(async move |this, cx| {
            let found = cx.background_spawn(async move { agentty_bridge::jira::search(&jira.site, &jira.email, &jira.jql) }).await;
            let _ = this.update(cx, |this, cx| {
                this.board_jira.busy = false;
                this.board_jira.message = Some(match found {
                    Ok(issues) => (true, tf(cx, "board.jira_found", &[("n", &issues.len().to_string())])),
                    Err(err) => (false, format!("{err:#}")),
                });
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn render_board_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let prefs = crate::settings::settings(cx).board.clone();
        let set = |change: fn(&mut crate::settings::BoardSettings)| {
            move |_: &ClickEvent, _: &mut Window, cx: &mut gpui::App| update_settings(cx, move |s| change(&mut s.board))
        };
        let agents = div()
            .flex()
            .gap_1()
            .child(chip("board-agent-claude", "Claude Code", prefs.agent == BoardAgent::Claude, set(|b| b.agent = BoardAgent::Claude)))
            .child(chip("board-agent-codex", "Codex", prefs.agent == BoardAgent::Codex, set(|b| b.agent = BoardAgent::Codex)));
        let run_modes = div()
            .flex()
            .gap_1()
            .child(chip(
                "board-run-worktree",
                t(cx, "board.run_worktree"),
                prefs.run_mode == BoardRunMode::Worktree,
                set(|b| b.run_mode = BoardRunMode::Worktree),
            ))
            .child(chip(
                "board-run-folder",
                t(cx, "board.run_folder"),
                prefs.run_mode == BoardRunMode::ProjectFolder,
                set(|b| b.run_mode = BoardRunMode::ProjectFolder),
            ));
        let splits = div()
            .flex()
            .gap_1()
            .child(chip(
                "board-split-auto",
                t(cx, "board.split_auto"),
                prefs.split == BoardSplit::Auto,
                set(|b| b.split = BoardSplit::Auto),
            ))
            .child(chip("board-split-ask", t(cx, "board.split_ask"), prefs.split == BoardSplit::Ask, set(|b| b.split = BoardSplit::Ask)))
            .child(chip("board-split-off", t(cx, "board.split_off"), prefs.split == BoardSplit::Off, set(|b| b.split = BoardSplit::Off)));
        let cleanups = div()
            .flex()
            .gap_1()
            .child(chip(
                "board-cleanup-ask",
                t(cx, "board.cleanup_ask"),
                prefs.cleanup == BoardCleanup::Ask,
                set(|b| b.cleanup = BoardCleanup::Ask),
            ))
            .child(chip(
                "board-cleanup-always",
                t(cx, "board.cleanup_always"),
                prefs.cleanup == BoardCleanup::Always,
                set(|b| b.cleanup = BoardCleanup::Always),
            ))
            .child(chip(
                "board-cleanup-never",
                t(cx, "board.cleanup_never"),
                prefs.cleanup == BoardCleanup::Never,
                set(|b| b.cleanup = BoardCleanup::Never),
            ));

        let mut page = div().flex().flex_col().child(section(t(cx, "settings.group.basics")).child(row_with_hint(
            t(cx, "board.enable"),
            t(cx, "board.enable_hint"),
            toggle("board-enabled", prefs.enabled, |s| s.board.enabled = !s.board.enabled, cx),
        )));
        if !prefs.enabled {
            return page;
        }
        page = page.child(
            section(t(cx, "board.how"))
                .child(row_with_hint(t(cx, "board.default_agent"), t(cx, "board.default_agent_hint"), agents))
                .child(row_with_hint(
                    t(cx, "board.run_mode"),
                    t(cx, if prefs.run_mode == BoardRunMode::Worktree { "board.run_worktree_hint" } else { "board.run_folder_hint" }),
                    run_modes,
                ))
                .child(row_with_hint(
                    t(cx, "board.split"),
                    t(
                        cx,
                        match prefs.split {
                            BoardSplit::Auto => "board.split_auto_hint",
                            BoardSplit::Ask => "board.split_ask_hint",
                            BoardSplit::Off => "board.split_off_hint",
                        },
                    ),
                    splits,
                ))
                .child(row_with_hint(
                    t(cx, "board.cleanup"),
                    t(
                        cx,
                        match prefs.cleanup {
                            BoardCleanup::Ask => "board.cleanup_ask_hint",
                            BoardCleanup::Always => "board.cleanup_always_hint",
                            BoardCleanup::Never => "board.cleanup_never_hint",
                        },
                    ),
                    cleanups,
                )),
        );

        let mut jira = section("Jira").child(row_with_hint(
            t(cx, "board.jira_enable"),
            t(cx, "board.jira_enable_hint"),
            toggle("board-jira", prefs.jira.enabled, |s| s.board.jira.enabled = !s.board.jira.enabled, cx),
        ));
        if prefs.jira.enabled {
            self.check_jira_token_saved(cx);
            let (site, email, jql, token) = {
                let fields = self.jira_fields(window, cx);
                (fields.site.clone(), fields.email.clone(), fields.jql.clone(), fields.token.clone())
            };
            let labelled = |label: &'static str, control: gpui::AnyElement| {
                div().flex().flex_col().gap_1().child(div().t_small().text_color(hex(Chrome::MUTED)).child(label)).child(control)
            };
            let saved = self.board_jira.token_saved == Some(true);
            let busy = self.board_jira.busy;
            let token_row = div()
                .flex()
                .items_center()
                .gap_2()
                .child(field_box(&token))
                .child(action_button(
                    "board-jira-token-save",
                    t(cx, "board.save"),
                    cx.listener(|this, _: &ClickEvent, _, cx| this.save_jira_token(cx)),
                ))
                .when(saved, |d| {
                    d.child(action_button(
                        "board-jira-token-delete",
                        t(cx, "board.jira_token_delete"),
                        cx.listener(|this, _: &ClickEvent, _, cx| this.delete_jira_token(cx)),
                    ))
                });
            jira = jira
                .child(labelled(t(cx, "board.jira_site"), field_box(&site).into_any_element()))
                .child(labelled(t(cx, "board.jira_email"), field_box(&email).into_any_element()))
                .child(labelled(
                    if saved { t(cx, "board.jira_token_is_saved") } else { t(cx, "board.jira_token") },
                    token_row.into_any_element(),
                ))
                .child(labelled(t(cx, "board.jira_jql"), field_box(&jql).into_any_element()))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(action_button(
                            "board-jira-check",
                            t(cx, "board.jira_check"),
                            cx.listener(|this, _: &ClickEvent, _, cx| {
                                if !this.board_jira.busy {
                                    this.check_jira(cx);
                                }
                            }),
                        ))
                        .when(busy, |d| d.child(crate::ui::spinner(crate::ui::IconSize::INLINE, hex(Chrome::MUTED))))
                        .when_some(self.board_jira.message.clone(), |d, (ok, text)| {
                            d.child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .t_small()
                                    .text_color(hex(if ok { Chrome::SUCCESS } else { Chrome::ERROR }))
                                    .child(text),
                            )
                        }),
                )
                .child(div().w(px(1.)).h(px(1.)))
                .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "board.jira_token_how")));
        }
        page.child(jira)
    }
}
