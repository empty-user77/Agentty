//! Extensions page: what Claude Code and Codex can use (skills, subagents, commands, plugins, MCP
//! servers), one-click use in the workspace, MCP management and secure API connectors.

use crate::i18n::{t, tf};
use crate::launch::run_in_login_shell;
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::TypeScale;
use crate::ui::{action_button, chip, hint, tilde};
use agentty_bridge::connectors::{parse_endpoints, Auth, Connector, ConnectorStore};
use agentty_bridge::extensions::{
    discover, mcp_add_command, mcp_remove_command, Extension, ExtensionKind, McpScope, McpServerSpec, McpTransport, Scope, CATALOG,
};
use agentty_bridge::model::Agent;
use gpui::{div, prelude::*, px, ClickEvent, Context, Div, Entity, EventEmitter, FontWeight, SharedString, Subscription, Window};
use std::collections::HashMap;
use std::path::PathBuf;

pub enum ExtensionsEvent {
    /// Type `text` into a pane running `agent` (optionally pressing Enter).
    Use { agent: Agent, text: String, submit: bool },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Category {
    All,
    Kind(ExtensionKind),
    Connectors,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AuthChoice {
    None,
    Bearer,
    Header,
    Query,
}

struct McpForm {
    name: Entity<TextInput>,
    target: Entity<TextInput>,
    args: Entity<TextInput>,
    env: Entity<TextInput>,
    transport: McpTransport,
    scope: McpScope,
}

struct ConnectorForm {
    name: Entity<TextInput>,
    base_url: Entity<TextInput>,
    secret: Entity<TextInput>,
    auth_name: Entity<TextInput>,
    endpoints: Entity<TextInput>,
    description: Entity<TextInput>,
    auth: AuthChoice,
}

pub struct ExtensionsView {
    agent: Agent,
    category: Category,
    project: Option<PathBuf>,
    items: HashMap<Agent, Vec<Extension>>,
    loading: bool,
    search: Entity<TextInput>,
    mcp: McpForm,
    connector: ConnectorForm,
    connectors: Vec<Connector>,
    busy: bool,
    message: Option<(String, bool)>,
    catalog_token: Entity<TextInput>,
    scroll: gpui::ScrollHandle,
    /// Item shown in the detail panel, with its file contents once loaded.
    detail: Option<(Extension, Option<String>)>,
    detail_scroll: gpui::ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ExtensionsEvent> for ExtensionsView {}

fn input(window: &mut Window, cx: &mut Context<ExtensionsView>, placeholder: &'static str) -> Entity<TextInput> {
    cx.new(|cx| TextInput::new("", t(cx, placeholder), window, cx))
}

impl ExtensionsView {
    fn open_detail(&mut self, item: Extension, cx: &mut Context<Self>) {
        let path = item.path.clone();
        self.detail = Some((item, None));
        self.detail_scroll = gpui::ScrollHandle::new();
        cx.notify();
        let Some(path) = path else {
            self.detail.as_mut().unwrap().1 = Some(String::new());
            return;
        };
        let task = cx.background_spawn(async move { read_definition(&path) });
        cx.spawn(async move |this, cx| {
            let text = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(detail) = this.detail.as_mut() {
                    detail.1 = Some(text);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn render_detail(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let (item, content) = self.detail.clone()?;
        let color = kind_color(item.kind);
        let agent = item.agent;
        let close = cx.listener(|this, _: &ClickEvent, _, cx| {
            this.detail = None;
            cx.notify();
        });
        let folder =
            item.path
                .as_ref()
                .map(|p| if p.is_dir() { p.clone() } else { p.parent().map(|d| d.to_path_buf()).unwrap_or_else(|| p.clone()) });
        let file = item.path.as_ref().and_then(|p| definition_file(p));
        let mut actions = div().flex().gap_2();
        if let Some(text) = item.invocation.clone() {
            let insert = text.clone();
            actions = actions
                .child(action_button(
                    "detail-insert",
                    t(cx, "ext.insert"),
                    cx.listener(move |_, _: &ClickEvent, _, cx| {
                        cx.emit(ExtensionsEvent::Use { agent, text: insert.clone(), submit: false })
                    }),
                ))
                .child(action_button(
                    "detail-run",
                    format!("▶ {}", t(cx, "ext.run")),
                    cx.listener(move |_, _: &ClickEvent, _, cx| cx.emit(ExtensionsEvent::Use { agent, text: text.clone(), submit: true })),
                ));
        }
        if let Some(folder) = folder.clone() {
            actions = actions.child(action_button("detail-reveal", t(cx, "ext.open_folder"), move |_, _, cx| cx.reveal_path(&folder)));
        }
        if let Some(file) = file.clone() {
            actions = actions.child(action_button("detail-open-file", t(cx, "ext.open_file"), move |_, _, cx| cx.open_with_system(&file)));
        }
        let body: gpui::AnyElement = match content {
            None => crate::ui::loading_row(t(cx, "ext.loading")).into_any_element(),
            Some(text) if text.trim().is_empty() => hint(t(cx, "ext.no_definition")).into_any_element(),
            Some(text) => div()
                .relative()
                .flex_1()
                .min_h_0()
                .child(
                    div()
                        .id("ext-detail-text")
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&self.detail_scroll)
                        .p_4()
                        .rounded_md()
                        .bg(hex(0x161616))
                        .font_family(crate::settings::BUNDLED_FONT)
                        .text_size(px(12.))
                        .text_color(hex(Chrome::FOREGROUND))
                        .child(text),
                )
                .child(crate::ui::scrollbar(self.detail_scroll.clone()))
                .into_any_element(),
        };
        Some(
            div()
                .id("ext-detail-overlay")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(hex_alpha(0, 0.45))
                .occlude()
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.detail = None;
                    cx.notify();
                }))
                .child(
                    div()
                        .id("ext-detail")
                        .w(px(760.))
                        .h(gpui::relative(0.82))
                        .p_5()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .rounded_xl()
                        .bg(hex(Chrome::OVERLAY))
                        .border_1()
                        .border_color(hex(Chrome::OVERLAY_BORDER))
                        .shadow_lg()
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .px_1p5()
                                        .rounded_sm()
                                        .bg(hex_alpha(color, 0.2))
                                        .t_small()
                                        .text_color(hex(color))
                                        .child(t(cx, kind_label(item.kind))),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .t_large()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(hex(Chrome::BRIGHT))
                                        .child(item.name.clone()),
                                )
                                .child(crate::ui::icon_only("ext-detail-close", "x", close)),
                        )
                        .when(!item.description.is_empty(), |d| {
                            d.child(div().t_body().text_color(hex(Chrome::FOREGROUND)).child(item.description.clone()))
                        })
                        .children(item.detail.clone().map(|detail| div().t_small().text_color(hex(Chrome::MUTED)).child(detail)))
                        .children(
                            item.path
                                .as_ref()
                                .map(|p| div().t_small().truncate().text_color(hex(Chrome::MUTED)).child(crate::ui::tilde(p))),
                        )
                        .child(actions)
                        .child(body),
                ),
        )
    }

    pub fn new(project: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = input(window, cx, "ext.search");
        let subscriptions = vec![cx.subscribe(&search, |_, _, event: &TextInputEvent, cx| {
            if matches!(event, TextInputEvent::Changed) {
                cx.notify();
            }
        })];
        let mcp = McpForm {
            name: input(window, cx, "ext.mcp_name"),
            target: input(window, cx, "ext.mcp_target"),
            args: input(window, cx, "ext.mcp_args"),
            env: input(window, cx, "ext.mcp_env"),
            transport: McpTransport::Stdio,
            scope: McpScope::User,
        };
        let secret = cx.new(|cx| TextInput::new("", t(cx, "ext.conn_secret"), window, cx).masked());
        let catalog_token = cx.new(|cx| TextInput::new("", t(cx, "ext.catalog_token"), window, cx).masked());
        let connector = ConnectorForm {
            name: input(window, cx, "ext.conn_name"),
            base_url: input(window, cx, "ext.conn_base"),
            secret,
            auth_name: input(window, cx, "ext.conn_auth_name"),
            endpoints: input(window, cx, "ext.conn_endpoints"),
            description: input(window, cx, "ext.conn_description"),
            auth: AuthChoice::Bearer,
        };
        let mut view = Self {
            agent: Agent::Claude,
            category: Category::All,
            project,
            items: HashMap::new(),
            loading: false,
            search,
            mcp,
            connector,
            connectors: ConnectorStore::load().connectors,
            busy: false,
            message: None,
            catalog_token,
            scroll: gpui::ScrollHandle::new(),
            detail: None,
            detail_scroll: gpui::ScrollHandle::new(),
            _subscriptions: subscriptions,
        };
        view.reload(cx);
        view
    }

    /// Development helper: switch agent/category by name (`claude:mcp`, `codex:connectors`, …).
    pub fn debug_select(&mut self, spec: &str, cx: &mut Context<Self>) {
        if let Some(index) = spec.strip_prefix("detail:").and_then(|i| i.parse::<usize>().ok()) {
            if let Some(item) = self.items.get(&self.agent).and_then(|items| items.get(index)).cloned() {
                self.open_detail(item, cx);
            }
            return;
        }
        let (agent, category) = spec.split_once(':').unwrap_or((spec, "all"));
        self.agent = if agent == "codex" { Agent::Codex } else { Agent::Claude };
        self.category = match category {
            "skills" => Category::Kind(ExtensionKind::Skill),
            "agents" => Category::Kind(ExtensionKind::Agent),
            "commands" => Category::Kind(ExtensionKind::Command),
            "plugins" => Category::Kind(ExtensionKind::Plugin),
            "mcp" => Category::Kind(ExtensionKind::Mcp),
            "connectors" => Category::Connectors,
            _ => Category::All,
        };
        cx.notify();
    }

    pub fn set_project(&mut self, project: Option<PathBuf>, cx: &mut Context<Self>) {
        if project != self.project {
            self.project = project;
            self.items.clear();
            self.reload(cx);
        }
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        let project = self.project.clone();
        let task =
            cx.background_spawn(async move { [Agent::Claude, Agent::Codex].map(|agent| (agent, discover(agent, project.as_deref()))) });
        cx.spawn(async move |this, cx| {
            let results = task.await;
            let _ = this.update(cx, |this, cx| {
                this.items = results.into_iter().collect();
                this.connectors = ConnectorStore::load().connectors;
                this.loading = false;
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn set_message(&mut self, text: impl Into<String>, error: bool, cx: &mut Context<Self>) {
        self.message = Some((text.into(), error));
        cx.notify();
    }

    /// Runs an agent CLI command in the background, then reloads the lists.
    fn run(&mut self, argv: Vec<String>, done_key: &'static str, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        let label = argv.iter().take(3).cloned().collect::<Vec<_>>().join(" ");
        self.set_message(tf(cx, "ext.running", &[("name", &label)]), false, cx);
        let task = cx.background_spawn(async move { run_in_login_shell(&argv) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(_) => {
                        let text = t(cx, done_key).to_string();
                        this.set_message(text, false, cx);
                    }
                    Err(err) => this.set_message(format!("{err:#}"), true, cx),
                }
                this.reload(cx);
            });
        })
        .detach();
    }

    fn add_mcp(&mut self, cx: &mut Context<Self>) {
        let read = |e: &Entity<TextInput>, cx: &Context<Self>| e.read(cx).text().trim().to_string();
        let env = read(&self.mcp.env, cx)
            .split(',')
            .filter_map(|pair| pair.split_once(['=', ':']).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())))
            .filter(|(k, _)| !k.is_empty())
            .collect();
        let spec = McpServerSpec {
            name: read(&self.mcp.name, cx),
            transport: self.mcp.transport,
            target: read(&self.mcp.target, cx),
            args: read(&self.mcp.args, cx).split_whitespace().map(str::to_string).collect(),
            env,
            scope: self.mcp.scope,
        };
        match mcp_add_command(self.agent, &spec) {
            Ok(argv) => {
                for field in [&self.mcp.name, &self.mcp.target, &self.mcp.args, &self.mcp.env] {
                    field.update(cx, |i, cx| i.set_text("", cx));
                }
                self.run(argv, "ext.mcp_added", cx);
            }
            Err(err) => self.set_message(format!("{err:#}"), true, cx),
        }
    }

    fn save_connector(&mut self, cx: &mut Context<Self>) {
        let read = |e: &Entity<TextInput>, cx: &Context<Self>| e.read(cx).text().trim().to_string();
        let auth_name = read(&self.connector.auth_name, cx);
        let auth = match self.connector.auth {
            AuthChoice::None => Auth::None,
            AuthChoice::Bearer => Auth::Bearer,
            AuthChoice::Header => Auth::Header { name: if auth_name.is_empty() { "X-API-Key".into() } else { auth_name } },
            AuthChoice::Query => Auth::Query { name: if auth_name.is_empty() { "api_key".into() } else { auth_name } },
        };
        let mut connector = Connector::new(&read(&self.connector.name, cx), &read(&self.connector.base_url, cx), auth);
        connector.description = read(&self.connector.description, cx);
        let result = parse_endpoints(&read(&self.connector.endpoints, cx)).and_then(|endpoints| {
            connector.endpoints = endpoints;
            let secret = self.connector.secret.read(cx).text().to_string();
            let mut store = ConnectorStore::load();
            store.upsert(connector.clone(), Some(secret.as_str()))
        });
        match result {
            Ok(()) => {
                // Clear the secret from memory as soon as it is in the Keychain.
                let c = &self.connector;
                for field in [&c.name, &c.base_url, &c.secret, &c.auth_name, &c.endpoints, &c.description] {
                    field.update(cx, |i, cx| i.set_text("", cx));
                }
                self.connectors = ConnectorStore::load().connectors;
                let name = connector.name.clone();
                self.set_message(tf(cx, "ext.conn_saved", &[("name", &name)]), false, cx);
            }
            Err(err) => self.set_message(format!("{err:#}"), true, cx),
        }
    }

    fn register_connector(&mut self, connector: &Connector, agent: Agent, cx: &mut Context<Self>) {
        let Ok(exe) = std::env::current_exe() else { return };
        let spec = McpServerSpec {
            name: connector.server_name(),
            transport: McpTransport::Stdio,
            target: exe.display().to_string(),
            args: vec!["mcp-connector".into(), "--id".into(), connector.id.clone()],
            env: Vec::new(),
            scope: McpScope::User,
        };
        match mcp_add_command(agent, &spec) {
            Ok(argv) => self.run(argv, "ext.conn_registered", cx),
            Err(err) => self.set_message(format!("{err:#}"), true, cx),
        }
    }

    fn is_registered(&self, connector: &Connector, agent: Agent) -> bool {
        let name = connector.server_name();
        self.items.get(&agent).is_some_and(|items| items.iter().any(|e| e.kind == ExtensionKind::Mcp && e.name == name))
    }
}

fn kind_label(kind: ExtensionKind) -> &'static str {
    match kind {
        ExtensionKind::Skill => "ext.skills",
        ExtensionKind::Agent => "ext.agents",
        ExtensionKind::Command => "ext.commands",
        ExtensionKind::Plugin => "ext.plugins",
        ExtensionKind::Mcp => "ext.mcp",
    }
}

