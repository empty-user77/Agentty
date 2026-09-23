//! Monitoring → Disk: how full the disk is, which folders take the room, and what can go without
//! anything noticing — a project's build output, tool caches that refill themselves, the trash.
//!
//! Only what this module found and recognised is ever removed, and it is checked again right
//! before it goes: a build folder still has the marker its tool writes (`CACHEDIR.TAG` in Cargo's
//! `target`) and the project file next to it, a cache is still the tool's own folder. Nothing is
//! followed through a symlink. Dependencies (`node_modules`, `~/.m2`), anything a project needs to
//! run, and anything that is not simply downloaded or built again are left out on purpose.

use serde::Serialize;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// The volume the home folder is on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Volume {
    pub total: u64,
    /// What can still be written (what the Finder calls available, minus purgeable space).
    pub available: u64,
}

impl Volume {
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.available)
    }
}

/// Size and free space of the volume `path` is on.
#[cfg(unix)]
pub fn volume(path: &Path) -> Option<Volume> {
    // `df -kP` is POSIX and says the same as the Finder; no unsafe code needed for it.
    let output = crate::process::command("df").arg("-kP").arg(path).stdin(std::process::Stdio::null()).output().ok()?;
    parse_df(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(windows)]
pub fn volume(path: &Path) -> Option<Volume> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let (mut available, mut total, mut free) = (0u64, 0u64, 0u64);
    // SAFETY: `wide` is NUL-terminated and outlives the call; the out pointers are valid u64s.
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut available, &mut total, &mut free) };
    (ok != 0).then_some(Volume { total, available })
}

/// `df -kP` output: a header, then `name 1024-blocks used available capacity mount`.
fn parse_df(raw: &str) -> Option<Volume> {
    let line = raw.lines().nth(1)?;
    let fields: Vec<&str> = line.split_whitespace().collect();
    // The device name may hold spaces; the numbers are counted from the end.
    let n = fields.len();
    if n < 6 {
        return None;
    }
    let total: u64 = fields[n - 5].parse().ok()?;
    let available: u64 = fields[n - 3].parse().ok()?;
    Some(Volume { total: total * 1024, available: available * 1024 })
}

/// Bytes a file really takes on disk (allocated blocks, so sparse files don't count in full).
fn allocated(meta: &fs::Metadata) -> u64 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        meta.blocks() * 512
    }
    #[cfg(not(unix))]
    {
        meta.len()
    }
}

/// A file with several names is counted once.
#[derive(Default)]
struct Seen(HashSet<(u64, u64)>);

impl Seen {
    fn first(&mut self, meta: &fs::Metadata) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if meta.nlink() > 1 {
                return self.0.insert((meta.dev(), meta.ino()));
            }
        }
        let _ = meta;
        true
    }
}

/// Folders a walk never enters. On macOS, reading another app's container asks the user for
/// permission ("… would like to access data from other apps"), and cloud folders may start
/// fetching: neither is worth it for a size.
fn skipped(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else { return false };
    let parent = path.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str()).unwrap_or("");
    cfg!(target_os = "macos")
        && parent == "Library"
        && matches!(name, "Containers" | "Group Containers" | "CloudStorage" | "Mobile Documents" | "Daemon Containers")
}

/// Total size of `path` (a file or a folder), never following symlinks. Stops early, returning
/// what it counted so far, once `cancel` is set.
pub fn size_of(path: &Path, cancel: &AtomicBool) -> u64 {
    let mut seen = Seen::default();
    size_in(path, cancel, &mut seen)
}

fn size_in(path: &Path, cancel: &AtomicBool, seen: &mut Seen) -> u64 {
    let Ok(meta) = fs::symlink_metadata(path) else { return 0 };
    if !meta.is_dir() {
        return if seen.first(&meta) { allocated(&meta) } else { 0 };
    }
    let mut total = allocated(&meta);
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                let child = entry.path();
                if !skipped(&child) {
                    total += allocated(&meta);
                    stack.push(child);
                }
            } else if seen.first(&meta) {
                total += allocated(&meta);
            }
        }
    }
    total
}

/// A folder and how much it holds; `children` are its biggest folders (one level).
#[derive(Debug, Clone, Default, Serialize)]
pub struct Usage {
    pub path: PathBuf,
    pub size: u64,
    pub children: Vec<Usage>,
}

/// What takes the room in the home folder: its folders (and files together as one entry, the
/// home folder itself), biggest first, each with its own biggest folders. The folders are measured
/// in parallel.
pub fn home_usage(cancel: &AtomicBool) -> Usage {
    usage_of(&crate::fsutil::home(), cancel)
}

