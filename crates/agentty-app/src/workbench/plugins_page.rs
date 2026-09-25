//! Plugins page: a list of plugins on the left, everything about the one you picked on the right
//! — what it adds, what it may reach, where it came from and what it is logging — plus installing
//! from the catalog, a folder or Git, and creating a new plugin with an AI agent.

use super::{LaunchTarget, Page, Workbench};
use crate::i18n::{t, tf};
use crate::launch::{LaunchSpec, PaneKind};
use crate::plugins::{self, RunState};
use crate::settings::settings;
use crate::text_input::TextInput;
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{action_button, chip, hint, icon, icon_named, tilde, IconSize, TypeScale};
use agentty_bridge::model::Agent;
use agentty_bridge::plugins::manifest::{Manifest, Runtime, Surface, PERMISSIONS};
use agentty_bridge::plugins::market;
use agentty_bridge::plugins::store::{self, InstalledPlugin, Source};
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, Entity, FontWeight, PathPromptOptions, SharedString, Window};

/// First message to the agent that builds a new plugin. Prompts are English whatever the UI
/// language is; `{language}` tells the agent which one to talk in.
const AI_PROMPT: &str = "Build the Agentty plugin \"{name}\" in this folder. It should: {idea}\n\nRead CLAUDE.md and PLUGIN_GUIDE.md first. Keep agentty-plugin.json in sync with main.mjs, check the code with `node --check main.mjs`, and when you are done tell me to press Restart on the plugin in Agentty's Plugins page.\n\nTalk to me in {language}.";
const AI_PROMPT_ASK: &str = "We are building the Agentty plugin \"{name}\" in this folder. Read CLAUDE.md and PLUGIN_GUIDE.md, then ask me what the plugin should do.\n\nTalk to me in {language}.";

/// Width of the list beside the details.
const LIST_WIDTH: f32 = 290.;
/// How much of the description a list row shows.
const SUMMARY_CHARS: usize = 46;

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum Tab {
    #[default]
    Details,
    Permissions,
    Logs,
}

/// The marketplace list, as the page has it.
#[derive(Default)]
enum Market {
    #[default]
    Idle,
    Loading,
    Ready(Vec<market::Entry>),
    /// The list kept from last time, while a fresh one is read — or because none could be.
    Stale(Vec<market::Entry>, String),
    Failed(String),
}

impl Market {
    /// What is in hand, whether it was just read or kept from last time.
    fn entries(&self) -> &[market::Entry] {
        match self {
            Market::Ready(entries) | Market::Stale(entries, _) => entries,
            _ => &[],
        }
    }
}

#[derive(Default)]
pub struct PluginsPage {
    /// Plugin to select and scroll to (from a link or the panel's settings button).
    pub focus: Option<String>,
    /// (text, is error)
    pub message: Option<(String, bool)>,
    /// A link that needs this built-in plugin: (plugin id, link), opened after installing it.
    pub pending_link: Option<(String, String)>,
    /// The plugin whose details fill the right-hand side.
    pub selected: Option<String>,
    /// The right-hand side shows the "new plugin" form instead of a plugin.
    creating: bool,
    tab: Tab,
    confirm_uninstall: Option<String>,
    /// A plugin whose update wants more than it had, shown once before it is taken.
    confirm_update: Option<String>,
    busy: bool,
    inputs: Option<Inputs>,
    list_scroll: gpui::ScrollHandle,
    detail_scroll: gpui::ScrollHandle,
    market: Market,
    /// The marketplace plugin being downloaded.
    installing: Option<String>,
    /// Plugins waiting their turn while "update everything" works through them.
    update_queue: Vec<String>,
}

struct Inputs {
    search: Entity<TextInput>,
    git_url: Entity<TextInput>,
    name: Entity<TextInput>,
    idea: Entity<TextInput>,
}

/// Where a row of the list comes from.
#[derive(PartialEq, Eq, Clone, Copy)]
enum Origin {
    Installed,
    /// Shipped inside Agentty.
    Builtin,
    /// Offered by the marketplace.
    Market,
}

/// One row of the list.
struct Row {
    id: String,
    name: String,
    version: String,
    publisher: String,
    description: String,
    /// What else this plugin is called: searched, never shown.
    keywords: String,
    icon: &'static str,
    /// The plugin's own logo on disk, preferred over `icon` — the same mark the detail card draws,
    /// so a row and its card never show two different things for one plugin.
    logo: Option<std::path::PathBuf>,
    origin: Origin,
}

