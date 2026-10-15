//! Run configurations of a project, the way an IDE offers them: the `package.json` scripts of a
//! Node project (with the package manager its lockfile names: npm, pnpm, yarn or bun) and the
//! Spring Boot applications of a Gradle or Maven build, plus commands the user added by hand.
//!
//! Detection only reads files — no build tool runs — and looks at the project folder and two
//! levels below it (`web/`, `apps/admin/`), skipping dependency and build output folders.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Folders below the project root that are looked into for more projects.
const SCAN_DEPTH: usize = 2;
/// Folders looked at in all, so a huge tree can't make detection slow.
const MAX_DIRS: usize = 400;
/// Source files read while looking for `@SpringBootApplication`.
const MAX_SOURCES: usize = 3_000;
/// Folders never looked into: dependencies, build output, tool state.
const SKIPPED: &[&str] =
    &["node_modules", "target", "build", "dist", "out", "vendor", "venv", "bin", "obj", "coverage", "tmp", "logs", "__pycache__"];
/// Scripts of a package below the root that are offered: the ones that start something. The
/// root's package offers all of its scripts.
const RUN_WORDS: &[&str] = &["dev", "start", "serve", "preview", "watch", "storybook"];
/// Scripts shown first, in this order.
const FIRST: &[&str] = &["dev", "start", "serve", "develop", "start:dev", "preview", "watch", "storybook"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tool {
    Npm,
    Pnpm,
    Yarn,
    Bun,
    Gradle,
    Maven,
    Custom,
}

impl Tool {
    pub fn label(self) -> &'static str {
        match self {
            Tool::Npm => "npm",
            Tool::Pnpm => "pnpm",
            Tool::Yarn => "yarn",
            Tool::Bun => "bun",
            Tool::Gradle => "Gradle",
            Tool::Maven => "Maven",
            Tool::Custom => "",
        }
    }

    pub fn is_node(self) -> bool {
        matches!(self, Tool::Npm | Tool::Pnpm | Tool::Yarn | Tool::Bun)
    }
}

/// Something the run button can start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunConfig {
    /// Stable within the project: `<folder>#<script>`, `<folder>#spring`, `custom#<n>`.
    pub id: String,
    /// What it is called: the script's name, the Spring Boot module's name, the user's name.
    pub name: String,
    /// The folder it belongs to, relative to the project ("" for the project itself).
    pub scope: String,
    pub tool: Tool,
    /// The command line, for the pane's shell.
    pub command: String,
    /// Where the command runs.
    pub dir: PathBuf,
    /// Kotlin sources (for a Spring Boot application), to say so.
    pub kotlin: bool,
    /// What the command runs, where it says (a `package.json` script's body; the command itself
    /// for a command added by hand): where the port it serves on is looked for.
    pub script: String,
}

impl RunConfig {
    /// Ports the configuration names (`--port 3000`, `PORT=8080`, `http.server 5198`): the ones
    /// to free when it fails because a port was taken and its output doesn't say which.
    pub fn named_ports(&self) -> Vec<u16> {
        // Only a number said to be a port: `--port 3000`, `-p 8080`, `PORT=8080`, `:5173`,
        // `http.server 5198` — not `--max-old-space-size=4096`.
        let lower = self.script.to_ascii_lowercase();
        let mut ports: Vec<u16> = Vec::new();
        let bytes = lower.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if !bytes[i].is_ascii_digit() || i > 0 && bytes[i - 1].is_ascii_digit() {
                i += 1;
                continue;
            }
            let end = (i..bytes.len()).find(|&j| !bytes[j].is_ascii_digit()).unwrap_or(bytes.len());
            let before = lower[..i].trim_end_matches([' ', '=']);
            let said = before.ends_with(':') || before.ends_with("port") || before.ends_with("-p") || before.ends_with("server");
            if let Some(port) = lower[i..end].parse::<u16>().ok().filter(|p| *p >= 1024 && said) {
                if !ports.contains(&port) {
                    ports.push(port);
                }
            }
            i = end;
        }
        ports
    }
}

