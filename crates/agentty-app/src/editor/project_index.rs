//! What the editor knows about a project as a whole: its files (git's list when it is a
//! repository, so `.gitignore` is respected; otherwise a walk that leaves out dependency and build
//! folders), the symbols defined in them (`symbols.rs`), kept per file and re-read only when a
//! file changed, and the text in them (⌘⇧F). Everything here reads files and runs `git ls-files`;
//! it is called off the UI thread.

use super::language::Language;
use super::symbols::{self, Kind, Symbol};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

/// Files listed at most: past this a folder is a home folder or a disk, not a project.
const MAX_FILES: usize = 60_000;
/// Files read for symbols at most, and the largest read (generated bundles say nothing useful).
const MAX_PARSED: usize = 25_000;
const MAX_PARSE_BYTES: u64 = 768 * 1024;
/// A file list younger than this is used as it is.
const LIST_FRESH: Duration = Duration::from_secs(8);
/// Usages found at most.
pub const MAX_USAGES: usize = 2_000;
/// The largest file a text search reads (past this it is data or a bundle, not something written).
const MAX_SEARCH_BYTES: u64 = 2 * 1024 * 1024;
/// Characters of a found line shown, and how many of them before the match when the line is long.
const SHOWN_CHARS: usize = 160;
const SHOWN_BEFORE: usize = 40;
/// Folders never walked into when the project is not a repository.
const SKIPPED: &[&str] = &[
    "node_modules",
    "target",
    "build",
    "dist",
    "out",
    ".git",
    ".gradle",
    ".idea",
    ".vscode",
    ".venv",
    "venv",
    "__pycache__",
    ".next",
    ".nuxt",
    ".svelte-kit",
    ".turbo",
    ".cache",
    "coverage",
    "Pods",
    "DerivedData",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub path: PathBuf,
    /// Path inside the project, `/`-separated (what the lists show).
    pub rel: String,
    pub line: usize,
    pub col: usize,
    pub name: String,
    pub kind: Kind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Usage {
    pub path: PathBuf,
    pub rel: String,
    pub line: usize,
    pub col: usize,
    /// The line, trimmed and cut short, to show in the list.
    pub text: String,
}

/// A file's modification time and size, when it could be read.
type Stamp = Option<(SystemTime, u64)>;

struct FileSymbols {
    stamp: Stamp,
    symbols: Arc<Vec<Symbol>>,
}

#[derive(Default)]
struct Project {
    files: Arc<Vec<String>>,
    listed: Option<Instant>,
    symbols: HashMap<String, FileSymbols>,
}

fn projects() -> &'static Mutex<HashMap<PathBuf, Project>> {
    static PROJECTS: OnceLock<Mutex<HashMap<PathBuf, Project>>> = OnceLock::new();
    PROJECTS.get_or_init(Default::default)
}

/// The project's files, relative and `/`-separated, sorted.
pub fn files(root: &Path) -> Arc<Vec<String>> {
    if let Some(project) = projects()
        .lock()
        .ok()
        .and_then(|p| p.get(root).filter(|p| p.listed.is_some_and(|at| at.elapsed() < LIST_FRESH)).map(|p| p.files.clone()))
    {
        return project;
    }
    let mut list = agentty_bridge::git::project_files(root).unwrap_or_else(|| walk(root));
    list.retain(|rel| !rel.ends_with('/'));
    list.truncate(MAX_FILES);
    list.sort();
    let list = Arc::new(list);
    if let Ok(mut projects) = projects().lock() {
        let project = projects.entry(root.to_path_buf()).or_default();
        project.files = list.clone();
        project.listed = Some(Instant::now());
    }
    list
}

fn walk(root: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut folders = vec![root.to_path_buf()];
    while let Some(folder) = folders.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() {
                if !name.starts_with('.') && !SKIPPED.contains(&name.as_str()) {
                    folders.push(entry.path());
                }
            } else if kind.is_file() && name != ".DS_Store" {
                if let Ok(rel) = entry.path().strip_prefix(root) {
                    found.push(rel.to_string_lossy().replace('\\', "/"));
                }
            }
            if found.len() >= MAX_FILES {
                return found;
            }
        }
    }
    found
}

