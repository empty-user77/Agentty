//! The editor's side of the language servers (`agentty_bridge::lsp`): which server serves a
//! language, how long an answer may take before the name-based index answers instead, and the
//! servers' places as the editor's own `Location`s and `Usage`s. Everything here blocks; it is
//! called off the UI thread.

use super::buffer::{Buffer, Pos};
use super::language::Language;
use super::project_index::{Location, Usage};
use super::symbols::{self, Kind};
use agentty_bridge::lsp::{self, Place, Status};
pub use agentty_bridge::lsp::{Action, CompletionItem, FileEdit, Severity, Signature};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A file's text as the editor had it when a question about it was asked, with its revision (see
/// [`lsp::Text`]). Reads as the text itself.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub text: String,
    pub revision: u64,
}

impl Snapshot {
    pub fn of(buffer: &Buffer) -> Self {
        Self { text: buffer.text(), revision: buffer.revision() }
    }

    fn lsp(&self) -> lsp::Text<'_> {
        lsp::Text { text: &self.text, revision: self.revision }
    }
}

impl std::ops::Deref for Snapshot {
    type Target = str;
    fn deref(&self) -> &str {
        &self.text
    }
}

/// The LSP language id of `language`, when a server is known for it.
pub fn id(language: Language) -> Option<&'static str> {
    Some(match language {
        Language::Rust => "rust",
        Language::Go => "go",
        Language::TypeScript => "typescript",
        Language::Tsx => "typescriptreact",
        Language::JavaScript => "javascript",
        Language::Jsx => "javascriptreact",
        Language::Python => "python",
        Language::Java => "java",
        Language::Kotlin => "kotlin",
        _ => return None,
    })
}

/// Starts the server for a file of `language` in `project`, so it has read the project by the
/// time it is asked.
pub fn warm(project: &Path, language: Language) {
    if let Some(id) = id(language) {
        if !project.as_os_str().is_empty() {
            lsp::warm(project, id);
        }
    }
}

pub fn close(project: &Path, language: Language, path: &Path) {
    if let Some(id) = id(language) {
        lsp::close(project, id, path);
    }
}

/// How long handing a file over for its problems waits for a server still starting (jdtls takes
/// a while on a cold start).
const CHECK_WAIT: Duration = Duration::from_secs(90);

/// How long a server gets to format a file.
const FORMAT_WAIT: Duration = Duration::from_secs(10);

/// How long a chosen fix may take (a server running a command first).
const APPLY_WAIT: Duration = Duration::from_secs(10);

/// How long to wait for the server before the index answers: a running server gets time for a
/// real answer, one still starting or reading the project only a moment.
fn patience(project: &Path, id: &str) -> Duration {
    match lsp::status(project, id) {
        Status::Ready => Duration::from_secs(3),
        _ => Duration::from_millis(1500),
    }
}

/// What the server for `language` is doing, for the debug state.
pub fn status(project: &Path, language: Language) -> &'static str {
    match id(language).map(|id| lsp::status(project, id)) {
        None | Some(Status::Missing) => "missing",
        Some(Status::Starting) => "starting",
        Some(Status::Indexing) => "indexing",
        Some(Status::Ready) => "ready",
        Some(Status::Failed) => "failed",
        Some(Status::Skipped) => "skipped",
    }
}

/// Where the name at `at` of `path` is defined, as the server says (`None`: no server answered,
/// or it found nothing).
pub fn definitions(project: &Path, language: Language, path: &Path, text: &Snapshot, at: Pos) -> Option<Vec<Location>> {
    let id = id(language)?;
    let places = lsp::definition(project, id, path, text.lsp(), at.line, at.col, patience(project, id))?;
    let mut files = Files::new(path, text);
    let found: Vec<Location> = places.iter().filter_map(|p| files.location(project, p)).collect();
    (!found.is_empty()).then_some(found)
}

/// Where the name at `at` of `path` is implemented, as the server says (`None`: no server
/// answered, or it found nothing).
pub fn implementations(project: &Path, language: Language, path: &Path, text: &Snapshot, at: Pos) -> Option<Vec<Location>> {
    let id = id(language)?;
    let places = lsp::implementation(project, id, path, text.lsp(), at.line, at.col, patience(project, id))?;
    let mut files = Files::new(path, text);
    let found: Vec<Location> = places.iter().filter_map(|p| files.location(project, p)).collect();
    (!found.is_empty()).then_some(found)
}