fn kind_color(kind: ExtensionKind) -> u32 {
    match kind {
        ExtensionKind::Skill => Chrome::PURPLE,
        ExtensionKind::Agent => Chrome::BLUE,
        ExtensionKind::Command => Chrome::GREEN,
        ExtensionKind::Plugin => Chrome::ORANGE,
        ExtensionKind::Mcp => Chrome::CLAUDE,
    }
}

fn card() -> Div {
    div().rounded_lg().border_1().border_color(hex(Chrome::BORDER)).bg(hex(0x232323))
}

fn field(input: &Entity<TextInput>) -> Div {
    div()
        .flex_1()
        .min_w_0()
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

fn label(text: &str) -> Div {
    div().w(px(110.)).flex_shrink_0().truncate().t_small().text_color(hex(Chrome::MUTED)).child(text.to_string())
}

impl Render for ExtensionsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let items = self.items.get(&self.agent).cloned().unwrap_or_default();
        let count = |kind: ExtensionKind| items.iter().filter(|e| e.kind == kind).count();

        let mut agents = div().flex().gap_1();
        for (agent, name) in [(Agent::Claude, "Claude Code"), (Agent::Codex, "Codex")] {
            agents = agents.child(chip(
                SharedString::from(format!("ext-agent-{name}")),
                name,
                self.agent == agent,
                cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.agent = agent;
                    cx.notify();
                }),
            ));
        }

        let mut categories = div().flex().gap_1();
        let mut entries = vec![(Category::All, format!("{} {}", t(cx, "filter.all"), items.len()))];
        for kind in [ExtensionKind::Skill, ExtensionKind::Agent, ExtensionKind::Command, ExtensionKind::Plugin, ExtensionKind::Mcp] {
            entries.push((Category::Kind(kind), format!("{} {}", t(cx, kind_label(kind)), count(kind))));
        }
        entries.push((Category::Connectors, format!("{} {}", t(cx, "ext.connectors"), self.connectors.len())));
        for (index, (category, text)) in entries.into_iter().enumerate() {
            categories = categories.child(chip(
                ("ext-cat", index),
                text,
                self.category == category,
                cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.category = category;
                    cx.notify();
                }),
            ));
        }

        let header =
            div()
                .flex()
                .items_center()
                .gap_3()
                .child(div().t_heading().font_weight(FontWeight::SEMIBOLD).text_color(hex(Chrome::BRIGHT)).child(t(cx, "page.extensions")))
                .child(agents)
                .children(self.project.as_ref().map(|p| {
                    div().px_1p5().py_0p5().rounded_sm().bg(hex(0x2a2a2a)).t_small().text_color(hex(Chrome::MUTED)).child(tilde(p))
                }))
                .child(action_button("ext-reload", t(cx, "usage.refresh"), cx.listener(|this, _: &ClickEvent, _, cx| this.reload(cx))))
                .child(div().flex_1());

        let message = self.message.clone().map(|(text, error)| {
            div()
                .px_3()
                .py_2()
                .rounded_md()
                .t_small()
                .bg(hex_alpha(if error { Chrome::ERROR } else { Chrome::ACCENT }, 0.15))
                .text_color(hex(if error { Chrome::ERROR } else { Chrome::FOREGROUND }))
                .child(text)
        });

        let body: gpui::AnyElement = match self.category {
            Category::Connectors => self.render_connectors(cx).into_any_element(),
            category => {
                let items_for_catalog = items.clone();
                let query = self.search.read(cx).text().to_lowercase();
                let filtered: Vec<Extension> = items
                    .into_iter()
                    .filter(|e| matches!(category, Category::All) || category == Category::Kind(e.kind))
                    .filter(|e| query.is_empty() || e.name.to_lowercase().contains(&query) || e.description.to_lowercase().contains(&query))
                    .collect();
                let mut list = div().flex().flex_col().gap_2();
                if category == Category::Kind(ExtensionKind::Mcp) {
                    list = list.child(self.render_catalog(&items_for_catalog, cx)).child(self.render_mcp_form(cx));
                }
                if self.loading && filtered.is_empty() {
                    list = list.child(crate::ui::loading_row(t(cx, "usage.loading")));
                } else if filtered.is_empty() {
                    list = list.child(hint(t(cx, "ext.empty")));
                }
                for (index, item) in filtered.into_iter().enumerate() {
                    list = list.child(self.render_item(index, item, cx));
                }
                list.into_any_element()
            }
        };
        let show_search = self.category != Category::Connectors;

        // Title, agent tabs, categories and search stay put; only the list scrolls.
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
                    .child(categories)
                    .when(show_search, |d| d.child(div().w(px(360.)).child(field(&self.search))))
                    .children(message),
            )
            .child(
                div()
                    .id("extensions-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .relative()
                    .child(div().px_6().py_4().child(body))
                    .child(crate::ui::scrollbar(self.scroll.clone())),
            )
            .children(self.render_detail(cx))
    }
}