fn stamp(path: &Path) -> Stamp {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// Files whose language has symbols, as (relative path, language).
fn code_files(root: &Path) -> Vec<(String, Language)> {
    files(root)
        .iter()
        .filter_map(|rel| {
            let language = Language::detect(Path::new(rel));
            symbols::supported(language).then(|| (rel.clone(), language))
        })
        .take(MAX_PARSED)
        .collect()
}

/// Every symbol of the project, per file, read again where a file changed since last time.
fn all_symbols(root: &Path) -> Vec<(String, Arc<Vec<Symbol>>)> {
    let files = code_files(root);
    let known: HashMap<String, (Stamp, Arc<Vec<Symbol>>)> = projects()
        .lock()
        .ok()
        .and_then(|p| p.get(root).map(|p| p.symbols.iter().map(|(k, v)| (k.clone(), (v.stamp, v.symbols.clone()))).collect()))
        .unwrap_or_default();
    let mut fresh = HashMap::new();
    let mut out = Vec::with_capacity(files.len());
    for (rel, language) in files {
        let path = root.join(&rel);
        let now = stamp(&path);
        let symbols = match known.get(&rel) {
            Some((was, symbols)) if *was == now => symbols.clone(),
            _ => {
                let text = match now {
                    Some((_, len)) if len <= MAX_PARSE_BYTES => std::fs::read_to_string(&path).unwrap_or_default(),
                    _ => String::new(),
                };
                Arc::new(symbols::symbols(language, &text))
            }
        };
        fresh.insert(rel.clone(), FileSymbols { stamp: now, symbols: symbols.clone() });
        out.push((rel, symbols));
    }
    if let Ok(mut projects) = projects().lock() {
        projects.entry(root.to_path_buf()).or_default().symbols = fresh;
    }
    out
}

/// Where `name` is defined in the project, best first for a reader in `from` (relative path).
/// Definitions in the reader's family of languages only, when there are any: in a repository
/// with a front end and a back end, a Kotlin `Order` is not the TypeScript `Order`.
pub fn definitions(root: &Path, name: &str, from: &str) -> Vec<Location> {
    let all = all_symbols(root);
    // An `impl` names a type, it does not define it.
    let mut found: Vec<(&str, &Symbol)> = all
        .iter()
        .flat_map(|(rel, list)| list.iter().filter(|s| s.name == name && s.kind != Kind::Impl).map(move |s| (rel.as_str(), s)))
        .collect();
    let reader = Language::detect(Path::new(from));
    if found.iter().any(|(rel, _)| same_family(reader, Language::detect(Path::new(rel)))) {
        found.retain(|(rel, _)| same_family(reader, Language::detect(Path::new(rel))));
    }
    symbols::rank(&mut found, from);
    found.into_iter().map(|(rel, s)| location(root, rel, s)).collect()
}

/// The names defined in the project's files of `language`'s family (classes, functions, fields,
/// …), each once with its kind, at most `limit`: what a file can use without a server to ask.
pub fn names(root: &Path, language: Language, limit: usize) -> Vec<(String, Kind)> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for (rel, list) in all_symbols(root) {
        if !same_family(language, Language::detect(Path::new(&rel))) {
            continue;
        }
        for symbol in list.iter().filter(|s| s.kind != Kind::Impl) {
            if out.len() >= limit {
                return out;
            }
            if seen.insert(symbol.name.clone()) {
                out.push((symbol.name.clone(), symbol.kind));
            }
        }
    }
    out
}

/// Symbols of the whole project whose name matches `query` (fuzzy), best first, at most `limit`.
pub fn search_symbols(root: &Path, query: &str, limit: usize) -> Vec<Location> {
    let all = all_symbols(root);
    let mut scored: Vec<(i64, &str, &Symbol)> = all
        .iter()
        .flat_map(|(rel, list)| list.iter().filter(|s| s.kind != Kind::Impl).map(move |s| (rel.as_str(), s)))
        .filter_map(|(rel, s)| symbols::fuzzy_score(&s.name, query).map(|score| (score, rel, s)))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.2.name.len().cmp(&b.2.name.len())).then_with(|| a.1.cmp(b.1)));
    scored.into_iter().take(limit).map(|(_, rel, s)| location(root, rel, s)).collect()
}

fn location(root: &Path, rel: &str, symbol: &Symbol) -> Location {
    Location {
        path: root.join(rel),
        rel: rel.to_string(),
        line: symbol.line,
        col: symbol.col,
        name: symbol.name.clone(),
        kind: symbol.kind,
    }
}

/// Languages whose files use each other's names: the web's (markup, styles, scripts) and the
/// JVM's (Java and Kotlin call each other).
fn same_family(a: Language, b: Language) -> bool {
    use Language::*;
    let web = |l: Language| matches!(l, Html | Css | Scss | Less | JavaScript | Jsx | TypeScript | Tsx);
    let jvm = |l: Language| matches!(l, Java | Kotlin);
    a == b || (web(a) && web(b)) || (jvm(a) && jvm(b))
}

