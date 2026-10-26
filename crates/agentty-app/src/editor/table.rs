//! Delimited text (CSV, TSV) and spreadsheets read as a table: rows of cells, each knowing where
//! it is in the text, so an edit in the grid changes only that cell's characters and leaves the
//! rest of the file — its quoting, its spacing — exactly as it was. Plain data, no UI: the grid
//! view (`table_view.rs`) shows a [`Table`] and turns what the user does into [`Edit`]s on the text.

use super::buffer::Pos;
use std::ops::Range;
use std::path::Path;

/// Delimiters the grid offers, in the order the menu lists them.
pub const DELIMITERS: [char; 4] = [',', ';', '\t', '|'];

#[derive(Debug, Default, Clone)]
pub struct Table {
    pub rows: Vec<Row>,
    /// Cells of the longest row.
    pub width: usize,
    pub delimiter: char,
    /// The text the table was read from (empty for a spreadsheet, which is not edited as text).
    source: String,
    /// Byte offset where each line of `source` starts.
    line_starts: Vec<usize>,
}

#[derive(Debug, Default, Clone)]
pub struct Row {
    pub cells: Vec<Cell>,
    /// Where the row's text starts and ends (without its line break).
    pub start: Pos,
    pub end: Pos,
}

#[derive(Debug, Default, Clone)]
pub struct Cell {
    /// The value, quotes taken off.
    pub value: String,
    pub start: Pos,
    pub end: Pos,
    /// The cell's text as written, in `source`.
    raw: Range<usize>,
}

/// One change to the text: `start..end` becomes `text`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub start: Pos,
    pub end: Pos,
    pub text: String,
}

/// Files the grid opens as text it can edit.
pub fn is_delimited(path: &Path) -> bool {
    matches!(extension(path).as_deref(), Some("csv" | "tsv" | "tab" | "psv"))
}

/// Spreadsheets read through calamine (shown, not edited).
pub fn is_spreadsheet(path: &Path) -> bool {
    matches!(extension(path).as_deref(), Some("xlsx" | "xlsm" | "xlsb" | "xls" | "ods"))
}

fn extension(path: &Path) -> Option<String> {
    path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase())
}

/// The delimiter a file uses: a `.tsv` is tab separated; otherwise the candidate that splits the
/// first lines into the same number of cells, more than one, wins (a comma when nothing does).
pub fn detect_delimiter(path: &Path, text: &str) -> char {
    match extension(path).as_deref() {
        Some("tsv" | "tab") => return '\t',
        Some("psv") => return '|',
        _ => {}
    }
    let sample: String = text.lines().take(20).collect::<Vec<_>>().join("\n");
    let mut best = (',', 0usize, false);
    for delimiter in DELIMITERS {
        let table = parse(&sample, delimiter);
        let counts: Vec<usize> = table.rows.iter().map(|r| r.cells.len()).filter(|n| *n > 1).collect();
        let Some(first) = counts.first().copied() else { continue };
        let steady = counts.iter().all(|n| *n == first) && counts.len() == table.rows.len();
        // A delimiter that splits every line alike beats one that splits into more cells unevenly.
        if (steady, first) > (best.2, best.1) {
            best = (delimiter, first, steady);
        }
    }
    best.0
}

