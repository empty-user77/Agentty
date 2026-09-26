//! The database page: the connections of the active pane's project (found in its configuration or
//! added by hand), their tables, the rows of one, and a query box. It only appears — as a chip in
//! the status bar — for projects that have a database configured.
//!
//! Agents in panes reach the same connections through `agentty db` (see `db_cli.rs`): reads run at
//! once, inside a read-only transaction; anything that writes or changes the schema waits here for
//! the user, in a dialog that shows the exact statement, the connection and who asked. Statements
//! typed into the query box go through the same dialog. Passwords never appear anywhere: not in the
//! page, not in answers to agents, not in logs.

use super::Workbench;
use crate::agent_signal::{browser_reply, DbRequest};
use crate::i18n::{t, tf};
use crate::text_input::TextInput;
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{action_button, icon, tilde, Tooltip, TypeScale};
use agentty_db::engine::{QueryResult, Session, TableInfo};
use agentty_db::guard::{classify_mongo, classify_sql, MongoOp, Verdict};
use agentty_db::model::{ConnectionConfig, Engine, Missing};
use agentty_db::store::{self, Connection, Source};
use gpui::{div, prelude::*, px, AnyElement, AppContext, ClickEvent, Context, Entity, Focusable, SharedString, Window};
use serde_json::Value;
use std::collections::VecDeque;
use std::path::PathBuf;

/// Rows shown on the page; agents may ask for up to [`AGENT_MAX_ROWS`].
const PAGE_ROWS: usize = 200;
const AGENT_MAX_ROWS: usize = 500;
/// Result grid: columns are as wide as what they hold, between these bounds.
const COLUMN_MIN_WIDTH: f32 = 90.;
const COLUMN_MAX_WIDTH: f32 = 420.;
/// Dragging a column goes further than measuring one: a long text column is read by widening it.
const COLUMN_DRAG_MAX_WIDTH: f32 = 900.;
/// Width per character at the grid's text size, for sizing a column to its content.
const COLUMN_CHAR_WIDTH: f32 = 7.1;
/// The row-number gutter, like a spreadsheet's.
const ROW_NUMBER_WIDTH: f32 = 44.;
/// Height of the query editor: default, and what dragging its handle allows.
const QUERY_HEIGHT: f32 = 230.;
const QUERY_MIN_HEIGHT: f32 = 60.;
const QUERY_MAX_HEIGHT: f32 = 520.;

/// Dragging the bar under the query editor. It draws nothing: the bar itself is the feedback.
#[derive(Clone)]
pub struct DbQueryDrag;

/// Dragging the edge of the connections column, or of the tables column beside it. Which one is
/// in the handler's own closure; the payload only has to exist for GPUI to track the drag.
#[derive(Clone, Copy)]
pub struct DbColumnDrag;

impl gpui::Render for DbColumnDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// Dragging the right edge of a result column. Which column is in the handler's own closure; the
/// payload only has to exist for GPUI to track the drag.
#[derive(Clone, Copy)]
pub struct DbGridDrag;

impl gpui::Render for DbGridDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// Widths of those two columns: a default, and what dragging allows.
const CONNECTIONS_WIDTH: f32 = 270.;
const TABLES_WIDTH: f32 = 230.;
const COLUMN_MIN: f32 = 150.;
const COLUMN_MAX: f32 = 560.;

impl gpui::Render for DbQueryDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}
/// How long an agent waits for an approval (matches the socket's wait in `agent_signal.rs`).
/// Approvals waiting at most; more are refused (an agent can't bury the user in dialogs).
const MAX_APPROVALS: usize = 20;
/// A newly shown approval ignores clicks this long (the second click of a double-click).
const APPROVAL_SETTLE: std::time::Duration = std::time::Duration::from_millis(700);
const APPROVAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// Something to run against a connection.
#[derive(Debug, Clone)]
pub enum Job {
    Sql(String),
    Mongo { op: MongoOp, collection: String, args: Value },
}

impl Job {
    fn verdict(&self, engine: Engine) -> Verdict {
        match self {
            Job::Sql(sql) => classify_sql(engine, sql),
            Job::Mongo { op, args, .. } => classify_mongo(*op, Some(args)),
        }
    }

    /// The statement as the approval dialog shows it — the same text that runs, because a
    /// statement only becomes a job after [`checked_statement`] has accepted it.
    fn describe(&self) -> String {
        match self {
            Job::Sql(sql) => sql.trim().to_string(),
            Job::Mongo { op, collection, args } => {
                format!("db.{collection}.{}({})", op.name(), serde_json::to_string_pretty(args).unwrap_or_default())
            }
        }
    }
}

/// The longest statement accepted at all: the dialog has to stay readable, and what the user can't
/// read they can't approve.
const MAX_STATEMENT: usize = 8_000;

/// Why a statement can't be shown as it would run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unshowable {
    Empty,
    TooLong,
    /// A character that would be rendered differently from how the server reads it.
    Hidden(char),
}

impl Unshowable {
    /// For an agent, which is told in English (like every other answer on the socket).
    fn english(self) -> String {
        match self {
            Unshowable::Empty => "the statement is empty".into(),
            Unshowable::TooLong => format!("the statement is longer than {MAX_STATEMENT} characters"),
            Unshowable::Hidden(c) => {
                format!("the statement holds a character that would not be shown as it runs (U+{:04X})", c as u32)
            }
        }
    }

    fn message(self, cx: &gpui::App) -> String {
        match self {
            Unshowable::Empty => t(cx, "db.statement_empty").to_string(),
            Unshowable::TooLong => tf(cx, "db.statement_too_long", &[("n", &MAX_STATEMENT.to_string())]),
            Unshowable::Hidden(c) => tf(cx, "db.statement_hidden", &[("code", &format!("U+{:04X}", c as u32))]),
        }
    }
}

/// A statement the approval dialog can show truthfully, or why it can't. Characters that make the
/// rendered text differ from the text that runs (direction overrides, zero-width marks, other
/// control characters) are refused rather than stripped, so the dialog never shows something the
/// server would read differently.
fn checked_statement(text: &str) -> Result<String, Unshowable> {
    let text = text.trim();
    if text.is_empty() {
        return Err(Unshowable::Empty);
    }
    if text.chars().count() > MAX_STATEMENT {
        return Err(Unshowable::TooLong);
    }
    let hidden = |c: char| {
        matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}')
            || (c.is_control() && c != '\n' && c != '\t' && c != '\r')
    };
    match text.chars().find(|c| hidden(*c)) {
        Some(c) => Err(Unshowable::Hidden(c)),
        None => Ok(text.to_string()),
    }
}

/// Runs `job` on a fresh session: a read inside a read-only transaction, a write as approved.
fn run_job(config: &ConnectionConfig, job: &Job, write: bool, limit: usize) -> anyhow::Result<QueryResult> {
    // Checked again here, before connecting: nothing reaches the read path that isn't a read.
    anyhow::ensure!(write || job.verdict(config.engine) == Verdict::Read, "this statement changes data: it needs the user's approval");
    let mut session = Session::connect(config)?;
    match job {
        Job::Sql(sql) if write => session.approved(config.engine, sql, limit.max(1)),
        Job::Sql(sql) => session.read(sql, limit),
        Job::Mongo { op, collection, args } if write => session.mongo_write(*op, collection, args),
        Job::Mongo { op, collection, args } => session.mongo_read(*op, collection, args, limit),
    }
}

/// A write or schema change waiting for the user's "Execute".
pub(super) struct Approval {
    /// Who asked: an agent pane's title, or the user from the query box.
    asker: String,
    connection: Connection,
    job: Job,
    verdict: Verdict,
    /// The asking agent's reply channel (`None` for the page's own query box).
    reply: Option<std::sync::mpsc::Sender<String>>,
    asked: std::time::Instant,
    /// Ties the dialog's buttons to this approval: a click meant for one never answers the next.
    id: u64,
}

/// Adding a connection by hand.
struct ManualForm {
    engine: Engine,
    name: Entity<TextInput>,
    host: Entity<TextInput>,
    port: Entity<TextInput>,
    database: Entity<TextInput>,
    user: Entity<TextInput>,
    password: Entity<TextInput>,
    tls_off: bool,
}