/// Every place the name at `at` of `path` is used, as the server says.
pub fn usages(project: &Path, language: Language, path: &Path, text: &Snapshot, at: Pos) -> Option<Vec<Usage>> {
    let id = id(language)?;
    let places = lsp::references(project, id, path, text.lsp(), at.line, at.col, patience(project, id))?;
    let mut files = Files::new(path, text);
    let mut found: Vec<Usage> = places.iter().filter_map(|p| files.usage(project, p)).collect();
    found.sort_by(|a, b| (&a.rel, a.line, a.col).cmp(&(&b.rel, b.line, b.col)));
    (!found.is_empty()).then_some(found)
}

/// The server's text for the name at `at` (its type or signature first), when it has one.
pub fn hover(project: &Path, language: Language, path: &Path, text: &Snapshot, at: Pos) -> Option<String> {
    let id = id(language)?;
    let text = lsp::hover(project, id, path, text.lsp(), at.line, at.col, patience(project, id))?;
    (!text.is_empty()).then_some(text)
}

/// A problem the server reported, at places in the editor's text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub start: Pos,
    pub end: Pos,
    pub severity: Severity,
    pub message: String,
    /// Who found it and its code, as shown under the message (`ts(2322)`).
    pub origin: Option<String>,
}

/// Hands the editor's text of `path` to its server, which reports the problems in it (waits for a
/// server still starting).
pub fn check(project: &Path, language: Language, path: &Path, text: &Snapshot) {
    if let Some(id) = id(language) {
        lsp::check(project, id, path, text.lsp(), CHECK_WAIT);
    }
}

/// The problems last reported in `path`, in the places of `buffer` as it is now. Hints (unused
/// names and the like, which servers send by the dozen) are left out.
pub fn problems(project: &Path, language: Language, path: &Path, buffer: &Buffer) -> Vec<Problem> {
    let Some(id) = id(language) else { return Vec::new() };
    let line = |i: usize| (i < buffer.line_count()).then(|| buffer.line(i));
    lsp::diagnostics(project, id, path, line)
        .into_iter()
        .filter(|d| d.severity != Severity::Hint)
        .map(|d| Problem {
            start: Pos::new(d.start.0, d.start.1),
            end: Pos::new(d.end.0, d.end.1),
            severity: d.severity,
            message: d.message,
            origin: match (d.source, d.code) {
                (Some(source), Some(code)) => Some(format!("{source}({code})")),
                (source, code) => source.or(code),
            },
        })
        .collect()
}

/// What the server offers for `start..end` of `path`: fixes first.
pub fn actions(project: &Path, language: Language, path: &Path, text: &Snapshot, start: Pos, end: Pos) -> Option<Vec<Action>> {
    let id = id(language)?;
    lsp::actions(project, id, path, text.lsp(), (start.line, start.col), (end.line, end.col), patience(project, id))
}

/// The changes `action` makes (`None`: the server did not say in time, or asked for more than
/// text changes).
pub fn apply(project: &Path, language: Language, action: &Action) -> Option<Vec<FileEdit>> {
    lsp::apply(project, id(language)?, action, APPLY_WAIT)
}

/// What renaming the name at `at` to `new_name` changes in every file (`Err`: the server's
/// reason for not doing it; `None`: no server answered).
pub fn rename(
    project: &Path,
    language: Language,
    path: &Path,
    text: &Snapshot,
    at: Pos,
    new_name: &str,
) -> Option<Result<Vec<FileEdit>, String>> {
    let id = id(language)?;
    lsp::rename(project, id, path, text.lsp(), at.line, at.col, new_name, APPLY_WAIT)
}

/// The number of changes in `edit`.
pub fn edit_count(edit: &FileEdit) -> usize {
    lsp::edit_count(edit)
}

/// What the server offers at `line`/`col` of `path` (`None`: no server answered).
#[allow(clippy::too_many_arguments)]
pub fn completions(
    project: &Path,
    language: Language,
    path: &Path,
    text: &Snapshot,
    line: usize,
    col: usize,
    trigger: Option<char>,
    wait: Duration,
) -> Option<(Vec<CompletionItem>, bool)> {
    lsp::completions(project, id(language)?, path, text.lsp(), line, col, trigger, wait)
}

/// The signature of the call at `at` (`None`: no call there, or no server answered).
pub fn signature_help(
    project: &Path,
    language: Language,
    path: &Path,
    text: &Snapshot,
    at: Pos,
    trigger: Option<char>,
    wait: Duration,
) -> Option<Signature> {
    lsp::signature_help(project, id(language)?, path, text.lsp(), at.line, at.col, trigger, wait)
}

/// `item` with its import, when the server adds it on request.
pub fn resolve_completion(project: &Path, language: Language, item: &CompletionItem, wait: Duration) -> CompletionItem {
    match id(language) {
        Some(id) => lsp::resolve_completion(project, id, item, wait),
        None => item.clone(),
    }
}

