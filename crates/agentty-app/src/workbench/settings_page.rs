//! Settings page: language, folder prompt, terminal theme, font, cursor and misc options.

use super::Workbench;
use crate::i18n::t;
use crate::settings::{
    reload_themes, settings, update_settings, CommandAlias, CursorShapeSetting, HarnessAgent, Language, Settings, SettingsStore,
    BUNDLED_FONT,
};
use crate::shell_integration::{is_valid_command, is_valid_word};
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::{hex, hex_alpha, themes_dir, Chrome, TerminalTheme};
use crate::ui::TypeScale;
use crate::ui::{action_button, chip};
use gpui::{div, prelude::*, px, ClickEvent, Context, Div, Entity, FontWeight, PathPromptOptions, SharedString, Subscription, Window};

/// Input for adding a harness pattern.
pub struct HarnessPatternForm {
    pub input: Entity<TextInput>,
    invalid: bool,
    _subscription: Subscription,
}

/// Inputs for adding a reserved word.
pub struct AliasForm {
    pub keyword: Entity<TextInput>,
    pub expansion: Entity<TextInput>,
    command: String,
    invalid: bool,
    _subscriptions: [Subscription; 2],
}

const THEMES_PER_ROW: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SettingsSection {
    #[default]
    General,
    Project,
    Accounts,
    Appearance,
    Browser,
    Shortcuts,
    /// Helper tools on Windows / Linux (hidden on macOS).
    System,
    About,
}

impl SettingsSection {
    pub const ALL: [SettingsSection; 8] =
        [Self::General, Self::Project, Self::Accounts, Self::Appearance, Self::Browser, Self::Shortcuts, Self::System, Self::About];

    /// Sections shown on this platform (the browser settings need the macOS web view).
    fn visible() -> impl Iterator<Item = SettingsSection> {
        Self::ALL.into_iter().filter(|s| match s {
            Self::Browser => crate::platform::HAS_WEBVIEW,
            _ => true,
        })
    }

    fn label(self) -> &'static str {
        match self {
            Self::General => "settings.general",
            Self::Project => "settings.project",
            Self::Accounts => "settings.accounts",
            Self::Appearance => "settings.appearance",
            Self::Browser => "settings.browser",
            Self::Shortcuts => "settings.shortcuts",
            Self::System => "settings.system",
            Self::About => "settings.about",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::General => "settings",
            Self::Project => "folder-open",
            Self::Accounts => "key-round",
            Self::Appearance => "terminal",
            Self::Browser => "globe",
            Self::Shortcuts => "command",
            Self::System => "wrench",
            Self::About => "sparkles",
        }
    }
}

/// Every default shortcut for the guide: (group key, [(action key, keys)]).
pub const SHORTCUTS: &[(&str, &[(&str, &str)])] = &[
    (
        "shortcuts.group.launch",
        &[
            ("new.terminal", "⌘T"),
            ("new.claude", "⌥⌘C"),
            ("new.codex", "⌥⌘X"),
            ("shortcuts.new_tab_here", "⌘N"),
            ("shortcuts.new_workspace_sidebar", "⌘N"),
            ("new.window", "⇧⌘N"),
            ("split.right", "⌘D"),
            ("split.down", "⇧⌘D"),
        ],
    ),
    (
        "shortcuts.group.navigate",
        &[
            ("shortcuts.next_prev_pane", "⌘]  /  ⌘["),
            ("shortcuts.next_prev_tab", "⇧⌘]  /  ⇧⌘[  ·  ⌃Tab"),
            ("shortcuts.tab_n", "⌃1 … ⌃9"),
            ("shortcuts.workspace_n", "⌘1 … ⌘9"),
            ("shortcuts.next_prev_workspace", "⌥⌘↓  /  ⌥⌘↑"),
            ("shortcuts.jump_unread", "⇧⌘U"),
            ("tooltip.zoom", "⇧⌘↩"),
            ("close.pane", "⌘W"),
            ("close.tab", "⇧⌘W"),
        ],
    ),
    (
        "shortcuts.group.views",
        &[
            ("shortcuts.palette", "⇧⌘P"),
            ("shortcuts.search_sessions", "⇧⌘O"),
            ("panel.workspaces", "⇧⌘E"),
            ("panel.sessions", "⇧⌘S"),
            ("page.git", "⇧⌘G"),
            ("page.flow", "⇧⌘F"),
            ("page.monitoring", "⌥⌘U"),
            ("page.extensions", "⇧⌘X"),
            ("page.settings", "⌘,"),
            ("shortcuts.toggle_sidebar", "⌘B"),
            ("mini.enter", "⌃⌘M"),
            ("shortcuts.browser", "⇧⌘B"),
        ],
    ),
    (
        "shortcuts.group.git",
        &[("shortcuts.git_commit", "⌘↩"), ("shortcuts.git_push", "⌘P"), ("shortcuts.git_fetch", "⇧⌘T"), ("shortcuts.git_refresh", "⌘R")],
    ),
    (
        "shortcuts.group.terminal",
        &[
            ("shortcuts.copy_paste", "⌘C  /  ⌘V"),
            ("shortcuts.select_all", "⌘A"),
            ("shortcuts.clear", "⌘K"),
            ("shortcuts.find", "⌘F"),
            ("shortcuts.zoom", "⌘=  /  ⌘-  /  ⌘0"),
            ("shortcuts.open_link", "⌘ Click"),
        ],
    ),
];

fn render_shortcuts(cx: &mut Context<Workbench>) -> Div {
    let mut list = div().flex().flex_col();
    for (group, items) in SHORTCUTS {
        let mut block = section(t(cx, group));
        for (action, keys) in items.iter() {
            block = block.child(row(
                t(cx, action),
                div()
                    .px_2()
                    .py_0p5()
                    .rounded_md()
                    .border_1()
                    .border_color(hex(Chrome::OVERLAY_BORDER))
                    .bg(hex(0x2a2a2a))
                    .t_small()
                    .text_color(hex(Chrome::BRIGHT))
                    .child(crate::keymap::display(keys).into_owned()),
            ));
        }
        list = list.child(block);
    }
    list
}