fn usage_of(root: &Path, cancel: &AtomicBool) -> Usage {
    let mut folders = Vec::new();
    let mut loose = 0u64;
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() && !skipped(&entry.path()) {
                folders.push(entry.path());
            } else if !meta.is_dir() {
                loose += allocated(&meta);
            }
        }
    }
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(2, 8);
    let mut children = in_parallel(folders, threads, |folder| folder_usage(&folder, cancel));
    children.sort_by_key(|u| std::cmp::Reverse(u.size));
    let size = loose + children.iter().map(|c| c.size).sum::<u64>();
    Usage { path: root.to_path_buf(), size, children }
}

/// `work` over every item on `threads` threads; the results in no particular order.
pub fn in_parallel<T: Send, R: Send>(items: Vec<T>, threads: usize, work: impl Fn(T) -> R + Sync) -> Vec<R> {
    let queue = std::sync::Mutex::new(items);
    let results = std::sync::Mutex::new(Vec::new());
    // Taken in a call of its own, so the lock is let go before the work starts.
    let next = || queue.lock().ok().and_then(|mut q| q.pop());
    std::thread::scope(|scope| {
        for _ in 0..threads.max(1) {
            scope.spawn(|| {
                while let Some(item) = next() {
                    let result = work(item);
                    if let Ok(mut results) = results.lock() {
                        results.push(result);
                    }
                }
            });
        }
    });
    results.into_inner().unwrap_or_default()
}

/// A folder's size, and the sizes of the folders right inside it.
fn folder_usage(folder: &Path, cancel: &AtomicBool) -> Usage {
    let mut seen = Seen::default();
    let mut children = Vec::new();
    let mut size = fs::symlink_metadata(folder).map(|m| allocated(&m)).unwrap_or(0);
    if let Ok(entries) = fs::read_dir(folder) {
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            let path = entry.path();
            if meta.is_dir() {
                if skipped(&path) {
                    continue;
                }
                let child = size_in(&path, cancel, &mut seen);
                size += child;
                children.push(Usage { path, size: child, children: Vec::new() });
            } else if seen.first(&meta) {
                size += allocated(&meta);
            }
        }
    }
    children.sort_by_key(|u| std::cmp::Reverse(u.size));
    children.truncate(12);
    Usage { path: folder.to_path_buf(), size, children }
}

/// What kind of room an [`Item`] frees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    /// A project's build output: built again on the next build.
    Build,
    /// A cache: filled again when the tool needs it.
    Cache,
    /// The trash, old logs.
    Other,
}

/// Something that can go: build output of a project, a tool's cache, the trash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    /// Stable id of what it is (`cargo-target`, `npm`, `trash`, …): the UI names it from this.
    pub id: String,
    pub kind: Kind,
    /// The tool, as it calls itself (`Cargo`, `Next.js`, `npm`): proper nouns, not translated.
    pub tool: String,
    /// The project it belongs to; `None` for the tools' own caches and the rest.
    pub project: Option<PathBuf>,
    pub path: PathBuf,
    /// Only what is inside goes; the folder itself stays (a cache folder the tool expects).
    pub contents_only: bool,
    pub size: u64,
}

/// A folder a build tool writes next to its project file.
struct BuildRule {
    id: &'static str,
    tool: &'static str,
    kind: Kind,
    /// Relative to the folder holding `beside`.
    folder: &'static str,
    /// One of these must sit next to it (`Cargo.toml`), or nothing is claimed.
    beside: &'static [&'static str],
    /// And this must be inside it, when set (`CACHEDIR.TAG`, written by Cargo).
    marker: Option<&'static str>,
}

