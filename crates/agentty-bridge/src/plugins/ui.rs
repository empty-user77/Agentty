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
        /// Opens these as a menu instead of sending `click`; picking one sends `select` with its
        /// value (API 4).
        #[serde(default)]
        menu: Vec<ChoiceOption>,
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
        /// Drawn in the monospace font: code, JSON, a script (API 4).
        #[serde(default)]
        mono: bool,
        /// Suggestions for the word being typed, shown under the field: any that contain it
        /// (case aside) are listed, ↑ ↓ pick, Enter or Tab inserts. A `{{` right before the word
        /// and a `}}` right after it are replaced too, so `{{base` becomes `{{baseUrl}}` (API 4).
        #[serde(default)]
        completions: Vec<Completion>,
        /// A text area's text colored as this language (`json`, `xml`, `js`, …) while it is
        /// edited (API 4).
        #[serde(default)]
        language: Option<String>,
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
    /// Steps drawn as cards joined top to bottom, for what an automation does in order. Clicking
    /// a step sends `select` with its id.
    Flow {
        id: String,
        #[serde(default)]
        steps: Vec<FlowStep>,
        /// Steps can be dragged onto another: `move` with the dragged step as `item` and the one
        /// it was dropped on as `value` (API 4).
        #[serde(default)]
        reorderable: bool,
        /// Hovering the gap after a step shows "+": these as a menu, and picking one sends
        /// `insert` with its value and the step above as `item` (API 4).
        #[serde(default, rename = "insertMenu")]
        insert_menu: Vec<ChoiceOption>,
    },
    /// Settings shown in a card beside the panel (over the page next to it) instead of in it:
    /// the one found in the tree is open, none is closed. Its close button sends `close`.
    Popover {
        id: String,
        title: String,
        #[serde(default)]
        children: Vec<Node>,
    },
    /// A raised box around what belongs together, with an optional heading (API 4).
    Card {
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        subtitle: Option<String>,
        #[serde(default)]
        icon: Option<String>,
        /// Colors the icon, and the card's edge when it is not `neutral`.
        #[serde(default)]
        tone: Tone,
        #[serde(default)]
        children: Vec<Node>,
    },
    /// Children in equal columns, wrapping onto new rows (API 4).
    Grid {
        #[serde(default)]
        children: Vec<Node>,
        /// 1 to [`MAX_COLUMNS`]; 2 when left out.
        #[serde(default = "two")]
        columns: usize,
        #[serde(default)]
        gap: Gap,
        /// Each column's width, which sets the number of columns: `"240px"` is fixed, `"2"` takes
        /// twice the share of a `"1"` — a sidebar beside a page is `["240px", "1"]` (API 4).
        #[serde(default)]
        widths: Vec<String>,
        /// Names the grid for `resize` events.
        #[serde(default)]
        id: Option<String>,
        /// How cells of a row line up: `start` (default) or `center` — a checkbox beside fields.
        #[serde(default)]
        align: Align,
        /// The first fixed column gets a handle on its right edge: dragging it resizes the
        /// column, and sends `resize` with the new width in pixels once the drag ends, for the
        /// plugin to keep (API 4).
        #[serde(default)]
        resizable: bool,
        /// Takes the height left in the panel, and each column scrolls on its own: a sidebar that
        /// stays put while the page beside it scrolls. In the panel's top column (or columns
        /// inside it); what comes before it stays on screen (API 4).
        #[serde(default)]
        fill: bool,
    },
    /// A tab strip. The plugin sends only the picked tab's content as `children`; picking another
    /// sends `change` with its id (API 4).
    Tabs {
        id: String,
        #[serde(default)]
        tabs: Vec<TabItem>,
        #[serde(default)]
        value: String,
        #[serde(default)]
        children: Vec<Node>,
        /// In a `fill` grid's column: the strip stays and only the picked tab's content scrolls
        /// (API 4).
        #[serde(default)]
        fill: bool,
        /// A `+` at the end of the strip that opens these as a menu; picking one sends `add`
        /// with its value (API 4).
        #[serde(default, rename = "addMenu")]
        add_menu: Vec<ChoiceOption>,
    },
    /// Rows under column headings. Clicking a row sends `select` with its id (API 4).
    Table {
        id: String,
        #[serde(default)]
        columns: Vec<TableColumn>,
        #[serde(default)]
        rows: Vec<TableRow>,
        #[serde(default)]
        empty: Option<String>,
        /// The row drawn highlighted.
        #[serde(default)]
        selected: Option<String>,
    },
    /// Label and value pairs, one per line (API 4).
    KeyValue {
        #[serde(default)]
        items: Vec<KeyValueItem>,
    },
    /// One number that matters, large, with what it is and how it moved (API 4).
    Stat {
        label: String,
        value: String,
        #[serde(default)]
        detail: Option<String>,
        #[serde(default)]
        icon: Option<String>,
        /// Colors the detail and the icon.
        #[serde(default)]
        tone: Tone,
    },
    /// A bar filled `value` of the way (0 to 1) (API 4).
    Progress {
        #[serde(default)]
        value: f64,
        #[serde(default)]
        label: Option<String>,
        #[serde(default)]
        detail: Option<String>,
        #[serde(default)]
        tone: Tone,
    },
    /// A tinted note: a hint, a warning, what went wrong (API 4).
    Callout {
        text: String,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        icon: Option<String>,
        #[serde(default = "info")]
        tone: Tone,
    },
    /// A drop-down of options: `change` with the picked value (API 4).
    Select {
        id: String,
        #[serde(default)]
        options: Vec<ChoiceOption>,
        #[serde(default)]
        value: String,
        #[serde(default)]
        placeholder: Option<String>,
        #[serde(default)]
        disabled: bool,
    },
    /// A box to tick: `change` with the new boolean (API 4).
    Checkbox {
        id: String,
        label: String,
        #[serde(default)]
        value: bool,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        disabled: bool,
    },
    /// Monospaced text, colored by `language` when Agentty knows it, with a copy button (API 4).
    Code {
        text: String,
        /// A language name or file extension: `rust`, `json`, `ts`, `sh`, …
        #[serde(default)]
        language: Option<String>,
        #[serde(default)]
        title: Option<String>,
    },
}

