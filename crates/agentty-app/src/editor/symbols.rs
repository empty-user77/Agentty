//! Where things are defined in a file: functions, classes, types and the like, read line by line
//! with a few patterns per language. No compiler and no language server — what this finds is what
//! a person skimming the file would point at, which is what "go to definition" and the symbol
//! lists need. Supported: Kotlin, Java, JavaScript, TypeScript, Python, HTML, CSS (Tailwind's
//! `@utility` / `@theme` included, SCSS and Less too), Rust and Go.

use super::language::Language;
use regex::Regex;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Function,
    Method,
    Class,
    Interface,
    Struct,
    Enum,
    Type,
    Module,
    Variable,
    Constant,
    Field,
    Selector,
    Id,
}

impl Kind {
    /// The short tag shown in front of a symbol in the lists.
    pub fn tag(self) -> &'static str {
        match self {
            Self::Function => "fn",
            Self::Method => "method",
            Self::Class => "class",
            Self::Interface => "interface",
            Self::Struct => "struct",
            Self::Enum => "enum",
            Self::Type => "type",
            Self::Module => "mod",
            Self::Variable => "var",
            Self::Constant => "const",
            Self::Field => "field",
            Self::Selector => "css",
            Self::Id => "id",
        }
    }

    /// Kinds that name a type: preferred when a name is defined both as a type and as something else.
    fn is_type(self) -> bool {
        matches!(self, Self::Class | Self::Interface | Self::Struct | Self::Enum | Self::Type)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub kind: Kind,
    /// Line (from 0) and byte column of the name.
    pub line: usize,
    pub col: usize,
}

/// Whether this language has symbols, navigation and usages (the languages the editor supports
/// for code navigation).
pub fn supported(language: Language) -> bool {
    matches!(
        language,
        Language::Kotlin
            | Language::Java
            | Language::JavaScript
            | Language::Jsx
            | Language::TypeScript
            | Language::Tsx
            | Language::Python
            | Language::Html
            | Language::Css
            | Language::Scss
            | Language::Less
            | Language::Rust
            | Language::Go
    )
}

/// Languages whose names may hold a dash (`btn-primary`, `--brand`): CSS and the HTML using it.
pub fn dashed_names(language: Language) -> bool {
    matches!(language, Language::Html | Language::Css | Language::Scss | Language::Less)
}

/// Words of the supported languages that are not names (marking every `return` says nothing).
pub fn is_keyword(word: &str) -> bool {
    const KEYWORDS: &[&str] = &[
        "abstract",
        "as",
        "async",
        "await",
        "break",
        "case",
        "catch",
        "class",
        "const",
        "continue",
        "def",
        "default",
        "defer",
        "del",
        "do",
        "elif",
        "else",
        "enum",
        "except",
        "export",
        "extends",
        "false",
        "final",
        "finally",
        "fn",
        "for",
        "from",
        "fun",
        "func",
        "function",
        "go",
        "if",
        "impl",
        "implements",
        "import",
        "in",
        "interface",
        "is",
        "let",
        "match",
        "mod",
        "mut",
        "new",
        "nil",
        "None",
        "null",
        "object",
        "override",
        "package",
        "pass",
        "private",
        "protected",
        "pub",
        "public",
        "raise",
        "return",
        "self",
        "Self",
        "static",
        "struct",
        "super",
        "switch",
        "this",
        "throw",
        "throws",
        "trait",
        "true",
        "True",
        "False",
        "try",
        "type",
        "typeof",
        "use",
        "val",
        "var",
        "void",
        "when",
        "where",
        "while",
        "with",
        "yield",
    ];
    KEYWORDS.contains(&word)
}

/// Whether `c` can be part of a name in `language`.
pub fn is_name_char(c: char, language: Language) -> bool {
    c.is_alphanumeric() || c == '_' || (c == '$' && is_js(language)) || (c == '-' && dashed_names(language))
}

fn is_js(language: Language) -> bool {
    matches!(language, Language::JavaScript | Language::Jsx | Language::TypeScript | Language::Tsx)
}

/// The name around byte `col` of `line`, as (start, end) bytes; `None` on anything else.
pub fn name_at(line: &str, col: usize, language: Language) -> Option<(usize, usize)> {
    let col = col.min(line.len());
    let mut start = col;
    for (i, c) in line[..col].char_indices().rev() {
        if !is_name_char(c, language) {
            break;
        }
        start = i;
    }
    let mut end = col;
    for (i, c) in line[col..].char_indices() {
        if !is_name_char(c, language) {
            break;
        }
        end = col + i + c.len_utf8();
    }
    // A run of dashes or digits is not a name.
    let name = &line[start..end];
    (end > start && name.chars().any(|c| c.is_alphabetic() || c == '_' || c == '$')).then_some((start, end))
}

/// One pattern: group 1 is the name (or group 2 when the kind is read from group 1).
struct Rule {
    regex: Regex,
    kind: Kind,
    /// Only lines indented at most this many columns (a tab counts 4): keeps locals out.
    max_indent: Option<usize>,
}

fn rule(pattern: &str, kind: Kind) -> Rule {
    Rule { regex: Regex::new(pattern).expect("symbol pattern"), kind, max_indent: None }
}

