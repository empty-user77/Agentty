//! Plain text of office documents an agent cannot read as they are: Word, Excel and PowerPoint
//! files (and their OpenDocument cousins) are zip archives of XML, read with the system's `unzip`;
//! the older binary Word and RTF files go through macOS's `textutil`.

use anyhow::{bail, Context as _, Result};
use std::io::Read as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Text kept from one document, and from one member of its archive (a zip bomb stops here).
const MAX_CHARS: usize = 400_000;
const MAX_MEMBER_BYTES: u64 = 20 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(30);

/// Whether `path` is a document this module turns into text (by its extension).
pub fn handles(path: &Path) -> bool {
    matches!(ext(path).as_str(), "docx" | "xlsx" | "pptx" | "odt" | "ods" | "odp" | "doc" | "rtf")
}

fn ext(path: &Path) -> String {
    path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase()
}

/// The text of the document at `path`, as Markdown-ish plain text.
pub fn text_of(path: &Path) -> Result<String> {
    let text = match ext(path).as_str() {
        "docx" => xml_text(&member(path, "word/document.xml")?),
        "odt" => xml_text(&member(path, "content.xml")?),
        "pptx" => {
            let mut slides: Vec<(u32, String)> = members(path)?
                .into_iter()
                .filter_map(|name| {
                    let n = name.strip_prefix("ppt/slides/slide")?.strip_suffix(".xml")?.parse().ok()?;
                    Some((n, name))
                })
                .collect();
            slides.sort();
            let mut out = String::new();
            for (n, name) in slides {
                out.push_str(&format!("## Slide {n}\n\n{}\n\n", xml_text(&member(path, &name)?)));
            }
            out
        }
        "odp" => xml_text(&member(path, "content.xml")?),
        "xlsx" => xlsx_text(path)?,
        "ods" => xml_text(&member(path, "content.xml")?),
        "doc" | "rtf" => textutil(path)?,
        other => bail!("not an office document ({other})"),
    };
    let text = text.trim().to_string();
    if text.is_empty() {
        bail!("no text in it");
    }
    Ok(if text.chars().count() > MAX_CHARS { text.chars().take(MAX_CHARS).collect::<String>() + "\n\n[… cut here]" } else { text })
}

/// The names in the archive.
fn members(path: &Path) -> Result<Vec<String>> {
    let out = run(Command::new("unzip").arg("-Z1").arg(path))?;
    Ok(out.lines().map(str::to_string).collect())
}

/// One member of the archive, as text. `unzip` matches the name as a pattern: the names asked
/// for here are fixed or come from the archive's own listing, never from what a user typed.
fn member(path: &Path, name: &str) -> Result<String> {
    run(Command::new("unzip").arg("-p").arg(path).arg(name))
}

fn textutil(path: &Path) -> Result<String> {
    if !cfg!(target_os = "macos") {
        bail!("old Word and RTF files are read on macOS only");
    }
    run(Command::new("/usr/bin/textutil").args(["-convert", "txt", "-stdout"]).arg(path))
}

/// Runs `command`, keeping at most [`MAX_MEMBER_BYTES`] of what it prints and stopping it after
/// [`TIMEOUT`].
fn run(command: &mut Command) -> Result<String> {
    let mut child = command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().context("could not run it")?;
    let stdout = child.stdout.take().context("no output")?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.take(MAX_MEMBER_BYTES).read_to_end(&mut bytes);
        bytes
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if started.elapsed() > TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    // Output cut at the limit: the command may still be writing; it is not waited on.
    let bytes = reader.join().unwrap_or_default();
    match status {
        Some(status) if status.success() || bytes.len() as u64 >= MAX_MEMBER_BYTES => Ok(String::from_utf8_lossy(&bytes).to_string()),
        Some(_) => bail!("the document could not be opened"),
        None => bail!("reading it took too long"),
    }
}

/// The text of an office XML part: paragraphs, line breaks and tabs kept, every tag dropped.
pub fn xml_text(xml: &str) -> String {
    let mut out = String::new();
    let mut rest = xml;
    while let Some(open) = rest.find('<') {
        out.push_str(&crate::orch_context::decode(&rest[..open]));
        let Some(close) = rest[open..].find('>') else { break };
        let tag = &rest[open + 1..open + close];
        let closing = tag.starts_with('/');
        let name = tag.trim_start_matches('/').split(|c: char| c.is_whitespace() || c == '/').next().unwrap_or_default();
        match name {
            // Word, PowerPoint, OpenDocument paragraphs and headings; spreadsheet rows.
            "w:p" | "a:p" | "text:p" | "text:h" | "table:table-row" if closing => out.push('\n'),
            "w:br" | "a:br" | "text:line-break" => out.push('\n'),
            "w:tab" | "text:tab" => out.push('\t'),
            "table:table-cell" if closing => out.push_str(" | "),
            _ => {}
        }
        rest = &rest[open + close + 1..];
    }
    out.push_str(&crate::orch_context::decode(rest));
    let lines: Vec<&str> = out.lines().map(str::trim_end).collect();
    let mut text = String::new();
    let mut blank = 0;
    for line in lines {
        if line.trim().is_empty() {
            blank += 1;
            if blank == 1 && !text.is_empty() {
                text.push('\n');
            }
            continue;
        }
        blank = 0;
        text.push_str(line);
        text.push('\n');
    }
    text.trim().to_string()
}

