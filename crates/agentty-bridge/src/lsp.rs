//! Language servers (LSP) for the code editor: the program each language already has for exact
//! "go to definition", "find usages" and hover text (rust-analyzer, gopls, typescript-language-
//! server, pyright, jdtls, kotlin-language-server). One server runs per project and language,
//! started on first use from the user's `PATH` and spoken to over stdin/stdout. A missing server
//! just means the editor keeps its own name-based index (and offers the server's official install
//! command), and so does a server still starting or indexing when an answer is wanted.
//!
//! Servers analyse code without running it where they can: rust-analyzer starts with build
//! scripts, proc macros and `cargo check` off. jdtls and kotlin-language-server read Gradle and
//! Maven projects the way an IDE opening the folder does.
//!
//! Servers also report problems (errors, warnings) in the open files: by themselves
//! (`publishDiagnostics`), or when asked (`textDocument/diagnostic`, TypeScript 7). [`check`]
//! hands a file over, [`diagnostics`] reads what was reported last.
//!
//! Every call here blocks (reads, waits for an answer) except [`diagnostics`] and [`status`]; the
//! editor calls them off the UI thread.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How long a server gets to answer `initialize` (jdtls takes a while on a cold start).
const INIT_TIMEOUT: Duration = Duration::from_secs(60);
/// A server that could not be found or started is looked for again after this (it may have been
/// installed meanwhile).
const RETRY_AFTER: Duration = Duration::from_secs(30);
/// The largest message read from a server.
const MAX_MESSAGE: usize = 64 * 1024 * 1024;
/// Places kept from one answer at most.
const MAX_PLACES: usize = 2_000;
/// Problems kept for one file at most.
const MAX_DIAGNOSTICS: usize = 1_000;
/// How long a server that is asked for a file's problems (`textDocument/diagnostic`) gets.
const PULL_TIMEOUT: Duration = Duration::from_secs(15);

/// A language server the editor knows how to start.
pub struct Spec {
    /// Shown in notes ("rust-analyzer").
    pub name: &'static str,
    /// The LSP language ids it serves.
    languages: &'static [&'static str],
    /// Programs to try, in order, with their arguments (`launches` adds what some need besides).
    commands: &'static [(&'static str, &'static [&'static str])],
}

const SPECS: &[Spec] = &[
    Spec { name: "rust-analyzer", languages: &["rust"], commands: &[("rust-analyzer", &[])] },
    Spec { name: "gopls", languages: &["go"], commands: &[("gopls", &[])] },
    Spec {
        name: "typescript-language-server",
        languages: &["typescript", "typescriptreact", "javascript", "javascriptreact"],
        commands: &[("typescript-language-server", &["--stdio"])],
    },
    Spec {
        name: "pyright",
        languages: &["python"],
        commands: &[("pyright-langserver", &["--stdio"]), ("basedpyright-langserver", &["--stdio"]), ("pylsp", &[])],
    },
    Spec { name: "jdtls", languages: &["java"], commands: &[("jdtls", &[])] },
    Spec { name: "kotlin-language-server", languages: &["kotlin"], commands: &[("kotlin-language-server", &[])] },
];

impl Spec {
    /// What gets installed and runs for these languages, as the user sees it.
    pub fn program(&self) -> &'static str {
        match self.name {
            "typescript-language-server" => "typescript",
            name => name,
        }
    }
}

/// The server for an LSP language id, when there is one.
pub fn spec_for(language: &str) -> Option<&'static Spec> {
    SPECS.iter().find(|s| s.languages.contains(&language))
}

/// A place a server pointed to, with the column in bytes of that line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub path: PathBuf,
    pub line: usize,
    pub col: usize,
    /// Bytes the name takes on that line (0 when it spans lines).
    pub len: usize,
}

/// What the server for a project and language is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// No server for this language, or none installed.
    Missing,
    Starting,
    /// Running, still reading the project.
    Indexing,
    Ready,
    /// Started and stopped, or did not answer `initialize`.
    Failed,
    /// Not started for this project: the server would run the project's build to read it.
    Skipped,
}

/// What an answer looked like: `None` when no server could be asked in time.
pub type Answer<T> = Option<T>;

/// How bad a problem is, worst first (LSP's numbering).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Error = 1,
    Warning = 2,
    Information = 3,
    Hint = 4,
}

/// A problem a server reported in a file: from `start` to `end` as (line, column in bytes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub start: (usize, usize),
    pub end: (usize, usize),
    pub severity: Severity,
    pub message: String,
    /// Who found it (`ts`, `Pyright`, `rustc`) and its code (`2322`, `reportUndefinedVariable`).
    pub source: Option<String>,
    pub code: Option<String>,
}

/// A problem as the server sent it: columns in its own units.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Reported {
    start: (usize, usize),
    end: (usize, usize),
    severity: Severity,
    message: String,
    source: Option<String>,
    code: Option<String>,
    /// As sent: handed back when asking for fixes of it.
    raw: Value,
}

/// Goes up whenever some server reports problems, so the editor knows when to read them again.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// See [`GENERATION`].
pub fn generation() -> u64 {
    GENERATION.load(Ordering::SeqCst)
}

/// Starts the server for `root` and `language` in the background, so it has read the project by
/// the time it is asked something.
pub fn warm(root: &Path, language: &str) {
    let Some(spec) = spec_for(language) else { return };
    let slot = slot(root, spec);
    if slot.needs_start() {
        let (root, slot) = (root.to_path_buf(), slot.clone());
        let _ = std::thread::Builder::new().name("lsp-start".into()).spawn(move || slot.start(&root, spec));
    }
}

/// What the server for `root` and `language` is doing (without starting it).
pub fn status(root: &Path, language: &str) -> Status {
    let Some(spec) = spec_for(language) else { return Status::Missing };
    let Some(slot) = registry().lock().ok().and_then(|r| r.get(&key(root, spec)).cloned()) else {
        return Status::Missing;
    };
    let state = slot.state.lock().map(|s| s.clone());
    match state {
        Ok(State::Ready(client)) if !client.alive() => Status::Failed,
        Ok(State::Ready(client)) if client.indexing() => Status::Indexing,
        Ok(State::Ready(_)) => Status::Ready,
        Ok(State::Starting) | Ok(State::Idle) => Status::Starting,
        Ok(State::Missing(_)) => Status::Missing,
        Ok(State::Skipped) => Status::Skipped,
        Ok(State::Failed(_)) | Err(_) => Status::Failed,
    }
}

/// Where the name at `line`/`col` (bytes) of `path` is defined. `text` is the file as the editor
/// has it, unsaved edits included.
pub fn definition(root: &Path, language: &str, path: &Path, text: &str, line: usize, col: usize, wait: Duration) -> Answer<Vec<Place>> {
    let (value, utf8) = ask(root, language, path, text, line, col, "textDocument/definition", json!({}), wait)?;
    Some(places(&value, path, text, utf8))
}

/// Every place the name at `line`/`col` of `path` is used, its definition included.
pub fn references(root: &Path, language: &str, path: &Path, text: &str, line: usize, col: usize, wait: Duration) -> Answer<Vec<Place>> {
    let extra = json!({ "context": { "includeDeclaration": true } });
    let (value, utf8) = ask(root, language, path, text, line, col, "textDocument/references", extra, wait)?;
    Some(places(&value, path, text, utf8))
}

/// The server's hover text for the name at `line`/`col` of `path` (its type or signature, then
/// its documentation), as plain text.
pub fn hover(root: &Path, language: &str, path: &Path, text: &str, line: usize, col: usize, wait: Duration) -> Answer<String> {
    let (value, _) = ask(root, language, path, text, line, col, "textDocument/hover", json!({}), wait)?;
    Some(hover_text(&value))
}

