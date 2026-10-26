//! A Word document (`.docx`) read as a list of blocks — paragraphs with their style and text
//! runs, and tables — and written back by rewriting only the paragraphs that changed. Every other
//! part of the package, and every block not touched, is copied byte for byte.
//!
//! A paragraph is edited as plain text: its text is written back as one run that keeps the
//! formatting of its first run and the paragraph's own properties. Paragraphs holding what plain
//! text cannot carry (pictures, links, fields, tracked changes) are shown but not edited.

use super::ooxml;
use quick_xml::events::Event;
use std::collections::HashMap;

/// Word documents the editor shows as a page (the older `.doc` is another format).
pub fn is_docx(path: &std::path::Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("docx"))
}

/// The part holding the document's body.
const BODY: &str = "word/document.xml";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Normal,
    Title,
    Heading(u8),
    Quote,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Run {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
}

#[derive(Clone, Debug)]
pub struct Paragraph {
    pub kind: Kind,
    pub runs: Vec<Run>,
    /// A list item, at this level (from 0).
    pub list: Option<u8>,
    /// Holds something plain text would lose: shown, not edited.
    pub locked: bool,
    /// The paragraph's properties (`<w:pPr>…</w:pPr>`), kept when it is written again.
    properties: String,
    /// The first run's properties (`<w:rPr>…</w:rPr>`), the formatting its new text takes.
    run_properties: String,
}

impl Paragraph {
    pub fn text(&self) -> String {
        self.runs.iter().map(|r| r.text.as_str()).collect()
    }

    /// An empty paragraph formatted like `like` (a heading's next paragraph is plain body text).
    pub fn after(like: &Paragraph) -> Self {
        let heading = matches!(like.kind, Kind::Heading(_) | Kind::Title);
        Self {
            kind: if heading { Kind::Normal } else { like.kind.clone() },
            runs: Vec::new(),
            list: if heading { None } else { like.list },
            locked: false,
            properties: if heading { String::new() } else { like.properties.clone() },
            run_properties: if heading { String::new() } else { like.run_properties.clone() },
        }
    }

    /// The paragraph's XML for `text` (line breaks and tabs as Word writes them).
    fn xml(&self, text: &str) -> String {
        let mut runs = String::new();
        if !text.is_empty() {
            runs.push_str("<w:r>");
            runs.push_str(&self.run_properties);
            for (i, line) in text.split('\n').enumerate() {
                if i > 0 {
                    runs.push_str("<w:br/>");
                }
                for (j, piece) in line.split('\t').enumerate() {
                    if j > 0 {
                        runs.push_str("<w:tab/>");
                    }
                    if !piece.is_empty() {
                        runs.push_str(&format!("<w:t xml:space=\"preserve\">{}</w:t>", ooxml::escape(piece)));
                    }
                }
            }
            runs.push_str("</w:r>");
        }
        format!("<w:p>{}{runs}</w:p>", self.properties)
    }
}

#[derive(Clone, Debug)]
pub enum Block {
    Paragraph(Paragraph),
    /// A table's cells as text, row by row.
    Table(Vec<Vec<String>>),
    /// Something else in the body (a section's settings, a content control): kept, not shown.
    Other,
}

/// One block of the document and where it came from.
#[derive(Clone, Debug)]
pub struct Item {
    pub block: Block,
    /// Its XML as read (`None` for a block added here).
    raw: Option<std::ops::Range<usize>>,
    /// Its text was changed here.
    changed: bool,
}

#[derive(Clone)]
pub struct Document {
    pub items: Vec<Item>,
    xml: String,
    /// The body's content, between `<w:body>` and `</w:body>`.
    body: std::ops::Range<usize>,
    undo: Vec<Vec<Item>>,
    redo: Vec<Vec<Item>>,
    /// Undo steps since the last save (negative after undoing past it).
    unsaved: isize,
}

impl std::fmt::Debug for Document {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Document").field("items", &self.items.len()).finish()
    }
}

