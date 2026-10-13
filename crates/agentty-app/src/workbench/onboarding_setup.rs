//! The setup steps of the first-run onboarding, on one large card: the agents found on this Mac
//! (and whether they ask before acting), the look (language, terminal theme, font), how to work
//! (links, working trees, servers, idea mode) and notifications. A progress bar and "n / 4" on top;
//! Skip at the bottom left, Back and Continue (⌘↩) at the right. The last step's Get started opens
//! the follow-along tour (`onboarding`).

use super::onboarding::Step;
use super::settings_page::switch;
use super::Workbench;
use crate::i18n::{t, tf};
use crate::settings::{settings, update_settings, Appearance, Language, LinkOpener, Settings, SettingsStore};
use crate::theme::{hex, hex_alpha, Chrome, TerminalTheme};
use crate::ui::{chip, icon, IconSize, TypeScale};
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, SharedString, Window};
use std::sync::atomic::{AtomicU64, Ordering};

/// The steps on the card, in order.
const STEPS: [Step; 5] = [Step::Appearance, Step::Agents, Step::Look, Step::Work, Step::Notify];

/// The card's own surfaces: a shade darker than the panels, as a sheet laid over the app.
const CARD: u32 = 0x19191b;
const CARD_BORDER: u32 = 0x2c2c30;
const TILE: u32 = 0x1f1f22;
/// The primary button: light on the dark card.
const PRIMARY: u32 = 0xebebee;
const PRIMARY_INK: u32 = 0x161618;

const THEMES_PER_ROW: usize = 4;
const AGENTS_PER_ROW: usize = 3;
/// Each card of a row as a share of it: the rest is the gaps (`justify_between`). Fixed shares,
/// not `flex_1`: growing cells end up sized by what they hold, and a short last row then
/// widens its cards.
const AGENT_WIDTH: f32 = 0.325;
const THEME_WIDTH: f32 = 0.24;

/// When the notification step last asked macOS about its permission (it polls while shown).
static LAST_PERMISSION_CHECK: AtomicU64 = AtomicU64::new(0);

