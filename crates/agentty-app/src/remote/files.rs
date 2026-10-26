//! Files the remote page may open: a picture, a PDF or a text file named in a terminal's output,
//! shown in the page's viewer the way the app's file editor shows it.
//!
//! A signed-in page can already run anything in a terminal, so this opens nothing new to whoever
//! holds the session; it is kept narrow anyway, so a stray path in some output can only ever show
//! what it plainly is:
//! - only inside the terminal's working tree or its workspace's folder, checked after every link
//!   in the path is followed (`canonicalize`), so `..` and symbolic links lead nowhere else;
//! - nothing hidden on the way (`.env`, `.ssh`, `.git`, …) and no key or certificate files;
//! - only pictures, PDFs and text, each with a size limit; text always goes out as plain text,
//!   never as a page, and the page shows it through `textContent`.

use std::path::{Component, Path, PathBuf};

/// Longest path the page may ask for.
pub const MAX_PATH: usize = 1024;
/// Largest picture or PDF sent.
pub const MAX_BINARY: u64 = 20 * 1024 * 1024;
/// Largest text sent; a longer file is cut here (the page says so).
pub const MAX_TEXT: u64 = 2 * 1024 * 1024;

/// What a file is shown as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A picture, with its media type.
    Image(&'static str),
    Pdf,
    /// Text, Markdown included (`markdown`: the page renders it).
    Text {
        markdown: bool,
    },
}