/// Installed font families for the full list: sorted, deduplicated, without hidden system faces.
fn installed_font_families(mut names: Vec<String>) -> Vec<String> {
    names.retain(|name| !name.is_empty() && !name.starts_with('.'));
    names.sort_by_key(|name| name.to_lowercase());
    names.dedup();
    names
}

#[cfg(target_os = "macos")]
const FONT_CHOICES: &[&str] =
    &[BUNDLED_FONT, "Menlo", "SF Mono", "Monaco", "Fira Code", "Cascadia Code", "Hack", "Source Code Pro", "D2Coding"];
/// Monospace fonts that ship with Windows first, then popular downloads.
#[cfg(windows)]
const FONT_CHOICES: &[&str] =
    &[BUNDLED_FONT, "Cascadia Mono", "Cascadia Code", "Consolas", "Fira Code", "Hack", "Source Code Pro", "D2Coding"];
#[cfg(not(any(target_os = "macos", windows)))]
const FONT_CHOICES: &[&str] =
    &[BUNDLED_FONT, "DejaVu Sans Mono", "Ubuntu Mono", "Noto Sans Mono", "Fira Code", "Hack", "Source Code Pro", "D2Coding"];

pub(super) fn section(title: &str) -> Div {
    div().flex().flex_col().gap_3().pb_6().child(
        div()
            .t_title()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(hex(Chrome::BRIGHT))
            .pb_1()
            .border_b_1()
            .border_color(hex(Chrome::BORDER))
            .child(title.to_string()),
    )
}

fn row(label: &str, control: impl IntoElement) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_4()
        .child(div().t_body().text_color(hex(Chrome::FOREGROUND)).child(label.to_string()))
        .child(control)
}

/// A row whose label has a second, muted line explaining it.
pub(super) fn row_with_hint(label: &str, hint: &str, control: impl IntoElement) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_4()
        // A long hint wraps inside its column: the control stays in line with those of the other rows.
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_0p5()
                .child(div().t_body().text_color(hex(Chrome::FOREGROUND)).child(label.to_string()))
                .child(div().t_small().text_color(hex(Chrome::MUTED)).child(hint.to_string())),
        )
        .child(div().flex_shrink_0().child(control))
}

fn stepper(
    id: &'static str,
    value: String,
    on_change: impl Fn(&mut Settings, f32) + Clone + 'static,
    step: f32,
    cx: &mut Context<Workbench>,
) -> Div {
    let dec = on_change.clone();
    div()
        .flex()
        .items_center()
        .gap_1()
        .child(action_button(
            SharedString::from(format!("{id}-dec")),
            "−",
            cx.listener(move |_, _: &ClickEvent, _, cx| {
                let dec = dec.clone();
                update_settings(cx, move |s| dec(s, -step));
            }),
        ))
        .child(div().w(px(56.)).flex().justify_center().t_body().text_color(hex(Chrome::BRIGHT)).child(value))
        .child(action_button(
            SharedString::from(format!("{id}-inc")),
            "+",
            cx.listener(move |_, _: &ClickEvent, _, cx| {
                let inc = on_change.clone();
                update_settings(cx, move |s| inc(s, step));
            }),
        ))
}

fn toggle(id: &'static str, on: bool, change: impl Fn(&mut Settings) + 'static, cx: &mut Context<Workbench>) -> impl IntoElement {
    let change = std::rc::Rc::new(change);
    div()
        .id(id)
        .w(px(36.))
        .h(px(20.))
        .rounded_full()
        .cursor_pointer()
        .bg(if on { hex(Chrome::ACCENT) } else { hex(0x3c3c3c) })
        .flex()
        .items_center()
        .when(on, |d| d.justify_end())
        .px_0p5()
        .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
            let change = change.clone();
            update_settings(cx, move |s| change(s));
        }))
        .child(div().size(px(16.)).rounded_full().bg(hex(Chrome::BRIGHT)))
}

fn theme_card(index: usize, theme: &TerminalTheme, selected: bool, cx: &mut Context<Workbench>) -> impl IntoElement {
    let name = theme.name.clone();
    let swatches = div().flex().gap_0p5().children(theme.ansi[1..7].iter().map(|c| div().size(px(10.)).rounded_sm().bg(hex(*c))));
    div()
        .id(("theme", index))
        .w(px(168.))
        .rounded_md()
        .cursor_pointer()
        .border_2()
        .border_color(if selected { hex(Chrome::ACCENT) } else { hex(Chrome::BORDER) })
        .hover(|s| s.border_color(hex_alpha(Chrome::ACCENT, 0.7)))
        .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
            let name = name.clone();
            update_settings(cx, move |s| s.theme = name);
        }))
        .child(
            div()
                .h(px(64.))
                .p_2()
                .rounded_t_md()
                .bg(hex(theme.background))
                .flex()
                .flex_col()
                .gap_1()
                .font_family(BUNDLED_FONT)
                .t_small()
                .child(
                    div()
                        .flex()
                        .gap_1()
                        .child(div().text_color(hex(theme.ansi[2])).child("~/code"))
                        .child(div().text_color(hex(theme.ansi[4])).child("main")),
                )
                .child(div().text_color(hex(theme.foreground)).child("$ cargo run"))
                .child(swatches),
        )
        .child(
            div()
                .px_2()
                .py_1()
                .flex()
                .justify_between()
                .t_small()
                .text_color(if selected { hex(Chrome::BRIGHT) } else { hex(Chrome::FOREGROUND) })
                .child(theme.name.clone())
                .when(theme.imported, |d| d.child(div().text_color(hex(Chrome::MUTED)).child("iTerm"))),
        )
}