/// Hands the server the editor's text of `path` and, from a server that is asked rather than
/// telling by itself, its problems. Waits up to `wait` for a server still starting. The problems
/// are read with [`diagnostics`] once [`generation`] moves.
pub fn check(root: &Path, language: &str, path: &Path, text: &str, wait: Duration) {
    let Some(spec) = spec_for(language) else { return };
    warm(root, language);
    let Some(client) = slot(root, spec).wait_ready(Instant::now() + wait) else { return };
    client.sync(path, language, text);
    if !client.pull.load(Ordering::SeqCst) {
        return;
    }
    let Some(uri) = file_uri(path) else { return };
    let Some(value) = client.request("textDocument/diagnostic", json!({ "textDocument": { "uri": uri } }), PULL_TIMEOUT) else {
        return;
    };
    // "unchanged" keeps what was reported last.
    if value.get("kind").and_then(Value::as_str) == Some("full") {
        let items = value.get("items").and_then(Value::as_array).map(|items| reported(items)).unwrap_or_default();
        client.report(path, items);
    }
}

/// The problems the server of `language` last reported in `path`, worst first then by place,
/// with columns in bytes of the lines `line` gives (the editor's text as it is now).
pub fn diagnostics<'a>(root: &Path, language: &str, path: &Path, line: impl Fn(usize) -> Option<&'a str>) -> Vec<Diagnostic> {
    let Some(client) = spec_for(language).and_then(|spec| ready_client(root, spec)) else { return Vec::new() };
    let utf8 = client.utf8.load(Ordering::SeqCst);
    let Some(found) = client.diagnostics.lock().ok().and_then(|d| d.get(&diagnostics_key(path)).cloned()) else { return Vec::new() };
    let column = |(row, units): (usize, usize)| {
        let text = line(row).unwrap_or("");
        let col = if utf8 { units.min(text.len()) } else { byte_col(text, units) };
        // A column in the middle of a character (a server counting otherwise) goes to its start.
        (row, (0..=col).rev().find(|c| text.is_char_boundary(*c)).unwrap_or(0))
    };
    let mut out: Vec<Diagnostic> = found
        .into_iter()
        .map(|r| Diagnostic {
            start: column(r.start),
            end: column(r.end),
            severity: r.severity,
            message: r.message,
            source: r.source,
            code: r.code,
        })
        .collect();
    out.sort_by_key(|d| (d.start, d.severity));
    out
}

/// A fix or refactoring a server offers for some text (`textDocument/codeAction`).
#[derive(Clone, Debug, PartialEq)]
pub struct Action {
    pub title: String,
    /// `quickfix`, `refactor.extract`, `source.organizeImports`, …
    pub kind: Option<String>,
    /// The server's pick among fixes for the same problem.
    pub preferred: bool,
    /// It fixes one of the problems it was asked about.
    pub fixes: bool,
    value: Value,
}

/// Text changes to one file, with columns in the server's units ([`edited`] applies them).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileEdit {
    pub path: PathBuf,
    edits: Vec<TextEdit>,
    utf8: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TextEdit {
    start: (usize, usize),
    end: (usize, usize),
    text: String,
}

/// What a server offers for `start..end` of `path` (line, byte column), with the problems
/// reported there: fixes first, the server's pick first among them. Actions that would create,
/// rename or delete files are left out.
#[allow(clippy::too_many_arguments)]
pub fn actions(
    root: &Path,
    language: &str,
    path: &Path,
    text: &str,
    start: (usize, usize),
    end: (usize, usize),
    wait: Duration,
) -> Answer<Vec<Action>> {
    let spec = spec_for(language)?;
    warm(root, language);
    let client = slot(root, spec).wait_ready(Instant::now() + wait)?;
    client.sync(path, language, text);
    let uri = file_uri(path)?;
    let line = |n: usize| text.split('\n').nth(n).unwrap_or("");
    let at = |(row, col): (usize, usize)| (row, client.encode_col(line(row), col));
    let (from, to) = (at(start), at(end));
    let problems: Vec<Value> = client
        .diagnostics
        .lock()
        .ok()
        .and_then(|d| d.get(&diagnostics_key(path)).cloned())
        .unwrap_or_default()
        .into_iter()
        .filter(|d| d.start <= to && d.end >= from)
        .map(|d| d.raw)
        .collect();
    let range = |(line, character): (usize, usize)| json!({ "line": line, "character": character });
    let params = json!({
        "textDocument": { "uri": uri },
        "range": { "start": range(from), "end": range(to) },
        "context": { "diagnostics": problems, "triggerKind": 1 },
    });
    let value = client.request("textDocument/codeAction", params, wait)?;
    let mut found: Vec<Action> = value.as_array().map(|items| items.iter().filter_map(action).collect()).unwrap_or_default();
    found.sort_by_key(|a| (!a.fixes, !a.preferred));
    Some(found)
}

/// The changes `action` makes: its own edit (asked for when the server sends it on request),
/// and those the server applies while running its command (`workspace/applyEdit`). `None` when
/// the server could not be asked or answered with something this editor cannot apply.
pub fn apply(root: &Path, language: &str, action: &Action, wait: Duration) -> Option<Vec<FileEdit>> {
    let client = ready_client(root, spec_for(language)?)?;
    let utf8 = client.utf8.load(Ordering::SeqCst);
    let mut value = action.value.clone();
    let is_command = value.get("command").is_some_and(Value::is_string);
    if !is_command && value.get("edit").is_none() && client.resolve.load(Ordering::SeqCst) {
        value = client.request("codeAction/resolve", value, wait)?;
    }
    let mut out = match value.get("edit") {
        Some(edit) => file_edits(edit, utf8)?,
        None => Vec::new(),
    };
    let command = if is_command { Some(&value) } else { value.get("command").filter(|c| c.is_object()) };
    if let Some(command) = command {
        if let Ok(mut applied) = client.applied.lock() {
            applied.clear();
        }
        let params = json!({ "command": command.get("command"), "arguments": command.get("arguments").cloned().unwrap_or(json!([])) });
        client.request("workspace/executeCommand", params, wait)?;
        let applied = client.applied.lock().map(|mut a| std::mem::take(&mut *a)).unwrap_or_default();
        for edit in applied {
            out.extend(file_edits(&edit, utf8)?);
        }
    }
    Some(out)
}

/// What renaming the name at `line`/`col` (bytes) of `path` to `new_name` changes, in every file
/// (`textDocument/rename`). `Err` carries the server's reason when it refuses (not a name, a
/// library's code, a rename that would also rename files). `None`: no server answered in time.
#[allow(clippy::too_many_arguments)]
pub fn rename(
    root: &Path,
    language: &str,
    path: &Path,
    text: &str,
    line: usize,
    col: usize,
    new_name: &str,
    wait: Duration,
) -> Answer<Result<Vec<FileEdit>, String>> {
    let spec = spec_for(language)?;
    warm(root, language);
    let client = slot(root, spec).wait_ready(Instant::now() + wait)?;
    client.sync(path, language, text);
    let uri = file_uri(path)?;
    let character = client.encode_col(text.split('\n').nth(line).unwrap_or(""), col);
    let params = json!({ "textDocument": { "uri": uri }, "position": { "line": line, "character": character }, "newName": new_name });
    Some(match client.ask("textDocument/rename", params, wait)? {
        Err(message) => Err(message),
        Ok(Value::Null) => Err(String::new()),
        Ok(edit) => file_edits(&edit, client.utf8.load(Ordering::SeqCst)).ok_or_else(String::new),
    })
}