/// The change accepting `item` makes over `word` (bytes of `text`).
pub fn completion_edit(path: &Path, text: &str, item: &CompletionItem, word: ((usize, usize), (usize, usize))) -> FileEdit {
    lsp::completion_edit(path, text, item, word)
}

/// A plain change, in bytes.
pub fn byte_edit(path: &Path, start: (usize, usize), end: (usize, usize), text: &str) -> FileEdit {
    lsp::byte_edit(path, start, end, text)
}

/// `text` with `edit` made, and where byte `mark` of it went.
pub fn edited_at(text: &str, edit: &FileEdit, mark: usize) -> (String, usize) {
    lsp::edited_at(text, edit, mark)
}

/// The running server's formatting of the text, indented with `indent` (`None`: no server, it
/// does not format, or it did not answer).
pub fn format(project: &Path, language: Language, path: &Path, text: &Snapshot, indent: &str) -> Option<String> {
    let (tab, spaces) = if indent == "\t" { (4, false) } else { (indent.len().max(1), true) };
    let edit = lsp::format(project, id(language)?, path, text.lsp(), tab, spaces, FORMAT_WAIT)?;
    Some(match edit {
        Some(edit) => lsp::edited(text, &edit),
        None => text.text.clone(),
    })
}

/// The program a language's server runs as, for notes ("gopls", "rust-analyzer").
pub fn server_name(language: Language) -> Option<&'static str> {
    lsp::spec_for(id(language)?).map(|spec| spec.program())
}

/// `text` with `edit` made.
pub fn edited(text: &str, edit: &FileEdit) -> String {
    lsp::edited(text, edit)
}

/// Goes up whenever a server reports problems.
pub fn generation() -> u64 {
    lsp::generation()
}

/// The files places point into, each read once (the editor's text for the file asked about).
struct Files {
    texts: HashMap<PathBuf, Option<String>>,
}

impl Files {
    fn new(path: &Path, text: &str) -> Self {
        Self { texts: HashMap::from([(path.to_path_buf(), Some(text.to_string()))]) }
    }

    fn line(&mut self, place: &Place) -> Option<String> {
        let text = self.texts.entry(place.path.clone()).or_insert_with(|| std::fs::read_to_string(&place.path).ok());
        text.as_deref()?.lines().nth(place.line).map(str::to_string)
    }

    fn location(&mut self, project: &Path, place: &Place) -> Option<Location> {
        let line = self.line(place)?;
        let name = line.get(place.col..place.col + place.len).filter(|n| !n.is_empty()).unwrap_or(line.trim()).to_string();
        // The kind the editor's own reading of that file gives the name there, if any.
        let language = Language::detect(&place.path);
        let kind = self
            .texts
            .get(&place.path)
            .and_then(|t| t.as_deref())
            .and_then(|text| symbols::symbols(language, text).into_iter().find(|s| s.line == place.line && s.col == place.col))
            .map_or(Kind::Variable, |s| s.kind);
        Some(Location { path: place.path.clone(), rel: relative(project, &place.path), line: place.line, col: place.col, name, kind })
    }

    fn usage(&mut self, project: &Path, place: &Place) -> Option<Usage> {
        let line = self.line(place)?;
        let text = line.trim().chars().take(160).collect();
        Some(Usage { path: place.path.clone(), rel: relative(project, &place.path), line: place.line, col: place.col, text })
    }
}

fn relative(project: &Path, path: &Path) -> String {
    path.strip_prefix(project).unwrap_or(path).to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_server_languages() {
        assert_eq!(id(Language::Tsx), Some("typescriptreact"));
        assert_eq!(id(Language::Kotlin), Some("kotlin"));
        assert_eq!(id(Language::Markdown), None);
        assert_eq!(status(Path::new("/nowhere"), Language::Css), "missing");
    }

    #[test]
    fn places_become_locations_and_usages() {
        let path = PathBuf::from("/p/src/a.rs");
        let text = "fn save() {}\nfn main() { save(); }\n";
        let mut files = Files::new(&path, text);
        let at = |line, col, len| Place { path: path.clone(), line, col, len };
        let def = files.location(Path::new("/p"), &at(0, 3, 4)).unwrap();
        assert_eq!((def.rel.as_str(), def.name.as_str(), def.kind), ("src/a.rs", "save", Kind::Function));
        let used = files.usage(Path::new("/p"), &at(1, 12, 4)).unwrap();
        assert_eq!((used.line, used.col, used.text.as_str()), (1, 12, "fn main() { save(); }"));
        assert!(files.location(Path::new("/p"), &at(9, 0, 1)).is_none());
    }
}