const BUILD_RULES: &[BuildRule] = &[
    BuildRule {
        id: "cargo-target",
        tool: "Cargo",
        kind: Kind::Build,
        folder: "target",
        beside: &["Cargo.toml"],
        marker: Some("CACHEDIR.TAG"),
    },
    BuildRule { id: "maven-target", tool: "Maven", kind: Kind::Build, folder: "target", beside: &["pom.xml"], marker: Some("classes") },
    BuildRule {
        id: "gradle-build",
        tool: "Gradle",
        kind: Kind::Build,
        folder: "build",
        beside: &["build.gradle", "build.gradle.kts"],
        marker: None,
    },
    BuildRule {
        id: "gradle-cache",
        tool: "Gradle",
        kind: Kind::Cache,
        folder: ".gradle",
        beside: &["build.gradle", "build.gradle.kts", "settings.gradle", "settings.gradle.kts"],
        marker: None,
    },
    BuildRule { id: "swift-build", tool: "SwiftPM", kind: Kind::Build, folder: ".build", beside: &["Package.swift"], marker: None },
    BuildRule { id: "next", tool: "Next.js", kind: Kind::Build, folder: ".next", beside: &["package.json"], marker: None },
    BuildRule { id: "nuxt", tool: "Nuxt", kind: Kind::Build, folder: ".nuxt", beside: &["package.json"], marker: None },
    BuildRule { id: "svelte-kit", tool: "SvelteKit", kind: Kind::Build, folder: ".svelte-kit", beside: &["package.json"], marker: None },
    BuildRule { id: "turbo", tool: "Turborepo", kind: Kind::Cache, folder: ".turbo", beside: &["package.json"], marker: None },
    BuildRule { id: "parcel", tool: "Parcel", kind: Kind::Cache, folder: ".parcel-cache", beside: &["package.json"], marker: None },
    BuildRule { id: "angular", tool: "Angular", kind: Kind::Cache, folder: ".angular/cache", beside: &["angular.json"], marker: None },
    BuildRule {
        id: "node-cache",
        tool: "node_modules/.cache",
        kind: Kind::Cache,
        folder: "node_modules/.cache",
        beside: &["package.json"],
        marker: None,
    },
    BuildRule { id: "vite", tool: "Vite", kind: Kind::Cache, folder: "node_modules/.vite", beside: &["package.json"], marker: None },
    BuildRule {
        id: "pytest",
        tool: "pytest",
        kind: Kind::Cache,
        folder: ".pytest_cache",
        beside: &["pyproject.toml", "setup.py", "setup.cfg", "pytest.ini", "tox.ini"],
        marker: Some("CACHEDIR.TAG"),
    },
    BuildRule {
        id: "mypy",
        tool: "mypy",
        kind: Kind::Cache,
        folder: ".mypy_cache",
        beside: &["pyproject.toml", "setup.py", "setup.cfg", "mypy.ini"],
        marker: Some("CACHEDIR.TAG"),
    },
    BuildRule {
        id: "ruff",
        tool: "Ruff",
        kind: Kind::Cache,
        folder: ".ruff_cache",
        beside: &["pyproject.toml", "ruff.toml", ".ruff.toml"],
        marker: Some("CACHEDIR.TAG"),
    },
];

/// Folders a search for nested projects never goes into.
fn not_a_project_folder(name: &str) -> bool {
    name.starts_with('.') || matches!(name, "node_modules" | "target" | "build" | "dist" | "vendor" | "Pods" | "DerivedData")
}

/// Whether `folder` (found at `project_dir/<rule.folder>`) still is what `rule` says it is.
fn matches_rule(project_dir: &Path, rule: &BuildRule) -> bool {
    let folder = project_dir.join(rule.folder);
    let real_dir = |p: &Path| fs::symlink_metadata(p).is_ok_and(|m| m.is_dir());
    real_dir(&folder)
        && rule.beside.iter().any(|name| project_dir.join(name).is_file())
        && rule.marker.is_none_or(|marker| folder.join(marker).exists())
        // Every folder on the way is a real folder too (`node_modules` could be a link elsewhere).
        && Path::new(rule.folder).ancestors().filter(|a| !a.as_os_str().is_empty()).all(|a| real_dir(&project_dir.join(a)))
}

/// Build output and caches inside the project at `root`, and in the projects nested up to two
/// folders below it (`apps/web`, `crates/x`). Sizes are measured.
pub fn project_items(root: &Path, cancel: &AtomicBool) -> Vec<Item> {
    let mut dirs = vec![root.to_path_buf()];
    let mut frontier = vec![root.to_path_buf()];
    for _ in 0..2 {
        let mut next = Vec::new();
        for dir in &frontier {
            let Ok(entries) = fs::read_dir(dir) else { continue };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if entry.file_type().is_ok_and(|t| t.is_dir()) && !not_a_project_folder(&name) {
                    next.push(entry.path());
                }
            }
        }
        dirs.extend(next.iter().cloned());
        frontier = next;
    }
    let mut items = Vec::new();
    let mut claimed: HashSet<PathBuf> = HashSet::new();
    for dir in &dirs {
        for rule in BUILD_RULES {
            if cancel.load(Ordering::Relaxed) {
                return items;
            }
            if !matches_rule(dir, rule) {
                continue;
            }
            let path = dir.join(rule.folder);
            // Cargo and Maven both say `target`; the first rule that fits takes it.
            if !claimed.insert(path.clone()) {
                continue;
            }
            let size = size_of(&path, cancel);
            items.push(Item {
                id: rule.id.to_string(),
                kind: rule.kind,
                tool: rule.tool.to_string(),
                project: Some(root.to_path_buf()),
                path,
                contents_only: false,
                size,
            });
        }
    }
    items
}