/// Reads the document's blocks from the package's bytes.
pub fn read(bytes: &[u8]) -> anyhow::Result<Document> {
    let xml = ooxml::read_part(bytes, BODY)?.ok_or_else(|| anyhow::anyhow!("not a Word document"))?;
    let styles = style_names(bytes)?;
    let mut reader = quick_xml::Reader::from_str(&xml);
    let mut items = Vec::new();
    let mut body = 0..0;
    let mut depth = 0usize;
    let mut in_body = false;
    loop {
        let before = reader.buffer_position() as usize;
        let event = reader.read_event()?;
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(e) if ooxml::local(&e) == "body" => {
                in_body = true;
                // Blocks are the body's own children.
                depth = 0;
                body.start = after;
            }
            Event::End(e) if in_body && e.local_name().as_ref() == "body" => {
                body.end = before;
                break;
            }
            Event::Start(e) if in_body && depth == 0 => {
                let name = e.name().as_ref().to_string();
                reader.read_to_end(quick_xml::name::QName(&name))?;
                let end = reader.buffer_position() as usize;
                let raw = &xml[before..end];
                let block = match ooxml::local(&e).as_str() {
                    "p" => Block::Paragraph(paragraph(raw, &styles)?),
                    "tbl" => Block::Table(table(raw)?),
                    _ => Block::Other,
                };
                items.push(Item { block, raw: Some(before..end), changed: false });
            }
            Event::Empty(e) if in_body && depth == 0 => {
                let block = if ooxml::local(&e) == "p" {
                    Block::Paragraph(Paragraph {
                        kind: Kind::Normal,
                        runs: Vec::new(),
                        list: None,
                        locked: false,
                        properties: String::new(),
                        run_properties: String::new(),
                    })
                } else {
                    Block::Other
                };
                items.push(Item { block, raw: Some(before..after), changed: false });
            }
            Event::Start(_) => depth += 1,
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::Eof => break,
            _ => {}
        }
    }
    if body.end == 0 {
        anyhow::bail!("the document has no body");
    }
    Ok(Document { items, xml, body, undo: Vec::new(), redo: Vec::new(), unsaved: 0 })
}

