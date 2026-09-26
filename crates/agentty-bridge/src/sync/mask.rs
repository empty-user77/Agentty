//! Masks credentials in transcript text before it leaves the computer.
//!
//! A transcript holds whatever an agent read or printed: `.env` files, tokens in command output,
//! private keys. The sync repository is encrypted, but a second layer costs little: well-known
//! credential shapes are replaced by `***` before encryption. The shapes follow
//! `scripts/check-secrets.py`. Replacing only characters inside a token keeps every JSON line valid.

/// Prefixes of token formats, each followed by at least `min` token characters.
const PREFIXES: &[(&str, usize)] = &[
    ("sk-ant-", 20),
    ("sk-proj-", 20),
    ("sk-", 32),
    ("ghp_", 30),
    ("gho_", 30),
    ("ghu_", 30),
    ("ghs_", 30),
    ("ghr_", 30),
    ("github_pat_", 30),
    ("glpat-", 20),
    ("xoxb-", 20),
    ("xoxp-", 20),
    ("xoxa-", 20),
    ("xapp-", 20),
    ("AIza", 30),
    ("figd_", 20),
    ("sb_secret_", 20),
    ("npm_", 30),
    ("hf_", 30),
    ("AKIA", 16),
    ("ASIA", 16),
    ("pk_live_", 20),
    ("sk_live_", 20),
    ("rk_live_", 20),
];

const MASK: &str = "***";
/// PEM armour, split so this file does not read as holding a key.
const DASHES: &str = "-----";
const PEM_BEGIN: &str = "BEGIN ";
const PEM_END: &str = "END ";
const PEM_PRIVATE: &str = "PRIVATE KEY";

fn token_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'-'
}

/// `text` with credentials replaced by `***`.
pub fn mask(text: &str) -> String {
    let masked = mask_private_keys(text);
    mask_tokens(&masked)
}

fn mask_tokens(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut i = 0;
    while i < bytes.len() {
        // A token starts at a word boundary.
        let boundary = i == 0 || !token_char(bytes[i - 1]);
        let hit = boundary
            .then(|| {
                PREFIXES.iter().find_map(|(prefix, min)| {
                    if !bytes[i..].starts_with(prefix.as_bytes()) {
                        return None;
                    }
                    let start = i + prefix.len();
                    let run = bytes[start..].iter().take_while(|&&c| token_char(c)).count();
                    (run >= *min).then_some((start, start + run))
                })
            })
            .flatten();
        match hit {
            Some((start, end)) => {
                out.push_str(&text[copied..start]);
                out.push_str(MASK);
                copied = end;
                i = end;
            }
            None => i += 1,
        }
    }
    out.push_str(&text[copied..]);
    out
}

/// PEM private keys, raw or JSON-escaped (`\n` inside a string): the body goes, the armour stays.
fn mask_private_keys(text: &str) -> String {
    let begin = format!("{DASHES}{PEM_BEGIN}");
    let end_marker = format!("{DASHES}{PEM_END}");
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(&begin) {
        let after_begin = &rest[at + begin.len()..];
        // The label (`RSA PRIVATE KEY`, `OPENSSH PRIVATE KEY`, …) ends at the next dashes.
        let Some(label_end) = after_begin.find(DASHES) else { break };
        let label = &after_begin[..label_end];
        let body_start = at + begin.len() + label_end + DASHES.len();
        if !label.ends_with(PEM_PRIVATE) || label.len() > 40 {
            out.push_str(&rest[..body_start]);
            rest = &rest[body_start..];
            continue;
        }
        out.push_str(&rest[..body_start]);
        out.push_str(MASK);
        rest = match rest[body_start..].find(&end_marker) {
            Some(end) => &rest[body_start + end..],
            // No end marker in this text: everything after the header is the key.
            None => "",
        };
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Credential-shaped values built at runtime, so this file never holds one.
    fn fake(prefix: &str, len: usize) -> String {
        format!("{prefix}{}", "examplenotarealkey0".repeat(4).chars().take(len).collect::<String>())
    }

    fn pem(label: &str, body: &str, newline: &str) -> String {
        format!("{DASHES}{PEM_BEGIN}{label}{DASHES}{newline}{body}{newline}{DASHES}{PEM_END}{label}{DASHES}")
    }

    #[test]
    fn known_token_shapes_are_masked() {
        for (prefix, len) in [("sk-ant-api03-", 40), ("ghp_", 36), ("github_pat_", 60), ("AKIA", 16), ("xoxb-", 30)] {
            let token = fake(prefix, len);
            let line = format!("{{\"text\":\"export TOKEN={token} done\"}}");
            let masked = mask(&line);
            assert!(!masked.contains(&token), "{prefix} not masked: {masked}");
            assert!(masked.contains("TOKEN="), "{masked}");
            assert!(serde_json::from_str::<serde_json::Value>(&masked).is_ok(), "{masked}");
        }
    }

    #[test]
    fn ordinary_text_is_left_alone() {
        let glued = format!("de{}", fake("sk-", 40));
        for text in ["task-list and sk-short", "the AKIA prefix alone", "ask-anthropic-now", glued.as_str()] {
            assert_eq!(mask(text), text);
        }
    }

    #[test]
    fn private_key_bodies_are_masked_in_json_strings() {
        let body = "MIIEexamplenotarealkeybody";
        let label = format!("RSA {PEM_PRIVATE}");
        let line = format!("{{\"out\":\"{}\\n\"}}", pem(&label, body, "\\n"));
        let masked = mask(&line);
        assert!(!masked.contains(body));
        assert!(masked.contains(&format!("{DASHES}{PEM_BEGIN}{label}{DASHES}***{DASHES}{PEM_END}{label}{DASHES}")), "{masked}");
        assert!(serde_json::from_str::<serde_json::Value>(&masked).is_ok());
        // A certificate is not a secret.
        let cert = pem("CERTIFICATE", "MIIexample", "\n");
        assert_eq!(mask(&cert), cert);
    }
}