impl Workbench {
    pub(super) fn prepare_plugins_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.page != Some(Page::Plugins) || self.plugins_page.inputs.is_some() {
            return;
        }
        self.fetch_market(cx);
        self.plugins_page.inputs = Some(Inputs {
            search: cx.new(|cx| TextInput::localized("", "plugins.search", window, cx)),
            git_url: cx.new(|cx| TextInput::localized("", "plugins.git_placeholder", window, cx)),
            name: cx.new(|cx| TextInput::localized("", "plugins.name_placeholder", window, cx)),
            idea: cx.new(|cx| TextInput::localized("", "plugins.idea_placeholder", window, cx)),
        });
    }

    /// Reads the marketplace list in the background. Called when the page opens and on Refresh.
    fn fetch_market(&mut self, cx: &mut Context<Self>) {
        if matches!(self.plugins_page.market, Market::Loading) {
            return;
        }
        // What was read last time, so the page is not empty while the network answers.
        self.plugins_page.market = match market::cached() {
            Some((entries, at)) => Market::Stale(entries, ago(at, cx)),
            None => Market::Loading,
        };
        let task = cx.background_spawn(async move { market::fetch() });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.plugins_page.market = match (result, std::mem::take(&mut this.plugins_page.market)) {
                    (Ok(entries), _) => Market::Ready(entries),
                    // The network failed, but last time's list is still worth showing.
                    (Err(err), Market::Stale(entries, _)) => Market::Stale(entries, format!("{err:#}")),
                    (Err(err), _) => Market::Failed(format!("{err:#}")),
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn market_entries(&self) -> &[market::Entry] {
        self.plugins_page.market.entries()
    }

    /// A marketplace entry newer than what is installed.
    fn market_update(&self, plugin: &InstalledPlugin) -> Option<&market::Entry> {
        self.market_entry(&plugin.id).filter(|entry| entry.newer_than(plugin))
    }

    /// Starts the next plugin waiting in an "update everything", if there is one.
    fn next_queued_update(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(next) = self.plugins_page.update_queue.pop() else { return };
        plugins::stop(&next, cx);
        self.install_from_market(next, window, cx);
    }

    /// "Update all": takes every update that asks for nothing new, one after another.
    ///
    /// A plugin that wants more than it had is left out — the user sees what it is asking for on
    /// its own row and answers that. Pressing this again while a round is running would build a
    /// second queue over the first and stop a plugin whose turn would never come.
    pub(super) fn update_everything(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.plugins_page.installing.is_some() || !self.plugins_page.update_queue.is_empty() {
            return;
        }
        let (mut waiting, asking) = self.updates_without_new_permissions(cx);
        let first = waiting.pop();
        self.plugins_page.update_queue = waiting;
        if asking > 0 {
            self.plugins_message(tf(cx, "plugins.update_all_asking", &[("n", &asking.to_string())]), false, cx);
        }
        let Some(first) = first else { return };
        plugins::stop(&first, cx);
        self.install_from_market(first, window, cx);
    }

    /// The updates that ask for nothing new, and how many were left out because they do.
    fn updates_without_new_permissions(&self, cx: &Context<Self>) -> (Vec<String>, usize) {
        let mut safe = Vec::new();
        let mut asking = 0;
        for plugin in plugins::host(cx).installed.iter().filter(|p| self.market_update(p).is_some()) {
            if self.permissions_gained(plugin).is_empty() {
                safe.push(plugin.id.clone());
            } else {
                asking += 1;
            }
        }
        (safe, asking)
    }

    /// Every installed plugin the marketplace has a newer version of.
    fn market_updates(&self, cx: &Context<Self>) -> Vec<String> {
        plugins::host(cx).installed.iter().filter(|plugin| self.market_update(plugin).is_some()).map(|plugin| plugin.id.clone()).collect()
    }

    fn market_entry(&self, id: &str) -> Option<&market::Entry> {
        self.market_entries().iter().find(|entry| entry.id == id)
    }

    /// Downloads a marketplace plugin and installs it once its checksum matches.
    fn install_from_market(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.market_entry(&id).cloned() else {
            // It is no longer offered: the rest of a queue still gets its turn.
            return self.next_queued_update(window, cx);
        };
        if !entry.supported() {
            // The page does not offer it, but a queue built before the list was read again might.
            self.plugins_message(t(cx, "plugins.needs_newer").to_string(), true, cx);
            return self.next_queued_update(window, cx);
        }
        if self.plugins_page.installing.is_some() {
            // Something else is already downloading. Every caller stops the plugin before asking
            // for this, so dropping it here would leave a plugin stopped for an update that never
            // comes: it waits its turn instead, and the install in flight starts it when it ends.
            if !self.plugins_page.update_queue.contains(&id) {
                self.plugins_page.update_queue.push(id);
            }
            return;
        }
        self.plugins_page.installing = Some(id.clone());
        self.plugins_message(tf(cx, "plugins.downloading", &[("name", &entry.name)]), false, cx);
        let task = cx.background_spawn(async move { market::install(&entry) });
        let window = window.window_handle();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = cx.update_window(window, |_, window, cx| {
                let _ = this.update(cx, |this, cx| {
                    this.plugins_page.installing = None;
                    this.after_install(result, window, cx);
                    // "Update everything": the next one starts when this one is done.
                    this.next_queued_update(window, cx);
                });
            });
        })
        .detach();
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
                self.select_plugin(plugin.id.clone(), cx);
                self.plugins_message(tf(cx, "plugins.installed_ok", &[("name", &name)]), false, cx);
                if let Some((_, link)) = self.plugins_page.pending_link.clone().filter(|(id, _)| *id == plugin.id) {
                    self.plugins_page.pending_link = None;
                    self.open_agentty_link(&link, window, cx);
                }
            }
            Err(err) => self.plugins_message(format!("{err:#}"), true, cx),
        }
    }

    /// The debug driver's `plugin-folder`: finishing an install the same way the button does.
    pub(super) fn after_install_debug(&mut self, result: anyhow::Result<InstalledPlugin>, window: &mut Window, cx: &mut Context<Self>) {
        self.after_install(result, window, cx);
    }

    /// The debug driver's `plugin-market`: the marketplace's buttons without the mouse, so the
    /// install, update and "update everything" paths can be driven end to end in a test.
    pub(super) fn debug_market(&mut self, argument: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (command, rest) = argument.split_once(' ').unwrap_or((argument, ""));
        match command.trim() {
            "refresh" => self.fetch_market(cx),
            "install" => {
                let id = rest.trim().to_string();
                plugins::stop(&id, cx);
                self.install_from_market(id, window, cx);
            }
            // The button's own path, so what a test drives is what a user presses.
            "update-all" => self.update_everything(window, cx),
            "uninstall" => self.uninstall_plugin(rest.trim(), cx),
            other => eprintln!("plugin-market: no such command {other:?}"),
        }
        eprintln!(
            "plugin-market: market={} installing={:?} queue={:?} message={:?}",
            match &self.plugins_page.market {
                Market::Idle => "idle".to_string(),
                Market::Loading => "loading".to_string(),
                Market::Ready(entries) => format!("ready({})", entries.len()),
                Market::Stale(entries, why) => format!("stale({}, {why})", entries.len()),
                Market::Failed(err) => format!("failed({err})"),
            },
            self.plugins_page.installing,
            self.plugins_page.update_queue,
            self.plugins_page.message,
        );
    }

    /// The debug driver's `plugin-enable`: the switch on the Plugins page, without the mouse.
    pub(super) fn set_plugin_enabled_debug(&mut self, id: &str, enabled: bool, cx: &mut Context<Self>) {
        self.set_plugin_enabled(id, enabled, cx);
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
        // The address may carry a token; what is put on the page is what anyone looking over a
        // shoulder, or at a screenshot, sees.
        let shown = agentty_bridge::extensions::redact_url(&url);
        self.plugins_message(tf(cx, "plugins.cloning", &[("name", &shown)]), false, cx);
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
                if !enabled {
                    self.drop_plugin_panel(id, cx);
                }
                plugins::reload(cx);
            }
            Err(err) => self.plugins_message(format!("{err:#}"), true, cx),
        }
        cx.notify();
    }

    /// What the marketplace's version of a plugin asks for that the installed one did not.
    ///
    /// An update is one click, and it writes the new entry's permissions over the old manifest
    /// while keeping the plugin enabled. Without this, a plugin published with none could ask for
    /// every one of them in its next version and be granted them by a user pressing "Update".
    fn permissions_gained(&self, plugin: &InstalledPlugin) -> Vec<String> {
        let Some(entry) = self.market_update(plugin) else { return Vec::new() };
        let had = plugin.manifest.as_ref().map(|m| m.permissions.clone()).unwrap_or_default();
        entry.permissions.iter().filter(|wanted| !had.contains(wanted)).cloned().collect()
    }

    fn uninstall_plugin(&mut self, id: &str, cx: &mut Context<Self>) {
        self.plugins_page.confirm_uninstall = None;
        // Read before it goes: afterwards there is nothing left to ask what it was called.
        let name = plugins::plugin(cx, id).map_or_else(|| id.to_string(), |plugin| plugin.name().to_string());
        plugins::stop(id, cx);
        self.drop_plugin_panel(id, cx);
        match store::uninstall(id) {
            Ok(()) => {
                // Its browser profiles (and the accounts signed in there) go with it, and so does
                // what the user allowed it.
                crate::browser_profiles::remove_all(id);
                super::plugin_browser::revoke(id, cx);
                let gone = id.to_string();
                crate::settings::update_settings(cx, move |s| {
                    s.plugin_browser_modes.remove(&gone);
                    s.plugin_permission_grants.remove(&gone);
                });
                plugins::reload(cx);
                if self.plugins_page.selected.as_deref() == Some(id) {
                    self.plugins_page.selected = None;
                }
                self.plugins_message(tf(cx, "plugins.uninstalled", &[("name", &name)]), false, cx);
            }
            Err(err) => self.plugins_message(format!("{err:#}"), true, cx),
        }
    }

    fn select_plugin(&mut self, id: String, cx: &mut Context<Self>) {
        self.plugins_page.selected = Some(id);
        self.plugins_page.creating = false;
        self.plugins_page.tab = Tab::Details;
        self.plugins_page.confirm_uninstall = None;
        self.plugins_page.confirm_update = None;
        cx.notify();
    }

    // ------------------------------------------------------------------ page

    pub(super) fn render_plugins_page(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let page = &self.plugins_page;
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
        let installed = plugins::host(cx).installed.clone();
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
                    .child(self.render_plugins_header(cx))
                    .children(pending)
                    .children(message),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(self.render_plugin_list(&installed, cx))
                    .child(self.render_plugin_detail(&installed, cx)),
            )
    }

    fn render_plugins_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap_3()
            .child(div().t_heading().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(t(cx, "page.plugins")))
            .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "plugins.subtitle")))
            .child(div().flex_1())
            .children(self.render_update_all(cx))
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
                    this.fetch_market(cx);
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
            ))
    }

    /// "3 updates" and the button that takes them all, when the marketplace has newer versions.
    fn render_update_all(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let updates = self.market_updates(cx);
        if updates.is_empty() {
            return None;
        }
        let count = updates.len().to_string();
        Some(
            action_button(
                "plugins-update-all",
                tf(cx, "plugins.update_all", &[("n", &count)]),
                cx.listener(move |this, _: &ClickEvent, window, cx| this.update_everything(window, cx)),
            )
            .bg(hex_alpha(Chrome::ORANGE, 0.22))
            .text_color(hex(Chrome::ORANGE)),
        )
    }

    /// A text field of the page (search, Git URL, name, idea).
    fn plugins_field(&self, input: Option<&Entity<TextInput>>) -> gpui::Div {
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
    }

    // ------------------------------------------------------------------ list

    fn search_text(&self, cx: &Context<Self>) -> String {
        self.plugins_page.inputs.as_ref().map(|i| i.search.read(cx).text().trim().to_lowercase()).unwrap_or_default()
    }

    fn render_plugin_list(&self, installed: &[InstalledPlugin], cx: &mut Context<Self>) -> impl IntoElement {
        let page = &self.plugins_page;
        let search = self.search_text(cx);
        let matches = |row: &Row| {
            search.is_empty()
                || row.name.to_lowercase().contains(&search)
                || row.id.contains(&search)
                || row.description.to_lowercase().contains(&search)
                || row.keywords.contains(&search)
        };
        let installed_rows: Vec<Row> = installed.iter().map(Row::installed).filter(matches).collect();
        let catalog_rows: Vec<Row> = store::BUILTIN
            .iter()
            .map(|builtin| builtin.manifest())
            .filter(|manifest| !installed.iter().any(|p| p.id == manifest.id))
            .map(|manifest| Row::catalog(&manifest))
            .filter(matches)
            .collect();

        let section = |title: String| {
            div().px_3().pt_3().pb_1().t_caption().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::MUTED)).child(title)
        };
        let mut list = div().w_full().flex().flex_col();
        list = list.child(section(tf(cx, "plugins.installed", &[("n", &installed_rows.len().to_string())])));
        if installed_rows.is_empty() {
            list = list.child(hint(t(cx, if search.is_empty() { "plugins.none_installed" } else { "plugins.no_matches" })));
        }
        for row in &installed_rows {
            list = list.child(self.render_plugin_row(row, cx));
        }
        if !catalog_rows.is_empty() {
            list = list.child(section(t(cx, "plugins.catalog").to_string()));
            for row in &catalog_rows {
                list = list.child(self.render_plugin_row(row, cx));
            }
        }

        // The marketplace: what is offered but not installed here yet.
        let market_rows: Vec<Row> = self
            .market_entries()
            .iter()
            .filter(|entry| !installed.iter().any(|plugin| plugin.id == entry.id))
            .map(Row::market)
            .filter(matches)
            .collect();
        list = list.child(section(t(cx, "plugins.market").to_string()));
        match &page.market {
            Market::Loading | Market::Idle => list = list.child(hint(t(cx, "plugins.market_loading"))),
            Market::Stale(_, note) => list = list.child(div().px_3().pb_1().t_caption().text_color(hex(Chrome::MUTED)).child(note.clone())),
            Market::Failed(error) => {
                list = list
                    .child(hint(t(cx, "plugins.market_failed")))
                    .child(div().px_3().pb_1().t_caption().text_color(hex(Chrome::MUTED)).child(error.clone()))
            }
            Market::Ready(_) if market_rows.is_empty() => {
                list = list.child(hint(t(cx, if search.is_empty() { "plugins.market_empty" } else { "plugins.no_matches" })))
            }
            Market::Ready(_) => {}
        }
        for row in &market_rows {
            list = list.child(self.render_plugin_row(row, cx));
        }

        div()
            .w(px(LIST_WIDTH))
            .flex_shrink_0()
            .h_full()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(hex(Chrome::BORDER))
            .bg(hex(Chrome::SIDE_BAR))
            .child(div().p_2().flex_shrink_0().flex().child(self.plugins_field(page.inputs.as_ref().map(|i| &i.search))))
            .child(
                div()
                    .id("plugins-list-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&page.list_scroll)
                    .relative()
                    .group(crate::ui::SCROLL_GROUP)
                    .child(list)
                    .child(crate::ui::scrollbar(page.list_scroll.clone())),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .p_2()
                    .flex()
                    .flex_col()
                    .gap_1p5()
                    .border_t_1()
                    .border_color(hex(Chrome::BORDER))
                    .child(
                        div()
                            .id("plugins-new")
                            .px_2()
                            .py_1p5()
                            .flex()
                            .items_center()
                            .gap_2()
                            .rounded_md()
                            .cursor_pointer()
                            .when(page.creating, |d| d.bg(hex(Chrome::SELECTED)))
                            .hover(|s| s.bg(hex(Chrome::HOVER)))
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.plugins_page.creating = true;
                                this.plugins_page.selected = None;
                                cx.notify();
                            }))
                            .child(icon("wand-sparkles", IconSize::INLINE, hex(Chrome::PURPLE)))
                            .child(div().flex_1().t_small().text_color(hex(Chrome::BRIGHT)).child(t(cx, "plugins.new"))),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_1p5()
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
                    ),
            )
    }

    fn render_plugin_row(&self, row: &Row, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.plugins_page.selected.as_deref() == Some(row.id.as_str());
        let focused = self.plugins_page.focus.as_deref() == Some(row.id.as_str());
        let id = row.id.clone();
        let state = (row.origin == Origin::Installed).then(|| self.plugin_state(&row.id, cx));
        let summary = match row.description.chars().count() > SUMMARY_CHARS {
            true => format!("{}…", row.description.chars().take(SUMMARY_CHARS).collect::<String>().trim_end()),
            false => row.description.clone(),
        };
        let update = plugins::plugin(cx, &row.id).and_then(|plugin| self.market_update(plugin).map(|entry| entry.version.clone()));
        let tag = match (row.origin, update) {
            (_, Some(version)) => Some((tf(cx, "plugins.update_to", &[("version", &version)]), Chrome::ORANGE)),
            (Origin::Builtin, _) => Some((t(cx, "plugins.builtin").to_string(), Chrome::BLUE)),
            (Origin::Market, _) => Some((t(cx, "plugins.market").to_string(), Chrome::PURPLE)),
            (Origin::Installed, _) => None,
        };
        div()
            .id(SharedString::from(format!("plugin-row-{}", row.id)))
            .w_full()
            .overflow_hidden()
            .px_3()
            .py_2()
            .flex()
            .gap_2p5()
            .cursor_pointer()
            .border_l_2()
            .border_color(if selected || focused { hex(Chrome::ACCENT) } else { hex_alpha(0, 0.) })
            .when(selected, |d| d.bg(hex(Chrome::SELECTED)))
            .hover(|s| s.bg(hex(Chrome::HOVER)))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.select_plugin(id.clone(), cx)))
            .child(
                div()
                    .size(px(32.))
                    .flex_shrink_0()
                    .rounded_md()
                    .bg(hex(0x2a2a2a))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(crate::ui::plugin_mark(row.logo.clone(), row.icon, IconSize::BUTTON, hex(Chrome::BRIGHT))),
            )
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
                            .gap_1p5()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .t_body()
                                    .font_weight(crate::theme::EMPHASIS)
                                    .text_color(hex(Chrome::BRIGHT))
                                    .child(row.name.clone()),
                            )
                            .child(div().flex_shrink_0().t_caption().text_color(hex(Chrome::MUTED)).child(format!("v{}", row.version))),
                    )
                    .child(div().truncate().t_caption().text_color(hex(Chrome::MUTED)).child(summary))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .when(!row.publisher.is_empty(), |d| {
                                d.child(div().t_caption().text_color(hex(Chrome::MUTED)).child(row.publisher.clone()))
                            })
                            .when_some(state, |d, (text, color)| {
                                d.child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .child(dot(color))
                                        .child(div().t_caption().text_color(hex(color)).child(text)),
                                )
                            })
                            .when_some(tag, |d, (label, color)| d.child(div().t_caption().text_color(hex(color)).child(label))),
                    ),
            )
    }

    /// The running state of an installed plugin, as words and a colour.
    fn plugin_state(&self, id: &str, cx: &Context<Self>) -> (String, u32) {
        let enabled = plugins::plugin(cx, id).is_some_and(|plugin| plugin.enabled);
        let state = plugins::runtime(cx, id).map(|r| r.state.clone()).unwrap_or(RunState::Stopped);
        match (&state, enabled) {
            (_, false) => (t(cx, "plugins.disabled").to_string(), Chrome::MUTED),
            (RunState::Running, _) => (t(cx, "plugins.running").to_string(), Chrome::SUCCESS),
            (RunState::Starting, _) => (t(cx, "plugins.starting").to_string(), Chrome::ORANGE),
            (RunState::Failed(_), _) => (t(cx, "plugins.failed_short").to_string(), Chrome::ERROR),
            (RunState::Stopped, _) => (t(cx, "plugins.idle").to_string(), Chrome::MUTED),
            (RunState::NeedsConsent, _) => (t(cx, "plugins.consent.state").to_string(), Chrome::ORANGE),
        }
    }

    // ---------------------------------------------------------------- detail

    fn render_plugin_detail(&self, installed: &[InstalledPlugin], cx: &mut Context<Self>) -> impl IntoElement {
        let page = &self.plugins_page;
        let body: AnyElement = if page.creating {
            self.render_create_plugin(cx).into_any_element()
        } else if let Some(plugin) = page.selected.as_ref().and_then(|id| installed.iter().find(|p| p.id == *id)) {
            self.render_installed_detail(plugin, cx)
        } else if let Some(manifest) = page.selected.as_ref().and_then(|id| store::builtin(id)).map(|builtin| builtin.manifest()) {
            self.render_catalog_detail(&manifest, cx)
        } else if let Some(entry) = page.selected.as_ref().and_then(|id| self.market_entry(id)) {
            self.render_market_detail(entry, cx)
        } else {
            div()
                .w_full()
                .p_6()
                .flex()
                .flex_col()
                .gap_2()
                .child(icon("puzzle", 28., hex(Chrome::BORDER)))
                .child(div().t_small().text_color(hex(Chrome::MUTED)).max_w(px(420.)).child(t(cx, "plugins.pick_one")))
                .into_any_element()
        };
        div()
            .id("plugins-detail-scroll")
            .flex_1()
            .min_w_0()
            .h_full()
            // A column: the body stacks from the top and keeps the height of what it holds. As a
            // row (the default) its one child is stretched over the whole page instead.
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .track_scroll(&page.detail_scroll)
            .relative()
            .group(crate::ui::SCROLL_GROUP)
            .child(body)
            .child(crate::ui::scrollbar(page.detail_scroll.clone()))
    }

    /// Icon, name, publisher and version above everything else. `logo` is the plugin's own artwork
    /// when it is on disk, which the mark prefers over the icon name.
    fn render_detail_head(
        &self,
        manifest: &Manifest,
        logo: Option<std::path::PathBuf>,
        badges: Vec<(String, u32)>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let _ = cx;
        div()
            .flex()
            .gap_4()
            .child(
                div()
                    .size(px(56.))
                    .flex_shrink_0()
                    .rounded_lg()
                    .bg(hex(0x2a2a2a))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(crate::ui::plugin_mark(logo, icon_named(manifest.icon.as_deref()), 26., hex(Chrome::BRIGHT))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div().t_heading().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(manifest.name.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .content_start()
                            .gap_2()
                            .when(!manifest.publisher.is_empty(), |d| {
                                d.child(div().t_small().text_color(hex(Chrome::MUTED)).child(manifest.publisher.clone()))
                            })
                            .child(div().t_small().text_color(hex(Chrome::MUTED)).child(format!("v{}", manifest.version)))
                            .child(self.render_badges(badges)),
                    )
                    .when(!manifest.description.is_empty(), |d| {
                        d.child(div().t_small().text_color(hex(Chrome::FOREGROUND)).child(manifest.description.clone()))
                    }),
            )
    }

    /// The little state and source tags.
    fn render_badges(&self, badges: Vec<(String, u32)>) -> impl IntoElement {
        let mut row = div().flex().flex_wrap().items_start().content_start().gap_1();
        for (text, color) in badges {
            row = row.child(div().px_1p5().py_0p5().rounded_sm().bg(hex_alpha(color, 0.18)).t_caption().text_color(hex(color)).child(text));
        }
        row
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = |id: &'static str, key: &'static str, which: Tab, cx: &mut Context<Self>| {
            let active = self.plugins_page.tab == which;
            div()
                .id(id)
                .pb_1p5()
                .px_0p5()
                .cursor_pointer()
                .border_b_2()
                .border_color(if active { hex(Chrome::ACCENT) } else { hex_alpha(0, 0.) })
                .t_caption()
                .font_weight(crate::theme::EMPHASIS)
                .text_color(hex(if active { Chrome::BRIGHT } else { Chrome::MUTED }))
                .hover(|s| s.text_color(hex(Chrome::BRIGHT)))
                .child(t(cx, key).to_uppercase())
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.plugins_page.tab = which;
                    cx.notify();
                }))
        };
        div()
            .flex()
            .gap_4()
            .border_b_1()
            .border_color(hex(Chrome::BORDER))
            .child(tab("plugins-tab-details", "plugins.tab.details", Tab::Details, cx))
            .child(tab("plugins-tab-permissions", "plugins.tab.permissions", Tab::Permissions, cx))
            .child(tab("plugins-tab-logs", "plugins.tab.logs", Tab::Logs, cx))
    }

    fn render_catalog_detail(&self, manifest: &Manifest, cx: &mut Context<Self>) -> AnyElement {
        let mut badges = vec![(t(cx, "plugins.builtin").to_string(), Chrome::BLUE)];
        if manifest.detected() {
            badges.insert(0, (t(cx, "plugins.detected").to_string(), Chrome::SUCCESS));
        }
        let id = manifest.id.clone();
        div()
            .w_full()
            .p_6()
            .max_w(px(900.))
            .flex()
            .flex_col()
            .gap_4()
            .child(self.render_detail_head(manifest, None, badges, cx))
            .child(div().flex().gap_2().child(action_button(
                SharedString::from(format!("plugin-install-{}", manifest.id)),
                t(cx, "plugins.install"),
                cx.listener(move |this, _: &ClickEvent, window, cx| this.install_builtin_plugin(id.clone(), window, cx)),
            )))
            .child(self.render_tabs(cx))
            .child(match self.plugins_page.tab {
                Tab::Permissions => self.render_permissions(manifest, cx).into_any_element(),
                // A plugin that is not installed has nothing to log yet.
                _ => self.render_details_tab(manifest, None, cx).into_any_element(),
            })
            .into_any_element()
    }

    /// A plugin the marketplace offers: what it is, where it comes from, and what installing it
    /// would let it do.
    fn render_market_detail(&self, entry: &market::Entry, cx: &mut Context<Self>) -> AnyElement {
        let manifest = entry.manifest();
        let badges = vec![(t(cx, "plugins.market").to_string(), Chrome::PURPLE)];
        let id = entry.id.clone();
        let installing = self.plugins_page.installing.as_deref() == Some(entry.id.as_str());
        let size = format!("{} KB", entry.module.size.div_ceil(1024));
        let mut info = vec![
            (t(cx, "plugins.info.identifier").to_string(), entry.id.clone()),
            (t(cx, "plugins.info.version").to_string(), entry.version.clone()),
        ];
        if !entry.license.is_empty() {
            info.push((t(cx, "plugins.info.license").to_string(), entry.license.clone()));
        }
        info.push((t(cx, "plugins.info.runtime").to_string(), t(cx, "plugins.runtime.wasm").to_string()));
        info.push((t(cx, "plugins.info.size").to_string(), size));
        info.push((t(cx, "plugins.info.checksum").to_string(), entry.module.sha256.clone()));
        div()
            .w_full()
            .p_6()
            .max_w(px(900.))
            .flex()
            .flex_col()
            .gap_4()
            .child(self.render_detail_head(&manifest, None, badges, cx))
            .child(if entry.supported() {
                div()
                    .flex()
                    .gap_2()
                    .child(action_button(
                        SharedString::from(format!("plugin-market-install-{}", entry.id)),
                        t(cx, if installing { "plugins.installing" } else { "plugins.install" }),
                        cx.listener(move |this, _: &ClickEvent, window, cx| this.install_from_market(id.clone(), window, cx)),
                    ))
                    .into_any_element()
            } else {
                div()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(hex_alpha(Chrome::ORANGE, 0.14))
                    .text_color(hex(Chrome::ORANGE))
                    .t_caption()
                    .child(t(cx, "plugins.needs_newer"))
                    .into_any_element()
            })
            .child(self.render_tabs(cx))
            .child(match self.plugins_page.tab {
                Tab::Permissions => self.render_permissions(&manifest, cx).into_any_element(),
                _ => div()
                    .pt_4()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(self.render_project(&manifest, cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1p5()
                            .child(detail_heading(t(cx, "plugins.info").to_string()))
                            .child(self.render_info_grid(info))
                            .child(div().t_caption().text_color(hex(Chrome::MUTED)).child(t(cx, "plugins.market_note"))),
                    )
                    .into_any_element(),
            })
            .into_any_element()
    }

    fn render_installed_detail(&self, plugin: &InstalledPlugin, cx: &mut Context<Self>) -> AnyElement {
        let Some(manifest) = plugin.manifest.clone() else {
            let id = plugin.id.clone();
            return div()
                .w_full()
                .p_6()
                .flex()
                .flex_col()
                .gap_2()
                .child(div().t_title().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(plugin.id.clone()))
                .child(div().t_small().text_color(hex(Chrome::ERROR)).child(plugin.error.clone().unwrap_or_default()))
                .child(div().t_caption().text_color(hex(Chrome::MUTED)).child(tilde(&plugin.dir)))
                .child(div().flex().gap_2().child(action_button(
                    SharedString::from(format!("plugin-remove-{}", plugin.id)),
                    t(cx, "plugins.uninstall"),
                    cx.listener(move |this, _: &ClickEvent, _, cx| this.uninstall_plugin(&id, cx)),
                )))
                .into_any_element();
        };
        let source_key = source_key(plugin.source);
        let (state_text, state_color) = self.plugin_state(&plugin.id, cx);
        let badges = vec![(t(cx, source_key).to_string(), Chrome::BLUE), (state_text, state_color)];
        let failure = match plugins::runtime(cx, &plugin.id).map(|r| r.state.clone()) {
            Some(RunState::Failed(error)) if plugin.enabled => Some(div().t_small().text_color(hex(Chrome::ERROR)).child(error)),
            _ => None,
        };
        div()
            .w_full()
            .p_6()
            .max_w(px(900.))
            .flex()
            .flex_col()
            .gap_4()
            .child(self.render_detail_head(&manifest, agentty_bridge::plugins::store::logo_file(plugin), badges, cx))
            .children(failure)
            .child(self.render_plugin_actions(plugin, &manifest, cx))
            .child(self.render_tabs(cx))
            .child(match self.plugins_page.tab {
                Tab::Details => self.render_details_tab(&manifest, Some(plugin), cx).into_any_element(),
                Tab::Permissions => self.render_permissions(&manifest, cx).into_any_element(),
                Tab::Logs => self.render_logs(&plugin.id, cx).into_any_element(),
            })
            .into_any_element()
    }

    fn render_plugin_actions(&self, plugin: &InstalledPlugin, manifest: &Manifest, cx: &mut Context<Self>) -> impl IntoElement {
        let id = plugin.id.clone();
        let enabled = plugin.enabled;
        let confirming = self.plugins_page.confirm_uninstall.as_deref() == Some(plugin.id.as_str());
        let button_id = |name: &str| SharedString::from(format!("plugin-{name}-{}", plugin.id));
        let mut row = div().flex().flex_wrap().items_start().content_start().gap_1p5();
        row = row.child(action_button(button_id("toggle"), t(cx, if enabled { "plugins.disable" } else { "plugins.enable" }), {
            let id = id.clone();
            cx.listener(move |this, _: &ClickEvent, _, cx| this.set_plugin_enabled(&id, !enabled, cx))
        }));
        if enabled && manifest.contributes.panel.is_some() {
            let id = id.clone();
            row = row.child(action_button(
                button_id("panel"),
                t(cx, "plugins.open_panel"),
                cx.listener(move |this, _: &ClickEvent, window, cx| this.show_plugin(&id, window, cx)),
            ));
        }
        if store::builtin_update_available(plugin) {
            let id = id.clone();
            row = row.child(action_button(
                button_id("update"),
                t(cx, "plugins.update"),
                cx.listener(move |this, _: &ClickEvent, window, cx| {
                    plugins::stop(&id, cx);
                    this.install_builtin_plugin(id.clone(), window, cx)
                }),
            ));
        }
        // A newer version in the marketplace than the one installed.
        if let Some(version) = self.market_update(plugin).map(|entry| entry.version.clone()) {
            let id = id.clone();
            let gained = self.permissions_gained(plugin);
            let asking = self.plugins_page.confirm_update.as_deref() == Some(plugin.id.as_str());
            let label = match (gained.is_empty(), asking) {
                // It wants more than it had: say what, and take a second press for it.
                (false, false) => {
                    let names: Vec<&str> = gained.iter().map(|p| t(cx, permission_strings(p).0)).collect();
                    tf(cx, "plugins.update_wants", &[("perms", &names.join(", "))])
                }
                (false, true) => t(cx, "plugins.update_confirm").to_string(),
                _ => tf(cx, "plugins.update_to", &[("version", &version)]),
            };
            let wants_more = !gained.is_empty();
            row = row.child(action_button(
                button_id("market-update"),
                label,
                cx.listener(move |this, _: &ClickEvent, window, cx| {
                    if wants_more && this.plugins_page.confirm_update.as_deref() != Some(id.as_str()) {
                        this.plugins_page.confirm_update = Some(id.clone());
                        return cx.notify();
                    }
                    this.plugins_page.confirm_update = None;
                    plugins::stop(&id, cx);
                    this.install_from_market(id.clone(), window, cx);
                }),
            ));
        }
        if enabled {
            let id = id.clone();
            row = row.child(action_button(
                button_id("restart"),
                t(cx, "plugins.restart"),
                cx.listener(move |_, _: &ClickEvent, _, cx| plugins::restart(&id, cx)),
            ));
        }
        row = row.child(action_button(button_id("reveal"), t(cx, "plugins.reveal"), {
            let dir = plugin.dir.clone();
            cx.listener(move |_, _: &ClickEvent, _, _| {
                crate::platform::open_folder(&dir);
            })
        }));
        if plugin.source == Source::Local || plugin.source == Source::Dev {
            let dir = plugin.dir.clone();
            row = row.child(action_button(
                button_id("develop"),
                t(cx, "plugins.develop_ai"),
                cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.page = None;
                    this.request_launch_in(PaneKind::Claude, dir.clone(), window, cx);
                }),
            ));
        }
        row.child(
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
        )
    }

    fn render_details_tab(&self, manifest: &Manifest, plugin: Option<&InstalledPlugin>, cx: &mut Context<Self>) -> impl IntoElement {
        let runtime_key = match manifest.runtime {
            Runtime::Node => "plugins.runtime.node",
            Runtime::Python => "plugins.runtime.python",
            Runtime::Executable => "plugins.runtime.executable",
            Runtime::Wasm => "plugins.runtime.wasm",
        };
        let mut info: Vec<(String, String)> = vec![
            (t(cx, "plugins.info.identifier").to_string(), manifest.id.clone()),
            (t(cx, "plugins.info.version").to_string(), manifest.version.clone()),
        ];
        if !manifest.publisher.is_empty() {
            info.push((t(cx, "plugins.info.publisher").to_string(), manifest.publisher.clone()));
        }
        info.push((t(cx, "plugins.info.runtime").to_string(), t(cx, runtime_key).to_string()));
        if let Some(plugin) = plugin {
            info.push((t(cx, "plugins.info.source").to_string(), t(cx, source_key(plugin.source)).to_string()));
            info.push((t(cx, "plugins.info.folder").to_string(), tilde(&plugin.dir)));
        }

        let mut contributes = div().flex().flex_col().gap_1();
        if let Some(panel) = &manifest.contributes.panel {
            let key = match manifest.surface() {
                Surface::Sidebar => "plugins.surface.sidebar",
                Surface::Pane => "plugins.surface.pane",
                Surface::Status => "plugins.surface.status",
            };
            contributes = contributes.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .child(icon(icon_named(panel.icon.as_deref().or(manifest.icon.as_deref())), 12., hex(Chrome::MUTED)))
                    .child(div().t_small().text_color(hex(Chrome::FOREGROUND)).child(tf(cx, key, &[("name", &panel.title)]))),
            );
        }
        for command in &manifest.contributes.commands {
            contributes = contributes.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .child(icon("chevron-right", 12., hex(Chrome::MUTED)))
                    .child(div().t_small().text_color(hex(Chrome::FOREGROUND)).child(command.title.clone())),
            );
        }
        let has_contributions = manifest.contributes.panel.is_some() || !manifest.contributes.commands.is_empty();
        let runtime_note = if manifest.runtime.is_process() { "plugins.runtime.process_note" } else { "plugins.runtime.wasm_note" };

        div()
            .pt_4()
            .flex()
            .flex_col()
            .gap_4()
            .child(self.render_project(manifest, cx))
            .when(has_contributions, |d| {
                d.child(
                    div().flex().flex_col().gap_1p5().child(detail_heading(t(cx, "plugins.contributes").to_string())).child(contributes),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1p5()
                    .child(detail_heading(t(cx, "plugins.info").to_string()))
                    .child(self.render_info_grid(info))
                    .child(div().t_caption().text_color(hex(Chrome::MUTED)).child(t(cx, runtime_note))),
            )
    }

    fn render_info_grid(&self, rows: Vec<(String, String)>) -> impl IntoElement {
        let mut grid = div().flex().flex_col().gap_1();
        for (label, value) in rows {
            grid = grid.child(
                div()
                    .flex()
                    .gap_3()
                    .child(div().w(px(130.)).flex_shrink_0().t_small().text_color(hex(Chrome::MUTED)).child(label))
                    .child(div().flex_1().min_w_0().t_small().text_color(hex(Chrome::FOREGROUND)).child(value)),
            );
        }
        grid
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
        let mut row = div().flex().flex_wrap().items_center().content_start().gap_1();
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

    /// Every permission the plugin declared, in full sentences — this is what installing it agrees
    /// to, so it is a tab of its own rather than a row of chips.
    fn render_permissions(&self, manifest: &Manifest, cx: &mut Context<Self>) -> impl IntoElement {
        let mut list = div().flex().flex_col().gap_2p5();
        if manifest.permissions.is_empty() {
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(icon("circle-check", 14., hex(Chrome::SUCCESS)))
                    .child(div().t_small().text_color(hex(Chrome::FOREGROUND)).child(t(cx, "plugins.perm.none"))),
            );
        }
        for permission in &manifest.permissions {
            debug_assert!(PERMISSIONS.iter().any(|(name, _)| name == permission));
            let (title, body) = permission_strings(permission);
            list = list.child(
                div().flex().gap_2().child(div().flex_shrink_0().pt_0p5().child(icon("shield-alert", 14., hex(Chrome::WARNING)))).child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .child(div().t_small().font_weight(FontWeight::MEDIUM).text_color(hex(Chrome::BRIGHT)).child(t(cx, title)))
                        .child(div().t_caption().text_color(hex(Chrome::MUTED)).child(t(cx, body))),
                ),
            );
        }
        // Reading the user's work and sending requests out is the pair that makes a leak possible.
        let reads = ["session.read", "workspace.read"].iter().any(|p| manifest.has_permission(p));
        let combo = reads && manifest.has_permission("net.request");
        let browser = manifest.has_permission("browser.control").then(|| self.render_browser_access(manifest, cx));
        div().pt_4().flex().flex_col().gap_3().child(list).children(browser).when(combo, |d| {
            d.child(
                div()
                    .p_3()
                    .rounded_md()
                    .flex()
                    .gap_2()
                    .bg(hex_alpha(Chrome::WARNING, 0.12))
                    .child(icon("triangle-alert", 14., hex(Chrome::WARNING)))
                    .child(div().flex_1().t_small().text_color(hex(Chrome::FOREGROUND)).child(t(cx, "plugins.perm.combo"))),
            )
        })
    }

    /// The sites a `browser.control` plugin may use, whether the user allowed it, and where its
    /// pages run. Allowing happens the first time the plugin tries (a dialog); taking it back is here.
    fn render_browser_access(&self, manifest: &Manifest, cx: &mut Context<Self>) -> AnyElement {
        use super::plugin_browser::{granted, revoke, BrowserMode};
        let sites = manifest.browser.clone().unwrap_or_default();
        let allowed = granted(&manifest.id, &sites, cx);
        let chosen = BrowserMode::chosen(&manifest.id, cx);
        let mut site_list = div().flex().flex_wrap().gap_1p5();
        for site in &sites.sites {
            let name = match site.aliases.is_empty() {
                true => site.host.clone(),
                false => format!("{} · {}", site.host, site.aliases.join(" · ")),
            };
            site_list = site_list.child(
                div().px_2().py_0p5().rounded_sm().bg(hex_alpha(Chrome::ERROR, 0.14)).t_small().text_color(hex(Chrome::BRIGHT)).child(name),
            );
        }
        let mut modes = div().flex().gap_1p5();
        for mode in BrowserMode::ALL {
            let id = manifest.id.clone();
            modes = modes.child(chip(
                SharedString::from(format!("browser-mode-{}", mode.id())),
                t(
                    cx,
                    match mode {
                        BrowserMode::Auto => "plugin_browser.mode.auto",
                        BrowserMode::Background => "plugin_browser.mode.background",
                        BrowserMode::Visible => "plugin_browser.mode.visible",
                    },
                ),
                chosen == mode,
                move |_, _, cx| {
                    let id = id.clone();
                    crate::settings::update_settings(cx, move |settings| match mode {
                        BrowserMode::Auto => {
                            settings.plugin_browser_modes.remove(&id);
                        }
                        other => {
                            settings.plugin_browser_modes.insert(id, other.id().to_string());
                        }
                    });
                },
            ));
        }
        let id = manifest.id.clone();
        div()
            .p_3()
            .rounded_md()
            .flex()
            .flex_col()
            .gap_2()
            .bg(hex_alpha(Chrome::ERROR, 0.08))
            .border_1()
            .border_color(hex_alpha(Chrome::ERROR, 0.3))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(icon("globe", 14., hex(Chrome::ERROR)))
                    .child(
                        div()
                            .flex_1()
                            .t_small()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(hex(Chrome::BRIGHT))
                            .child(t(cx, "plugin_browser.sites_title")),
                    )
                    .child(
                        div()
                            .t_caption()
                            .text_color(hex(if allowed { Chrome::SUCCESS } else { Chrome::MUTED }))
                            .child(t(cx, if allowed { "plugin_browser.allowed" } else { "plugin_browser.not_allowed" })),
                    )
                    .when(allowed, |d| {
                        d.child(action_button(
                            SharedString::from(format!("browser-revoke-{id}")),
                            t(cx, "plugin_browser.revoke"),
                            move |_, _, cx| revoke(&id, cx),
                        ))
                    }),
            )
            .child(site_list)
            .child(div().t_caption().text_color(hex(Chrome::MUTED)).child(t(cx, "plugin_browser.sites_note")))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().t_small().text_color(hex(Chrome::FOREGROUND)).child(t(cx, "plugin_browser.mode")))
                    .child(modes),
            )
            .into_any_element()
    }

    fn render_logs(&self, id: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let lines: Vec<String> =
            plugins::runtime(cx, id).map(|r| r.logs.iter().rev().take(200).rev().cloned().collect()).unwrap_or_default();
        div().pt_4().child(
            div()
                .id(SharedString::from(format!("plugin-log-{id}")))
                .max_h(px(420.))
                .overflow_y_scroll()
                .p_3()
                .rounded_md()
                .bg(hex(0x141414))
                .t_caption()
                .font_family("JetBrains Mono")
                .text_color(hex(Chrome::FOREGROUND))
                .child(if lines.is_empty() { t(cx, "plugins.no_logs").to_string() } else { lines.join("\n") }),
        )
    }

    /// The right-hand side when "New plugin" is picked: build one with an agent, or install one
    /// from a Git repository.
    fn render_create_plugin(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let inputs = self.plugins_page.inputs.as_ref();
        div()
            .w_full()
            .p_6()
            .max_w(px(760.))
            .flex()
            .flex_col()
            .gap_5()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div().flex().items_center().gap_2().child(icon("wand-sparkles", IconSize::BUTTON, hex(Chrome::PURPLE))).child(
                            div()
                                .t_title()
                                .font_weight(crate::theme::EMPHASIS)
                                .text_color(hex(Chrome::BRIGHT))
                                .child(t(cx, "plugins.create_title")),
                        ),
                    )
                    .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "plugins.create_body")))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(self.plugins_field(inputs.map(|i| &i.name)).max_w(px(220.)))
                            .child(self.plugins_field(inputs.map(|i| &i.idea))),
                    )
                    .child(div().flex().gap_2().child(action_button(
                        "plugins-create",
                        t(cx, "plugins.create_button"),
                        cx.listener(|this, _: &ClickEvent, window, cx| this.create_plugin_with_ai(window, cx)),
                    ))),
            )
            .child(div().flex().flex_col().gap_2().child(detail_heading(t(cx, "plugins.install_git").to_string())).child(
                div().flex().gap_2().items_center().child(self.plugins_field(inputs.map(|i| &i.git_url))).child(action_button(
                    "plugins-git",
                    if self.plugins_page.busy { t(cx, "plugins.installing") } else { t(cx, "plugins.install_git") },
                    cx.listener(|this, _: &ClickEvent, window, cx| this.install_plugin_git(window, cx)),
                )),
            ))
    }

    /// A new tab (or workspace when none is open) running `kind` in `dir`.
    fn request_launch_in(&mut self, kind: PaneKind, dir: std::path::PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.launch(kind.into(), LaunchTarget::NewWorkspace, dir, window, cx);
    }
}

