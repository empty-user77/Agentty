//! What the remote page shows, copied out of the app on its own thread so the web server never
//! touches the UI: the sessions with their state, and the screens of the terminals being watched.

use serde::Serialize;
use serde_json::{json, Value};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// One workspace as the app's sidebar shows it, in the sidebar's order, with its tabs and each
/// tab's split panes, so the page can be laid out the way the app is.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WorkspaceInfo {
    pub id: u64,
    pub name: String,
    /// The group heading it sits under (`None`: no headings, nothing is grouped).
    pub group: Option<String>,
    pub group_color: Option<String>,
    /// The card's colour, `#rrggbb`, when the user picked one.
    pub color: Option<String>,
    pub branch: Option<String>,
    /// Its folder, home shortened to `~`.
    pub folder: String,
    /// Closed to save memory until it is opened again: no terminals to show.
    pub sleeping: bool,
    pub active_tab: usize,
    pub tabs: Vec<TabInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TabInfo {
    /// As the tab bar titles it: its focused pane's title.
    pub title: String,
    /// The pane focused in it.
    pub active_pane: u64,
    /// Its split panes, in layout order.
    pub panes: Vec<u64>,
}

/// One terminal, as the session list shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionInfo {
    pub pane: u64,
    pub workspace: String,
    pub workspace_id: u64,
    /// Index of its tab in the workspace.
    pub tab: usize,
    pub title: String,
    /// Brand id: claude, codex, gemini, shell.
    pub tool: String,
    /// The status as the app words it ("Working", "Needs permission").
    pub status: String,
    /// The status color, `#rrggbb`.
    pub color: String,
    pub needs_user: bool,
    pub working: bool,
    /// Finished or asked something the user has not looked at yet.
    pub unread: bool,
    pub elapsed: Option<u64>,
    pub last_activity_ms: u64,
    /// What a permission request or a question asks, when the agent said.
    pub asks: Option<String>,
}

/// A stretch of cells in one style. `f` / `b` unset: the terminal's own foreground / background.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct Run {
    pub t: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub f: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub b: Option<u32>,
    /// Style bits: see `BOLD` and the rest.
    #[serde(skip_serializing_if = "is_zero")]
    pub s: u8,
}

pub const BOLD: u8 = 1;
pub const ITALIC: u8 = 2;
pub const UNDERLINE: u8 = 4;
pub const STRIKE: u8 = 8;
pub const DIM: u8 = 16;
/// Two cells wide (CJK): the page keeps the grid by giving it two cells.
pub const WIDE: u8 = 32;

fn is_zero(value: &u8) -> bool {
    *value == 0
}

#[derive(Debug, Clone, PartialEq)]
pub struct Screen {
    pub cols: u16,
    pub rows: u16,
    /// Column and row of the cursor, when it shows.
    pub cursor: Option<(u16, u16)>,
    pub fg: u32,
    pub bg: u32,
    pub lines: Vec<Vec<Run>>,
}

fn line_hash(line: &[Run]) -> u64 {
    let mut hasher = DefaultHasher::new();
    line.hash(&mut hasher);
    hasher.finish()
}

/// What changed on a screen since a viewer last got it. `sent` holds that viewer's row hashes and
/// is brought up to date; the first call (or a new size) sends every row.
pub fn screen_update(pane: u64, screen: &Screen, sent: &mut Vec<u64>) -> Option<Value> {
    let hashes: Vec<u64> = screen.lines.iter().map(|l| line_hash(l)).collect();
    let full = sent.len() != hashes.len();
    let changed: Vec<Value> =
        hashes.iter().enumerate().filter(|(i, h)| full || sent.get(*i) != Some(h)).map(|(i, _)| json!([i, screen.lines[i]])).collect();
    *sent = hashes;
    Some(json!({
        "pane": pane,
        "full": full,
        "cols": screen.cols,
        "rows": screen.rows,
        "cursor": screen.cursor,
        "fg": screen.fg,
        "bg": screen.bg,
        "lines": changed,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(rows: &[&str]) -> Screen {
        Screen {
            cols: 10,
            rows: rows.len() as u16,
            cursor: Some((0, 0)),
            fg: 0xffffff,
            bg: 0,
            lines: rows.iter().map(|t| vec![Run { t: t.to_string(), f: None, b: None, s: 0 }]).collect(),
        }
    }

    #[test]
    fn sends_everything_first_then_only_what_changed() {
        let mut sent = Vec::new();
        let first = screen_update(3, &screen(&["a", "b", "c"]), &mut sent).unwrap();
        assert_eq!(first["full"], true);
        assert_eq!(first["lines"].as_array().unwrap().len(), 3);
        let second = screen_update(3, &screen(&["a", "B", "c"]), &mut sent).unwrap();
        assert_eq!(second["full"], false);
        assert_eq!(second["lines"], json!([[1, [{ "t": "B" }]]]));
        let resized = screen_update(3, &screen(&["a", "B"]), &mut sent).unwrap();
        assert_eq!(resized["full"], true, "a new size sends every row");
    }

    #[test]
    fn runs_leave_defaults_out() {
        let run = Run { t: "x".into(), f: Some(0xff0000), b: None, s: BOLD | UNDERLINE };
        assert_eq!(serde_json::to_value(&run).unwrap(), json!({ "t": "x", "f": 0xff0000, "s": 5 }));
    }
}
