//! What the browser's address field turns input into.

/// Turns what the user typed into a URL: full URLs pass through, bare hosts get `https://`,
/// anything else becomes a search.
/// `normalize_url` with a search engine prefix (the encoded query is appended).
pub fn normalize_url_with(input: &str, search_prefix: &str) -> String {
    let input = input.trim();
    if input.contains("://") || input.starts_with("about:") {
        return input.to_string();
    }
    let local = ["localhost", "127.0.0.1", "0.0.0.0", "[::1]"].iter().any(|h| input.starts_with(h));
    if local {
        return format!("http://{input}");
    }
    if !input.contains(' ') && input.contains('.') {
        return format!("https://{input}");
    }
    let query: String = input
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            b' ' => "+".into(),
            _ => format!("%{b:02X}"),
        })
        .collect();
    format!("{search_prefix}{query}")
}

#[cfg(test)]
mod tests {
    use super::normalize_url_with;

    fn normalize_url(input: &str) -> String {
        normalize_url_with(input, "https://www.google.com/search?q=")
    }

    #[test]
    fn normalizes_input() {
        assert_eq!(normalize_url("https://a.dev/x"), "https://a.dev/x");
        assert_eq!(normalize_url("localhost:3000"), "http://localhost:3000");
        assert_eq!(normalize_url("github.com/empty-user77"), "https://github.com/empty-user77");
        assert_eq!(normalize_url("rust gpui"), "https://www.google.com/search?q=rust+gpui");
        assert_eq!(normalize_url_with("러스트", "https://duckduckgo.com/?q="), "https://duckduckgo.com/?q=%EB%9F%AC%EC%8A%A4%ED%8A%B8");
    }
}
