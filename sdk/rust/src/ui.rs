//! The panel, built as a tree. Each function makes one node of the protocol's UI tree; see
//! `docs/plugins/protocol.md` for what Agentty draws.

use serde_json::{json, Value};

/// A column of nodes.
pub fn column(children: Vec<Value>) -> Value {
    json!({ "type": "column", "children": children })
}

/// A row of nodes, wrapping onto more lines when there is no room.
pub fn row(children: Vec<Value>) -> Value {
    json!({ "type": "row", "children": children, "wrap": true })
}

/// A titled group.
pub fn section(title: impl Into<String>, children: Vec<Value>) -> Value {
    json!({ "type": "section", "title": title.into(), "children": children })
}

pub fn text(text: impl Into<String>) -> Value {
    json!({ "type": "text", "text": text.into() })
}

/// `body`, `title`, `muted`, `small`, `code`, `error` or `success`.
pub fn styled_text(text: impl Into<String>, style: &str) -> Value {
    json!({ "type": "text", "text": text.into(), "style": style })
}

pub fn button(id: impl Into<String>, label: impl Into<String>) -> Value {
    json!({ "type": "button", "id": id.into(), "label": label.into() })
}

/// `primary`, `secondary`, `ghost` or `danger`.
pub fn styled_button(id: impl Into<String>, label: impl Into<String>, variant: &str) -> Value {
    json!({ "type": "button", "id": id.into(), "label": label.into(), "variant": variant })
}

pub fn input(id: impl Into<String>, placeholder: impl Into<String>, value: impl Into<String>) -> Value {
    json!({ "type": "input", "id": id.into(), "placeholder": placeholder.into(), "value": value.into() })
}

/// A field of several lines (a request body, a note): Enter adds a line and a paste keeps its
/// own. At most 24 rows.
pub fn textarea(id: impl Into<String>, placeholder: impl Into<String>, value: impl Into<String>, rows: usize) -> Value {
    json!({ "type": "input", "id": id.into(), "placeholder": placeholder.into(), "value": value.into(), "rows": rows.max(2) })
}

pub fn choice(id: impl Into<String>, options: &[(&str, &str)], value: impl Into<String>) -> Value {
    let options: Vec<Value> = options.iter().map(|(v, label)| json!({ "value": v, "label": label })).collect();
    json!({ "type": "choice", "id": id.into(), "options": options, "value": value.into() })
}

pub fn toggle(id: impl Into<String>, label: impl Into<String>, on: bool) -> Value {
    json!({ "type": "toggle", "id": id.into(), "label": label.into(), "value": on })
}

pub fn list(id: impl Into<String>, items: Vec<Value>, empty: impl Into<String>) -> Value {
    json!({ "type": "list", "id": id.into(), "items": items, "empty": empty.into() })
}

pub fn item(id: impl Into<String>, title: impl Into<String>, subtitle: impl Into<String>) -> Value {
    json!({ "id": id.into(), "title": title.into(), "subtitle": subtitle.into() })
}

/// `neutral`, `info`, `success`, `warning` or `error`.
pub fn badge(text: impl Into<String>, tone: &str) -> Value {
    json!({ "type": "badge", "text": text.into(), "tone": tone })
}

pub fn spinner(text: impl Into<String>) -> Value {
    json!({ "type": "spinner", "text": text.into() })
}

pub fn divider() -> Value {
    json!({ "type": "divider" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nodes_carry_their_type() {
        let tree = column(vec![section("Request", vec![input("url", "https://…", ""), button("go", "Send")]), divider()]);
        assert_eq!(tree["type"], "column");
        assert_eq!(tree["children"][0]["title"], "Request");
        assert_eq!(tree["children"][0]["children"][1]["id"], "go");
        assert_eq!(tree["children"][1]["type"], "divider");
        assert_eq!(choice("m", &[("GET", "GET")], "GET")["options"][0]["value"], "GET");
        assert_eq!(textarea("body", "…", "", 10)["rows"], 10);
    }
}