fn two() -> usize {
    2
}

fn info() -> Tone {
    Tone::Info
}

/// Columns a `grid` may have.
pub const MAX_COLUMNS: usize = 6;

/// One tab of a `tabs` strip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TabItem {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub icon: Option<String>,
    /// A count or a word after the label.
    #[serde(default)]
    pub badge: Option<String>,
    /// Shows a close button, which sends `close` with the tab's id as `value`.
    #[serde(default)]
    pub closable: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TableColumn {
    pub label: String,
    #[serde(default)]
    pub align: Align,
    /// Share of the width against the other columns (1 when left out).
    #[serde(default = "one")]
    pub grow: u32,
}

fn one() -> u32 {
    1
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Align {
    #[default]
    Start,
    Center,
    End,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TableRow {
    pub id: String,
    /// One per column, in the same order.
    #[serde(default)]
    pub cells: Vec<String>,
    /// Colors the row's first cell: a failed run in red.
    #[serde(default)]
    pub tone: Tone,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyValueItem {
    pub label: String,
    pub value: String,
    #[serde(default)]
    pub tone: Tone,
    /// Monospaced: ids, hashes, paths.
    #[serde(default)]
    pub mono: bool,
}

/// One card of a `flow`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowStep {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub subtitle: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub state: FlowState,
    /// Picked by the user: drawn highlighted (its settings are usually shown beside the flow).
    #[serde(default)]
    pub selected: bool,
    /// An optional step off the main line: drawn indented, branching from the step above it.
    #[serde(default)]
    pub side: bool,
}

/// How a `flow` step is drawn: `off` dimmed and joined by a faint line, `active` with a spinner.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FlowState {
    Off,
    #[default]
    On,
    Active,
    Done,
    Error,
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
    /// How far the row is indented, for a tree (0 to [`MAX_LIST_DEPTH`]).
    #[serde(default)]
    pub depth: u8,
    /// A short label before the title in its own color — an HTTP method, a status (at most 8
    /// characters).
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub tag_tone: Tone,
    /// Drawn highlighted: the row whose page is open (API 4).
    #[serde(default)]
    pub selected: bool,
}

/// The deepest a list row is indented.
pub const MAX_LIST_DEPTH: u8 = 8;
/// The narrowest and widest a fixed grid column may be, in pixels.
pub const GRID_PX: (f32, f32) = (16., 1200.);

/// A grid column's width, as `widths` gives it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GridWidth {
    /// Pixels, fixed.
    Px(f32),
    /// A share of what the fixed columns leave.
    Share(u32),
}