impl Workbench {
    pub(super) fn render_setup(&self, step: Step, cx: &mut Context<Self>) -> AnyElement {
        let index = STEPS.iter().position(|s| *s == step).unwrap_or(0);
        let last = index + 1 == STEPS.len();
        let body = match step {
            Step::Appearance => self.setup_appearance(cx),
            Step::Agents => self.setup_agents(cx),
            Step::Look => self.setup_look(cx),
            Step::Work => self.setup_work(cx),
            _ => self.setup_notify(cx),
        };

        let progress = div()
            .flex()
            .items_center()
            .gap_1p5()
            .children(STEPS.iter().enumerate().map(|(i, _)| {
                div().h(px(3.)).rounded_full().w(px(if i == index { 46. } else { 26. })).bg(if i == index {
                    hex(Chrome::BRIGHT)
                } else if i < index {
                    hex_alpha(Chrome::BRIGHT, 0.45)
                } else {
                    hex_alpha(Chrome::BRIGHT, 0.18)
                })
            }))
            .child(div().ml_3().t_small().text_color(hex(Chrome::MUTED)).child(format!("{} / {}", index + 1, STEPS.len())));

        let header = div()
            .flex()
            .items_center()
            .gap_2p5()
            .child(gpui::img("brand/logo.png").size(px(30.)))
            .child(div().t_large().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child("Agentty"));

        let mut right = div().flex().items_center().gap_2();
        if index > 0 {
            right =
                right.child(secondary_button("onboarding-back", format!("‹  {}", t(cx, "onboarding.back")), cx, move |this, _, cx| {
                    this.go_to(STEPS[index - 1], cx)
                }));
        }
        let primary_label = if last { t(cx, "onboarding.start") } else { t(cx, "onboarding.continue") };
        right = right.child(primary_button("onboarding-continue", primary_label.to_string(), cx, |this, _, cx| this.setup_next(cx)));

        let footer = div()
            .flex()
            .items_center()
            .justify_between()
            .pt_5()
            .border_t_1()
            .border_color(hex(CARD_BORDER))
            .child(
                div()
                    .id("onboarding-skip")
                    .px_3()
                    .py_1p5()
                    .rounded_md()
                    .cursor_pointer()
                    .t_body()
                    .text_color(hex(Chrome::MUTED))
                    .hover(|s| s.text_color(hex(Chrome::BRIGHT)))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.close_onboarding(cx)))
                    .child(t(cx, "onboarding.skip")),
            )
            .child(right);

        div()
            .id("onboarding-overlay")
            .absolute()
            .inset_0()
            .p(px(32.))
            .flex()
            .justify_center()
            .bg(hex_alpha(0x000000, 0.6))
            .occlude()
            .child(
                div()
                    .size_full()
                    .max_w(px(1240.))
                    .px(px(40.))
                    .pt(px(36.))
                    .pb(px(28.))
                    .flex()
                    .flex_col()
                    .gap_6()
                    .rounded_xl()
                    .bg(hex(CARD))
                    .border_1()
                    .border_color(hex(CARD_BORDER))
                    .shadow_lg()
                    .child(header)
                    .child(progress)
                    .child(div().id("onboarding-body").flex_1().min_h_0().overflow_y_scroll().child(body))
                    .child(footer),
            )
            .into_any_element()
    }

    /// ⌘↩ (Ctrl+Enter elsewhere) is Continue while a setup step is shown.
    pub(super) fn onboarding_key(&mut self, event: &gpui::KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) -> bool {
        let shown = self.onboarding.as_ref().is_some_and(|o| o.step != Step::Tour);
        let keys = &event.keystroke;
        if shown && keys.key == "enter" && keys.modifiers.secondary() && !keys.modifiers.shift {
            self.setup_next(cx);
            return true;
        }
        false
    }

    fn setup_next(&mut self, cx: &mut Context<Self>) {
        let Some(step) = self.onboarding.as_ref().map(|o| o.step) else { return };
        match STEPS.iter().position(|s| *s == step) {
            Some(i) if i + 1 < STEPS.len() => self.go_to(STEPS[i + 1], cx),
            // "Get started" goes on to the follow-along tour.
            _ => self.go_to(Step::Tour, cx),
        }
    }

    // -- the steps ---------------------------------------------------------------------------

    fn setup_agents(&self, cx: &mut Context<Self>) -> gpui::Div {
        let prefs = settings(cx).clone();
        let installed = self.installed.as_ref();
        // Claude Code and Codex always (found or not: they are what Agentty is built around), then
        // every other agent CLI found.
        let mut agents: Vec<(&'static str, &'static str, &'static str)> =
            vec![("claude", "Claude Code", "claude"), ("codex", "Codex", "codex")];
        if let Some(installed) = installed {
            agents.extend(installed.other_agents().map(|a| (a.id, a.name, a.binary)));
        }
        let found = installed.map(|i| agents.iter().filter(|(_, _, binary)| i.has(binary)).count());

        let mut grid = div().flex().flex_col().gap_3();
        for (row_index, chunk) in agents.chunks(AGENTS_PER_ROW).enumerate() {
            let mut row = div().w_full().flex().justify_between();
            for (offset, (id, name, binary)) in chunk.iter().enumerate() {
                let has = installed.map(|i| i.has(binary));
                let status = match has {
                    None => t(cx, "onboarding.agent_looking").to_string(),
                    Some(true) => installed.and_then(|i| i.version(binary)).map(|v| format!("v{v}")).unwrap_or_default(),
                    Some(false) => t(cx, "onboarding.agent_missing").to_string(),
                };
                row = row.child(
                    div()
                        .id(("onboarding-agent", row_index * AGENTS_PER_ROW + offset))
                        .w(gpui::relative(AGENT_WIDTH))
                        .flex_none()
                        .min_w_0()
                        .h(px(58.))
                        .px_4()
                        .flex()
                        .items_center()
                        .gap_3()
                        .rounded_lg()
                        .bg(hex(TILE))
                        .border_1()
                        .border_color(hex(if has == Some(true) { 0x3a3a40 } else { CARD_BORDER }))
                        .when(has == Some(false), |d| d.opacity(0.55))
                        .child(crate::brand::tile(id, 30.))
                        .child(div().flex_1().min_w_0().truncate().t_large().text_color(hex(Chrome::BRIGHT)).child(*name))
                        .child(div().flex_shrink_0().t_small().text_color(hex(Chrome::MUTED)).child(status))
                        .when(has == Some(true), |d| d.child(icon("circle-check", IconSize::BUTTON, hex(Chrome::SUCCESS)))),
                );
            }
            // A short last row keeps its cards the width of the others.
            for _ in chunk.len()..AGENTS_PER_ROW {
                row = row.child(div().w(gpui::relative(AGENT_WIDTH)).flex_none());
            }
            grid = grid.child(row);
        }

        let found_label = match found {
            Some(n) => tf(cx, "onboarding.agents_found", &[("n", &n.to_string())]),
            None => t(cx, "onboarding.agent_looking").to_string(),
        };
        div()
            .flex()
            .flex_col()
            .child(title(t(cx, "onboarding.agents_title")))
            .child(subtitle(t(cx, "onboarding.agents_body")))
            .child(div().mt(px(36.)).mb_3().t_body().text_color(hex(Chrome::MUTED)).child(found_label))
            .child(grid)
            .child(div().mt_6().border_t_1().border_color(hex(CARD_BORDER)))
            .child(toggle_row(
                "onboarding-bypass",
                t(cx, "onboarding.bypass"),
                t(cx, "settings.always_bypass_hint"),
                prefs.always_bypass,
                |s| s.always_bypass = !s.always_bypass,
            ))
    }

    /// The first step: light or dark (or the system's), as three large previews.
    fn setup_appearance(&self, cx: &mut Context<Self>) -> gpui::Div {
        let current = settings(cx).appearance;
        div()
            .flex()
            .flex_col()
            .child(eyebrow(t(cx, "onboarding.eyebrow")))
            .child(title(t(cx, "onboarding.appearance_title")))
            .child(subtitle(t(cx, "onboarding.appearance_body")))
            .child(div().mt(px(36.)).w_full().flex().justify_between().children(Appearance::ALL.into_iter().enumerate().map(
                |(i, appearance)| {
                    appearance_card("onboarding-appearance", i, appearance, current == appearance, 180., cx)
                        .w(gpui::relative(AGENT_WIDTH))
                        .flex_none()
                },
            )))
    }

    fn setup_look(&self, cx: &mut Context<Self>) -> gpui::Div {
        let prefs = settings(cx).clone();
        let themes: Vec<TerminalTheme> = cx.global::<SettingsStore>().themes.iter().filter(|t| !t.imported).cloned().collect();
        let mut grid = div().flex().flex_col().gap_3();
        for (row_index, chunk) in themes.chunks(THEMES_PER_ROW).enumerate() {
            let mut row = div().w_full().flex().justify_between();
            for (offset, theme) in chunk.iter().enumerate() {
                row = row.child(theme_preview(row_index * THEMES_PER_ROW + offset, theme, theme.name == prefs.theme, cx));
            }
            for _ in chunk.len()..THEMES_PER_ROW {
                row = row.child(div().w(gpui::relative(THEME_WIDTH)).flex_none());
            }
            grid = grid.child(row);
        }

        let mut languages = div().flex().flex_wrap().gap_1();
        for language in Language::ALL {
            languages = languages.child(chip(
                SharedString::from(format!("onboarding-lang-{language:?}")),
                if language == Language::System { t(cx, "settings.language_system") } else { language.native_name() },
                prefs.language == language,
                cx.listener(move |_, _: &ClickEvent, _, cx| {
                    update_settings(cx, move |s| s.language = language);
                    crate::set_app_menus(cx);
                }),
            ));
        }
        // The default font (which follows the language), the bundled one and, on macOS when it is
        // installed, D2Coding; every other font is in Settings → Terminal Style.
        let default_font = crate::settings::default_terminal_font(cx);
        let font_chip = |id: &'static str, label: String, value: &'static str, cx: &mut Context<Self>| {
            chip(
                id,
                label,
                prefs.font_family == value,
                cx.listener(move |_, _: &ClickEvent, _, cx| update_settings(cx, move |s| s.font_family = value.to_string())),
            )
        };
        let fonts = div()
            .flex()
            .flex_wrap()
            .gap_1()
            .child(font_chip("onboarding-font-default", tf(cx, "settings.font_default", &[("font", default_font)]), "", cx))
            .when(default_font != crate::settings::BUNDLED_FONT, |d| {
                d.child(font_chip("onboarding-font-bundled", crate::settings::BUNDLED_FONT.to_string(), crate::settings::BUNDLED_FONT, cx))
            })
            .when(
                cfg!(target_os = "macos") && default_font != crate::settings::KOREAN_FONT && crate::settings::korean_font_installed(cx),
                |d| {
                    d.child(font_chip("onboarding-font-korean", crate::settings::KOREAN_FONT.to_string(), crate::settings::KOREAN_FONT, cx))
                },
            );
        // Only on a fresh install: showing the onboarding again later is not a setup step.
        let font_notice =
            (!prefs.onboarding_done).then(|| super::settings_page::korean_font_notice("onboarding-korean-font", cx)).flatten();

        let labeled = |label: &str, control: gpui::Div| {
            div()
                .flex()
                .items_center()
                .gap_4()
                .py_1()
                .child(div().w(px(120.)).flex_shrink_0().t_body().text_color(hex(Chrome::MUTED)).child(label.to_string()))
                .child(control)
        };
        div()
            .flex()
            .flex_col()
            .child(title(t(cx, "onboarding.look_title")))
            .child(subtitle(t(cx, "onboarding.look_body")))
            .child(div().mt(px(36.)).child(grid))
            .child(
                div()
                    .mt_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .t_small()
                    .text_color(hex(Chrome::MUTED))
                    .child(icon("settings", IconSize::INLINE, hex(Chrome::MUTED)))
                    .child(t(cx, "onboarding.look_more")),
            )
            .child(
                div()
                    .mt_6()
                    .pt_4()
                    .border_t_1()
                    .border_color(hex(CARD_BORDER))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(labeled(t(cx, "settings.language"), languages))
                    .child(labeled(t(cx, "onboarding.opt_font"), fonts))
                    .children(font_notice),
            )
    }

    fn setup_work(&self, cx: &mut Context<Self>) -> gpui::Div {
        let prefs = settings(cx).clone();
        let opener = |id: &'static str, label: &'static str, value: LinkOpener, cx: &mut Context<Self>| {
            chip(
                id,
                t(cx, label),
                prefs.link_opener == value,
                cx.listener(move |_, _: &ClickEvent, _, cx| update_settings(cx, move |s| s.link_opener = value)),
            )
        };
        let openers = div()
            .flex()
            .flex_shrink_0()
            .gap_1()
            .when(crate::platform::HAS_WEBVIEW, |d| d.child(opener("onboarding-link-inapp", "settings.link_inapp", LinkOpener::InApp, cx)))
            .child(opener("onboarding-link-external", "settings.link_external", LinkOpener::External, cx));
        let in_app = prefs.link_opener == LinkOpener::InApp && crate::platform::HAS_WEBVIEW;
        div().flex().flex_col().child(title(t(cx, "onboarding.work_title"))).child(subtitle(t(cx, "onboarding.basics_body"))).child(
            div()
                .mt(px(28.))
                .flex()
                .flex_col()
                .child(feature_row("globe", Chrome::BLUE, t(cx, "onboarding.opt_links"), t(cx, "onboarding.opt_links_body"), openers))
                .when(in_app, |d| {
                    d.child(feature_row(
                        "zap",
                        Chrome::GREEN,
                        t(cx, "onboarding.opt_auto_open"),
                        t(cx, "onboarding.opt_auto_open_body"),
                        setting_switch("onboarding-auto-open", prefs.browser.auto_open_servers, |s| {
                            s.browser.auto_open_servers = !s.browser.auto_open_servers
                        }),
                    ))
                })
                .child(feature_row(
                    "git-fork",
                    Chrome::PURPLE,
                    t(cx, "onboarding.opt_worktree"),
                    t(cx, "onboarding.opt_worktree_body"),
                    setting_switch("onboarding-worktree", prefs.auto_worktree, |s| s.auto_worktree = !s.auto_worktree),
                ))
                .child(feature_row(
                    "power",
                    Chrome::ERROR,
                    t(cx, "onboarding.opt_stop_servers"),
                    t(cx, "onboarding.opt_stop_servers_body"),
                    setting_switch("onboarding-stop-servers", prefs.stop_servers_on_close, |s| {
                        s.stop_servers_on_close = !s.stop_servers_on_close
                    }),
                ))
                .child(feature_row(
                    "lightbulb",
                    Chrome::WARNING,
                    t(cx, "onboarding.opt_idea"),
                    t(cx, "onboarding.opt_idea_body"),
                    setting_switch("onboarding-idea", prefs.idea_mode, |s| s.idea_mode = !s.idea_mode),
                )),
        )
    }

    fn setup_notify(&self, cx: &mut Context<Self>) -> gpui::Div {
        use crate::notifications::{self, Permission};
        let prefs = settings(cx).clone();
        // Whether macOS lets Agentty notify can change in System Settings while this step is
        // open: read it again about once a second while it is shown.
        let now = crate::ui::now_ms();
        if now.saturating_sub(LAST_PERMISSION_CHECK.swap(now, Ordering::Relaxed)) >= 900 {
            notifications::check_permission();
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(std::time::Duration::from_secs(1)).await;
                let _ = this.update(cx, |_, cx| cx.notify());
            })
            .detach();
        }
        let permission = notifications::permission();
        let (state, state_color) = match permission {
            Permission::Allowed => (t(cx, "onboarding.notify_allowed"), Chrome::SUCCESS),
            Permission::Denied => (t(cx, "onboarding.notify_denied"), Chrome::WARNING),
            Permission::NotAsked => (t(cx, "onboarding.notify_not_asked"), Chrome::MUTED),
            Permission::Unknown => (t(cx, "onboarding.notify_unknown"), Chrome::MUTED),
        };
        let action = match permission {
            Permission::NotAsked => Some(
                secondary_button("onboarding-notify-ask", t(cx, "onboarding.notify_ask").to_string(), cx, |_, _, _| {
                    notifications::ask_permission()
                })
                .into_any_element(),
            ),
            Permission::Allowed => None,
            _ if !notifications::SETTINGS_URL.is_empty() => Some(
                secondary_button("onboarding-notify-settings", t(cx, "onboarding.notify_open_settings").to_string(), cx, |_, _, cx| {
                    cx.open_url(notifications::SETTINGS_URL)
                })
                .into_any_element(),
            ),
            _ => None,
        };
        let permission_card = div()
            .flex()
            .items_center()
            .gap_3()
            .px_5()
            .py_4()
            .rounded_lg()
            .bg(hex(TILE))
            .border_1()
            .border_color(hex(CARD_BORDER))
            .child(icon("bell", IconSize::BUTTON, hex(Chrome::FOREGROUND)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(div().t_large().text_color(hex(Chrome::BRIGHT)).child(t(cx, "onboarding.notify_permission")))
                    .child(div().t_small().text_color(hex(state_color)).child(state)),
            )
            .children(action);
        let test = secondary_button("onboarding-notify-test", t(cx, "onboarding.notify_test").to_string(), cx, |_, _, cx| {
            notifications::show(0, "Agentty", t(cx, "onboarding.notify_test_body"))
        });
        div()
            .flex()
            .flex_col()
            .child(title(t(cx, "onboarding.notify_title")))
            .child(subtitle(t(cx, "onboarding.notify_body")))
            .child(div().mt(px(36.)).child(permission_card))
            .child(
                div()
                    .mt_4()
                    .flex()
                    .flex_col()
                    .child(toggle_row(
                        "onboarding-notifications",
                        t(cx, "onboarding.opt_notify"),
                        t(cx, "onboarding.opt_notify_body"),
                        prefs.system_notifications,
                        |s| s.system_notifications = !s.system_notifications,
                    ))
                    .when(prefs.system_notifications, |d| {
                        d.child(toggle_row(
                            "onboarding-notify-requests",
                            t(cx, "settings.notify_answer_requests"),
                            t(cx, "onboarding.notify_requests_body"),
                            prefs.notify_answer_requests,
                            |s| s.notify_answer_requests = !s.notify_answer_requests,
                        ))
                        .child(toggle_row(
                            "onboarding-notify-focused",
                            t(cx, "settings.notify_when_focused"),
                            t(cx, "settings.notify_when_focused_hint"),
                            prefs.notify_when_focused,
                            |s| s.notify_when_focused = !s.notify_when_focused,
                        ))
                    }),
            )
            .child(div().mt_4().flex().child(test))
    }
}