#[derive(Default)]
pub(super) struct DbState {
    /// Active pane and its folder when last looked, to notice a tab switch or a `cd`.
    seen: Option<(gpui::EntityId, PathBuf)>,
    pub(super) root: Option<PathBuf>,
    /// The panel docked beside the terminals is open (the full page is a separate view).
    pub(super) panel_open: bool,
    pub(super) connections: Vec<Connection>,
    loading: bool,
    generation: u64,
    stale: bool,
    selected: Option<String>,
    tables: Vec<TableInfo>,
    table: Option<String>,
    result: Option<QueryResult>,
    message: Option<(String, bool)>,
    busy: bool,
    query: Option<Entity<TextInput>>,
    /// The SQL editor: multi-line, highlighted, and as tall as the user drags it.
    editor: Option<Entity<crate::editor::CodeEditor>>,
    editor_height: f32,
    /// Widths of the connections and tables columns (0 until the user drags them).
    connections_width: f32,
    tables_width: f32,
    /// Row the user clicked in the result grid, shown in full below it.
    picked_row: Option<usize>,
    /// Widths the user dragged a result column to, by position and name. A column not in here is
    /// as wide as what it holds. The name keeps the width across a re-run of the same query; the
    /// position keeps two same-named columns (a self-join's two `id`s) apart.
    grid_widths: std::collections::HashMap<(usize, String), f32>,
    /// What each column of the current result measures. Worked out once when the result arrives:
    /// measuring every cell of 200 rows again on every frame is work the grid does not need.
    grid_measured: Vec<f32>,
    /// Scroll position of the result grid, so it can carry scrollbars both ways.
    grid_scroll: gpui::ScrollHandle,
    password: Option<(String, Entity<TextInput>)>,
    form: Option<ManualForm>,
    pub(super) approvals: VecDeque<Approval>,
    next_approval: u64,
    /// When the approval now in front was first shown; clicks in the first moments are ignored.
    front_since: Option<std::time::Instant>,
}

impl DbState {
    /// Shows a query result, measuring its columns once here rather than on every frame.
    fn show_result(&mut self, result: QueryResult) {
        self.grid_measured = measure_columns(&result);
        self.picked_row = None;
        self.result = Some(result);
    }

    fn clear_result(&mut self) {
        self.grid_measured.clear();
        self.picked_row = None;
        self.result = None;
    }

    pub(super) fn debug_state(&self) -> Value {
        serde_json::json!({
            "root": self.root,
            "connections": self.connections.iter().map(|c| serde_json::json!({
                "id": c.id, "name": c.name, "summary": c.config.summary(), "missing": c.missing,
            })).collect::<Vec<_>>(),
            "selected": self.selected,
            "tables": self.tables.iter().map(|t| &t.name).collect::<Vec<_>>(),
            "table": self.table,
            "rows": self.result.as_ref().map(|r| r.rows.len()),
            "message": self.message,
            "approvals": self.approvals.len(),
        })
    }
}

/// How wide each column of a result wants to be, from what it holds. An id column should not take
/// the room a message column needs, so this is measured rather than shared out evenly.
fn measure_columns(result: &QueryResult) -> Vec<f32> {
    result
        .columns
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let longest = result.rows.iter().filter_map(|row| row.get(i)).map(cell_chars).max().unwrap_or(0);
            let chars = longest.max(name.chars().count()) as f32;
            (chars * COLUMN_CHAR_WIDTH + 20.).clamp(COLUMN_MIN_WIDTH, COLUMN_MAX_WIDTH)
        })
        .collect()
}

/// The length of a cell as text, without building that text: measuring a result allocated a string
/// per cell, and a result has as many cells as it has rows times columns.
fn cell_chars(value: &Value) -> usize {
    const LONGEST_THAT_MATTERS: usize = 120;
    match value {
        Value::Null => 4,
        Value::String(s) => s.chars().take(LONGEST_THAT_MATTERS).count(),
        other => other.to_string().chars().take(LONGEST_THAT_MATTERS).count(),
    }
}