impl Workbench {
    fn render_browser_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        use crate::settings::{LinkOpener, SearchEngine};
        let prefs = settings(cx).clone();
        let b = prefs.browser.clone();
        let mut engines = div().flex().gap_1();
        for engine in SearchEngine::ALL {
            engines = engines.child(chip(
                SharedString::from(format!("engine-{engine:?}")),
                engine.name(),
                b.search_engine == engine,
                cx.listener(move |_, _: &ClickEvent, _, cx| update_settings(cx, move |s| s.browser.search_engine = engine)),
            ));
        }
        let opener = |id: &'static str, label: &'static str, value: LinkOpener, cx: &mut Context<Self>| {
            chip(
                id,
                t(cx, label),
                prefs.link_opener == value,
                cx.listener(move |_, _: &ClickEvent, _, cx| update_settings(cx, move |s| s.link_opener = value)),
            )
        };
        div()
            .flex()
            .flex_col()
            .child(
                section(t(cx, "settings.browser_general"))
                    .child(row(t(cx, "settings.browser_home"), div().w(px(320.)).child(self.browser_home_field(window, cx))))
                    .child(row(
                        t(cx, "settings.link_opener"),
                        div()
                            .flex()
                            .gap_1()
                            .child(opener("link-external", "settings.link_external", LinkOpener::External, cx))
                            .child(opener("link-inapp", "settings.link_inapp", LinkOpener::InApp, cx)),
                    ))
                    .child(row(t(cx, "settings.search_engine"), engines))
                    .child(row(
                        t(cx, "settings.browser_zoom"),
                        stepper(
                            "browser-zoom",
                            format!("{:.0}%", b.zoom * 100.),
                            |s, d| s.browser.zoom = ((s.browser.zoom + d) * 10.).round() / 10.,
                            0.1,
                            cx,
                        ),
                    )),
            )
            .child(
                section(t(cx, "settings.browser_content"))
                    .child(row(
                        t(cx, "settings.browser_javascript"),
                        toggle("browser-js", b.javascript, |s| s.browser.javascript = !s.browser.javascript, cx),
                    ))
                    .child(row(
                        t(cx, "settings.browser_popups"),
                        toggle("browser-popups", b.popups, |s| s.browser.popups = !s.browser.popups, cx),
                    ))
                    .child(row(
                        t(cx, "settings.browser_mobile"),
                        toggle("browser-mobile", b.mobile, |s| s.browser.mobile = !s.browser.mobile, cx),
                    )),
            )
            .child(
                section(t(cx, "settings.browser_privacy"))
                    .child(row(
                        t(cx, "settings.browser_private"),
                        toggle("browser-private", b.private_mode, |s| s.browser.private_mode = !s.browser.private_mode, cx),
                    ))
                    .child(row(
                        t(cx, "settings.browser_agent_tools"),
                        toggle("browser-agent-tools", b.agent_tools, |s| s.browser.agent_tools = !s.browser.agent_tools, cx),
                    ))
                    .child(row_with_hint(
                        t(cx, "settings.browser_auto_open"),
                        t(cx, "settings.browser_auto_open_hint"),
                        toggle(
                            "browser-auto-open",
                            b.auto_open_servers,
                            |s| s.browser.auto_open_servers = !s.browser.auto_open_servers,
                            cx,
                        ),
                    ))
                    .child(row(
                        t(cx, "settings.browser_inspect"),
                        toggle("browser-inspect", b.inspectable, |s| s.browser.inspectable = !s.browser.inspectable, cx),
                    ))
                    .child(row(
                        t(cx, "settings.browser_clear"),
                        action_button(
                            "browser-clear-data",
                            t(cx, "settings.browser_clear_button"),
                            cx.listener(|this, _: &ClickEvent, _, cx| {
                                crate::webview::clear_website_data();
                                this.set_status(t(cx, "settings.browser_cleared"), cx);
                            }),
                        ),
                    ))
                    .child(row(
                        t(cx, "settings.browser_width_reset"),
                        action_button(
                            "browser-width-reset",
                            t(cx, "settings.browser_reset_button"),
                            cx.listener(|_, _: &ClickEvent, _, cx| {
                                update_settings(cx, |s| s.browser.width = crate::settings::BrowserSettings::default().width)
                            }),
                        ),
                    ))
                    .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "settings.browser_applies"))),
            )
    }

    /// Text field bound to the browser's start page (saved as you type).
    fn browser_home_field(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let input = match &self.browser_home_input {
            Some((input, _)) => input.clone(),
            None => {
                let home = settings(cx).browser.home.clone();
                let input = cx.new(|cx| TextInput::new(home, "https://…", window, cx));
                let subscription = cx.subscribe(&input, |_, input, event: &TextInputEvent, cx| {
                    if matches!(event, TextInputEvent::Changed | TextInputEvent::Confirmed) {
                        let home = input.read(cx).text().trim().to_string();
                        update_settings(cx, move |s| s.browser.home = home);
                    }
                });
                self.browser_home_input = Some((input.clone(), subscription));
                input
            }
        };
        div()
            .w_full()
            .px_2()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(hex(Chrome::BORDER))
            .bg(hex(0x1a1a1a))
            .t_body()
            .text_color(hex(Chrome::BRIGHT))
            .child(input)
    }

    fn render_about(&self, cx: &mut Context<Self>) -> Div {
        use super::update::{UpdateState, CURRENT_VERSION};
        let status = match &self.updates.state {
            UpdateState::Available(release) => crate::i18n::tf(cx, "update.available_short", &[("version", &release.version)]),
            UpdateState::Checking => t(cx, "update.checking").to_string(),
            UpdateState::UpToDate => t(cx, "update.up_to_date").to_string(),
            UpdateState::Failed(error) => error.clone(),
            UpdateState::CheckFailed(_) => t(cx, "update.check_failed").to_string(),
            _ => String::new(),
        };
        let link = |id: &'static str, label: String, url: &'static str| action_button(id, label, move |_, _, cx| cx.open_url(url));
        div()
            .flex()
            .flex_col()
            .gap_6()
            .child(
                div().flex().items_center().gap_4().child(gpui::img("brand/logo.png").size(px(72.)).flex_shrink_0()).child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(div().t_display().font_weight(FontWeight::SEMIBOLD).text_color(hex(Chrome::BRIGHT)).child("Agentty"))
                        .child(div().t_body().text_color(hex(Chrome::FOREGROUND)).child(t(cx, "tagline")))
                        .child(div().t_small().text_color(hex(Chrome::MUTED)).child(crate::i18n::tf(
                            cx,
                            "update.up_to_date_body",
                            &[("version", CURRENT_VERSION)],
                        ))),
                ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(action_button(
                        "about-check-update",
                        t(cx, "update.check_menu"),
                        cx.listener(|this, _: &ClickEvent, _, cx| this.check_for_updates(true, cx)),
                    ))
                    .when(matches!(self.updates.state, UpdateState::Checking), |d| {
                        d.child(crate::ui::spinner(crate::ui::IconSize::INLINE, hex(Chrome::MUTED)))
                    })
                    .child(div().t_small().text_color(hex(Chrome::MUTED)).child(status)),
            )
            .child(
                section(t(cx, "settings.about_links"))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(link("about-website", "agentty.run".into(), super::update::WEBSITE))
                            .child(link(
                                "about-releases",
                                t(cx, "update.notes").into(),
                                "https://github.com/empty-user77/agentty-releases/releases",
                            ))
                            .child(link("about-license", "GPL-3.0-or-later".into(), "https://www.gnu.org/licenses/gpl-3.0.html"))
                            .child(action_button(
                                "about-onboarding",
                                t(cx, "onboarding.show_again"),
                                cx.listener(|this, _: &ClickEvent, _, cx| this.open_onboarding(cx)),
                            )),
                    )
                    .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "settings.about_notice"))),
            )
    }

    fn harness_pattern_form(&mut self, window: &mut Window, cx: &mut Context<Self>) -> &mut HarnessPatternForm {
        if self.harness_pattern_form.is_none() {
            let input = cx.new(|cx| TextInput::localized("", "settings.harness_pattern_placeholder", window, cx));
            let subscription = cx.subscribe_in(&input, window, |this: &mut Workbench, _, event: &TextInputEvent, _, cx| {
                if matches!(event, TextInputEvent::Confirmed) {
                    this.add_harness_pattern(cx);
                }
            });
            self.harness_pattern_form = Some(HarnessPatternForm { input, invalid: false, _subscription: subscription });
        }
        self.harness_pattern_form.as_mut().expect("pattern form was just created")
    }

    fn add_harness_pattern(&mut self, cx: &mut Context<Self>) {
        let Some(form) = self.harness_pattern_form.as_mut() else { return };
        let pattern = form.input.read(cx).text().trim().trim_start_matches("./").to_string();
        if !agentty_bridge::harness::is_valid_pattern(&pattern) {
            form.invalid = true;
            return cx.notify();
        }
        form.invalid = false;
        form.input.update(cx, |input, cx| input.set_text("", cx));
        update_settings(cx, move |s| {
            if !s.harness_patterns.contains(&pattern) {
                s.harness_patterns.push(pattern);
            }
        });
        self.recheck_harnesses(cx);
    }

    fn render_project_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let prefs = settings(cx).clone();
        let form = self.harness_pattern_form(window, cx);
        let (input, invalid) = (form.input.clone(), form.invalid);

        let mut agents = div().flex().gap_1();
        for (choice, label) in [
            (HarnessAgent::Auto, t(cx, "settings.harness_agent_auto")),
            (HarnessAgent::Claude, "Claude Code"),
            (HarnessAgent::Codex, "Codex"),
        ] {
            agents = agents.child(chip(
                SharedString::from(format!("harness-agent-{label}")),
                label,
                prefs.harness_agent == choice,
                cx.listener(move |_, _: &ClickEvent, _, cx| update_settings(cx, move |s| s.harness_agent = choice)),
            ));
        }

        let pattern_chip = |text: String| {
            div()
                .px_1p5()
                .py_0p5()
                .rounded_sm()
                .bg(hex(0x232323))
                .font_family(BUNDLED_FONT)
                .t_small()
                .text_color(hex(Chrome::MUTED))
                .child(text)
        };
        let defaults = div()
            .flex()
            .flex_wrap()
            .gap_1()
            .children(agentty_bridge::harness::DEFAULT_PATTERNS.iter().map(|p| pattern_chip(p.to_string())))
            .child(pattern_chip("agentty.json → \"harness\"".into()));

        let mut custom = div().flex().flex_col().gap_1();
        if prefs.harness_patterns.is_empty() {
            custom = custom.child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "settings.harness_patterns_empty")));
        }
        for (index, pattern) in prefs.harness_patterns.iter().enumerate() {
            let target = pattern.clone();
            custom = custom.child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .py_1p5()
                    .rounded_md()
                    .bg(hex(0x232323))
                    .font_family(BUNDLED_FONT)
                    .t_body()
                    .child(div().flex_1().min_w_0().truncate().text_color(hex(Chrome::BRIGHT)).child(pattern.clone()))
                    .child(crate::ui::icon_only(
                        ("harness-pattern-remove", index),
                        "x",
                        cx.listener(move |this, _: &ClickEvent, _, cx| {
                            let target = target.clone();
                            update_settings(cx, move |s| s.harness_patterns.retain(|p| p != &target));
                            this.recheck_harnesses(cx);
                        }),
                    )),
            );
        }
        let focus = input.clone();
        let field = div()
            .flex_1()
            .min_w_0()
            .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| window.focus(&gpui::Focusable::focus_handle(&focus, cx)))
            .flex()
            .px_2()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(hex(Chrome::BORDER))
            .bg(hex(0x1a1a1a))
            .t_body()
            .font_family(BUNDLED_FONT)
            .text_color(hex(Chrome::BRIGHT))
            .child(input);

        div()
            .flex()
            .flex_col()
            .child(
                section(t(cx, "settings.project_sessions"))
                    .child(row_with_hint(
                        t(cx, "settings.resume_bar"),
                        t(cx, "settings.resume_bar_hint"),
                        toggle("resume-bar", prefs.resume_bar, |s| s.resume_bar = !s.resume_bar, cx),
                    ))
                    .child(row(
                        t(cx, "settings.ask_dir"),
                        toggle("ask-dir", prefs.ask_directory, |s| s.ask_directory = !s.ask_directory, cx),
                    ))
                    .child(row(
                        t(cx, "settings.ask_dir_tabs"),
                        toggle("ask-dir-tabs", prefs.ask_directory_for_tabs, |s| s.ask_directory_for_tabs = !s.ask_directory_for_tabs, cx),
                    )),
            )
            .child(
                section(t(cx, "settings.project_harness"))
                    .child(row_with_hint(
                        t(cx, "settings.harness_detect"),
                        t(cx, "settings.harness_detect_hint"),
                        div()
                            .id("harness-detect-row")
                            .child(toggle("harness-detect", prefs.harness_detect, |s| s.harness_detect = !s.harness_detect, cx))
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.recheck_harnesses(cx))),
                    ))
                    .child(row_with_hint(t(cx, "settings.harness_agent"), t(cx, "settings.harness_agent_hint"), agents))
                    .child(row_with_hint(
                        t(cx, "settings.harness_submit"),
                        t(cx, "settings.harness_submit_hint"),
                        toggle("harness-submit", prefs.harness_submit, |s| s.harness_submit = !s.harness_submit, cx),
                    ))
                    .child(div().pt_2().t_body().text_color(hex(Chrome::FOREGROUND)).child(t(cx, "settings.harness_patterns")))
                    .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "settings.harness_patterns_hint")))
                    .child(custom)
                    .child(div().flex().items_center().gap_2().child(field).child(action_button(
                        "harness-pattern-add",
                        t(cx, "settings.alias_add"),
                        cx.listener(|this, _: &ClickEvent, _, cx| this.add_harness_pattern(cx)),
                    )))
                    .when(invalid, |d| {
                        d.child(div().t_small().text_color(hex(Chrome::ERROR)).child(t(cx, "settings.harness_pattern_invalid")))
                    })
                    .child(div().pt_2().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "settings.harness_defaults")))
                    .child(defaults),
            )
    }

    fn alias_form(&mut self, window: &mut Window, cx: &mut Context<Self>) -> &mut AliasForm {
        if self.alias_form.is_none() {
            let keyword = cx.new(|cx| TextInput::localized("", "settings.alias_keyword", window, cx));
            let expansion = cx.new(|cx| TextInput::localized("", "settings.alias_expansion", window, cx));
            let on_event =
                |this: &mut Workbench, _: &Entity<TextInput>, event: &TextInputEvent, _: &mut Window, cx: &mut Context<Workbench>| {
                    if matches!(event, TextInputEvent::Confirmed) {
                        this.add_alias(cx);
                    }
                };
            let subscriptions = [cx.subscribe_in(&keyword, window, on_event), cx.subscribe_in(&expansion, window, on_event)];
            self.alias_form =
                Some(AliasForm { keyword, expansion, command: "claude".into(), invalid: false, _subscriptions: subscriptions });
        }
        self.alias_form.as_mut().expect("alias form was just created")
    }

    fn add_alias(&mut self, cx: &mut Context<Self>) {
        let Some(form) = self.alias_form.as_mut() else { return };
        let keyword = form.keyword.read(cx).text().trim().to_string();
        let expansion = form.expansion.read(cx).text().trim().to_string();
        let command = form.command.clone();
        if !is_valid_word(&keyword) || !is_valid_command(&command) || expansion.is_empty() {
            form.invalid = true;
            return cx.notify();
        }
        form.invalid = false;
        form.keyword.update(cx, |input, cx| input.set_text("", cx));
        form.expansion.update(cx, |input, cx| input.set_text("", cx));
        update_settings(cx, move |s| {
            s.aliases.retain(|a| !(a.keyword == keyword && a.command == command));
            s.aliases.push(CommandAlias { keyword, expansion, command });
        });
    }

    fn render_aliases(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let aliases = settings(cx).aliases.clone();
        let form = self.alias_form(window, cx);
        let (keyword, expansion, command, invalid) = (form.keyword.clone(), form.expansion.clone(), form.command.clone(), form.invalid);

        let field = |input: Entity<TextInput>| {
            let focus = input.clone();
            div()
                .flex_1()
                .min_w_0()
                // The whole box focuses the field (the text element alone can be narrower than the box).
                .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| window.focus(&gpui::Focusable::focus_handle(&focus, cx)))
                .flex()
                .px_2()
                .py_1()
                .rounded_md()
                .border_1()
                .border_color(hex(Chrome::BORDER))
                .bg(hex(0x1a1a1a))
                .t_body()
                .text_color(hex(Chrome::BRIGHT))
                .child(input)
        };
        let mut commands = div().flex().gap_1();
        for name in ["claude", "codex"] {
            commands = commands.child(chip(
                SharedString::from(format!("alias-cmd-{name}")),
                name,
                command == name,
                cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if let Some(form) = this.alias_form.as_mut() {
                        form.command = name.to_string();
                    }
                    cx.notify();
                }),
            ));
        }

        let mut list = div().flex().flex_col().gap_1();
        if aliases.is_empty() {
            list = list.child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "settings.alias_empty")));
        }
        for (index, alias) in aliases.iter().enumerate() {
            let target = alias.clone();
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .py_1p5()
                    .rounded_md()
                    .bg(hex(0x232323))
                    .font_family(BUNDLED_FONT)
                    .t_body()
                    .child(div().text_color(hex(Chrome::MUTED)).child(alias.command.clone()))
                    .child(
                        div()
                            .px_1p5()
                            .rounded_sm()
                            .bg(hex_alpha(Chrome::ACCENT, 0.3))
                            .text_color(hex(Chrome::BRIGHT))
                            .child(alias.keyword.clone()),
                    )
                    .child(div().text_color(hex(Chrome::MUTED)).child("→"))
                    .child(div().flex_1().min_w_0().truncate().text_color(hex(Chrome::FOREGROUND)).child(alias.expansion.clone()))
                    .child(crate::ui::icon_only(
                        ("alias-remove", index),
                        "x",
                        cx.listener(move |_, _: &ClickEvent, _, cx| {
                            let target = target.clone();
                            update_settings(cx, move |s| s.aliases.retain(|a| a != &target));
                        }),
                    )),
            );
        }

        section(t(cx, "settings.aliases"))
            .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "settings.aliases_hint")))
            .child(list)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(commands)
                    .child(div().w(px(150.)).flex().child(field(keyword)))
                    .child(div().text_color(hex(Chrome::MUTED)).child("→"))
                    .child(field(expansion))
                    .child(action_button(
                        "alias-add",
                        t(cx, "settings.alias_add"),
                        cx.listener(|this, _: &ClickEvent, _, cx| this.add_alias(cx)),
                    )),
            )
            .child(div().t_small().text_color(if invalid { hex(Chrome::ERROR) } else { hex(Chrome::MUTED) }).child(if invalid {
                t(cx, "settings.alias_invalid")
            } else {
                t(cx, "settings.alias_applies")
            }))
    }

    fn import_theme(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: None });
        cx.spawn(async move |_, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                let dir = themes_dir();
                let _ = std::fs::create_dir_all(&dir);
                for path in paths.iter().filter(|p| p.extension().is_some_and(|e| e == "itermcolors")) {
                    if let Some(name) = path.file_name() {
                        let _ = std::fs::copy(path, dir.join(name));
                    }
                }
                let _ = cx.update(reload_themes);
            }
        })
        .detach();
    }

    pub(super) fn render_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let aliases = self.render_aliases(window, cx);
        let prefs = settings(cx).clone();
        let themes = cx.global::<SettingsStore>().themes.clone();

        let mut languages = div().flex().gap_1();
        for language in Language::ALL {
            languages = languages.child(chip(
                SharedString::from(format!("lang-{language:?}")),
                if language == Language::System { t(cx, "settings.language_system") } else { language.native_name() },
                prefs.language == language,
                cx.listener(move |_, _: &ClickEvent, _, cx| {
                    update_settings(cx, move |s| s.language = language);
                    crate::set_app_menus(cx);
                }),
            ));
        }

        // Explicit rows: GPUI's layout doesn't size wrapped flex rows correctly inside a column.
        let mut theme_grid = div().flex().flex_col().gap_3();
        for (row_index, chunk) in themes.chunks(THEMES_PER_ROW).enumerate() {
            let mut row = div().flex().gap_3();
            for (offset, theme) in chunk.iter().enumerate() {
                row = row.child(theme_card(row_index * THEMES_PER_ROW + offset, theme, theme.name == prefs.theme, cx));
            }
            theme_grid = theme_grid.child(row);
        }

        let mut fonts = div().flex().gap_1().justify_end();
        // Enumerating system fonts is slow; do it once per app run.
        let installed = self.installed_fonts.get_or_insert_with(|| installed_font_families(cx.text_system().all_font_names())).clone();
        for (index, family) in FONT_CHOICES.iter().enumerate() {
            if *family != BUNDLED_FONT && !installed.iter().any(|name| name == family) {
                continue;
            }
            let label = if *family == BUNDLED_FONT { format!("{family} ({})", t(cx, "settings.bundled")) } else { family.to_string() };
            let family = family.to_string();
            fonts = fonts.child(chip(
                ("font", index),
                label,
                prefs.font_family == family,
                cx.listener(move |_, _: &ClickEvent, _, cx| {
                    let family = family.clone();
                    update_settings(cx, move |s| s.font_family = family);
                }),
            ));
        }
        // A font picked from the full list shows as its own chip next to the suggestions.
        let custom_font = !FONT_CHOICES.contains(&prefs.font_family.as_str());
        if custom_font {
            fonts = fonts.child(chip("font-custom", prefs.font_family.clone(), true, |_, _, _| {}));
        }
        let font_list_open = self.font_list_open;
        fonts = fonts.child(chip(
            "font-more",
            format!("{} {}", t(cx, "settings.font_more"), if font_list_open { "▴" } else { "▾" }),
            font_list_open,
            cx.listener(|this, _: &ClickEvent, _, cx| {
                this.font_list_open = !this.font_list_open;
                cx.notify();
            }),
        ));
        let font_list = font_list_open.then(|| {
            let mut list = div()
                .id("font-list")
                .max_h(px(260.))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .p_1()
                .rounded_md()
                .border_1()
                .border_color(hex(Chrome::OVERLAY_BORDER))
                .bg(hex(0x1a1a1a));
            for (index, family) in installed.iter().enumerate() {
                let active = prefs.font_family == *family;
                let target = family.clone();
                list = list.child(
                    div()
                        .id(("font-item", index))
                        .flex()
                        .items_center()
                        .gap_3()
                        .px_2()
                        .py_1()
                        .rounded_sm()
                        .cursor_pointer()
                        .when(active, |d| d.bg(hex_alpha(Chrome::ACCENT, 0.25)))
                        .hover(|s| s.bg(hex(Chrome::HOVER)))
                        .child(
                            div()
                                .w(px(220.))
                                .flex_shrink_0()
                                .truncate()
                                .t_small()
                                .text_color(if active { hex(Chrome::BRIGHT) } else { hex(Chrome::FOREGROUND) })
                                .child(family.clone()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .font_family(family.clone())
                                .t_body()
                                .text_color(hex(Chrome::MUTED))
                                .child("AaBb 0O1l 한글 -> =>"),
                        )
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            let family = target.clone();
                            this.font_list_open = false;
                            update_settings(cx, move |s| s.font_family = family);
                        })),
                );
            }
            list
        });

        let mut advisors = div().flex().gap_1();
        for choice in crate::settings::AdvisorChoice::ALL {
            advisors = advisors.child(chip(
                SharedString::from(format!("advisor-{choice:?}")),
                t(cx, choice.label_key()),
                prefs.advisor == choice,
                cx.listener(move |_, _: &ClickEvent, _, cx| update_settings(cx, move |s| s.advisor = choice)),
            ));
        }

        let mut bar_positions = div().flex().gap_1();
        for (position, key) in
            [(crate::hud::HudPosition::Top, "settings.position_top"), (crate::hud::HudPosition::Bottom, "settings.position_bottom")]
        {
            bar_positions = bar_positions.child(chip(
                SharedString::from(format!("bar-position-{position:?}")),
                t(cx, key),
                prefs.agent_bar_position == position,
                cx.listener(move |_, _: &ClickEvent, _, cx| update_settings(cx, move |s| s.agent_bar_position = position)),
            ));
        }

        let mut cursors = div().flex().gap_1();
        for (shape, key) in [
            (CursorShapeSetting::Block, "settings.cursor.block"),
            (CursorShapeSetting::Beam, "settings.cursor.beam"),
            (CursorShapeSetting::Underline, "settings.cursor.underline"),
        ] {
            cursors = cursors.child(chip(
                SharedString::from(format!("cursor-{shape:?}")),
                t(cx, key),
                prefs.cursor_shape == shape,
                cx.listener(move |_, _: &ClickEvent, _, cx| {
                    update_settings(cx, move |s| s.cursor_shape = shape);
                }),
            ));
        }

        let section_id = self.settings_section;
        let content: gpui::AnyElement = match section_id {
            SettingsSection::General => div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .pb_6()
                        .child(row(t(cx, "settings.language"), languages))
                        .child(row_with_hint(t(cx, "advisor.setting"), t(cx, "advisor.setting_hint"), advisors))
                        .child(row_with_hint(
                            t(cx, "settings.idea_mode"),
                            t(cx, "settings.idea_mode_hint"),
                            toggle("idea-mode", prefs.idea_mode, |s| s.idea_mode = !s.idea_mode, cx),
                        ))
                        .child(row_with_hint(
                            t(cx, "settings.agent_bar"),
                            t(cx, "settings.agent_bar_hint"),
                            toggle("agent-bar", prefs.agent_bar, |s| s.agent_bar = !s.agent_bar, cx),
                        ))
                        .child(row_with_hint(
                            t(cx, "settings.agent_bar_position"),
                            t(cx, "settings.agent_bar_position_hint"),
                            bar_positions,
                        ))
                        .child(row(
                            t(cx, "settings.confirm_close"),
                            toggle("confirm-close", prefs.confirm_close, |s| s.confirm_close = !s.confirm_close, cx),
                        ))
                        .child(row_with_hint(
                            t(cx, "settings.auto_worktree"),
                            t(cx, "settings.auto_worktree_hint"),
                            toggle("auto-worktree", prefs.auto_worktree, |s| s.auto_worktree = !s.auto_worktree, cx),
                        ))
                        .child(row_with_hint(
                            t(cx, "settings.agent_tasks"),
                            t(cx, "settings.agent_tasks_hint"),
                            toggle("agent-tasks", prefs.agent_tasks, |s| s.agent_tasks = !s.agent_tasks, cx),
                        ))
                        .child(row_with_hint(
                            t(cx, "settings.agent_guide"),
                            t(cx, "settings.agent_guide_hint"),
                            toggle("agent-guide", prefs.agent_guide, |s| s.agent_guide = !s.agent_guide, cx),
                        ))
                        .child(row_with_hint(
                            t(cx, "settings.stop_servers"),
                            t(cx, "settings.stop_servers_hint"),
                            toggle("stop-servers", prefs.stop_servers_on_close, |s| s.stop_servers_on_close = !s.stop_servers_on_close, cx),
                        ))
                        .child(row(
                            t(cx, "settings.system_notifications"),
                            toggle(
                                "system-notifications",
                                prefs.system_notifications,
                                |s| s.system_notifications = !s.system_notifications,
                                cx,
                            ),
                        ))
                        .child(row(
                            t(cx, "settings.notify_when_focused"),
                            toggle("notify-focused", prefs.notify_when_focused, |s| s.notify_when_focused = !s.notify_when_focused, cx),
                        ))
                        .when(crate::platform::HAS_STATUS_ITEM, |d| {
                            d.child(row(t(cx, "settings.menu_bar"), toggle("menu-bar", prefs.menu_bar, |s| s.menu_bar = !s.menu_bar, cx)))
                        }),
                )
                .child(aliases)
                .into_any_element(),
            SettingsSection::Project => self.render_project_settings(window, cx).into_any_element(),
            SettingsSection::Accounts => self.render_accounts(window, cx).into_any_element(),
            SettingsSection::Appearance => div()
                .flex()
                .flex_col()
                .child(
                    section(t(cx, "settings.theme")).child(theme_grid).child(
                        div()
                            .flex()
                            .gap_2()
                            .child(action_button(
                                "import-theme",
                                t(cx, "settings.import_theme"),
                                cx.listener(|this, _: &ClickEvent, _, cx| this.import_theme(cx)),
                            ))
                            .child(action_button(
                                "open-themes",
                                t(cx, "settings.open_themes"),
                                cx.listener(|_, _: &ClickEvent, _, cx| {
                                    let dir = themes_dir();
                                    let _ = std::fs::create_dir_all(&dir);
                                    cx.open_with_system(&dir);
                                }),
                            )),
                    ),
                )
                .child(
                    section(t(cx, "settings.font"))
                        .child(row(t(cx, "settings.font"), fonts))
                        .children(font_list)
                        .child(row(
                            t(cx, "settings.font_size"),
                            stepper(
                                "font-size",
                                format!("{:.0}", prefs.font_size),
                                |s, d| s.font_size = (s.font_size + d).clamp(8.0, 32.0),
                                1.0,
                                cx,
                            ),
                        ))
                        .child(row(
                            t(cx, "settings.line_height"),
                            stepper(
                                "line-height",
                                format!("{:.2}", prefs.line_height),
                                |s, d| s.line_height = (((s.line_height + d) * 100.0).round() / 100.0).clamp(0.8, 1.6),
                                0.05,
                                cx,
                            ),
                        ))
                        .child(
                            div()
                                .p_3()
                                .rounded_md()
                                .bg(hex(crate::settings::terminal_theme(cx).background))
                                .font_family(prefs.font_family.clone())
                                .text_size(px(prefs.font_size))
                                .text_color(hex(crate::settings::terminal_theme(cx).foreground))
                                .child(format!("{} — ~/Agentty \u{e0a0} main  한글 日本語 中文  -> => != ", t(cx, "settings.preview"))),
                        ),
                )
                .child(self.render_hud_settings(cx))
                .child(section(t(cx, "settings.cursor")).child(row(t(cx, "settings.cursor"), cursors)).child(row(
                    t(cx, "settings.cursor_blink"),
                    toggle("cursor-blink", prefs.cursor_blink, |s| s.cursor_blink = !s.cursor_blink, cx),
                )))
                .child(
                    section(t(cx, "settings.terminal"))
                        .child(row(
                            t(cx, "settings.padding"),
                            stepper(
                                "padding",
                                format!("{:.0}", prefs.padding),
                                |s, d| s.padding = (s.padding + d).clamp(0.0, 40.0),
                                2.0,
                                cx,
                            ),
                        ))
                        .child(row(
                            t(cx, "settings.option_meta"),
                            toggle("option-meta", prefs.option_as_meta, |s| s.option_as_meta = !s.option_as_meta, cx),
                        ))
                        .child(row(
                            &format!("{} ({})", t(cx, "settings.scrollback"), t(cx, "settings.new_only")),
                            stepper(
                                "scrollback",
                                prefs.scrollback.to_string(),
                                |s, d| s.scrollback = (s.scrollback as f32 + d).clamp(1_000.0, 200_000.0) as usize,
                                5_000.0,
                                cx,
                            ),
                        ))
                        .child(div().flex().child(action_button(
                            "open-settings-json",
                            t(cx, "settings.open_json"),
                            cx.listener(|_, _: &ClickEvent, _, cx| {
                                cx.open_with_system(&Settings::path());
                            }),
                        ))),
                )
                .into_any_element(),
            SettingsSection::Shortcuts => render_shortcuts(cx).into_any_element(),
            SettingsSection::System => self.render_system_check(cx).into_any_element(),
            SettingsSection::Browser => self.render_browser_settings(window, cx).into_any_element(),
            SettingsSection::About => self.render_about(cx).into_any_element(),
        };

        let mut nav = div()
            .w(px(210.))
            .flex_shrink_0()
            .h_full()
            .flex()
            .flex_col()
            .gap_0p5()
            .p_3()
            .border_r_1()
            .border_color(hex(Chrome::BORDER))
            .bg(hex(Chrome::SIDE_BAR));
        for section in SettingsSection::visible() {
            let active = section == section_id;
            nav = nav.child(
                div()
                    .id(SharedString::from(format!("settings-nav-{section:?}")))
                    .px_3()
                    .py_1p5()
                    .rounded_md()
                    .flex()
                    .items_center()
                    .gap_2()
                    .t_body()
                    .cursor_pointer()
                    .when(active, |d| d.bg(hex(Chrome::SELECTED)).text_color(hex(Chrome::BRIGHT)))
                    .when(!active, |d| d.text_color(hex(Chrome::FOREGROUND)).hover(|s| s.bg(hex(Chrome::HOVER))))
                    .child(crate::ui::icon(
                        section.icon(),
                        crate::ui::IconSize::INLINE,
                        hex(if active { Chrome::BRIGHT } else { Chrome::MUTED }),
                    ))
                    .child(t(cx, section.label()))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.settings_section = section;
                        this.settings_scroll = gpui::ScrollHandle::new();
                        cx.notify();
                    })),
            );
        }

        let scroll = self.settings_scroll.clone();
        div().size_full().flex().bg(hex(Chrome::EDITOR)).child(nav).child(
            div().relative().flex_1().min_w_0().h_full().child(crate::ui::scrollbar(scroll.clone())).child(
                div().id("settings-page").size_full().overflow_y_scroll().track_scroll(&scroll).child(
                    div()
                        .max_w(px(820.))
                        .px_8()
                        .py_6()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .t_heading()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(hex(Chrome::BRIGHT))
                                .pb_5()
                                .child(t(cx, section_id.label())),
                        )
                        .child(content),
                ),
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_list_is_sorted_without_hidden_faces() {
        let names = ["Menlo", ".SF NS Mono", "d2coding", "Andale Mono", "Menlo", ""].map(String::from).to_vec();
        assert_eq!(installed_font_families(names), ["Andale Mono", "d2coding", "Menlo"]);
    }
}
