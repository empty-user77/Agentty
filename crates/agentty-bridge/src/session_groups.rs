//! Which project a session belongs to, for listing sessions by project instead of as one long
//! run: the repository's main working tree for a session started in it, in one of its folders or
//! in one of its worktrees; a plugin for the runs it started in its own data folder; and one
//! shared place for everything started in a temporary folder.

use crate::worktree;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GroupKind {
    /// A folder of the user's: a repository's main tree, or the folder itself outside git.
    Project,
    /// Runs a plugin started in its data folder (`plugin-data/<id>`).
    Plugin,
    /// Sessions started in a temporary folder: scripts, tests and one-off checks.
    Temporary,
    /// The session does not say where it ran.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionGroup {
    pub kind: GroupKind,
    /// The project's folder, or the plugin's data folder.
    pub root: Option<PathBuf>,
    /// The folder's name, or the plugin's id. Empty for the kinds the app names itself.
    pub name: String,
}

impl SessionGroup {
    fn of(kind: GroupKind) -> Self {
        SessionGroup { kind, root: None, name: String::new() }
    }

    fn project(root: PathBuf) -> Self {
        let name = root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| root.display().to_string());
        SessionGroup { kind: GroupKind::Project, root: Some(root), name }
    }

    /// Stable across refreshes: what a folded group is remembered by.
    pub fn key(&self) -> String {
        match (self.kind, &self.root) {
            (GroupKind::Project, Some(root)) => format!("project:{}", root.display()),
            (GroupKind::Plugin, _) => format!("plugin:{}", self.name),
            (GroupKind::Temporary, _) => "temporary".into(),
            _ => "unknown".into(),
        }
    }

    /// Whether these are runs nobody sat in front of (a plugin's, a script's): listed after the
    /// user's projects, folded.
    pub fn is_automated(&self) -> bool {
        matches!(self.kind, GroupKind::Plugin | GroupKind::Temporary)
    }
}

/// The folders that are not projects.
pub struct Places {
    /// Where plugins keep their data (`plugin-data`), one folder per plugin.
    pub plugin_data: Vec<PathBuf>,
    /// Where Agentty keeps the worktrees it makes, one folder per project.
    pub managed_worktrees: Vec<PathBuf>,
    pub temporary: Vec<PathBuf>,
}

impl Places {
    pub fn of_this_computer() -> Self {
        let mut temporary = vec![std::env::temp_dir()];
        if cfg!(unix) {
            // macOS names its per-user temporary folder both ways (`/var` is a link to
            // `/private/var`), and sessions record either.
            temporary.extend(
                ["/tmp", "/private/tmp", "/var/tmp", "/private/var/tmp", "/var/folders", "/private/var/folders"].map(PathBuf::from),
            );
        }
        // This build's data folder and the default one: a session says where it ran, whichever
        // build started it, and both are only read to name a group.
        let mut data = vec![crate::fsutil::data_dir()];
        let default = crate::fsutil::home().join(".agentty");
        if !data.contains(&default) {
            data.push(default);
        }
        Places {
            plugin_data: data.iter().map(|d| d.join("plugin-data")).collect(),
            managed_worktrees: data.iter().map(|d| d.join("worktrees")).collect(),
            temporary,
        }
    }
}

/// The group of each session, by the folder it ran in (`cwds`, in the sessions' order). Reads
/// `.git` entries: call it off the main thread.
pub fn assign(cwds: &[Option<&str>]) -> Vec<SessionGroup> {
    assign_in(cwds, &Places::of_this_computer())
}

pub fn assign_in(cwds: &[Option<&str>], places: &Places) -> Vec<SessionGroup> {
    let mut known: HashMap<&str, SessionGroup> = HashMap::new();
    // Worktrees Agentty made that are gone now, by the folder they were kept in
    // (`worktrees/<project>-<hash>`): they join their project once it is known from another session.
    let mut orphans: Vec<(&str, &Path, PathBuf)> = Vec::new();
    for cwd in cwds.iter().flatten() {
        if known.contains_key(cwd) {
            continue;
        }
        let path = Path::new(cwd);
        let group = if let Some((data, plugin)) = places.plugin_data.iter().find_map(|d| Some((d, child_of(path, d)?))) {
            SessionGroup { kind: GroupKind::Plugin, root: Some(data.join(&plugin)), name: plugin }
        } else if places.temporary.iter().any(|t| path.starts_with(t)) {
            SessionGroup::of(GroupKind::Temporary)
        } else if let Some(main) = worktree::main_tree(path).or_else(|| claude_worktree_owner(path)) {
            SessionGroup::project(main)
        } else if let Some((managed, home)) = places.managed_worktrees.iter().find_map(|m| Some((m, child_of(path, m)?))) {
            // Until its project is found, with the other worktrees kept in the same folder,
            // under the project's name (the folder's, without its hash).
            let kept_in = managed.join(&home);
            orphans.push((cwd, managed, kept_in.clone()));
            let name = home.rsplit_once('-').map_or(home.as_str(), |(name, _)| name).to_string();
            SessionGroup { kind: GroupKind::Project, root: Some(kept_in), name }
        } else {
            SessionGroup::project(path.to_path_buf())
        };
        known.insert(cwd, group);
    }
    for (cwd, managed, kept_in) in orphans {
        let owner = known.values().find(|g| {
            g.kind == GroupKind::Project
                && g.root.as_deref().is_some_and(|root| root != kept_in && worktree::managed_home_in(root, managed) == kept_in)
        });
        if let Some(owner) = owner.cloned() {
            known.insert(cwd, owner);
        }
    }
    cwds.iter().map(|cwd| cwd.and_then(|c| known.get(c).cloned()).unwrap_or_else(|| SessionGroup::of(GroupKind::Unknown))).collect()
}

