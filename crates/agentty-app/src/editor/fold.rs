//! Folded regions of a file. A region starts at a line whose next lines are indented deeper
//! (as VS Code folds by indentation): folded, those lines are hidden and the first one shows `⋯`
//! after it. The closing line (`}`, `</div>`) stays in view. Rows are the lines as shown.

use super::buffer::Splice;

/// Lines looked through for the end of a region at most.
const SCAN: usize = 100_000;
/// Lines above the cursor looked through for the region it is in.
const SCAN_UP: usize = 10_000;
/// Blank lines skipped looking for the line after a region's first one.
const BLANKS: usize = 64;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Folds {
    /// Folded regions (first line, last hidden line), by first line; they may nest.
    ranges: Vec<(usize, usize)>,
    /// The hidden lines, as sorted runs that do not overlap.
    hidden: Vec<(usize, usize)>,
}

fn indent(line: &str) -> Option<usize> {
    if line.trim().is_empty() {
        return None;
    }
    Some(line.chars().take_while(|c| c.is_whitespace()).map(|c| if c == '\t' { super::buffer::TAB_WIDTH } else { 1 }).sum())
}

/// Whether the lines after `index` are indented deeper than it (the next line with text).
pub fn foldable<'a>(line: impl Fn(usize) -> &'a str, index: usize, count: usize) -> bool {
    let Some(base) = indent(line(index)) else { return false };
    (index + 1..count.min(index + 1 + BLANKS)).find_map(|i| indent(line(i))).is_some_and(|next| next > base)
}

/// The last line of the region starting at `index`: the last line with text indented deeper
/// than it, before the first one that is not. `None` when nothing after it is deeper.
pub fn region_end<'a>(line: &impl Fn(usize) -> &'a str, index: usize, count: usize) -> Option<usize> {
    let base = indent(line(index))?;
    let mut end = None;
    for i in index + 1..count.min(index + 1 + SCAN) {
        match indent(line(i)) {
            Some(depth) if depth > base => end = Some(i),
            Some(_) => break,
            None => {}
        }
    }
    end
}

/// The innermost region with line `index` in it or starting there, `taken` ones (folded already)
/// passed over for the one around them.
pub fn enclosing<'a>(
    line: impl Fn(usize) -> &'a str,
    index: usize,
    count: usize,
    taken: impl Fn((usize, usize)) -> bool,
) -> Option<(usize, usize)> {
    // Only a line indented less than the last one tried can start a region around it.
    let mut below = usize::MAX;
    for start in (index.saturating_sub(SCAN_UP)..=index).rev() {
        let Some(depth) = indent(line(start)) else { continue };
        if depth >= below {
            continue;
        }
        if let Some(end) = region_end(&line, start, count).filter(|end| *end >= index) {
            if !taken((start, end)) {
                return Some((start, end));
            }
        }
        below = depth;
    }
    None
}

/// The row line `line` is shown on, given the hidden runs (a hidden line: its region's first).
pub fn row_of(hidden: &[(usize, usize)], line: usize) -> usize {
    let mut before = 0;
    for &(from, to) in hidden {
        if from > line {
            break;
        }
        if line <= to {
            return from - 1 - before;
        }
        before += to - from + 1;
    }
    line - before
}

/// The line shown on `row`.
pub fn line_of(hidden: &[(usize, usize)], row: usize) -> usize {
    let mut line = row;
    for &(from, to) in hidden {
        if from > line {
            break;
        }
        line += to - from + 1;
    }
    line
}

impl Folds {
    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    pub fn hidden(&self) -> &[(usize, usize)] {
        &self.hidden
    }

    pub fn ranges(&self) -> &[(usize, usize)] {
        &self.ranges
    }

    pub fn is_hidden(&self, line: usize) -> bool {
        self.hidden.iter().any(|&(from, to)| from <= line && line <= to)
    }

    /// `line` is the first line of a folded region (shown with `⋯`).
    pub fn folded_at(&self, line: usize) -> bool {
        self.ranges.iter().any(|&(start, _)| start == line) && !self.is_hidden(line)
    }

    pub fn row_of(&self, line: usize) -> usize {
        row_of(&self.hidden, line)
    }

    pub fn line_of(&self, row: usize) -> usize {
        line_of(&self.hidden, row)
    }

    /// Rows a text of `count` lines takes.
    pub fn rows(&self, count: usize) -> usize {
        count.saturating_sub(self.hidden.iter().map(|(from, to)| to - from + 1).sum::<usize>()).max(1)
    }

    fn refresh(&mut self) {
        self.ranges.sort_unstable();
        self.ranges.dedup();
        let mut hidden: Vec<(usize, usize)> = Vec::new();
        for &(start, end) in &self.ranges {
            match hidden.last_mut() {
                Some(last) if start <= last.1 => last.1 = last.1.max(end),
                _ => hidden.push((start + 1, end)),
            }
        }
        self.hidden = hidden;
    }

    /// The text changed in ways not followed (too many at once): regions that no longer fit go.
    pub fn keep_within(&mut self, count: usize) {
        let before = self.ranges.len();
        self.ranges.retain(|&(_, end)| end < count);
        if self.ranges.len() != before {
            self.refresh();
        }
    }

    pub fn fold(&mut self, start: usize, end: usize) {
        if end > start {
            self.ranges.push((start, end));
            self.refresh();
        }
    }