/// The number of changes in `edit`.
pub fn edit_count(edit: &FileEdit) -> usize {
    edit.edits.len()
}

/// `text` with the changes of `edit` made (overlapping ones after the first are skipped).
pub fn edited(text: &str, edit: &FileEdit) -> String {
    let starts: Vec<usize> = std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect();
    let offset = |(line, units): (usize, usize)| match starts.get(line) {
        Some(&start) => {
            let end = text[start..].find('\n').map_or(text.len(), |i| start + i);
            let line = text[start..end].strip_suffix('\r').unwrap_or(&text[start..end]);
            start + if edit.utf8 { units.min(line.len()) } else { byte_col(line, units) }
        }
        None => text.len(),
    };
    let mut changes: Vec<(usize, usize, &str)> = edit.edits.iter().map(|e| (offset(e.start), offset(e.end), e.text.as_str())).collect();
    changes.sort_by_key(|c| (c.0, c.1));
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (start, end, new) in changes {
        if start < at || end < start {
            continue;
        }
        out.push_str(&text[at..start]);
        out.push_str(new);
        at = end;
    }
    out.push_str(&text[at..]);
    out
}

/// An action from a `codeAction` answer (a `CodeAction` or a bare `Command`).
fn action(item: &Value) -> Option<Action> {
    if item.get("disabled").is_some() {
        return None;
    }
    // Creating, renaming or deleting files is not something to do from a quick menu.
    let changes_files = item
        .pointer("/edit/documentChanges")
        .and_then(Value::as_array)
        .is_some_and(|changes| changes.iter().any(|c| c.get("kind").is_some()));
    if changes_files {
        return None;
    }
    Some(Action {
        title: item.get("title")?.as_str()?.to_string(),
        kind: item.get("kind").and_then(Value::as_str).map(str::to_string),
        preferred: item.get("isPreferred").and_then(Value::as_bool).unwrap_or(false),
        fixes: item.get("diagnostics").and_then(Value::as_array).is_some_and(|d| !d.is_empty())
            || item.get("kind").and_then(Value::as_str).is_some_and(|k| k.starts_with("quickfix")),
        value: item.clone(),
    })
}