// -- pieces -----------------------------------------------------------------------------------

fn eyebrow(text: &str) -> impl IntoElement {
    div()
        .mb_2()
        .t_small()
        .text_color(hex(Chrome::MUTED))
        .child(text.to_uppercase().chars().flat_map(|c| [c, '\u{2009}']).collect::<String>())
}

fn title(text: &str) -> impl IntoElement {
    div()
        .text_size(px(32.))
        .line_height(px(42.))
        .font_weight(crate::theme::EMPHASIS)
        .text_color(hex(Chrome::BRIGHT))
        .child(text.to_string())
}

fn subtitle(text: &str) -> impl IntoElement {
    div().mt_2().t_large().text_color(hex(Chrome::MUTED)).child(text.to_string())
}

/// A setting with its name and one line on what it does, a switch at the right.
fn toggle_row(id: &'static str, name: &str, body: &str, on: bool, change: fn(&mut Settings)) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_6()
        .py_4()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_0p5()
                .child(div().t_large().text_color(hex(Chrome::BRIGHT)).child(name.to_string()))
                .child(div().t_small().text_color(hex(Chrome::MUTED)).child(body.to_string())),
        )
        .child(setting_switch(id, on, change))
}

/// A feature with an icon, its name, one line, and its control.
fn feature_row(glyph: &'static str, color: u32, name: &str, body: &str, control: impl IntoElement) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_4()
        .py_3()
        .border_b_1()
        .border_color(hex(CARD_BORDER))
        .child(crate::brand::tinted_tile(color, 34.).child(icon(glyph, IconSize::BUTTON, hex(color))))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_0p5()
                .child(div().t_large().text_color(hex(Chrome::BRIGHT)).child(name.to_string()))
                .child(div().t_small().text_color(hex(Chrome::MUTED)).child(body.to_string())),
        )
        .child(control)
}