/// Reads `text` as rows of cells separated by `delimiter` (RFC 4180: a cell in double quotes may
/// hold the delimiter, line breaks and `""` for a quote). A last line break ends the last row
/// rather than starting an empty one. Malformed quoting is read leniently, never refused.
pub fn parse(text: &str, delimiter: char) -> Table {
    let mut line_starts = vec![0];
    let mut rows = Vec::new();
    let mut cells = Vec::new();
    let mut value = String::new();
    let (mut line, mut col) = (0usize, 0usize);
    let mut row_start = Pos::default();
    let mut cell_start = (Pos::default(), 0usize);
    let mut in_quotes = false;
    let mut chars = text.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        let here = Pos::new(line, col);
        if c == '\n' {
            line += 1;
            col = 0;
            line_starts.push(at + 1);
        } else {
            col += c.len_utf8();
        }
        if in_quotes {
            if c == '"' {
                if chars.peek().is_some_and(|(_, next)| *next == '"') {
                    chars.next();
                    col += 1;
                    value.push('"');
                } else {
                    in_quotes = false;
                }
            } else {
                value.push(c);
            }
            continue;
        }
        if c == delimiter {
            cells.push(Cell { value: std::mem::take(&mut value), start: cell_start.0, end: here, raw: cell_start.1..at });
            cell_start = (Pos::new(line, col), at + c.len_utf8());
        } else if c == '\n' {
            cells.push(Cell { value: std::mem::take(&mut value), start: cell_start.0, end: here, raw: cell_start.1..at });
            rows.push(Row { cells: std::mem::take(&mut cells), start: row_start, end: here });
            row_start = Pos::new(line, 0);
            cell_start = (row_start, at + 1);
        } else if c == '"' && here == cell_start.0 {
            in_quotes = true;
        } else if c != '\r' {
            value.push(c);
        }
    }
    let end = Pos::new(line, col);
    if end != row_start {
        cells.push(Cell { value, start: cell_start.0, end, raw: cell_start.1..text.len() });
        rows.push(Row { cells, start: row_start, end });
    }
    let width = rows.iter().map(|r| r.cells.len()).max().unwrap_or(0);
    Table { rows, width, delimiter, source: text.to_string(), line_starts }
}