/// The file edits of an LSP `WorkspaceEdit` (`None` when it also creates, renames or deletes files).
fn file_edits(edit: &Value, utf8: bool) -> Option<Vec<FileEdit>> {
    let text_edits = |edits: &Value| -> Vec<TextEdit> {
        let at = |e: &Value, key: &str| {
            let pos = e.pointer(&format!("/range/{key}"))?;
            Some((pos.get("line")?.as_u64()? as usize, pos.get("character")?.as_u64()? as usize))
        };
        edits
            .as_array()
            .map(|edits| {
                edits
                    .iter()
                    .filter_map(|e| {
                        Some(TextEdit { start: at(e, "start")?, end: at(e, "end")?, text: e.get("newText")?.as_str()?.to_string() })
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut out: Vec<FileEdit> = Vec::new();
    let mut add = |uri: &str, edits: Vec<TextEdit>| {
        if let Some(path) = uri_path(uri) {
            match out.iter_mut().find(|f| f.path == path) {
                Some(file) => file.edits.extend(edits),
                None => out.push(FileEdit { path, edits, utf8 }),
            }
        }
    };
    if let Some(changes) = edit.get("documentChanges").and_then(Value::as_array) {
        for change in changes {
            if change.get("kind").is_some() {
                return None;
            }
            let uri = change.pointer("/textDocument/uri").and_then(Value::as_str)?;
            add(uri, text_edits(change.get("edits")?));
        }
    } else if let Some(changes) = edit.get("changes").and_then(Value::as_object) {
        for (uri, edits) in changes {
            add(uri, text_edits(edits));
        }
    }
    Some(out)
}

/// The editor closed `path`: the server reads it from disk again.
pub fn close(root: &Path, language: &str, path: &Path) {
    if let Some(client) = spec_for(language).and_then(|spec| ready_client(root, spec)) {
        client.close(path);
    }
}

/// Stops every server (when the app quits; they also stop by themselves once their input closes).
pub fn shutdown_all() {
    let slots: Vec<Arc<Slot>> = registry().lock().map(|r| r.values().cloned().collect()).unwrap_or_default();
    for slot in slots {
        if let Ok(State::Ready(client)) = slot.state.lock().map(|s| s.clone()) {
            client.stop();
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Installing servers (Settings → Project and the editor's status bar run the command in a
// terminal tab, where the user sees it work).

/// A server as Settings lists it.
#[derive(Clone)]
pub struct Installable {
    pub name: &'static str,
    /// What gets installed, as the user sees it: the server, or the package that has it built in
    /// (`typescript`, whose TypeScript 7 serves it as `tsc --lsp`).
    pub program: &'static str,
    /// The languages it serves, as the user knows them ("TypeScript, JavaScript").
    pub languages: &'static str,
    pub installed: bool,
    /// The command that installs it here, when the tool it needs is installed.
    pub install: Option<String>,
    /// The tool the command needs and this machine lacks ("Node.js").
    pub needs: Option<&'static str>,
    /// Its installation page.
    pub site: &'static str,
}

/// Every server, whether it is installed and how to install it. Blocks (looks programs up, asks
/// rust-analyzer its version): call it off the UI thread.
pub fn servers() -> Vec<Installable> {
    let path = search_path();
    SPECS
        .iter()
        .map(|spec| {
            let (install, needs) = install_command(spec, &path);
            Installable {
                name: spec.name,
                program: spec.program(),
                languages: shown_languages(spec),
                installed: installed(spec, &path),
                install,
                needs,
                site: site(spec),
            }
        })
        .collect()
}

/// Looks for the servers that were missing or failed again at the next request instead of after
/// [`RETRY_AFTER`] (one was just installed).
pub fn retry_now() {
    let slots: Vec<Arc<Slot>> = registry().lock().map(|r| r.values().cloned().collect()).unwrap_or_default();
    for slot in slots {
        if let Ok(mut state) = slot.state.lock() {
            if matches!(*state, State::Missing(_) | State::Failed(_)) {
                *state = State::Idle;
            }
        }
    }
}

fn search_path() -> std::ffi::OsString {
    crate::process::merge_paths(&crate::process::current_path(), &crate::process::tool_dirs())
}

fn installed(spec: &'static Spec, path: &std::ffi::OsStr) -> bool {
    let found = launches(Path::new(""), spec, path);
    match spec.name {
        // rustup puts a `rust-analyzer` on PATH that only works once the component is added.
        "rust-analyzer" => found.first().is_some_and(|launch| {
            let mut command = crate::process::command(&launch.program);
            command.arg("--version").env("PATH", path).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
            command.status().is_ok_and(|status| status.success())
        }),
        _ => !found.is_empty(),
    }
}

/// The official way to install `spec` with what this machine has, or the tool that way needs.
fn install_command(spec: &'static Spec, path: &std::ffi::OsStr) -> (Option<String>, Option<&'static str>) {
    let has = |program: &str| crate::process::which_in(program, path).is_some();
    let brew = !cfg!(windows) && has("brew");
    let pick = |command: Option<&str>, needs: &'static str| match command {
        Some(command) => (Some(command.to_string()), None),
        None => (None, Some(needs)),
    };
    match spec.name {
        "rust-analyzer" if has("rustup") => pick(Some("rustup component add rust-analyzer"), "rustup"),
        "rust-analyzer" => pick(brew.then_some("brew install rust-analyzer"), "rustup"),
        "gopls" => pick(has("go").then_some("go install golang.org/x/tools/gopls@latest"), "Go"),
        "typescript-language-server" => pick(has("npm").then_some("npm install -g typescript@latest"), "Node.js"),
        "pyright" => pick(has("npm").then_some("npm install -g pyright"), "Node.js"),
        "jdtls" => pick(brew.then_some("brew install jdtls"), "Homebrew"),
        "kotlin-language-server" => pick(brew.then_some("brew install kotlin-language-server"), "Homebrew"),
        _ => (None, None),
    }
}

fn shown_languages(spec: &'static Spec) -> &'static str {
    match spec.name {
        "rust-analyzer" => "Rust",
        "gopls" => "Go",
        "typescript-language-server" => "TypeScript, JavaScript",
        "pyright" => "Python",
        "jdtls" => "Java",
        "kotlin-language-server" => "Kotlin",
        _ => "",
    }
}

fn site(spec: &'static Spec) -> &'static str {
    match spec.name {
        "rust-analyzer" => "https://rust-analyzer.github.io/book/installation.html",
        "gopls" => "https://go.dev/gopls/",
        "typescript-language-server" => "https://www.typescriptlang.org/download/",
        "pyright" => "https://microsoft.github.io/pyright/#/installation",
        "jdtls" => "https://github.com/eclipse-jdtls/eclipse.jdt.ls",
        "kotlin-language-server" => "https://github.com/fwcd/kotlin-language-server",
        _ => "",
    }
}

#[allow(clippy::too_many_arguments)]
fn ask(
    root: &Path,
    language: &str,
    path: &Path,
    text: &str,
    line: usize,
    col: usize,
    method: &str,
    extra: Value,
    wait: Duration,
) -> Option<(Value, bool)> {
    let spec = spec_for(language)?;
    let deadline = Instant::now() + wait;
    warm(root, language);
    let client = slot(root, spec).wait_ready(deadline)?;
    client.sync(path, language, text);
    let uri = file_uri(path)?;
    let character = client.encode_col(text.lines().nth(line).unwrap_or(""), col);
    let mut params = json!({ "textDocument": { "uri": uri }, "position": { "line": line, "character": character } });
    if let (Some(params), Some(extra)) = (params.as_object_mut(), extra.as_object()) {
        params.extend(extra.clone());
    }
    let value = client.request(method, params, deadline.saturating_duration_since(Instant::now()))?;
    Some((value, client.utf8.load(Ordering::SeqCst)))
}

fn ready_client(root: &Path, spec: &'static Spec) -> Option<Arc<Client>> {
    let slot = registry().lock().ok()?.get(&key(root, spec)).cloned()?;
    let state = slot.state.lock().ok()?.clone();
    match state {
        State::Ready(client) if client.alive() => Some(client),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------
// One server per project and language.

#[derive(Clone)]
enum State {
    Idle,
    Starting,
    Ready(Arc<Client>),
    Missing(Instant),
    Skipped,
    Failed(Instant),
}

struct Slot {
    state: Mutex<State>,
    changed: Condvar,
}

impl Slot {
    /// Whether nobody started the server yet, or the last try is old enough to try again.
    fn needs_start(&self) -> bool {
        let Ok(mut state) = self.state.lock() else { return false };
        let start = match &*state {
            State::Idle => true,
            State::Starting => false,
            State::Ready(client) => !client.alive() && client.started.elapsed() > RETRY_AFTER,
            State::Missing(at) | State::Failed(at) => at.elapsed() > RETRY_AFTER,
            State::Skipped => false,
        };
        if start {
            *state = State::Starting;
        }
        start
    }

    fn start(&self, root: &Path, spec: &'static Spec) {
        let next = match Client::start(root, spec) {
            Ok(client) => State::Ready(client),
            Err(StartError::Missing) => State::Missing(Instant::now()),
            Err(StartError::Skipped) => State::Skipped,
            Err(StartError::Failed) => State::Failed(Instant::now()),
        };
        if let Ok(mut state) = self.state.lock() {
            *state = next;
        }
        self.changed.notify_all();
    }

    /// The running server, waiting for it to start until `deadline`.
    fn wait_ready(&self, deadline: Instant) -> Option<Arc<Client>> {
        let mut state = self.state.lock().ok()?;
        loop {
            match &*state {
                State::Ready(client) => return client.alive().then(|| client.clone()),
                State::Starting => {
                    let left = deadline.checked_duration_since(Instant::now()).filter(|d| !d.is_zero())?;
                    state = self.changed.wait_timeout(state, left).ok()?.0;
                }
                _ => return None,
            }
        }
    }
}

/// The servers by project and server name.
type Registry = Mutex<HashMap<(PathBuf, &'static str), Arc<Slot>>>;

fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(Default::default)
}

fn key(root: &Path, spec: &'static Spec) -> (PathBuf, &'static str) {
    (root.to_path_buf(), spec.name)
}

fn slot(root: &Path, spec: &'static Spec) -> Arc<Slot> {
    let mut registry = registry().lock().unwrap_or_else(|e| e.into_inner());
    registry.entry(key(root, spec)).or_insert_with(|| Arc::new(Slot { state: Mutex::new(State::Idle), changed: Condvar::new() })).clone()
}

// ---------------------------------------------------------------------------------------------
// The connection to one server.

enum StartError {
    Missing,
    Failed,
    Skipped,
}

type Reply = Result<Value, String>;

struct Client {
    child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    pending: Mutex<HashMap<i64, mpsc::Sender<Reply>>>,
    next_id: AtomicI64,
    /// Open documents: their version and a hash of the text the server has.
    open: Mutex<HashMap<String, (i64, u64)>>,
    alive: AtomicBool,
    /// Work the server reported as begun and not yet ended (`$/progress`).
    busy: AtomicU64,
    /// The server counts columns in bytes (`utf-8`) rather than UTF-16 units.
    utf8: AtomicBool,
    /// What the server is told when it asks for its settings (`workspace/configuration`).
    settings: Value,
    started: Instant,
    /// The problems reported per file ([`diagnostics_key`]).
    diagnostics: Mutex<HashMap<String, Vec<Reported>>>,
    /// The server is asked for a file's problems (`textDocument/diagnostic`) instead of telling.
    pull: AtomicBool,
    /// The server fills in an action's edit only when asked (`codeAction/resolve`).
    resolve: AtomicBool,
    /// Edits the server applied while running a command (`workspace/applyEdit`).
    applied: Mutex<Vec<Value>>,
}

impl Client {
    /// The first of the ways to run `spec` here that starts and answers `initialize`.
    fn start(root: &Path, spec: &'static Spec) -> Result<Arc<Client>, StartError> {
        if runs_the_build(root, spec) {
            return Err(StartError::Skipped);
        }
        let path = search_path();
        let launches = launches(root, spec, &path);
        if launches.is_empty() {
            return Err(StartError::Missing);
        }
        launches.into_iter().find_map(|launch| Client::spawn(root, &launch, &path)).ok_or(StartError::Failed)
    }

    fn spawn(root: &Path, launch: &Launch, path: &std::ffi::OsStr) -> Option<Arc<Client>> {
        let mut command = crate::process::command(&launch.program);
        command.args(&launch.args).current_dir(root).env("PATH", path);
        let mut child = command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            return None;
        };
        let client = Arc::new(Client {
            child: Mutex::new(child),
            stdin: Mutex::new(stdin),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicI64::new(1),
            open: Mutex::new(HashMap::new()),
            alive: AtomicBool::new(true),
            busy: AtomicU64::new(0),
            utf8: AtomicBool::new(false),
            settings: launch.settings.clone(),
            started: Instant::now(),
            diagnostics: Mutex::new(HashMap::new()),
            pull: AtomicBool::new(false),
            resolve: AtomicBool::new(false),
            applied: Mutex::new(Vec::new()),
        });
        let reader = Arc::downgrade(&client);
        let _ = std::thread::Builder::new().name("lsp-read".into()).spawn(move || {
            let mut stdout = BufReader::new(stdout);
            while let Some(message) = read_message(&mut stdout) {
                let Some(client) = reader.upgrade() else { return };
                client.receive(message);
            }
            if let Some(client) = reader.upgrade() {
                client.alive.store(false, Ordering::SeqCst);
                client.pending.lock().map(|mut p| p.clear()).ok();
            }
        });
        let Some(result) = client.request("initialize", initialize_params(root, launch.options.clone()), INIT_TIMEOUT) else {
            client.stop();
            return None;
        };
        let encoding = result.pointer("/capabilities/positionEncoding").and_then(Value::as_str);
        client.utf8.store(encoding == Some("utf-8"), Ordering::SeqCst);
        client.pull.store(result.pointer("/capabilities/diagnosticProvider").is_some_and(|p| !p.is_null()), Ordering::SeqCst);
        let resolve = result.pointer("/capabilities/codeActionProvider/resolveProvider").and_then(Value::as_bool);
        client.resolve.store(resolve == Some(true), Ordering::SeqCst);
        client.notify("initialized", json!({}));
        Some(client)
    }

    fn alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    fn indexing(&self) -> bool {
        self.busy.load(Ordering::SeqCst) > 0
    }

    fn stop(&self) {
        if self.alive() {
            let _ = self.request("shutdown", Value::Null, Duration::from_millis(500));
            self.notify("exit", Value::Null);
        }
        self.alive.store(false, Ordering::SeqCst);
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn send(&self, message: &Value) -> bool {
        let body = message.to_string();
        let Ok(mut stdin) = self.stdin.lock() else { return false };
        let sent = write!(stdin, "Content-Length: {}\r\n\r\n{body}", body.len()).and_then(|_| stdin.flush()).is_ok();
        if !sent {
            self.alive.store(false, Ordering::SeqCst);
        }
        sent
    }

    fn notify(&self, method: &str, params: Value) {
        self.send(&json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    /// Sends a request and waits up to `wait` for its result (`None` on an error, a timeout or a
    /// server that went away).
    fn request(&self, method: &str, params: Value, wait: Duration) -> Option<Value> {
        self.ask(method, params, wait)?.ok()
    }

    /// [`Client::request`] with the server's error message when it answers with one.
    fn ask(&self, method: &str, params: Value, wait: Duration) -> Option<Reply> {
        if !self.alive() {
            return None;
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel();
        self.pending.lock().ok()?.insert(id, tx);
        if !self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })) {
            return None;
        }
        let reply = rx.recv_timeout(wait);
        self.pending.lock().ok()?.remove(&id);
        if reply.is_err() && self.alive() {
            self.notify("$/cancelRequest", json!({ "id": id }));
        }
        reply.ok()
    }

    fn receive(self: &Arc<Self>, message: Value) {
        let method = message.get("method").and_then(Value::as_str);
        match (message.get("id"), method) {
            // An answer to one of ours.
            (Some(id), None) => {
                let Some(id) = id.as_i64() else { return };
                let reply = match message.get("error") {
                    Some(error) => Err(error.get("message").and_then(Value::as_str).unwrap_or("error").to_string()),
                    None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
                };
                if let Some(tx) = self.pending.lock().ok().and_then(|mut p| p.remove(&id)) {
                    let _ = tx.send(reply);
                }
            }
            // A request from the server: its settings when it asks for them, otherwise what a
            // client without the feature says. Answered from another thread: this one must keep
            // reading, or a server waiting to write could block our write to it for good.
            (Some(id), Some(method)) => {
                let result = match method {
                    "workspace/configuration" => {
                        let n = message.pointer("/params/items").and_then(Value::as_array).map_or(0, Vec::len);
                        Value::Array(vec![self.settings.clone(); n])
                    }
                    // A command the user chose changes text: kept for `apply`, which makes it.
                    "workspace/applyEdit" => match (message.pointer("/params/edit"), self.applied.lock()) {
                        (Some(edit), Ok(mut applied)) => {
                            applied.push(edit.clone());
                            json!({ "applied": true })
                        }
                        _ => json!({ "applied": false }),
                    },
                    _ => Value::Null,
                };
                let (client, reply) = (self.clone(), json!({ "jsonrpc": "2.0", "id": id, "result": result }));
                let _ = std::thread::Builder::new().name("lsp-reply".into()).spawn(move || client.send(&reply));
            }
            (None, Some("textDocument/publishDiagnostics")) => {
                let Some(path) = message.pointer("/params/uri").and_then(Value::as_str).and_then(uri_path) else { return };
                // Problems of an older text than the server has now are already out of date.
                if let Some(version) = message.pointer("/params/version").and_then(Value::as_i64) {
                    if self.version_of(&path).is_some_and(|current| version < current) {
                        return;
                    }
                }
                let items = message.pointer("/params/diagnostics").and_then(Value::as_array).map(|items| reported(items));
                self.report(&path, items.unwrap_or_default());
            }
            (None, Some("$/progress")) => match message.pointer("/params/value/kind").and_then(Value::as_str) {
                Some("begin") => {
                    self.busy.fetch_add(1, Ordering::SeqCst);
                }
                Some("end") => {
                    let _ = self.busy.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1));
                }
                _ => {}
            },
            _ => {}
        }
    }

    /// Hands the server the editor's text of `path` when it differs from what it has.
    fn sync(&self, path: &Path, language: &str, text: &str) {
        let Some(uri) = file_uri(path) else { return };
        let hash = text_hash(text);
        let Ok(mut open) = self.open.lock() else { return };
        match open.get(&uri).copied() {
            Some((_, h)) if h == hash => {}
            Some((version, _)) => {
                open.insert(uri.clone(), (version + 1, hash));
                drop(open);
                self.notify(
                    "textDocument/didChange",
                    json!({ "textDocument": { "uri": uri, "version": version + 1 }, "contentChanges": [{ "text": text }] }),
                );
            }
            None => {
                open.insert(uri.clone(), (1, hash));
                drop(open);
                self.notify(
                    "textDocument/didOpen",
                    json!({ "textDocument": { "uri": uri, "languageId": language, "version": 1, "text": text } }),
                );
            }
        }
    }

    fn close(&self, path: &Path) {
        let Some(uri) = file_uri(path) else { return };
        if let Ok(mut found) = self.diagnostics.lock() {
            found.remove(&diagnostics_key(path));
        }
        if self.open.lock().ok().and_then(|mut o| o.remove(&uri)).is_some() {
            self.notify("textDocument/didClose", json!({ "textDocument": { "uri": uri } }));
        }
    }

    /// A byte column of `line` as the server counts it.
    /// The version of `path` the server was given last.
    fn version_of(&self, path: &Path) -> Option<i64> {
        let key = diagnostics_key(path);
        let open = self.open.lock().ok()?;
        open.iter().find(|(uri, _)| uri_path(uri).is_some_and(|p| diagnostics_key(&p) == key)).map(|(_, (version, _))| *version)
    }

    fn report(&self, path: &Path, items: Vec<Reported>) {
        let key = diagnostics_key(path);
        let Ok(mut found) = self.diagnostics.lock() else { return };
        if found.get(&key) == Some(&items) {
            return;
        }
        if items.is_empty() {
            found.remove(&key);
        } else {
            found.insert(key, items);
        }
        GENERATION.fetch_add(1, Ordering::SeqCst);
    }

    fn encode_col(&self, line: &str, col: usize) -> usize {
        if self.utf8.load(Ordering::SeqCst) {
            col
        } else {
            utf16_col(line, col)
        }
    }
}

/// How to run a server: the program, its arguments, its `initializationOptions` and the settings
/// it gets when it asks for them.
struct Launch {
    program: PathBuf,
    args: Vec<String>,
    options: Value,
    settings: Value,
}

/// The ways to run `spec` for `root`, best first, among the programs on `path`.
fn launches(root: &Path, spec: &'static Spec, path: &std::ffi::OsStr) -> Vec<Launch> {
    let found = |name: &str| crate::process::which_in(name, path);
    let strings = |args: &[&str]| args.iter().map(|a| a.to_string()).collect::<Vec<_>>();
    let mut out = Vec::new();
    match spec.name {
        // TypeScript 7 has the server built in (`tsc --lsp`); before it, typescript-language-
        // server drives the `tsserver.js` of the project, or of a TypeScript installed beside it.
        // The project's own TypeScript comes first, then what is installed for the user.
        "typescript-language-server" => {
            let local = root.join("node_modules").join(".bin").join(if cfg!(windows) { "tsc.cmd" } else { "tsc" });
            let native = |tsc: Option<PathBuf>| {
                tsc.filter(|tsc| typescript_major(tsc).is_some_and(|v| v >= 7)).map(|program| Launch {
                    program,
                    args: strings(&["--lsp", "--stdio"]),
                    options: Value::Null,
                    settings: Value::Null,
                })
            };
            let own = own_typescript(root);
            out.extend(native(Some(local).filter(|p| own && p.is_file())));
            if let Some(program) = found("typescript-language-server") {
                // Left to itself it runs the project's `tsserver.js`: only the user's own install.
                let options = match (own, typescript_beside(&program)) {
                    (true, _) => Some(Value::Null),
                    (false, Some(tsserver)) => Some(json!({ "tsserver": { "path": tsserver } })),
                    (false, None) => None,
                };
                if let Some(options) = options {
                    out.push(Launch { program, args: strings(&["--stdio"]), options, settings: Value::Null });
                }
            }
            out.extend(native(found("tsc")));
        }
        // Static analysis only: nothing of the project (build scripts, proc macros, cargo check)
        // runs, also after the server asks for its settings again. It watches the files itself,
        // as the editor sends no file events.
        "rust-analyzer" => out.extend(found("rust-analyzer").map(|program| {
            let settings = json!({
                "cargo": { "buildScripts": { "enable": false } },
                "procMacro": { "enable": false },
                "checkOnSave": false,
                "files": { "watcher": "server" },
            });
            Launch { program, args: Vec::new(), options: settings.clone(), settings }
        })),
        // jdtls keeps its own index of each project in a folder of its own. Importing a Gradle
        // project runs its build scripts: off, as is rebuilding when a build file changes.
        "jdtls" => out.extend(found("jdtls").map(|program| {
            let java = json!({
                "import": { "gradle": { "enabled": false, "wrapper": { "enabled": false } } },
                "configuration": { "updateBuildConfiguration": "disabled" },
            });
            Launch {
                program,
                args: vec!["-data".to_string(), jdtls_workspace(root).to_string_lossy().into_owned()],
                options: json!({ "settings": { "java": java.clone() } }),
                settings: java,
            }
        })),
        _ => {
            for (name, args) in spec.commands {
                out.extend(found(name).map(|program| Launch { program, args: strings(args), options: Value::Null, settings: Value::Null }));
            }
        }
    }
    out
}

/// The major version of the TypeScript a `tsc` link belongs to.
fn typescript_major(tsc: &Path) -> Option<u64> {
    let real = std::fs::canonicalize(tsc).ok()?;
    let package = real.ancestors().find(|dir| dir.file_name().is_some_and(|n| n == "typescript"))?.join("package.json");
    let manifest: Value = serde_json::from_str(&std::fs::read_to_string(package).ok()?).ok()?;
    manifest.get("version")?.as_str()?.split('.').next()?.parse().ok()
}

fn initialize_params(root: &Path, options: Value) -> Value {
    let uri = file_uri(root);
    let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    json!({
        "processId": std::process::id(),
        "clientInfo": { "name": "Agentty" },
        "rootUri": uri,
        "rootPath": root.to_string_lossy(),
        "workspaceFolders": [{ "uri": uri, "name": name }],
        "initializationOptions": options,
        "capabilities": {
            "general": { "positionEncodings": ["utf-8", "utf-16"] },
            "window": { "workDoneProgress": true },
            "workspace": {
                "configuration": true,
                "workspaceFolders": true,
                "applyEdit": true,
                "workspaceEdit": { "documentChanges": true },
            },
            "textDocument": {
                "synchronization": { "didSave": false },
                "definition": { "linkSupport": true },
                "references": {},
                "hover": { "contentFormat": ["plaintext", "markdown"] },
                "publishDiagnostics": { "versionSupport": true },
                "diagnostic": { "dynamicRegistration": false },
                "rename": { "prepareSupport": false },
                "codeAction": {
                    "codeActionLiteralSupport": {
                        "codeActionKind": { "valueSet": ["", "quickfix", "refactor", "refactor.extract", "refactor.inline", "refactor.rewrite", "source", "source.organizeImports"] },
                    },
                    "isPreferredSupport": true,
                    "disabledSupport": true,
                    "dataSupport": true,
                    "resolveSupport": { "properties": ["edit"] },
                },
            },
        },
    })
}

/// The `tsserver.js` of a TypeScript (before 7) installed beside `program` (in a `node_modules`
/// above where its link points), when the project has no TypeScript of its own.
fn typescript_beside(program: &Path) -> Option<PathBuf> {
    let real = std::fs::canonicalize(program).ok()?;
    real.ancestors()
        .filter(|dir| dir.file_name().is_some_and(|n| n == "node_modules"))
        .map(|dir| dir.join("typescript").join("lib").join("tsserver.js"))
        .find(|tsserver| tsserver.is_file())
}

/// Whether the project's own TypeScript (`node_modules/typescript`) may run: one the user's
/// package manager installed, which a git repository leaves untracked. A repository that ships
/// `node_modules`, or a folder outside git, could put any program there — opening a file must not
/// run it.
fn own_typescript(root: &Path) -> bool {
    root.join("node_modules").join("typescript").is_dir() && crate::git::tracks(root, "node_modules") == Some(false)
}

/// Servers that read a project by running its build (Gradle's and Maven's scripts and plugins)
/// are not started where there is one: opening a file of a cloned project must not run its code.
/// jdtls is started with its Gradle import off instead (see [`launches`]).
fn runs_the_build(root: &Path, spec: &'static Spec) -> bool {
    const BUILDS: &[&str] = &["build.gradle", "build.gradle.kts", "settings.gradle", "settings.gradle.kts", "pom.xml"];
    spec.name == "kotlin-language-server" && BUILDS.iter().any(|file| root.join(file).exists())
}

fn jdtls_workspace(root: &Path) -> PathBuf {
    let digest = Sha256::digest(root.to_string_lossy().as_bytes());
    let id: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    crate::fsutil::data_dir().join("lsp").join("jdtls").join(id)
}

// ---------------------------------------------------------------------------------------------
// The wire format and what comes over it.

/// One message (`Content-Length` header, blank line, JSON body); `None` at the end of the stream.
fn read_message(reader: &mut impl BufRead) -> Option<Value> {
    loop {
        let mut length = None;
        loop {
            let mut header = String::new();
            if reader.read_line(&mut header).ok()? == 0 {
                return None;
            }
            let header = header.trim_end();
            if header.is_empty() {
                break;
            }
            if let Some((name, value)) = header.split_once(':') {
                if name.eq_ignore_ascii_case("content-length") {
                    length = value.trim().parse::<usize>().ok();
                }
            }
        }
        let Some(length) = length.filter(|n| *n <= MAX_MESSAGE) else {
            continue;
        };
        let mut body = vec![0; length];
        reader.read_exact(&mut body).ok()?;
        if let Ok(value) = serde_json::from_slice(&body) {
            return Some(value);
        }
    }
}

fn file_uri(path: &Path) -> Option<String> {
    url::Url::from_file_path(path).ok().map(String::from)
}

fn uri_path(uri: &str) -> Option<PathBuf> {
    url::Url::parse(uri).ok()?.to_file_path().ok()
}

/// The key of a file's problems: its path, without case where the file system ignores it (a
/// server may send `/users/…` for `/Users/…`).
fn diagnostics_key(path: &Path) -> String {
    let text = path.to_string_lossy();
    if cfg!(any(target_os = "macos", windows)) {
        text.to_lowercase()
    } else {
        text.into_owned()
    }
}

/// The problems in an LSP `Diagnostic[]`.
fn reported(items: &[Value]) -> Vec<Reported> {
    let at = |item: &Value, key: &str| {
        let pos = item.pointer(&format!("/range/{key}"))?;
        Some((pos.get("line")?.as_u64()? as usize, pos.get("character")?.as_u64()? as usize))
    };
    let text = |value: Option<&Value>| match value {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    };
    items
        .iter()
        .filter_map(|item| {
            let severity = match item.get("severity").and_then(Value::as_u64) {
                Some(2) => Severity::Warning,
                Some(3) => Severity::Information,
                Some(4) => Severity::Hint,
                _ => Severity::Error,
            };
            Some(Reported {
                start: at(item, "start")?,
                end: at(item, "end")?,
                severity,
                message: item.get("message")?.as_str()?.trim_end().to_string(),
                source: text(item.get("source")),
                code: text(item.get("code")),
                raw: item.clone(),
            })
        })
        .take(MAX_DIAGNOSTICS)
        .collect()
}

fn text_hash(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// UTF-16 units before byte `col` of `line`.
fn utf16_col(line: &str, col: usize) -> usize {
    let col = col.min(line.len());
    line.char_indices().take_while(|(i, _)| *i < col).map(|(_, c)| c.len_utf16()).sum()
}

/// The byte column of `line` that `units` UTF-16 units reach.
fn byte_col(line: &str, units: usize) -> usize {
    let mut seen = 0;
    for (i, c) in line.char_indices() {
        if seen >= units {
            return i;
        }
        seen += c.len_utf16();
    }
    line.len()
}

/// `Location`, `Location[]` or `LocationLink[]` as places, with columns in bytes (`utf8`: the
/// server counted bytes already). Lines are read from `text` for `path` itself and from disk for
/// other files.
fn places(value: &Value, path: &Path, text: &str, utf8: bool) -> Vec<Place> {
    let items: Vec<&Value> = match value {
        Value::Array(items) => items.iter().collect(),
        Value::Object(_) => vec![value],
        _ => Vec::new(),
    };
    let mut files: HashMap<PathBuf, Option<String>> = HashMap::new();
    let mut out = Vec::new();
    for item in items.into_iter().take(MAX_PLACES) {
        let uri = item.get("targetUri").or_else(|| item.get("uri")).and_then(Value::as_str);
        let range = item.get("targetSelectionRange").or_else(|| item.get("range"));
        let (Some(target), Some(range)) = (uri.and_then(uri_path), range) else { continue };
        let at = |key: &str| {
            let point = range.get(key)?;
            Some((point.get("line")?.as_u64()? as usize, point.get("character")?.as_u64()? as usize))
        };
        let (Some((line, start)), Some((end_line, end))) = (at("start"), at("end")) else { continue };
        let source = files.entry(target.clone()).or_insert_with(|| {
            if target == path {
                Some(text.to_string())
            } else {
                std::fs::read_to_string(&target).ok()
            }
        });
        let row = source.as_deref().and_then(|s| s.lines().nth(line)).unwrap_or("");
        let (col, end_col) = if utf8 { (start.min(row.len()), end.min(row.len())) } else { (byte_col(row, start), byte_col(row, end)) };
        let len = if end_line == line { end_col.saturating_sub(col) } else { 0 };
        let place = Place { path: target, line, col, len };
        if !out.contains(&place) {
            out.push(place);
        }
    }
    out
}

/// A hover result (`MarkupContent`, `MarkedString` or a list of them) as plain text: code fences
/// dropped, their contents kept.
fn hover_text(value: &Value) -> String {
    fn part(value: &Value) -> String {
        match value {
            Value::String(s) => s.clone(),
            Value::Object(o) => o.get("value").and_then(Value::as_str).unwrap_or_default().to_string(),
            Value::Array(items) => items.iter().map(part).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n\n"),
            _ => String::new(),
        }
    }
    let raw = value.get("contents").map(part).unwrap_or_default();
    let lines: Vec<&str> = raw.lines().filter(|l| !l.trim_start().starts_with("```")).collect();
    let mut text = lines.join("\n");
    while text.contains("\n\n\n") {
        text = text.replace("\n\n\n", "\n\n");
    }
    text.trim().to_string()
}

/// Reads one message from bytes (for tests).
#[cfg(test)]
fn read_one(bytes: &[u8]) -> Option<Value> {
    read_message(&mut BufReader::new(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_framed_messages() {
        let body = r#"{"jsonrpc":"2.0","id":1,"result":null}"#;
        let framed = format!("Content-Length: {}\r\nContent-Type: application/vscode-jsonrpc\r\n\r\n{body}", body.len());
        assert_eq!(read_one(framed.as_bytes()).unwrap()["id"], 1);
        assert_eq!(read_one(b""), None);
        // A message without a length is skipped, the next one read.
        let two = format!("X: y\r\n\r\nContent-Length: {}\r\n\r\n{body}", body.len());
        assert_eq!(read_one(two.as_bytes()).unwrap()["id"], 1);
    }

    #[test]
    fn converts_columns_between_bytes_and_utf16() {
        let line = "let é = \"😀\"; x";
        let x = line.find('x').unwrap();
        let units = utf16_col(line, x);
        assert_eq!(units, line.encode_utf16().count() - 1);
        assert_eq!(byte_col(line, units), x);
        assert_eq!(utf16_col("abc", 10), 3);
        assert_eq!(byte_col("abc", 10), 3);
    }

    #[test]
    fn reads_locations_and_links() {
        let dir = std::env::temp_dir().join(format!("agentty-lsp-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let other = dir.join("other.rs");
        std::fs::write(&other, "fn one() {}\nfn twö() {}\n").unwrap();
        let here = dir.join("here.rs");
        let text = "use other::twö;\n";
        let range = |l, s, e| json!({ "start": { "line": l, "character": s }, "end": { "line": l, "character": e } });
        let links = json!([
            { "targetUri": file_uri(&other).unwrap(), "targetRange": range(1, 0, 11), "targetSelectionRange": range(1, 3, 6) },
            { "uri": file_uri(&here).unwrap(), "range": range(0, 11, 14) },
        ]);
        let found = places(&links, &here, text, false);
        assert_eq!(found[0], Place { path: other.clone(), line: 1, col: 3, len: "twö".len() });
        assert_eq!(found[1], Place { path: here.clone(), line: 0, col: 11, len: "twö".len() });
        let single = json!({ "uri": file_uri(&other).unwrap(), "range": range(0, 3, 6) });
        assert_eq!(places(&single, &here, text, false).len(), 1);
        assert!(places(&Value::Null, &here, text, true).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hover_text_is_plain() {
        let markup = json!({ "contents": { "kind": "markdown", "value": "```rust\nfn save(&self)\n```\n\n\n\nSaves it." } });
        assert_eq!(hover_text(&markup), "fn save(&self)\n\nSaves it.");
        let marked = json!({ "contents": [{ "language": "python", "value": "def f() -> int" }, "Docs"] });
        assert_eq!(hover_text(&marked), "def f() -> int\n\nDocs");
        assert_eq!(hover_text(&Value::Null), "");
    }

    #[cfg(unix)]
    #[test]
    fn a_project_on_typescript_7_is_served_by_its_own_tsc() {
        let root = std::env::temp_dir().join(format!("agentty-lsp-ts-{}", std::process::id()));
        let package = root.join("node_modules").join("typescript");
        std::fs::create_dir_all(package.join("bin")).unwrap();
        std::fs::create_dir_all(root.join("node_modules").join(".bin")).unwrap();
        std::fs::write(package.join("bin").join("tsc"), "").unwrap();
        std::fs::write(package.join("package.json"), r#"{ "version": "7.0.2" }"#).unwrap();
        let link = root.join("node_modules").join(".bin").join("tsc");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink("../typescript/bin/tsc", &link).unwrap();
        assert_eq!(typescript_major(&link), Some(7));
        let typescript = || launches(&root, spec_for("typescript").unwrap(), std::ffi::OsStr::new(""));
        // Outside git nothing says who put it there: not run.
        assert!(typescript().is_empty());
        let git = |args: &[&str]| std::process::Command::new("git").arg("-C").arg(&root).args(args).output().unwrap();
        git(&["init", "-q"]);
        let found = typescript();
        assert_eq!(found[0].program, link);
        assert_eq!(found[0].args, ["--lsp", "--stdio"]);
        // A repository that ships node_modules: not run either.
        git(&["add", "-f", "node_modules/typescript/package.json"]);
        assert!(typescript().is_empty());
        git(&["rm", "-q", "--cached", "node_modules/typescript/package.json"]);
        std::fs::write(package.join("package.json"), r#"{ "version": "5.9.3" }"#).unwrap();
        assert!(typescript().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn kotlin_is_not_started_where_it_would_run_the_build() {
        let root = std::env::temp_dir().join(format!("agentty-lsp-kt-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let kotlin = spec_for("kotlin").unwrap();
        assert!(!runs_the_build(&root, kotlin));
        std::fs::write(root.join("build.gradle.kts"), "").unwrap();
        assert!(runs_the_build(&root, kotlin));
        assert!(!runs_the_build(&root, spec_for("java").unwrap()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn finds_servers_by_language() {
        assert_eq!(spec_for("typescriptreact").map(|s| s.name), Some("typescript-language-server"));
        assert_eq!(spec_for("python").map(|s| s.name), Some("pyright"));
        assert!(spec_for("markdown").is_none());
        assert_eq!(status(Path::new("/nowhere"), "markdown"), Status::Missing);
    }

    #[test]
    fn reads_problems() {
        let items = json!([
            { "range": { "start": { "line": 4, "character": 6 }, "end": { "line": 4, "character": 11 } }, "severity": 1, "code": 2322, "source": "ts", "message": "Type 'string' is not assignable.\n" },
            { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 3 } }, "severity": 2, "message": "unused" },
            { "range": { "start": { "line": 1 } }, "message": "broken" },
        ]);
        let found = reported(items.as_array().unwrap());
        assert_eq!(found.len(), 2);
        assert_eq!((found[0].start, found[0].end, found[0].severity), ((4, 6), (4, 11), Severity::Error));
        assert_eq!((found[0].code.as_deref(), found[0].source.as_deref()), (Some("2322"), Some("ts")));
        assert_eq!(found[0].message, "Type 'string' is not assignable.");
        assert_eq!(found[1].severity, Severity::Warning);
        if cfg!(target_os = "macos") {
            assert_eq!(diagnostics_key(Path::new("/Users/a/x.ts")), diagnostics_key(Path::new("/users/a/x.ts")));
        }
    }

    #[test]
    fn reads_actions_and_applies_their_edits() {
        let answer = json!([
            { "title": "Extract function", "kind": "refactor.extract", "edit": { "changes": {} } },
            { "title": "Add import from \"./bad\"", "kind": "quickfix", "isPreferred": true, "diagnostics": [{}],
              "edit": { "changes": { "file:///p/src/uses.ts": [{ "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } }, "newText": "import { total } from \"./bad\";\n" }] } } },
            { "title": "Disabled", "disabled": { "reason": "no" } },
            { "title": "Move to a new file", "edit": { "documentChanges": [{ "kind": "create", "uri": "file:///p/new.ts" }] } },
            { "title": "Run it", "command": "do.it", "arguments": [1] },
        ]);
        let mut found: Vec<Action> = answer.as_array().unwrap().iter().filter_map(action).collect();
        found.sort_by_key(|a| (!a.fixes, !a.preferred));
        assert_eq!(found.iter().map(|a| a.title.as_str()).collect::<Vec<_>>(), ["Add import from \"./bad\"", "Extract function", "Run it"]);
        let edits = file_edits(found[0].value.get("edit").unwrap(), false).unwrap();
        assert_eq!(edits[0].path, PathBuf::from("/p/src/uses.ts"));
        assert_eq!(edited("const a = total();\n", &edits[0]), "import { total } from \"./bad\";\nconst a = total();\n");
        assert!(file_edits(&json!({ "documentChanges": [{ "kind": "delete", "uri": "file:///p/a" }] }), false).is_none());
    }

    #[test]
    fn edits_count_columns_as_the_server_does() {
        let edit = |utf8, start, end, text: &str| FileEdit {
            path: PathBuf::from("/p/a"),
            edits: vec![TextEdit { start, end, text: text.to_string() }],
            utf8,
        };
        // "é" is one UTF-16 unit and two bytes.
        assert_eq!(edited("é = x\r\nb\n", &edit(false, (0, 4), (0, 5), "y")), "é = y\r\nb\n");
        assert_eq!(edited("é = x\nb\n", &edit(true, (0, 5), (0, 6), "y")), "é = y\nb\n");
        assert_eq!(edited("a\nb", &edit(false, (1, 0), (1, 1), "c")), "a\nc");
        // Edits apply against the original text, in any order; overlapping ones are skipped.
        let two = FileEdit {
            path: PathBuf::from("/p/a"),
            edits: vec![
                TextEdit { start: (1, 0), end: (1, 1), text: "B".into() },
                TextEdit { start: (0, 0), end: (0, 1), text: "A".into() },
                TextEdit { start: (0, 0), end: (0, 1), text: "Z".into() },
            ],
            utf8: false,
        };
        assert_eq!(edited("a\nb", &two), "A\nB");
    }

    #[test]
    fn every_server_has_a_page_and_a_way_in() {
        let empty = std::ffi::OsStr::new("");
        for spec in SPECS {
            assert!(site(spec).starts_with("https://"), "{}", spec.name);
            assert!(!shown_languages(spec).is_empty(), "{}", spec.name);
            // Nothing on PATH: no command, but the tool it needs is named.
            let (command, needs) = install_command(spec, empty);
            assert!(command.is_none() && needs.is_some(), "{}", spec.name);
            assert!(!installed(spec, empty), "{}", spec.name);
        }
    }
}