impl Row {
    fn market(entry: &market::Entry) -> Self {
        Self {
            id: entry.id.clone(),
            name: entry.name.clone(),
            version: entry.version.clone(),
            publisher: entry.publisher.clone(),
            description: entry.description.clone(),
            keywords: entry.keywords.join(" ").to_lowercase(),
            icon: icon_named(entry.icon.as_deref()),
            // Nothing is installed yet, so there is no file to draw: a listing fetches nothing.
            logo: None,
            origin: Origin::Market,
        }
    }

    fn installed(plugin: &InstalledPlugin) -> Self {
        let manifest = plugin.manifest.as_ref();
        Self {
            id: plugin.id.clone(),
            name: plugin.name().to_string(),
            version: manifest.map(|m| m.version.clone()).unwrap_or_default(),
            publisher: manifest.map(|m| m.publisher.clone()).unwrap_or_default(),
            description: manifest.map(|m| m.description.clone()).unwrap_or_else(|| plugin.error.clone().unwrap_or_default()),
            keywords: manifest.map(|m| m.keywords.join(" ").to_lowercase()).unwrap_or_default(),
            icon: icon_named(manifest.and_then(|m| m.icon.as_deref())),
            logo: agentty_bridge::plugins::store::logo_file(plugin),
            origin: Origin::Installed,
        }
    }

