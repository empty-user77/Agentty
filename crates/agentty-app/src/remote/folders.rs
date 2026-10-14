//! The folder the remote page may open workspaces in: `~/AgenttyRemote`, and nothing outside it.
//! The page lists its subfolders, makes new ones and opens one as a workspace; every path it sends
//! is a list of folder names under that root, checked here name by name and again once resolved,
//! so neither `..`, an absolute path nor a symbolic link leads out of it. Hidden folders are not
//! shown and can't be named.
//!
//! The folder is the user's own, shared on purpose by every build and data folder, like
//! `~/AgenttyProjects`: what is in it are projects, not app state.

use std::io;
use std::path::{Path, PathBuf};

/// The folder's name in the home folder.
pub const ROOT_NAME: &str = "AgenttyRemote";
/// Folders listed at most, and folder levels at most below the root.
const MAX_LISTED: usize = 500;
const MAX_DEPTH: usize = 16;
/// Characters in a folder name at most.
const MAX_NAME: usize = 100;

/// `~/AgenttyRemote`.
pub fn root() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(ROOT_NAME))
}

/// Whether `name` may name a folder here: not empty, not hidden, no path separators, no
/// characters a file system or a shell line would trip on.
pub fn valid_name(name: &str) -> bool {
    let count = name.chars().count();
    (1..=MAX_NAME).contains(&count)
        && name.trim() == name
        && !name.starts_with('.')
        && !name.chars().any(|c| c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
}

/// The folder at `path` (names joined with `/`, `""` for the root itself) under `root`, which is
/// made if it isn't there yet. `InvalidInput` for a path that names anything else.
pub fn resolve(root: &Path, path: &str) -> io::Result<PathBuf> {
    let refused = || io::Error::new(io::ErrorKind::InvalidInput, "not a folder under the remote folder");
    let names: Vec<&str> = if path.is_empty() { Vec::new() } else { path.split('/').collect() };
    if names.len() > MAX_DEPTH || !names.iter().all(|name| valid_name(name)) {
        return Err(refused());
    }
    std::fs::create_dir_all(root)?;
    let base = root.canonicalize()?;
    let mut folder = base.clone();
    folder.extend(&names);
    let folder = folder.canonicalize()?;
    if !folder.starts_with(&base) || !folder.is_dir() {
        return Err(refused());
    }
    Ok(folder)
}

/// The subfolders of `path`, sorted by name (hidden ones left out).
pub fn list(root: &Path, path: &str) -> io::Result<Vec<String>> {
    let folder = resolve(root, path)?;
    let mut names: Vec<String> = std::fs::read_dir(folder)?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| valid_name(name))
        .take(MAX_LISTED)
        .collect();
    names.sort_by_key(|name| name.to_lowercase());
    Ok(names)
}

/// Makes the folder `name` in `path` and returns it. `AlreadyExists` when it is there.
pub fn create(root: &Path, path: &str, name: &str) -> io::Result<PathBuf> {
    if !valid_name(name) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a folder name"));
    }
    let folder = resolve(root, path)?.join(name);
    std::fs::create_dir(&folder)?;
    Ok(folder)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("agentty-remote-folders-{}-{}", std::process::id(), crate::ui::now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("AgenttyRemote")
    }

    #[test]
    fn names_are_plain_folder_names() {
        assert!(valid_name("my-app") && valid_name("새 프로젝트") && valid_name("a b"));
        for bad in ["", ".", "..", ".git", "a/b", "a\\b", " a", "a ", "a\nb", "a:b", &"x".repeat(101)] {
            assert!(!valid_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn stays_inside_the_root() {
        let root = temp_root();
        assert_eq!(list(&root, "").unwrap(), Vec::<String>::new(), "made on first use");
        create(&root, "", "app").unwrap();
        create(&root, "app", "web").unwrap();
        std::fs::create_dir(root.join(".hidden")).unwrap();
        std::fs::write(root.join("notes.txt"), "x").unwrap();
        assert_eq!(list(&root, "").unwrap(), vec!["app".to_string()], "folders only, hidden ones left out");
        assert_eq!(list(&root, "app").unwrap(), vec!["web".to_string()]);
        assert!(resolve(&root, "app/web").unwrap().ends_with("AgenttyRemote/app/web"));
        assert_eq!(create(&root, "", "app").unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        for bad in ["..", "app/..", "/etc", "app//web", ".hidden", "missing", "notes.txt"] {
            assert!(resolve(&root, bad).is_err(), "{bad:?}");
        }
        assert!(create(&root, "", "../out").is_err());
        #[cfg(unix)]
        {
            let outside = root.parent().unwrap().join("outside");
            std::fs::create_dir(&outside).unwrap();
            std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
            assert!(resolve(&root, "link").is_err(), "a link out of the root");
            assert!(!list(&root, "").unwrap().contains(&"link".to_string()));
        }
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }
}