fn setting_switch(id: &'static str, on: bool, change: fn(&mut Settings)) -> impl IntoElement {
    switch(id, on, move |_, _, cx| update_settings(cx, change)).flex_shrink_0()
}

fn primary_button(
    id: &'static str,
    label: String,
    cx: &mut Context<Workbench>,
    on_click: impl Fn(&mut Workbench, &mut Window, &mut Context<Workbench>) + 'static,
) -> impl IntoElement {
    let keys = if cfg!(target_os = "macos") { "⌘ ↩" } else { "Ctrl ↩" };
    div()
        .id(id)
        .h(px(40.))
        .px_5()
        .flex()
        .items_center()
        .gap_3()
        .rounded_lg()
        .cursor_pointer()
        .bg(hex(PRIMARY))
        .text_color(hex(PRIMARY_INK))
        .t_body()
        .font_weight(crate::theme::EMPHASIS)
        .hover(|s| s.opacity(0.9))
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| on_click(this, window, cx)))
        .child(label)
        .child(div().px_1p5().rounded_sm().border_1().border_color(hex_alpha(PRIMARY_INK, 0.35)).t_caption().child(keys))
}

fn secondary_button(
    id: &'static str,
    label: String,
    cx: &mut Context<Workbench>,
    on_click: impl Fn(&mut Workbench, &mut Window, &mut Context<Workbench>) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .flex_shrink_0()
        .h(px(40.))
        .px_4()
        .flex()
        .items_center()
        .rounded_lg()
        .cursor_pointer()
        .border_1()
        .border_color(hex(0x3a3a40))
        .t_body()
        .text_color(hex(Chrome::FOREGROUND))
        .hover(|s| s.bg(hex(Chrome::HOVER)).text_color(hex(Chrome::BRIGHT)))
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| on_click(this, window, cx)))
        .child(label)
}

