//! What the editor knows about a project as a whole: its files (git's list when it is a
//! repository, so `.gitignore` is respected; otherwise a walk that leaves out dependency and build
//! folders) and the symbols defined in them (`symbols.rs`), kept per file and re-read only when a
//! file changed. Everything here reads files and runs `git ls-files`; it is called off the UI
//! thread.

use super::language::Language;
use super::symbols::{self, Kind, Symbol};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
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
pub fn definitions(root: &Path, name: &str, from: &str) -> Vec<Location> {
    let all = all_symbols(root);
    // An `impl` names a type, it does not define it.
    let mut found: Vec<(&str, &Symbol)> = all
        .iter()
        .flat_map(|(rel, list)| list.iter().filter(|s| s.name == name && s.kind != Kind::Impl).map(move |s| (rel.as_str(), s)))
        .collect();
    symbols::rank(&mut found, from);
    found.into_iter().map(|(rel, s)| location(root, rel, s)).collect()
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

/// Every place `name` is written as a whole name, in files of the same family of languages as
/// `language` (a CSS class is looked for in markup and scripts too).
pub fn usages(root: &Path, name: &str, language: Language) -> Vec<Usage> {
    let family = |other: Language| -> bool {
        use Language::*;
        let web = |l: Language| matches!(l, Html | Css | Scss | Less | JavaScript | Jsx | TypeScript | Tsx);
        let jvm = |l: Language| matches!(l, Java | Kotlin);
        other == language || (web(language) && web(other)) || (jvm(language) && jvm(other))
    };
    let mut found = Vec::new();
    for (rel, other) in code_files(root) {
        if !family(other) {
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let _ = std::fs::remove_dir_all(&root);
    }
}