impl RunConfig {
    /// "web › dev", or just "dev" at the project's root.
    pub fn title(&self) -> String {
        if self.scope.is_empty() {
            self.name.clone()
        } else {
            format!("{} › {}", self.scope, self.name)
        }
    }

    pub fn is_custom(&self) -> bool {
        self.tool == Tool::Custom
    }
}

/// A command the user added to a project by hand.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomRun {
    pub name: String,
    pub command: String,
    /// Folder relative to the project ("" for the project itself).
    #[serde(default)]
    pub dir: String,
}

/// The project's run configurations: what was detected, then the user's own commands.
pub fn configs(root: &Path, custom: &[CustomRun]) -> Vec<RunConfig> {
    let mut found = detect(root);
    for (index, run) in custom.iter().enumerate() {
        let dir = if run.dir.is_empty() { root.to_path_buf() } else { root.join(&run.dir) };
        found.push(RunConfig {
            id: format!("custom#{index}"),
            name: if run.name.trim().is_empty() { run.command.clone() } else { run.name.clone() },
            scope: String::new(),
            tool: Tool::Custom,
            command: run.command.clone(),
            dir,
            kotlin: false,
            script: run.command.clone(),
        });
    }
    found
}

/// What the run button can start in the project at `root` (detected only).
pub fn detect(root: &Path) -> Vec<RunConfig> {
    let mut found = Vec::new();
    for dir in project_dirs(root) {
        let scope = dir.strip_prefix(root).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_default();
        found.extend(spring(root, &dir, &scope));
        found.extend(node(root, &dir, &scope));
    }
    found
}

/// `root` and the folders below it, breadth first, as deep as [`SCAN_DEPTH`].
fn project_dirs(root: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![root.to_path_buf()];
    let mut level = vec![root.to_path_buf()];
    for _ in 0..SCAN_DEPTH {
        let mut next = Vec::new();
        for dir in &level {
            let Ok(entries) = std::fs::read_dir(dir) else { continue };
            let mut children: Vec<PathBuf> = entries
                .flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .filter(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    !name.starts_with('.') && !SKIPPED.contains(&name.as_str())
                })
                .map(|e| e.path())
                .collect();
            children.sort();
            next.extend(children);
            if dirs.len() + next.len() >= MAX_DIRS {
                break;
            }
        }
        next.truncate(MAX_DIRS.saturating_sub(dirs.len()));
        dirs.extend(next.iter().cloned());
        level = next;
    }
    dirs
}

// -- Node ---------------------------------------------------------------------------------------

fn node(root: &Path, dir: &Path, scope: &str) -> Vec<RunConfig> {
    let Some(text) = read_small(&dir.join("package.json")) else { return Vec::new() };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else { return Vec::new() };
    let Some(scripts) = json.get("scripts").and_then(|s| s.as_object()) else { return Vec::new() };
    let tool = package_manager(root, dir, json.get("packageManager").and_then(|v| v.as_str()));
    let mut names: Vec<&String> = scripts
        .iter()
        .filter(|(_, body)| body.is_string())
        .map(|(name, _)| name)
        .filter(|name| scope.is_empty() || is_run_script(name))
        // `predev`, `postinstall`: hooks the package manager runs by itself.
        .filter(|name| !is_hook(name, scripts))
        // A name starting with `-` would reach the package manager as an option.
        .filter(|name| !name.starts_with('-'))
        .collect();
    names.sort_by_key(|name| (FIRST.iter().position(|f| f == name).unwrap_or(FIRST.len()), name.to_string()));
    names
        .into_iter()
        .map(|name| RunConfig {
            id: format!("{scope}#{name}"),
            name: name.clone(),
            scope: scope.to_string(),
            tool,
            command: format!("{} run {}", tool.label(), quote(name)),
            dir: dir.to_path_buf(),
            kotlin: false,
            script: scripts.get(name).and_then(|s| s.as_str()).unwrap_or_default().to_string(),
        })
        .collect()
}