/// Style id → its name in lower case (`Heading1` → `heading 1`), from `word/styles.xml`.
fn style_names(bytes: &[u8]) -> anyhow::Result<HashMap<String, String>> {
    let mut names = HashMap::new();
    let Some(xml) = ooxml::read_part(bytes, "word/styles.xml")? else { return Ok(names) };
    let mut reader = quick_xml::Reader::from_str(&xml);
    let mut current: Option<String> = None;
    loop {
        match reader.read_event()? {
            Event::Start(e) if ooxml::local(&e) == "style" => current = ooxml::attribute(&e, "styleId"),
            Event::Empty(e) if ooxml::local(&e) == "name" => {
                if let (Some(id), Some(name)) = (current.clone(), ooxml::attribute(&e, "val")) {
                    names.insert(id, name.to_lowercase());
                }
            }
            Event::End(e) if e.local_name().as_ref() == "style" => current = None,
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(names)
}

fn kind_of(style: &str, styles: &HashMap<String, String>) -> Kind {
    let name = styles.get(style).cloned().unwrap_or_else(|| style.to_lowercase());
    let compact: String = name.chars().filter(|c| !c.is_whitespace()).collect();
    if compact == "title" {
        return Kind::Title;
    }
    if let Some(level) = compact.strip_prefix("heading").and_then(|n| n.parse::<u8>().ok()) {
        return Kind::Heading(level.clamp(1, 6));
    }
    if compact.contains("quote") {
        return Kind::Quote;
    }
    Kind::Normal
}

/// Whether a run property element (`<w:b/>`, `<w:b w:val="0"/>`) switches its format on.
fn switched_on(element: &quick_xml::events::BytesStart) -> bool {
    !matches!(ooxml::attribute(element, "val").as_deref(), Some("0" | "false" | "none" | "off"))
}

/// Reads one `<w:p>`.
fn paragraph(raw: &str, styles: &HashMap<String, String>) -> anyhow::Result<Paragraph> {
    let mut reader = quick_xml::Reader::from_str(raw);
    let mut p = Paragraph {
        kind: Kind::Normal,
        runs: Vec::new(),
        list: None,
        locked: false,
        properties: String::new(),
        run_properties: String::new(),
    };
    let mut run: Option<Run> = None;
    let mut in_text = false;
    let mut first_rpr = true;
    loop {
        let before = reader.buffer_position() as usize;
        let event = reader.read_event()?;
        match event {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let empty = matches!(event, Event::Empty(_));
                match ooxml::local(e).as_str() {
                    "pPr" if run.is_none() => {
                        if empty {
                            p.properties = raw[before..reader.buffer_position() as usize].to_string();
                        } else {
                            let name = e.name().as_ref().to_string();
                            reader.read_to_end(quick_xml::name::QName(&name))?;
                            let end = reader.buffer_position() as usize;
                            p.properties = raw[before..end].to_string();
                            // The properties are read again for what they say.
                            let properties = p.properties.clone();
                            inspect_properties(&properties, styles, &mut p);
                        }
                    }
                    "r" => run = Some(Run::default()),
                    "rPr" if run.is_some() => {
                        if first_rpr {
                            let end = if empty {
                                reader.buffer_position() as usize
                            } else {
                                let name = e.name().as_ref().to_string();
                                reader.read_to_end(quick_xml::name::QName(&name))?;
                                reader.buffer_position() as usize
                            };
                            p.run_properties = raw[before..end].to_string();
                            first_rpr = false;
                            if let Some(run) = run.as_mut() {
                                read_run_format(&p.run_properties, run)?;
                            }
                        } else if !empty {
                            let name = e.name().as_ref().to_string();
                            let start = before;
                            reader.read_to_end(quick_xml::name::QName(&name))?;
                            let end = reader.buffer_position() as usize;
                            if let Some(run) = run.as_mut() {
                                read_run_format(&raw[start..end], run)?;
                            }
                        }
                    }
                    "t" if run.is_some() => in_text = !empty,
                    "tab" if run.is_some() => push(&mut run, "\t"),
                    "br" | "cr" if run.is_some() => push(&mut run, "\n"),
                    // What plain text would lose.
                    "drawing" | "pict" | "object" | "hyperlink" | "fldSimple" | "fldChar" | "ins" | "del" | "moveFrom" | "moveTo"
                    | "oMath" | "oMathPara" | "sdt" | "footnoteReference" | "endnoteReference" | "commentReference" => p.locked = true,
                    _ => {}
                }
            }
            Event::Text(t) if in_text => push(&mut run, &ooxml::text(&t)),
            Event::GeneralRef(r) if in_text => push(&mut run, &ooxml::reference(&r)),
            Event::End(e) => match e.local_name().as_ref() {
                "t" => in_text = false,
                "r" => {
                    if let Some(done) = run.take() {
                        p.runs.push(done);
                    }
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    // Neighbouring runs formatted alike read as one.
    let mut runs: Vec<Run> = Vec::new();
    for r in p.runs.drain(..) {
        match runs.last_mut() {
            Some(last) if (last.bold, last.italic, last.underline, last.strike) == (r.bold, r.italic, r.underline, r.strike) => {
                last.text.push_str(&r.text)
            }
            _ => runs.push(r),
        }
    }
    p.runs = runs;
    Ok(p)
}

fn push(run: &mut Option<Run>, text: &str) {
    if let Some(run) = run.as_mut() {
        run.text.push_str(text);
    }
}

/// The paragraph's style and list level, from its properties.
fn inspect_properties(xml: &str, styles: &HashMap<String, String>, p: &mut Paragraph) {
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut list = false;
    while let Ok(event) = reader.read_event() {
        match event {
            Event::Start(ref e) | Event::Empty(ref e) => match ooxml::local(e).as_str() {
                "pStyle" => {
                    if let Some(style) = ooxml::attribute(e, "val") {
                        p.kind = kind_of(&style, styles);
                        let name = styles.get(&style).cloned().unwrap_or_default();
                        if name.contains("list") {
                            p.list = p.list.or(Some(0));
                        }
                    }
                }
                "numPr" => list = true,
                "ilvl" if list => p.list = Some(ooxml::attribute(e, "val").and_then(|v| v.parse().ok()).unwrap_or(0)),
                "outlineLvl" if p.kind == Kind::Normal => {
                    if let Some(level) = ooxml::attribute(e, "val").and_then(|v| v.parse::<u8>().ok()) {
                        p.kind = Kind::Heading((level + 1).min(6));
                    }
                }
                // The paragraph mark's own run properties: not the text's.
                "rPr" => break,
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    if list && p.list.is_none() {
        p.list = Some(0);
    }
}

fn read_run_format(xml: &str, run: &mut Run) -> anyhow::Result<()> {
    let mut reader = quick_xml::Reader::from_str(xml);
    loop {
        match reader.read_event()? {
            Event::Start(ref e) | Event::Empty(ref e) => match ooxml::local(e).as_str() {
                "b" => run.bold = switched_on(e),
                "i" => run.italic = switched_on(e),
                "u" => run.underline = switched_on(e),
                "strike" | "dstrike" => run.strike = switched_on(e),
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(())
}

/// Reads one `<w:tbl>` as rows of cell text.
fn table(raw: &str) -> anyhow::Result<Vec<Vec<String>>> {
    let mut reader = quick_xml::Reader::from_str(raw);
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut in_text = false;
    let mut depth = 0usize;
    loop {
        match reader.read_event()? {
            Event::Start(e) => match ooxml::local(&e).as_str() {
                "tbl" => depth += 1,
                "tr" if depth == 1 => rows.push(Vec::new()),
                "tc" if depth == 1 => {
                    if let Some(row) = rows.last_mut() {
                        row.push(String::new());
                    }
                }
                "p" => {
                    if let Some(cell) = rows.last_mut().and_then(|r| r.last_mut()).filter(|c| !c.is_empty()) {
                        cell.push('\n');
                    }
                }
                "t" => in_text = true,
                _ => {}
            },
            Event::Empty(e) if ooxml::local(&e) == "tab" => {
                if let Some(cell) = rows.last_mut().and_then(|r| r.last_mut()) {
                    cell.push('\t');
                }
            }
            Event::Text(t) if in_text => {
                if let Some(cell) = rows.last_mut().and_then(|r| r.last_mut()) {
                    cell.push_str(&ooxml::text(&t));
                }
            }
            Event::GeneralRef(r) if in_text => {
                if let Some(cell) = rows.last_mut().and_then(|r| r.last_mut()) {
                    cell.push_str(&ooxml::reference(&r));
                }
            }
            Event::End(e) => match e.local_name().as_ref() {
                "t" => in_text = false,
                "tbl" => depth = depth.saturating_sub(1),
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(rows)
}

impl Document {
    pub fn is_dirty(&self) -> bool {
        self.unsaved != 0
    }

    fn step(&mut self) {
        self.undo.push(self.items.clone());
        self.redo.clear();
        self.unsaved += 1;
    }

    /// Gives paragraph `index` new text (one undo step). False when nothing changed.
    pub fn set_text(&mut self, index: usize, text: &str) -> bool {
        let Some(Block::Paragraph(p)) = self.items.get(index).map(|i| &i.block) else { return false };
        if p.locked || p.text() == text {
            return false;
        }
        self.step();
        let item = &mut self.items[index];
        if let Block::Paragraph(p) = &mut item.block {
            let format = p.runs.first().cloned().unwrap_or_default();
            p.runs = vec![Run { text: text.to_string(), ..format }];
        }
        item.changed = true;
        true
    }

    /// An empty paragraph put at `at`, formatted like block `like`. Returns where it went.
    pub fn insert(&mut self, at: usize, like: usize) -> usize {
        self.insert_like(at.min(self.items.len()), Some(like))
    }

    /// An empty paragraph after `index` (formatted like it), or after the last one. Returns where.
    pub fn insert_after(&mut self, index: Option<usize>) -> usize {
        let at = index.map_or(self.end_of_text(), |i| (i + 1).min(self.items.len()));
        self.insert_like(at, index)
    }

    fn insert_like(&mut self, at: usize, like: Option<usize>) -> usize {
        let like = like
            .and_then(|i| self.items.get(i))
            .filter(|i| matches!(i.block, Block::Paragraph(_)))
            .or_else(|| self.items[..at].iter().rev().find(|i| matches!(i.block, Block::Paragraph(_))))
            .and_then(|i| if let Block::Paragraph(p) = &i.block { Some(Paragraph::after(p)) } else { None });
        let paragraph = like.unwrap_or(Paragraph {
            kind: Kind::Normal,
            runs: Vec::new(),
            list: None,
            locked: false,
            properties: String::new(),
            run_properties: String::new(),
        });
        self.step();
        self.items.insert(at, Item { block: Block::Paragraph(paragraph), raw: None, changed: true });
        at
    }

    /// Where blocks added at the end go: before the body's own section settings.
    fn end_of_text(&self) -> usize {
        let trailing = self.items.iter().rev().take_while(|i| matches!(i.block, Block::Other)).count();
        self.items.len() - trailing
    }

    pub fn remove(&mut self, index: usize) {
        if !matches!(self.items.get(index).map(|i| &i.block), Some(Block::Paragraph(_) | Block::Table(_))) {
            return;
        }
        self.step();
        self.items.remove(index);
        // A body needs a paragraph: Word refuses one that ends without.
        if !self.items.iter().any(|i| matches!(i.block, Block::Paragraph(_))) {
            let at = self.end_of_text();
            self.items
                .insert(at, Item { block: Block::Paragraph(Paragraph::after(&Paragraph::default_normal())), raw: None, changed: true });
        }
    }

    pub fn undo(&mut self) -> bool {
        let Some(items) = self.undo.pop() else { return false };
        self.redo.push(std::mem::replace(&mut self.items, items));
        self.unsaved -= 1;
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(items) = self.redo.pop() else { return false };
        self.undo.push(std::mem::replace(&mut self.items, items));
        self.unsaved += 1;
        true
    }

    /// The package's bytes with the document as it is now.
    pub fn render(&self, bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
        let mut body = String::with_capacity(self.body.len() + 256);
        for item in &self.items {
            match (&item.raw, item.changed, &item.block) {
                (_, true, Block::Paragraph(p)) => body.push_str(&p.xml(&p.text())),
                (Some(raw), _, _) => body.push_str(&self.xml[raw.clone()]),
                _ => {}
            }
        }
        let xml = format!("{}{body}{}", &self.xml[..self.body.start], &self.xml[self.body.end..]);
        ooxml::rewrite(bytes, &HashMap::from([(BODY.to_string(), xml)]), &[])
    }

    /// What was rendered is on disk now.
    pub fn mark_saved(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        let undo = std::mem::take(&mut self.undo);
        let redo = std::mem::take(&mut self.redo);
        *self = read(bytes)?;
        // Steps taken before the save can still be undone (they make the file differ again).
        self.undo = undo;
        self.redo = redo;
        Ok(())
    }
}

impl Paragraph {
    fn default_normal() -> Self {
        Self { kind: Kind::Normal, runs: Vec::new(), list: None, locked: false, properties: String::new(), run_properties: String::new() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STYLES: &str = r#"<w:styles xmlns:w="w"><w:style w:styleId="Heading1"><w:name w:val="heading 1"/></w:style><w:style w:styleId="Title"><w:name w:val="Title"/></w:style></w:styles>"#;

    fn docx(body: &str) -> Vec<u8> {
        let document =
            format!(r#"<?xml version="1.0"?><w:document xmlns:w="w"><w:body>{body}<w:sectPr><w:pgSz/></w:sectPr></w:body></w:document>"#);
        ooxml::tests::package(&[("word/document.xml", &document), ("word/styles.xml", STYLES), ("word/media/a.png", "png")])
    }

    fn paragraphs(doc: &Document) -> Vec<(Kind, String)> {
        doc.items.iter().filter_map(|i| if let Block::Paragraph(p) = &i.block { Some((p.kind.clone(), p.text())) } else { None }).collect()
    }

    #[test]
    fn reads_paragraphs_styles_and_tables() {
        let bytes = docx(concat!(
            r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Plan</w:t></w:r></w:p>"#,
            r#"<w:p><w:r><w:rPr><w:b/></w:rPr><w:t>Bold</w:t></w:r><w:r><w:t xml:space="preserve"> and &amp; plain</w:t><w:tab/><w:t>x</w:t></w:r></w:p>"#,
            r#"<w:tbl><w:tr><w:tc><w:p><w:r><w:t>a</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>b</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#,
            r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="1"/><w:numId w:val="3"/></w:numPr></w:pPr><w:r><w:t>item</w:t></w:r></w:p>"#,
            r#"<w:p><w:r><w:drawing/></w:r></w:p><w:p/>"#,
        ));
        let doc = read(&bytes).unwrap();
        assert_eq!(
            paragraphs(&doc),
            vec![
                (Kind::Heading(1), "Plan".into()),
                (Kind::Normal, "Bold and & plain\tx".into()),
                (Kind::Normal, "item".into()),
                (Kind::Normal, String::new()),
                (Kind::Normal, String::new())
            ]
        );
        let Block::Paragraph(bold) = &doc.items[1].block else { panic!() };
        assert!(bold.runs[0].bold && !bold.runs[1].bold);
        assert!(matches!(&doc.items[2].block, Block::Table(rows) if rows == &vec![vec!["a".to_string(), "b".to_string()]]));
        let Block::Paragraph(item) = &doc.items[3].block else { panic!() };
        assert_eq!(item.list, Some(1));
        let Block::Paragraph(picture) = &doc.items[4].block else { panic!() };
        assert!(picture.locked);
        assert!(matches!(doc.items.last().unwrap().block, Block::Other));
    }

    #[test]
    fn writes_back_only_what_changed() {
        let first =
            r#"<w:p><w:pPr><w:jc w:val="center"/></w:pPr><w:r><w:rPr><w:i/></w:rPr><w:t>Old</w:t></w:r><w:r><w:t> tail</w:t></w:r></w:p>"#;
        let second = r#"<w:p><w:r><w:t>Keep   me</w:t></w:r></w:p>"#;
        let bytes = docx(&format!("{first}{second}"));
        let mut doc = read(&bytes).unwrap();
        assert!(doc.set_text(0, "New <text>\nline"));
        let at = doc.insert_after(Some(1));
        doc.set_text(at, "added");
        let out = doc.render(&bytes).unwrap();
        let xml = ooxml::read_part(&out, BODY).unwrap().unwrap();
        assert!(xml.contains(r#"<w:p><w:pPr><w:jc w:val="center"/></w:pPr><w:r><w:rPr><w:i/></w:rPr><w:t xml:space="preserve">New &lt;text&gt;</w:t><w:br/><w:t xml:space="preserve">line</w:t></w:r></w:p>"#));
        assert!(xml.contains(second));
        assert!(xml.contains(r#"<w:t xml:space="preserve">added</w:t>"#));
        assert!(xml.ends_with("<w:sectPr><w:pgSz/></w:sectPr></w:body></w:document>"));
        assert_eq!(ooxml::read_part(&out, "word/media/a.png").unwrap().as_deref(), Some("png"));
        assert!(doc.is_dirty());
        doc.mark_saved(&out).unwrap();
        assert!(!doc.is_dirty());
        assert_eq!(paragraphs(&doc)[0].1, "New <text>\nline");
    }

    #[test]
    fn undo_redo_and_remove() {
        let bytes = docx(r#"<w:p><w:r><w:t>a</w:t></w:r></w:p><w:p><w:r><w:t>b</w:t></w:r></w:p>"#);
        let mut doc = read(&bytes).unwrap();
        doc.remove(0);
        assert_eq!(paragraphs(&doc).len(), 1);
        assert!(doc.undo());
        assert_eq!(paragraphs(&doc).len(), 2);
        assert!(!doc.is_dirty());
        assert!(doc.redo());
        assert!(doc.is_dirty());
        doc.remove(0);
        // The last paragraph is replaced by an empty one, not left out.
        assert_eq!(paragraphs(&doc), vec![(Kind::Normal, String::new())]);
    }
}
