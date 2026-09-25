//! Markdown preview: the file rendered the way VS Code's markdown preview renders it — the same
//! markdown-it setup, highlight.js, VS Code's preview styles — in a web view over the editor's text
//! area (`assets/markdown-preview`, see its README for versions and licenses).
//!
//! The page is built in memory and has no address. Only its bundled scripts run: the
//! Content-Security-Policy lists their hashes, and the document's own HTML is sanitized
//! (DOMPurify) before it is shown. The page reads no files: a local image is read here, only from
//! the project or the document's folder, and handed over as a `data:` URL. A click on a link is
//! queued in the page and picked up here, so web pages open in a browser and files in the editor.

use crate::webview::WebView;
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

const WEBVIEW_DEFAULTS: &str = include_str!("../../assets/markdown-preview/webview-defaults.css");
const THEME: &str = include_str!("../../assets/markdown-preview/theme.css");
const MARKDOWN_CSS: &str = include_str!("../../assets/markdown-preview/markdown.css");
const HIGHLIGHT_CSS: &str = include_str!("../../assets/markdown-preview/highlight.css");
const MARKDOWN_IT: &str = include_str!("../../assets/markdown-preview/markdown-it.min.js");
const HIGHLIGHT_JS: &str = include_str!("../../assets/markdown-preview/highlight.min.js");
const PURIFY: &str = include_str!("../../assets/markdown-preview/purify.min.js");
const PREVIEW_JS: &str = include_str!("../../assets/markdown-preview/preview.js");

/// Images larger than this are not shown (they would travel into the page as text).
const MAX_IMAGE_BYTES: u64 = 16 * 1024 * 1024;

/// The page: styles and scripts inline, each script allowed by its hash and nothing else.
pub fn page_html() -> String {
    let scripts = [MARKDOWN_IT, HIGHLIGHT_JS, PURIFY, PREVIEW_JS];
    let hashes: Vec<String> = scripts.iter().map(|script| format!("'sha256-{}'", base64_of(&Sha256::digest(script.as_bytes())))).collect();
    let csp = format!(
        "default-src 'none'; script-src {}; style-src 'unsafe-inline'; img-src https: data:; media-src https:; font-src data:",
        hashes.join(" ")
    );
    let mut html = String::with_capacity(scripts.iter().map(|s| s.len()).sum::<usize>() + 32 * 1024);
    html.push_str("<!DOCTYPE html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n");
    html.push_str(&format!("<meta http-equiv=\"Content-Security-Policy\" content=\"{csp}\">\n"));
    for style in [WEBVIEW_DEFAULTS, THEME, MARKDOWN_CSS, HIGHLIGHT_CSS] {
        html.push_str("<style>\n");
        html.push_str(style);
        html.push_str("\n</style>\n");
    }
    for script in scripts {
        html.push_str("<script>");
        html.push_str(script);
        html.push_str("</script>\n");
    }
    html.push_str("</head>\n<body class=\"vscode-body vscode-dark\">\n<div class=\"markdown-body\" dir=\"auto\" id=\"agentty-markdown\"></div>\n</body>\n</html>\n");
    html
}

