//! Agent Rest Client: an HTTP client inside Agentty — requests, environments, saved collections,
//! authorization and a proxy — the kind of tool Agentty itself does not provide, added as a plugin.
//!
//! It is a Rust program compiled to WebAssembly, so it has no files, no sockets and no processes
//! of its own. Requests go through `net/fetch` (the `net.request` permission), which bounds the
//! method, headers, sizes, redirects and time; what the plugin remembers goes through
//! `storage/*`, which is its own folder under `plugin-data` and nothing else.

use agentty_plugin::{export_plugin, ui, FetchRequest, Host, Plugin, UiEvent};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Response body kept for the panel (the panel itself refuses much more).
const BODY_SHOWN: usize = 8_000;
/// Requests remembered.
const HISTORY: usize = 20;
/// Headers per request, and variables per environment.
const MAX_ROWS: usize = 24;

const METHODS: &[(&str, &str)] =
    &[("GET", "GET"), ("POST", "POST"), ("PUT", "PUT"), ("PATCH", "PATCH"), ("DELETE", "DELETE"), ("HEAD", "HEAD"), ("OPTIONS", "OPTIONS")];

const VIEWS: &[(&str, &str)] =
    &[("request", "Request"), ("environments", "Environments"), ("collection", "Collection"), ("settings", "Settings")];

const AUTH_KINDS: &[(&str, &str)] = &[("none", "No auth"), ("bearer", "Bearer token"), ("basic", "Basic"), ("header", "API key header")];

/// What is kept between runs, one storage key each.
const KEYS: &[&str] = &["request", "environments", "collection", "history", "settings"];

