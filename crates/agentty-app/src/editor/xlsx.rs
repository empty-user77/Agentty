//! A spreadsheet open in the grid: its sheets as tables, the cells changed since it was saved, and
//! writing those cells back into the `.xlsx` file. Only the changed cells' XML is rewritten (their
//! style kept); shared strings, formulas elsewhere, charts and formatting stay as they were, and
//! Excel is asked to recalculate when it next opens the file.

use super::ooxml;
use super::table::Table;
use quick_xml::events::Event;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;

/// (sheet, row, column), all from 0.
type Key = (usize, usize, usize);

pub struct Workbook {
    pub sheets: Vec<(String, Arc<Table>)>,
    /// An `.xlsx` / `.xlsm` the user may change (older formats are shown only).
    pub writable: bool,
    /// Cells changed since the last save, with their new values.
    changes: BTreeMap<Key, String>,
    /// What the changed cells held when the file was read or saved.
    original: HashMap<Key, String>,
    undo: Vec<Vec<(Key, String, String)>>,
    redo: Vec<Vec<(Key, String, String)>>,
}

/// Files whose cells can be written back.
pub fn is_writable(path: &Path) -> bool {
    matches!(path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).as_deref(), Some("xlsx" | "xlsm"))
}

impl Workbook {
    pub fn new(sheets: Vec<(String, Arc<Table>)>, writable: bool) -> Self {
        Self { sheets, writable, changes: BTreeMap::new(), original: HashMap::new(), undo: Vec::new(), redo: Vec::new() }
    }

    pub fn is_dirty(&self) -> bool {
        !self.changes.is_empty()
    }

    /// Sets cells of `sheet` (row, column, value) as one undo step.
    pub fn set(&mut self, sheet: usize, cells: &[(usize, usize, String)]) {
        let step: Vec<(Key, String, String)> = cells
            .iter()
            .filter_map(|(row, col, value)| {
                let before = self.value(sheet, *row, *col).to_string();
                (before != *value).then(|| ((sheet, *row, *col), before, value.clone()))
            })
            .collect();
        if step.is_empty() {
            return;
        }
        self.apply(&step, false);
        self.undo.push(step);
        self.redo.clear();
    }

    pub fn undo(&mut self) -> bool {
        let Some(step) = self.undo.pop() else { return false };
        self.apply(&step, true);
        self.redo.push(step);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(step) = self.redo.pop() else { return false };
        self.apply(&step, false);
        self.undo.push(step);
        true
    }

    fn value(&self, sheet: usize, row: usize, col: usize) -> &str {
        self.sheets.get(sheet).map_or("", |(_, t)| t.value(row, col))
    }

    fn apply(&mut self, step: &[(Key, String, String)], back: bool) {
        for (key, before, after) in step {
            let (old, new) = if back { (after, before) } else { (before, after) };
            let original = self.original.entry(*key).or_insert_with(|| old.clone()).clone();
            if *new == original {
                self.changes.remove(key);
            } else {
                self.changes.insert(*key, new.clone());
            }
            if let Some((_, table)) = self.sheets.get_mut(key.0) {
                Arc::make_mut(table).set_value(key.1, key.2, new.clone());
            }
        }
    }

    /// A row added after the last one (empty until something is typed into it).
    pub fn add_row(&mut self, sheet: usize) {
        if let Some((_, table)) = self.sheets.get_mut(sheet) {
            let table = Arc::make_mut(table);
            table.rows.push(Default::default());
        }
    }

    /// The file's bytes with the changed cells written in.
    pub fn render(&self, bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
        let mut by_sheet: BTreeMap<String, BTreeMap<(usize, usize), String>> = BTreeMap::new();
        for ((sheet, row, col), value) in &self.changes {
            if let Some((name, _)) = self.sheets.get(*sheet) {
                by_sheet.entry(name.clone()).or_default().insert((*row, *col), value.clone());
            }
        }
        write_cells(bytes, &by_sheet)
    }

    /// The changed cells are on disk now.
    pub fn mark_saved(&mut self) {
        self.changes.clear();
        self.original.clear();
    }
}