fn base64_of(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// What a reply from the page brings back to the editor.
pub enum PageReply {
    /// The document at `revision` is shown; `images` are the local images it still needs.
    Rendered { path: PathBuf, revision: u64, images: Vec<String> },
    /// The page was not ready (still loading) or failed: try again on the next tick.
    NotRendered,
    /// Links the user clicked, as written in the document.
    Links(Vec<String>),
}

/// The web view and what it shows.
pub struct Preview {
    pub view: Rc<RefCell<Option<WebView>>>,
    /// The document and text revision the page shows.
    pub shown: Option<(PathBuf, u64)>,
    /// A render call is on its way.
    pub rendering: bool,
    /// Asking the page for clicked links is on its way.
    pub asking: bool,
    /// Something drawn by the app is over the text area, or the editor is not on screen.
    pub covered: bool,
}

impl Preview {
    pub fn new() -> Self {
        Self { view: Rc::new(RefCell::new(None)), shown: None, rendering: false, asking: false, covered: true }
    }

    pub fn hide(&mut self) {
        if let Some(view) = self.view.borrow_mut().as_mut() {
            view.hide();
        }
    }
}

/// The web view's settings: scripts on (the renderer is a script), nothing kept on disk, no pop-ups.
pub fn view_settings() -> crate::settings::BrowserSettings {
    crate::settings::BrowserSettings {
        javascript: true,
        popups: false,
        zoom: 1.0,
        mobile: false,
        private_mode: true,
        inspectable: false,
        ..Default::default()
    }
}

/// Where a link or image path written in `document` points on disk: relative to its folder, or —
/// starting with `/` — to the project's root, as in VS Code. `None` for web addresses and anchors.
pub fn local_target(written: &str, document: &Path, project: &Path) -> Option<PathBuf> {
    let written = written.split(['#', '?']).next().unwrap_or_default();
    if written.is_empty() || written.starts_with("//") || has_scheme(written) {
        return None;
    }
    let decoded = percent_decode(written);
    let path = match decoded.strip_prefix('/') {
        Some(rest) if !project.as_os_str().is_empty() => project.join(rest),
        _ => document.parent()?.join(&decoded),
    };
    Some(path)
}

/// Whether `link` starts with a URL scheme (`https:`, `mailto:`, `file:`, …).
pub fn has_scheme(link: &str) -> bool {
    let Some((scheme, _)) = link.split_once(':') else { return false };
    // A Windows drive letter (`C:\…`) is a path, not a scheme.
    scheme.len() > 1
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Some(byte) = std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The image at `path` as a `data:` URL, when it is an image, not too large, and inside the
/// project or the document's folder (a document must not be able to show files from elsewhere).
pub fn image_data_url(path: &Path, document: &Path, project: &Path) -> Option<String> {
    let mime = match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "avif" => "image/avif",
        _ => return None,
    };
    let real = path.canonicalize().ok()?;
    let inside = |root: &Path| !root.as_os_str().is_empty() && root.canonicalize().is_ok_and(|root| real.starts_with(root));
    if !inside(project) && !document.parent().is_some_and(inside) {
        return None;
    }
    let metadata = real.metadata().ok()?;
    if !metadata.is_file() || metadata.len() > MAX_IMAGE_BYTES {
        return None;
    }
    let bytes = std::fs::read(&real).ok()?;
    Some(format!("data:{mime};base64,{}", base64_of(&bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_script_is_allowed_by_its_hash_and_nothing_else_runs() {
        let html = page_html();
        let csp = html.split("content=\"").nth(1).and_then(|s| s.split('"').next()).expect("CSP");
        assert!(csp.starts_with("default-src 'none'; script-src 'sha256-"));
        assert!(!csp.contains("script-src 'unsafe"));
        assert_eq!(csp.matches("'sha256-").count(), 4);
        assert!(!csp.contains("file:"));
    }

    #[test]
    fn links_resolve_like_vs_code() {
        let doc = Path::new("/p/docs/guide.md");
        let project = Path::new("/p");
        assert_eq!(local_target("other.md#part", doc, project), Some(PathBuf::from("/p/docs/other.md")));
        assert_eq!(local_target("/README.md", doc, project), Some(PathBuf::from("/p/README.md")));
        assert_eq!(local_target("a%20b.png", doc, project), Some(PathBuf::from("/p/docs/a b.png")));
        assert_eq!(local_target("https://example.com", doc, project), None);
        assert_eq!(local_target("mailto:someone@example.com", doc, project), None);
        assert_eq!(local_target("#part", doc, project), None);
        assert_eq!(local_target("//cdn.example.com/x.png", doc, project), None);
    }

    #[test]
    fn images_outside_the_project_are_not_read() {
        let dir = std::env::temp_dir().join(format!("agentty-md-preview-{}", std::process::id()));
        let project = dir.join("project");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("a.png"), [0x89, b'P', b'N', b'G']).unwrap();
        std::fs::write(dir.join("outside.png"), [0x89, b'P', b'N', b'G']).unwrap();
        std::fs::write(project.join("notes.txt"), "not an image").unwrap();
        let doc = project.join("README.md");
        assert!(image_data_url(&project.join("a.png"), &doc, &project).is_some_and(|u| u.starts_with("data:image/png;base64,")));
        assert_eq!(image_data_url(&project.join("../outside.png"), &doc, &project), None);
        assert_eq!(image_data_url(&project.join("notes.txt"), &doc, &project), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