/// `predev` / `postbuild`: run by the package manager around `dev` / `build`, not by themselves.
fn is_hook(name: &str, scripts: &serde_json::Map<String, serde_json::Value>) -> bool {
    ["pre", "post"].iter().any(|prefix| name.strip_prefix(prefix).is_some_and(|rest| scripts.contains_key(rest)))
        || matches!(name, "prepare" | "preinstall" | "postinstall" | "prepublishOnly")
}

fn is_run_script(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    RUN_WORDS.iter().any(|word| lower.split([':', '-', '_']).any(|part| part == *word)) || lower == "develop"
}

/// The package manager `dir` is set up for: `packageManager` in its `package.json`, else the
/// lockfile in it or in a folder above it up to the project's root (a workspace keeps one lockfile
/// at its root), else npm.
fn package_manager(root: &Path, dir: &Path, declared: Option<&str>) -> Tool {
    if let Some(declared) = declared {
        let name = declared.split('@').next().unwrap_or_default();
        match name {
            "pnpm" => return Tool::Pnpm,
            "yarn" => return Tool::Yarn,
            "bun" => return Tool::Bun,
            "npm" => return Tool::Npm,
            _ => {}
        }
    }
    for folder in dir.ancestors() {
        if folder.join("pnpm-lock.yaml").exists() || folder.join("pnpm-workspace.yaml").exists() {
            return Tool::Pnpm;
        }
        if folder.join("yarn.lock").exists() {
            return Tool::Yarn;
        }
        if folder.join("bun.lockb").exists() || folder.join("bun.lock").exists() {
            return Tool::Bun;
        }
        if folder.join("package-lock.json").exists() {
            return Tool::Npm;
        }
        if folder == root || !folder.starts_with(root) {
            break;
        }
    }
    Tool::Npm
}

// -- Spring Boot --------------------------------------------------------------------------------

fn spring(root: &Path, dir: &Path, scope: &str) -> Option<RunConfig> {
    let gradle = ["build.gradle.kts", "build.gradle"].iter().map(|f| dir.join(f)).find(|p| p.is_file());
    let maven = dir.join("pom.xml");
    let (tool, build) = match gradle {
        Some(file) => (Tool::Gradle, read_small(&file)?),
        None if maven.is_file() => (Tool::Maven, read_small(&maven)?),
        None => return None,
    };
    // Spring Boot somewhere in the build: the module's own file, or the project's (a root build
    // that applies the plugin to its modules, a version catalog the module takes it from as
    // `alias(libs.plugins.spring.boot)`). The application is where the main class is.
    if !(mentions_boot(&build) || project_mentions_boot(root)) || !has_boot_application(dir) {
        return None;
    }
    let kotlin = dir.join("src/main/kotlin").is_dir() || dir.join("build.gradle.kts").is_file() && build.contains("kotlin(");
    let name = if scope.is_empty() {
        root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "app".into())
    } else {
        scope.rsplit('/').next().unwrap_or(scope).to_string()
    };
    let (command, run_dir) = match tool {
        Tool::Gradle => gradle_command(root, dir),
        _ => maven_command(root, dir),
    };
    Some(RunConfig {
        // Not `#…`: that is a script's id, and a script may be called `spring`.
        id: format!("{scope}@spring-boot"),
        name,
        scope: if scope.is_empty() { String::new() } else { parent_scope(scope) },
        tool,
        command,
        dir: run_dir,
        kotlin,
        // Spring Boot names the taken port itself ("Port 8080 was already in use").
        script: String::new(),
    })
}

/// "apps/api" → "apps": a module is named by its own folder, shown under the one above it.
fn parent_scope(scope: &str) -> String {
    scope.rsplit_once('/').map(|(parent, _)| parent.to_string()).unwrap_or_default()
}

/// Whether `dir/src/main` has a class annotated `@SpringBootApplication`.
fn mentions_boot(build: &str) -> bool {
    ["springframework.boot", "spring.boot", "spring-boot"].iter().any(|word| build.contains(word))
}

