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
    /// Local servers its terminals run (`npm run dev`, …), as the sidebar's port chips.
    pub ports: Vec<PortInfo>,
}

/// A local server a workspace's terminal runs, and where the page can open it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PortInfo {
    pub port: u16,
    /// Its address on the tailnet once it is shown there (asked for from the page).
    pub url: Option<String>,
    /// Why it could not be shown, when it could not.
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TabInfo {
    /// As the tab bar titles it: its focused pane's title.
    pub title: String,
    /// The pane focused in it.
    pub active_pane: u64,
    /// Its split panes, in layout order.
    pub panes: Vec<u64>,
    /// A chat tab: its first pane is the lead, shown as this chat; the others are its workers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat: Option<ChatInfo>,
}

/// A chat tab, as the page shows its lead: the conversation and what the team runs on.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChatInfo {
    /// The lead's pane: messages go to it, and its terminal is one switch away.
    pub lead: u64,
    /// The chat's own branch, when the lead works on one.
    pub branch: Option<String>,
    /// "Workers: Claude", "Reviewers: off", in the app's language.
    pub roles: Vec<String>,
    /// Plan usage of the agents the chat runs on ("Claude 5h 62%").
    pub usage: Vec<UsageInfo>,
    /// Said above the conversation while a plan limit is nearly used up.
    pub warning: Option<String>,
    /// The newest entries of the conversation, oldest first.
    pub entries: Vec<ChatEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UsageInfo {
    pub label: String,
    /// 0 calm, 1 getting full, 2 nearly used up.
    pub level: u8,
    /// "resets in 2h 10m".
    pub resets: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChatEntry {
    /// `user`, `lead` or `report`.
    pub role: &'static str,
    pub text: String,
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
    /// Where the terminal works now: a path its output names is read from here. Never sent to the
    /// page.
    #[serde(skip)]
    pub dir: std::path::PathBuf,
    /// The folders the page may open files from for this terminal (`files`): its working tree
    /// and its workspace's. Never sent to the page.
    #[serde(skip)]
    pub roots: Vec<std::path::PathBuf>,
}

/// An installed plugin and how it runs, for the page's home. Only shown: the page can't start,
/// stop or reach a plugin.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PluginInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    /// `running`, `starting`, `stopped`, `failed`, `consent` (waits for the user to allow what it
    /// asks for), or `off` (turned off in the Plugins page).
    pub state: &'static str,
    /// Why it failed, cut short.
    pub error: Option<String>,
    pub automations: Vec<AutomationInfo>,
}

/// One automation of a plugin: a tab of its workspace.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AutomationInfo {
    pub title: String,
    /// `working`, `idle`, `error`, or `""` when it has said nothing.
    pub state: &'static str,
    pub text: Option<String>,
    /// Seconds it has been working.
    pub elapsed: Option<u64>,
    /// Its workspace is asleep: the automation does not run.
    pub sleeping: bool,
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

/// Lines of scrollback sent above the screen, the newest ones: enough to read back through an
/// agent's last answers on a phone, little enough to copy on every round.
pub const REMOTE_HISTORY: usize = 1000;

#[derive(Debug, Clone, PartialEq)]
pub struct Screen {
    pub cols: u16,
    pub rows: u16,
    /// How many of `lines` are scrollback, above the screen's `rows`.
    pub history: u16,
    /// The program scrolls itself (Claude Code's full-screen UI, vim, less): it has no scrollback
    /// here, and a swipe on the page goes to it as the mouse wheel.
    pub program_scrolls: bool,
    /// Column and row of the cursor on the screen (not counting `history`), when it shows.
    pub cursor: Option<(u16, u16)>,
    pub fg: u32,
    pub bg: u32,
    /// The scrollback lines, oldest first, then the screen's.
    pub lines: Vec<Vec<Run>>,
}

/// What one viewer was last sent of a screen.
#[derive(Debug, Default)]
pub struct Sent {
    cols: u16,
    hashes: Vec<u64>,
}

fn line_hash(line: &[Run]) -> u64 {
    let mut hasher = DefaultHasher::new();
    line.hash(&mut hasher);
    hasher.finish()
}

/// How many lines left the top since `old`: output scrolls everything up, so without this every
/// line would look changed and all of the scrollback would go again. The shift that leaves the
/// fewest lines to send wins; 0 when nothing scrolled.
fn scrolled_by(old: &[u64], new: &[u64]) -> usize {
    // Lines to send if `shift` lines left: those that differ, and those past the old end.
    let cost = |shift: usize, limit: usize| {
        let overlap = old.len() - shift;
        let mut cost = new.len().saturating_sub(overlap);
        for (i, hash) in new.iter().enumerate().take(overlap) {
            if old[i + shift] != *hash {
                cost += 1;
                if cost >= limit {
                    break;
                }
            }
        }
        cost
    };
    let mut best = (0, cost(0, usize::MAX));
    // Only shifts that line up the first few lines are worth counting.
    const ANCHOR: usize = 3;
    if new.len() < ANCHOR {
        return 0;
    }
    let candidates = (1..old.len().saturating_sub(ANCHOR - 1)).filter(|&k| old[k..k + ANCHOR] == new[..ANCHOR]);
    for shift in candidates.take(16) {
        let cost = cost(shift, best.1);
        if cost < best.1 {
            best = (shift, cost);
        }
    }
    best.0
}

