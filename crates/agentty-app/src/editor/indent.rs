//! The indentation step for a file that shows none of its own yet (a new, empty file): what
//! `.editorconfig` says for it, else what the files of its kind beside it use, as VS Code with
//! EditorConfig does. A file with indented lines keeps its own (see [`detect_indent`]).

use std::path::Path;

use super::buffer::detect_indent;

/// Files of the same kind looked at, at most, and how much of each (of `.editorconfig` too).
const NEIGHBOURS: usize = 12;
const NEIGHBOUR_BYTES: u64 = 256 * 1024;
/// Folders looked through for `.editorconfig`, at most.
const LEVELS: usize = 32;
/// A section's glob is looked at only up to this long, and spelled out into this many patterns
/// at most (`{a,b}{a,b}…` doubles them each time): a repository's `.editorconfig` is not trusted.
const GLOB_BYTES: usize = 256;
const GLOB_PATTERNS: usize = 64;

pub fn for_file(path: &Path, project: &Path) -> Option<String> {
    from_editorconfig(path, project).or_else(|| from_neighbours(path))
}

/// The `indent_style` / `indent_size` that applies to `path`: the nearest `.editorconfig` that
/// sets them wins, up to one marked `root = true` or the project's folder.
fn from_editorconfig(path: &Path, project: &Path) -> Option<String> {
    let (mut style, mut size, mut tab_width) = (None, None, None);
    for dir in path.ancestors().skip(1).take(LEVELS) {
        let file = dir.join(".editorconfig");
        let small = std::fs::metadata(&file).is_ok_and(|meta| meta.is_file() && meta.len() <= NEIGHBOUR_BYTES);
        if let Some(text) = small.then(|| std::fs::read_to_string(&file).ok()).flatten() {
            let relative = path.strip_prefix(dir).ok()?.to_string_lossy().replace('\\', "/");
            let found = parse(&text, &relative);
            style = style.or(found.style);
            size = size.or(found.size);
            tab_width = tab_width.or(found.tab_width);
            if found.root {
                break;
            }
        }
        if dir == project {
            break;
        }
    }
    match style.as_deref() {
        Some("tab") => Some("\t".into()),
        Some("space") => Some(" ".repeat(size.or(tab_width).unwrap_or(4))),
        _ => size.map(|n| " ".repeat(n)),
    }
}

#[derive(Default)]
struct Found {
    root: bool,
    style: Option<String>,
    size: Option<usize>,
    tab_width: Option<usize>,
}

/// What one `.editorconfig` says for the file at `relative` (from its folder); later sections win.
fn parse(text: &str, relative: &str) -> Found {
    let mut found = Found::default();
    let mut applies = false;
    let mut preamble = true;
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(section) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            preamble = false;
            applies = section_matches(section, relative);
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let (key, value) = (key.trim().to_ascii_lowercase(), value.trim().to_ascii_lowercase());
        if preamble {
            found.root |= key == "root" && value == "true";
            continue;
        }
        if !applies {
            continue;
        }
        match key.as_str() {
            "indent_style" => found.style = Some(value),
            "indent_size" if value == "tab" => found.style = Some("tab".into()),
            "indent_size" => found.size = value.parse().ok().filter(|n| (1..=16).contains(n)),
            "tab_width" => found.tab_width = value.parse().ok().filter(|n| (1..=16).contains(n)),
            _ => {}
        }
    }
    found
}

/// A section's glob against the file's path: one without `/` matches the file's name anywhere
/// below, one with `/` the path from the `.editorconfig`'s folder.
fn section_matches(glob: &str, relative: &str) -> bool {
    let glob = glob.trim_start_matches('/');
    if glob.len() > GLOB_BYTES {
        return false;
    }
    let mut patterns = Vec::new();
    expand(glob, &mut patterns);
    patterns.iter().any(|pattern| {
        if pattern.contains('/') {
            matches(pattern.as_bytes(), relative.as_bytes())
        } else {
            let name = relative.rsplit('/').next().unwrap_or(relative);
            matches(pattern.as_bytes(), name.as_bytes())
        }
    })
}

/// `{a,b}` alternatives spelled out (nested ones too) into `out`, [`GLOB_PATTERNS`] at most.
fn expand(glob: &str, out: &mut Vec<String>) {
    if out.len() >= GLOB_PATTERNS {
        return;
    }
    let Some(open) = glob.find('{') else { return out.push(glob.to_string()) };
    let mut depth = 0;
    let mut parts = Vec::new();
    let mut start = open + 1;
    for (i, c) in glob[open..].char_indices().map(|(i, c)| (i + open, c)) {
        match c {
            '{' => depth += 1,
            ',' if depth == 1 => {
                parts.push(&glob[start..i]);
                start = i + 1;
            }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    parts.push(&glob[start..i]);
                    let (head, tail) = (&glob[..open], &glob[i + 1..]);
                    for part in parts {
                        expand(&format!("{head}{part}{tail}"), out);
                    }
                    return;
                }
            }
            _ => {}
        }
    }
    out.push(glob.to_string());
}

