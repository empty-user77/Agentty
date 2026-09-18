//! `agentty://` links other apps open to hand work to Agentty.
//!
//! - `agentty://plugin/<id>/<path>?<query>` → delivered to plugin `<id>` as `url/open`
//! - `agentty://prompt?text=…&file=…&title=…&agent=claude|codex|shell&cwd=…` → asks where to send it
//! - `agentty://plugins[/<id>]` → opens the plugin store
//!
//! Anything arriving by link is untrusted: prompts always go through the "Send to…" dialog.

use super::manifest::valid_id;
use super::PromptRequest;
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const SCHEME: &str = "agentty";

/// Largest file a `prompt` link may attach.
const MAX_FILE_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub enum Link {
    Plugin { id: String, path: String, query: BTreeMap<String, String> },
    Prompt(PromptRequest),
    Store { plugin: Option<String> },
}

pub fn parse(raw: &str) -> Result<Link, String> {
    let url = url::Url::parse(raw).map_err(|e| format!("invalid link: {e}"))?;
    if url.scheme() != SCHEME {
        return Err(format!("not an {SCHEME}:// link"));
    }
    let query: BTreeMap<String, String> = url.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
    let segments: Vec<String> = url.path_segments().map(|s| s.filter(|p| !p.is_empty()).map(percent_decode).collect()).unwrap_or_default();
    match url.host_str().unwrap_or_default() {
        "plugin" => {
            let id = segments.first().cloned().unwrap_or_default();
            if !valid_id(&id) {
                return Err("link names no valid plugin".into());
            }
            Ok(Link::Plugin { id, path: segments[1..].join("/"), query })
        }
        "plugins" => Ok(Link::Store { plugin: segments.first().cloned().filter(|id| valid_id(id)) }),
        "prompt" => {
            let mut text = query.get("text").cloned().unwrap_or_default();
            if let Some(file) = query.get("file") {
                let content = read_text_file(&PathBuf::from(file))?;
                if !text.is_empty() {
                    text.push_str("\n\n");
                }
                text.push_str(&content);
            }
            if text.trim().is_empty() {
                return Err("the link has no prompt text".into());
            }
            Ok(Link::Prompt(PromptRequest {
                text,
                title: query.get("title").cloned(),
                agent: query.get("agent").cloned(),
                cwd: query.get("cwd").map(PathBuf::from).filter(|p| p.is_absolute()),
                // Who asked is decided by Agentty (a link, or the plugin's real name), never by
                // the link itself: `source=Agentty Update` must not look like a trusted sender.
                source: None,
                ..PromptRequest::default()
            }))
        }
        other => Err(format!("unknown {SCHEME}:// link \"{other}\"")),
    }
}

/// Text files only (Markdown / plain text), and not huge. The rules are checked on the resolved
/// path, so a symlink can't lead into a hidden folder or to a file of another kind.
fn read_text_file(path: &std::path::Path) -> Result<String, String> {
    if !path.is_absolute() {
        return Err("only absolute paths can be attached".into());
    }
    let resolved = std::fs::canonicalize(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let allowed =
        resolved.extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e.to_lowercase().as_str(), "md" | "markdown" | "txt"));
    // No hidden folders (`~/.ssh`, `~/.config`, …): links come from anywhere.
    let hidden = resolved.components().any(|c| matches!(c, std::path::Component::Normal(name) if name.to_string_lossy().starts_with('.')));
    if !allowed || hidden {
        return Err("only .md or .txt files outside hidden folders can be attached".into());
    }
    let metadata = std::fs::metadata(&resolved).map_err(|e| format!("{}: {e}", resolved.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is not a file", resolved.display()));
    }
    if metadata.len() > MAX_FILE_BYTES {
        return Err(format!("{} is larger than 1 MB", resolved.display()));
    }
    std::fs::read_to_string(&resolved).map_err(|e| format!("{}: {e}", resolved.display()))
}

fn percent_decode(segment: &str) -> String {
    url::form_urlencoded::parse(format!("x={}", segment.replace('+', "%2B")).as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())
        .unwrap_or_else(|| segment.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plugin_links() {
        let link = parse("agentty://plugin/cosmica/continue?path=%2FUsers%2Fme%2FNote%2Fa%20b.md&title=%ED%95%9C").unwrap();
        let Link::Plugin { id, path, query } = link else { panic!("not a plugin link") };
        assert_eq!((id.as_str(), path.as_str()), ("cosmica", "continue"));
        assert_eq!(query["path"], "/Users/me/Note/a b.md");
        assert_eq!(query["title"], "한");
        assert!(parse("agentty://plugin/Bad_Id/x").is_err());
        assert!(parse("https://plugin/cosmica").is_err());
    }

    #[test]
    fn parses_prompt_and_store_links() {
        let Link::Prompt(request) = parse("agentty://prompt?text=Fix+the+build&agent=codex&cwd=%2Ftmp&title=Build").unwrap() else {
            panic!("not a prompt link")
        };
        assert_eq!(request.text, "Fix the build");
        assert_eq!(request.agent.as_deref(), Some("codex"));
        assert_eq!(request.cwd, Some(PathBuf::from("/tmp")));
        assert!(request.submit);
        assert!(parse("agentty://prompt?text=").is_err());
        assert!(parse("agentty://prompt?file=%2Fetc%2Fpasswd").is_err());
        assert!(parse("agentty://prompt?file=%2FUsers%2Fme%2F.config%2Fnotes.md").is_err());
        // The link cannot claim who sent it.
        let Link::Prompt(request) = parse("agentty://prompt?text=hi&source=Agentty%20Update").unwrap() else { panic!() };
        assert_eq!(request.source, None);
        assert_eq!(parse("agentty://plugins/cosmica").unwrap(), Link::Store { plugin: Some("cosmica".into()) });
        assert_eq!(parse("agentty://plugins").unwrap(), Link::Store { plugin: None });
    }

    #[test]
    fn attaches_markdown_files() {
        let dir = std::env::temp_dir().join(format!("agentty-link-{}", std::process::id()));
        let hidden = dir.join(".private");
        std::fs::create_dir_all(&hidden).unwrap();
        let path = dir.join("note.md");
        std::fs::write(&path, "# Note\nbody").unwrap();
        let encode = |p: &std::path::Path| -> String { url::form_urlencoded::byte_serialize(p.display().to_string().as_bytes()).collect() };
        let Link::Prompt(request) = parse(&format!("agentty://prompt?text=Continue&file={}", encode(&path))).unwrap() else { panic!() };
        assert_eq!(request.text, "Continue\n\n# Note\nbody");

        // A symlink that leads into a hidden folder is refused, like the folder itself.
        let secret = hidden.join("secret.md");
        std::fs::write(&secret, "# Secret").unwrap();
        #[cfg(unix)]
        {
            let link = dir.join("shortcut.md");
            let _ = std::fs::remove_file(&link);
            std::os::unix::fs::symlink(&secret, &link).unwrap();
            assert!(parse(&format!("agentty://prompt?text=Continue&file={}", encode(&link))).is_err());
        }
        assert!(parse(&format!("agentty://prompt?text=Continue&file={}", encode(&secret))).is_err());
        std::fs::remove_dir_all(dir).ok();
    }
}