/// A terminal theme as a small screen of it: a prompt, a command, an agent's line and its colors.
fn theme_preview(index: usize, theme: &TerminalTheme, selected: bool, cx: &mut Context<Workbench>) -> impl IntoElement {
    let name = theme.name.clone();
    let line = |width: f32, color: u32| div().h(px(5.)).w(px(width)).rounded_full().bg(crate::theme::raw(color));
    let screen = div()
        .h(px(78.))
        .p_3()
        .rounded_md()
        .bg(crate::theme::raw(theme.background))
        .border_1()
        .border_color(hex_alpha(Chrome::BRIGHT, 0.08))
        .flex()
        .flex_col()
        .gap_2()
        .child(div().flex().gap_1p5().child(line(34., theme.ansi[2])).child(line(22., theme.ansi[4])).child(line(46., theme.foreground)))
        .child(
            div()
                .flex()
                .gap_1p5()
                .child(div().size(px(5.)).rounded_full().bg(crate::theme::raw(theme.ansi[5])))
                .child(line(96., theme.foreground)),
        )
        .child(line(70., hex_dim(theme.foreground)))
        .child(div().flex().gap_1p5().child(line(18., theme.ansi[1])).child(line(58., theme.ansi[3])))
        .child(div().flex_1())
        .child(div().flex().gap_1().children(theme.ansi[1..7].iter().map(|c| div().size(px(8.)).rounded_sm().bg(crate::theme::raw(*c)))));
    div()
        .id(("onboarding-theme", index))
        .w(gpui::relative(THEME_WIDTH))
        .flex_none()
        .min_w_0()
        .p_2()
        .flex()
        .flex_col()
        .gap_2()
        .rounded_lg()
        .cursor_pointer()
        .bg(hex(if selected { 0x2a2a2e } else { TILE }))
        .border_1()
        .border_color(if selected { hex_alpha(Chrome::BRIGHT, 0.6) } else { hex(CARD_BORDER) })
        .hover(|s| s.border_color(hex_alpha(Chrome::BRIGHT, 0.35)))
        .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
            let name = name.clone();
            update_settings(cx, move |s| s.theme = name);
        }))
        .child(screen)
        .child(
            div()
                .px_1()
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .child(div().min_w_0().truncate().t_body().text_color(hex(Chrome::BRIGHT)).child(theme.name.clone()))
                .when(selected, |d| d.child(icon("circle-check", IconSize::INLINE, hex(Chrome::BRIGHT)))),
        )
}