/// The name of the folder right under `parent` that `path` is in (or is).
fn child_of(path: &Path, parent: &Path) -> Option<String> {
    match path.strip_prefix(parent).ok()?.components().next()? {
        Component::Normal(name) => Some(name.to_string_lossy().to_string()),
        _ => None,
    }
}

/// The project of a worktree Claude Code made (`<project>/.claude/worktrees/<name>`), by its path:
/// what is left to go by once the worktree was removed.
fn claude_worktree_owner(path: &Path) -> Option<PathBuf> {
    let parts: Vec<Component> = path.components().collect();
    let at = parts.windows(2).position(|w| w[0].as_os_str() == ".claude" && w[1].as_os_str() == "worktrees")?;
    (at > 0 && parts.len() > at + 2).then(|| parts[..at].iter().collect())
}

/// What tells a session's folder apart inside its group: the worktree's or the subfolder's path
/// from the project, the full path for a worktree kept elsewhere, nothing for the project's own
/// folder.
pub fn place_in_group(cwd: &Path, group: &SessionGroup) -> Option<String> {
    let root = group.root.as_deref()?;
    if cwd == root {
        return None;
    }
    match cwd.strip_prefix(root) {
        Ok(rest) => Some(rest.display().to_string()),
        // A worktree beside the project (`app-fix-login` next to `app`) or in Agentty's folder: its
        // own name says which.
        Err(_) => cwd.file_name().map(|n| n.to_string_lossy().to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("agentty-session-groups-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    /// Nothing is temporary here, so folders made for the test count as projects.
    fn places(base: &Path) -> Places {
        Places {
            plugin_data: vec![base.join("data/plugin-data")],
            managed_worktrees: vec![base.join("data/worktrees")],
            temporary: vec![base.join("tmp")],
        }
    }

    /// A repository at `main` with a linked worktree at `tree`, as git lays them out.
    fn link_worktree(main: &Path, tree: &Path, name: &str) {
        let private = main.join(".git/worktrees").join(name);
        fs::create_dir_all(&private).unwrap();
        fs::write(private.join("commondir"), "../..\n").unwrap();
        fs::create_dir_all(tree).unwrap();
        fs::write(tree.join(".git"), format!("gitdir: {}\n", private.display())).unwrap();
    }

    #[test]
    fn a_worktree_and_a_subfolder_are_listed_with_their_project() {
        let base = scratch("worktree");
        let main = base.join("app");
        fs::create_dir_all(main.join(".git")).unwrap();
        fs::create_dir_all(main.join("crates/core")).unwrap();
        let beside = base.join("app-fix-login");
        link_worktree(&main, &beside, "fix-login");
        let cwds = [main.clone(), main.join("crates/core"), beside.clone()].map(|p| p.display().to_string());
        let groups = assign_in(&cwds.iter().map(|c| Some(c.as_str())).collect::<Vec<_>>(), &places(&base));
        assert!(groups.iter().all(|g| g.kind == GroupKind::Project && g.root.as_deref() == Some(main.as_path())), "{groups:?}");
        assert_eq!(groups[0].name, "app");
        assert_eq!(place_in_group(&main, &groups[0]), None);
        assert_eq!(place_in_group(&main.join("crates/core"), &groups[1]), Some(Path::new("crates").join("core").display().to_string()));
        assert_eq!(place_in_group(&beside, &groups[2]).as_deref(), Some("app-fix-login"));
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn a_removed_worktree_still_finds_its_project() {
        let base = scratch("removed");
        let places = places(&base);
        let main = base.join("app");
        fs::create_dir_all(main.join(".git")).unwrap();
        // Claude Code's own worktrees say where they came from in their path.
        let claude = main.join(".claude/worktrees/gone");
        // Agentty's are kept in a folder named after the project, known once the project is.
        let managed = worktree::managed_home_in(&main, &places.managed_worktrees[0]).join("gone-too");
        let stray = places.managed_worktrees[0].join("other-00000000/tree");
        let cwds = [claude, managed, stray.clone(), main.clone()].map(|p| p.display().to_string());
        let groups = assign_in(&cwds.iter().map(|c| Some(c.as_str())).collect::<Vec<_>>(), &places);
        assert_eq!(groups[0].root.as_deref(), Some(main.as_path()));
        assert_eq!(groups[1].root.as_deref(), Some(main.as_path()));
        assert_eq!(
            (groups[2].root.as_deref(), groups[2].name.as_str()),
            (stray.parent(), "other"),
            "no project to join: the folder it was kept in"
        );
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn plugin_runs_temporary_folders_and_no_folder_are_groups_of_their_own() {
        let base = scratch("kinds");
        let places = places(&base);
        let cwds = [
            Some(places.plugin_data[0].join("x-feed/files/runs/7").display().to_string()),
            Some(base.join("tmp/check-1").display().to_string()),
            Some(base.join("tmp/check-2").display().to_string()),
            None,
        ];
        let groups = assign_in(&cwds.iter().map(|c| c.as_deref()).collect::<Vec<_>>(), &places);
        assert_eq!((groups[0].kind, groups[0].name.as_str()), (GroupKind::Plugin, "x-feed"));
        assert_eq!(groups[0].key(), "plugin:x-feed");
        assert_eq!(groups[1], groups[2]);
        assert_eq!(groups[1].kind, GroupKind::Temporary);
        assert_eq!(groups[3].kind, GroupKind::Unknown);
        assert!(groups[0].is_automated() && groups[1].is_automated() && !groups[3].is_automated());
        let _ = fs::remove_dir_all(base);
    }
}