/// What changed on a screen since a viewer last got it. `sent` is what that viewer has and is
/// brought up to date; the first call (or a new width) sends every line. Otherwise `shift` lines
/// leave the top of what the viewer has, what remains is made `len` lines long, and `lines` are
/// the ones that differ, by their new index.
pub fn screen_update(pane: u64, screen: &Screen, sent: &mut Sent) -> Option<Value> {
    let hashes: Vec<u64> = screen.lines.iter().map(|l| line_hash(l)).collect();
    let full = sent.hashes.is_empty() || sent.cols != screen.cols;
    let shift = if full { 0 } else { scrolled_by(&sent.hashes, &hashes) };
    let changed: Vec<Value> = hashes
        .iter()
        .enumerate()
        .filter(|(i, h)| full || sent.hashes.get(i + shift) != Some(h))
        .map(|(i, _)| json!([i, screen.lines[i]]))
        .collect();
    *sent = Sent { cols: screen.cols, hashes };
    Some(json!({
        "pane": pane,
        "full": full,
        "shift": shift,
        "len": screen.lines.len(),
        "cols": screen.cols,
        "rows": screen.rows,
        "history": screen.history,
        "programScrolls": screen.program_scrolls,
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
            history: 0,
            program_scrolls: false,
            cursor: Some((0, 0)),
            fg: 0xffffff,
            bg: 0,
            lines: rows.iter().map(|t| vec![Run { t: t.to_string(), f: None, b: None, s: 0 }]).collect(),
        }
    }

    #[test]
    fn sends_everything_first_then_only_what_changed() {
        let mut sent = Sent::default();
        let first = screen_update(3, &screen(&["a", "b", "c"]), &mut sent).unwrap();
        assert_eq!(first["full"], true);
        assert_eq!(first["lines"].as_array().unwrap().len(), 3);
        let second = screen_update(3, &screen(&["a", "B", "c"]), &mut sent).unwrap();
        assert_eq!(second["full"], false);
        assert_eq!(second["shift"], 0);
        assert_eq!(second["lines"], json!([[1, [{ "t": "B" }]]]));
        let mut wider = screen(&["a", "B", "c"]);
        wider.cols = 20;
        let resized = screen_update(3, &wider, &mut sent).unwrap();
        assert_eq!(resized["full"], true, "a new width sends every line");
    }

    #[test]
    fn scrollback_growing_sends_only_the_new_lines() {
        let mut sent = Sent::default();
        screen_update(3, &screen(&["a", "b", "c", "d"]), &mut sent).unwrap();
        // A line went into the scrollback and a new one came in at the bottom.
        let grown = screen_update(3, &screen(&["a", "b", "c", "d", "e"]), &mut sent).unwrap();
        assert_eq!(grown["full"], false);
        assert_eq!((grown["shift"].clone(), grown["len"].clone()), (json!(0), json!(5)));
        assert_eq!(grown["lines"], json!([[4, [{ "t": "e" }]]]));
    }

    #[test]
    fn scrolling_past_the_kept_history_shifts_instead_of_resending() {
        let mut sent = Sent::default();
        screen_update(3, &screen(&["a", "b", "c", "d", "e", "f"]), &mut sent).unwrap();
        let scrolled = screen_update(3, &screen(&["c", "d", "e", "f", "g", "h"]), &mut sent).unwrap();
        assert_eq!(scrolled["full"], false);
        assert_eq!(scrolled["shift"], 2);
        assert_eq!(scrolled["lines"], json!([[4, [{ "t": "g" }]], [5, [{ "t": "h" }]]]));
    }

    #[test]
    fn a_cleared_screen_is_not_taken_for_a_scroll() {
        let mut sent = Sent::default();
        screen_update(3, &screen(&["a", "b", "c", "d"]), &mut sent).unwrap();
        let cleared = screen_update(3, &screen(&["x", "", "", ""]), &mut sent).unwrap();
        assert_eq!(cleared["shift"], 0);
        assert_eq!(cleared["lines"].as_array().unwrap().len(), 4);
    }

    #[test]
    fn runs_leave_defaults_out() {
        let run = Run { t: "x".into(), f: Some(0xff0000), b: None, s: BOLD | UNDERLINE };
        assert_eq!(serde_json::to_value(&run).unwrap(), json!({ "t": "x", "f": 0xff0000, "s": 5 }));
    }
}