/// A value as a cell's text: quoted when it holds the delimiter, a quote or a line break.
pub fn quote(value: &str, delimiter: char) -> String {
    if value.contains([delimiter, '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

impl Table {
    /// A spreadsheet's sheet as a table (rows of values, not tied to any text).
    pub fn from_values(values: Vec<Vec<String>>) -> Self {
        let rows: Vec<Row> = values
            .into_iter()
            .map(|cells| Row { cells: cells.into_iter().map(|value| Cell { value, ..Default::default() }).collect(), ..Default::default() })
            .collect();
        let width = rows.iter().map(|r| r.cells.len()).max().unwrap_or(0);
        Self { rows, width, delimiter: ',', ..Default::default() }
    }

    /// Sets a value of a spreadsheet's table (one not tied to text), lengthening it as needed.
    pub fn set_value(&mut self, row: usize, col: usize, value: String) {
        while self.rows.len() <= row {
            self.rows.push(Row::default());
        }
        let cells = &mut self.rows[row].cells;
        while cells.len() <= col {
            cells.push(Cell::default());
        }
        cells[col].value = value;
        self.width = self.width.max(col + 1);
    }

    pub fn value(&self, row: usize, col: usize) -> &str {
        self.rows.get(row).and_then(|r| r.cells.get(col)).map_or("", |c| c.value.as_str())
    }

    /// Where the text ends: rows added at the end go here.
    fn end(&self) -> Pos {
        let line = self.line_starts.len() - 1;
        Pos::new(line, self.source.len() - self.line_starts[line])
    }

    fn offset(&self, pos: Pos) -> usize {
        self.line_starts.get(pos.line).map_or(self.source.len(), |start| start + pos.col)
    }

    /// Sets cells to new values (row, column, value). A cell past the end of its row lengthens
    /// the row; a row past the end of the table is added. Untouched cells keep their text.
    pub fn set_cells(&self, changes: &[(usize, usize, String)]) -> Vec<Edit> {
        let d = self.delimiter;
        let mut by_row: std::collections::BTreeMap<usize, Vec<(usize, &str)>> = Default::default();
        for (row, col, value) in changes {
            by_row.entry(*row).or_default().push((*col, value));
        }
        let mut edits = Vec::new();
        let mut added: Vec<(usize, String)> = Vec::new();
        for (index, mut cells) in by_row {
            cells.sort_by_key(|(col, _)| *col);
            cells.dedup_by_key(|(col, _)| *col);
            let Some(row) = self.rows.get(index) else {
                let width = cells.last().map_or(0, |(col, _)| col + 1);
                let mut values = vec![String::new(); width];
                for (col, value) in cells {
                    values[col] = quote(value, d);
                }
                added.push((index, values.join(&d.to_string())));
                continue;
            };
            if cells.iter().all(|(col, _)| *col < row.cells.len()) {
                for (col, value) in cells.iter().rev() {
                    let cell = &row.cells[*col];
                    edits.push(Edit { start: cell.start, end: cell.end, text: quote(value, d) });
                }
                continue;
            }
            // Longer than the row is: the row is written again, its other cells as they were.
            let width = row.cells.len().max(cells.last().map_or(0, |(col, _)| col + 1));
            let text: Vec<String> = (0..width)
                .map(|col| match cells.iter().find(|(c, _)| *c == col) {
                    Some((_, value)) => quote(value, d),
                    None => row.cells.get(col).map_or(String::new(), |cell| self.source[cell.raw.clone()].to_string()),
                })
                .collect();
            edits.push(Edit { start: row.start, end: row.end, text: text.join(&d.to_string()) });
        }
        if !added.is_empty() {
            let last = added.last().map_or(0, |(i, _)| *i);
            let lines: Vec<String> = (self.rows.len()..=last)
                .map(|i| added.iter().find(|(r, _)| *r == i).map_or(String::new(), |(_, text)| text.clone()))
                .collect();
            edits.push(self.append(lines.join("\n")));
        }
        edits.sort_by_key(|e| std::cmp::Reverse(e.start));
        edits
    }

    /// Lines added after the last row, keeping a last line break where there was one.
    fn append(&self, lines: String) -> Edit {
        let end = self.end();
        let text = if self.source.is_empty() {
            lines
        } else if self.source.ends_with('\n') {
            format!("{lines}\n")
        } else {
            format!("\n{lines}")
        };
        Edit { start: end, end, text }
    }

    /// An empty row put at `index` (after the last one when `index` is past the end).
    pub fn insert_row(&self, index: usize) -> Edit {
        let empty = self.delimiter.to_string().repeat(self.width.saturating_sub(1));
        match self.rows.get(index) {
            Some(row) => Edit { start: row.start, end: row.start, text: format!("{empty}\n") },
            None => self.append(empty),
        }
    }

    /// Removes rows (indexes in any order), each with its line break.
    pub fn delete_rows(&self, indexes: &[usize]) -> Vec<Edit> {
        let mut indexes: Vec<usize> = indexes.iter().copied().filter(|i| *i < self.rows.len()).collect();
        indexes.sort_unstable();
        indexes.dedup();
        let mut edits = Vec::new();
        // Runs of neighbouring rows go in one edit: their ranges would touch otherwise.
        let mut i = 0;
        while i < indexes.len() {
            let first = indexes[i];
            let mut last = first;
            while i + 1 < indexes.len() && indexes[i + 1] == last + 1 {
                i += 1;
                last = indexes[i];
            }
            i += 1;
            let edit = if let Some(next) = self.rows.get(last + 1) {
                Edit { start: self.rows[first].start, end: next.start, text: String::new() }
            } else if first > 0 {
                Edit { start: self.rows[first - 1].end, end: self.rows[last].end, text: String::new() }
            } else {
                Edit { start: self.rows[first].start, end: self.end(), text: String::new() }
            };
            edits.push(edit);
        }
        edits.reverse();
        edits
    }

    /// An empty column put at `col` in every row.
    pub fn insert_column(&self, col: usize) -> Vec<Edit> {
        let d = self.delimiter.to_string();
        self.rows
            .iter()
            .rev()
            .filter_map(|row| match row.cells.get(col) {
                Some(cell) => Some(Edit { start: cell.start, end: cell.start, text: d.clone() }),
                None if col == row.cells.len() => Some(Edit { start: row.end, end: row.end, text: d.clone() }),
                None => None,
            })
            .collect()
    }

    /// Takes column `col` out of every row.
    pub fn delete_column(&self, col: usize) -> Vec<Edit> {
        self.rows
            .iter()
            .rev()
            .filter_map(|row| {
                let cells = &row.cells;
                let (start, end) = match (cells.get(col), col) {
                    (None, _) => return None,
                    (Some(cell), _) if cells.len() == 1 => (cell.start, cell.end),
                    (Some(_), _) if col + 1 < cells.len() => (cells[col].start, cells[col + 1].start),
                    (Some(cell), _) => (cells[col - 1].end, cell.end),
                };
                Some(Edit { start, end, text: String::new() })
            })
            .collect()
    }

    /// The text with `edits` made (they come last-first, as every edit here is listed).
    pub fn apply(&self, edits: &[Edit]) -> String {
        let mut text = self.source.clone();
        let mut sorted: Vec<&Edit> = edits.iter().collect();
        sorted.sort_by_key(|e| std::cmp::Reverse(e.start));
        for edit in sorted {
            let (start, end) = (self.offset(edit.start), self.offset(edit.end));
            text.replace_range(start..end, &edit.text);
        }
        text
    }
}

/// Cells as text to copy: tab separated, one row a line, as spreadsheets put them on the clipboard.
pub fn to_clipboard(rows: &[Vec<String>]) -> String {
    rows.iter().map(|row| row.iter().map(|v| quote(v, '\t')).collect::<Vec<_>>().join("\t")).collect::<Vec<_>>().join("\n")
}

/// Text from the clipboard as cells: tab separated when it has tabs (what a spreadsheet copies),
/// otherwise one value per line.
pub fn from_clipboard(text: &str) -> Vec<Vec<String>> {
    let text = text.strip_suffix('\n').unwrap_or(text).replace("\r\n", "\n");
    if !text.contains('\t') {
        return text.split('\n').map(|line| vec![line.to_string()]).collect();
    }
    parse(&text, '\t').rows.into_iter().map(|row| row.cells.into_iter().map(|c| c.value).collect()).collect()
}

/// Orders two values as a spreadsheet sorts: numbers by size before text, text without regard to case.
pub fn compare(a: &str, b: &str) -> std::cmp::Ordering {
    let number = |s: &str| s.trim().replace(',', "").parse::<f64>().ok();
    match (number(a), number(b)) {
        (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.to_lowercase().cmp(&b.to_lowercase()),
    }
}

/// A spreadsheet file's sheets (name and table), read off the UI thread.
pub fn read_workbook(path: &Path) -> Result<Vec<(String, Table)>, String> {
    use calamine::Reader;
    let mut workbook = calamine::open_workbook_auto(path).map_err(|e| e.to_string())?;
    let mut sheets = Vec::new();
    for name in workbook.sheet_names() {
        let range = workbook.worksheet_range(&name).map_err(|e| e.to_string())?;
        // Rows and columns before the first used cell are kept, so A1 stays A1.
        let (top, left) = range.start().map_or((0, 0), |(r, c)| (r as usize, c as usize));
        let mut values: Vec<Vec<String>> = vec![Vec::new(); top];
        values.extend(range.rows().map(|row| {
            let mut cells = vec![String::new(); left];
            cells.extend(row.iter().map(display));
            while cells.last().is_some_and(String::is_empty) {
                cells.pop();
            }
            cells
        }));
        sheets.push((name, Table::from_values(values)));
    }
    Ok(sheets)
}

/// A spreadsheet value as it reads: dates as dates (Excel keeps them as day counts).
fn display(value: &calamine::Data) -> String {
    match value {
        calamine::Data::DateTime(at) if at.is_datetime() => {
            let serial = at.as_f64();
            let base = chrono::NaiveDate::from_ymd_opt(1899, 12, 30).and_then(|d| d.and_hms_opt(0, 0, 0));
            let Some(at) = base.and_then(|b| b.checked_add_signed(chrono::Duration::milliseconds((serial * 86_400_000.).round() as i64)))
            else {
                return value.to_string();
            };
            if serial.fract() == 0. {
                at.format("%Y-%m-%d").to_string()
            } else if serial < 1. {
                at.format("%H:%M:%S").to_string()
            } else {
                at.format("%Y-%m-%d %H:%M:%S").to_string()
            }
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(table: &Table) -> Vec<Vec<&str>> {
        table.rows.iter().map(|r| r.cells.iter().map(|c| c.value.as_str()).collect()).collect()
    }

    #[test]
    fn reads_quoted_cells_and_line_breaks() {
        let table = parse("a,b,c\n\"x, y\",\"say \"\"hi\"\"\",\"two\nlines\"\n", ',');
        assert_eq!(values(&table), vec![vec!["a", "b", "c"], vec!["x, y", "say \"hi\"", "two\nlines"]]);
        assert_eq!(table.width, 3);
        assert_eq!(table.rows[1].end, Pos::new(2, 6));
    }

    #[test]
    fn last_line_without_break_and_empty_cells() {
        let table = parse("a,,c\n,\nlast", ',');
        assert_eq!(values(&table), vec![vec!["a", "", "c"], vec!["", ""], vec!["last"]]);
        assert!(parse("", ',').rows.is_empty());
    }

    #[test]
    fn detects_the_delimiter() {
        assert_eq!(detect_delimiter(Path::new("a.csv"), "a;b;c\n1;2;3\n"), ';');
        assert_eq!(detect_delimiter(Path::new("a.csv"), "a,b\n1,2\n"), ',');
        assert_eq!(detect_delimiter(Path::new("a.txt"), "a\tb\n1\t2\n"), '\t');
        // A comma inside a value does not outvote the semicolons that split every line.
        assert_eq!(detect_delimiter(Path::new("a.csv"), "name;price\nx;1,5\ny;2\n"), ';');
        assert_eq!(detect_delimiter(Path::new("a.tsv"), "a,b"), '\t');
    }

    #[test]
    fn editing_a_cell_touches_only_that_cell() {
        let source = "id, name ,\"q\"\n1,\"Kim, A\",x\n";
        let table = parse(source, ',');
        let edits = table.set_cells(&[(1, 1, "Lee".into())]);
        assert_eq!(table.apply(&edits), "id, name ,\"q\"\n1,Lee,x\n");
        let edits = table.set_cells(&[(1, 2, "a\"b".into())]);
        assert_eq!(table.apply(&edits), "id, name ,\"q\"\n1,\"Kim, A\",\"a\"\"b\"\n");
    }

    #[test]
    fn editing_past_the_row_or_table_end_grows_it() {
        let table = parse("a,b\n1\n", ',');
        let edits = table.set_cells(&[(1, 2, "z".into()), (3, 1, "new".into())]);
        assert_eq!(table.apply(&edits), "a,b\n1,,z\n\n,new\n");
        let table = parse("a,b", ',');
        assert_eq!(table.apply(&table.set_cells(&[(1, 0, "x".into())])), "a,b\nx");
    }

    #[test]
    fn rows_are_added_and_removed() {
        let table = parse("a,b\n1,2\n3,4\n", ',');
        assert_eq!(table.apply(&[table.insert_row(1)]), "a,b\n,\n1,2\n3,4\n");
        assert_eq!(table.apply(&[table.insert_row(9)]), "a,b\n1,2\n3,4\n,\n");
        assert_eq!(table.apply(&table.delete_rows(&[1])), "a,b\n3,4\n");
        assert_eq!(table.apply(&table.delete_rows(&[2, 1])), "a,b\n");
        let table = parse("a,b\n1,2", ',');
        assert_eq!(table.apply(&table.delete_rows(&[1])), "a,b");
        assert_eq!(table.apply(&table.delete_rows(&[0, 1])), "");
    }

    #[test]
    fn columns_are_added_and_removed() {
        let table = parse("a,b\n1,2\n", ',');
        assert_eq!(table.apply(&table.insert_column(0)), ",a,b\n,1,2\n");
        assert_eq!(table.apply(&table.insert_column(2)), "a,b,\n1,2,\n");
        assert_eq!(table.apply(&table.delete_column(0)), "b\n2\n");
        assert_eq!(table.apply(&table.delete_column(1)), "a\n1\n");
    }

    #[test]
    fn clipboard_round_trip() {
        let rows = vec![vec!["a".to_string(), "b\tc".to_string()], vec!["1".to_string(), String::new()]];
        assert_eq!(from_clipboard(&to_clipboard(&rows)), rows);
        assert_eq!(from_clipboard("one\ntwo\n"), vec![vec!["one".to_string()], vec!["two".to_string()]]);
    }

    #[test]
    fn sorts_numbers_before_text() {
        let mut v = vec!["b", "10", "2", "A", "1,000"];
        v.sort_by(|a, b| compare(a, b));
        assert_eq!(v, vec!["2", "10", "1,000", "A", "b"]);
    }
}