/// `*` (not across `/`), `**` (across), `?`, everything else literally. Step by step over the
/// pattern with the set of text positions reached, so no pattern backtracks without end.
fn matches(pattern: &[u8], text: &[u8]) -> bool {
    let mut reached = vec![false; text.len() + 1];
    reached[0] = true;
    let mut i = 0;
    while i < pattern.len() {
        let mut next = vec![false; text.len() + 1];
        match pattern[i] {
            b'*' => {
                let across = pattern.get(i + 1) == Some(&b'*');
                if across {
                    i += 1;
                }
                let mut on = false;
                for j in 0..=text.len() {
                    // Carried on from an earlier reached position, unless a `/` stops a `*`.
                    on = reached[j] || (on && (across || text[j - 1] != b'/'));
                    next[j] = on;
                }
            }
            c => {
                for j in 0..text.len() {
                    next[j + 1] = reached[j] && if c == b'?' { text[j] != b'/' } else { text[j] == c };
                }
            }
        }
        reached = next;
        i += 1;
    }
    reached[text.len()]
}

/// What the other files with the same extension in its folder (else the folder above) use.
fn from_neighbours(path: &Path) -> Option<String> {
    let extension = path.extension()?;
    for dir in path.ancestors().skip(1).take(2) {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        let mut names: Vec<_> =
            entries.flatten().map(|entry| entry.path()).filter(|other| other != path && other.extension() == Some(extension)).collect();
        names.sort();
        for other in names.into_iter().take(NEIGHBOURS) {
            // Not through a link: it may point anywhere.
            if std::fs::symlink_metadata(&other).map_or(true, |meta| !meta.is_file() || meta.len() > NEIGHBOUR_BYTES) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&other) else { continue };
            if let Some(unit) = detect_indent(text.split('\n')) {
                return Some(unit);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editorconfig_sections_match_by_name_or_path() {
        let text = "root = true\n\n[*]\nindent_style = space\nindent_size = 4\n\n[*.{ts,tsx}]\nindent_size = 2\n\n[Makefile]\nindent_style = tab\n\n[docs/**.md]\nindent_size = 3\n";
        let at = |relative| {
            let found = parse(text, relative);
            (found.root, found.style, found.size)
        };
        assert_eq!(at("src/App.tsx"), (true, Some("space".into()), Some(2)));
        assert_eq!(at("src/main.rs"), (true, Some("space".into()), Some(4)));
        assert_eq!(at("Makefile"), (true, Some("tab".into()), Some(4)));
        assert_eq!(at("docs/a/b.md"), (true, Some("space".into()), Some(3)));
        assert_eq!(at("notes/b.md"), (true, Some("space".into()), Some(4)));
        assert!(section_matches("*.{js,{ts,tsx}}", "a/b.tsx") && !section_matches("src/*.ts", "src/a/b.ts"));
        assert!(section_matches("src/**.ts", "src/a/b.ts") && section_matches("Make?ile", "Makefile") && !section_matches("a?b", "a/b"));
    }

    #[test]
    fn a_hostile_editorconfig_glob_takes_no_time() {
        let started = std::time::Instant::now();
        let stars = "**a".repeat(80) + "b";
        assert!(!section_matches(&stars, &"a".repeat(200)));
        let braces = "{a,b}".repeat(50);
        assert!(!section_matches(&braces, "c"));
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }

    #[test]
    fn a_new_file_takes_the_projects_indentation() {
        let dir = std::env::temp_dir().join(format!("agentty-indent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src/components")).unwrap();
        std::fs::write(dir.join("src/components/List.tsx"), "export function List() {\n  return null\n}\n").unwrap();
        std::fs::write(dir.join("src/components/New.tsx"), "").unwrap();
        std::fs::write(dir.join("src/main.go"), "").unwrap();
        // The files beside it.
        assert_eq!(for_file(&dir.join("src/components/New.tsx"), &dir).as_deref(), Some("  "));
        assert_eq!(for_file(&dir.join("src/main.go"), &dir), None, "nothing to go by");
        // `.editorconfig` wins, the nearest one first.
        std::fs::write(dir.join(".editorconfig"), "root = true\n[*]\nindent_style = tab\n").unwrap();
        std::fs::write(dir.join("src/.editorconfig"), "[*.tsx]\nindent_style = space\nindent_size = 4\n").unwrap();
        assert_eq!(for_file(&dir.join("src/components/New.tsx"), &dir).as_deref(), Some("    "));
        assert_eq!(for_file(&dir.join("src/main.go"), &dir).as_deref(), Some("\t"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