/// A tool's own cache folder: everything in it is downloaded or built again when needed.
struct CacheRule {
    id: &'static str,
    tool: &'static str,
    kind: Kind,
    /// Where it is, per platform, from the home folder (`~`) or the system cache folder (`@cache`).
    macos: &'static [&'static str],
    linux: &'static [&'static str],
    windows: &'static [&'static str],
}

const CACHE_RULES: &[CacheRule] = &[
    CacheRule {
        id: "npm",
        tool: "npm",
        kind: Kind::Cache,
        macos: &["~/.npm/_cacache"],
        linux: &["~/.npm/_cacache"],
        windows: &["@cache/npm-cache/_cacache"],
    },
    CacheRule {
        id: "yarn",
        tool: "Yarn",
        kind: Kind::Cache,
        macos: &["@cache/Yarn"],
        linux: &["@cache/yarn"],
        windows: &["@cache/Yarn/Cache"],
    },
    CacheRule {
        id: "bun",
        tool: "Bun",
        kind: Kind::Cache,
        macos: &["~/.bun/install/cache"],
        linux: &["~/.bun/install/cache"],
        windows: &["~/.bun/install/cache"],
    },
    CacheRule { id: "pip", tool: "pip", kind: Kind::Cache, macos: &["@cache/pip"], linux: &["@cache/pip"], windows: &["@cache/pip/cache"] },
    CacheRule { id: "uv", tool: "uv", kind: Kind::Cache, macos: &["~/.cache/uv"], linux: &["@cache/uv"], windows: &["@cache/uv/cache"] },
    CacheRule {
        id: "go-build",
        tool: "Go",
        kind: Kind::Cache,
        macos: &["@cache/go-build"],
        linux: &["@cache/go-build"],
        windows: &["@cache/go-build"],
    },
    CacheRule {
        id: "cargo-registry",
        tool: "Cargo",
        kind: Kind::Cache,
        macos: &["~/.cargo/registry/cache", "~/.cargo/registry/src", "~/.cargo/git/checkouts"],
        linux: &["~/.cargo/registry/cache", "~/.cargo/registry/src", "~/.cargo/git/checkouts"],
        windows: &["~/.cargo/registry/cache", "~/.cargo/registry/src", "~/.cargo/git/checkouts"],
    },
    CacheRule {
        id: "gradle",
        tool: "Gradle",
        kind: Kind::Cache,
        macos: &["~/.gradle/caches"],
        linux: &["~/.gradle/caches"],
        windows: &["~/.gradle/caches"],
    },
    CacheRule {
        id: "homebrew",
        tool: "Homebrew",
        kind: Kind::Cache,
        macos: &["@cache/Homebrew"],
        linux: &["@cache/Homebrew"],
        windows: &[],
    },
    CacheRule { id: "cocoapods", tool: "CocoaPods", kind: Kind::Cache, macos: &["@cache/CocoaPods"], linux: &[], windows: &[] },
    CacheRule {
        id: "xcode-derived",
        tool: "Xcode",
        kind: Kind::Build,
        macos: &["~/Library/Developer/Xcode/DerivedData"],
        linux: &[],
        windows: &[],
    },
    CacheRule {
        id: "jetbrains",
        tool: "JetBrains",
        kind: Kind::Cache,
        macos: &["@cache/JetBrains"],
        linux: &["@cache/JetBrains"],
        windows: &["@cache/JetBrains"],
    },
    CacheRule {
        id: "trash",
        tool: "",
        kind: Kind::Other,
        macos: &["~/.Trash"],
        linux: &["~/.local/share/Trash/files", "~/.local/share/Trash/info"],
        windows: &[],
    },
    CacheRule { id: "logs", tool: "", kind: Kind::Other, macos: &["~/Library/Logs"], linux: &[], windows: &[] },
];