/// Every sheet of a workbook as rows of cells (`a | b | c`), with the shared strings filled in.
fn xlsx_text(path: &Path) -> Result<String> {
    let shared: Vec<String> = member(path, "xl/sharedStrings.xml")
        .map(|xml| between_all(&xml, "<si", "</si>").into_iter().map(|si| xml_text(inner(&si)).replace('\n', " ")).collect())
        .unwrap_or_default();
    let mut sheets: Vec<(u32, String)> = members(path)?
        .into_iter()
        .filter_map(|name| {
            let n = name.strip_prefix("xl/worksheets/sheet")?.strip_suffix(".xml")?.parse().ok()?;
            Some((n, name))
        })
        .collect();
    sheets.sort();
    let mut out = String::new();
    for (n, name) in sheets {
        out.push_str(&format!("## Sheet {n}\n\n"));
        let xml = member(path, &name)?;
        for row in between_all(&xml, "<row", "</row>") {
            let cells: Vec<String> = between_all(&row, "<c", "</c>").iter().map(|cell| cell_value(cell, &shared)).collect();
            if cells.iter().any(|c| !c.is_empty()) {
                out.push_str(&cells.join(" | "));
                out.push('\n');
            }
        }
        out.push('\n');
    }
    Ok(out)
}

/// A cell's shown value: a shared string, an inline string, or the number or formula result as stored.
fn cell_value(cell: &str, shared: &[String]) -> String {
    let head = &cell[..cell.find('>').unwrap_or(cell.len())];
    if head.ends_with('/') {
        return String::new();
    }
    let value =
        between_all(cell, "<v", "</v>").into_iter().next().map(|v| crate::orch_context::decode(v.split_once('>').map_or("", |(_, v)| v)));
    if head.contains("t=\"s\"") {
        return value.and_then(|v| v.trim().parse::<usize>().ok()).and_then(|i| shared.get(i).cloned()).unwrap_or_default();
    }
    if head.contains("t=\"inlineStr\"") {
        return xml_text(inner(cell)).replace('\n', " ");
    }
    value.unwrap_or_default()
}

/// What an element found by [`between_all`] holds: everything after the rest of its opening tag.
fn inner(element: &str) -> &str {
    element.split_once('>').map_or("", |(_, inside)| inside)
}

/// Each element of `text` that starts with `open` (a tag's name, then a space, `>` or `/`): what
/// follows the name up to `close`, or up to and with the `/>` of one that closes itself.
fn between_all(text: &str, open: &str, close: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(open) {
        let after = &rest[start + open.len()..];
        // `<c` must not match `<cols`: the tag's name ends here.
        if !after.starts_with([' ', '>', '/']) {
            rest = after;
            continue;
        }
        let Some(gt) = after.find('>') else { break };
        if after[..gt].ends_with('/') {
            found.push(after[..=gt].to_string());
            rest = &after[gt + 1..];
            continue;
        }
        let Some(end) = after.find(close) else { break };
        found.push(after[..end].to_string());
        rest = &after[end + close.len()..];
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_and_slide_xml_read_as_paragraphs() {
        let xml = r#"<w:document><w:body><w:p><w:r><w:t>Hello</w:t></w:r><w:r><w:tab/><w:t>world &amp; more</w:t></w:r></w:p><w:p><w:r><w:t>Second</w:t><w:br/><w:t>line</w:t></w:r></w:p></w:body></w:document>"#;
        assert_eq!(xml_text(xml), "Hello\tworld & more\nSecond\nline");
        let slide = r#"<p:sld><a:p><a:r><a:t>Title</a:t></a:r></a:p><a:p><a:r><a:t>Point</a:t></a:r></a:p></p:sld>"#;
        assert_eq!(xml_text(slide), "Title\nPoint");
    }

    #[test]
    fn sheet_cells_take_their_shared_strings() {
        let shared = vec!["Name".to_string(), "Price".to_string()];
        assert_eq!(cell_value(r#" r="A1" t="s"><v>1</v>"#, &shared), "Price");
        assert_eq!(cell_value(r#" r="B2"><v>42.5</v>"#, &shared), "42.5");
        assert_eq!(cell_value(r#" r="C2" t="inlineStr"><is><t>inline</t></is>"#, &shared), "inline");
        assert_eq!(cell_value(r#" r="D2"/>"#, &shared), "");
        let row = r#"<c r="A1" t="s"><v>0</v></c><c r="B1"/><c r="C1"><v>7</v></c>"#;
        let cells: Vec<String> = between_all(row, "<c", "</c>").iter().map(|c| cell_value(c, &shared)).collect();
        assert_eq!(cells, ["Name", "", "7"]);
        assert!(between_all("<cols><col/></cols>", "<c", "</c>").is_empty());
        let shared: Vec<String> =
            between_all("<sst><si><t>Product</t></si><si><r><t>Pri</t></r><r><t>ce</t></r></si></sst>", "<si", "</si>")
                .iter()
                .map(|si| xml_text(inner(si)))
                .collect();
        assert_eq!(shared, ["Product", "Price"]);
    }

    #[test]
    fn only_office_documents_are_handled() {
        assert!(handles(Path::new("a/Plan.DOCX")) && handles(Path::new("deck.pptx")) && handles(Path::new("old.doc")));
        assert!(!handles(Path::new("notes.md")) && !handles(Path::new("shot.png")) && !handles(Path::new("report.pdf")));
    }
}