fn shallow(pattern: &str, kind: Kind, indent: usize) -> Rule {
    Rule { max_indent: Some(indent), ..rule(pattern, kind) }
}

const JAVA_MODIFIERS: &str = r"(?:(?:public|private|protected|internal|static|final|abstract|sealed|non-sealed|strictfp|synchronized|native|default|transient|volatile)\s+)";
const KOTLIN_MODIFIERS: &str = r"(?:(?:public|private|protected|internal|open|override|abstract|final|inline|suspend|operator|infix|tailrec|external|private\s+set|lateinit|const|expect|actual|data|sealed|enum|annotation|inner|value|companion|fun)\s+)";
const TS_MODIFIERS: &str = r"(?:(?:public|private|protected|static|async|readonly|override|abstract|declare|get|set)\s+)";
/// Words that look like a call or a statement in front of `(`: never a method's name.
const NOT_METHODS: &[&str] = &[
    "if",
    "for",
    "while",
    "switch",
    "catch",
    "return",
    "function",
    "new",
    "throw",
    "else",
    "do",
    "try",
    "typeof",
    "await",
    "yield",
    "super",
    "this",
    "synchronized",
    "when",
    "with",
    "elif",
    "assert",
    "sizeof",
    "constructor",
];

fn rules(language: Language) -> &'static [Rule] {
    static RUST: OnceLock<Vec<Rule>> = OnceLock::new();
    static GO: OnceLock<Vec<Rule>> = OnceLock::new();
    static PYTHON: OnceLock<Vec<Rule>> = OnceLock::new();
    static JS: OnceLock<Vec<Rule>> = OnceLock::new();
    static JAVA: OnceLock<Vec<Rule>> = OnceLock::new();
    static KOTLIN: OnceLock<Vec<Rule>> = OnceLock::new();
    static HTML: OnceLock<Vec<Rule>> = OnceLock::new();
    static CSS: OnceLock<Vec<Rule>> = OnceLock::new();
    let vis = r"(?:pub(?:\s*\([^)]*\))?\s+)?";
    match language {
        Language::Rust => RUST.get_or_init(|| {
            vec![
                rule(&format!(r#"^\s*{vis}(?:(?:default|const|async|unsafe|extern(?:\s+"[^"]*")?)\s+)*fn\s+([A-Za-z_]\w*)"#), Kind::Function),
                rule(&format!(r"^\s*{vis}struct\s+([A-Za-z_]\w*)"), Kind::Struct),
                rule(&format!(r"^\s*{vis}union\s+([A-Za-z_]\w*)"), Kind::Struct),
                rule(&format!(r"^\s*{vis}enum\s+([A-Za-z_]\w*)"), Kind::Enum),
                rule(&format!(r"^\s*{vis}(?:unsafe\s+)?(?:auto\s+)?trait\s+([A-Za-z_]\w*)"), Kind::Interface),
                rule(&format!(r"^\s*{vis}type\s+([A-Za-z_]\w*)"), Kind::Type),
                rule(&format!(r"^\s*{vis}mod\s+([A-Za-z_]\w*)"), Kind::Module),
                rule(&format!(r"^\s*{vis}(?:const|static)\s+(?:mut\s+)?([A-Za-z_]\w*)\s*:"), Kind::Constant),
                rule(r"^\s*(?:#\[macro_export\]\s*)?macro_rules!\s*([A-Za-z_]\w*)", Kind::Function),
            ]
        }),
        Language::Go => GO.get_or_init(|| {
            vec![
                rule(r"^func\s+\([^)]*\)\s*([A-Za-z_]\w*)", Kind::Method),
                rule(r"^func\s+([A-Za-z_]\w*)", Kind::Function),
                rule(r"^\s*type\s+([A-Za-z_]\w*)\s+(?:\[[^\]]*\]\s*)?struct\b", Kind::Struct),
                rule(r"^\s*type\s+([A-Za-z_]\w*)\s+(?:\[[^\]]*\]\s*)?interface\b", Kind::Interface),
                rule(r"^\s*type\s+([A-Za-z_]\w*)", Kind::Type),
                // Inside `type ( … )`.
                shallow(r"^\s+([A-Za-z_]\w*)\s+(?:struct|interface)\s*\{", Kind::Struct, 4),
                rule(r"^(?:var|const)\s+([A-Za-z_]\w*)", Kind::Variable),
                // Inside `const ( … )` / `var ( … )`.
                shallow(r"^\s+([A-Z]\w*)\s*(?:=|\s+[\w.\[\]*]+\s*=)", Kind::Constant, 4),
            ]
        }),
        Language::Python => PYTHON.get_or_init(|| {
            vec![
                rule(r"^\s*(?:async\s+)?def\s+([A-Za-z_]\w*)", Kind::Function),
                rule(r"^\s*class\s+([A-Za-z_]\w*)", Kind::Class),
                shallow(r"^([A-Za-z_]\w*)\s*(?::[^=]+)?=[^=]", Kind::Variable, 0),
            ]
        }),
        Language::JavaScript | Language::Jsx | Language::TypeScript | Language::Tsx => JS.get_or_init(|| {
            let ex = r"(?:export\s+)?(?:default\s+)?";
            vec![
                rule(&format!(r"^\s*{ex}(?:async\s+)?function\s*\*?\s*([A-Za-z_$][\w$]*)"), Kind::Function),
                rule(&format!(r"^\s*{ex}(?:abstract\s+)?class\s+([A-Za-z_$][\w$]*)"), Kind::Class),
                rule(&format!(r"^\s*{ex}(?:declare\s+)?interface\s+([A-Za-z_$][\w$]*)"), Kind::Interface),
                rule(&format!(r"^\s*{ex}(?:declare\s+)?type\s+([A-Za-z_$][\w$]*)\s*(?:<[^=]*>)?\s*="), Kind::Type),
                rule(&format!(r"^\s*{ex}(?:declare\s+)?(?:const\s+)?enum\s+([A-Za-z_$][\w$]*)"), Kind::Enum),
                rule(&format!(r"^\s*{ex}(?:declare\s+)?(?:namespace|module)\s+([A-Za-z_$][\w$]*)"), Kind::Module),
                shallow(r"^\s*(?:export\s+)?(?:declare\s+)?(?:const|let|var)\s+([A-Za-z_$][\w$]*)", Kind::Variable, 0),
                // `export const x`, anywhere (inside a `declare module` too).
                rule(r"^\s*export\s+(?:declare\s+)?(?:const|let|var)\s+([A-Za-z_$][\w$]*)", Kind::Variable),
                // Methods in classes and object literals: `name(…) {`, `async name(…): T {`, and
                // ones written on one line (`area() { return 4; }`).
                rule(&format!(r"^\s+{TS_MODIFIERS}*\*?([A-Za-z_$][\w$]*)\s*(?:<[^>()]*>)?\s*\([^)]*\)?\s*(?::\s*[^{{=;]+)?\s*\{{(?:\s*$|.*\}}\s*;?\s*$)"), Kind::Method),
                // Class fields with an arrow function: `name = (…) =>`.
                rule(&format!(r"^\s+{TS_MODIFIERS}*([A-Za-z_$][\w$]*)\s*(?::[^=]+)?=\s*(?:async\s+)?(?:\([^)]*\)|[A-Za-z_$][\w$]*)\s*(?::\s*[^=]+)?=>"), Kind::Method),
            ]
        }),
        Language::Java => JAVA.get_or_init(|| {
            vec![
                rule(&format!(r"^\s*{JAVA_MODIFIERS}*(?:@\w+\s+)*(?:class|record)\s+([A-Za-z_$][\w$]*)"), Kind::Class),
                rule(&format!(r"^\s*{JAVA_MODIFIERS}*(?:@interface|interface)\s+([A-Za-z_$][\w$]*)"), Kind::Interface),
                rule(&format!(r"^\s*{JAVA_MODIFIERS}*enum\s+([A-Za-z_$][\w$]*)"), Kind::Enum),
                // `<T> List<T> name(` — a return type (or a constructor's modifiers) before the name.
                rule(&format!(r"^\s*{JAVA_MODIFIERS}*(?:<[^>]+>\s+)?[\w$][\w$.]*(?:<[^()=]*>)?(?:\[\])*\s+([A-Za-z_$][\w$]*)\s*\("), Kind::Method),
                rule(&format!(r"^\s*{JAVA_MODIFIERS}+([A-Z][\w$]*)\s*\("), Kind::Method),
                rule(&format!(r"^\s*{JAVA_MODIFIERS}+[\w$][\w$.]*(?:<[^()=]*>)?(?:\[\])*\s+([A-Za-z_$][\w$]*)\s*(?:=|;)"), Kind::Field),
                // Enum constants: `RED,` `RED("r"),` alone on their line.
                shallow(r"^\s+([A-Z][A-Z0-9_]*)\s*(?:\([^)]*\))?\s*[,;]?\s*$", Kind::Constant, 8),
            ]
        }),
        Language::Kotlin => KOTLIN.get_or_init(|| {
            vec![
                rule(&format!(r"^\s*(?:@\w+(?:\([^)]*\))?\s+)*{KOTLIN_MODIFIERS}*fun\s+(?:<[^>]*>\s*)?(?:[\w.<>?, ]+\.)?([A-Za-z_]\w*|`[^`]+`)\s*\("), Kind::Function),
                rule(&format!(r"^\s*(?:@\w+(?:\([^)]*\))?\s+)*{KOTLIN_MODIFIERS}*interface\s+([A-Za-z_]\w*)"), Kind::Interface),
                rule(&format!(r"^\s*(?:@\w+(?:\([^)]*\))?\s+)*{KOTLIN_MODIFIERS}*(?:class|object)\s+([A-Za-z_]\w*)"), Kind::Class),
                rule(&format!(r"^\s*{KOTLIN_MODIFIERS}*typealias\s+([A-Za-z_]\w*)"), Kind::Type),
                shallow(&format!(r"^\s*(?:@\w+(?:\([^)]*\))?\s+)*{KOTLIN_MODIFIERS}*(?:val|var)\s+(?:<[^>]*>\s*)?(?:[\w.<>?]+\.)?([A-Za-z_]\w*)"), Kind::Variable, 4),
                // Enum entries alone on their line: `RED,` `RED("r"),`.
                shallow(r"^\s+([A-Z][A-Z0-9_]*)\s*(?:\([^)]*\))?\s*[,;]?\s*$", Kind::Constant, 8),
            ]
        }),
        Language::Html => HTML.get_or_init(|| vec![rule(r#"\bid\s*=\s*["']([A-Za-z_][\w-]*)["']"#, Kind::Id)]),
        Language::Css | Language::Scss | Language::Less => CSS.get_or_init(|| {
            vec![
                rule(r"^\s*(--[A-Za-z_][\w-]*)\s*:", Kind::Variable),
                rule(r"^\s*\$([A-Za-z_][\w-]*)\s*:", Kind::Variable),
                rule(r"^\s*@([A-Za-z_][\w-]*)\s*:", Kind::Variable),
                rule(r"^\s*@(?:utility|keyframes|mixin|function|custom-variant|variant)\s+([A-Za-z_-][\w-]*)", Kind::Function),
            ]
        }),
        _ => &[],
    }
}

/// The symbols defined in `text`, in order.
pub fn symbols(language: Language, text: &str) -> Vec<Symbol> {
    if !supported(language) {
        return Vec::new();
    }
    let rules = rules(language);
    let css = matches!(language, Language::Css | Language::Scss | Language::Less);
    let python = language == Language::Python;
    let mut found = Vec::new();
    let mut in_comment = false;
    let mut in_docstring: Option<&str> = None;
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        // Block comments (and Python's docstrings) hold examples that are not definitions.
        if python {
            if let Some(quote) = in_docstring {
                if trimmed.contains(quote) {
                    in_docstring = None;
                }
                continue;
            }
            if let Some(quote) = ["\"\"\"", "'''"].into_iter().find(|q| trimmed.starts_with(q)) {
                if trimmed[3..].find(quote).is_none() {
                    in_docstring = Some(quote);
                }
                continue;
            }
        } else if language != Language::Html {
            if in_comment {
                if trimmed.contains("*/") {
                    in_comment = false;
                }
                continue;
            }
            if trimmed.starts_with("/*") {
                in_comment = !trimmed.contains("*/");
                continue;
            }
            if trimmed.starts_with("//") || trimmed.starts_with('*') {
                continue;
            }
        }
        if python && trimmed.starts_with('#') {
            continue;
        }
        let indent = line.chars().take_while(|c| c.is_whitespace()).map(|c| if c == '\t' { 4 } else { 1 }).sum::<usize>();
        for rule in rules {
            if rule.max_indent.is_some_and(|max| indent > max) {
                continue;
            }
            if language == Language::Html {
                for captures in rule.regex.captures_iter(line) {
                    let name = captures.get(1).expect("name group");
                    found.push(Symbol { name: name.as_str().to_string(), kind: rule.kind, line: index, col: name.start() });
                }
                continue;
            }
            let Some(name) = rule.regex.captures(line).and_then(|c| c.get(1)) else { continue };
            let text = name.as_str().trim_matches('`');
            if matches!(rule.kind, Kind::Method | Kind::Field | Kind::Constant) && NOT_METHODS.contains(&text) {
                continue;
            }
            found.push(Symbol { name: text.to_string(), kind: rule.kind, line: index, col: name.start() });
            break;
        }
        if css {
            css_selectors(line, index, &mut found);
        }
    }
    found
}

/// Class and id selectors on a line that opens a rule (or lists selectors before one).
fn css_selectors(line: &str, index: usize, found: &mut Vec<Symbol>) {
    let head = match line.find('{') {
        Some(brace) => &line[..brace],
        None if line.trim_end().ends_with(',') => line,
        None => return,
    };
    // A declaration (`color: #fff;`) or an at-rule's condition is not a selector.
    if head.contains(';') || head.trim_start().starts_with('@') || head.trim().is_empty() {
        return;
    }
    static SELECTOR: OnceLock<Regex> = OnceLock::new();
    let selector = SELECTOR.get_or_init(|| Regex::new(r"(?:^|[\s,>+~(&])([.#])(-?[A-Za-z_][\w-]*)").expect("selector pattern"));
    for captures in selector.captures_iter(head) {
        let (sigil, name) = (captures.get(1).expect("sigil"), captures.get(2).expect("name"));
        let kind = if sigil.as_str() == "#" { Kind::Id } else { Kind::Selector };
        if !found.iter().any(|s| s.line == index && s.name == name.as_str()) {
            found.push(Symbol { name: name.as_str().to_string(), kind, line: index, col: name.start() });
        }
    }
}

/// Kinds that hold others (what a breadcrumb names).
fn is_container(kind: Kind) -> bool {
    matches!(kind, Kind::Class | Kind::Interface | Kind::Struct | Kind::Enum | Kind::Module | Kind::Function | Kind::Method | Kind::Type)
}

/// The definitions the cursor's line sits in, outermost first, judged by indentation: a definition
/// holds the lines after it that are indented deeper (or its closing line at its own depth).
pub fn enclosing<'a>(symbols: &'a [Symbol], line_at: impl Fn(usize) -> &'a str, cursor: usize) -> Vec<&'a Symbol> {
    let indent = |text: &str| text.chars().take_while(|c| c.is_whitespace()).map(|c| if c == '\t' { 4 } else { 1 }).sum::<usize>();
    let mut stack: Vec<(usize, &Symbol)> = Vec::new();
    for symbol in symbols.iter().filter(|s| s.line <= cursor && is_container(s.kind)) {
        let depth = indent(line_at(symbol.line));
        while stack.last().is_some_and(|(d, _)| *d >= depth) {
            stack.pop();
        }
        stack.push((depth, symbol));
    }
    // A definition whose block ended above the cursor (a line back at its depth or shallower,
    // that is not just its closing brace) does not hold it.
    let here = line_at(cursor);
    if !here.trim().is_empty() {
        let depth = indent(here);
        while let Some((d, s)) = stack.last() {
            let closing = depth == *d && here.trim_start().starts_with(['}', ')', ']']);
            if s.line != cursor && depth <= *d && !closing {
                stack.pop();
            } else {
                break;
            }
        }
    }
    stack.into_iter().map(|(_, s)| s).collect()
}

/// Orders definitions of one name, best first: in this file, then types over the rest, then by
/// how close the file is to `from` (same folder first).
pub fn rank<'a>(candidates: &mut [(&'a str, &'a Symbol)], from: &str) {
    let from_dir = from.rsplit_once('/').map_or("", |(d, _)| d);
    let shared = |path: &str| {
        let dir = path.rsplit_once('/').map_or("", |(d, _)| d);
        from_dir.split('/').zip(dir.split('/')).take_while(|(a, b)| a == b).count()
    };
    candidates.sort_by(|a, b| {
        (b.0 == from)
            .cmp(&(a.0 == from))
            .then_with(|| b.1.kind.is_type().cmp(&a.1.kind.is_type()))
            .then_with(|| shared(b.0).cmp(&shared(a.0)))
            .then_with(|| a.0.cmp(b.0))
            .then_with(|| a.1.line.cmp(&b.1.line))
    });
}

/// Every place `name` appears as a whole name in `text`: (line, byte column).
pub fn occurrences(text: &str, name: &str, language: Language, limit: usize) -> Vec<(usize, usize)> {
    let mut found = Vec::new();
    if name.is_empty() {
        return found;
    }
    for (index, line) in text.lines().enumerate() {
        let mut from = 0;
        while let Some(offset) = line[from..].find(name) {
            let start = from + offset;
            let end = start + name.len();
            let before = line[..start].chars().next_back();
            let after = line[end..].chars().next();
            if !before.is_some_and(|c| is_name_char(c, language)) && !after.is_some_and(|c| is_name_char(c, language)) {
                found.push((index, start));
                if found.len() >= limit {
                    return found;
                }
            }
            from = end;
        }
    }
    found
}

/// A file a name under the cursor points at: the path in an import string (`'./api'`,
/// `"../lib/util.js"`), a Rust `mod name;` or a Python `from pkg.mod import`. Relative to the
/// folder of the file it is written in (`dir`, inside `project`); the first that exists wins.
pub fn linked_file(
    line: &str,
    col: usize,
    language: Language,
    dir: &std::path::Path,
    project: &std::path::Path,
) -> Option<std::path::PathBuf> {
    use std::path::{Path, PathBuf};
    let exists = |path: PathBuf| path.is_file().then_some(path);
    let inside = |path: &Path| path.starts_with(project);
    // A quoted path around the cursor.
    let quoted = || -> Option<&str> {
        let before = &line[..col.min(line.len())];
        let open = before.rfind(['"', '\'', '`'])?;
        let quote = line.as_bytes()[open] as char;
        let close = line[open + 1..].find(quote)? + open + 1;
        (close >= col).then(|| &line[open + 1..close])
    };
    if let Some(text) = quoted().filter(|t| !t.is_empty() && !t.contains("://")) {
        let base = if text.starts_with('/') { project.to_path_buf() } else { dir.to_path_buf() };
        let joined = base.join(text.trim_start_matches('/'));
        let candidates: Vec<PathBuf> = if text.starts_with('.') || text.starts_with('/') || text.contains('.') && !text.contains(' ') {
            let mut list = vec![joined.clone()];
            for extension in ["ts", "tsx", "js", "jsx", "mjs", "cjs", "d.ts", "css", "scss", "html", "py", "go", "rs", "kt", "java"] {
                list.push(PathBuf::from(format!("{}.{extension}", joined.display())));
            }
            for index in ["index.ts", "index.tsx", "index.js", "index.jsx", "mod.rs", "__init__.py"] {
                list.push(joined.join(index));
            }
            list
        } else {
            Vec::new()
        };
        if let Some(found) = candidates.into_iter().filter(|p| inside(p)).find_map(exists) {
            return Some(normalize(&found));
        }
    }
    let trimmed = line.trim();
    if language == Language::Rust {
        static MOD: OnceLock<Regex> = OnceLock::new();
        let re = MOD.get_or_init(|| Regex::new(r"^(?:pub(?:\([^)]*\))?\s+)?mod\s+([A-Za-z_]\w*)\s*;").expect("mod pattern"));
        if let Some(name) = re.captures(trimmed).and_then(|c| c.get(1)) {
            return [dir.join(format!("{}.rs", name.as_str())), dir.join(name.as_str()).join("mod.rs")].into_iter().find_map(exists);
        }
    }
    if language == Language::Python {
        static FROM: OnceLock<Regex> = OnceLock::new();
        let re = FROM.get_or_init(|| Regex::new(r"^(?:from\s+(\.*[\w.]*)\s+import|import\s+([\w.]+))").expect("import pattern"));
        let captures = re.captures(trimmed)?;
        let module = captures.get(1).or_else(|| captures.get(2))?.as_str();
        let dots = module.chars().take_while(|&c| c == '.').count();
        let rest = module[dots..].replace('.', "/");
        let mut bases: Vec<PathBuf> = Vec::new();
        if dots > 0 {
            let mut base = dir.to_path_buf();
            for _ in 1..dots {
                base.pop();
            }
            bases.push(base);
        } else {
            // The package may start in any folder from the file's up to the project's
            // (`py/shop/cart.py` importing `shop.pricing` reads `py/` as the root).
            bases.extend(dir.ancestors().take_while(|a| a.starts_with(project)).map(Path::to_path_buf));
            bases.push(project.join("src"));
        }
        return bases.into_iter().find_map(|base| {
            let path = if rest.is_empty() { base.clone() } else { base.join(&rest) };
            [PathBuf::from(format!("{}.py", path.display())), path.join("__init__.py")].into_iter().filter(|p| inside(p)).find_map(exists)
        });
    }
    None
}

/// `a/./b/../c` → `a/c`, without touching the disk (links stay as they are).
fn normalize(path: &std::path::Path) -> std::path::PathBuf {
    use std::path::Component;
    let mut out = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// How well `query` matches `text` as a subsequence of its characters, ignoring case; `None` when
/// it does not. Higher is better: runs of characters, starts of words and a match in the last part
/// (the file name) count most.
pub fn fuzzy_score(text: &str, query: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }
    let name_start = text.rfind('/').map_or(0, |i| i + 1);
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut score = 0i64;
    let mut at = 0;
    let mut previous: Option<usize> = None;
    for wanted in query.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase) {
        let found = chars[at..].iter().position(|(_, c)| c.to_lowercase().eq(std::iter::once(wanted)))? + at;
        let (byte, c) = chars[found];
        let boundary = found == 0 || {
            let before = chars[found - 1].1;
            !before.is_alphanumeric() || (before.is_lowercase() && c.is_uppercase())
        };
        score += 1;
        if previous == Some(found.wrapping_sub(1)) {
            score += 6;
        }
        if boundary {
            score += 8;
        }
        if byte >= name_start {
            score += 3;
        }
        previous = Some(found);
        at = found + 1;
    }
    // Shorter texts first among equals; the file name matching from its start most of all.
    if text[name_start..].to_lowercase().starts_with(&query.to_lowercase()) {
        score += 30;
    }
    Some(score * 16 - text.len() as i64 / 4)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(language: Language, text: &str) -> Vec<(String, Kind, usize)> {
        symbols(language, text).into_iter().map(|s| (s.name, s.kind, s.line)).collect()
    }

    #[test]
    fn rust_items() {
        let text = "pub(crate) async fn load() {}\nstruct Point { x: i32 }\nimpl Point {\n    pub fn new() -> Self {}\n}\npub enum Tone {}\ntrait Draw {}\nconst MAX: usize = 3;\nmod tests;\n// fn commented() {}\nmacro_rules! key {}";
        assert_eq!(
            names(Language::Rust, text),
            vec![
                ("load".into(), Kind::Function, 0),
                ("Point".into(), Kind::Struct, 1),
                ("new".into(), Kind::Function, 3),
                ("Tone".into(), Kind::Enum, 5),
                ("Draw".into(), Kind::Interface, 6),
                ("MAX".into(), Kind::Constant, 7),
                ("tests".into(), Kind::Module, 8),
                ("key".into(), Kind::Function, 10),
            ]
        );
    }

    #[test]
    fn go_declarations() {
        let text = "func main() {}\nfunc (s *Server) Start() error {}\ntype Server struct {}\ntype Store interface {}\ntype ID string\nvar ErrX = 1\nconst (\n    Limit = 3\n)";
        assert_eq!(
            names(Language::Go, text),
            vec![
                ("main".into(), Kind::Function, 0),
                ("Start".into(), Kind::Method, 1),
                ("Server".into(), Kind::Struct, 2),
                ("Store".into(), Kind::Interface, 3),
                ("ID".into(), Kind::Type, 4),
                ("ErrX".into(), Kind::Variable, 5),
                ("Limit".into(), Kind::Constant, 7),
            ]
        );
    }

    #[test]
    fn python_definitions_skip_docstrings_and_locals() {
        let text = "import os\nVERSION = '1'\nclass Repo:\n    \"\"\"\n    def not_this(): pass\n    \"\"\"\n    async def fetch(self):\n        local = 1\n# def nope(): pass\ndef main(): pass";
        assert_eq!(
            names(Language::Python, text),
            vec![
                ("VERSION".into(), Kind::Variable, 1),
                ("Repo".into(), Kind::Class, 2),
                ("fetch".into(), Kind::Function, 6),
                ("main".into(), Kind::Function, 9)
            ]
        );
    }

    #[test]
    fn typescript_declarations_and_methods() {
        let text = "export default async function handler() {}\nexport interface User {}\ntype Id = string;\nexport const api = {};\nexport class Store {\n  private cache = new Map();\n  async load(id: string): Promise<User> {\n    if (x) {\n    const inner = 1;\n  }\n  onClick = (e) => {}\n}\nenum Color { Red }";
        assert_eq!(
            names(Language::TypeScript, text),
            vec![
                ("handler".into(), Kind::Function, 0),
                ("User".into(), Kind::Interface, 1),
                ("Id".into(), Kind::Type, 2),
                ("api".into(), Kind::Variable, 3),
                ("Store".into(), Kind::Class, 4),
                ("load".into(), Kind::Method, 6),
                ("onClick".into(), Kind::Method, 10),
                ("Color".into(), Kind::Enum, 12),
            ]
        );
    }

    #[test]
    fn typescript_methods_on_one_line() {
        let text = "export class Square implements Shape {\n  area() { return 4; }\n  get side(): number { return 2; }\n  if (x) { y(); }\n  call(a) { b(a) }\n}";
        assert_eq!(
            names(Language::TypeScript, text),
            vec![
                ("Square".into(), Kind::Class, 0),
                ("area".into(), Kind::Method, 1),
                ("side".into(), Kind::Method, 2),
                ("call".into(), Kind::Method, 4),
            ]
        );
    }

    #[test]
    fn java_types_methods_and_fields() {
        let text = "public final class OrderService {\n    private final OrderRepository repository;\n    public OrderService(OrderRepository r) {\n    @Override\n    public List<Order> findAll(int page) {\n        return repository.findAll(page);\n    }\n    if (x) {\n}\ninterface Repo {}\nenum Status {\n    PAID,\n    CANCELLED\n}";
        assert_eq!(
            names(Language::Java, text),
            vec![
                ("OrderService".into(), Kind::Class, 0),
                ("repository".into(), Kind::Field, 1),
                ("OrderService".into(), Kind::Method, 2),
                ("findAll".into(), Kind::Method, 4),
                ("Repo".into(), Kind::Interface, 9),
                ("Status".into(), Kind::Enum, 10),
                ("PAID".into(), Kind::Constant, 11),
                ("CANCELLED".into(), Kind::Constant, 12),
            ]
        );
    }

    #[test]
    fn kotlin_declarations() {
        let text = "data class User(val id: Long)\nobject Registry\ninterface Repo\n@Service\nclass UserService(private val repo: Repo) {\n    private val cache = mutableMapOf<Long, User>()\n    suspend fun find(id: Long): User? {\n        val local = 1\n    }\n}\nfun String.slug(): String = this\ntypealias Ids = List<Long>";
        assert_eq!(
            names(Language::Kotlin, text),
            vec![
                ("User".into(), Kind::Class, 0),
                ("Registry".into(), Kind::Class, 1),
                ("Repo".into(), Kind::Interface, 2),
                ("UserService".into(), Kind::Class, 4),
                ("cache".into(), Kind::Variable, 5),
                ("find".into(), Kind::Function, 6),
                ("slug".into(), Kind::Function, 10),
                ("Ids".into(), Kind::Type, 11),
            ]
        );
    }

    #[test]
    fn css_selectors_variables_and_tailwind_utilities() {
        let text = ":root {\n  --brand: #ff0;\n}\n.btn, .btn-primary:hover {\n  color: #fff;\n}\n#app > .card {}\n@utility tab-4 {\n  tab-size: 4;\n}\n@media (min-width: 640px) {\n  .wide { }\n}";
        assert_eq!(
            names(Language::Css, text),
            vec![
                ("--brand".into(), Kind::Variable, 1),
                ("btn".into(), Kind::Selector, 3),
                ("btn-primary".into(), Kind::Selector, 3),
                ("app".into(), Kind::Id, 6),
                ("card".into(), Kind::Selector, 6),
                ("tab-4".into(), Kind::Function, 7),
                ("wide".into(), Kind::Selector, 11),
            ]
        );
    }

    #[test]
    fn the_cursor_is_inside_its_definitions() {
        let text = "export class Store {\n  cache = 1;\n\n  load(id) {\n    return id;\n  }\n}\n\nfunction after() {}";
        let lines: Vec<&str> = text.split('\n').collect();
        let found = symbols(Language::TypeScript, text);
        let names = |cursor: usize| enclosing(&found, |i| lines[i], cursor).iter().map(|s| s.name.as_str()).collect::<Vec<_>>();
        assert_eq!(names(4), vec!["Store", "load"]);
        assert_eq!(names(5), vec!["Store", "load"], "its closing brace");
        assert_eq!(names(1), vec!["Store"]);
        assert_eq!(names(8), vec!["after"]);
        let py = "class Cart:\n    def total(self):\n        return 1\n\nVALUE = 2";
        let lines: Vec<&str> = py.split('\n').collect();
        let found = symbols(Language::Python, py);
        let names = |cursor: usize| enclosing(&found, |i| lines[i], cursor).iter().map(|s| s.name.as_str()).collect::<Vec<_>>();
        assert_eq!(names(2), vec!["Cart", "total"]);
        assert!(names(4).is_empty(), "back at the top level");
    }

    #[test]
    fn html_ids() {
        assert_eq!(
            names(Language::Html, r#"<div id="main"><a id='go-home'>"#),
            vec![("main".into(), Kind::Id, 0), ("go-home".into(), Kind::Id, 0)]
        );
    }

    #[test]
    fn names_under_the_cursor() {
        let line = "let total = order.sumPrice(items);";
        assert_eq!(name_at(line, 20, Language::TypeScript).map(|(s, e)| &line[s..e]), Some("sumPrice"));
        assert_eq!(name_at(line, 26, Language::TypeScript).map(|(s, e)| &line[s..e]), Some("sumPrice"), "right after the name");
        assert_eq!(name_at(line, 27, Language::TypeScript).map(|(s, e)| &line[s..e]), Some("items"));
        assert_eq!(name_at("a + b", 2, Language::Rust), None);
        let html = r#"<div class="btn-primary mt-4">"#;
        assert_eq!(name_at(html, 15, Language::Html).map(|(s, e)| &html[s..e]), Some("btn-primary"));
        assert_eq!(name_at("--", 1, Language::Css), None);
    }

    #[test]
    fn occurrences_are_whole_names() {
        let text = "user = User()\nusers = [user]\nprint(user.name)";
        assert_eq!(occurrences(text, "user", Language::Python, 100), vec![(0, 0), (1, 9), (2, 6)]);
        assert_eq!(occurrences(".btn .btn-x", "btn", Language::Css, 100), vec![(0, 1)]);
    }

    #[test]
    fn ranking_prefers_this_file_types_and_nearby_folders() {
        let (a, b, c) = (
            Symbol { name: "User".into(), kind: Kind::Variable, line: 3, col: 0 },
            Symbol { name: "User".into(), kind: Kind::Class, line: 1, col: 0 },
            Symbol { name: "User".into(), kind: Kind::Class, line: 1, col: 0 },
        );
        let mut list = vec![("web/far/x.ts", &c), ("src/model/user.ts", &b), ("src/api/user.ts", &a)];
        rank(&mut list, "src/api/handler.ts");
        assert_eq!(list.iter().map(|(p, _)| *p).collect::<Vec<_>>(), vec!["src/model/user.ts", "web/far/x.ts", "src/api/user.ts"]);
        rank(&mut list, "src/api/user.ts");
        assert_eq!(list[0].0, "src/api/user.ts", "the file itself first");
    }

    #[test]
    fn fuzzy_matching_prefers_file_names_and_word_starts() {
        assert!(fuzzy_score("src/editor/mod.rs", "xyz").is_none());
        let name = fuzzy_score("src/order/OrderService.kt", "ordserv").unwrap();
        let scattered = fuzzy_score("src/old/random/dserv/v.kt", "ordserv").unwrap();
        assert!(name > scattered);
        assert!(fuzzy_score("src/user.ts", "user").unwrap() > fuzzy_score("src/user/types/index.ts", "user").unwrap());
    }

    #[test]
    fn import_strings_and_modules_lead_to_files() {
        let root = std::env::temp_dir().join(format!("agentty-nav-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src/lib")).unwrap();
        std::fs::create_dir_all(root.join("pkg")).unwrap();
        for file in ["src/lib/api.ts", "src/lib/index.ts", "src/net.rs", "pkg/util.py", "pkg/__init__.py"] {
            std::fs::write(root.join(file), "").unwrap();
        }
        let src = root.join("src");
        let line = "import { get } from './lib/api';";
        assert_eq!(linked_file(line, 25, Language::TypeScript, &src, &root), Some(root.join("src/lib/api.ts")));
        let line = "import x from \"./lib\";";
        assert_eq!(linked_file(line, 17, Language::TypeScript, &src, &root), Some(root.join("src/lib/index.ts")));
        assert_eq!(linked_file("mod net;", 5, Language::Rust, &src, &root), Some(root.join("src/net.rs")));
        assert_eq!(linked_file("from pkg.util import x", 7, Language::Python, &src, &root), Some(root.join("pkg/util.py")));
        assert_eq!(linked_file("import pkg", 8, Language::Python, &src, &root), Some(root.join("pkg/__init__.py")));
        std::fs::create_dir_all(root.join("py/shop")).unwrap();
        std::fs::write(root.join("py/shop/pricing.py"), "").unwrap();
        let shop = root.join("py/shop");
        assert_eq!(
            linked_file("from shop.pricing import price_of", 12, Language::Python, &shop, &root),
            Some(root.join("py/shop/pricing.py"))
        );
        assert_eq!(linked_file("const a = 'hello';", 12, Language::TypeScript, &src, &root), None);
        let _ = std::fs::remove_dir_all(&root);
    }
}
