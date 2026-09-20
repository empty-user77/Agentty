//! Plugins page: installed plugins (enable, restart, logs, uninstall), the built-in catalog,
//! installing from a folder or Git, and creating a new plugin with an AI agent.

use super::{LaunchTarget, Page, Workbench};
use crate::i18n::{t, tf};
use crate::launch::{LaunchSpec, PaneKind};
use crate::plugins::{self, RunState};
use crate::settings::settings;
use crate::text_input::TextInput;
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{action_button, hint, icon, icon_named, icon_only_sized, tilde, IconSize, TypeScale};
use agentty_bridge::model::Agent;
use agentty_bridge::plugins::manifest::{Manifest, PERMISSIONS};
use agentty_bridge::plugins::store::{self, InstalledPlugin, Source};
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, Entity, FontWeight, PathPromptOptions, SharedString, Window};

/// First message to the agent that builds a new plugin. Prompts are English whatever the UI
/// language is; `{language}` tells the agent which one to talk in.
const AI_PROMPT: &str = "Build the Agentty plugin \"{name}\" in this folder. It should: {idea}\n\nRead CLAUDE.md and PLUGIN_GUIDE.md first. Keep agentty-plugin.json in sync with main.mjs, check the code with `node --check main.mjs`, and when you are done tell me to press Restart on the plugin in Agentty's Plugins page.\n\nTalk to me in {language}.";
const AI_PROMPT_ASK: &str = "We are building the Agentty plugin \"{name}\" in this folder. Read CLAUDE.md and PLUGIN_GUIDE.md, then ask me what the plugin should do.\n\nTalk to me in {language}.";

/// Width of a closed plugin tile: they sit side by side and wrap onto as many rows as needed.
const CARD_WIDTH: f32 = 260.;
/// How much of the description a closed tile shows.
const SUMMARY_CHARS: usize = 82;

#[derive(Default)]
pub struct PluginsPage {
    /// Plugin to scroll to / highlight (from a link or the panel's settings button).
    pub focus: Option<String>,
    /// (text, is error)
    pub message: Option<(String, bool)>,
    /// A link that needs this built-in plugin: (plugin id, link), opened after installing it.
    pub pending_link: Option<(String, String)>,
    logs_open: Option<String>,
    /// The card the user opened: it spans the row and shows everything about the plugin.
    pub expanded: Option<String>,
    confirm_uninstall: Option<String>,
    busy: bool,
    inputs: Option<Inputs>,
    scroll: gpui::ScrollHandle,
}

struct Inputs {
    git_url: Entity<TextInput>,
    name: Entity<TextInput>,
    idea: Entity<TextInput>,
}