impl GridWidth {
    /// `"240px"` or `"2"`; anything else is a share of 1.
    pub fn parse(text: &str) -> GridWidth {
        let text = text.trim();
        match text.strip_suffix("px").and_then(|n| n.trim().parse::<f32>().ok()) {
            Some(px) if px.is_finite() => GridWidth::Px(px.clamp(GRID_PX.0, GRID_PX.1)),
            _ => GridWidth::Share(text.parse::<u32>().unwrap_or(1).clamp(1, 12)),
        }
    }
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

/// One suggestion of an `input`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Completion {
    /// What is matched against the word typed, and shown.
    pub label: String,
    /// What replaces the word (and the braces around it); the label when left out.
    #[serde(default)]
    pub insert: Option<String>,
    /// A hint on the right: the variable's value, where it comes from.
    #[serde(default)]
    pub detail: Option<String>,
}

/// Suggestions an `input` may carry.
pub const MAX_COMPLETIONS: usize = 500;

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
    /// Id of the element (button, input, list, choice, toggle, tabs, table, select, checkbox).
    pub element: String,
    /// `click`, `change`, `submit`, `select`, `action` or `close`.
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
        // Everything the plugin wrote that reaches the screen. A node's cost to draw is the
        // number of things in it, not the number of nodes it is, so a `choice` of half a million
        // options is one node and would wedge the window while it laid them out.
        let more = |count: &mut usize, n: usize| -> Result<(), String> {
            *count += n;
            if *count > MAX_NODES {
                return Err(format!("UI tree has more than {MAX_NODES} elements"));
            }
            Ok(())
        };
        match self {
            Node::Column { children, .. } | Node::Row { children, .. } => {
                for child in children {
                    child.check(depth + 1, count)?;
                }
            }
            Node::Section { title, children } | Node::Popover { title, children, .. } => {
                cut(title);
                for child in children {
                    child.check(depth + 1, count)?;
                }
            }
            Node::Text { text, .. } | Node::Badge { text, .. } | Node::Spinner { text } => cut(text),
            Node::List { items, empty, .. } => {
                if let Some(empty) = empty.as_mut() {
                    cut(empty);
                }
                more(count, items.len())?;
                for item in items {
                    more(count, item.actions.len())?;
                    item.depth = item.depth.min(MAX_LIST_DEPTH);
                    if let Some(tag) = item.tag.as_mut() {
                        if let Some((at, _)) = tag.char_indices().nth(8) {
                            tag.truncate(at);
                        }
                    }
                    cut(&mut item.title);
                    for text in [item.subtitle.as_mut(), item.detail.as_mut(), item.icon.as_mut()].into_iter().flatten() {
                        cut(text);
                    }
                    for action in &mut item.actions {
                        for text in [action.label.as_mut(), action.icon.as_mut(), action.tooltip.as_mut()].into_iter().flatten() {
                            cut(text);
                        }
                    }
                }
            }
            Node::Input { value, placeholder, completions, language, .. } => {
                cut(value);
                cut(placeholder);
                if let Some(language) = language.as_mut() {
                    cut(language);
                }
                completions.truncate(MAX_COMPLETIONS);
                more(count, completions.len())?;
                for completion in completions {
                    cut(&mut completion.label);
                    for text in [completion.insert.as_mut(), completion.detail.as_mut()].into_iter().flatten() {
                        cut(text);
                    }
                }
            }
            Node::Button { label, icon, menu, .. } => {
                more(count, menu.len())?;
                for option in menu.iter_mut() {
                    cut(&mut option.label);
                    cut(&mut option.value);
                }
                cut(label);
                if let Some(icon) = icon.as_mut() {
                    cut(icon);
                }
            }
            Node::Choice { options, .. } => {
                more(count, options.len())?;
                for option in options {
                    cut(&mut option.label);
                    cut(&mut option.value);
                }
            }
            Node::Toggle { label, .. } => cut(label),
            Node::Flow { steps, insert_menu, .. } => {
                more(count, steps.len() + insert_menu.len())?;
                for option in insert_menu.iter_mut() {
                    cut(&mut option.label);
                    cut(&mut option.value);
                }
                for step in steps {
                    cut(&mut step.title);
                    for text in [step.subtitle.as_mut(), step.icon.as_mut()].into_iter().flatten() {
                        cut(text);
                    }
                }
            }
            Node::Divider => {}
            Node::Card { title, subtitle, icon, children, .. } => {
                for text in [title.as_mut(), subtitle.as_mut(), icon.as_mut()].into_iter().flatten() {
                    cut(text);
                }
                for child in children {
                    child.check(depth + 1, count)?;
                }
            }
            Node::Grid { children, columns, widths, id, .. } => {
                if let Some(id) = id.as_mut() {
                    cut(id);
                }
                widths.truncate(MAX_COLUMNS);
                for width in widths.iter_mut() {
                    cut(width);
                }
                // Widths say how many columns there are.
                *columns = if widths.is_empty() { (*columns).clamp(1, MAX_COLUMNS) } else { widths.len() };
                for child in children {
                    child.check(depth + 1, count)?;
                }
            }
            Node::Tabs { tabs, value, children, add_menu, .. } => {
                more(count, tabs.len() + add_menu.len())?;
                for option in add_menu.iter_mut() {
                    cut(&mut option.label);
                    cut(&mut option.value);
                }
                cut(value);
                for tab in tabs {
                    cut(&mut tab.id);
                    cut(&mut tab.label);
                    for text in [tab.icon.as_mut(), tab.badge.as_mut()].into_iter().flatten() {
                        cut(text);
                    }
                }
                for child in children {
                    child.check(depth + 1, count)?;
                }
            }
            Node::Table { columns, rows, empty, selected, .. } => {
                more(count, columns.len())?;
                for column in columns.iter_mut() {
                    cut(&mut column.label);
                    column.grow = column.grow.clamp(1, 12);
                }
                for text in [empty.as_mut(), selected.as_mut()].into_iter().flatten() {
                    cut(text);
                }
                for row in rows {
                    // A row draws one cell per column and no more.
                    row.cells.truncate(columns.len());
                    more(count, 1 + row.cells.len())?;
                    cut(&mut row.id);
                    row.cells.iter_mut().for_each(cut);
                }
            }
            Node::KeyValue { items } => {
                more(count, items.len())?;
                for item in items {
                    cut(&mut item.label);
                    cut(&mut item.value);
                }
            }
            Node::Stat { label, value, detail, icon, .. } => {
                cut(label);
                cut(value);
                for text in [detail.as_mut(), icon.as_mut()].into_iter().flatten() {
                    cut(text);
                }
            }
            Node::Progress { value, label, detail, .. } => {
                *value = if value.is_finite() { value.clamp(0., 1.) } else { 0. };
                for text in [label.as_mut(), detail.as_mut()].into_iter().flatten() {
                    cut(text);
                }
            }
            Node::Callout { text, title, icon, .. } => {
                cut(text);
                for text in [title.as_mut(), icon.as_mut()].into_iter().flatten() {
                    cut(text);
                }
            }
            Node::Select { options, value, placeholder, .. } => {
                more(count, options.len())?;
                cut(value);
                if let Some(placeholder) = placeholder.as_mut() {
                    cut(placeholder);
                }
                for option in options {
                    cut(&mut option.label);
                    cut(&mut option.value);
                }
            }
            Node::Checkbox { label, description, .. } => {
                cut(label);
                if let Some(description) = description.as_mut() {
                    cut(description);
                }
            }
            Node::Code { text, language, title } => {
                cut(text);
                for text in [language.as_mut(), title.as_mut()].into_iter().flatten() {
                    cut(text);
                }
            }
        }
        Ok(())
    }

    /// The nodes this one holds, for walking the tree.
    pub fn children(&self) -> &[Node] {
        match self {
            Node::Column { children, .. }
            | Node::Row { children, .. }
            | Node::Section { children, .. }
            | Node::Popover { children, .. }
            | Node::Card { children, .. }
            | Node::Grid { children, .. }
            | Node::Tabs { children, .. } => children,
            _ => &[],
        }
    }

    /// Whether this node takes the panel's height (a `fill` grid, or a column holding one).
    pub fn fills_height(&self) -> bool {
        match self {
            Node::Grid { fill, .. } => *fill,
            Node::Column { children, .. } => children.iter().any(Node::fills_height),
            _ => false,
        }
    }

    /// The popover the tree holds, if any (the first one: there is room for one beside a panel).
    pub fn popover(&self) -> Option<&Node> {
        match self {
            Node::Popover { .. } => Some(self),
            _ => self.children().iter().find_map(Node::popover),
        }
    }

    /// Every input's id and plugin-provided value, for syncing text fields.
    pub fn inputs(&self, out: &mut Vec<InputField>) {
        match self {
            Node::Input { id, placeholder, value, rows, completions, .. } => out.push(InputField {
                id: id.clone(),
                placeholder: placeholder.clone(),
                value: value.clone(),
                rows: (*rows).min(MAX_ROWS),
                completions: completions.clone(),
            }),
            _ => self.children().iter().for_each(|c| c.inputs(out)),
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
    pub completions: Vec<Completion>,
}

/// The word a suggestion would replace, with the cursor at `cursor`: its range in `text` (a `{{`
/// before it and a `}}` after it included) and the word itself. None when the cursor is not at
/// the end of a word, or of a `{{` just typed.
pub fn completion_word(text: &str, cursor: usize) -> Option<(std::ops::Range<usize>, &str)> {
    if cursor > text.len() || !text.is_char_boundary(cursor) {
        return None;
    }
    let is_word = |c: char| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '$');
    let before = &text[..cursor];
    let start = before.char_indices().rev().take_while(|(_, c)| is_word(*c)).last().map_or(cursor, |(i, _)| i);
    // The cursor in the middle of a word is editing it, not finishing it.
    if text[cursor..].chars().next().is_some_and(is_word) {
        return None;
    }
    let word = &text[start..cursor];
    let braced = before[..start].ends_with("{{");
    if word.is_empty() && !braced {
        return None;
    }
    let from = if braced { start - 2 } else { start };
    let to = if braced && text[cursor..].starts_with("}}") { cursor + 2 } else { cursor };
    Some((from..to, word))
}