fn cell_text(value: &Value) -> String {
    match value {
        Value::Null => "NULL".into(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn missing_label(missing: Missing) -> &'static str {
    match missing {
        Missing::Password => "db.needs_password",
        Missing::User => "db.needs_user",
        Missing::Host => "db.needs_host",
        Missing::Database => "db.needs_database",
    }
}

impl Workbench {
    // -- following the project -------------------------------------------------------------------

    /// Follows the active pane's project; called on render.
    pub(super) fn prepare_db(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.db.query.is_none() && self.page == Some(super::Page::Database) {
            self.db.query = Some(cx.new(|cx| TextInput::localized("", "db.query_placeholder", window, cx)));
        }
        if self.db.editor.is_none() && self.page == Some(super::Page::Database) {
            self.db.editor = Some(self.new_query_editor(window, cx));
        }
        let seen = self.active_pane().map(|pane| (pane.entity_id(), pane.read(cx).display_cwd()));
        if seen != self.db.seen {
            self.db.seen = seen;
            let root = self.active_tree(cx);
            if root != self.db.root {
                self.db.root = root;
                self.db.connections.clear();
                self.db.selected = None;
                self.db.tables.clear();
                self.db.table = None;
                self.db.clear_result();
                self.db.message = None;
                self.db.stale = true;
            }
        }
        if self.db.stale {
            self.refresh_db(cx);
        }
    }

    /// Reads the project's connections again (configuration files and saved ones).
    pub(super) fn refresh_db(&mut self, cx: &mut Context<Self>) {
        self.db.stale = false;
        let Some(root) = self.db.root.clone() else { return };
        if self.db.loading {
            self.db.stale = true;
            return;
        }
        self.db.loading = true;
        self.db.generation += 1;
        let generation = self.db.generation;
        cx.spawn(async move |this, cx| {
            let scanned = root.clone();
            let connections = cx.background_spawn(async move { store::connections(&scanned) }).await;
            let _ = this.update(cx, |this, cx| {
                if this.db.generation != generation {
                    return;
                }
                this.db.loading = false;
                if this.db.root.as_ref() != Some(&root) {
                    this.db.stale = true;
                    return cx.notify();
                }
                this.db.connections = connections;
                if this.db.selected.as_ref().is_some_and(|id| !this.db.connections.iter().any(|c| &c.id == id)) {
                    this.db.selected = None;
                    this.db.tables.clear();
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn selected_connection(&self) -> Option<&Connection> {
        let id = self.db.selected.as_ref()?;
        self.db.connections.iter().find(|c| &c.id == id)
    }

    // -- page actions ----------------------------------------------------------------------------

    pub(super) fn open_db_page(&mut self, cx: &mut Context<Self>) {
        if self.page != Some(super::Page::Database) {
            self.open_page(super::Page::Database, cx);
        }
        if self.db.selected.is_none() {
            if let Some(first) = self.db.connections.iter().find(|c| c.missing.is_empty()).map(|c| c.id.clone()) {
                self.select_db_connection(first, cx);
            }
        }
        self.refresh_db(cx);
    }

    fn select_db_connection(&mut self, id: String, cx: &mut Context<Self>) {
        self.db.selected = Some(id.clone());
        self.db.tables.clear();
        self.db.table = None;
        self.db.clear_result();
        self.db.message = None;
        let Some(connection) = self.selected_connection().cloned() else { return };
        // Only a password missing is not a reason to refuse: plenty of local databases have none,
        // and the connection itself gives a far better answer than a guess does.
        if connection.missing.iter().any(|m| *m != Missing::Password) {
            return cx.notify();
        }
        self.db.busy = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let config = connection.config.clone();
            let tables = cx.background_spawn(async move { Session::connect(&config).and_then(|mut s| s.tables()) }).await;
            let _ = this.update(cx, |this, cx| {
                if this.db.selected.as_deref() != Some(id.as_str()) {
                    return;
                }
                this.db.busy = false;
                match tables {
                    Ok(tables) => this.db.tables = tables,
                    Err(err) => this.db.message = Some((format!("{err:#}"), true)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn open_db_table(&mut self, table: String, cx: &mut Context<Self>) {
        let Some(connection) = self.selected_connection().cloned() else { return };
        self.db.table = Some(table.clone());
        self.db.busy = true;
        self.db.message = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let config = connection.config.clone();
            let name = table.clone();
            let result = cx.background_spawn(async move { Session::connect(&config).and_then(|mut s| s.preview(&name, PAGE_ROWS)) }).await;
            let _ = this.update(cx, |this, cx| {
                if this.db.table.as_deref() != Some(table.as_str()) {
                    return;
                }
                this.db.busy = false;
                match result {
                    Ok(result) => this.db.show_result(result),
                    Err(err) => this.db.message = Some((format!("{err:#}"), true)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The query box: a read runs at once; anything else goes to the approval dialog first.
    /// A code editor on a scratch `.sql` file: multi-line editing, highlighting and undo, without
    /// teaching the one-line input field about any of it.
    fn new_query_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<crate::editor::CodeEditor> {
        let dir = agentty_bridge::fsutil::data_dir().join("db");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("query.sql");
        if !path.is_file() {
            let _ = std::fs::write(&path, "");
        }
        // A query the user wrote is theirs alone: it can name their data, and a connection string
        // pasted into it would carry a password.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        let editor = cx.new(crate::editor::CodeEditor::new);
        editor.update(cx, |editor, cx| editor.open(&path, &dir, window, cx));
        editor
    }

    /// What the query box holds: the editor when it is up, else the plain field.
    fn db_query_text(&self, cx: &gpui::App) -> String {
        if let Some(text) = self.db.editor.as_ref().and_then(|e| e.read(cx).active_text()) {
            return text.trim().to_string();
        }
        self.db.query.as_ref().map(|q| q.read(cx).text().trim().to_string()).unwrap_or_default()
    }

    fn run_db_query(&mut self, cx: &mut Context<Self>) {
        let Some(connection) = self.selected_connection().cloned() else { return };
        let text = self.db_query_text(cx);
        let Some(text) = Some(text).filter(|t| !t.is_empty()) else { return };
        let text = match checked_statement(&text) {
            Ok(text) => text,
            Err(problem) => {
                self.db.message = Some((problem.message(cx), true));
                return cx.notify();
            }
        };
        let job = if connection.config.engine == Engine::MongoDb {
            // `find users {"age": 30}` style: operation, collection, JSON arguments.
            let mut parts = text.splitn(3, char::is_whitespace);
            let (op, collection, args) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""), parts.next().unwrap_or("{}"));
            let Some(op) = MongoOp::parse(op) else {
                self.db.message = Some((t(cx, "db.mongo_syntax").to_string(), true));
                return cx.notify();
            };
            let args: Value = serde_json::from_str(args).unwrap_or(Value::Null);
            Job::Mongo { op, collection: collection.to_string(), args }
        } else {
            Job::Sql(text)
        };
        let verdict = job.verdict(connection.config.engine);
        if verdict.needs_approval() {
            let asker = t(cx, "db.asker_you").to_string();
            self.queue_db_approval(asker, connection, job, verdict, None, cx);
            return;
        }
        self.run_db_job(connection, job, false, PAGE_ROWS, None, cx);
    }

    /// Runs `job` in the background; the result shows on the page, and goes to `reply` when an agent asked.
    fn run_db_job(
        &mut self,
        connection: Connection,
        job: Job,
        write: bool,
        limit: usize,
        reply: Option<std::sync::mpsc::Sender<String>>,
        cx: &mut Context<Self>,
    ) {
        let for_page = reply.is_none();
        if for_page {
            self.db.busy = true;
            self.db.message = None;
            cx.notify();
        }
        cx.spawn(async move |this, cx| {
            let config = connection.config.clone();
            let result = cx.background_spawn(async move { run_job(&config, &job, write, limit) }).await;
            if let Some(reply) = reply {
                let answer = match &result {
                    Ok(result) => browser_reply(Ok(serde_json::to_string(result).unwrap_or_default())),
                    Err(err) => browser_reply(Err(format!("{err:#}"))),
                };
                let _ = reply.send(answer);
            }
            let _ = this.update(cx, |this, cx| {
                if for_page {
                    this.db.busy = false;
                }
                match result {
                    Ok(result) if write && result.affected.is_none() && for_page => this.db.show_result(result),
                    Ok(result) if write => {
                        // Show the table again with the change (the page's own statements).
                        if let Some(table) = this.db.table.clone().filter(|_| for_page) {
                            this.open_db_table(table, cx);
                        }
                        this.db.message = Some((tf(cx, "db.affected", &[("n", &result.affected.unwrap_or(0).to_string())]), false));
                    }
                    Ok(result) if for_page => this.db.show_result(result),
                    Ok(_) => {}
                    Err(err) if for_page || write => this.db.message = Some((format!("{err:#}"), true)),
                    Err(_) => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn queue_db_approval(
        &mut self,
        asker: String,
        connection: Connection,
        job: Job,
        verdict: Verdict,
        reply: Option<std::sync::mpsc::Sender<String>>,
        cx: &mut Context<Self>,
    ) {
        if self.db.approvals.len() >= MAX_APPROVALS {
            if let Some(reply) = reply {
                let _ = reply.send(browser_reply(Err("too many statements are waiting for the user's approval".into())));
            }
            return;
        }
        if self.db.approvals.is_empty() {
            self.db.front_since = Some(std::time::Instant::now());
        }
        self.db.next_approval += 1;
        let id = self.db.next_approval;
        self.db.approvals.push_back(Approval { asker, connection, job, verdict, reply, asked: std::time::Instant::now(), id });
        cx.notify();
    }

    /// The user's answer to the approval `id` (the one in front; anything else is a stale click).
    fn answer_db_approval(&mut self, id: u64, execute: bool, cx: &mut Context<Self>) {
        if self.db.approvals.front().map(|a| a.id) != Some(id) {
            return;
        }
        if self.db.front_since.is_some_and(|since| since.elapsed() < APPROVAL_SETTLE) {
            return;
        }
        let Some(approval) = self.db.approvals.pop_front() else { return };
        self.db.front_since = (!self.db.approvals.is_empty()).then(std::time::Instant::now);
        if !self.db.approvals.is_empty() {
            // Paint the next one, then let the settle time count from there.
            cx.notify();
        }
        if !execute {
            if let Some(reply) = approval.reply {
                let _ = reply.send(browser_reply(Err("the user declined this statement".into())));
            }
            return cx.notify();
        }
        // The agent stopped waiting (the socket gives up after 15 minutes): it no longer expects this
        // to run, so it doesn't.
        if approval.reply.is_some() && approval.asked.elapsed() > APPROVAL_TIMEOUT {
            self.db.message = Some((t(cx, "db.expired").to_string(), true));
            return cx.notify();
        }
        let limit = if approval.reply.is_some() { AGENT_MAX_ROWS } else { PAGE_ROWS };
        self.run_db_job(approval.connection, approval.job, true, limit, approval.reply, cx);
        cx.notify();
    }

    fn ask_db_password(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| TextInput::localized("", "db.password", window, cx).masked());
        input.update(cx, |_, cx| cx.notify());
        window.focus(&input.read(cx).focus_handle(cx));
        self.db.password = Some((id, input));
        cx.notify();
    }

    fn save_db_password(&mut self, cx: &mut Context<Self>) {
        let Some((id, input)) = self.db.password.take() else { return };
        let password = input.read(cx).text().to_string();
        if password.is_empty() {
            return cx.notify();
        }
        match store::save_password(&id, &password) {
            Ok(()) => {
                self.db.selected = Some(id);
                self.db.stale = true;
            }
            Err(err) => self.db.message = Some((format!("{err:#}"), true)),
        }
        cx.notify();
    }

    fn open_db_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = |text: &str, key: &'static str, cx: &mut Context<Self>, window: &mut Window| {
            let text = text.to_string();
            cx.new(|cx| TextInput::localized(text, key, window, cx))
        };
        let form = ManualForm {
            engine: Engine::MySql,
            name: input("", "db.form.name", cx, window),
            host: input("", "db.form.host", cx, window),
            port: input("3306", "db.form.port", cx, window),
            database: input("", "db.form.database", cx, window),
            user: input("", "db.form.user", cx, window),
            password: cx.new(|cx| TextInput::localized("", "db.form.password", window, cx).masked()),
            tls_off: false,
        };
        window.focus(&form.name.read(cx).focus_handle(cx));
        self.db.form = Some(form);
        cx.notify();
    }

    fn save_db_form(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.db.root.clone() else { return };
        let Some(form) = self.db.form.as_ref() else { return };
        let text = |input: &Entity<TextInput>| input.read(cx).text().trim().to_string();
        let host = text(&form.host);
        if host.is_empty() {
            self.db.message = Some((t(cx, "db.needs_host").to_string(), true));
            return cx.notify();
        }
        let config = ConnectionConfig {
            engine: form.engine,
            host: host.clone(),
            port: text(&form.port).parse().unwrap_or(form.engine.default_port()),
            database: Some(text(&form.database)).filter(|d| !d.is_empty()),
            user: Some(text(&form.user)).filter(|u| !u.is_empty()),
            password: None,
            srv: false,
            options: Vec::new(),
        };
        let name = Some(text(&form.name)).filter(|n| !n.is_empty()).or(config.database.clone()).unwrap_or(host);
        let password = form.password.read(cx).text().to_string();
        match store::add_manual(&root, &name, &config, Some(&password), form.tls_off) {
            Ok(id) => {
                self.db.form = None;
                self.db.selected = Some(id);
                self.db.stale = true;
            }
            Err(err) => self.db.message = Some((format!("{err:#}"), true)),
        }
        cx.notify();
    }

    /// Debug driver: `db` (open), `db refresh`, `db select <n>`, `db table <name>`, `db query <text>`,
    /// `db execute` / `db decline` (the first approval), `db password <n> <text>`.
    pub(super) fn debug_db(&mut self, argument: &str, _window: &mut Window, cx: &mut Context<Self>) {
        let (verb, rest) = argument.split_once(' ').unwrap_or((argument, ""));
        match verb {
            "" => self.open_db_page(cx),
            "refresh" => self.refresh_db(cx),
            "select" => {
                if let Some(id) = store::find(&self.db.connections, Some(rest)).map(|c| c.id.clone()) {
                    self.select_db_connection(id, cx);
                }
            }
            "table" => self.open_db_table(rest.to_string(), cx),
            "query" => {
                if let Some(editor) = self.db.editor.clone() {
                    editor.update(cx, |editor, cx| editor.set_active_text(rest, cx));
                }
                if let Some(query) = self.db.query.clone() {
                    query.update(cx, |input, cx| input.set_text(rest.to_string(), cx));
                }
                self.run_db_query(cx);
            }
            // Never answers for an agent: the debug driver runs with the app's environment, which a
            // pane's agent inherits, so it must not be a way to approve one's own statement.
            "execute" | "decline" => {
                if let Some(approval) = self.db.approvals.front().filter(|a| a.reply.is_none()) {
                    let id = approval.id;
                    self.db.front_since = None;
                    self.answer_db_approval(id, verb == "execute", cx);
                }
            }
            "password" => {
                let (n, password) = rest.split_once(' ').unwrap_or((rest, ""));
                if let Some(id) = store::find(&self.db.connections, Some(n)).map(|c| c.id.clone()) {
                    match store::save_password(&id, password) {
                        Ok(()) => self.db.stale = true,
                        Err(err) => self.db.message = Some((format!("{err:#}"), true)),
                    }
                }
            }
            _ => {}
        }
        cx.notify();
    }

    // -- agents ----------------------------------------------------------------------------------

    /// `agentty db …` from an agent pane. Listing never shows passwords; reads run at once; writes and
    /// schema changes wait for the user in the approval dialog.
    pub fn answer_db_request(&mut self, request: DbRequest, cx: &mut Context<Self>) {
        let pane = self.all_panes().into_iter().find(|p| p.read(cx).pane_id == request.pane);
        let asker = pane.as_ref().map(|p| p.read(cx).display_title()).unwrap_or_default();
        // The folder the pane was opened in decides which databases it reaches — not the folder
        // named in the request, which is wherever the shell has since been told to go. Otherwise an
        // agent could `cd` into any other project and read it with the password the user once
        // entered there.
        let Some(home) = pane.map(|p| p.read(cx).spec.cwd.clone()) else {
            let _ = request.reply.send(browser_reply(Err("this pane is gone".into())));
            return;
        };
        let home_root = agentty_bridge::worktree::tree_root(&home).unwrap_or(home);
        let asked = agentty_bridge::worktree::tree_root(&request.cwd).unwrap_or_else(|| request.cwd.clone());
        if asked != home_root && !asked.starts_with(&home_root) {
            let _ = request.reply.send(browser_reply(Err(format!(
                "this pane reaches only the databases of {}: run `agentty db` from that project",
                tilde(&home_root)
            ))));
            return;
        }
        let project = home_root;
        let args = request.args.clone();
        let reply = request.reply.clone();
        cx.spawn(async move |this, cx| {
            let scanned = project.clone();
            let connections = cx.background_spawn(async move { store::connections(&scanned) }).await;
            let _ = this.update(cx, |this, cx| {
                let fail = |text: String| {
                    let _ = reply.send(browser_reply(Err(text)));
                };
                let action = args["action"].as_str().unwrap_or("");
                if action == "list" {
                    let list: Vec<Value> = connections
                        .iter()
                        .enumerate()
                        .map(|(i, c)| {
                            serde_json::json!({
                                "n": i + 1,
                                "name": c.name,
                                "engine": c.config.engine.label(),
                                "where": c.config.summary(),
                                "source": match &c.source { Source::Detected(file) => file.clone(), Source::Manual => "added in Agentty".into() },
                                "ready": c.missing.is_empty(),
                            })
                        })
                        .collect();
                    let _ = reply.send(browser_reply(Ok(Value::Array(list).to_string())));
                    return;
                }
                let Some(connection) = store::find(&connections, args["conn"].as_str()).cloned() else {
                    return fail(if connections.is_empty() {
                        format!("no database is configured for {}", tilde(&project))
                    } else {
                        "several connections: pick one with --conn <name or number> (see `agentty db list`)".into()
                    });
                };
                if !connection.missing.is_empty() {
                    return fail(format!(
                        "{} needs a password first: ask the user to open Agentty's database page and enter it",
                        connection.name
                    ));
                }
                let limit = args["limit"].as_u64().map(|n| n as usize).unwrap_or(100).clamp(1, AGENT_MAX_ROWS);
                let table = args["table"].as_str().unwrap_or("").to_string();
                let job = match action {
                    "tables" | "describe" | "preview" => {
                        let config = connection.config.clone();
                        let action = action.to_string();
                        cx.spawn(async move |_, cx| {
                            let result = cx
                                .background_spawn(async move {
                                    let mut session = Session::connect(&config)?;
                                    Ok::<Value, anyhow::Error>(match action.as_str() {
                                        "tables" => serde_json::to_value(session.tables()?)?,
                                        "describe" => serde_json::to_value(session.columns(&table)?)?,
                                        _ => serde_json::to_value(session.preview(&table, limit)?)?,
                                    })
                                })
                                .await;
                            let _ = reply.send(match result {
                                Ok(value) => browser_reply(Ok(value.to_string())),
                                Err(err) => browser_reply(Err(format!("{err:#}"))),
                            });
                        })
                        .detach();
                        return;
                    }
                    "query" if connection.config.engine != Engine::MongoDb => match checked_statement(args["sql"].as_str().unwrap_or("")) {
                        Ok(sql) => Job::Sql(sql),
                        Err(problem) => return fail(problem.english()),
                    },
                    "mongo" if connection.config.engine == Engine::MongoDb => {
                        let Some(op) = args["op"].as_str().and_then(MongoOp::parse) else { return fail("unknown MongoDB operation".into()) };
                        Job::Mongo { op, collection: args["collection"].as_str().unwrap_or("").to_string(), args: args["args"].clone() }
                    }
                    "query" => return fail("this is a MongoDB connection: use `agentty db mongo …`".into()),
                    "mongo" => return fail("this is a SQL connection: use `agentty db query …`".into()),
                    _ => return fail(format!("unknown action {action}")),
                };
                let verdict = job.verdict(connection.config.engine);
                if verdict.needs_approval() {
                    this.queue_db_approval(asker, connection, job, verdict, Some(reply), cx);
                    return;
                }
                this.run_db_job(connection, job, false, limit, Some(reply), cx);
            });
        })
        .detach();
    }

    // -- status bar ------------------------------------------------------------------------------

    /// "DB 2" in the status bar, for a project with databases configured. Opens the page.
    pub(super) fn render_db_chip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.db.connections.is_empty() {
            return None;
        }
        let waiting = self.db.connections.iter().any(|c| !c.missing.is_empty());
        let open = self.page == Some(super::Page::Database);
        Some(
            div()
                .id("status-db")
                .tooltip(Tooltip::text(t(cx, "db.tooltip"), None))
                .h_full()
                .px_1p5()
                .flex()
                .items_center()
                .gap_1()
                .cursor_pointer()
                .when(open, |d| d.bg(hex(Chrome::SELECTED)))
                .hover(|s| s.bg(hex(Chrome::HOVER)))
                .child(icon("database", 13., hex(if waiting { Chrome::WARNING } else { Chrome::BLUE })))
                .child(div().text_color(hex(Chrome::FOREGROUND)).child(tf(cx, "db.chip", &[("n", &self.db.connections.len().to_string())])))
                // Opens beside the terminals, like Docker; the panel's ⤢ button gives the full page.
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.toggle_db_panel(cx)))
                .into_any_element(),
        )
    }

    /// Opens or closes the database panel docked beside the terminals (the Docker panel's place).
    pub(super) fn toggle_db_panel(&mut self, cx: &mut Context<Self>) {
        self.db.panel_open = !self.db.panel_open;
        if self.db.panel_open {
            self.db.stale = true;
            self.refresh_db(cx);
        }
        cx.notify();
    }

    /// The database beside the terminals: connections, their tables, and a way to the full page.
    /// Everything that needs room — the query editor and the result grid — lives on that page.
    pub(super) fn render_db_panel(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.db.panel_open {
            return None;
        }
        let width = self.db_panel_width(cx);
        let db = &self.db;
        let header = div()
            .h(px(36.))
            .flex_shrink_0()
            .px_2()
            .flex()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(hex(Chrome::BORDER))
            .bg(hex(Chrome::SIDE_BAR))
            .child(icon("database", crate::ui::IconSize::BUTTON, hex(Chrome::BLUE)))
            .child(div().flex_1().t_body().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(t(cx, "db.title")))
            .child(
                crate::ui::icon_only("db-panel-expand", "maximize-2", cx.listener(|this, _: &ClickEvent, _, cx| this.open_db_page(cx)))
                    .tooltip(Tooltip::text(t(cx, "db.expand"), None)),
            )
            .child(
                crate::ui::icon_only("db-panel-refresh", "refresh-cw", cx.listener(|this, _: &ClickEvent, _, cx| this.refresh_db(cx)))
                    .tooltip(Tooltip::text(t(cx, "usage.refresh"), None)),
            )
            .child(crate::ui::icon_only("db-panel-close", "x", cx.listener(|this, _: &ClickEvent, _, cx| this.toggle_db_panel(cx))));

        let mut body = div().id("db-panel-body").flex_1().min_h_0().overflow_y_scroll().flex().flex_col();
        if db.connections.is_empty() {
            body = body.child(div().px_3().py_2().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "db.no_connections")));
        }
        for connection in &db.connections {
            let id = connection.id.clone();
            let selected = db.selected.as_deref() == Some(connection.id.as_str());
            body = body.child(
                div()
                    .id(SharedString::from(format!("db-panel-conn-{}", connection.id)))
                    .mx_1()
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .when(selected, |d| d.bg(hex(Chrome::SELECTED)))
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.select_db_connection(id.clone(), cx)))
                    .child(icon("database", 12., hex(if connection.missing.is_empty() { Chrome::BLUE } else { Chrome::WARNING })))
                    .child(div().flex_1().min_w_0().truncate().t_small().text_color(hex(Chrome::BRIGHT)).child(connection.name.clone())),
            );
            if !selected {
                continue;
            }
            // Why a click seems to do nothing otherwise: the connection needs a password, is still
            // being opened, or failed. The page said so; the panel has to say so too.
            for missing in &connection.missing {
                body = body.child(div().px_3().pb_1().t_caption().text_color(hex(Chrome::WARNING)).child(t(cx, missing_label(*missing))));
            }
            if connection.missing.contains(&Missing::Password) {
                let id = connection.id.clone();
                body = body.child(div().px_3().pb_2().child(action_button(
                    SharedString::from(format!("db-panel-password-{}", connection.id)),
                    t(cx, "db.enter_password"),
                    cx.listener(move |this, _: &ClickEvent, window, cx| {
                        cx.stop_propagation();
                        this.ask_db_password(id.clone(), window, cx)
                    }),
                )));
            }
            if db.busy {
                body = body.child(crate::ui::loading_row(t(cx, "db.loading")));
            }
            if let Some((text, error)) = &db.message {
                body = body.child(
                    div()
                        .px_3()
                        .pb_2()
                        .t_caption()
                        .text_color(hex(if *error { Chrome::ERROR } else { Chrome::SUCCESS }))
                        .child(text.clone()),
                );
            }
            if !db.busy && db.tables.is_empty() && db.message.is_none() && connection.missing.is_empty() {
                body = body.child(div().px_3().pb_2().t_caption().text_color(hex(Chrome::MUTED)).child(t(cx, "db.no_tables")));
            }
            for table in &db.tables {
                let name = table.name.clone();
                let open = db.table.as_deref() == Some(table.name.as_str());
                body = body.child(
                    div()
                        .id(SharedString::from(format!("db-panel-table-{}", table.name)))
                        .mx_1()
                        .pl_6()
                        .pr_2()
                        .py_0p5()
                        .rounded_md()
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .when(open, |d| d.bg(hex(Chrome::SELECTED)))
                        .hover(|s| s.bg(hex(Chrome::HOVER)))
                        // Opening a table is what the full page is for: it shows the rows.
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.open_db_table(name.clone(), cx);
                            this.open_db_page(cx);
                        }))
                        .child(icon(if table.kind == "view" { "eye" } else { "rows-2" }, 11., hex(Chrome::MUTED)))
                        .child(div().min_w_0().truncate().t_small().text_color(hex(Chrome::FOREGROUND)).child(table.name.clone())),
                );
            }
        }
        Some(
            div()
                .w(px(width))
                .flex_shrink_0()
                .h_full()
                .flex()
                .flex_col()
                .bg(hex(Chrome::PANEL))
                .border_l_1()
                .border_color(hex(Chrome::BORDER))
                .child(header)
                .child(body)
                .into_any_element(),
        )
    }

    // -- page ------------------------------------------------------------------------------------

    pub(super) fn render_db_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let db = &self.db;
        let heading = |text: String| {
            div()
                .px_3()
                .pt_3()
                .pb_1()
                .t_caption()
                .font_weight(crate::theme::EMPHASIS)
                .text_color(hex(Chrome::MUTED))
                .child(text.to_uppercase())
        };

        // Connections.
        let connections_width = if db.connections_width > 0. { db.connections_width } else { CONNECTIONS_WIDTH };
        let tables_width = if db.tables_width > 0. { db.tables_width } else { TABLES_WIDTH };
        let mut connections = div()
            .id("db-connections")
            .w(px(connections_width))
            .flex_shrink_0()
            .h_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(hex(Chrome::BORDER));
        connections = connections.child(
            div()
                .flex()
                .items_center()
                .pr_2()
                .child(div().flex_1().min_w_0().truncate().child(heading(t(cx, "db.connections").to_string())))
                .child(action_button(
                    "db-add",
                    t(cx, "db.add"),
                    cx.listener(|this, _: &ClickEvent, window, cx| this.open_db_form(window, cx)),
                )),
        );
        if let Some(root) = &db.root {
            connections = connections.child(div().px_3().pb_2().t_caption().text_color(hex(Chrome::MUTED)).truncate().child(tilde(root)));
        }
        if db.connections.is_empty() {
            connections = connections.child(div().px_3().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "db.no_connections")));
        }
        for connection in &db.connections {
            let id = connection.id.clone();
            let selected = db.selected.as_deref() == Some(connection.id.as_str());
            let source = match &connection.source {
                Source::Detected(file) => tf(cx, "db.from", &[("file", file)]),
                Source::Manual => t(cx, "db.manual").to_string(),
            };
            let mut row =
                div()
                    .id(SharedString::from(format!("db-conn-{}", connection.id)))
                    .mx_1()
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .cursor_pointer()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .when(selected, |d| d.bg(hex(Chrome::SELECTED)))
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .child(
                        div().flex().items_center().gap_1p5().child(icon("database", 13., hex(Chrome::BLUE))).child(
                            div().flex_1().min_w_0().truncate().t_body().text_color(hex(Chrome::BRIGHT)).child(connection.name.clone()),
                        ),
                    )
                    .child(div().t_caption().text_color(hex(Chrome::FOREGROUND)).truncate().child(connection.config.summary()))
                    .child(div().t_caption().text_color(hex(Chrome::MUTED)).truncate().child(source))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.select_db_connection(id.clone(), cx)));
            for missing in &connection.missing {
                row = row.child(div().t_caption().text_color(hex(Chrome::WARNING)).child(t(cx, missing_label(*missing))));
            }
            if connection.missing.contains(&Missing::Password) {
                let id = connection.id.clone();
                row = row.child(div().pt_1().child(action_button(
                    SharedString::from(format!("db-password-{}", connection.id)),
                    t(cx, "db.enter_password"),
                    cx.listener(move |this, _: &ClickEvent, window, cx| {
                        cx.stop_propagation();
                        this.ask_db_password(id.clone(), window, cx)
                    }),
                )));
            }
            if connection.source == Source::Manual {
                let id = connection.id.clone();
                row = row.child(div().pt_1().child(action_button(
                    SharedString::from(format!("db-remove-{}", connection.id)),
                    t(cx, "db.remove"),
                    cx.listener(move |this, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        let _ = store::remove_manual(&id);
                        this.db.stale = true;
                        cx.notify();
                    }),
                )));
            }
            connections = connections.child(row);
        }

        // Tables.
        let mut tables = div()
            .id("db-tables")
            .w(px(tables_width))
            .flex_shrink_0()
            .h_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(hex(Chrome::BORDER));
        tables = tables.child(heading(t(cx, "db.tables").to_string()));
        for table in &db.tables {
            let name = table.name.clone();
            let selected = db.table.as_deref() == Some(table.name.as_str());
            tables = tables.child(
                div()
                    .id(SharedString::from(format!("db-table-{}", table.name)))
                    .mx_1()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .when(selected, |d| d.bg(hex(Chrome::SELECTED)))
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .child(icon(if table.kind == "view" { "eye" } else { "rows-2" }, 12., hex(Chrome::MUTED)))
                    .child(div().min_w_0().truncate().t_small().text_color(hex(Chrome::FOREGROUND)).child(table.name.clone()))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.open_db_table(name.clone(), cx))),
            );
        }

        // Query editor: as tall as the user dragged it, with its own toolbar underneath.
        let height = if db.editor_height > 0. { db.editor_height } else { QUERY_HEIGHT };
        let query = db.editor.clone().map(|editor| {
            div()
                .flex()
                .flex_col()
                .flex_shrink_0()
                .border_b_1()
                .border_color(hex(Chrome::BORDER))
                .child(div().h(px(height)).w_full().bg(hex(Chrome::EDITOR)).child(editor))
                // Drag the bar under the editor to make it taller.
                .child(
                    div()
                        .id("db-query-resize")
                        .h(px(5.))
                        .w_full()
                        .flex_shrink_0()
                        .cursor(gpui::CursorStyle::ResizeUpDown)
                        .bg(hex(Chrome::BORDER))
                        .hover(|s| s.bg(hex(Chrome::ACCENT)))
                        .on_drag(DbQueryDrag, |_, _, _, cx| cx.new(|_| DbQueryDrag))
                        // Moves by how far the pointer is from the bar drawn in the last frame, so
                        // it follows the pointer however far it was dragged in one go.
                        .on_drag_move(cx.listener(|this, event: &gpui::DragMoveEvent<DbQueryDrag>, _, cx| {
                            let delta = f32::from(event.event.position.y) - f32::from(event.bounds.origin.y);
                            let current = if this.db.editor_height > 0. { this.db.editor_height } else { QUERY_HEIGHT };
                            this.db.editor_height = (current + delta).clamp(QUERY_MIN_HEIGHT, QUERY_MAX_HEIGHT);
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .py_1p5()
                        .child(div().flex_1().t_caption().text_color(hex(Chrome::MUTED)).child(t(cx, "db.query_hint")))
                        .child(action_button("db-run", t(cx, "db.run"), cx.listener(|this, _: &ClickEvent, _, cx| this.run_db_query(cx)))),
                )
        });
        let mut body = div().flex_1().min_w_0().h_full().flex().flex_col().children(query);
        if let Some((text, error)) = &db.message {
            body = body.child(
                div().px_3().py_2().t_small().text_color(hex(if *error { Chrome::ERROR } else { Chrome::SUCCESS })).child(text.clone()),
            );
        }
        if db.busy {
            body = body.child(crate::ui::loading_row(t(cx, "db.loading")));
        }
        body = body.child(match &db.result {
            Some(result) => self.render_db_grid(result, cx),
            None => div().p_4().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "db.pick_table")).into_any_element(),
        });

        div()
            .size_full()
            .relative()
            .flex()
            .bg(hex(Chrome::EDITOR))
            .child(connections)
            .child(column_handle(false, cx))
            .child(tables)
            .child(column_handle(true, cx))
            .child(body)
            .children(self.render_db_password(cx))
            .children(self.render_db_form(cx))
            .into_any_element()
    }

    fn render_db_grid(&self, result: &QueryResult, cx: &mut Context<Self>) -> AnyElement {
        // Measured when the result arrived; what the user dragged a column to wins over it.
        let widths: Vec<f32> = result
            .columns
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let measured = self.db.grid_measured.get(i).copied().unwrap_or(COLUMN_MIN_WIDTH);
                self.db.grid_widths.get(&(i, name.clone())).copied().unwrap_or(measured)
            })
            .collect();
        let width = px(ROW_NUMBER_WIDTH + widths.iter().sum::<f32>());
        let number_cell = |text: String, header: bool| {
            div()
                .w(px(ROW_NUMBER_WIDTH))
                .flex_shrink_0()
                .px_1p5()
                .py_0p5()
                .text_align(gpui::TextAlign::Right)
                .t_caption()
                .text_color(hex(Chrome::MUTED))
                .when(header, |d| d.py_1())
                .child(text)
        };
        let header = div()
            .flex()
            .w(width)
            .border_b_1()
            .border_color(hex(Chrome::BORDER))
            .bg(hex(Chrome::PANEL))
            .child(number_cell("#".into(), true))
            .children(result.columns.iter().enumerate().map(|(i, c)| {
                let name = c.clone();
                let start = widths[i];
                div()
                    .relative()
                    .w(px(widths[i]))
                    .flex_shrink_0()
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .truncate()
                            .t_small()
                            .font_weight(crate::theme::EMPHASIS)
                            .text_color(hex(Chrome::BRIGHT))
                            .child(c.clone()),
                    )
                    // The edge between this column and the next one, as in any table.
                    .child(
                        div()
                            .id(("db-grid-resize", i))
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .right(px(-2.))
                            .w(px(5.))
                            .cursor(gpui::CursorStyle::ResizeLeftRight)
                            .hover(|s| s.bg(hex(Chrome::ACCENT)))
                            .on_drag(DbGridDrag, |_, _, _, cx| cx.new(|_| DbGridDrag))
                            .on_drag_move(cx.listener(move |this, event: &gpui::DragMoveEvent<DbGridDrag>, _, cx| {
                                let delta = f32::from(event.event.position.x) - f32::from(event.bounds.origin.x);
                                let key = (i, name.clone());
                                let current = this.db.grid_widths.get(&key).copied().unwrap_or(start);
                                this.db.grid_widths.insert(key, (current + delta).clamp(COLUMN_MIN_WIDTH, COLUMN_DRAG_MAX_WIDTH));
                                cx.notify();
                            })),
                    )
            }));
        let picked = self.db.picked_row;
        let rows = result.rows.iter().enumerate().map(|(i, row)| {
            div()
                .id(("db-row", i))
                .flex()
                .w(width)
                .cursor_pointer()
                .when(i % 2 == 1, |d| d.bg(hex_alpha(0xffffff, 0.025)))
                .when(picked == Some(i), |d| d.bg(hex_alpha(Chrome::ACCENT, 0.35)))
                .hover(|s| s.bg(hex(Chrome::HOVER)))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.db.picked_row = if this.db.picked_row == Some(i) { None } else { Some(i) };
                    cx.notify();
                }))
                .child(number_cell((i + 1).to_string(), false))
                .children(row.iter().enumerate().map(|(c, value)| {
                    let text = cell_text(value);
                    div()
                        .w(px(widths.get(c).copied().unwrap_or(COLUMN_MIN_WIDTH)))
                        .flex_shrink_0()
                        .px_2()
                        .py_0p5()
                        .truncate()
                        .t_small()
                        .font_family("monospace")
                        .text_color(hex(if value.is_null() { Chrome::MUTED } else { Chrome::FOREGROUND }))
                        .child(if value.is_null() { "NULL".to_string() } else { text.lines().next().unwrap_or("").to_string() })
                }))
        });
        let footer = if result.truncated {
            tf(cx, "db.rows_first", &[("n", &result.rows.len().to_string())])
        } else {
            tf(cx, "db.rows", &[("n", &result.rows.len().to_string())])
        };
        // The picked row in full: values a cell had to cut off are readable here.
        let detail = picked.and_then(|i| result.rows.get(i)).map(|row| {
            div()
                .id("db-row-detail")
                .max_h(px(200.))
                .overflow_y_scroll()
                .flex_shrink_0()
                .border_t_1()
                .border_color(hex(Chrome::BORDER))
                .bg(hex(Chrome::PANEL))
                .children(result.columns.iter().enumerate().map(|(i, name)| {
                    let value = row.get(i);
                    let null = value.is_some_and(|v| v.is_null());
                    div()
                        .flex()
                        .gap_2()
                        .px_3()
                        .py_0p5()
                        .t_small()
                        .child(div().w(px(160.)).flex_shrink_0().truncate().text_color(hex(Chrome::MUTED)).child(name.clone()))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .font_family("monospace")
                                .text_color(hex(if null { Chrome::MUTED } else { Chrome::BRIGHT }))
                                .child(value.map(cell_text).unwrap_or_default()),
                        )
                }))
        });
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .id("db-grid")
                            .size_full()
                            .overflow_scroll()
                            .track_scroll(&self.db.grid_scroll)
                            // As wide as its columns, so the grid scrolls sideways as well as down.
                            .child(div().flex().flex_col().flex_shrink_0().min_w(width).child(header).children(rows)),
                    )
                    .group(crate::ui::SCROLL_GROUP)
                    .child(crate::ui::scrollbar(self.db.grid_scroll.clone()))
                    .child(crate::ui::scrollbar_h(self.db.grid_scroll.clone())),
            )
            .children(detail)
            .child(
                div().px_3().py_1().border_t_1().border_color(hex(Chrome::BORDER)).t_caption().text_color(hex(Chrome::MUTED)).child(footer),
            )
            .into_any_element()
    }

    fn render_db_password(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (id, input) = self.db.password.as_ref()?;
        let name = self.db.connections.iter().find(|c| &c.id == id).map(|c| c.config.summary()).unwrap_or_default();
        Some(
            overlay(
                "db-password-dialog",
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .t_title()
                            .font_weight(crate::theme::EMPHASIS)
                            .text_color(hex(Chrome::BRIGHT))
                            .child(t(cx, "db.enter_password")),
                    )
                    .child(div().t_small().text_color(hex(Chrome::FOREGROUND)).child(name))
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(hex(Chrome::ACCENT))
                            .bg(hex(0x1a1a1a))
                            .t_body()
                            .child(input.clone()),
                    )
                    .child(div().t_caption().text_color(hex(Chrome::MUTED)).child(tf(
                        cx,
                        "db.password_where",
                        &[("store", agentty_bridge::secret_store::backend_name())],
                    )))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(action_button(
                                "db-password-cancel",
                                t(cx, "confirm.cancel"),
                                cx.listener(|this, _: &ClickEvent, _, cx| {
                                    this.db.password = None;
                                    cx.notify();
                                }),
                            ))
                            .child(action_button(
                                "db-password-save",
                                t(cx, "db.save"),
                                cx.listener(|this, _: &ClickEvent, _, cx| this.save_db_password(cx)),
                            )),
                    ),
            )
            .into_any_element(),
        )
    }

    fn render_db_form(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let form = self.db.form.as_ref()?;
        let field = |label: &'static str, input: &Entity<TextInput>, cx: &mut Context<Self>| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().w(px(90.)).flex_shrink_0().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, label)))
                .child(
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
                        .child(input.clone()),
                )
        };
        let mut engines = div().flex().flex_wrap().gap_1();
        for engine in [Engine::MySql, Engine::MariaDb, Engine::Postgres, Engine::Oracle, Engine::MongoDb] {
            engines = engines.child(crate::ui::chip(
                SharedString::from(format!("db-engine-{engine:?}")),
                engine.label().to_string(),
                form.engine == engine,
                cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if let Some(form) = this.db.form.as_mut() {
                        form.engine = engine;
                        let port = engine.default_port().to_string();
                        form.port.update(cx, |input, cx| input.set_text(port, cx));
                    }
                    cx.notify();
                }),
            ));
        }
        let tls_off = form.tls_off;
        Some(
            overlay(
                "db-form-dialog",
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().t_title().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(t(cx, "db.add_title")))
                    .child(div().t_caption().text_color(hex(Chrome::MUTED)).child(t(cx, "db.rds_hint")))
                    .child(engines)
                    .child(field("db.form.name", &form.name, cx))
                    .child(field("db.form.host", &form.host, cx))
                    .child(field("db.form.port", &form.port, cx))
                    .child(field("db.form.database", &form.database, cx))
                    .child(field("db.form.user", &form.user, cx))
                    .child(field("db.form.password", &form.password, cx))
                    .child(
                        div()
                            .id("db-form-tls")
                            .flex()
                            .items_center()
                            .gap_2()
                            .cursor_pointer()
                            .t_small()
                            .text_color(hex(Chrome::FOREGROUND))
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                if let Some(form) = this.db.form.as_mut() {
                                    form.tls_off = !form.tls_off;
                                }
                                cx.notify();
                            }))
                            .child(icon(if tls_off { "check" } else { "square" }, 13., hex(Chrome::FOREGROUND)))
                            .child(t(cx, "db.form.tls_off")),
                    )
                    .child(
                        div()
                            .pt_1()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(action_button(
                                "db-form-cancel",
                                t(cx, "confirm.cancel"),
                                cx.listener(|this, _: &ClickEvent, _, cx| {
                                    this.db.form = None;
                                    cx.notify();
                                }),
                            ))
                            .child(action_button(
                                "db-form-save",
                                t(cx, "db.save"),
                                cx.listener(|this, _: &ClickEvent, _, cx| this.save_db_form(cx)),
                            )),
                    ),
            )
            .into_any_element(),
        )
    }

    /// The approval dialog for a write or schema change (from an agent or the query box).
    pub(super) fn render_db_approval(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let approval = self.db.approvals.front()?;
        let id = approval.id;
        let waiting = self.db.approvals.len() - 1;
        let kind = if approval.verdict == Verdict::Ddl { t(cx, "db.kind_ddl") } else { t(cx, "db.kind_write") };
        let button = |id: &'static str, label: String, danger: bool| {
            div()
                .id(id)
                .px_3()
                .py_1p5()
                .rounded_md()
                .t_body()
                .cursor_pointer()
                .bg(if danger { hex(Chrome::ERROR) } else { hex(0x2d2d30) })
                .text_color(hex(Chrome::BRIGHT))
                .hover(|s| s.opacity(0.85))
                .child(label)
        };
        Some(
            div()
                .id("db-approval-overlay")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(hex_alpha(0x000000, 0.5))
                .occlude()
                .child(
                    div()
                        .id("db-approval")
                        .w(px(640.))
                        .max_h(gpui::relative(0.9))
                        .p_5()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .rounded_xl()
                        .bg(hex(Chrome::OVERLAY))
                        .border_1()
                        .border_color(hex(Chrome::ERROR))
                        .shadow_lg()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(icon("shield-alert", crate::ui::IconSize::BUTTON, hex(Chrome::ERROR)))
                                .child(
                                    div()
                                        .t_title()
                                        .font_weight(crate::theme::EMPHASIS)
                                        .text_color(hex(Chrome::BRIGHT))
                                        .child(t(cx, "db.approve_title")),
                                )
                                .child(div().flex_1())
                                .when(waiting > 0, |d| {
                                    d.child(div().t_small().text_color(hex(Chrome::MUTED)).child(tf(
                                        cx,
                                        "tasks.waiting",
                                        &[("n", &waiting.to_string())],
                                    )))
                                }),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(detail(t(cx, "db.approve_asker"), approval.asker.clone(), Chrome::FOREGROUND))
                                .child(detail(t(cx, "db.approve_conn"), approval.connection.config.summary(), Chrome::FOREGROUND))
                                .child(detail(t(cx, "db.approve_kind"), kind.to_string(), Chrome::ERROR)),
                        )
                        .child(
                            div()
                                .id("db-approval-statement")
                                .max_h(px(320.))
                                .overflow_y_scroll()
                                .p_3()
                                .rounded_md()
                                .bg(hex(0x141414))
                                .border_1()
                                .border_color(hex(Chrome::BORDER))
                                .font_family(crate::settings::BUNDLED_FONT)
                                .t_small()
                                .text_color(hex(Chrome::BRIGHT))
                                .child(approval.job.describe()),
                        )
                        .child(div().t_caption().text_color(hex(Chrome::MUTED)).child(t(cx, "db.approve_note")))
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .child(
                                    button("db-approval-decline", t(cx, "db.decline").to_string(), false)
                                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.answer_db_approval(id, false, cx))),
                                )
                                .child(
                                    button("db-approval-execute", t(cx, "db.execute").to_string(), true)
                                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.answer_db_approval(id, true, cx))),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}