fn cache_paths(rule: &CacheRule) -> Vec<PathBuf> {
    let list = if cfg!(target_os = "macos") {
        rule.macos
    } else if cfg!(windows) {
        rule.windows
    } else {
        rule.linux
    };
    let home = crate::fsutil::home();
    let cache = dirs::cache_dir();
    list.iter()
        .filter_map(|spec| {
            if let Some(rest) = spec.strip_prefix("~/") {
                Some(home.join(rest))
            } else {
                spec.strip_prefix("@cache/").and_then(|rest| cache.as_ref().map(|c| c.join(rest)))
            }
        })
        .collect()
}

/// The tools' caches and the rest that exist on this computer, measured.
pub fn global_items(cancel: &AtomicBool) -> Vec<Item> {
    let mut items = Vec::new();
    for rule in CACHE_RULES {
        for path in cache_paths(rule) {
            if cancel.load(Ordering::Relaxed) {
                return items;
            }
            if !fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
                continue;
            }
            let size = size_of(&path, cancel);
            items.push(Item {
                id: rule.id.to_string(),
                kind: rule.kind,
                tool: rule.tool.to_string(),
                project: None,
                path,
                contents_only: true,
                size,
            });
        }
    }
    items
}

/// Whether `item` is still something this module would offer: the checks that found it pass
/// again, so a folder that changed since the scan (or an item built by hand) is refused.
fn still_valid(item: &Item) -> bool {
    match &item.project {
        Some(project) => {
            let Some(rule) = BUILD_RULES.iter().find(|r| r.id == item.id) else { return false };
            // The folder is `rule.folder` inside the project or a project nested in it.
            let Some(dir) = item.path.to_str().and_then(|p| p.strip_suffix(rule.folder)).map(PathBuf::from) else { return false };
            let dir = dir.components().collect::<PathBuf>();
            !item.contents_only && dir.starts_with(project) && matches_rule(&dir, rule)
        }
        None => {
            item.contents_only
                && CACHE_RULES.iter().find(|r| r.id == item.id).is_some_and(|rule| cache_paths(rule).contains(&item.path))
                && fs::symlink_metadata(&item.path).is_ok_and(|m| m.is_dir())
        }
    }
}

