//! Settings → Integrations: the outside services Agentty works with, one card each with whether it
//! is connected. A card opens to the setup it needs: GitHub through the `gh` CLI (sync, cloning,
//! pull requests), Slack, Discord and Telegram for notifications, Jira for the development board.

use super::settings_agents::{logo_tile, pill};
use super::settings_page::{note_for_search, section_with_hint};
use super::Workbench;
use crate::i18n::{t, tf};
use crate::theme::{hex, Chrome};
use crate::ui::{action_button, action_button_with_icon, icon, IconSize, TypeScale};
use agentty_bridge::clone::GhStatus;
use agentty_bridge::notify::Channel;
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, Div, SharedString, Window};

const INTEGRATIONS: [&str; 5] = ["github", "slack", "discord", "telegram", "jira"];
const GH_INSTALL_URL: &str = "https://cli.github.com";

/// The id of an integration card from the debug driver's argument.
pub(super) fn integration_id(argument: &str) -> Option<&'static str> {
    INTEGRATIONS.into_iter().find(|id| *id == argument.trim())
}

/// Where a service stands, as its card's pill says it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Checking,
    Connected,
    NotSetUp,
    NotInstalled,
}

impl State {
    fn pill(self, cx: &gpui::App) -> Div {
        match self {
            State::Checking => pill(t(cx, "integrations.checking"), Chrome::MUTED),
            State::Connected => pill(t(cx, "integrations.connected"), Chrome::SUCCESS),
            State::NotSetUp => pill(t(cx, "integrations.not_set_up"), Chrome::WARNING),
            State::NotInstalled => pill(t(cx, "integrations.not_installed"), Chrome::WARNING),
        }
    }
}

fn channel_id(channel: Channel) -> &'static str {
    match channel {
        Channel::Slack => "slack",
        Channel::Discord => "discord",
        Channel::Telegram => "telegram",
    }
}

/// What a card's header shows: its mark, name, what Agentty uses the service for, and its state.
struct CardHead {
    id: &'static str,
    glyph: &'static str,
    name: &'static str,
    about: String,
    state: State,
}

impl Workbench {
    fn check_github(&mut self, cx: &mut Context<Self>) {
        if self.settings_ui.github_checking {
            return;
        }
        self.settings_ui.github_checking = true;
        let task = cx.background_spawn(async { agentty_bridge::clone::gh_status() });
        cx.spawn(async move |this, cx| {
            let status = task.await;
            let _ = this.update(cx, |this, cx| {
                this.settings_ui.github_checking = false;
                this.settings_ui.github = Some(status);
                cx.notify();
            });
        })
        .detach();
    }