impl Workbench {
    pub(super) fn prepare_plugins_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.page != Some(Page::Plugins) || self.plugins_page.inputs.is_some() {
            return;
        }
        self.plugins_page.inputs = Some(Inputs {
            git_url: cx.new(|cx| TextInput::localized("", "plugins.git_placeholder", window, cx)),
            name: cx.new(|cx| TextInput::localized("", "plugins.name_placeholder", window, cx)),
            idea: cx.new(|cx| TextInput::localized("", "plugins.idea_placeholder", window, cx)),
        });
    }

    fn plugins_message(&mut self, text: String, error: bool, cx: &mut Context<Self>) {
        self.plugins_page.message = Some((text, error));
        cx.notify();
    }

    fn after_install(&mut self, result: anyhow::Result<InstalledPlugin>, window: &mut Window, cx: &mut Context<Self>) {
        self.plugins_page.busy = false;
        match result {
            Ok(plugin) => {
                plugins::reload(cx);
                let name = plugin.name().to_string();
                self.plugins_page.focus = Some(plugin.id.clone());
                self.plugins_page.expanded = Some(plugin.id.clone());
                self.plugins_message(tf(cx, "plugins.installed_ok", &[("name", &name)]), false, cx);
                if let Some((_, link)) = self.plugins_page.pending_link.clone().filter(|(id, _)| *id == plugin.id) {
                    self.plugins_page.pending_link = None;
                    self.open_agentty_link(&link, window, cx);
                }
            }
            Err(err) => self.plugins_message(format!("{err:#}"), true, cx),
        }
    }

    pub(super) fn install_builtin_plugin(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let result = store::install_builtin(&id);
        self.after_install(result, window, cx);
    }

    fn install_plugin_folder(&mut self, link: bool, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: None });
        let window = window.window_handle();
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = paths.await else { return };
            let Some(folder) = paths.pop() else { return };
            let result = if link { store::link_dev_folder(&folder) } else { store::install_from_folder(&folder) };
            let _ = cx.update_window(window, |_, window, cx| {
                let _ = this.update(cx, |this, cx| this.after_install(result, window, cx));
            });
        })
        .detach();
    }

    fn install_plugin_git(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(url) = self.plugins_page.inputs.as_ref().map(|i| i.git_url.read(cx).text().trim().to_string()) else { return };
        if url.is_empty() || self.plugins_page.busy {
            return;
        }
        self.plugins_page.busy = true;
        self.plugins_message(tf(cx, "plugins.cloning", &[("name", &url)]), false, cx);
        let task = cx.background_spawn(async move { store::install_from_git(&url) });
        let window = window.window_handle();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = cx.update_window(window, |_, window, cx| {
                let _ = this.update(cx, |this, cx| {
                    if result.is_ok() {
                        if let Some(inputs) = this.plugins_page.inputs.as_ref() {
                            inputs.git_url.update(cx, |i, cx| i.set_text("", cx));
                        }
                    }
                    this.after_install(result, window, cx);
                });
            });
        })
        .detach();
    }

    /// Creates a plugin from the template and opens Claude Code in its folder to build it.
    fn create_plugin_with_ai(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(inputs) = self.plugins_page.inputs.as_ref() else { return };
        let name = inputs.name.read(cx).text().trim().to_string();
        let idea = inputs.idea.read(cx).text().trim().to_string();
        match store::create_plugin(&name) {
            Ok(plugin) => {
                plugins::reload(cx);
                if let Some(inputs) = self.plugins_page.inputs.as_ref() {
                    inputs.name.update(cx, |i, cx| i.set_text("", cx));
                    inputs.idea.update(cx, |i, cx| i.set_text("", cx));
                }
                let template = if idea.is_empty() { AI_PROMPT_ASK } else { AI_PROMPT };
                let language = agentty_bridge::idea::language_name(settings(cx).language.code());
                let prompt = template.replace("{name}", plugin.name()).replace("{idea}", &idea).replace("{language}", language);
                let title = tf(cx, "plugins.ai_workspace", &[("name", plugin.name())]);
                self.page = None;
                self.create_workspace(LaunchSpec::with_prompt(Agent::Claude, prompt, title.clone(), plugin.dir.clone()), window, cx);
                if let Some(ws) = self.workspaces.last_mut() {
                    ws.name = Some(title);
                }
                self.persist(cx);
            }
            Err(err) => self.plugins_message(format!("{err:#}"), true, cx),
        }
    }

    fn set_plugin_enabled(&mut self, id: &str, enabled: bool, cx: &mut Context<Self>) {
        match store::set_enabled(id, enabled) {
            Ok(()) => {
                if !enabled && self.plugin_panel.as_deref() == Some(id) {
                    self.plugin_panel = None;
                }
                plugins::reload(cx);
            }
            Err(err) => self.plugins_message(format!("{err:#}"), true, cx),
        }
        cx.notify();
    }

    fn uninstall_plugin(&mut self, id: &str, cx: &mut Context<Self>) {
        self.plugins_page.confirm_uninstall = None;
        plugins::stop(id, cx);
        if self.plugin_panel.as_deref() == Some(id) {
            self.plugin_panel = None;
        }
        match store::uninstall(id) {
            Ok(()) => {
                plugins::reload(cx);
                self.plugins_message(tf(cx, "plugins.uninstalled", &[("name", id)]), false, cx);
            }
            Err(err) => self.plugins_message(format!("{err:#}"), true, cx),
        }
    }

    pub(super) fn render_plugins_page(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let page = &self.plugins_page;
        let installed = plugins::host(cx).installed.clone();
        let header = div()
            .flex()
            .items_center()
            .gap_3()
            .child(div().t_heading().font_weight(FontWeight::SEMIBOLD).text_color(hex(Chrome::BRIGHT)).child(t(cx, "page.plugins")))
            .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "plugins.subtitle")))
            .child(div().flex_1())
            .child(action_button(
                "plugins-guide",
                t(cx, "plugins.guide"),
                // Read inside Agentty (the panel next to the terminals), not in another app.
                cx.listener(|this, _: &ClickEvent, _, cx| match super::guide::unpack() {
                    Ok(page) => {
                        this.page = None;
                        this.open_browser(Some(super::guide::file_url(&page)), cx);
                    }
                    Err(err) => this.plugins_message(err.to_string(), true, cx),
                }),
            ))
            .child(action_button(
                "plugins-copy-prompt",
                t(cx, "plugins.copy_prompt"),
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(store::AI_PROMPT.to_string()));
                    this.show_toast(t(cx, "plugins.prompt_copied"), cx);
                }),
            ))
            .child(action_button(
                "plugins-refresh",
                t(cx, "usage.refresh"),
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.plugins_page.message = None;
                    plugins::reload(cx);
                }),
            ))
            .child(action_button(
                "plugins-open-folder",
                t(cx, "plugins.open_folder"),
                cx.listener(|_, _: &ClickEvent, _, _| {
                    let dir = store::plugins_dir();
                    let _ = std::fs::create_dir_all(&dir);
                    crate::platform::open_folder(&dir);
                }),
            ));

        let message = page.message.clone().map(|(text, error)| {
            div()
                .px_3()
                .py_2()
                .rounded_md()
                .t_small()
                .bg(hex_alpha(if error { Chrome::ERROR } else { Chrome::ACCENT }, 0.15))
                .text_color(hex(if error { Chrome::ERROR } else { Chrome::FOREGROUND }))
                .child(text)
        });

        let pending = page.pending_link.clone().filter(|(id, _)| !installed.iter().any(|p| p.id == *id)).map(|(id, _)| {
            let name = store::builtin(&id).map(|b| b.manifest().name).unwrap_or_else(|| id.clone());
            div()
                .px_3()
                .py_2()
                .flex()
                .items_center()
                .gap_3()
                .rounded_md()
                .bg(hex_alpha(Chrome::ATTENTION, 0.15))
                .child(icon("puzzle", IconSize::BUTTON, hex(Chrome::ATTENTION)))
                .child(div().flex_1().t_small().text_color(hex(Chrome::BRIGHT)).child(tf(cx, "plugins.link_needs", &[("name", &name)])))
                .child(action_button(
                    "plugins-install-pending",
                    t(cx, "plugins.install_continue"),
                    cx.listener(move |this, _: &ClickEvent, window, cx| this.install_builtin_plugin(id.clone(), window, cx)),
                ))
        });

        let mut installed_list = div().flex().flex_wrap().items_start().content_start().gap_2();
        if installed.is_empty() {
            installed_list = installed_list.child(hint(t(cx, "plugins.none_installed")));
        }
        for plugin in &installed {
            installed_list = installed_list.child(self.render_installed_plugin(plugin, cx));
        }

        let mut catalog = div().flex().flex_wrap().items_start().content_start().gap_2();
        let available: Vec<Manifest> =
            store::BUILTIN.iter().map(|b| b.manifest()).filter(|m| !installed.iter().any(|p| p.id == m.id)).collect();
        if available.is_empty() {
            catalog = catalog.child(hint(t(cx, "plugins.catalog_all_installed")));
        }
        for manifest in available {
            catalog = catalog.child(self.render_catalog_plugin(manifest, cx));
        }

        let inputs = page.inputs.as_ref();
        let field = |input: Option<&Entity<TextInput>>| {
            div()
                .flex_1()
                .min_w_0()
                .px_2()
                .py_1()
                .rounded_md()
                .border_1()
                .border_color(hex(Chrome::BORDER))
                .bg(hex(0x1a1a1a))
                .t_small()
                .text_color(hex(Chrome::BRIGHT))
                .children(input.cloned())
        };
        let create = div()
            .p_4()
            .flex()
            .flex_col()
            .gap_2()
            .rounded_lg()
            .border_1()
            .border_color(hex(Chrome::BORDER))
            .bg(hex(Chrome::PANEL))
            .child(div().flex().items_center().gap_2().child(icon("wand-sparkles", IconSize::BUTTON, hex(Chrome::PURPLE))).child(
                div().t_title().font_weight(FontWeight::SEMIBOLD).text_color(hex(Chrome::BRIGHT)).child(t(cx, "plugins.create_title")),
            ))
            .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "plugins.create_body")))
            .child(div().flex().gap_2().child(field(inputs.map(|i| &i.name)).max_w(px(220.))).child(field(inputs.map(|i| &i.idea))))
            .child(div().flex().gap_2().child(action_button(
                "plugins-create",
                t(cx, "plugins.create_button"),
                cx.listener(|this, _: &ClickEvent, window, cx| this.create_plugin_with_ai(window, cx)),
            )));

        let install = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().flex().gap_2().items_center().child(field(inputs.map(|i| &i.git_url))).child(action_button(
                "plugins-git",
                if page.busy { t(cx, "plugins.installing") } else { t(cx, "plugins.install_git") },
                cx.listener(|this, _: &ClickEvent, window, cx| this.install_plugin_git(window, cx)),
            )))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(action_button(
                        "plugins-folder",
                        t(cx, "plugins.install_folder"),
                        cx.listener(|this, _: &ClickEvent, window, cx| this.install_plugin_folder(false, window, cx)),
                    ))
                    .child(action_button(
                        "plugins-link",
                        t(cx, "plugins.link_folder"),
                        cx.listener(|this, _: &ClickEvent, window, cx| this.install_plugin_folder(true, window, cx)),
                    )),
            );

        let section = |title: &str| {
            div().pt_3().t_caption().font_weight(FontWeight::SEMIBOLD).text_color(hex(Chrome::MUTED)).child(title.to_uppercase())
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(hex(Chrome::EDITOR))
            .child(
                div()
                    .flex_shrink_0()
                    .px_6()
                    .pt_5()
                    .pb_3()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .border_b_1()
                    .border_color(hex(Chrome::BORDER))
                    .child(header)
                    .children(pending)
                    .children(message),
            )
            .child(
                div()
                    .id("plugins-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&page.scroll)
                    .relative()
                    .child(
                        div()
                            .px_6()
                            .py_4()
                            .max_w(px(980.))
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(section(&tf(cx, "plugins.installed", &[("n", &installed.len().to_string())])))
                            .child(installed_list)
                            .child(section(t(cx, "plugins.catalog")))
                            .child(catalog)
                            .child(section(t(cx, "plugins.develop")))
                            .child(create)
                            .child(install),
                    )
                    .child(crate::ui::scrollbar(page.scroll.clone())),
            )
    }

    /// Plugins are shown as small tiles side by side; the one the user opens takes the whole row
    /// and shows its links, permissions, folder and logs.
    fn card(&self, id: &str, focused: bool) -> gpui::Stateful<gpui::Div> {
        let open = self.expanded(id);
        div()
            .id(SharedString::from(format!("plugin-card-{id}")))
            .p_2p5()
            .flex()
            .flex_col()
            .gap_1p5()
            .rounded_lg()
            .border_1()
            .border_color(hex(if focused { Chrome::ACCENT } else { Chrome::BORDER }))
            .bg(hex(Chrome::PANEL))
            .when(open, |d| d.w_full())
            .when(!open, |d| d.w(px(CARD_WIDTH)))
    }

    fn expanded(&self, id: &str) -> bool {
        self.plugins_page.expanded.as_deref() == Some(id)
    }

    /// Opens or closes a card (the ⌄ button and the card's own head).
    fn toggle_plugin_card(&mut self, id: &str, cx: &mut Context<Self>) {
        self.plugins_page.expanded = if self.expanded(id) { None } else { Some(id.to_string()) };
        if self.plugins_page.expanded.is_none() {
            self.plugins_page.logs_open = None;
        }
        cx.notify();
    }

    /// Title row of a tile: icon, name, version and the button that opens the card.
    fn render_card_head(&self, manifest: &Manifest, cx: &mut Context<Self>) -> impl IntoElement {
        let _ = cx;
        let open = self.expanded(&manifest.id);
        let id = manifest.id.clone();
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(div().size(px(24.)).flex_shrink_0().rounded_md().bg(hex(0x2a2a2a)).flex().items_center().justify_center().child(icon(
                icon_named(manifest.icon.as_deref()),
                IconSize::INLINE,
                hex(Chrome::BRIGHT),
            )))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .t_body()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(hex(Chrome::BRIGHT))
                            .child(manifest.name.clone()),
                    )
                    .child(div().flex_shrink_0().t_caption().text_color(hex(Chrome::MUTED)).child(format!("v{}", manifest.version))),
            )
            .child(icon_only_sized(
                SharedString::from(format!("plugin-open-{}", manifest.id)),
                if open { "chevron-up" } else { "chevron-down" },
                20.,
                12.,
                cx.listener(move |this, _: &ClickEvent, _, cx| this.toggle_plugin_card(&id, cx)),
            ))
    }

    /// The little state and source tags under the title.
    fn render_badges(&self, badges: Vec<(String, u32)>) -> impl IntoElement {
        let mut row = div().flex().flex_wrap().gap_1();
        for (text, color) in badges {
            row = row.child(div().px_1p5().py_0p5().rounded_sm().bg(hex_alpha(color, 0.18)).t_caption().text_color(hex(color)).child(text));
        }
        row
    }

    /// Project links and the app a plugin is for ("Cosmica" → where to get it).
    fn render_project(&self, manifest: &Manifest, cx: &mut Context<Self>) -> impl IntoElement {
        let link = |id: SharedString, label: String, url: String, tone: u32, cx: &mut Context<Self>| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_1()
                .px_1p5()
                .py_0p5()
                .rounded_sm()
                .cursor_pointer()
                .bg(hex_alpha(tone, 0.12))
                .t_caption()
                .text_color(hex(tone))
                .hover(|s| s.bg(hex_alpha(tone, 0.22)))
                .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| cx.open_url(&url)))
                .child(label)
                .child(icon("external-link", 11., hex(tone)))
        };
        let mut row = div().flex().flex_wrap().items_center().gap_1();
        let mut links: Vec<(String, String)> = manifest.links.iter().map(|l| (l.label.clone(), l.url.clone())).collect();
        if let Some(homepage) = manifest.homepage.clone() {
            if !links.iter().any(|(_, url)| *url == homepage) {
                links.insert(0, (t(cx, "plugins.homepage").to_string(), homepage));
            }
        }
        for (index, (label, url)) in links.into_iter().enumerate() {
            row = row.child(link(SharedString::from(format!("plugin-link-{}-{index}", manifest.id)), label, url, Chrome::BLUE, cx));
        }
        // What the plugin talks to, and where to get it when it isn't here yet.
        if let Some(requires) = manifest.requires.clone() {
            let found = manifest.detected();
            let tone = if found { Chrome::SUCCESS } else { Chrome::WARNING };
            let label = if found {
                tf(cx, "plugins.requires_found", &[("name", &requires.name)])
            } else {
                tf(cx, "plugins.requires_missing", &[("name", &requires.name)])
            };
            row = row.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_1p5()
                    .py_0p5()
                    .rounded_sm()
                    .bg(hex_alpha(tone, 0.12))
                    .t_caption()
                    .text_color(hex(tone))
                    .child(icon(if found { "circle-check" } else { "info" }, 11., hex(tone)))
                    .child(label),
            );
            if let Some(url) = requires.url.filter(|_| !found) {
                row = row.child(link(
                    SharedString::from(format!("plugin-get-{}", manifest.id)),
                    tf(cx, "plugins.get_app", &[("name", &requires.name)]),
                    url,
                    Chrome::ACCENT,
                    cx,
                ));
            }
        }
        row.when_some(manifest.requires.as_ref().and_then(|r| r.note.clone()), |d, note| {
            d.child(div().w_full().t_caption().text_color(hex(Chrome::MUTED)).child(note))
        })
    }

    fn render_capabilities(&self, manifest: &Manifest, cx: &mut Context<Self>) -> impl IntoElement {
        let mut details = div().flex().flex_col().gap_1();
        if !manifest.permissions.is_empty() {
            let mut list = div().flex().flex_wrap().gap_1();
            for permission in &manifest.permissions {
                let key = match permission.as_str() {
                    "prompt.inject" => "plugins.perm.prompt",
                    "terminal.write" => "plugins.perm.terminal",
                    "session.read" => "plugins.perm.session",
                    "workspace.read" => "plugins.perm.workspace",
                    _ => "plugins.perm.unknown",
                };
                debug_assert!(PERMISSIONS.iter().any(|(name, _)| name == permission));
                list = list.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_1p5()
                        .py_0p5()
                        .rounded_sm()
                        .bg(hex(0x2a2a2a))
                        .t_caption()
                        .text_color(hex(Chrome::FOREGROUND))
                        .child(icon("shield-alert", 11., hex(Chrome::WARNING)))
                        .child(t(cx, key)),
                );
            }
            details = details.child(list);
        }
        let commands: Vec<String> = manifest.contributes.commands.iter().map(|c| c.title.clone()).collect();
        if !commands.is_empty() || manifest.contributes.panel.is_some() {
            let mut parts = Vec::new();
            if manifest.contributes.panel.is_some() {
                parts.push(t(cx, "plugins.has_panel").to_string());
            }
            parts.extend(commands);
            details = details.child(div().t_caption().text_color(hex(Chrome::MUTED)).child(parts.join(" · ")));
        }
        details
    }

    fn render_catalog_plugin(&self, manifest: Manifest, cx: &mut Context<Self>) -> AnyElement {
        let mut badges = vec![(t(cx, "plugins.builtin").to_string(), Chrome::BLUE)];
        if manifest.detected() {
            badges.insert(0, (t(cx, "plugins.detected").to_string(), Chrome::SUCCESS));
        }
        let focused = self.plugins_page.focus.as_deref() == Some(manifest.id.as_str());
        let open = self.expanded(&manifest.id);
        let id = manifest.id.clone();
        self.card(&manifest.id, focused)
            .child(self.render_card_head(&manifest, cx))
            .child(self.render_badges(badges))
            .child(self.render_summary(&manifest.description, open))
            .when(open, |d| d.child(self.render_project(&manifest, cx)).child(self.render_capabilities(&manifest, cx)))
            .child(div().flex().gap_2().child(action_button(
                SharedString::from(format!("plugin-install-{}", manifest.id)),
                t(cx, "plugins.install"),
                cx.listener(move |this, _: &ClickEvent, window, cx| this.install_builtin_plugin(id.clone(), window, cx)),
            )))
            .into_any_element()
    }

    /// The description: one short line on a closed tile, everything once it is open.
    fn render_summary(&self, description: &str, open: bool) -> impl IntoElement {
        let text = match open || description.chars().count() <= SUMMARY_CHARS {
            true => description.to_string(),
            false => format!("{}…", description.chars().take(SUMMARY_CHARS).collect::<String>().trim_end()),
        };
        div().t_small().text_color(hex(Chrome::FOREGROUND)).child(text)
    }

    fn render_installed_plugin(&self, plugin: &InstalledPlugin, cx: &mut Context<Self>) -> AnyElement {
        let focused = self.plugins_page.focus.as_deref() == Some(plugin.id.as_str());
        let Some(manifest) = plugin.manifest.clone() else {
            let id = plugin.id.clone();
            return self
                .card(&plugin.id, focused)
                .child(div().truncate().t_body().font_weight(FontWeight::SEMIBOLD).text_color(hex(Chrome::BRIGHT)).child(plugin.id.clone()))
                .child(div().t_caption().text_color(hex(Chrome::ERROR)).child(plugin.error.clone().unwrap_or_default()))
                .child(div().truncate().t_caption().text_color(hex(Chrome::MUTED)).child(tilde(&plugin.dir)))
                .child(div().flex().gap_2().child(action_button(
                    SharedString::from(format!("plugin-remove-{}", plugin.id)),
                    t(cx, "plugins.uninstall"),
                    cx.listener(move |this, _: &ClickEvent, _, cx| this.uninstall_plugin(&id, cx)),
                )))
                .into_any_element();
        };
        let runtime = plugins::runtime(cx, &plugin.id);
        let state = runtime.map(|r| r.state.clone()).unwrap_or(RunState::Stopped);
        let (state_text, state_color) = match (&state, plugin.enabled) {
            (_, false) => (t(cx, "plugins.disabled").to_string(), Chrome::MUTED),
            (RunState::Running, _) => (t(cx, "plugins.running").to_string(), Chrome::SUCCESS),
            (RunState::Starting, _) => (t(cx, "plugins.starting").to_string(), Chrome::ORANGE),
            (RunState::Failed(_), _) => (t(cx, "plugins.failed_short").to_string(), Chrome::ERROR),
            (RunState::Stopped, _) => (t(cx, "plugins.idle").to_string(), Chrome::MUTED),
        };
        let source_key = match plugin.source {
            Source::Builtin => "plugins.builtin",
            Source::Folder => "plugins.source_folder",
            Source::Git => "plugins.source_git",
            Source::Local => "plugins.source_local",
            Source::Dev => "plugins.source_dev",
        };
        let badges = vec![(t(cx, source_key).to_string(), Chrome::BLUE), (state_text, state_color)];
        let id = plugin.id.clone();
        let enabled = plugin.enabled;
        let logs_open = self.plugins_page.logs_open.as_deref() == Some(plugin.id.as_str());
        let confirming = self.plugins_page.confirm_uninstall.as_deref() == Some(plugin.id.as_str());
        let has_panel = manifest.contributes.panel.is_some();
        let update = store::builtin_update_available(plugin);

        let button_id = |name: &str| SharedString::from(format!("plugin-{name}-{}", plugin.id));
        // A closed tile keeps the everyday buttons; the rest appear when the card is opened.
        let mut primary = div().flex().flex_wrap().gap_1p5();
        primary = primary.child(action_button(button_id("toggle"), t(cx, if enabled { "plugins.disable" } else { "plugins.enable" }), {
            let id = id.clone();
            cx.listener(move |this, _: &ClickEvent, _, cx| this.set_plugin_enabled(&id, !enabled, cx))
        }));
        if enabled && has_panel {
            let id = id.clone();
            primary = primary.child(action_button(
                button_id("panel"),
                t(cx, "plugins.open_panel"),
                cx.listener(move |this, _: &ClickEvent, _, cx| this.open_plugin_panel(&id, cx)),
            ));
        }
        if update {
            let id = id.clone();
            primary = primary.child(action_button(
                button_id("update"),
                t(cx, "plugins.update"),
                cx.listener(move |this, _: &ClickEvent, window, cx| {
                    plugins::stop(&id, cx);
                    this.install_builtin_plugin(id.clone(), window, cx)
                }),
            ));
        }
        let mut buttons = div().flex().flex_wrap().gap_1p5();
        if enabled {
            let id = id.clone();
            buttons = buttons.child(action_button(
                button_id("restart"),
                t(cx, "plugins.restart"),
                cx.listener(move |_, _: &ClickEvent, _, cx| plugins::restart(&id, cx)),
            ));
        }
        buttons = buttons.child(action_button(button_id("logs"), t(cx, if logs_open { "plugins.hide_logs" } else { "plugins.logs" }), {
            let id = id.clone();
            cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.plugins_page.logs_open = if logs_open { None } else { Some(id.clone()) };
                cx.notify();
            })
        }));
        buttons = buttons.child(action_button(button_id("reveal"), t(cx, "plugins.reveal"), {
            let dir = plugin.dir.clone();
            cx.listener(move |_, _: &ClickEvent, _, _| {
                crate::platform::open_folder(&dir);
            })
        }));
        if plugin.source == Source::Local || plugin.source == Source::Dev {
            let dir = plugin.dir.clone();
            buttons = buttons.child(action_button(
                button_id("develop"),
                t(cx, "plugins.develop_ai"),
                cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.page = None;
                    this.request_launch_in(PaneKind::Claude, dir.clone(), window, cx);
                }),
            ));
        }
        buttons = buttons.child(
            action_button(button_id("uninstall"), t(cx, if confirming { "plugins.confirm_uninstall" } else { "plugins.uninstall" }), {
                let id = id.clone();
                cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if confirming {
                        this.uninstall_plugin(&id, cx);
                    } else {
                        this.plugins_page.confirm_uninstall = Some(id.clone());
                        cx.notify();
                    }
                })
            })
            .when(confirming, |d| d.bg(hex(Chrome::ERROR)).text_color(hex(Chrome::BRIGHT))),
        );

        let logs = logs_open.then(|| {
            let lines: Vec<String> = runtime.map(|r| r.logs.iter().rev().take(80).rev().cloned().collect()).unwrap_or_default();
            div()
                .id(button_id("log-view"))
                .max_h(px(220.))
                .overflow_y_scroll()
                .p_2()
                .rounded_md()
                .bg(hex(0x141414))
                .t_caption()
                .font_family("JetBrains Mono")
                .text_color(hex(Chrome::FOREGROUND))
                .child(if lines.is_empty() { t(cx, "plugins.no_logs").to_string() } else { lines.join("\n") })
        });
        let failure = match &state {
            RunState::Failed(error) if enabled => Some(div().t_caption().text_color(hex(Chrome::ERROR)).child(error.clone())),
            _ => None,
        };

        let open = self.expanded(&plugin.id);
        self.card(&plugin.id, focused)
            .child(self.render_card_head(&manifest, cx))
            .child(self.render_badges(badges))
            .when(!manifest.description.is_empty(), |d| d.child(self.render_summary(&manifest.description, open)))
            .when(open, |d| {
                d.child(self.render_project(&manifest, cx))
                    .child(self.render_capabilities(&manifest, cx))
                    .child(div().t_caption().text_color(hex(Chrome::MUTED)).child(tilde(&plugin.dir)))
            })
            .children(failure)
            .child(primary)
            .when(open, |d| d.child(buttons))
            .children(logs.filter(|_| open))
            .into_any_element()
    }

    /// A new tab (or workspace when none is open) running `kind` in `dir`.
    fn request_launch_in(&mut self, kind: PaneKind, dir: std::path::PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.launch(kind.into(), LaunchTarget::NewWorkspace, dir, window, cx);
    }
}