/// Whether the project's own build files (at its root) bring in Spring Boot.
fn project_mentions_boot(root: &Path) -> bool {
    ["build.gradle.kts", "build.gradle", "pom.xml", "gradle/libs.versions.toml"]
        .iter()
        .filter_map(|file| read_small(&root.join(file)))
        .any(|text| mentions_boot(&text))
}

/// Answers of [`has_boot_application`] for a while: detection runs again every few seconds while
/// a project is shown, and a module without one would have its sources read each time.
static BOOT_APPS: std::sync::Mutex<Option<std::collections::HashMap<PathBuf, (std::time::Instant, bool)>>> = std::sync::Mutex::new(None);
const BOOT_APPS_FOR: std::time::Duration = std::time::Duration::from_secs(120);

fn has_boot_application(dir: &Path) -> bool {
    if let Ok(mut cache) = BOOT_APPS.lock() {
        let cache = cache.get_or_insert_with(Default::default);
        if let Some((at, found)) = cache.get(dir).filter(|(at, _)| at.elapsed() < BOOT_APPS_FOR) {
            let _ = at;
            return *found;
        }
    }
    let found = scan_for_boot_application(dir);
    if let Ok(mut cache) = BOOT_APPS.lock() {
        let cache = cache.get_or_insert_with(Default::default);
        cache.retain(|_, (at, _)| at.elapsed() < BOOT_APPS_FOR);
        cache.insert(dir.to_path_buf(), (std::time::Instant::now(), found));
    }
    found
}

fn scan_for_boot_application(dir: &Path) -> bool {
    let mut stack = vec![dir.join("src/main")];
    let mut read = 0;
    while let Some(folder) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                if path.file_name().is_none_or(|n| n != "resources") {
                    stack.push(path);
                }
                continue;
            }
            if !path.extension().is_some_and(|e| e == "java" || e == "kt") {
                continue;
            }
            read += 1;
            if read > MAX_SOURCES {
                return false;
            }
            // The annotation sits above the class, near the top: the head of the file is enough.
            if read_head(&path, SOURCE_HEAD).is_some_and(|text| text.contains("@SpringBootApplication")) {
                return true;
            }
        }
    }
    false
}

/// The nearest folder from `dir` up to `root` that has `file` (a build tool's wrapper script).
fn wrapper(root: &Path, dir: &Path, file: &str) -> Option<PathBuf> {
    dir.ancestors().take_while(|folder| folder.starts_with(root)).find(|folder| folder.join(file).is_file()).map(Path::to_path_buf)
}

/// How a wrapper script in the folder the pane runs in is started.
fn wrapper_call(folder: &Path, name: &str) -> String {
    if cfg!(windows) {
        return format!(".\\{name}.{}", if name == "gradlew" { "bat" } else { "cmd" });
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let executable = std::fs::metadata(folder.join(name)).is_ok_and(|m| m.permissions().mode() & 0o111 != 0);
        if !executable {
            return format!("sh {name}");
        }
    }
    let _ = folder;
    format!("./{name}")
}

/// `./gradlew bootRun` in the module, or `./gradlew :api:bootRun` from the wrapper's folder for a
/// module of a multi-project build; `gradle bootRun` without a wrapper.
fn gradle_command(root: &Path, dir: &Path) -> (String, PathBuf) {
    match wrapper(root, dir, "gradlew") {
        Some(folder) if folder == dir => (format!("{} bootRun", wrapper_call(&folder, "gradlew")), folder),
        Some(folder) => {
            let path = dir.strip_prefix(&folder).map(|p| p.to_string_lossy().replace(['/', '\\'], ":")).unwrap_or_default();
            (format!("{} {}", wrapper_call(&folder, "gradlew"), quote(&format!(":{path}:bootRun"))), folder)
        }
        None => ("gradle bootRun".to_string(), dir.to_path_buf()),
    }
}