/// Removes what `item` stands for and returns how many bytes were freed (measured before and
/// after, since some files may be in use and stay). Refuses anything [`still_valid`] rejects.
pub fn clean(item: &Item) -> anyhow::Result<u64> {
    anyhow::ensure!(still_valid(item), "{} is not something Agentty cleans", item.path.display());
    let never = AtomicBool::new(false);
    let before = size_of(&item.path, &never);
    let mut first_error = None;
    if item.contents_only {
        for entry in fs::read_dir(&item.path)?.flatten() {
            let path = entry.path();
            let result = match entry.file_type() {
                Ok(t) if t.is_dir() => fs::remove_dir_all(&path),
                _ => fs::remove_file(&path),
            };
            if let Err(err) = result {
                first_error.get_or_insert(err);
            }
        }
    } else if let Err(err) = fs::remove_dir_all(&item.path) {
        first_error = Some(err);
    }
    let after = if item.path.exists() { size_of(&item.path, &never) } else { 0 };
    let freed = before.saturating_sub(after);
    match first_error {
        // Something in use or protected stayed; what could go, went.
        Some(err) if freed == 0 => Err(err.into()),
        _ => Ok(freed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("agentty-disk-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn reads_df() {
        let raw = "Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/disk3s5 482797652 314572800 146800640 69% /System/Volumes/Data\n";
        let volume = parse_df(raw).unwrap();
        assert_eq!(volume.total, 482797652 * 1024);
        assert_eq!(volume.available, 146800640 * 1024);
        assert_eq!(volume.used(), (482797652 - 146800640) * 1024);
        // A device name with a space still reads.
        let spaced = "Filesystem 1024-blocks Used Available Capacity Mounted on\nmap auto home 100 40 60 40% /home\n";
        assert_eq!(parse_df(spaced).unwrap().available, 60 * 1024);
        assert!(parse_df("").is_none());
    }

    #[test]
    fn finds_build_output_only_where_a_tool_made_it() {
        let dir = scratch("build");
        // A Cargo project with its target, and an app nested two folders down with .next.
        fs::write(dir.join("Cargo.toml"), "[package]\n").unwrap();
        fs::create_dir_all(dir.join("target/debug")).unwrap();
        fs::write(dir.join("target/CACHEDIR.TAG"), "Signature: 8a477f597d28d172789f06886806bc55\n").unwrap();
        fs::write(dir.join("target/debug/app"), vec![0u8; 8192]).unwrap();
        fs::create_dir_all(dir.join("apps/web/.next/cache")).unwrap();
        fs::write(dir.join("apps/web/package.json"), "{}").unwrap();
        fs::write(dir.join("apps/web/.next/cache/x"), vec![0u8; 4096]).unwrap();
        // Look-alikes that are nobody's build output: a `target` folder without Cargo's tag, a
        // `build` folder without a Gradle file, a `.next` without a package.json.
        fs::create_dir_all(dir.join("docs/target")).unwrap();
        fs::write(dir.join("docs/Cargo.toml"), "").unwrap();
        fs::create_dir_all(dir.join("assets/build")).unwrap();
        fs::create_dir_all(dir.join("notes/.next")).unwrap();

        let never = AtomicBool::new(false);
        let items = project_items(&dir, &never);
        let mut ids: Vec<(&str, PathBuf)> =
            items.iter().map(|i| (i.id.as_str(), i.path.strip_prefix(&dir).unwrap().to_path_buf())).collect();
        ids.sort();
        assert_eq!(ids, vec![("cargo-target", PathBuf::from("target")), ("next", PathBuf::from("apps/web/.next"))]);
        assert!(items.iter().all(|i| i.size > 0 && i.project.as_deref() == Some(dir.as_path())));

        // Cleaning removes exactly that folder; the project stays.
        let target = items.iter().find(|i| i.id == "cargo-target").unwrap();
        assert!(clean(target).unwrap() > 0);
        assert!(!dir.join("target").exists() && dir.join("Cargo.toml").exists());
        // Asked again, it is gone: nothing to clean, and it is refused rather than guessed at.
        assert!(clean(target).is_err());

        // An item that was not found by the scan is refused: a path outside the project, a rule
        // that does not fit, a folder that lost its project file.
        let forged = Item { path: dir.join("docs/target"), ..target.clone() };
        assert!(clean(&forged).is_err());
        let elsewhere =
            Item { project: Some(dir.join("apps")), path: dir.join("apps/web/.next"), id: "cargo-target".into(), ..target.clone() };
        assert!(clean(&elsewhere).is_err());
        fs::remove_file(dir.join("apps/web/package.json")).unwrap();
        let next = items.iter().find(|i| i.id == "next").unwrap();
        assert!(clean(next).is_err());
        assert!(dir.join("apps/web/.next").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn never_follows_a_link() {
        let dir = scratch("link");
        let outside = scratch("link-outside");
        fs::write(outside.join("keep.txt"), "keep").unwrap();
        fs::write(dir.join("package.json"), "{}").unwrap();
        std::os::unix::fs::symlink(&outside, dir.join(".next")).unwrap();
        let never = AtomicBool::new(false);
        assert!(project_items(&dir, &never).is_empty(), "a link is not build output");
        let forged = Item {
            id: "next".into(),
            kind: Kind::Build,
            tool: "Next.js".into(),
            project: Some(dir.clone()),
            path: dir.join(".next"),
            contents_only: false,
            size: 1,
        };
        assert!(clean(&forged).is_err());
        assert!(outside.join("keep.txt").exists());
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&outside);
    }

    #[test]
    fn only_known_cache_folders_are_emptied() {
        // A made-up cache item is refused, whatever it points at.
        let dir = scratch("cache");
        let forged = Item {
            id: "npm".into(),
            kind: Kind::Cache,
            tool: "npm".into(),
            project: None,
            path: dir.clone(),
            contents_only: true,
            size: 1,
        };
        assert!(clean(&forged).is_err());
        let unknown = Item { id: "whatever".into(), ..forged.clone() };
        assert!(clean(&unknown).is_err());
        assert!(dir.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn measures_folders() {
        let dir = scratch("usage");
        fs::create_dir_all(dir.join("big/inner")).unwrap();
        fs::create_dir_all(dir.join("small")).unwrap();
        fs::write(dir.join("big/inner/a"), vec![1u8; 64 * 1024]).unwrap();
        fs::write(dir.join("small/b"), vec![1u8; 4096]).unwrap();
        let never = AtomicBool::new(false);
        let usage = usage_of(&dir, &never);
        assert_eq!(usage.children.len(), 2);
        assert!(usage.children[0].path.ends_with("big") && usage.children[0].size >= 64 * 1024);
        assert!(usage.children[0].children[0].path.ends_with("inner"));
        assert!(usage.size >= usage.children[0].size + usage.children[1].size);
        let _ = fs::remove_dir_all(&dir);
    }
}