/// The definition file behind an extension: the file itself, or `SKILL.md` / `README.md` in its folder.
fn definition_file(path: &std::path::Path) -> Option<PathBuf> {
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    ["SKILL.md", "skill.md", "README.md", ".claude-plugin/plugin.json", "plugin.json"]
        .iter()
        .map(|name| path.join(name))
        .find(|p| p.is_file())
}

fn read_definition(path: &std::path::Path) -> String {
    const LIMIT: u64 = 256 * 1024;
    let Some(file) = definition_file(path) else { return String::new() };
    match std::fs::File::open(&file) {
        Ok(handle) => {
            use std::io::Read;
            let mut text = String::new();
            let _ = handle.take(LIMIT).read_to_string(&mut text);
            text
        }
        Err(err) => err.to_string(),
    }
}

impl ExtensionsView {
    fn render_item(&self, index: usize, item: Extension, cx: &mut Context<Self>) -> impl IntoElement {
        let scope = match &item.scope {
            Scope::User => t(cx, "ext.scope_user").to_string(),
            Scope::Project => t(cx, "ext.scope_project").to_string(),
            Scope::System => t(cx, "ext.scope_system").to_string(),
            Scope::Plugin(name) => format!("plugin · {name}"),
        };
        let color = kind_color(item.kind);
        let invocation = item.invocation.clone();
        let agent = item.agent;
        let path = item.path.clone();
        let removable = item.kind == ExtensionKind::Mcp && !matches!(item.scope, Scope::Plugin(_) | Scope::System);

        card()
            .px_4()
            .py_3()
            .flex()
            .items_center()
            .gap_4()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .px_1p5()
                                    .rounded_sm()
                                    .bg(hex_alpha(color, 0.2))
                                    .t_small()
                                    .text_color(hex(color))
                                    .child(t(cx, kind_label(item.kind))),
                            )
                            .child(
                                div()
                                    .font_family(crate::settings::BUNDLED_FONT)
                                    .t_body()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(hex(Chrome::BRIGHT))
                                    .child(item.name.clone()),
                            )
                            .child(div().t_small().text_color(hex(Chrome::MUTED)).child(scope))
                            .when(!item.enabled, |d| {
                                d.child(
                                    div()
                                        .px_1()
                                        .rounded_sm()
                                        .bg(hex(0x3a3a3a))
                                        .t_small()
                                        .text_color(hex(Chrome::MUTED))
                                        .child(t(cx, "ext.disabled")),
                                )
                            })
                            .children(
                                item.detail.clone().map(|d| div().min_w_0().truncate().t_small().text_color(hex(Chrome::MUTED)).child(d)),
                            ),
                    )
                    .when(!item.description.is_empty(), |d| {
                        d.child(div().truncate().t_small().text_color(hex(Chrome::FOREGROUND)).child(item.description.clone()))
                    }),
            )
            .child(
                div()
                    .flex()
                    .gap_1()
                    .flex_shrink_0()
                    .when_some(invocation, |d, text| {
                        let insert = text.clone();
                        d.child(action_button(
                            ("ext-insert", index),
                            t(cx, "ext.insert"),
                            cx.listener(move |_, _: &ClickEvent, _, cx| {
                                cx.emit(ExtensionsEvent::Use { agent, text: insert.clone(), submit: false });
                            }),
                        ))
                        .child(action_button(
                            ("ext-run", index),
                            format!("▶ {}", t(cx, "ext.run")),
                            cx.listener(move |_, _: &ClickEvent, _, cx| {
                                cx.emit(ExtensionsEvent::Use { agent, text: text.clone(), submit: true });
                            }),
                        ))
                    })
                    .when(item.path.is_some() || !item.description.is_empty(), |d| {
                        let item = item.clone();
                        d.child(action_button(
                            ("ext-detail", index),
                            t(cx, "ext.details"),
                            cx.listener(move |this, _: &ClickEvent, _, cx| this.open_detail(item.clone(), cx)),
                        ))
                    })
                    .when_some(path.filter(|p| p.is_file() || p.is_dir()), |d, path| {
                        let folder =
                            if path.is_dir() { path.clone() } else { path.parent().map(|p| p.to_path_buf()).unwrap_or(path.clone()) };
                        d.child(action_button(
                            ("ext-open", index),
                            t(cx, "ext.open_folder"),
                            cx.listener(move |_, _: &ClickEvent, _, cx| cx.reveal_path(&folder)),
                        ))
                    })
                    .when(removable, |d| {
                        let (name, scope) = (item.name.clone(), item.scope.clone());
                        d.child(action_button(
                            ("ext-remove", index),
                            t(cx, "ext.remove"),
                            cx.listener(move |this, _: &ClickEvent, _, cx| match mcp_remove_command(agent, &name, &scope) {
                                Ok(argv) => this.run(argv, "ext.mcp_removed", cx),
                                Err(err) => this.set_message(format!("{err:#}"), true, cx),
                            }),
                        ))
                    }),
            )
    }

    fn render_catalog(&self, items: &[Extension], cx: &mut Context<Self>) -> impl IntoElement {
        let agent = self.agent;
        let installed = |name: &str| items.iter().any(|e| e.kind == ExtensionKind::Mcp && e.name == name);
        let mut grid = div().flex().flex_col().gap_2();
        let entries: Vec<_> = CATALOG.iter().enumerate().collect();
        for row in entries.chunks(3) {
            let mut line = div().flex().gap_2();
            for (index, entry) in row {
                let supported = entry.agents.contains(&agent);
                let done = installed(entry.name);
                let transport = match entry.transport {
                    McpTransport::Stdio => "stdio",
                    McpTransport::Http => "http",
                    McpTransport::Sse => "sse",
                };
                let button: gpui::AnyElement = if done {
                    div()
                        .px_2()
                        .py_0p5()
                        .rounded_sm()
                        .t_small()
                        .text_color(hex(Chrome::SUCCESS))
                        .child(format!("✓ {}", t(cx, "ext.catalog_added")))
                        .into_any_element()
                } else if !supported {
                    div()
                        .px_2()
                        .py_0p5()
                        .t_small()
                        .text_color(hex(Chrome::MUTED))
                        .child(t(cx, "ext.catalog_unsupported"))
                        .into_any_element()
                } else {
                    let index = *index;
                    action_button(
                        ("catalog-add", index),
                        t(cx, "ext.catalog_add"),
                        cx.listener(move |this, _: &ClickEvent, _, cx| {
                            let entry = &CATALOG[index];
                            let token = this.catalog_token.read(cx).text().to_string();
                            let token = entry.token_header.is_some().then_some(token);
                            match mcp_add_command(this.agent, &entry.spec(this.agent, token.as_deref())) {
                                Ok(argv) => {
                                    this.catalog_token.update(cx, |i, cx| i.set_text("", cx));
                                    this.run(argv, "ext.mcp_added", cx);
                                }
                                Err(err) => this.set_message(format!("{err:#}"), true, cx),
                            }
                        }),
                    )
                    .into_any_element()
                };
                line = line.child(
                    card()
                        .flex_1()
                        .min_w_0()
                        .px_3()
                        .py_2p5()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .when(!supported, |d| d.opacity(0.55))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(div().t_body().font_weight(FontWeight::SEMIBOLD).text_color(hex(Chrome::BRIGHT)).child(entry.title))
                                .child(
                                    div().px_1().rounded_sm().bg(hex(0x333333)).t_caption().text_color(hex(Chrome::MUTED)).child(transport),
                                )
                                .child(div().flex_1())
                                .child(button),
                        )
                        .child(div().truncate().t_small().text_color(hex(Chrome::MUTED)).child(entry.description)),
                );
            }
            for _ in row.len()..3 {
                line = line.child(div().flex_1());
            }
            grid = grid.child(line);
        }
        card()
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div().t_title().font_weight(FontWeight::SEMIBOLD).text_color(hex(Chrome::BRIGHT)).child(t(cx, "ext.catalog_title")),
                    )
                    .child(div().flex_1().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "ext.catalog_hint")))
                    .child(div().w(px(260.)).child(field(&self.catalog_token))),
            )
            .child(grid)
    }

    fn render_mcp_form(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let transport = self.mcp.transport;
        let scope = self.mcp.scope;
        let mut transports = div().flex().gap_1();
        for (value, text) in [(McpTransport::Stdio, "stdio"), (McpTransport::Http, "http")] {
            transports = transports.child(chip(
                SharedString::from(format!("mcp-t-{text}")),
                text,
                transport == value,
                cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.mcp.transport = value;
                    cx.notify();
                }),
            ));
        }
        let mut scopes = div().flex().gap_1();
        for (value, key) in [(McpScope::User, "ext.scope_user"), (McpScope::Project, "ext.scope_project")] {
            if self.agent == Agent::Codex && value == McpScope::Project {
                continue;
            }
            scopes = scopes.child(chip(
                SharedString::from(format!("mcp-s-{key}")),
                t(cx, key),
                scope == value,
                cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.mcp.scope = value;
                    cx.notify();
                }),
            ));
        }
        card()
            .p_4()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().t_body().font_weight(FontWeight::SEMIBOLD).text_color(hex(Chrome::BRIGHT)).child(t(cx, "ext.mcp_add")))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(label(t(cx, "ext.mcp_transport")))
                    .child(transports)
                    .child(div().w(px(16.)))
                    .child(scopes),
            )
            .child(div().flex().items_center().gap_2().child(label(t(cx, "ext.lbl_name"))).child(field(&self.mcp.name)))
            .child(div().flex().items_center().gap_2().child(label(t(cx, "ext.lbl_target"))).child(field(&self.mcp.target)))
            .when(transport == McpTransport::Stdio, |d| {
                d.child(div().flex().items_center().gap_2().child(label(t(cx, "ext.lbl_args"))).child(field(&self.mcp.args)))
            })
            .child(div().flex().items_center().gap_2().child(label(t(cx, "ext.lbl_env"))).child(field(&self.mcp.env)))
            .child(div().t_small().text_color(hex(Chrome::WARNING)).child(t(cx, "ext.mcp_secret_warning")))
            .child(div().flex().justify_end().child(action_button(
                "mcp-add",
                t(cx, "settings.alias_add"),
                cx.listener(|this, _: &ClickEvent, _, cx| this.add_mcp(cx)),
            )))
    }

    fn render_connectors(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let auth = self.connector.auth;
        let mut auths = div().flex().gap_1();
        for (value, key) in [
            (AuthChoice::Bearer, "ext.auth_bearer"),
            (AuthChoice::Header, "ext.auth_header"),
            (AuthChoice::Query, "ext.auth_query"),
            (AuthChoice::None, "ext.auth_none"),
        ] {
            auths = auths.child(chip(
                SharedString::from(format!("conn-auth-{key}")),
                t(cx, key),
                auth == value,
                cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.connector.auth = value;
                    cx.notify();
                }),
            ));
        }
        let c = &self.connector;
        let form = card()
            .p_4()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().t_body().font_weight(FontWeight::SEMIBOLD).text_color(hex(Chrome::BRIGHT)).child(t(cx, "ext.conn_add")))
            .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "ext.conn_hint")))
            .child(div().flex().items_center().gap_2().child(label(t(cx, "ext.lbl_name"))).child(field(&c.name)))
            .child(div().flex().items_center().gap_2().child(label(t(cx, "ext.lbl_base"))).child(field(&c.base_url)))
            .child(div().flex().items_center().gap_2().child(label(t(cx, "ext.conn_auth"))).child(auths))
            .when(matches!(auth, AuthChoice::Header | AuthChoice::Query), |d| {
                d.child(div().flex().items_center().gap_2().child(label(t(cx, "ext.lbl_auth_name"))).child(field(&c.auth_name)))
            })
            .when(auth != AuthChoice::None, |d| {
                d.child(div().flex().items_center().gap_2().child(label(t(cx, "ext.lbl_secret"))).child(field(&c.secret)))
            })
            .child(div().flex().items_center().gap_2().child(label(t(cx, "ext.lbl_endpoints"))).child(field(&c.endpoints)))
            .child(div().flex().items_center().gap_2().child(label(t(cx, "ext.lbl_description"))).child(field(&c.description)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().t_small().text_color(hex(Chrome::SUCCESS)).child(format!("🔒 {}", t(cx, "ext.conn_security"))))
                    .child(action_button(
                        "conn-save",
                        t(cx, "ext.conn_save"),
                        cx.listener(|this, _: &ClickEvent, _, cx| this.save_connector(cx)),
                    )),
            );

        let mut list = div().flex().flex_col().gap_2();
        if self.connectors.is_empty() {
            list = list.child(hint(t(cx, "ext.conn_empty")));
        }
        for (index, connector) in self.connectors.iter().enumerate() {
            let auth_text = match &connector.auth {
                Auth::None => t(cx, "ext.auth_none").to_string(),
                Auth::Bearer => "Bearer".to_string(),
                Auth::Header { name } => format!("header {name}"),
                Auth::Query { name } => format!("query {name}"),
            };
            let mut actions = div().flex().gap_1().flex_shrink_0();
            for (offset, agent) in [Agent::Claude, Agent::Codex].into_iter().enumerate() {
                let registered = self.is_registered(connector, agent);
                let target = connector.clone();
                let name = agent.display_name();
                let text = if registered { format!("✓ {name}") } else { tf(cx, "ext.conn_register", &[("name", name)]) };
                actions = actions.child(action_button(
                    ("conn-reg", index * 2 + offset),
                    text,
                    cx.listener(move |this, _: &ClickEvent, _, cx| {
                        if registered {
                            if let Ok(argv) = mcp_remove_command(agent, &target.server_name(), &Scope::User) {
                                this.run(argv, "ext.mcp_removed", cx);
                            }
                        } else {
                            this.register_connector(&target, agent, cx);
                        }
                    }),
                ));
            }
            let id = connector.id.clone();
            actions = actions.child(action_button(
                ("conn-delete", index),
                t(cx, "ext.remove"),
                cx.listener(move |this, _: &ClickEvent, _, cx| {
                    let mut store = ConnectorStore::load();
                    match store.remove(&id) {
                        Ok(()) => {
                            this.connectors = store.connectors;
                            let text = t(cx, "ext.conn_deleted").to_string();
                            this.set_message(text, false, cx);
                        }
                        Err(err) => this.set_message(format!("{err:#}"), true, cx),
                    }
                }),
            ));
            list = list.child(
                card()
                    .px_4()
                    .py_3()
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .font_family(crate::settings::BUNDLED_FONT)
                                            .t_body()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(hex(Chrome::BRIGHT))
                                            .child(connector.server_name()),
                                    )
                                    .child(div().t_small().text_color(hex(Chrome::MUTED)).child(auth_text))
                                    .child(div().t_small().text_color(hex(Chrome::MUTED)).child(tf(
                                        cx,
                                        "ext.conn_tools",
                                        &[("n", &(connector.endpoints.len() + connector.allow_any_path as usize).to_string())],
                                    ))),
                            )
                            .child(div().truncate().t_small().text_color(hex(Chrome::FOREGROUND)).child(connector.base_url.clone())),
                    )
                    .child(actions),
            );
        }
        div().flex().flex_col().gap_3().child(form).child(list)
    }
}