impl Kind {
    pub fn media_type(self) -> &'static str {
        match self {
            Kind::Image(media) => media,
            Kind::Pdf => "application/pdf",
            Kind::Text { .. } => "text/plain; charset=utf-8",
        }
    }

    fn limit(self) -> u64 {
        match self {
            Kind::Text { .. } => MAX_TEXT,
            _ => MAX_BINARY,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum FileError {
    /// Not a path that may be asked for (empty, too long, a hidden part, a key file).
    Refused,
    /// Not there, not a file, or outside the folders it may come from.
    NotFound,
    /// A kind the viewer does not show.
    Unsupported,
    /// Larger than its kind's limit (pictures and PDFs; text is cut instead).
    TooLarge,
}

const TEXT: &[&str] = &[
    "txt",
    "log",
    "md",
    "markdown",
    "mdx",
    "json",
    "jsonl",
    "yaml",
    "yml",
    "toml",
    "csv",
    "tsv",
    "xml",
    "html",
    "htm",
    "css",
    "scss",
    "js",
    "mjs",
    "cjs",
    "jsx",
    "ts",
    "tsx",
    "rs",
    "py",
    "rb",
    "go",
    "java",
    "kt",
    "kts",
    "swift",
    "c",
    "h",
    "cc",
    "cpp",
    "hpp",
    "cs",
    "php",
    "sh",
    "bash",
    "zsh",
    "fish",
    "sql",
    "graphql",
    "proto",
    "vue",
    "svelte",
    "gradle",
    "ini",
    "cfg",
    "conf",
    "diff",
    "patch",
    "lock",
    "dart",
    "lua",
    "r",
    "scala",
    "ex",
    "exs",
    "erl",
    "hs",
    "ml",
    "clj",
    "tf",
    "dockerfile",
    "makefile",
];

/// Names that are never shown, whatever their extension says: private keys and certificates.
const SECRET_EXTENSIONS: &[&str] = &["pem", "key", "p12", "pfx", "keystore", "jks", "der", "crt", "cer", "asc", "gpg", "kdbx"];

/// The kind `path` is shown as, by its name; `None`: not shown.
pub fn kind_of(path: &Path) -> Option<Kind> {
    let name = path.file_name()?.to_str()?.to_ascii_lowercase();
    if name.starts_with("id_rsa") || name.starts_with("id_ed25519") || name.starts_with("id_ecdsa") || name.starts_with("id_dsa") {
        return None;
    }
    let ext = match name.rsplit_once('.') {
        Some((_, ext)) => ext.to_string(),
        // `Dockerfile`, `Makefile`: text by their whole name.
        None => name.clone(),
    };
    if SECRET_EXTENSIONS.contains(&ext.as_str()) {
        return None;
    }
    Some(match ext.as_str() {
        "png" => Kind::Image("image/png"),
        "jpg" | "jpeg" => Kind::Image("image/jpeg"),
        "gif" => Kind::Image("image/gif"),
        "webp" => Kind::Image("image/webp"),
        "svg" => Kind::Image("image/svg+xml"),
        "bmp" => Kind::Image("image/bmp"),
        "ico" => Kind::Image("image/x-icon"),
        "avif" => Kind::Image("image/avif"),
        "pdf" => Kind::Pdf,
        "md" | "markdown" | "mdx" => Kind::Text { markdown: true },
        ext if TEXT.contains(&ext) => Kind::Text { markdown: false },
        _ => return None,
    })
}

/// The file `requested` names, as a terminal working in `dir` would read it (`~/` is the home
/// folder), checked against `roots`. Returns the real path and how it is shown.
pub fn resolve(requested: &str, dir: &Path, roots: &[PathBuf], home: &Path) -> Result<(PathBuf, Kind), FileError> {
    let requested = requested.trim();
    if requested.is_empty() || requested.len() > MAX_PATH || requested.contains('\0') {
        return Err(FileError::Refused);
    }
    let home_relative = requested.strip_prefix("~/").or_else(|| if cfg!(windows) { requested.strip_prefix("~\\") } else { None });
    let path = match home_relative {
        Some(rest) => home.join(rest),
        None => dir.join(requested),
    };
    // Hidden parts as written (`.env`, `.git/…`): refused before anything is looked up. `.` and
    // `..` are not names; where they lead is checked below.
    if path.components().any(|c| matches!(c, Component::Normal(name) if name.to_string_lossy().starts_with('.'))) {
        return Err(FileError::Refused);
    }
    let kind = kind_of(&path).ok_or(FileError::Unsupported)?;
    let real = path.canonicalize().map_err(|_| FileError::NotFound)?;
    let meta = std::fs::metadata(&real).map_err(|_| FileError::NotFound)?;
    if !meta.is_file() {
        return Err(FileError::NotFound);
    }
    // Where it really is (links followed): inside a root, with nothing hidden between the root and
    // the file, and still a kind that is shown (a link named `notes.txt` to a key is not text).
    let inside =
        roots.iter().filter_map(|root| root.canonicalize().ok()).find_map(|root| real.strip_prefix(&root).ok().map(Path::to_path_buf));
    let Some(within) = inside else { return Err(FileError::NotFound) };
    if within.components().any(|c| matches!(c, Component::Normal(name) if name.to_string_lossy().starts_with('.'))) {
        return Err(FileError::Refused);
    }
    let kind = match kind_of(&real) {
        Some(real_kind) if real_kind == kind => kind,
        _ => return Err(FileError::Unsupported),
    };
    if !matches!(kind, Kind::Text { .. }) && meta.len() > kind.limit() {
        return Err(FileError::TooLarge);
    }
    Ok((real, kind))
}

/// The bytes to send for `path`: the whole file, or for text its first `MAX_TEXT` bytes (cut at a
/// character boundary) and whether it was cut.
pub fn read(path: &Path, kind: Kind) -> std::io::Result<(Vec<u8>, bool)> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.by_ref().take(kind.limit() + 1).read_to_end(&mut bytes)?;
    let cut = bytes.len() as u64 > kind.limit();
    if cut {
        bytes.truncate(kind.limit() as usize);
        // Never half a character: back up to where one starts.
        while !bytes.is_empty() && std::str::from_utf8(&bytes).is_err() {
            bytes.pop();
        }
    }
    Ok((bytes, cut))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tree(PathBuf);
    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn tree(name: &str) -> (Tree, PathBuf) {
        let root = std::env::temp_dir().join(format!("agentty-files-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["project/src", "project/.git", "project/shots", "elsewhere"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        for (file, text) in [
            ("project/README.md", "# hi"),
            ("project/src/main.rs", "fn main() {}"),
            ("project/shots/a.png", "png"),
            ("project/.env", "KEY=example_not_a_real_value"),
            ("project/.git/config", "x"),
            ("project/server.key", "example"),
            ("project/app.exe", "x"),
            ("elsewhere/notes.txt", "outside"),
        ] {
            std::fs::write(root.join(file), text).unwrap();
        }
        let canonical = root.canonicalize().unwrap();
        (Tree(root), canonical)
    }

    #[test]
    fn files_in_the_tree_open_as_their_kind() {
        let (_guard, root) = tree("open");
        let project = root.join("project");
        let roots = [project.clone()];
        let at = |p: &str| {
            resolve(p, &project.join("src"), &roots, &root).map(|(path, kind)| (path.strip_prefix(&project).unwrap().to_path_buf(), kind))
        };
        assert_eq!(at("main.rs"), Ok((PathBuf::from("src/main.rs"), Kind::Text { markdown: false })));
        assert_eq!(at("../README.md"), Ok((PathBuf::from("README.md"), Kind::Text { markdown: true })));
        assert_eq!(at(project.join("shots/a.png").to_str().unwrap()), Ok((PathBuf::from("shots/a.png"), Kind::Image("image/png"))));
    }

    #[test]
    fn nothing_outside_hidden_or_secret_opens() {
        let (_guard, root) = tree("refuse");
        let project = root.join("project");
        let roots = [project.clone()];
        let at = |p: &str| resolve(p, &project, &roots, &root).err();
        assert_eq!(at("../elsewhere/notes.txt"), Some(FileError::NotFound), "out of the tree");
        assert_eq!(at("~/elsewhere/notes.txt"), Some(FileError::NotFound), "home is not a root");
        assert_eq!(at(".env"), Some(FileError::Refused));
        assert_eq!(at(".git/config"), Some(FileError::Refused));
        assert_eq!(at("server.key"), Some(FileError::Unsupported));
        assert_eq!(at("app.exe"), Some(FileError::Unsupported));
        assert_eq!(at("missing.md"), Some(FileError::NotFound));
        assert_eq!(at(""), Some(FileError::Refused));
        assert_eq!(at(&"a".repeat(MAX_PATH + 1)), Some(FileError::Refused));
    }

    #[cfg(unix)]
    #[test]
    fn a_link_can_not_lead_out() {
        let (_guard, root) = tree("link");
        let project = root.join("project");
        std::os::unix::fs::symlink(root.join("elsewhere/notes.txt"), project.join("notes.txt")).unwrap();
        std::os::unix::fs::symlink(project.join(".env"), project.join("env.txt")).unwrap();
        let roots = [project.clone()];
        assert_eq!(resolve("notes.txt", &project, &roots, &root).err(), Some(FileError::NotFound));
        assert_eq!(resolve("env.txt", &project, &roots, &root).err(), Some(FileError::Refused));
    }

    #[test]
    fn long_text_is_cut_at_a_character() {
        let (_guard, root) = tree("cut");
        let file = root.join("project/long.txt");
        let mut text = "a".repeat(MAX_TEXT as usize - 1);
        text.push('한');
        std::fs::write(&file, &text).unwrap();
        let (bytes, cut) = read(&file, Kind::Text { markdown: false }).unwrap();
        assert!(cut);
        assert_eq!(bytes.len(), MAX_TEXT as usize - 1, "the cut character is left out whole");
        assert!(std::str::from_utf8(&bytes).is_ok());
    }
}