    /// One service: its mark, name and what Agentty uses it for, whether it is connected, and —
    /// opened — `body`, how to connect it.
    fn integration_card(
        &mut self,
        head: CardHead,
        body: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) -> AnyElement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let CardHead { id, glyph, name, about, state } = head;
        note_for_search(name, &about);
        let open = self.settings_ui.integrations_open.contains(id) || super::settings_page::collecting();
        let header = div()
            .id(SharedString::from(format!("integration-{id}")))
            .px_4()
            .py_3()
            .flex()
            .items_center()
            .gap_3()
            .cursor_pointer()
            .hover(|s| s.bg(hex(Chrome::HOVER)))
            .rounded_lg()
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                if !this.settings_ui.integrations_open.remove(id) {
                    this.settings_ui.integrations_open.insert(id);
                }
                this.settings_ui.reindex();
                cx.notify();
            }))
            .child(logo_tile(icon(glyph, 17., hex(Chrome::BRIGHT)), false))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(div().t_body().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(name))
                    .child(div().t_small().text_color(hex(Chrome::MUTED)).child(about)),
            )
            .child(state.pill(cx))
            .child(icon(if open { "chevron-up" } else { "chevron-down" }, IconSize::INLINE, hex(Chrome::MUTED)));
        let mut card = div()
            .flex()
            .flex_col()
            .rounded_lg()
            .border_1()
            .border_color(hex(Chrome::BORDER))
            .bg(hex(Chrome::EDITOR))
            .child(super::settings_page::highlighted(name, header));
        if open {
            card = card.child(
                div()
                    .mx_4()
                    .mb_4()
                    .pt_3()
                    .border_t_1()
                    .border_color(hex(Chrome::BORDER))
                    .child(super::settings_page::in_card(id, || body(self, window, cx))),
            );
        }
        card
    }

    pub(super) fn render_integrations(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        self.look_up_chat_secrets(cx);
        if self.settings_ui.github.is_none() {
            self.check_github(cx);
        }

        let github_state = match &self.settings_ui.github {
            None => State::Checking,
            Some(GhStatus::Missing) => State::NotInstalled,
            Some(GhStatus::SignedOut) => State::NotSetUp,
            Some(GhStatus::SignedIn(_)) => State::Connected,
        };
        let github = self.integration_card(
            CardHead {
                id: "github",
                glyph: "git-pull-request",
                name: "GitHub",
                about: t(cx, "integrations.github_about").to_string(),
                state: github_state,
            },
            |this, _, cx| this.render_github_setup(cx),
            window,
            cx,
        );
        let code = section_with_hint(t(cx, "integrations.code"), t(cx, "integrations.code_hint")).child(github);

        let mut chats = section_with_hint(t(cx, "integrations.chat"), t(cx, "integrations.chat_hint"));
        for channel in Channel::ALL {
            let state = match self.chat_configured(channel, cx) {
                None => State::Checking,
                Some(true) => State::Connected,
                Some(false) => State::NotSetUp,
            };
            let (glyph, about) = match channel {
                Channel::Slack => ("message-square", t(cx, "integrations.slack_about")),
                Channel::Discord => ("message-square-code", t(cx, "integrations.discord_about")),
                Channel::Telegram => ("send", t(cx, "integrations.telegram_about")),
            };
            let card = self.integration_card(
                CardHead { id: channel_id(channel), glyph, name: channel.label(), about: about.to_string(), state },
                move |this, window, cx| this.render_chat_setup(channel, window, cx).into_any_element(),
                window,
                cx,
            );
            chats = chats.child(card);
        }

        let jira_state = match self.jira_connected(cx) {
            None => State::Checking,
            Some(true) => State::Connected,
            Some(false) => State::NotSetUp,
        };
        let jira = self.integration_card(
            CardHead {
                id: "jira",
                glyph: "square-kanban",
                name: "Jira",
                about: t(cx, "integrations.jira_about").to_string(),
                state: jira_state,
            },
            |this, window, cx| this.render_jira_setup(window, cx).into_any_element(),
            window,
            cx,
        );
        let issues = section_with_hint(t(cx, "integrations.issues"), t(cx, "integrations.issues_hint")).child(jira);
        div().flex().flex_col().child(code).child(chats).child(issues)
    }

    fn render_github_setup(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let checking = self.settings_ui.github_checking;
        let (text, signed_in) = match &self.settings_ui.github {
            Some(GhStatus::SignedIn(Some(login))) => (tf(cx, "integrations.github_signed_in_as", &[("login", login)]), true),
            Some(GhStatus::SignedIn(None)) => (t(cx, "integrations.github_signed_in").to_string(), true),
            Some(GhStatus::SignedOut) => (t(cx, "integrations.github_signed_out").to_string(), false),
            Some(GhStatus::Missing) => (t(cx, "integrations.github_missing").to_string(), false),
            None => (t(cx, "integrations.checking").to_string(), false),
        };
        let missing = self.settings_ui.github == Some(GhStatus::Missing);
        let mut buttons = div().flex().items_center().gap_2();
        if missing {
            buttons = buttons.child(action_button_with_icon(
                "github-install",
                "external-link",
                t(cx, "integrations.github_install"),
                |_: &ClickEvent, _, cx| cx.open_url(GH_INSTALL_URL),
            ));
        } else if !signed_in && self.settings_ui.github.is_some() {
            buttons = buttons.child(action_button_with_icon(
                "github-sign-in",
                "square-terminal",
                t(cx, "integrations.github_sign_in"),
                cx.listener(|this, _: &ClickEvent, window, cx| {
                    let choice = crate::launch::LaunchChoice::Command { title: "gh auth login".into(), command: "gh auth login".into() };
                    this.request_launch(choice, super::LaunchTarget::NewTab, window, cx);
                }),
            ));
        }
        buttons = buttons
            .child(action_button(
                "github-check",
                t(cx, "integrations.check_again"),
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.settings_ui.github = None;
                    this.check_github(cx);
                    cx.notify();
                }),
            ))
            .when(checking, |d| d.child(crate::ui::spinner(IconSize::INLINE, hex(Chrome::MUTED))));
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(div().t_small().text_color(hex(if signed_in { Chrome::SUCCESS } else { Chrome::MUTED })).child(text))
            .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "integrations.github_uses")))
            .child(buttons)
            .child(div().w(px(1.)).h(px(1.)))
            .into_any_element()
    }
}