    fn catalog(manifest: &Manifest) -> Self {
        Self {
            id: manifest.id.clone(),
            name: manifest.name.clone(),
            version: manifest.version.clone(),
            publisher: manifest.publisher.clone(),
            description: manifest.description.clone(),
            keywords: manifest.keywords.join(" ").to_lowercase(),
            icon: icon_named(manifest.icon.as_deref()),
            logo: None,
            origin: Origin::Builtin,
        }
    }
}

fn source_key(source: Source) -> &'static str {
    match source {
        Source::Builtin => "plugins.builtin",
        Source::Folder => "plugins.source_folder",
        Source::Git => "plugins.source_git",
        Source::Local => "plugins.source_local",
        Source::Dev => "plugins.source_dev",
        Source::Market => "plugins.market",
    }
}

/// "from 3m ago", for the list kept from last time — the same short form the sessions list uses.
fn ago(at: std::time::SystemTime, cx: &Context<Workbench>) -> String {
    let then = at.duration_since(std::time::UNIX_EPOCH).map(|since| since.as_millis() as u64).unwrap_or(0);
    let when = crate::ui::relative_time(crate::ui::now_ms(), then);
    // Under a minute `relative_time` says "now", and "read now ago" is not a sentence in any of
    // the four languages.
    if when == "now" {
        return t(cx, "plugins.market_kept_now").to_string();
    }
    tf(cx, "plugins.market_kept", &[("when", &when)])
}