/// `./mvnw spring-boot:run` in the module, or `./mvnw -f api/pom.xml spring-boot:run` from the
/// wrapper's folder; `mvn spring-boot:run` without a wrapper.
fn maven_command(root: &Path, dir: &Path) -> (String, PathBuf) {
    match wrapper(root, dir, "mvnw") {
        Some(folder) if folder == dir => (format!("{} spring-boot:run", wrapper_call(&folder, "mvnw")), folder),
        Some(folder) => {
            let pom = dir.join("pom.xml");
            let path = pom.strip_prefix(&folder).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_default();
            (format!("{} -f {} spring-boot:run", wrapper_call(&folder, "mvnw"), quote(&path)), folder)
        }
        None => ("mvn spring-boot:run".to_string(), dir.to_path_buf()),
    }
}

/// `word` as it can stand in a command line: as it is when it is plain, else in single quotes
/// (the same in POSIX shells and PowerShell).
fn quote(word: &str) -> String {
    if !word.is_empty() && word.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:@+=,".contains(c)) {
        word.to_string()
    } else if cfg!(windows) {
        // PowerShell takes the typographic single quotes for `'` too: each is doubled like it.
        let mut quoted = String::from("'");
        for c in word.chars() {
            quoted.push(c);
            if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}') {
                quoted.push(c);
            }
        }
        quoted.push('\'');
        quoted
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

/// Files read during detection, which runs without a click on whatever was cloned: no more than
/// this is read, and only from a regular file (a link to `/dev/zero` would never end).
const MAX_READ: u64 = 1 << 20;

/// How much of a source file is looked at for `@SpringBootApplication`.
const SOURCE_HEAD: u64 = 64 << 10;

fn read_small(path: &Path) -> Option<String> {
    read_head(path, MAX_READ)
}