// ----------------------------------------------------------------------------- state

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Auth {
    /// `none`, `bearer`, `basic` or `header`.
    kind: String,
    token: String,
    user: String,
    password: String,
    header: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Request {
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    body: String,
    auth: Auth,
}

impl Request {
    fn method(&self) -> String {
        if self.method.is_empty() {
            "GET".to_string()
        } else {
            self.method.clone()
        }
    }

    fn has_body(&self) -> bool {
        !matches!(self.method().as_str(), "GET" | "HEAD")
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Environment {
    name: String,
    /// Ordered, so rows stay where the user put them.
    values: Vec<(String, String)>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    name: String,
    request: Request,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    /// `http://host:port`, empty for none.
    proxy: String,
    /// Seconds; 0 means Agentty's own default.
    timeout: u64,
}

struct Outcome {
    status: u16,
    status_text: String,
    ok: bool,
    duration_ms: u64,
    bytes: usize,
    truncated: bool,
    headers: Vec<(String, String)>,
    body: String,
    pretty: Option<String>,
}

#[derive(Default)]
struct AgentRestClient {
    view: String,
    request: Request,
    environments: Vec<Environment>,
    active_env: String,
    collection: Vec<Saved>,
    history: Vec<Request>,
    settings: Settings,
    /// The `net/fetch` being waited for.
    pending: Option<u64>,
    outcome: Option<Outcome>,
    /// Stored values still being read at startup: (request id, key).
    loading: Vec<(u64, String)>,
    /// Name typed for "Save" and for a new environment.
    name: String,
    /// The environment whose values are open, by name.
    editing: String,
    pretty: bool,
    show_response_headers: bool,
    message: String,
}

// ----------------------------------------------------------------------------- variables

/// `{{name}}` replaced from the active environment. What has no value is left as it is and
/// reported, so a request never goes out half-filled without the user being told.
fn substitute(text: &str, values: &[(String, String)], missing: &mut Vec<String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            out.push_str(&rest[start..]);
            return out;
        };
        let name = after[..end].trim();
        match values.iter().find(|(key, _)| key == name) {
            Some((_, value)) => out.push_str(value),
            None => {
                if !name.is_empty() && !missing.iter().any(|already| already == name) {
                    missing.push(name.to_string());
                }
                out.push_str(&rest[start..start + 2 + end + 2]);
            }
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

/// Pretty JSON, when it is JSON.
fn pretty_json(text: &str) -> Option<String> {
    let value: Value = serde_json::from_str(text.trim()).ok()?;
    serde_json::to_string_pretty(&value).ok()
}

fn base64(bytes: &[u8]) -> String {
    const SET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let block = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let packed = (u32::from(block[0]) << 16) | (u32::from(block[1]) << 8) | u32::from(block[2]);
        for index in 0..4 {
            if index <= chunk.len() {
                out.push(SET[(packed >> (18 - 6 * index)) as usize & 0x3f] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn cut(text: &str, limit: usize) -> String {
    match text.chars().count() > limit {
        true => format!("{}…", text.chars().take(limit).collect::<String>()),
        false => text.to_string(),
    }
}

/// The line under the body field: how much is in it.
fn body_note(body: &str) -> Option<String> {
    let lines = body.lines().count();
    (!body.is_empty()).then(|| format!("{lines} lines, {} characters", body.chars().count()))
}

// ----------------------------------------------------------------------------- the plugin

impl AgentRestClient {
    fn view(&self) -> &str {
        if self.view.is_empty() {
            "request"
        } else {
            &self.view
        }
    }

    fn env_values(&self) -> Vec<(String, String)> {
        self.environments.iter().find(|env| env.name == self.active_env).map(|env| env.values.clone()).unwrap_or_default()
    }

    fn save(&self, host: &Host, key: &str) {
        let value = match key {
            "request" => serde_json::to_value(&self.request).ok(),
            "environments" => Some(serde_json::json!({ "list": self.environments, "active": self.active_env })),
            "collection" => serde_json::to_value(&self.collection).ok(),
            "history" => serde_json::to_value(&self.history).ok(),
            "settings" => serde_json::to_value(&self.settings).ok(),
            _ => None,
        };
        if let Some(value) = value {
            host.storage_set(key, value);
        }
    }

    fn restore(&mut self, key: &str, value: Value) {
        if value.is_null() {
            return;
        }
        match key {
            "request" => self.request = serde_json::from_value(value).unwrap_or_default(),
            "environments" => {
                self.environments = serde_json::from_value(value.get("list").cloned().unwrap_or(Value::Null)).unwrap_or_default();
                self.active_env = value.get("active").and_then(Value::as_str).unwrap_or_default().to_string();
            }
            "collection" => self.collection = serde_json::from_value(value).unwrap_or_default(),
            "history" => self.history = serde_json::from_value(value).unwrap_or_default(),
            "settings" => self.settings = serde_json::from_value(value).unwrap_or_default(),
            _ => {}
        }
    }

    // -- sending -------------------------------------------------------------------------

    fn send(&mut self, host: &Host) {
        if self.pending.is_some() {
            host.notify_user("info", "That request is still running");
            return;
        }
        let values = self.env_values();
        let mut missing = Vec::new();
        let url = substitute(self.request.url.trim(), &values, &mut missing);
        if url.is_empty() {
            host.notify_user("warning", "Enter a URL first");
            return;
        }
        let method = self.request.method();
        let mut fetch = FetchRequest::new(&method, &url);
        for (name, value) in &self.request.headers {
            let name = substitute(name.trim(), &values, &mut missing);
            if name.is_empty() {
                continue;
            }
            fetch = fetch.header(name, substitute(value, &values, &mut missing));
        }
        if let Some((name, value)) = self.auth_header(&values, &mut missing) {
            fetch = fetch.header(name, value);
        }
        let body = substitute(&self.request.body, &values, &mut missing);
        if self.request.has_body() && !body.trim().is_empty() {
            fetch = fetch.body(body);
        }
        fetch = fetch.proxy(substitute(self.settings.proxy.trim(), &values, &mut missing));
        if self.settings.timeout > 0 {
            fetch = fetch.timeout_ms(self.settings.timeout.min(60) * 1000);
        }
        if !missing.is_empty() {
            host.notify_user("warning", format!("No value for {{{{{}}}}}", missing.join("}}, {{")));
        }
        self.outcome = None;
        self.message = format!("{method} {url}");
        self.pending = Some(host.fetch(fetch));
        self.remember(host);
        host.set_badge("…");
    }

    /// The header the chosen authorization adds, if any.
    fn auth_header(&self, values: &[(String, String)], missing: &mut Vec<String>) -> Option<(String, String)> {
        let auth = &self.request.auth;
        let fill = |text: &str, missing: &mut Vec<String>| substitute(text.trim(), values, missing);
        match auth.kind.as_str() {
            "bearer" => {
                let token = fill(&auth.token, missing);
                (!token.is_empty()).then(|| ("Authorization".to_string(), format!("Bearer {token}")))
            }
            "basic" => {
                let user = fill(&auth.user, missing);
                let password = fill(&auth.password, missing);
                (!user.is_empty() || !password.is_empty())
                    .then(|| ("Authorization".to_string(), format!("Basic {}", base64(format!("{user}:{password}").as_bytes()))))
            }
            "header" => {
                let name = fill(&auth.header, missing);
                let token = fill(&auth.token, missing);
                (!name.is_empty() && !token.is_empty()).then_some((name, token))
            }
            _ => None,
        }
    }

    fn remember(&mut self, host: &Host) {
        let entry = self.request.clone();
        self.history.retain(|old| !(old.url == entry.url && old.method() == entry.method()));
        self.history.insert(0, entry);
        self.history.truncate(HISTORY);
        self.save(host, "history");
        self.save(host, "request");
    }

    // -- panel ---------------------------------------------------------------------------

    fn draw(&self, host: &Host) {
        let mut children = vec![ui::choice("view", VIEWS, self.view())];
        if !self.message.is_empty() {
            children.push(ui::styled_text(cut(&self.message, 300), "muted"));
        }
        children.push(ui::divider());
        match self.view() {
            "environments" => children.extend(self.environments_view()),
            "collection" => children.extend(self.collection_view()),
            "settings" => children.extend(self.settings_view()),
            _ => children.extend(self.request_view()),
        }
        host.set_panel(ui::column(children));
    }

    fn request_view(&self) -> Vec<Value> {
        let mut children = Vec::new();
        let env = if self.active_env.is_empty() { "No environment".to_string() } else { format!("Environment: {}", self.active_env) };
        children.push(ui::row(vec![ui::choice("method", METHODS, self.request.method()), ui::badge(env, "info")]));
        children.push(ui::input("url", "https://api.example.com/v1/things   ({{baseUrl}} works)", self.request.url.clone()));
        children.push(ui::row(vec![
            ui::styled_button("send", if self.pending.is_some() { "Sending…" } else { "Send" }, "primary"),
            ui::button("save", "Save"),
        ]));
        children.push(ui::input("name", "Name to save it under", self.name.clone()));

        let mut headers = Vec::new();
        for (index, (name, value)) in self.request.headers.iter().enumerate() {
            headers.push(ui::row(vec![
                ui::input(format!("h.name.{index}"), "Header", name.clone()),
                ui::input(format!("h.value.{index}"), "Value", value.clone()),
                ui::styled_button(format!("h.del.{index}"), "Remove", "ghost"),
            ]));
        }
        headers.push(ui::button("h.add", "Add header"));
        children.push(ui::section("Headers", headers));

        let auth = &self.request.auth;
        let kind = if auth.kind.is_empty() { "none".to_string() } else { auth.kind.clone() };
        let mut auth_rows = vec![ui::choice("auth.kind", AUTH_KINDS, kind.clone())];
        match kind.as_str() {
            "bearer" => auth_rows.push(ui::input("auth.token", "Token   ({{token}} works)", auth.token.clone())),
            "basic" => auth_rows
                .push(ui::row(vec![ui::input("auth.user", "User", auth.user.clone()), ui::input("auth.password", "Password", auth.password.clone())])),
            "header" => auth_rows.push(ui::row(vec![
                ui::input("auth.header", "Header name (X-API-Key)", auth.header.clone()),
                ui::input("auth.token", "Value", auth.token.clone()),
            ])),
            _ => {}
        }
        children.push(ui::section("Authorization", auth_rows));

        if self.request.has_body() {
            let mut body = vec![
                ui::textarea("body", "{\n  \"name\": \"value\"\n}", self.request.body.clone(), 10),
                ui::row(vec![ui::button("body.format", "Format JSON"), ui::button("body.clear", "Clear")]),
            ];
            if let Some(note) = body_note(&self.request.body) {
                body.push(ui::styled_text(note, "muted"));
            }
            children.push(ui::section("Body", body));
        }

        if self.pending.is_some() {
            children.push(ui::spinner("Waiting for the server…"));
        }
        if let Some(outcome) = &self.outcome {
            children.push(ui::divider());
            children.push(ui::row(vec![
                ui::badge(format!("{} {}", outcome.status, outcome.status_text), if outcome.ok { "success" } else { "error" }),
                ui::styled_text(format!("{} ms", outcome.duration_ms), "muted"),
                ui::styled_text(format!("{} bytes", outcome.bytes), "muted"),
            ]));
            let mut row = vec![ui::toggle("response.headers", "Response headers", self.show_response_headers)];
            if outcome.pretty.is_some() {
                row.push(ui::toggle("response.pretty", "Pretty", self.pretty));
            }
            children.push(ui::row(row));
            if self.show_response_headers {
                let items: Vec<Value> =
                    outcome.headers.iter().map(|(name, value)| ui::item(name.clone(), name.clone(), cut(value, 200))).collect();
                children.push(ui::list("response.header.list", items, "None"));
            }
            if outcome.truncated {
                children.push(ui::styled_text("The response was longer than 4 MB and is cut off.", "muted"));
            }
            let body = match (&outcome.pretty, self.pretty) {
                (Some(pretty), true) => pretty.clone(),
                _ => outcome.body.clone(),
            };
            children.push(ui::styled_text(if body.is_empty() { "(empty body)".to_string() } else { body }, "code"));
        }

        if !self.history.is_empty() {
            let items: Vec<Value> = self
                .history
                .iter()
                .enumerate()
                .map(|(index, request)| ui::item(index.to_string(), request.url.clone(), request.method()))
                .collect();
            children.push(ui::section("History", vec![ui::list("history", items, "Nothing yet")]));
        }
        children
    }

    fn environments_view(&self) -> Vec<Value> {
        let mut children = vec![ui::styled_text(
            "Values here fill in {{name}} anywhere in a request: the URL, a header, the body, a token, even the proxy.",
            "muted",
        )];
        children.push(ui::row(vec![
            ui::input("name", "New environment name", self.name.clone()),
            ui::button("env.new", "Add environment"),
        ]));
        let items: Vec<Value> = self
            .environments
            .iter()
            .map(|env| {
                let mark = if env.name == self.active_env { " · in use" } else { "" };
                ui::item(env.name.clone(), env.name.clone(), format!("{} values{mark}", env.values.len()))
            })
            .collect();
        children.push(ui::list("env.list", items, "No environments yet"));

        if let Some(env) = self.environments.iter().find(|env| env.name == self.editing) {
            let mut rows = Vec::new();
            for (index, (key, value)) in env.values.iter().enumerate() {
                rows.push(ui::row(vec![
                    ui::input(format!("v.name.{index}"), "Name", key.clone()),
                    ui::input(format!("v.value.{index}"), "Value", value.clone()),
                    ui::styled_button(format!("v.del.{index}"), "Remove", "ghost"),
                ]));
            }
            rows.push(ui::row(vec![
                ui::button("v.add", "Add value"),
                ui::styled_button("env.use", if env.name == self.active_env { "In use" } else { "Use this one" }, "secondary"),
                ui::styled_button("env.delete", "Delete environment", "danger"),
            ]));
            children.push(ui::section(format!("{} — values", env.name), rows));
        }
        children
    }

    fn collection_view(&self) -> Vec<Value> {
        let items: Vec<Value> = self
            .collection
            .iter()
            .map(|saved| {
                let mut item = ui::item(saved.name.clone(), saved.name.clone(), format!("{} {}", saved.request.method(), saved.request.url));
                item["actions"] = serde_json::json!([{ "id": "delete", "label": "Delete" }]);
                item
            })
            .collect();
        vec![
            ui::styled_text("Saved requests. Pick one to load it back into the request view.", "muted"),
            ui::list("collection.list", items, "Nothing saved yet — fill in a request, name it and press Save."),
        ]
    }

    fn settings_view(&self) -> Vec<Value> {
        vec![
            ui::section(
                "Proxy",
                vec![
                    ui::input("settings.proxy", "http://127.0.0.1:8888   (empty: none)", self.settings.proxy.clone()),
                    ui::styled_text("HTTP and HTTPS proxies, with user:password@ if yours asks for it.", "muted"),
                ],
            ),
            ui::section(
                "Timeout",
                vec![
                    ui::input(
                        "settings.timeout",
                        "Seconds (empty: 15)",
                        if self.settings.timeout == 0 { String::new() } else { self.settings.timeout.to_string() },
                    ),
                    ui::styled_text("Agentty allows 60 seconds at most.", "muted"),
                ],
            ),
            ui::section(
                "What this plugin may do",
                vec![ui::styled_text(
                    "It runs as WebAssembly inside Agentty: no files, no processes, no network of its own. Requests go out through \
                     Agentty with the net.request permission, and what you save here is kept in this plugin's own folder.",
                    "muted",
                )],
            ),
        ]
    }

    // -- events --------------------------------------------------------------------------

    fn edit_row(rows: &mut [(String, String)], index: usize, value: String, name_side: bool) {
        if let Some(row) = rows.get_mut(index) {
            if name_side {
                row.0 = value;
            } else {
                row.1 = value;
            }
        }
    }

    fn request_event(&mut self, host: &Host, event: &UiEvent) -> bool {
        let value = event.text();
        let parts: Vec<&str> = event.element.split('.').collect();
        match parts.as_slice() {
            ["method"] => self.request.method = value,
            ["url"] => {
                self.request.url = value;
                if event.event == "submit" {
                    self.send(host);
                }
            }
            ["name"] => self.name = value,
            ["send"] => self.send(host),
            ["save"] => {
                let name = if self.name.trim().is_empty() { self.request.url.trim().to_string() } else { self.name.trim().to_string() };
                if name.is_empty() {
                    host.notify_user("warning", "Give it a name first");
                    return true;
                }
                let saved = Saved { name: name.clone(), request: self.request.clone() };
                self.collection.retain(|old| old.name != name);
                self.collection.insert(0, saved);
                self.name.clear();
                self.save(host, "collection");
                host.notify_user("success", format!("Saved {name}"));
            }
            ["h", "add"] => {
                if self.request.headers.len() < MAX_ROWS {
                    self.request.headers.push((String::new(), String::new()));
                }
            }
            ["h", what, index] => {
                let index: usize = index.parse().unwrap_or(usize::MAX);
                match *what {
                    "del" if index < self.request.headers.len() => {
                        self.request.headers.remove(index);
                    }
                    "name" => Self::edit_row(&mut self.request.headers, index, value, true),
                    "value" => Self::edit_row(&mut self.request.headers, index, value, false),
                    _ => return false,
                }
            }
            ["auth", field] => match *field {
                "kind" => self.request.auth.kind = value,
                "token" => self.request.auth.token = value,
                "user" => self.request.auth.user = value,
                "password" => self.request.auth.password = value,
                "header" => self.request.auth.header = value,
                _ => return false,
            },
            ["body"] => self.request.body = value,
            ["body", "format"] => match pretty_json(&self.request.body) {
                Some(pretty) => self.request.body = pretty,
                None => host.notify_user("warning", "That body is not JSON"),
            },
            ["body", "clear"] => self.request.body.clear(),
            ["response", "pretty"] => self.pretty = event.is_on(),
            ["response", "headers"] => self.show_response_headers = event.is_on(),
            ["history"] => {
                if let Some(request) = event.item.as_deref().and_then(|id| id.parse::<usize>().ok()).and_then(|i| self.history.get(i)) {
                    self.request = request.clone();
                }
            }
            _ => return false,
        }
        self.save(host, "request");
        true
    }

    fn environment_event(&mut self, host: &Host, event: &UiEvent) -> bool {
        let value = event.text();
        let parts: Vec<&str> = event.element.split('.').collect();
        match parts.as_slice() {
            ["name"] => {
                self.name = value;
                return true;
            }
            ["env", "new"] => {
                let name = self.name.trim().to_string();
                if name.is_empty() {
                    host.notify_user("warning", "Name it first");
                    return true;
                }
                if !self.environments.iter().any(|env| env.name == name) {
                    self.environments.push(Environment { name: name.clone(), values: Vec::new() });
                }
                self.editing = name.clone();
                if self.active_env.is_empty() {
                    self.active_env = name;
                }
                self.name.clear();
            }
            ["env", "list"] => {
                if let Some(name) = event.item.clone() {
                    self.editing = name;
                }
            }
            ["env", "use"] => self.active_env = self.editing.clone(),
            ["env", "delete"] => {
                self.environments.retain(|env| env.name != self.editing);
                if self.active_env == self.editing {
                    self.active_env.clear();
                }
                self.editing.clear();
            }
            ["v", "add"] => {
                if let Some(env) = self.environments.iter_mut().find(|env| env.name == self.editing) {
                    if env.values.len() < MAX_ROWS {
                        env.values.push((String::new(), String::new()));
                    }
                }
            }
            ["v", what, index] => {
                let index: usize = index.parse().unwrap_or(usize::MAX);
                let editing = self.editing.clone();
                let Some(env) = self.environments.iter_mut().find(|env| env.name == editing) else { return false };
                match *what {
                    "del" if index < env.values.len() => {
                        env.values.remove(index);
                    }
                    "name" => Self::edit_row(&mut env.values, index, value, true),
                    "value" => Self::edit_row(&mut env.values, index, value, false),
                    _ => return false,
                }
            }
            _ => return false,
        }
        self.save(host, "environments");
        true
    }

    fn collection_event(&mut self, host: &Host, event: &UiEvent) -> bool {
        if event.element != "collection.list" {
            return false;
        }
        let Some(name) = event.item.clone() else { return false };
        if event.action.as_deref() == Some("delete") {
            self.collection.retain(|saved| saved.name != name);
            self.save(host, "collection");
            return true;
        }
        if let Some(saved) = self.collection.iter().find(|saved| saved.name == name) {
            self.request = saved.request.clone();
            self.view = "request".to_string();
            self.save(host, "request");
        }
        true
    }

    fn settings_event(&mut self, host: &Host, event: &UiEvent) -> bool {
        let value = event.text();
        match event.element.as_str() {
            "settings.proxy" => self.settings.proxy = value,
            "settings.timeout" => self.settings.timeout = value.trim().parse().unwrap_or(0).min(60),
            _ => return false,
        }
        self.save(host, "settings");
        true
    }
}

impl Plugin for AgentRestClient {
    fn init(&mut self, host: &Host, _: &Value) {
        for key in KEYS {
            self.loading.push((host.storage_get(key), key.to_string()));
        }
    }

    fn panel_open(&mut self, host: &Host) {
        self.draw(host);
    }

    fn command(&mut self, host: &Host, _: &str) {
        host.show_panel();
        self.draw(host);
    }

    fn ui_event(&mut self, host: &Host, event: UiEvent) {
        if event.element == "view" {
            self.view = event.text();
            self.message.clear();
        } else {
            let known = match self.view() {
                "environments" => self.environment_event(host, &event),
                "collection" => self.collection_event(host, &event),
                "settings" => self.settings_event(host, &event),
                _ => self.request_event(host, &event),
            };
            if !known {
                return;
            }
        }
        self.draw(host);
    }

    fn answer(&mut self, host: &Host, id: u64, result: Result<Value, String>) {
        // A stored value coming back while the plugin starts.
        if let Some(position) = self.loading.iter().position(|(pending, _)| *pending == id) {
            let (_, key) = self.loading.remove(position);
            if let Ok(value) = result {
                self.restore(&key, value.get("value").cloned().unwrap_or(Value::Null));
            }
            if self.loading.is_empty() {
                self.draw(host);
            }
            return;
        }
        if self.pending != Some(id) {
            return;
        }
        self.pending = None;
        match result {
            Ok(response) => {
                let status = response.get("status").and_then(Value::as_u64).unwrap_or(0) as u16;
                let body = response.get("body").and_then(Value::as_str).unwrap_or("");
                let pretty = pretty_json(body).map(|pretty| cut(&pretty, BODY_SHOWN));
                self.pretty = pretty.is_some();
                self.outcome = Some(Outcome {
                    status,
                    status_text: response.get("statusText").and_then(Value::as_str).unwrap_or("").to_string(),
                    ok: (200..400).contains(&status),
                    duration_ms: response.get("durationMs").and_then(Value::as_u64).unwrap_or(0),
                    bytes: response.get("bytes").and_then(Value::as_u64).unwrap_or(0) as usize,
                    truncated: response.get("truncated").and_then(Value::as_bool).unwrap_or(false),
                    headers: response
                        .get("headers")
                        .and_then(Value::as_object)
                        .map(|headers| {
                            headers.iter().map(|(name, value)| (name.clone(), value.as_str().unwrap_or_default().to_string())).collect()
                        })
                        .unwrap_or_default(),
                    body: cut(body, BODY_SHOWN),
                    pretty,
                });
                host.set_badge(status.to_string());
            }
            Err(error) => {
                host.notify_user("error", &error);
                host.set_badge("!");
                self.message = error;
            }
        }
        self.draw(host);
    }
}

export_plugin!(AgentRestClient);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variables_are_filled_in_and_what_is_missing_is_named() {
        let values = vec![("baseUrl".to_string(), "https://api.example.com".to_string()), ("v".to_string(), "v2".to_string())];
        let mut missing = Vec::new();
        assert_eq!(substitute("{{baseUrl}}/{{v}}/things", &values, &mut missing), "https://api.example.com/v2/things");
        assert!(missing.is_empty());
        // What has no value stays as it is, and is named once.
        assert_eq!(substitute("{{baseUrl}}/{{token}}/{{token}}", &values, &mut missing), "https://api.example.com/{{token}}/{{token}}");
        assert_eq!(missing, ["token"]);
        // Half a placeholder is text.
        let mut missing = Vec::new();
        assert_eq!(substitute("{{unclosed", &values, &mut missing), "{{unclosed");
        assert_eq!(substitute("no placeholders", &values, &mut missing), "no placeholders");
        assert!(missing.is_empty());
    }

    #[test]
    fn basic_authorization_is_encoded() {
        assert_eq!(base64(b"user:password"), "dXNlcjpwYXNzd29yZA==");
        assert_eq!(base64(b"a"), "YQ==");
        assert_eq!(base64(b"ab"), "YWI=");
        assert_eq!(base64(b"abc"), "YWJj");
    }

    #[test]
    fn the_authorization_choice_becomes_a_header() {
        let mut client = AgentRestClient::default();
        let mut missing = Vec::new();
        assert!(client.auth_header(&[], &mut missing).is_none());

        client.request.auth = Auth { kind: "bearer".into(), token: "{{token}}".into(), ..Default::default() };
        let values = vec![("token".to_string(), "example_not_a_real_token".to_string())];
        assert_eq!(
            client.auth_header(&values, &mut missing),
            Some(("Authorization".to_string(), "Bearer example_not_a_real_token".to_string()))
        );

        client.request.auth = Auth { kind: "header".into(), header: "X-API-Key".into(), token: "k".into(), ..Default::default() };
        assert_eq!(client.auth_header(&values, &mut missing), Some(("X-API-Key".to_string(), "k".to_string())));

        client.request.auth = Auth { kind: "basic".into(), user: "u".into(), password: "p".into(), ..Default::default() };
        assert_eq!(client.auth_header(&values, &mut missing), Some(("Authorization".to_string(), format!("Basic {}", base64(b"u:p")))));
    }

    #[test]
    fn the_body_says_how_much_is_in_it() {
        assert!(body_note("").is_none());
        assert!(body_note("{\"a\":1}").unwrap().starts_with("1 lines"));
        assert!(body_note("{\n  \"a\": 1\n}").unwrap().starts_with("3 lines"));
        assert_eq!(pretty_json("{\"a\":1}").as_deref(), Some("{\n  \"a\": 1\n}"));
        assert!(pretty_json("not json").is_none());
    }

    #[test]
    fn a_get_carries_no_body_and_a_post_does() {
        let mut request = Request::default();
        assert_eq!(request.method(), "GET");
        assert!(!request.has_body());
        request.method = "POST".into();
        assert!(request.has_body());
    }

    #[test]
    fn what_is_saved_comes_back_the_same() {
        let mut client = AgentRestClient::default();
        client.request = Request {
            method: "POST".into(),
            url: "{{baseUrl}}/things".into(),
            headers: vec![("Accept".into(), "application/json".into())],
            body: "{\"a\":1}".into(),
            auth: Auth { kind: "bearer".into(), token: "t".into(), ..Default::default() },
        };
        let stored = serde_json::to_value(&client.request).unwrap();
        let mut restored = AgentRestClient::default();
        restored.restore("request", stored);
        assert_eq!(restored.request, client.request);

        // An environment keeps its order and which one is in use.
        client.environments = vec![Environment { name: "local".into(), values: vec![("baseUrl".into(), "http://localhost:3000".into())] }];
        client.active_env = "local".into();
        let stored = serde_json::json!({ "list": client.environments, "active": client.active_env });
        restored.restore("environments", stored);
        assert_eq!(restored.active_env, "local");
        assert_eq!(restored.env_values(), [("baseUrl".to_string(), "http://localhost:3000".to_string())]);
    }
}
