//! Declarative UI a plugin sends for its panel (`ui/setPanel`). Agentty renders it natively, so
//! plugins look like the rest of the app and can't draw outside the area they are given.

use serde::{Deserialize, Serialize};

/// Limits that keep a misbehaving plugin from stalling the UI.
pub const MAX_NODES: usize = 2_000;
pub const MAX_DEPTH: usize = 12;
pub const MAX_TEXT: usize = 20_000;
/// Lines a text area may be tall.
pub const MAX_ROWS: usize = 24;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Node {
    /// Children stacked vertically.
    Column {
        #[serde(default)]
        children: Vec<Node>,
        #[serde(default)]
        gap: Gap,
    },
    /// Children side by side (wrapping when `wrap`).
    Row {
        #[serde(default)]
        children: Vec<Node>,
        #[serde(default)]
        gap: Gap,
        #[serde(default)]
        wrap: bool,
    },
    /// A titled group.
    Section {
        title: String,
        #[serde(default)]
        children: Vec<Node>,
    },
    Text {
        text: String,
        #[serde(default)]
        style: TextStyle,
    },
    Button {
        id: String,
        label: String,
        #[serde(default)]
        icon: Option<String>,
        #[serde(default)]
        variant: Variant,
        #[serde(default)]
        disabled: bool,
    },
    /// Single-line text field: `change` events while typing (debounced), `submit` on Enter.
    Input {
        id: String,
        #[serde(default)]
        placeholder: String,
        /// Applied when it differs from the previous value the plugin sent.
        #[serde(default)]
        value: String,
        /// Lines the field shows. More than one makes it a text area: Enter adds a line, a paste
        /// keeps its line breaks, and the field is that many lines tall (at most 24).
        #[serde(default)]
        rows: usize,
    },
    /// Rows with a title, optional subtitle and per-row buttons. Clicking a row sends `select`.
    List {
        id: String,
        #[serde(default)]
        items: Vec<ListItem>,
        #[serde(default)]
        empty: Option<String>,
    },
    /// Segmented choice: `change` with the picked value.
    Choice {
        id: String,
        #[serde(default)]
        options: Vec<ChoiceOption>,
        #[serde(default)]
        value: String,
    },
    Toggle {
        id: String,
        label: String,
        #[serde(default)]
        value: bool,
    },
    Badge {
        text: String,
        #[serde(default)]
        tone: Tone,
    },
    Spinner {
        #[serde(default)]
        text: String,
    },
    Divider,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListItem {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub subtitle: Option<String>,
    /// Right-aligned secondary text (a date, a count).
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    /// Color of the icon: a finished step in green, a failed one in red.
    #[serde(default)]
    pub tone: Tone,
    #[serde(default)]
    pub actions: Vec<ItemAction>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemAction {
    pub id: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub tooltip: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChoiceOption {
    pub value: String,
    pub label: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Gap {
    None,
    Small,
    #[default]
    Medium,
    Large,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TextStyle {
    #[default]
    Body,
    Title,
    Muted,
    Small,
    Code,
    Error,
    Success,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Variant {
    Primary,
    #[default]
    Secondary,
    Ghost,
    Danger,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tone {
    #[default]
    Neutral,
    Info,
    Success,
    Warning,
    Error,
}

/// A user interaction sent back to the plugin as `ui/event`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiEvent {
    /// Id of the element (button, input, list, choice, toggle).
    pub element: String,
    /// `click`, `change`, `submit`, `select` or `action`.
    pub event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,
    /// List row the event belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<String>,
    /// Row button pressed (`action` events).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
}

impl Node {
    /// Parses a panel tree and enforces the size limits (long strings are cut, not rejected).
    pub fn from_value(value: serde_json::Value) -> Result<Node, String> {
        let mut node: Node = serde_json::from_value(value).map_err(|e| format!("invalid UI tree: {e}"))?;
        let mut count = 0;
        node.check(0, &mut count)?;
        Ok(node)
    }

    fn check(&mut self, depth: usize, count: &mut usize) -> Result<(), String> {
        *count += 1;
        if *count > MAX_NODES {
            return Err(format!("UI tree has more than {MAX_NODES} elements"));
        }
        if depth > MAX_DEPTH {
            return Err(format!("UI tree is nested deeper than {MAX_DEPTH} levels"));
        }
        let cut = |text: &mut String| {
            if let Some((index, _)) = text.char_indices().nth(MAX_TEXT) {
                text.truncate(index);
                text.push('…');
            }
        };
        match self {
            Node::Column { children, .. } | Node::Row { children, .. } => {
                for child in children {
                    child.check(depth + 1, count)?;
                }
            }
            Node::Section { title, children } => {
                cut(title);
                for child in children {
                    child.check(depth + 1, count)?;
                }
            }
            Node::Text { text, .. } | Node::Badge { text, .. } | Node::Spinner { text } => cut(text),
            Node::List { items, .. } => {
                *count += items.len();
                if *count > MAX_NODES {
                    return Err(format!("UI tree has more than {MAX_NODES} elements"));
                }
                for item in items {
                    cut(&mut item.title);
                    if let Some(subtitle) = item.subtitle.as_mut() {
                        cut(subtitle);
                    }
                }
            }
            Node::Input { value, .. } => cut(value),
            Node::Button { .. } | Node::Choice { .. } | Node::Toggle { .. } | Node::Divider => {}
        }
        Ok(())
    }

    /// Every input's id and plugin-provided value, for syncing text fields.
    pub fn inputs(&self, out: &mut Vec<InputField>) {
        match self {
            Node::Column { children, .. } | Node::Row { children, .. } | Node::Section { children, .. } => {
                children.iter().for_each(|c| c.inputs(out))
            }
            Node::Input { id, placeholder, value, rows } => {
                out.push(InputField { id: id.clone(), placeholder: placeholder.clone(), value: value.clone(), rows: (*rows).min(MAX_ROWS) })
            }
            _ => {}
        }
    }
}

/// A text field of a panel, as the window needs it.
pub struct InputField {
    pub id: String,
    pub placeholder: String,
    pub value: String,
    /// More than one: a text area of that many lines.
    pub rows: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_a_panel() {
        let tree = Node::from_value(json!({
            "type": "column",
            "children": [
                { "type": "text", "text": "Notes", "style": "title" },
                { "type": "input", "id": "q", "placeholder": "Search" },
                { "type": "list", "id": "notes", "items": [
                    { "id": "a.md", "title": "A", "actions": [{ "id": "insert", "icon": "plus" }] },
                    { "id": "done", "title": "Done", "icon": "circle-check", "tone": "success" }
                ]},
                { "type": "row", "children": [{ "type": "button", "id": "save", "label": "Save", "variant": "primary" }] },
                { "type": "divider" }
            ]
        }))
        .unwrap();
        let mut inputs = Vec::new();
        tree.inputs(&mut inputs);
        assert_eq!(inputs.len(), 1);
        assert_eq!((inputs[0].id.as_str(), inputs[0].placeholder.as_str(), inputs[0].value.as_str()), ("q", "Search", ""));
        assert_eq!(inputs[0].rows, 0, "a field is one line unless it says otherwise");
        let Node::Column { children, .. } = &tree else { panic!("not a column") };
        let Node::List { items, .. } = &children[2] else { panic!("not a list") };
        assert_eq!((items[0].tone, items[1].tone), (Tone::Neutral, Tone::Success));
    }

    #[test]
    fn a_field_can_be_a_text_area() {
        let tree = Node::from_value(json!({
            "type": "column",
            "children": [{ "type": "input", "id": "body", "rows": 10, "value": "{\n  \"a\": 1\n}" }]
        }))
        .unwrap();
        let mut inputs = Vec::new();
        tree.inputs(&mut inputs);
        assert_eq!(inputs[0].rows, 10);
        assert!(inputs[0].value.contains('\n'));
        // However tall a plugin asks for, the panel is not filled with one field.
        let tall = Node::from_value(json!({ "type": "input", "id": "b", "rows": 400 })).unwrap();
        let mut inputs = Vec::new();
        tall.inputs(&mut inputs);
        assert_eq!(inputs[0].rows, MAX_ROWS);
    }

    #[test]
    fn enforces_limits() {
        assert!(Node::from_value(json!({ "type": "canvas" })).is_err());
        let mut deep = json!({ "type": "text", "text": "x" });
        for _ in 0..=MAX_DEPTH {
            deep = json!({ "type": "column", "children": [deep] });
        }
        assert!(Node::from_value(deep).is_err());
        let many: Vec<_> = (0..=MAX_NODES).map(|i| json!({ "id": i.to_string(), "title": "t" })).collect();
        assert!(Node::from_value(json!({ "type": "list", "id": "l", "items": many })).is_err());
        let long = Node::from_value(json!({ "type": "text", "text": "a".repeat(MAX_TEXT + 10) })).unwrap();
        assert!(matches!(long, Node::Text { text, .. } if text.chars().count() == MAX_TEXT + 1));
    }
}