/// Every place `name` is written as a whole name, in files of the same family of languages as
/// `language` (a CSS class is looked for in markup and scripts too).
pub fn usages(root: &Path, name: &str, language: Language) -> Vec<Usage> {
    let mut found = Vec::new();
    for (rel, other) in code_files(root) {
        if !same_family(language, other) {
            continue;
        }
        let path = root.join(&rel);
        if stamp(&path).is_none_or(|(_, len)| len > MAX_PARSE_BYTES * 2) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        if !text.contains(name) {
            continue;
        }
        let lines: Vec<&str> = text.lines().collect();
        for (line, col) in symbols::occurrences(&text, name, other, MAX_USAGES - found.len()) {
            let shown: String = lines.get(line).map(|l| l.trim()).unwrap_or("").chars().take(160).collect();
            found.push(Usage { path: path.clone(), rel: rel.clone(), line, col, text: shown });
        }
        if found.len() >= MAX_USAGES {
            break;
        }
    }
    found
}

/// A place a text search found: the line as shown, and where the match is in what is shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextMatch {
    pub usage: Usage,
    /// The match's length in bytes, at `usage.col` of the line.
    pub len: usize,
    /// The match in `usage.text`, when it is in what is shown.
    pub mark: Option<(usize, usize)>,
}

/// Every place `query` is written in the project's files (any file, not only code), at most
/// `limit`; case is ignored unless the query has a capital letter. Text of files open with
/// unsaved changes comes from `open` instead of the disk. Binary and very large files are left
/// out. `None` when `cancelled` was set before the search finished.
pub fn search_text(root: &Path, query: &str, open: &[(PathBuf, String)], limit: usize, cancelled: &AtomicBool) -> Option<Vec<TextMatch>> {
    let any_case = !query.chars().any(char::is_uppercase);
    let pattern = regex::RegexBuilder::new(&regex::escape(query)).case_insensitive(any_case).build().ok()?;
    let mut found = Vec::new();
    for rel in files(root).iter() {
        if cancelled.load(Ordering::Relaxed) {
            return None;
        }
        let path = root.join(rel);
        let text = match open.iter().find(|(open, _)| *open == path) {
            Some((_, text)) => text.clone(),
            None => {
                if stamp(&path).is_none_or(|(_, len)| len > MAX_SEARCH_BYTES) {
                    continue;
                }
                let Ok(bytes) = std::fs::read(&path) else { continue };
                if bytes[..bytes.len().min(8192)].contains(&0) {
                    continue;
                }
                match String::from_utf8(bytes) {
                    Ok(text) => text,
                    Err(_) => continue,
                }
            }
        };
        if !pattern.is_match(&text) {
            continue;
        }
        for (line, content) in text.lines().enumerate() {
            for m in pattern.find_iter(content) {
                let (text, mark) = shown_line(content, m.start(), m.end());
                let usage = Usage { path: path.clone(), rel: rel.clone(), line, col: m.start(), text };
                found.push(TextMatch { usage, len: m.len(), mark });
                if found.len() >= limit {
                    return Some(found);
                }
            }
        }
    }
    Some(found)
}