    /// Unfolds the region starting at `line`; false when none does.
    pub fn unfold(&mut self, line: usize) -> bool {
        let before = self.ranges.len();
        self.ranges.retain(|&(start, _)| start != line);
        self.refresh();
        self.ranges.len() != before
    }

    /// The innermost folded region with `line` in it or starting there: unfolded.
    pub fn unfold_around(&mut self, line: usize) -> bool {
        let inner = self.ranges.iter().filter(|&&(start, end)| start <= line && line <= end).max_by_key(|(start, _)| *start).copied();
        match inner {
            Some((start, _)) => self.unfold(start),
            None => false,
        }
    }

    /// Unfolds what hides `line` (the cursor went there), but a region whose first line the
    /// selection takes in from line `from` on (a folded line selected whole) stays folded.
    pub fn reveal(&mut self, line: usize, from: usize) -> bool {
        if !self.is_hidden(line) {
            return false;
        }
        self.ranges.retain(|&(start, end)| !(start < line && line <= end && start < from));
        self.refresh();
        true
    }

    /// Follows an edit: regions above or below it move with their lines, a region whose hidden
    /// lines it touched unfolds (an edit on a region's first line alone keeps it).
    pub fn apply(&mut self, splice: Splice) {
        if self.is_empty() {
            return;
        }
        let old_to = (splice.to as isize - splice.delta).max(splice.from as isize) as usize;
        self.ranges.retain_mut(|(start, end)| {
            // Lines removed above it end on its first line (a splice takes in the line after what
            // it removed): it moves up with that line.
            if old_to < *start || (splice.delta < 0 && old_to == *start && splice.from < *start) {
                *start = (*start as isize + splice.delta) as usize;
                *end = (*end as isize + splice.delta) as usize;
                true
            } else {
                splice.from > *end || (splice.from == *start && old_to == *start && splice.delta == 0)
            }
        });
        self.refresh();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CODE: &str = "class A {\n    void f() {\n        x();\n\n        y();\n    }\n    void g() {}\n}\nend";

    fn lines() -> Vec<&'static str> {
        CODE.split('\n').collect()
    }

    #[test]
    fn regions_follow_the_indentation() {
        let lines = lines();
        let at = |i: usize| lines[i];
        let n = lines.len();
        assert!(foldable(at, 0, n) && foldable(at, 1, n));
        assert!(!foldable(at, 2, n) && !foldable(at, 6, n) && !foldable(at, 3, n), "a line, a one-line method, a blank line");
        assert_eq!(region_end(&at, 0, n), Some(6), "the closing brace stays out");
        assert_eq!(region_end(&at, 1, n), Some(4), "the blank line inside is in, the closing brace is not");
        assert_eq!(region_end(&at, 7, n), None);
        // From inside the method: the method, then (folded already) the class around it.
        assert_eq!(enclosing(at, 2, n, |_| false), Some((1, 4)));
        assert_eq!(enclosing(at, 2, n, |r| r == (1, 4)), Some((0, 6)));
        assert_eq!(enclosing(at, 1, n, |_| false), Some((1, 4)), "a region's own first line");
        assert_eq!(enclosing(at, 6, n, |_| false), Some((0, 6)), "a one-line method is in the class");
        assert_eq!(enclosing(at, 8, n, |_| false), None);
    }

    #[test]
    fn rows_skip_the_hidden_lines() {
        let mut folds = Folds::default();
        folds.fold(1, 4);
        assert_eq!(folds.hidden(), [(2, 4)]);
        assert_eq!((0..9).map(|l| folds.row_of(l)).collect::<Vec<_>>(), [0, 1, 1, 1, 1, 2, 3, 4, 5]);
        assert_eq!((0..6).map(|r| folds.line_of(r)).collect::<Vec<_>>(), [0, 1, 5, 6, 7, 8]);
        assert_eq!(folds.rows(9), 6);
        assert!(folds.folded_at(1) && folds.is_hidden(3) && !folds.is_hidden(5));
        // The outer one folded too: the inner stays folded inside it.
        folds.fold(0, 6);
        assert_eq!(folds.hidden(), [(1, 6)]);
        assert!(!folds.folded_at(1), "hidden itself");
        assert!(folds.unfold(0));
        assert_eq!(folds.hidden(), [(2, 4)]);
        assert!(!folds.reveal(1, 1));
        assert!(folds.reveal(3, 1) && !folds.is_empty(), "selected from its first line on");
        assert!(folds.reveal(3, 3) && folds.is_empty());
    }

    #[test]
    fn regions_move_with_edits_and_open_when_touched() {
        let mut folds = Folds::default();
        folds.fold(10, 20);
        // Two lines added above.
        folds.apply(Splice { from: 2, to: 4, delta: 2 });
        assert_eq!(folds.ranges(), [(12, 22)]);
        // Typing on its first line keeps it; below it changes nothing.
        folds.apply(Splice { from: 12, to: 12, delta: 0 });
        folds.apply(Splice { from: 30, to: 31, delta: 1 });
        assert_eq!(folds.ranges(), [(12, 22)]);
        // The line just above it deleted (⇧⌘K): it moves up.
        folds.apply(Splice { from: 11, to: 11, delta: -1 });
        assert_eq!(folds.ranges(), [(11, 21)]);
        // An edit inside opens it.
        folds.apply(Splice { from: 15, to: 15, delta: 0 });
        assert!(folds.is_empty());
    }
}