/// One "label  value" line of the approval dialog.
/// The draggable edge between two of the page's columns.
fn column_handle(tables: bool, cx: &mut Context<Workbench>) -> gpui::Stateful<gpui::Div> {
    div()
        .id(if tables { "db-tables-resize" } else { "db-connections-resize" })
        .w(px(5.))
        .flex_shrink_0()
        .h_full()
        .cursor(gpui::CursorStyle::ResizeLeftRight)
        .hover(|s| s.bg(hex(Chrome::ACCENT)))
        .on_drag(DbColumnDrag, |_, _, _, cx| cx.new(|_| DbColumnDrag))
        // Moves by how far the pointer is from the edge drawn in the last frame.
        .on_drag_move(cx.listener(move |this, event: &gpui::DragMoveEvent<DbColumnDrag>, _, cx| {
            let delta = f32::from(event.event.position.x) - f32::from(event.bounds.origin.x);
            if tables {
                let current = if this.db.tables_width > 0. { this.db.tables_width } else { TABLES_WIDTH };
                this.db.tables_width = (current + delta).clamp(COLUMN_MIN, COLUMN_MAX);
            } else {
                let current = if this.db.connections_width > 0. { this.db.connections_width } else { CONNECTIONS_WIDTH };
                this.db.connections_width = (current + delta).clamp(COLUMN_MIN, COLUMN_MAX);
            }
            cx.notify();
        }))
}