/// The sheets of a workbook package: name → its part (`xl/worksheets/sheet1.xml`).
fn sheet_parts(bytes: &[u8]) -> anyhow::Result<Vec<(String, String)>> {
    let workbook = ooxml::read_part(bytes, "xl/workbook.xml")?.ok_or_else(|| anyhow::anyhow!("not an Excel workbook"))?;
    let rels = ooxml::relationships(bytes, "xl/_rels/workbook.xml.rels", "xl")?;
    let mut reader = quick_xml::Reader::from_str(&workbook);
    let mut sheets = Vec::new();
    loop {
        match reader.read_event()? {
            Event::Empty(e) | Event::Start(e) if ooxml::local(&e) == "sheet" => {
                if let (Some(name), Some(id)) = (ooxml::attribute(&e, "name"), ooxml::attribute(&e, "id")) {
                    if let Some(part) = rels.get(&id) {
                        sheets.push((name, part.clone()));
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(sheets)
}

/// Writes cells (by sheet name: (row, column) → value) into the package.
pub fn write_cells(bytes: &[u8], cells: &BTreeMap<String, BTreeMap<(usize, usize), String>>) -> anyhow::Result<Vec<u8>> {
    let parts = sheet_parts(bytes)?;
    let mut replace = HashMap::new();
    let mut formulas = false;
    for (name, cells) in cells {
        let Some((_, part)) = parts.iter().find(|(n, _)| n == name) else { anyhow::bail!("sheet {name} not found") };
        let xml = ooxml::read_part(bytes, part)?.ok_or_else(|| anyhow::anyhow!("{part} missing"))?;
        let (patched, had_formula) = patch_sheet(&xml, cells)?;
        formulas |= had_formula;
        replace.insert(part.clone(), patched);
    }
    // Values changed under formulas: Excel works them out again when it opens the file.
    if let Some(workbook) = ooxml::read_part(bytes, "xl/workbook.xml")? {
        replace.insert("xl/workbook.xml".into(), recalculate_on_load(&workbook));
    }
    let mut remove = Vec::new();
    // A formula replaced by a value would leave the calculation chain naming it: Excel rebuilds
    // the chain when it is missing, and "repairs" the file when it is wrong.
    if formulas && ooxml::read_part(bytes, "xl/calcChain.xml")?.is_some() {
        remove.push("xl/calcChain.xml".to_string());
        let calc_chain = |text: &str, element: &str| {
            regex::Regex::new(&format!(r#"<{element}\b[^>]*calcChain\.xml"[^>]*/>"#)).map(|re| re.replace_all(text, "").into_owned())
        };
        if let Some(types) = ooxml::read_part(bytes, "[Content_Types].xml")? {
            replace.insert("[Content_Types].xml".into(), calc_chain(&types, "Override")?);
        }
        if let Some(rels) = ooxml::read_part(bytes, "xl/_rels/workbook.xml.rels")? {
            replace.insert("xl/_rels/workbook.xml.rels".into(), calc_chain(&rels, "Relationship")?);
        }
    }
    ooxml::rewrite(bytes, &replace, &remove)
}

/// `workbook.xml` with `fullCalcOnLoad` set (a `calcPr` added in its place in the schema's order).
fn recalculate_on_load(xml: &str) -> String {
    if let Some(at) = xml.find("<calcPr") {
        let end = xml[at..].find('>').map_or(xml.len(), |e| at + e);
        if xml[at..end].contains("fullCalcOnLoad") {
            return xml.to_string();
        }
        return format!("{} fullCalcOnLoad=\"1\"{}", &xml[..at + "<calcPr".len()], &xml[at + "<calcPr".len()..]);
    }
    for after in ["</definedNames>", "</externalReferences>", "</functionGroups>", "</sheets>"] {
        if let Some(at) = xml.find(after) {
            let at = at + after.len();
            return format!("{}<calcPr fullCalcOnLoad=\"1\"/>{}", &xml[..at], &xml[at..]);
        }
    }
    xml.to_string()
}

/// A1-style reference → (row, column) from 0.
fn parse_ref(reference: &str) -> Option<(usize, usize)> {
    let letters: String = reference.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
    let row: usize = reference[letters.len()..].parse().ok()?;
    let col = letters.bytes().try_fold(0usize, |acc, b| Some(acc * 26 + (b.to_ascii_uppercase() - b'A') as usize + 1))?;
    Some((row.checked_sub(1)?, col.checked_sub(1)?))
}

fn cell_ref(row: usize, col: usize) -> String {
    format!("{}{}", super::table_view::column_name(col), row + 1)
}

/// A value as a cell's XML: a number when it reads as one, a date (as Excel's day count) when the
/// cell is styled and it reads as a date, otherwise text in the cell itself.
fn cell_xml(row: usize, col: usize, value: &str, style: Option<&str>) -> String {
    let at = cell_ref(row, col);
    let s = style.map(|s| format!(" s=\"{}\"", ooxml::escape(s))).unwrap_or_default();
    if value.is_empty() {
        return if style.is_some() { format!("<c r=\"{at}\"{s}/>") } else { String::new() };
    }
    if let Some(number) = as_number(value) {
        return format!("<c r=\"{at}\"{s}><v>{number}</v></c>");
    }
    if let (Some(_), Some(serial)) = (style, as_date(value)) {
        return format!("<c r=\"{at}\"{s}><v>{serial}</v></c>");
    }
    format!("<c r=\"{at}\"{s} t=\"inlineStr\"><is><t xml:space=\"preserve\">{}</t></is></c>", ooxml::escape(value))
}

/// A number written as Excel keeps it; text that only looks like one (`007`, `+1`, ` 2`) stays text.
fn as_number(value: &str) -> Option<String> {
    if value.trim() != value || value.starts_with('+') || (value.starts_with('0') && value.len() > 1 && !value.starts_with("0.")) {
        return None;
    }
    if value.starts_with("-0") && value.len() > 2 && !value.starts_with("-0.") {
        return None;
    }
    let number: f64 = value.parse().ok()?;
    number.is_finite().then(|| value.to_string())
}

/// `2026-01-31` or `2026-01-31 13:45:00` as Excel's day count.
fn as_date(value: &str) -> Option<String> {
    let base = chrono::NaiveDate::from_ymd_opt(1899, 12, 30)?.and_hms_opt(0, 0, 0)?;
    let at = chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S")
        .ok()
        .or_else(|| chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").ok().and_then(|d| d.and_hms_opt(0, 0, 0)))?;
    let seconds = (at - base).num_seconds() as f64;
    let days = seconds / 86_400.;
    Some(if days.fract() == 0. { format!("{}", days as i64) } else { format!("{days}") })
}

/// The sheet's XML with `cells` written in: changed cells replaced (their style kept), new cells
/// and rows put where they belong, everything else byte for byte. Also says whether a formula
/// was replaced.
fn patch_sheet(xml: &str, cells: &BTreeMap<(usize, usize), String>) -> anyhow::Result<(String, bool)> {
    let mut pending: BTreeMap<usize, BTreeMap<usize, String>> = BTreeMap::new();
    for ((row, col), value) in cells {
        pending.entry(*row).or_default().insert(*col, value.clone());
    }
    let new_row = |row: usize, cells: &BTreeMap<usize, String>| {
        let inner: String = cells.iter().map(|(col, value)| cell_xml(row, *col, value, None)).collect();
        if inner.is_empty() {
            String::new()
        } else {
            format!("<row r=\"{}\">{inner}</row>", row + 1)
        }
    };
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut out = String::with_capacity(xml.len() + 256);
    let mut copied = 0usize;
    let mut in_data = false;
    let mut row: Option<(usize, BTreeMap<usize, String>)> = None;
    let mut last_row: Option<usize> = None;
    let mut last_col: Option<usize> = None;
    let mut had_formula = false;
    loop {
        let before = reader.buffer_position() as usize;
        let event = reader.read_event()?;
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(e) if ooxml::local(&e) == "sheetData" => in_data = true,
            Event::Empty(e) if ooxml::local(&e) == "sheetData" => {
                out.push_str(&xml[copied..before]);
                let rows: String = pending.iter().map(|(r, c)| new_row(*r, c)).collect();
                out.push_str(&format!("<sheetData>{rows}</sheetData>"));
                pending.clear();
                copied = after;
            }
            Event::End(e) if in_data && e.local_name().as_ref() == "sheetData" => {
                out.push_str(&xml[copied..before]);
                let rows: String = pending.iter().map(|(r, c)| new_row(*r, c)).collect();
                out.push_str(&rows);
                pending.clear();
                copied = before;
                in_data = false;
            }
            Event::Start(ref e) | Event::Empty(ref e) if in_data && ooxml::local(e) == "row" => {
                let empty = matches!(event, Event::Empty(_));
                let number = ooxml::attribute(e, "r").and_then(|r| r.parse::<usize>().ok()).map(|r| r - 1);
                let index = number.unwrap_or_else(|| last_row.map_or(0, |r| r + 1));
                last_row = Some(index);
                last_col = None;
                out.push_str(&xml[copied..before]);
                copied = before;
                let earlier: Vec<usize> = pending.range(..index).map(|(r, _)| *r).collect();
                for r in earlier {
                    if let Some(cells) = pending.remove(&r) {
                        out.push_str(&new_row(r, &cells));
                    }
                }
                if let Some(cells) = pending.remove(&index) {
                    if empty {
                        // `<row r="5"/>` opens up to take its cells.
                        let tag = &xml[before..after];
                        let open = tag.trim_end_matches('>').trim_end().trim_end_matches('/');
                        let inner: String = cells.iter().map(|(col, value)| cell_xml(index, *col, value, None)).collect();
                        out.push_str(&format!("{open}>{inner}</row>"));
                        copied = after;
                    } else {
                        row = Some((index, cells));
                    }
                }
            }
            Event::Start(ref e) | Event::Empty(ref e) if ooxml::local(e) == "c" && row.is_some() => {
                let empty = matches!(event, Event::Empty(_));
                let (index, cells) = row.as_mut().expect("checked");
                let col =
                    ooxml::attribute(e, "r").and_then(|r| parse_ref(&r)).map(|(_, c)| c).unwrap_or_else(|| last_col.map_or(0, |c| c + 1));
                last_col = Some(col);
                out.push_str(&xml[copied..before]);
                copied = before;
                let earlier: Vec<usize> = cells.range(..col).map(|(c, _)| *c).collect();
                for c in earlier {
                    if let Some(value) = cells.remove(&c) {
                        out.push_str(&cell_xml(*index, c, &value, None));
                    }
                }
                if let Some(value) = cells.remove(&col) {
                    let style = ooxml::attribute(e, "s");
                    let end = if empty {
                        after
                    } else {
                        let name = e.name().as_ref().to_string();
                        reader.read_to_end(quick_xml::name::QName(&name))?;
                        reader.buffer_position() as usize
                    };
                    let old = &xml[before..end];
                    // A formula other cells share is written once, here: replacing it would break theirs.
                    if (old.contains("t=\"shared\"") && old.contains("ref=\"")) || old.contains("t=\"array\"") {
                        anyhow::bail!("{} holds a formula other cells share; change it in Excel", cell_ref(*index, col));
                    }
                    had_formula |= old.contains("<f") || old.contains(":f>") || old.contains(":f ");
                    out.push_str(&cell_xml(*index, col, &value, style.as_deref()));
                    copied = end;
                }
            }
            Event::End(e) if e.local_name().as_ref() == "row" => {
                if let Some((index, cells)) = row.take() {
                    out.push_str(&xml[copied..before]);
                    for (col, value) in cells {
                        out.push_str(&cell_xml(index, col, &value, None));
                    }
                    copied = before;
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    out.push_str(&xml[copied..]);
    Ok((out, had_formula))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sheet(data: &str) -> String {
        format!(r#"<?xml version="1.0"?><worksheet xmlns="x"><dimension ref="A1:B2"/>{data}<pageMargins/></worksheet>"#)
    }

    fn patch(data: &str, cells: &[((usize, usize), &str)]) -> String {
        let cells: BTreeMap<(usize, usize), String> = cells.iter().map(|(k, v)| (*k, v.to_string())).collect();
        let (out, _) = patch_sheet(&sheet(data), &cells).unwrap();
        out.split_once("<sheetData").map(|(_, rest)| format!("<sheetData{}", rest.split_once("<pageMargins").unwrap().0)).unwrap()
    }

    #[test]
    fn replaces_a_cell_keeping_its_style() {
        let data = r#"<sheetData><row r="1" spans="1:2"><c r="A1" t="s"><v>0</v></c><c r="B1" s="3"><v>5</v></c></row></sheetData>"#;
        assert_eq!(
            patch(data, &[((0, 1), "hello & bye")]),
            r#"<sheetData><row r="1" spans="1:2"><c r="A1" t="s"><v>0</v></c><c r="B1" s="3" t="inlineStr"><is><t xml:space="preserve">hello &amp; bye</t></is></c></row></sheetData>"#
        );
        assert_eq!(
            patch(data, &[((0, 0), "42")]),
            r#"<sheetData><row r="1" spans="1:2"><c r="A1"><v>42</v></c><c r="B1" s="3"><v>5</v></c></row></sheetData>"#
        );
    }

    #[test]
    fn adds_cells_and_rows_in_order() {
        let data = r#"<sheetData><row r="2"><c r="B2"><v>1</v></c></row><row r="5"/></sheetData>"#;
        assert_eq!(
            patch(data, &[((0, 0), "x"), ((1, 0), "1"), ((1, 3), "2"), ((4, 1), "y"), ((6, 0), "z")]),
            concat!(
                r#"<sheetData><row r="1"><c r="A1" t="inlineStr"><is><t xml:space="preserve">x</t></is></c></row>"#,
                r#"<row r="2"><c r="A2"><v>1</v></c><c r="B2"><v>1</v></c><c r="D2"><v>2</v></c></row>"#,
                r#"<row r="5"><c r="B5" t="inlineStr"><is><t xml:space="preserve">y</t></is></c></row>"#,
                r#"<row r="7"><c r="A7" t="inlineStr"><is><t xml:space="preserve">z</t></is></c></row></sheetData>"#,
            )
        );
        assert_eq!(patch("<sheetData/>", &[((0, 0), "1")]), r#"<sheetData><row r="1"><c r="A1"><v>1</v></c></row></sheetData>"#);
    }

    #[test]
    fn notices_formulas_and_refuses_shared_ones() {
        let cells = BTreeMap::from([((0, 0), "1".to_string())]);
        let (_, had) = patch_sheet(&sheet(r#"<sheetData><row r="1"><c r="A1"><f>B1*2</f><v>4</v></c></row></sheetData>"#), &cells).unwrap();
        assert!(had);
        let shared = sheet(r#"<sheetData><row r="1"><c r="A1"><f t="shared" ref="A1:A3" si="0">B1</f><v>4</v></c></row></sheetData>"#);
        assert!(patch_sheet(&shared, &cells).is_err());
    }

    #[test]
    fn numbers_dates_and_text() {
        assert_eq!(as_number("12.5").as_deref(), Some("12.5"));
        assert_eq!(as_number("007"), None);
        assert_eq!(as_number("+1"), None);
        assert_eq!(as_number("abc"), None);
        assert_eq!(as_date("2026-01-01").as_deref(), Some("46023"));
        assert_eq!(cell_xml(0, 0, "2026-01-01", Some("1")), r#"<c r="A1" s="1"><v>46023</v></c>"#);
        assert_eq!(cell_xml(0, 0, "", None), "");
        assert_eq!(parse_ref("AB12"), Some((11, 27)));
    }

    #[test]
    fn asks_excel_to_recalculate() {
        assert_eq!(recalculate_on_load("<workbook><sheets/></workbook>"), "<workbook><sheets/></workbook>");
        assert_eq!(
            recalculate_on_load("<workbook><sheets></sheets></workbook>"),
            "<workbook><sheets></sheets><calcPr fullCalcOnLoad=\"1\"/></workbook>"
        );
        assert_eq!(recalculate_on_load(r#"<w><calcPr calcId="1"/></w>"#), r#"<w><calcPr fullCalcOnLoad="1" calcId="1"/></w>"#);
    }

    #[test]
    fn workbook_round_trip() {
        let bytes = ooxml::tests::package(&[
            ("[Content_Types].xml", r#"<Types><Override PartName="/xl/calcChain.xml" ContentType="x"/></Types>"#),
            ("xl/workbook.xml", r#"<workbook xmlns:r="r"><sheets><sheet name="Data" sheetId="1" r:id="rId1"/></sheets></workbook>"#),
            (
                "xl/_rels/workbook.xml.rels",
                r#"<Relationships><Relationship Id="rId1" Target="worksheets/sheet1.xml"/><Relationship Id="rId9" Target="calcChain.xml"/></Relationships>"#,
            ),
            ("xl/worksheets/sheet1.xml", &sheet(r#"<sheetData><row r="1"><c r="A1"><f>1+1</f><v>2</v></c></row></sheetData>"#)),
            ("xl/calcChain.xml", "<calcChain/>"),
        ]);
        let cells = BTreeMap::from([("Data".to_string(), BTreeMap::from([((0, 0), "7".to_string())]))]);
        let out = write_cells(&bytes, &cells).unwrap();
        assert!(ooxml::read_part(&out, "xl/worksheets/sheet1.xml").unwrap().unwrap().contains(r#"<c r="A1"><v>7</v></c>"#));
        assert_eq!(ooxml::read_part(&out, "xl/calcChain.xml").unwrap(), None);
        assert!(!ooxml::read_part(&out, "[Content_Types].xml").unwrap().unwrap().contains("calcChain"));
        assert!(!ooxml::read_part(&out, "xl/_rels/workbook.xml.rels").unwrap().unwrap().contains("calcChain"));
        assert!(ooxml::read_part(&out, "xl/workbook.xml").unwrap().unwrap().contains("fullCalcOnLoad"));
    }
}