/// At most `limit` bytes of the regular file `path`, as text (a cut character is replaced).
fn read_head(path: &Path, limit: u64) -> Option<String> {
    use std::io::Read;
    if !std::fs::metadata(path).ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path).ok()?.take(limit).read_to_end(&mut bytes).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("agentty-run-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn node_scripts_with_the_lockfiles_package_manager() {
        let root = temp("node");
        write(
            &root.join("package.json"),
            r#"{"scripts":{"lint":"eslint .","build":"vite build","dev":"vite","predev":"x","postinstall":"y","start":"node s"}}"#,
        );
        write(&root.join("pnpm-lock.yaml"), "");
        let found = detect(&root);
        let names: Vec<&str> = found.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["dev", "start", "build", "lint"]);
        assert_eq!(found[0].command, "pnpm run dev");
        assert_eq!(found[0].tool, Tool::Pnpm);
        assert_eq!(found[0].dir, root);
        assert_eq!(found[0].title(), "dev");
    }

    #[test]
    fn packages_below_the_root_offer_what_starts_something() {
        let root = temp("monorepo");
        write(&root.join("yarn.lock"), "");
        write(&root.join("apps/web/package.json"), r#"{"scripts":{"dev":"next dev","test":"jest","start:prod":"node ."}}"#);
        write(&root.join("node_modules/x/package.json"), r#"{"scripts":{"dev":"x"}}"#);
        write(&root.join("tools/bun-app/package.json"), r#"{"packageManager":"bun@1.1.0","scripts":{"serve":"bun s"}}"#);
        let found = detect(&root);
        let titles: Vec<String> = found.iter().map(RunConfig::title).collect();
        assert_eq!(titles, ["apps/web › dev", "apps/web › start:prod", "tools/bun-app › serve"]);
        assert_eq!(found[0].command, "yarn run dev");
        assert_eq!(found[0].dir, root.join("apps/web"));
        assert_eq!(found[2].command, "bun run serve");
    }

    #[test]
    fn spring_boot_with_gradle_and_maven() {
        let root = temp("spring");
        write(&root.join("settings.gradle.kts"), "");
        write(&root.join("gradlew"), "#!/bin/sh\n");
        write(&root.join("build.gradle.kts"), "plugins { id(\"org.springframework.boot\") apply false }");
        write(&root.join("api/build.gradle.kts"), "plugins { id(\"org.springframework.boot\") kotlin(\"jvm\") }");
        write(&root.join("api/src/main/kotlin/App.kt"), "@SpringBootApplication\nclass App");
        write(&root.join("lib/build.gradle.kts"), "plugins { id(\"org.springframework.boot\") }");
        write(&root.join("lib/src/main/kotlin/Lib.kt"), "class Lib");
        let found = detect(&root);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].name, "api");
        assert!(found[0].kotlin);
        assert!(found[0].command.ends_with("gradlew :api:bootRun"), "{}", found[0].command);
        assert_eq!(found[0].dir, root);

        let maven = temp("maven");
        write(&maven.join("pom.xml"), "<parent><artifactId>spring-boot-starter-parent</artifactId></parent>");
        write(&maven.join("src/main/java/demo/App.java"), "@SpringBootApplication public class App {}");
        let found = detect(&maven);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].command, "mvn spring-boot:run");
        assert_eq!(found[0].tool, Tool::Maven);
        assert!(!found[0].kotlin);
    }

    #[test]
    fn spring_boot_from_a_version_catalog() {
        let root = temp("catalog");
        write(&root.join("gradle/libs.versions.toml"), "[plugins]\nspring-boot = { id = \"org.springframework.boot\" }\n");
        write(&root.join("app/build.gradle.kts"), "plugins { alias(libs.plugins.spring.boot) }");
        write(&root.join("app/src/main/kotlin/App.kt"), "@SpringBootApplication\nclass App");
        write(&root.join("package.json"), r#"{"scripts":{"spring":"echo not spring boot"}}"#);
        let found = detect(&root);
        let ids: Vec<&str> = found.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["#spring", "app@spring-boot"]);
    }

    #[test]
    fn custom_commands_come_after_the_detected_ones() {
        let root = temp("custom");
        write(&root.join("package.json"), r#"{"scripts":{"dev":"vite"}}"#);
        let custom = [CustomRun { name: String::new(), command: "make run".into(), dir: "server".into() }];
        let found = configs(&root, &custom);
        assert_eq!(found.len(), 2);
        assert_eq!(found[1].name, "make run");
        assert_eq!(found[1].dir, root.join("server"));
        assert!(found[1].is_custom());
        assert_eq!(found[1].id, "custom#0");
    }

    #[test]
    fn ports_named_by_a_script() {
        let root = temp("ports");
        write(&root.join("package.json"), r#"{"scripts":{"dev":"vite --port 5173","serve":"PORT=3000 node server.js","build":"tsc -b"}}"#);
        let found = detect(&root);
        let ports = |name: &str| found.iter().find(|c| c.name == name).unwrap().named_ports();
        assert_eq!(ports("dev"), [5173]);
        assert_eq!(ports("serve"), [3000]);
        assert!(ports("build").is_empty());
        let config = |script: &str| RunConfig { script: script.into(), ..found[0].clone() };
        assert_eq!(config("node --max-old-space-size=4096 server.js -p 8080").named_ports(), [8080]);
        assert_eq!(config("python3 -m http.server 5198").named_ports(), [5198]);
        assert_eq!(config("serve --listen tcp://localhost:4321").named_ports(), [4321]);
    }

    #[cfg(unix)]
    #[test]
    fn detection_reads_only_small_regular_files() {
        let root = temp("links");
        std::os::unix::fs::symlink("/dev/zero", root.join("package.json")).unwrap();
        assert!(detect(&root).is_empty());
        write(&root.join("big/package.json"), &format!("{{\"scripts\":{{\"dev\":\"{}\"}}}}", "x".repeat(2 << 20)));
        assert!(read_small(&root.join("big/package.json")).is_some_and(|t| t.len() as u64 <= MAX_READ));
    }

    #[test]
    fn odd_script_names_are_quoted() {
        assert_eq!(quote("dev"), "dev");
        assert_eq!(quote("start:dev"), "start:dev");
        assert!(quote("dev; rm -rf /").starts_with('\''));
    }
}