impl Workbench {
    /// Asks the user, one plugin at a time, to allow what a plugin asks for before it first runs.
    pub(super) fn prepare_plugin_consent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.plugin_consent_open.is_some() {
            return;
        }
        let Some(id) = plugins::next_consent(cx) else { return };
        let Some(plugin) = plugins::plugin(cx, &id).cloned() else {
            plugins::answer_consent(&id, false, cx);
            return;
        };
        let Some(manifest) = plugin.manifest.clone() else { return };
        self.plugin_consent_open = Some(id.clone());
        let mut lines = Vec::new();
        for permission in &manifest.permissions {
            let (label, body) = permission_strings(permission);
            lines.push(format!("• {} — {}", t(cx, label), t(cx, body)));
            if permission == "browser.control" {
                if let Some(browser) = &manifest.browser {
                    let sites: Vec<String> = browser.sites.iter().flat_map(|s| s.domains().map(str::to_string)).collect();
                    lines.push(format!("   {}", sites.join(", ")));
                }
            }
        }
        let runtime_note = match manifest.runtime {
            agentty_bridge::plugins::manifest::Runtime::Wasm => t(cx, "plugins.runtime.wasm_note"),
            _ => t(cx, "plugins.runtime.process_note"),
        };
        let title = tf(cx, "plugins.consent.title", &[("name", &manifest.name)]);
        let body = format!("{}\n\n{}\n\n{}", t(cx, "plugins.consent.lead"), lines.join("\n"), runtime_note);
        let (allow, deny) = (t(cx, "plugins.consent.allow"), t(cx, "plugins.consent.deny"));
        window.activate_window();
        let answer = window.prompt(
            gpui::PromptLevel::Warning,
            &title,
            Some(&body),
            &[gpui::PromptButton::new(allow), gpui::PromptButton::cancel(deny)],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let allowed = answer.await == Ok(0);
            let _ = this.update_in(cx, |this, _, cx| {
                this.plugin_consent_open = None;
                plugins::answer_consent(&id, allowed, cx);
                cx.notify();
            });
        })
        .detach();
    }
}

/// The two strings that name a permission to the user.
fn permission_strings(permission: &str) -> (&'static str, &'static str) {
    match permission {
        "prompt.inject" => ("plugins.perm.prompt", "plugins.perm.prompt.body"),
        "terminal.write" => ("plugins.perm.terminal", "plugins.perm.terminal.body"),
        "session.read" => ("plugins.perm.session", "plugins.perm.session.body"),
        "workspace.read" => ("plugins.perm.workspace", "plugins.perm.workspace.body"),
        "net.request" => ("plugins.perm.net", "plugins.perm.net.body"),
        "browser.control" => ("plugins.perm.browser", "plugins.perm.browser.body"),
        "files" => ("plugins.perm.files", "plugins.perm.files.body"),
        _ => ("plugins.perm.unknown", "plugins.perm.unknown"),
    }
}

fn detail_heading(title: String) -> impl IntoElement {
    div().t_caption().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::MUTED)).child(title.to_uppercase())
}

/// The little coloured dot in front of a plugin's state.
fn dot(color: u32) -> impl IntoElement {
    div().size(px(6.)).rounded_full().bg(hex(color))
}
