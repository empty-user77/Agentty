//! Office Open XML packages (`.xlsx`, `.docx`): a zip of XML parts. Edits rewrite only the parts
//! they change; every other entry is copied byte for byte (not even recompressed), so what the
//! editor does not understand — charts, pictures, macros, styles — comes through untouched.

use quick_xml::events::{BytesRef, BytesStart, BytesText};
use std::collections::HashMap;
use std::io::{Read, Write};

/// The largest part read (unpacked): a sheet or a document body far beyond what a person edits.
const PART_LIMIT: u64 = 256 * 1024 * 1024;

/// A part of the package as text (`xl/workbook.xml`); `None` when the package has no such part.
pub fn read_part(bytes: &[u8], name: &str) -> anyhow::Result<Option<String>> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
    let Ok(entry) = archive.by_name(name) else { return Ok(None) };
    // A part that unpacks to more than this is not read: a small file must not fill the memory.
    if entry.size() > PART_LIMIT {
        anyhow::bail!("{name} is too large to open ({} MB)", entry.size() / 1_048_576);
    }
    let mut text = String::new();
    entry.take(PART_LIMIT + 1).read_to_string(&mut text)?;
    if text.len() as u64 > PART_LIMIT {
        anyhow::bail!("{name} is too large to open");
    }
    Ok(Some(text))
}

/// The package with `replace` parts written anew and `remove` parts left out, every other entry
/// as it was.
pub fn rewrite(bytes: &[u8], replace: &HashMap<String, String>, remove: &[String]) -> anyhow::Result<Vec<u8>> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).compression_level(Some(6));
    for i in 0..archive.len() {
        let entry = archive.by_index_raw(i)?;
        let name = entry.name()?.to_string();
        if remove.contains(&name) {
            continue;
        }
        match replace.get(&name) {
            Some(text) => {
                drop(entry);
                out.start_file(name, options)?;
                out.write_all(text.as_bytes())?;
            }
            None => out.raw_copy_file(entry)?,
        }
    }
    Ok(out.finish()?.into_inner())
}

/// The value of attribute `name` (its local name: `r:id` is found as `id`), unescaped.
pub fn attribute(element: &BytesStart, name: &str) -> Option<String> {
    element
        .attributes()
        .flatten()
        .find(|a| a.key.local_name().as_ref() == name)
        .map(|a| quick_xml::escape::unescape(&a.value).map_or_else(|_| a.value.to_string(), |v| v.into_owned()))
}

/// The local name of an element (`w:p` is `p`).
pub fn local(element: &BytesStart) -> String {
    element.local_name().as_ref().to_string()
}

/// Text content as it reads (the XML's own escapes are separate events).
pub fn text(content: &BytesText) -> String {
    content.to_string()
}

/// What `&amp;`, `&#x41;` and the like stand for.
pub fn reference(reference: &BytesRef) -> String {
    if let Ok(Some(c)) = reference.resolve_char_ref() {
        return c.to_string();
    }
    match &**reference {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        "apos" => "'",
        _ => "",
    }
    .to_string()
}

/// Text made safe to put between tags or in an attribute.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            // Control characters XML 1.0 refuses (other than tab and line breaks) are left out.
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => {}
            c => out.push(c),
        }
    }
    out
}

/// Where a part's relationship points, as a path in the package: `Target` is relative to the
/// part's folder unless it starts with `/`.
pub fn resolve_target(folder: &str, target: &str) -> String {
    if let Some(absolute) = target.strip_prefix('/') {
        return absolute.to_string();
    }
    let mut parts: Vec<&str> = folder.split('/').filter(|p| !p.is_empty()).collect();
    for piece in target.split('/') {
        match piece {
            ".." => {
                parts.pop();
            }
            "." | "" => {}
            piece => parts.push(piece),
        }
    }
    parts.join("/")
}

/// The relationships of a part (`xl/_rels/workbook.xml.rels`): id → target in the package.
pub fn relationships(bytes: &[u8], rels: &str, folder: &str) -> anyhow::Result<HashMap<String, String>> {
    let mut found = HashMap::new();
    let Some(xml) = read_part(bytes, rels)? else { return Ok(found) };
    let mut reader = quick_xml::Reader::from_str(&xml);
    loop {
        match reader.read_event()? {
            quick_xml::events::Event::Empty(e) | quick_xml::events::Event::Start(e) if local(&e) == "Relationship" => {
                if let (Some(id), Some(target)) = (attribute(&e, "Id"), attribute(&e, "Target")) {
                    found.insert(id, resolve_target(folder, &target));
                }
            }
            quick_xml::events::Event::Eof => break,
            _ => {}
        }
    }
    Ok(found)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A package with these parts (stored as given), for tests.
    pub fn package(parts: &[(&str, &str)]) -> Vec<u8> {
        let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, text) in parts {
            out.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            out.write_all(text.as_bytes()).unwrap();
        }
        out.finish().unwrap().into_inner()
    }

    #[test]
    fn rewrites_only_what_changed() {
        let bytes = package(&[("a.xml", "<a/>"), ("b.bin", "binary"), ("c.xml", "<c/>")]);
        let replace = HashMap::from([("a.xml".to_string(), "<a>new</a>".to_string())]);
        let out = rewrite(&bytes, &replace, &["c.xml".to_string()]).unwrap();
        assert_eq!(read_part(&out, "a.xml").unwrap().as_deref(), Some("<a>new</a>"));
        assert_eq!(read_part(&out, "b.bin").unwrap().as_deref(), Some("binary"));
        assert_eq!(read_part(&out, "c.xml").unwrap(), None);
    }

    #[test]
    fn targets_resolve_against_their_folder() {
        assert_eq!(resolve_target("xl", "worksheets/sheet1.xml"), "xl/worksheets/sheet1.xml");
        assert_eq!(resolve_target("xl", "/xl/worksheets/sheet2.xml"), "xl/worksheets/sheet2.xml");
        assert_eq!(resolve_target("xl/worksheets", "../media/a.png"), "xl/media/a.png");
    }

    #[test]
    fn escapes_markup() {
        assert_eq!(escape("a<b & \"c\""), "a&lt;b &amp; &quot;c&quot;");
    }
}
