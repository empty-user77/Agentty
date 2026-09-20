//! Agent Rest Client: the kind of tool Agentty itself does not provide, added as a plugin.
//!
//! Everything it does with the network goes through `net/fetch`, which needs the `net.request`
//! permission and is bounded by Agentty (methods, headers, sizes, time, redirects). The module
//! has no socket of its own, so this is the only way out.

use agentty_plugin::{export_plugin, ui, FetchRequest, Host, Plugin, UiEvent};
use serde_json::Value;

/// Response body shown in the panel; the panel itself refuses anything much larger.
const BODY_SHOWN: usize = 8_000;
/// Requests remembered, newest first.
const HISTORY: usize = 8;

const METHODS: &[(&str, &str)] = &[
    ("GET", "GET"),
    ("POST", "POST"),
    ("PUT", "PUT"),
    ("PATCH", "PATCH"),
    ("DELETE", "DELETE"),
    ("HEAD", "HEAD"),
    ("OPTIONS", "OPTIONS"),
];

#[derive(Default)]
struct AgentRestClient {
    method: String,
    url: String,
    headers: String,
    body: String,
    /// The `net/fetch` call being waited for.
    pending: Option<u64>,
    outcome: Option<Outcome>,
    history: Vec<(String, String)>,
}

struct Outcome {
    status: u16,
    status_text: String,
    ok: bool,
    duration_ms: u64,
    bytes: usize,
    truncated: bool,
    content_type: String,
    body: String,
}

impl AgentRestClient {
    fn method(&self) -> String {
        if self.method.is_empty() {
            "GET".to_string()
        } else {
            self.method.clone()
        }
    }

    /// `Accept: application/json; Authorization: Bearer …` → header pairs. Whatever cannot be read
    /// as `name: value` is left out, and Agentty refuses anything that is not a header anyway.
    fn headers(&self) -> Vec<(String, String)> {
        self.headers
            .split(';')
            .filter_map(|part| part.split_once(':'))
            .map(|(name, value)| (name.trim().to_string(), value.trim().to_string()))
            .filter(|(name, _)| !name.is_empty())
            .collect()
    }

    fn send(&mut self, host: &Host) {
        let url = self.url.trim().to_string();
        if url.is_empty() {
            host.notify_user("warning", "Enter a URL first");
            return;
        }
        if self.pending.is_some() {
            host.notify_user("info", "That request is still running");
            return;
        }
        let method = self.method();
        let mut request = FetchRequest::new(&method, &url);
        for (name, value) in self.headers() {
            request = request.header(name, value);
        }
        let body = self.body.trim();
        if !body.is_empty() && !matches!(method.as_str(), "GET" | "HEAD") {
            request = request.body(body);
        }
        self.outcome = None;
        self.pending = Some(host.fetch(request));
        self.remember(&method, &url);
        host.set_badge("…");
        self.draw(host);
    }

    fn remember(&mut self, method: &str, url: &str) {
        let entry = (method.to_string(), url.to_string());
        self.history.retain(|old| *old != entry);
        self.history.insert(0, entry);
        self.history.truncate(HISTORY);
    }

    fn draw(&self, host: &Host) {
        let mut children = vec![
            ui::section(
                "Request",
                vec![
                    ui::row(vec![ui::choice("method", METHODS, self.method())]),
                    ui::input("url", "https://api.example.com/v1/things", self.url.clone()),
                    ui::input("headers", "Accept: application/json; Authorization: Bearer …", self.headers.clone()),
                    ui::input("body", "Request body (POST, PUT, PATCH)", self.body.clone()),
                    ui::row(vec![
                        ui::styled_button("send", if self.pending.is_some() { "Sending…" } else { "Send" }, "primary"),
                        ui::button("clear", "Clear"),
                    ]),
                ],
            ),
            ui::divider(),
        ];
        if self.pending.is_some() {
            children.push(ui::spinner("Waiting for the server…"));
        }
        if let Some(outcome) = &self.outcome {
            let tone = if outcome.ok { "success" } else { "error" };
            children.push(ui::row(vec![
                ui::badge(format!("{} {}", outcome.status, outcome.status_text), tone),
                ui::styled_text(format!("{} ms", outcome.duration_ms), "muted"),
                ui::styled_text(format!("{} bytes", outcome.bytes), "muted"),
                ui::styled_text(outcome.content_type.clone(), "muted"),
            ]));
            if outcome.truncated {
                children.push(ui::styled_text("The response was longer than 4 MB and is cut off.", "muted"));
            }
            children.push(ui::styled_text(outcome.body.clone(), "code"));
        }
        if !self.history.is_empty() {
            let items: Vec<Value> =
                self.history.iter().enumerate().map(|(index, (method, url))| ui::item(index.to_string(), url, method)).collect();
            children.push(ui::divider());
            children.push(ui::section("Recent", vec![ui::list("history", items, "Nothing yet")]));
        }
        host.set_panel(ui::column(children));
    }
}

impl Plugin for AgentRestClient {
    fn panel_open(&mut self, host: &Host) {
        self.draw(host);
    }

    fn command(&mut self, host: &Host, _command: &str) {
        host.show_panel();
        self.draw(host);
    }

    fn ui_event(&mut self, host: &Host, event: UiEvent) {
        match event.element.as_str() {
            "method" => self.method = event.text(),
            "url" => {
                self.url = event.text();
                if event.event == "submit" {
                    return self.send(host);
                }
                return;
            }
            "headers" => {
                self.headers = event.text();
                return;
            }
            "body" => {
                self.body = event.text();
                return;
            }
            "send" => return self.send(host),
            "clear" => {
                self.outcome = None;
                self.body.clear();
                host.set_badge("");
            }
            "history" => {
                // The row's id is its place in the list.
                let picked = event.item.as_deref().and_then(|id| id.parse::<usize>().ok()).and_then(|index| self.history.get(index));
                if let Some((method, url)) = picked.cloned() {
                    self.method = method;
                    self.url = url;
                }
            }
            _ => return,
        }
        self.draw(host);
    }

    fn answer(&mut self, host: &Host, id: u64, result: Result<Value, String>) {
        if self.pending != Some(id) {
            return;
        }
        self.pending = None;
        match result {
            Ok(response) => {
                let status = response.get("status").and_then(Value::as_u64).unwrap_or(0) as u16;
                let body = response.get("body").and_then(Value::as_str).unwrap_or("");
                let shown: String = body.chars().take(BODY_SHOWN).collect();
                let cut = shown.chars().count() < body.chars().count();
                self.outcome = Some(Outcome {
                    status,
                    status_text: response.get("statusText").and_then(Value::as_str).unwrap_or("").to_string(),
                    ok: (200..400).contains(&status),
                    duration_ms: response.get("durationMs").and_then(Value::as_u64).unwrap_or(0),
                    bytes: response.get("bytes").and_then(Value::as_u64).unwrap_or(0) as usize,
                    truncated: response.get("truncated").and_then(Value::as_bool).unwrap_or(false) || cut,
                    content_type: response
                        .get("headers")
                        .and_then(|headers| headers.get("content-type"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    body: if shown.is_empty() { "(empty body)".to_string() } else { shown },
                });
                host.set_badge(status.to_string());
            }
            Err(error) => {
                host.notify_user("error", &error);
                host.set_badge("!");
                self.outcome =
                    Some(Outcome { status: 0, status_text: "failed".into(), ok: false, duration_ms: 0, bytes: 0, truncated: false, content_type: String::new(), body: error });
            }
        }
        self.draw(host);
    }
}

export_plugin!(AgentRestClient);