/// The suggestions that fit `word`, best first: those starting with it, then those holding it.
/// Typed without braces a word takes two letters before anything is offered; one that is already
/// a whole suggestion offers nothing.
pub fn matching_completions<'a>(completions: &'a [Completion], word: &str, braced: bool) -> Vec<&'a Completion> {
    if !braced && word.chars().count() < 2 {
        return Vec::new();
    }
    let lower = word.to_lowercase();
    let mut starts = Vec::new();
    let mut holds = Vec::new();
    for completion in completions {
        let label = completion.label.to_lowercase();
        if label == lower && !braced {
            return Vec::new();
        }
        if label.starts_with(&lower) {
            starts.push(completion);
        } else if label.contains(&lower) {
            holds.push(completion);
        }
    }
    starts.extend(holds);
    starts
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
    fn the_word_a_suggestion_replaces() {
        let text = "{{base}}/users";
        assert_eq!(completion_word(text, 6), Some((0..8, "base")), "braces on both sides go");
        assert_eq!(completion_word("{{ba", 4), Some((0..4, "ba")));
        assert_eq!(completion_word("{{", 2), Some((0..2, "")), "a brace pair alone lists everything");
        assert_eq!(completion_word("http://ho", 9), Some((7..9, "ho")));
        assert_eq!(completion_word("abc", 1), None, "inside a word");
        assert_eq!(completion_word("a ", 2), None);
        let all = vec![
            Completion { label: "baseUrl".into(), insert: Some("{{baseUrl}}".into()), detail: None },
            Completion { label: "authBase".into(), insert: None, detail: None },
            Completion { label: "token".into(), insert: None, detail: None },
        ];
        let labels = |found: Vec<&Completion>| found.iter().map(|c| c.label.clone()).collect::<Vec<_>>();
        assert_eq!(labels(matching_completions(&all, "base", true)), ["baseUrl", "authBase"], "prefix first");
        assert_eq!(labels(matching_completions(&all, "", true)).len(), 3);
        assert!(matching_completions(&all, "b", false).is_empty(), "one letter without braces is too little");
        assert!(matching_completions(&all, "token", false).is_empty(), "already whole");
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

    /// The cost of drawing a node is the number of things in it, not the number of nodes it is.
    /// A `choice` of half a million options was one node, under every limit, and would have wedged
    /// the window laying them out; so was a list item carrying a thousand buttons.
    #[test]
    fn what_a_node_holds_counts_towards_the_limit_too() {
        let options: Vec<_> = (0..=MAX_NODES).map(|i| json!({ "value": i.to_string(), "label": "x" })).collect();
        let err = Node::from_value(json!({ "type": "choice", "id": "c", "options": options })).unwrap_err();
        assert!(err.contains("more than"), "{err}");

        let actions: Vec<_> = (0..=MAX_NODES).map(|i| json!({ "id": i.to_string() })).collect();
        let err =
            Node::from_value(json!({ "type": "list", "id": "l", "items": [{ "id": "a", "title": "t", "actions": actions }] })).unwrap_err();
        assert!(err.contains("more than"), "{err}");

        // And a reasonable one still passes.
        let ok = json!({ "type": "choice", "id": "c", "options": [{ "value": "a", "label": "A" }] });
        assert!(Node::from_value(ok).is_ok());
    }

    /// Every string a plugin writes reaches the screen, so every one of them is cut.
    #[test]
    fn every_string_a_plugin_writes_is_cut() {
        let long = "a".repeat(MAX_TEXT * 2);
        let tree = json!({ "type": "column", "children": [
            { "type": "button", "id": "b", "label": long, "icon": long },
            { "type": "toggle", "id": "t", "label": long },
            { "type": "input", "id": "i", "value": long, "placeholder": long },
            { "type": "choice", "id": "c", "options": [{ "value": long, "label": long }] },
            { "type": "list", "id": "l", "empty": long, "items": [
                { "id": "x", "title": long, "subtitle": long, "detail": long, "icon": long,
                  "actions": [{ "id": "a", "label": long, "icon": long, "tooltip": long }] },
            ] },
        ] });
        let node = Node::from_value(tree).expect("it is a tree");
        let drawn = serde_json::to_string(&node).expect("it serializes");
        let longest = drawn.split('"').map(|part| part.chars().count()).max().unwrap_or(0);
        assert!(longest <= MAX_TEXT + 1, "a string of {longest} characters reaches the panel");
    }

    #[test]
    fn parses_the_api_4_elements() {
        let tree = Node::from_value(json!({ "type": "tabs", "id": "t", "value": "runs",
            "tabs": [{ "id": "runs", "label": "Runs", "badge": "3" }, { "id": "settings", "label": "Settings" }],
            "children": [
                { "type": "grid", "columns": 40, "children": [
                    { "type": "stat", "label": "Passed", "value": "128", "detail": "+4", "tone": "success" },
                    { "type": "stat", "label": "Failed", "value": "2" },
                ] },
                { "type": "card", "title": "Latest", "children": [
                    { "type": "keyValue", "items": [{ "label": "Commit", "value": "ebfe735", "mono": true }] },
                    { "type": "progress", "value": 3.5, "label": "Upload" },
                    { "type": "input", "id": "note" },
                ] },
                { "type": "table", "id": "tbl", "columns": [{ "label": "Name" }, { "label": "Time", "align": "end", "grow": 99 }],
                  "rows": [{ "id": "a", "cells": ["build", "2m", "extra"] }] },
                { "type": "callout", "text": "Heads up" },
                { "type": "select", "id": "s", "options": [{ "value": "a", "label": "A" }], "value": "a" },
                { "type": "checkbox", "id": "c", "label": "Notify me", "value": true },
                { "type": "code", "text": "fn main() {}", "language": "rust" },
            ] }))
        .unwrap();
        let mut inputs = Vec::new();
        tree.inputs(&mut inputs);
        assert_eq!(inputs.len(), 1, "a field inside a card inside a tab is found");
        let Node::Tabs { children, .. } = &tree else { panic!("tabs") };
        assert!(matches!(children[0], Node::Grid { columns: MAX_COLUMNS, .. }), "columns are capped");
        let Node::Card { children: card, .. } = &children[1] else { panic!("card") };
        assert!(matches!(card[1], Node::Progress { value, .. } if value == 1.), "progress is clamped to 0..1");
        let Node::Table { rows, columns, .. } = &children[2] else { panic!("table") };
        assert_eq!(rows[0].cells.len(), 2, "a row has no more cells than there are columns");
        assert_eq!((columns[0].grow, columns[1].grow, columns[1].align), (1, 12, Align::End));
        assert!(matches!(children[3], Node::Callout { tone: Tone::Info, .. }), "a callout is a hint unless it says otherwise");
    }

    #[test]
    fn layout_and_tree_additions() {
        let tree = Node::from_value(json!({ "type": "column", "children": [
            { "type": "grid", "widths": ["240px", "2", "nonsense", "9000px", "1", "1", "1", "1"], "children": [] },
            { "type": "list", "id": "tree", "items": [
                { "id": "a", "title": "Create", "depth": 40, "tag": "OPTIONS-LONG", "tagTone": "warning" },
            ] },
            { "type": "tabs", "id": "t", "value": "a", "tabs": [{ "id": "a", "label": "GET Users", "closable": true }] },
            { "type": "input", "id": "body", "rows": 8, "mono": true },
        ] }))
        .unwrap();
        let Node::Column { children, .. } = &tree else { panic!("column") };
        let Node::Grid { columns, widths, .. } = &children[0] else { panic!("grid") };
        assert_eq!((*columns, widths.len()), (MAX_COLUMNS, MAX_COLUMNS), "widths set the columns, capped");
        let parsed: Vec<GridWidth> = widths.iter().map(|w| GridWidth::parse(w)).collect();
        assert_eq!(&parsed[..4], &[GridWidth::Px(240.), GridWidth::Share(2), GridWidth::Share(1), GridWidth::Px(GRID_PX.1)]);
        let Node::List { items, .. } = &children[1] else { panic!("list") };
        assert_eq!((items[0].depth, items[0].tag.as_deref(), items[0].tag_tone), (MAX_LIST_DEPTH, Some("OPTIONS-"), Tone::Warning));
        let Node::Tabs { tabs, .. } = &children[2] else { panic!("tabs") };
        assert!(tabs[0].closable);
        assert!(matches!(children[3], Node::Input { mono: true, rows: 8, .. }));
    }

    #[test]
    fn a_table_counts_its_cells() {
        let rows: Vec<_> = (0..MAX_NODES / 2).map(|i| json!({ "id": i.to_string(), "cells": ["a", "b"] })).collect();
        let table = json!({ "type": "table", "id": "t", "columns": [{ "label": "A" }, { "label": "B" }], "rows": rows });
        assert!(Node::from_value(table).is_err());
    }

    #[test]
    fn enforces_limits() {
        assert!(Node::from_value(json!({ "type": "canvas" })).is_err());
        let flow = Node::from_value(json!({ "type": "flow", "id": "f", "steps": [
            { "id": "a", "title": "Collect" },
            { "id": "b", "title": "Post", "state": "active", "selected": true },
        ] }))
        .unwrap();
        let Node::Flow { steps, .. } = flow else { panic!("a flow") };
        assert_eq!((steps[0].state, steps[1].state, steps[1].selected), (FlowState::On, FlowState::Active, true));
        let tree = Node::from_value(json!({ "type": "column", "children": [
            { "type": "text", "text": "x" },
            { "type": "popover", "id": "p", "title": "Settings", "children": [{ "type": "input", "id": "i" }] },
        ] }))
        .unwrap();
        assert!(matches!(tree.popover(), Some(Node::Popover { id, .. }) if id == "p"));
        let mut fields = Vec::new();
        tree.inputs(&mut fields);
        assert_eq!(fields.len(), 1, "an input in the popover is synced like any other");
        let many: Vec<_> = (0..MAX_NODES + 1).map(|i| json!({ "id": i.to_string(), "title": "t" })).collect();
        assert!(Node::from_value(json!({ "type": "flow", "id": "f", "steps": many })).is_err());
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
