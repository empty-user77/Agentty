//! Settings → Project → Language servers: which of the file editor's language servers are
//! installed, and the official install command of a missing one, run in a new terminal tab the
//! way Settings → Agents installs agent CLIs. The editor's status bar offers the same for the file
//! in front.

use super::settings_page::{note_for_search, section_card};
use super::Workbench;
use crate::i18n::{t, tf};
use crate::theme::{hex, Chrome};
use crate::ui::{action_button, icon_only, TypeScale};
use agentty_bridge::lsp;
use gpui::{div, prelude::*, ClickEvent, Context, Div, SharedString, Window};

impl Workbench {
    /// Looks the servers up again in the background, and lets the editor try a server it could
    /// not find at once (one may have just been installed).
    pub(super) fn check_language_servers(&mut self, cx: &mut Context<Self>) {
        if self.lsp_checking {
            return;
        }
        self.lsp_checking = true;
        let task = cx.background_spawn(async {
            lsp::retry_now();
            lsp::servers()
        });
        cx.spawn(async move |this, cx| {
            let servers = task.await;
            let _ = this.update(cx, |this, cx| {
                this.lsp_servers = Some(servers);
                this.lsp_checking = false;
                cx.notify();
            });
        })
        .detach();
    }

    /// The editor's "Install" for the server of an LSP language id: its command in a terminal
    /// tab, its page when this machine lacks what the command needs, or (installed after all) a
    /// fresh start.
    pub(super) fn install_language_server(&mut self, language: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = lsp::spec_for(language).map(|spec| spec.name) else { return };
        let task = cx.background_spawn(async move { lsp::servers().into_iter().find(|s| s.name == name) });
        cx.spawn_in(window, async move |this, cx| {
            let Some(server) = task.await else { return };
            let _ = this.update_in(cx, |this, window, cx| {
                match (server.installed, server.install.clone()) {
                    (true, _) => lsp::retry_now(),
                    (false, Some(command)) => this.run_install(server.name, server.program, command, window, cx),
                    (false, None) => cx.open_url(server.site),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn render_language_servers(&mut self, cx: &mut Context<Self>) -> Div {
        // An install whose tab was closed has finished one way or another: look again.
        let installing: Vec<&'static str> =
            self.lsp_servers.iter().flatten().map(|s| s.name).filter(|name| self.install_tab(name).is_some()).collect();
        let finished = self.lsp_installs_seen.iter().any(|name| !installing.contains(name));
        self.lsp_installs_seen = installing.clone();
        let disabled = &crate::settings::settings(cx).lsp_disabled;
        let stale =
            self.lsp_servers.as_ref().is_some_and(|servers| servers.iter().any(|s| s.enabled == disabled.iter().any(|n| n == s.name)));
        if stale {
            agentty_bridge::lsp::set_disabled(disabled);
        }
        if (self.lsp_servers.is_none() || finished || stale) && !self.lsp_checking {
            self.check_language_servers(cx);
        }

        note_for_search(t(cx, "lsp.title"), t(cx, "lsp.hint"));
        let header = div()
            .flex()
            .items_start()
            .gap_3()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(div().t_body().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(t(cx, "lsp.title")))
                    .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "lsp.hint"))),
            )
            .child(div().flex_shrink_0().child(action_button(
                "lsp-recheck",
                t(cx, if self.lsp_checking { "system.checking" } else { "system.recheck" }),
                cx.listener(|this, _: &ClickEvent, _, cx| this.check_language_servers(cx)),
            )));
        let mut card = section_card().child(header);
        let trusted_builds = crate::settings::settings(cx).lsp_trusted_builds.len();
        for server in self.lsp_servers.clone().unwrap_or_default() {
            let (name, program, site) = (server.name, server.program, server.site);
            note_for_search(server.program, server.languages);
            let action = if server.installed {
                // Installed: on or off (a running one stops at once).
                super::settings_page::toggle(
                    name,
                    server.enabled,
                    move |s| {
                        if s.lsp_disabled.iter().any(|n| n == name) {
                            s.lsp_disabled.retain(|n| n != name);
                        } else {
                            s.lsp_disabled.push(name.to_string());
                        }
                    },
                    cx,
                )
                .tooltip(crate::ui::Tooltip::text(t(cx, if server.enabled { "lsp.turn_off" } else { "lsp.turn_on" }), None))
                .into_any_element()
            } else if let Some(command) = server.install.clone() {
                let busy = installing.contains(&name);
                action_button(
                    SharedString::from(format!("lsp-install-{name}")),
                    t(cx, if busy { "system.installing" } else { "system.install" }),
                    cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.run_install(name, program, command.clone(), window, cx);
                        cx.notify();
                    }),
                )
                .into_any_element()
            } else if let Some(needs) = server.needs {
                div().t_small().text_color(hex(Chrome::MUTED)).child(tf(cx, "lsp.needs", &[("tool", needs)])).into_any_element()
            } else {
                // Installed from its own page (the link beside it), with nothing this machine lacks.
                div().into_any_element()
            };
            card = card.child(
                div()
                    .pt_3()
                    .border_t_1()
                    .border_color(hex(Chrome::BORDER))
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().t_body().text_color(hex(Chrome::FOREGROUND)).child(server.languages))
                            .child(
                                div()
                                    .t_small()
                                    .font_family(crate::settings::BUNDLED_FONT)
                                    .text_color(hex(Chrome::MUTED))
                                    .child(server.program),
                            )
                            .when(name == "jdtls" && trusted_builds > 0, |d| {
                                d.child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .t_small()
                                        .text_color(hex(Chrome::MUTED))
                                        .child(tf(cx, "lsp.trusted_builds", &[("n", &trusted_builds.to_string())]))
                                        .child(
                                            div()
                                                .id("lsp-forget-builds")
                                                .cursor_pointer()
                                                .text_color(hex(Chrome::ACCENT))
                                                .hover(|s| s.text_color(hex(Chrome::BRIGHT)))
                                                .child(t(cx, "lsp.forget_builds"))
                                                .on_click(|_, _, cx| {
                                                    crate::settings::update_settings(cx, |s| s.lsp_trusted_builds.clear())
                                                }),
                                        ),
                                )
                            }),
                    )
                    .child(div().flex_shrink_0().child(action))
                    .child(
                        icon_only(SharedString::from(format!("lsp-site-{name}")), "external-link", move |_, _, cx| cx.open_url(site))
                            .tooltip(crate::ui::Tooltip::text(t(cx, "agents.website"), None)),
                    ),
            );
        }
        card
    }
}
