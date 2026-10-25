//! Spelling fixes the editor makes by itself: a problem that says a name is not known (`Cannot
//! find name 'onlyInStok'`, `Unresolved reference: ordrs`, `Unknown property: 'colr'`) offers the
//! names of the file and the project spelled closest to it, as "Change to 'onlyInStock'" next to
//! the server's own fixes, without asking the server or an agent.

use super::buffer::Pos;
use super::language::Language;
use super::lsp::{self, Action, Problem};
use super::{complete, project_index, symbols};
use std::path::Path;

/// Names of the project looked at, at most.
const NAMES: usize = 5_000;
/// Names offered for one problem, at most.
const OFFERED: usize = 3;

/// Whether a server's message says a name is not known (each words it its own way).
pub fn says_unknown(message: &str) -> bool {
    const SAID: &[&str] = &[
        "cannot find name",
        "cannot find symbol",
        "cannot find value",
        "cannot find type",
        "cannot find function",
        "unresolved reference",
        "cannot be resolved",
        "is not defined",
        "undefined:",
        "undefined name",
        "unknown property",
        "not found in this scope",
        "does not exist on type",
        "has no attribute",
        "is not a known",
    ];
    let message = message.to_lowercase();
    SAID.iter().any(|said| message.contains(said))
}

/// The edits between two names, a swap of neighbours counting as one (`ordrs` → `orders`: 1).
fn distance(a: &[char], b: &[char]) -> usize {
    let mut rows = vec![(0..=b.len()).collect::<Vec<_>>()];
    for i in 1..=a.len() {
        let mut row = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            row[j] = (rows[i - 1][j] + 1).min(row[j - 1] + 1).min(rows[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                row[j] = row[j].min(rows[i - 2][j - 2] + 1);
            }
        }
        rows.push(row);
    }
    rows[a.len()][b.len()]
}

/// The names `name` is most likely a misspelling of, closest first: within one edit for a short
/// name, two for a longer one, three for a long one.
pub fn closest(name: &str, candidates: impl IntoIterator<Item = String>) -> Vec<String> {
    let wanted: Vec<char> = name.chars().collect();
    let most = match wanted.len() {
        0..=3 => return Vec::new(),
        4..=6 => 1,
        7..=11 => 2,
        _ => 3,
    };
    let mut found: Vec<(usize, String)> = Vec::new();
    for candidate in candidates {
        if candidate == name || found.iter().any(|(_, c)| *c == candidate) {
            continue;
        }
        let chars: Vec<char> = candidate.chars().collect();
        if chars.len().abs_diff(wanted.len()) > most {
            continue;
        }
        let d = distance(&wanted, &chars);
        if d <= most {
            found.push((d, candidate));
        }
    }
    // Closest first; on a tie, a letter left out (the more common slip) before one too many.
    let key = |(d, c): &(usize, String)| (*d, c.len() < name.len(), c.len().abs_diff(name.len()), c.clone());
    found.sort_by_key(key);
    found.into_iter().take(OFFERED).map(|(_, c)| c).collect()
}

/// Where the problem's unknown name is in `text`, and the names it could be changed to.
pub fn suggestions(problem: &Problem, text: &str, project: &Path, language: Language) -> Option<((Pos, Pos), Vec<String>)> {
    if problem.start.line != problem.end.line || !says_unknown(&problem.message) {
        return None;
    }
    let line = text.split('\n').nth(problem.start.line)?;
    let name = line.get(problem.start.col..problem.end.col)?;
    if name.is_empty() || !name.chars().all(|c| symbols::is_name_char(c, language) || c == '-') {
        return None;
    }
    let mut candidates = complete::words(text, language, Some((problem.start.line, problem.start.col, problem.end.col)));
    if !project.as_os_str().is_empty() {
        candidates.extend(project_index::names(project, language, NAMES).into_iter().map(|(name, _)| name));
    }
    let names = closest(name, candidates);
    (!names.is_empty()).then_some(((problem.start, problem.end), names))
}

/// The fixes for `found` in `path`, leaving out a name one of the server's `fixes` already puts
/// in.
pub fn actions(path: &Path, found: Option<((Pos, Pos), Vec<String>)>, fixes: &[Action], cx: &gpui::App) -> Vec<Action> {
    let Some(((start, end), names)) = found else { return Vec::new() };
    names
        .into_iter()
        .filter(|name| !fixes.iter().any(|f| f.title.contains(&format!("'{name}'")) || f.title.contains(&format!("\"{name}\""))))
        .filter_map(|name| {
            let title = crate::i18n::tf(cx, "editor.change_to", &[("name", &name)]);
            lsp::local_fix(&title, path, (start.line, start.col), (end.line, end.col), &name)
        })
        .collect()
}

/// The server's actions with the editor's spelling fixes (`own`) among them: after the fixes the
/// server prefers, before its other ones. A name typed wrong is fixed by spelling it right more
/// often than by what a server offers next (Kotlin's "Remove invocation", say).
pub fn merged(found: Vec<Action>, own: Vec<Action>) -> Vec<Action> {
    in_order(found, own, |a| a.fixes && a.preferred)
}

fn in_order<T>(found: Vec<T>, own: Vec<T>, preferred: impl Fn(&T) -> bool) -> Vec<T> {
    let (mut out, rest): (Vec<T>, Vec<T>) = found.into_iter().partition(|a| preferred(a));
    out.extend(own);
    out.extend(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spelling_fixes_come_after_the_preferred_ones() {
        let order = in_order(vec![(1, false), (2, true), (3, false)], vec![(9, false)], |a| a.1);
        assert_eq!(order.iter().map(|a| a.0).collect::<Vec<_>>(), [2, 9, 1, 3]);
    }

    #[test]
    fn names_spelled_close() {
        let names = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(closest("onlyInStok", names(&["onlyInStock", "setOnlyInStock", "only"])), ["onlyInStock"]);
        assert_eq!(closest("ordrs", names(&["orders", "order", "nextId"])), ["orders"], "order is two edits away");
        assert_eq!(closest("colr", names(&["color", "column", "col"])), ["color", "col"]);
        assert_eq!(closest("lenght", names(&["length"])), ["length"], "a swap is one edit");
        assert_eq!(closest("Total", names(&["total"])), ["total"], "the case alone");
        assert!(closest("abc", names(&["abd"])).is_empty(), "too short to guess");
        assert!(closest("customer", names(&["counter", "custom"])).len() <= 3);
        assert!(closest("zzzzzz", names(&["orders"])).is_empty());
    }

    #[test]
    fn messages_that_say_a_name_is_unknown() {
        for message in [
            "Cannot find name 'onlyInStok'. Did you mean 'onlyInStock'?",
            "Unresolved reference: ordrs",
            "ordrs cannot be resolved to a variable",
            "\"totl\" is not defined",
            "undefined: Totl",
            "cannot find value `totl` in this scope",
            "Unknown property: 'colr'",
        ] {
            assert!(says_unknown(message), "{message}");
        }
        assert!(!says_unknown("Type mismatch: cannot convert from int to String"));
    }
}