fn detail(label: &str, value: String, color: u32) -> gpui::Div {
    div()
        .flex()
        .gap_2()
        .t_small()
        .child(div().w(px(80.)).flex_shrink_0().text_color(hex(Chrome::MUTED)).child(label.to_string()))
        .child(div().flex_1().min_w_0().truncate().text_color(hex(color)).child(value))
}

/// A small modal box over the page.
fn overlay(id: &'static str, content: gpui::Div) -> gpui::Stateful<gpui::Div> {
    div().id(id).absolute().inset_0().flex().items_center().justify_center().bg(hex_alpha(0x000000, 0.45)).occlude().child(
        div()
            .w(px(460.))
            .p_5()
            .rounded_xl()
            .bg(hex(Chrome::OVERLAY))
            .border_1()
            .border_color(hex(Chrome::OVERLAY_BORDER))
            .shadow_lg()
            .child(content),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jobs_describe_themselves_and_classify() {
        let job = Job::Sql("DELETE FROM users WHERE id = 1".into());
        assert_eq!(job.verdict(Engine::MySql), Verdict::Write);
        assert_eq!(job.describe(), "DELETE FROM users WHERE id = 1");
        let job = Job::Mongo { op: MongoOp::DropCollection, collection: "users".into(), args: Value::Null };
        assert_eq!(job.verdict(Engine::MongoDb), Verdict::Ddl);
        assert!(job.describe().starts_with("db.users.drop("));
        assert_eq!(Job::Sql("select 1".into()).verdict(Engine::MySql), Verdict::Read);
        assert_eq!(cell_text(&Value::Null), "NULL");
        assert_eq!(cell_text(&serde_json::json!({"a": 1})), "{\"a\":1}");
    }

    /// Columns are measured once, off the values themselves: the old code built a string per cell
    /// and did it again on every frame.
    #[test]
    fn columns_are_measured_from_what_they_hold() {
        let result = QueryResult {
            columns: vec!["id".into(), "note".into()],
            rows: vec![
                vec![serde_json::json!(1), serde_json::json!("a longer note than the id column")],
                vec![serde_json::json!(2), Value::Null],
            ],
            truncated: false,
            affected: None,
        };
        let widths = measure_columns(&result);
        assert_eq!(widths.len(), 2);
        assert!(widths[1] > widths[0], "the note column is wider than the id column");
        assert!(widths.iter().all(|w| (COLUMN_MIN_WIDTH..=COLUMN_MAX_WIDTH).contains(w)));
        // Measuring never builds the cell's text.
        assert_eq!(cell_chars(&Value::Null), "NULL".len());
        assert_eq!(cell_chars(&serde_json::json!("한글")), 2);
        assert_eq!(cell_chars(&serde_json::json!("x".repeat(500))), 120);
    }

    /// The dialog shows what runs: a statement that would render differently is refused outright.
    #[test]
    fn statements_that_would_not_read_as_they_run_are_refused() {
        assert_eq!(checked_statement("  select 1  ").unwrap(), "select 1");
        assert!(checked_statement("").is_err());
        assert!(checked_statement(&"x".repeat(MAX_STATEMENT + 1)).is_err());
        // A right-to-left override would show the clauses in another order than they run.
        assert!(checked_statement("select 1 \u{202e} delete from users").is_err());
        assert!(checked_statement("select\u{200b}1").is_err());
        assert!(checked_statement("select 1\u{0}").is_err());
        // Newlines and tabs are how people write SQL.
        assert!(checked_statement("select 1\n\tfrom t").is_ok());
    }

    #[test]
    fn a_read_job_never_writes() {
        // `run_job` checks again: a write handed to the read path is refused before connecting.
        let config = agentty_db::detect::parse_url("mysql://nobody@127.0.0.1:1/none").unwrap();
        let error = run_job(&config, &Job::Sql("DROP TABLE users".into()), false, 10).unwrap_err();
        assert!(format!("{error:#}").contains("approval"), "refused before connecting: {error:#}");
    }
}
