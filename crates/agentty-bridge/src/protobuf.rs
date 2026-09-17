//! Minimal protobuf reader: walks the wire format and collects UTF-8 string fields, for CLIs that
//! store conversations as protobuf blobs (Antigravity CLI).

/// A string field: nesting depth (0 = top level), field number, text.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub depth: usize,
    pub number: u64,
    pub text: String,
}

fn varint(bytes: &[u8], pos: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *bytes.get(*pos)?;
        *pos += 1;
        value |= ((byte & 0x7f) as u64) << shift;
        if byte < 0x80 {
            return Some(value);
        }
    }
    None
}

/// Length-delimited fields that are readable text; nested messages are walked into.
pub fn strings(bytes: &[u8]) -> Vec<Field> {
    let mut out = Vec::new();
    walk(bytes, 0, &mut out);
    out
}

fn readable(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|c| !c.is_control() || c == '\n' || c == '\t' || c == '\r')
}

fn walk(bytes: &[u8], depth: usize, out: &mut Vec<Field>) {
    if depth > 12 {
        return;
    }
    let mut pos = 0;
    while pos < bytes.len() {
        let Some(key) = varint(bytes, &mut pos) else { return };
        let number = key >> 3;
        match key & 7 {
            0 => {
                if varint(bytes, &mut pos).is_none() {
                    return;
                }
            }
            1 => pos += 8,
            5 => pos += 4,
            2 => {
                let Some(len) = varint(bytes, &mut pos) else { return };
                let end = pos.saturating_add(len as usize);
                let Some(sub) = bytes.get(pos..end) else { return };
                pos = end;
                match std::str::from_utf8(sub) {
                    Ok(text) if readable(text) => out.push(Field { depth, number, text: text.to_string() }),
                    _ => walk(sub, depth + 1, out),
                }
            }
            _ => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_nested_strings() {
        // field 1 { field 2: "hi" }, field 3: "top"
        let bytes = [0x0a, 0x04, 0x12, 0x02, b'h', b'i', 0x1a, 0x03, b't', b'o', b'p'];
        let fields = strings(&bytes);
        assert!(fields.contains(&Field { depth: 1, number: 2, text: "hi".into() }));
        assert!(fields.contains(&Field { depth: 0, number: 3, text: "top".into() }));
    }
}