/// A found line as a list shows it, trimmed and cut short around the match (from a long line, the
/// part with the match, after `…`), with where the match is in it.
fn shown_line(line: &str, start: usize, end: usize) -> (String, Option<(usize, usize)>) {
    let trimmed = line.trim_start();
    let mut from = line.len() - trimmed.len();
    let mut lead = "";
    if start < from {
        from = start;
    } else if line[from..start].chars().count() > SHOWN_CHARS - SHOWN_BEFORE {
        from = line[..start].char_indices().rev().nth(SHOWN_BEFORE - 1).map_or(from, |(i, _)| i);
        lead = "…";
    }
    let shown: String = line[from..].trim_end().chars().take(SHOWN_CHARS).collect();
    let mark = (end - from <= shown.len()).then_some((lead.len() + start - from, lead.len() + end - from));
    (format!("{lead}{shown}"), mark)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_text_in_any_file() {
        let root = std::env::temp_dir().join(format!("agentty-text-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/lib")).unwrap();
        std::fs::write(root.join("src/api.ts"), "const BASE = '/api/products';\n  fetch(`/API/products/${id}`);\n").unwrap();
        std::fs::write(root.join("README.md"), "Calls /api/products and /api/products/:id.\n").unwrap();
        std::fs::write(root.join("logo.png"), b"\x89PNG\0\0/api/products").unwrap();
        std::fs::write(root.join("node_modules/lib/x.js"), "'/api/products'\n").unwrap();
        let never = AtomicBool::new(false);
        let places = |query: &str, open: &[(PathBuf, String)]| {
            search_text(&root, query, open, 100, &never)
                .unwrap()
                .into_iter()
                .map(|m| (m.usage.rel, m.usage.line, m.usage.col))
                .collect::<Vec<_>>()
        };
        // Lower case finds any case; dependencies and binary files are left out.
        assert_eq!(
            places("/api/products", &[]),
            [("README.md".into(), 0, 6), ("README.md".into(), 0, 24), ("src/api.ts".into(), 0, 14), ("src/api.ts".into(), 1, 9)]
        );
        // A capital letter: case matters.
        assert_eq!(places("/API", &[]), [("src/api.ts".to_string(), 1, 9)]);
        // An open file's unsaved text is what is searched.
        let open = [(root.join("src/api.ts"), "\n\nconst BASE = '/api/products';\n".to_string())];
        assert_eq!(places("BASE", &open), [("src/api.ts".to_string(), 2, 6)]);
        assert!(search_text(&root, "api", &[], 100, &AtomicBool::new(true)).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_found_line_shows_its_match() {
        assert_eq!(shown_line("    let total = 3;", 8, 13), ("let total = 3;".into(), Some((4, 9))));
        let long = format!("{}needle{}", "x".repeat(300), "y".repeat(300));
        let (text, mark) = shown_line(&long, 300, 306);
        let (start, end) = mark.unwrap();
        assert!(text.starts_with('…') && text.chars().count() == 161);
        assert_eq!(&text[start..end], "needle");
        // A match longer than what is shown: the line without a mark.
        assert_eq!(shown_line(&"z".repeat(200), 0, 200), ("z".repeat(160), None));
    }

    #[test]
    fn finds_definitions_and_usages_across_files() {
        let root = std::env::temp_dir().join(format!("agentty-index-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src/model")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/lib")).unwrap();
        std::fs::write(root.join("src/model/user.ts"), "export class User {}\nexport function makeUser() { return new User(); }\n")
            .unwrap();
        std::fs::write(root.join("src/app.ts"), "import { User } from './model/user';\nconst u: User = makeUser();\n").unwrap();
        std::fs::write(root.join("node_modules/lib/user.ts"), "export class User {}\n").unwrap();
        std::fs::create_dir_all(root.join("api/src")).unwrap();
        std::fs::write(root.join("api/src/User.kt"), "data class User(val id: Long)\n").unwrap();
        std::fs::write(root.join("api/src/Repo.java"), "class Repo { User find() { return null; } }\n").unwrap();
        let found = definitions(&root, "User", "src/app.ts");
        assert_eq!(found.len(), 1, "dependencies are not the project: {found:?}");
        assert_eq!((found[0].rel.as_str(), found[0].line, found[0].kind), ("src/model/user.ts", 0, Kind::Class));
        let used = usages(&root, "User", Language::TypeScript);
        assert_eq!(
            used.iter().map(|u| (u.rel.as_str(), u.line)).collect::<Vec<_>>(),
            vec![("src/app.ts", 0), ("src/app.ts", 1), ("src/model/user.ts", 0), ("src/model/user.ts", 1)]
        );
        assert_eq!(search_symbols(&root, "mkus", 5).first().map(|l| l.name.as_str()), Some("makeUser"));

        // A changed file is read again.
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(root.join("src/model/user.ts"), "\n\nexport class User {}\n").unwrap();
        assert_eq!(definitions(&root, "User", "src/app.ts")[0].line, 2);

        // The back end beside it: each side finds its own `User`, Java and Kotlin each other's.
        let rels = |from: &str| definitions(&root, "User", from).into_iter().map(|l| l.rel).collect::<Vec<_>>();
        assert_eq!(rels("src/app.ts"), ["src/model/user.ts"]);
        assert_eq!(rels("api/src/Repo.java"), ["api/src/User.kt"]);
        // Nothing of its own family: the others still come.
        assert_eq!(rels("styles/main.py").len(), 2);
        let jvm: Vec<String> = names(&root, Language::Kotlin, 100).into_iter().map(|(n, _)| n).collect();
        assert!(jvm.contains(&"User".to_string()) && jvm.contains(&"Repo".to_string()) && !jvm.contains(&"makeUser".to_string()));
        let _ = std::fs::remove_dir_all(&root);
    }
}