/// A foreground color halfway to grey: the dimmer second line of the preview.
fn hex_dim(color: u32) -> u32 {
    let mix = |shift: u32| ((((color >> shift) & 0xff) + 0x80) / 2) << shift;
    mix(16) | mix(8) | mix(0)
}

/// Light, dark or the system's, as a small window of Agentty in it: the system's card is half of
/// each. The preview colours are fixed (`raw`): they show the choice, not the chrome around it.
pub(super) fn appearance_card(
    id: &'static str,
    index: usize,
    appearance: Appearance,
    selected: bool,
    preview_height: f32,
    cx: &mut Context<Workbench>,
) -> gpui::Stateful<gpui::Div> {
    // (window, sidebar, line, border) of each appearance.
    const DARK: (u32, u32, u32, u32) = (0x1e1e21, 0x151517, 0x3a3a42, 0x2f2f34);
    const LIGHT: (u32, u32, u32, u32) = (0xffffff, 0xf1f1f4, 0xd9d9e0, 0xdedee3);
    let raw = crate::theme::raw;
    let half = |(window, sidebar, line, border): (u32, u32, u32, u32), with_sidebar: bool, share: f32| {
        let bar = |width: f32| div().h(px(5.)).w(px(width)).rounded_full().bg(raw(line));
        div()
            .w(gpui::relative(share))
            .flex_none()
            .overflow_hidden()
            .h_full()
            .flex()
            .bg(raw(window))
            .when(with_sidebar, |d| {
                d.child(
                    div()
                        .w(px(64.))
                        .h_full()
                        .p_2()
                        .flex()
                        .flex_col()
                        .gap_1p5()
                        .bg(raw(sidebar))
                        .border_r_1()
                        .border_color(raw(border))
                        .child(div().flex().gap_1().child(div().size(px(5.)).rounded_full().bg(raw(0x8b5cf6))).child(bar(38.)))
                        .child(bar(30.))
                        .child(bar(42.)),
                )
            })
            .child(
                div()
                    .flex_1()
                    .p_2()
                    .flex()
                    .flex_col()
                    .gap_1p5()
                    .child(div().flex().gap_1().child(bar(26.)).child(bar(18.)))
                    .child(bar(90.))
                    .child(bar(70.))
                    .child(div().flex_1())
                    .child(div().h(px(7.)).w_full().rounded_full().border_1().border_color(raw(border))),
            )
    };
    let screen = div().h(px(preview_height)).rounded_md().overflow_hidden().border_1().border_color(raw(0x3a3a40)).flex();
    let screen = match appearance {
        Appearance::Dark => screen.child(half(DARK, true, 1.)),
        Appearance::Light => screen.child(half(LIGHT, true, 1.)),
        Appearance::System => screen.child(half(DARK, true, 0.5)).child(half(LIGHT, true, 0.5)),
    };
    let (glyph, body) = match appearance {
        Appearance::System => ("monitor", "appearance.system_body"),
        Appearance::Dark => ("moon", "appearance.dark_body"),
        Appearance::Light => ("sun", "appearance.light_body"),
    };
    div()
        .id((id, index))
        .p_2()
        .flex()
        .flex_col()
        .gap_2()
        .rounded_lg()
        .cursor_pointer()
        .bg(hex(if selected { Chrome::SELECTED } else { TILE }))
        .border_1()
        .border_color(if selected { hex_alpha(Chrome::BRIGHT, 0.6) } else { hex(CARD_BORDER) })
        .hover(|s| s.border_color(hex_alpha(Chrome::BRIGHT, 0.35)))
        .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| update_settings(cx, move |s| s.appearance = appearance)))
        .child(screen)
        .child(
            div()
                .px_1()
                .flex()
                .items_center()
                .gap_2()
                .child(icon(glyph, IconSize::BUTTON, hex(Chrome::FOREGROUND)))
                .child(div().t_large().text_color(hex(Chrome::BRIGHT)).child(t(cx, super::settings_page::appearance_label(appearance))))
                .child(div().flex_1())
                .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, body))),
        )
}
