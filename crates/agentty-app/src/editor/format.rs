//! "Format": runs the installed formatter for the file's language over the editor's text.
//!
//! The text goes in on stdin and comes back on stdout, so what is formatted is what is on screen
//! (saved or not). Programs are found on `PATH` (absolute folders only) and started with an
//! argument list, never through a shell. Nothing of the project is ever run: a formatter found
//! inside the project (`node_modules/.bin`) is not used, a project setting that is code — a
//! `prettier.config.js`, Prettier plugins — makes Agentty refuse rather than execute it, and the
//! formatter starts outside the project with only absolute `PATH` folders, so nothing picks a
//! program by the project folder (rustup's toolchain file, version-manager shims, `./node`).
//! Setting files that are plain data are honored: the file's path is passed for them.

use super::language::Formatter;
use std::ffi::OsString;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(30);
/// Formatter output larger than this is not taken (something went wrong).
const MAX_OUTPUT: usize = 64 * 1024 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum FormatError {
    /// The formatter is not installed (how to install it).
    Missing {
        program: &'static str,
        hint: &'static str,
    },
    /// The project's setting for it is code, which Agentty never runs (the file).
    ProjectCode(PathBuf),
    /// It ran and complained (its first lines of output).
    Failed(String),
    TimedOut,
}

/// The program, arguments and folder a formatter is started with.
#[derive(Debug, PartialEq, Eq)]
pub struct Invocation {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub cwd: PathBuf,
    /// The Prettier settings written for this run (the project's plain options only).
    temp_config: Option<PathBuf>,
}

impl Drop for Invocation {
    fn drop(&mut self) {
        if let Some(temp) = &self.temp_config {
            let _ = std::fs::remove_file(temp);
        }
    }
}

/// Formats `text` (the contents of `file`, which belongs to `project`) with `formatter`.
pub fn format(formatter: Formatter, file: &Path, project: &Path, text: &str) -> Result<String, FormatError> {
    let program = find_program(formatter.program(), project)
        .ok_or(FormatError::Missing { program: formatter.program(), hint: formatter.install_hint() })?;
    let invocation = plan(formatter, program, file, project)?;
    run(&invocation, formatter, text)
}

/// `name` on `PATH` (plus the usual install folders), unless it lives inside the project.
pub fn find_program(name: &str, project: &Path) -> Option<PathBuf> {
    let path = agentty_bridge::process::merge_paths(&agentty_bridge::process::current_path(), &agentty_bridge::process::tool_dirs());
    let found = agentty_bridge::process::which_in(name, &path)?;
    let real = std::fs::canonicalize(&found).unwrap_or_else(|_| found.clone());
    let root = std::fs::canonicalize(project).unwrap_or_else(|_| project.to_path_buf());
    (!real.starts_with(&root) && !found.starts_with(project)).then_some(found)
}

/// How `formatter` is started for `file`: arguments, folder and settings file.
pub fn plan(formatter: Formatter, program: PathBuf, file: &Path, project: &Path) -> Result<Invocation, FormatError> {
    let path = file.as_os_str().to_os_string();
    let mut temp_config = None;
    let args: Vec<OsString> = match formatter {
        Formatter::Prettier => {
            let mut args: Vec<OsString> = Vec::new();
            match prettier_config(file, project)? {
                None => args.push("--no-config".into()),
                // Always Agentty's own copy of the plain options, never the project's file.
                Some(options) => {
                    let temp = std::env::temp_dir().join(format!("agentty-prettierrc-{}.json", uuid::Uuid::new_v4().simple()));
                    // A new file only: never through something already planted under that name.
                    std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&temp)
                        .and_then(|mut file| file.write_all(serde_json::Value::Object(options).to_string().as_bytes()))
                        .map_err(|e| FormatError::Failed(e.to_string()))?;
                    args.push("--config".into());
                    args.push(temp.clone().into_os_string());
                    temp_config = Some(temp);
                }
            }
            if let Some(ignore) = find_up(file, project, &[".prettierignore"]) {
                args.push("--ignore-path".into());
                args.push(ignore.into_os_string());
            }
            args.push("--stdin-filepath".into());
            args.push(path);
            args
        }
        Formatter::GoogleJavaFormat => vec!["-".into()],
        // `--stdin-path`: `.editorconfig` is found from the file, not from the folder ktlint runs in.
        Formatter::Ktlint => vec!["--stdin".into(), "--format".into(), "--stdin-path".into(), path],
        Formatter::Rustfmt => {
            let mut args: Vec<OsString> = vec!["--edition".into(), rust_edition(file, project).into()];
            if let Some(config) = find_up(file, project, &["rustfmt.toml", ".rustfmt.toml"]) {
                args.push("--config-path".into());
                args.push(config.into_os_string());
            }
            args
        }
        Formatter::Gofmt => Vec::new(),
        Formatter::Black => vec!["--quiet".into(), "--stdin-filename".into(), path, "-".into()],
    };
    // Never inside the project (see the module comment).
    Ok(Invocation { program, args, cwd: work_dir(), temp_config })
}

/// The folder formatters start in: an empty one of Agentty's own (outside every project, and not
/// the shared temp folder, where others could leave a `.tool-versions` for a shim to follow).
fn work_dir() -> PathBuf {
    agentty_bridge::fsutil::data_dir().join("formatter")
}

fn run(invocation: &Invocation, formatter: Formatter, text: &str) -> Result<String, FormatError> {
    if !invocation.cwd.is_dir() {
        std::fs::create_dir_all(&invocation.cwd).map_err(|e| FormatError::Failed(e.to_string()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&invocation.cwd, std::fs::Permissions::from_mode(0o700));
        }
    }
    let mut child = agentty_bridge::process::command(&invocation.program)
        .args(&invocation.args)
        .current_dir(&invocation.cwd)
        .env("PATH", safe_path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| FormatError::Failed(e.to_string()))?;
    let mut stdin = child.stdin.take().expect("piped");
    let input = text.to_string();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(input.as_bytes());
    });
    let mut stdout = child.stdout.take().expect("piped");
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = (&mut stdout).take(MAX_OUTPUT as u64 + 1).read_to_end(&mut out);
        out
    });
    let mut stderr = child.stderr.take().expect("piped");
    let errors = std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = (&mut stderr).take(64 * 1024).read_to_end(&mut out);
        out
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() > TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(FormatError::TimedOut);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(err) => return Err(FormatError::Failed(err.to_string())),
        }
    };
    let _ = writer.join();
    let out = reader.join().unwrap_or_default();
    let err = String::from_utf8_lossy(&errors.join().unwrap_or_default()).to_string();
    let out = String::from_utf8(out).map_err(|_| FormatError::Failed("output is not UTF-8".into()))?;
    // ktlint prints the formatted code and still exits 1 when problems remain it cannot fix.
    let usable = status.success() || (formatter == Formatter::Ktlint && !out.trim().is_empty());
    if !usable || out.len() > MAX_OUTPUT || (out.trim().is_empty() && !text.trim().is_empty()) {
        let message = if err.trim().is_empty() { out.clone() } else { err };
        return Err(FormatError::Failed(first_lines(&message, 6)));
    }
    Ok(out.replace("\r\n", "\n"))
}

/// `PATH` for formatters: the usual folders, absolute ones only (a relative entry would resolve
/// against a folder that could be the project's).
fn safe_path() -> OsString {
    let path = agentty_bridge::process::merge_paths(&agentty_bridge::process::current_path(), &agentty_bridge::process::tool_dirs());
    let absolute: Vec<PathBuf> = std::env::split_paths(&path).filter(|dir| dir.is_absolute()).collect();
    std::env::join_paths(absolute).unwrap_or_default()
}

fn first_lines(text: &str, count: usize) -> String {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).take(count).collect();
    lines.join("\n")
}

/// Prettier options that are plain formatting choices (a string, number or boolean each).
/// Everything else in a settings file — `plugins`, `overrides`, `parser`, a shared config
/// package — is left out, because it can make Prettier load code.
const PRETTIER_OPTIONS: &[&str] = &[
    "printWidth",
    "tabWidth",
    "useTabs",
    "semi",
    "singleQuote",
    "jsxSingleQuote",
    "quoteProps",
    "trailingComma",
    "bracketSpacing",
    "bracketSameLine",
    "jsxBracketSameLine",
    "objectWrap",
    "arrowParens",
    "proseWrap",
    "htmlWhitespaceSensitivity",
    "endOfLine",
    "embeddedLanguageFormatting",
    "singleAttributePerLine",
    "vueIndentScriptAndStyle",
    "experimentalTernaries",
    "experimentalOperatorPosition",
];

/// Prettier's settings for `file`, searched upward like Prettier does (up to the project root),
/// reduced to [`PRETTIER_OPTIONS`]. Agentty always hands Prettier its own copy of them, never the
/// project's file: settings that are code (`prettier.config.js`), a shared config package, or a
/// file that can't be read as plain data make it refuse.
fn prettier_config(file: &Path, project: &Path) -> Result<Option<serde_json::Map<String, serde_json::Value>>, FormatError> {
    const DATA: &[&str] =
        &[".prettierrc", ".prettierrc.json", ".prettierrc.yaml", ".prettierrc.yml", ".prettierrc.json5", ".prettierrc.toml"];
    const CODE: &[&str] = &[
        ".prettierrc.js",
        ".prettierrc.cjs",
        ".prettierrc.mjs",
        ".prettierrc.ts",
        ".prettierrc.cts",
        ".prettierrc.mts",
        "prettier.config.js",
        "prettier.config.cjs",
        "prettier.config.mjs",
        "prettier.config.ts",
        "prettier.config.cts",
        "prettier.config.mts",
    ];
    for dir in ancestors_within(file, project) {
        let package = dir.join("package.json");
        if let Some(value) = std::fs::read_to_string(&package).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) {
            match value.get("prettier") {
                Some(serde_json::Value::Object(map)) => return Ok(Some(allowed_options(map))),
                // A shared config package (code from node_modules), or something else.
                Some(_) => return Err(FormatError::ProjectCode(package)),
                None => {}
            }
        }
        if let Some(code) = CODE.iter().map(|name| dir.join(name)).find(|p| p.is_file()) {
            return Err(FormatError::ProjectCode(code));
        }
        if let Some(data) = DATA.iter().map(|name| dir.join(name)).find(|p| p.is_file()) {
            let text = std::fs::read_to_string(&data).unwrap_or_default();
            return match parse_settings(&data, &text) {
                Some(map) => Ok(Some(allowed_options(&map))),
                None => Err(FormatError::ProjectCode(data)),
            };
        }
    }
    Ok(None)
}

fn allowed_options(map: &serde_json::Map<String, serde_json::Value>) -> serde_json::Map<String, serde_json::Value> {
    map.iter()
        .filter(|(key, value)| {
            PRETTIER_OPTIONS.contains(&key.as_str())
                && matches!(value, serde_json::Value::String(_) | serde_json::Value::Number(_) | serde_json::Value::Bool(_))
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// A Prettier settings file as a JSON object: JSON, TOML, or YAML of plain `key: value` lines.
/// `None` when it is anything else (a string naming a config package, nested YAML, JSON5).
fn parse_settings(path: &Path, text: &str) -> Option<serde_json::Map<String, serde_json::Value>> {
    let name = path.file_name()?.to_string_lossy();
    if name.ends_with(".toml") {
        let table: toml::Table = text.parse().ok()?;
        return serde_json::to_value(table).ok()?.as_object().cloned();
    }
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(text) {
        return value.as_object().cloned();
    }
    if name.ends_with(".json") || name.ends_with(".json5") {
        return None;
    }
    // YAML (`.prettierrc`, `.yaml`, `.yml`): only flat `key: scalar` lines.
    let mut map = serde_json::Map::new();
    for line in text.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() || line.trim_start().starts_with('#') || line == "---" {
            continue;
        }
        let (key, value) = line.split_once(':')?;
        let key = key.trim();
        if line.starts_with([' ', '\t']) || key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric()) {
            return None;
        }
        let value = value.split(" #").next().unwrap_or_default().trim();
        let value = match value {
            "true" => serde_json::Value::Bool(true),
            "false" => serde_json::Value::Bool(false),
            "" => return None,
            v => match v.parse::<i64>() {
                Ok(n) => serde_json::Value::from(n),
                Err(_) => serde_json::Value::String(v.trim_matches(['"', '\'']).to_string()),
            },
        };
        map.insert(key.to_string(), value);
    }
    Some(map)
}

/// `file`'s folder and its parents, up to and including `project` (just the folder when the file
/// is not in the project).
fn ancestors_within<'a>(file: &'a Path, project: &'a Path) -> impl Iterator<Item = &'a Path> {
    let inside = file.starts_with(project);
    file.ancestors().skip(1).take_while(move |dir| !inside || dir.starts_with(project)).take(if inside { usize::MAX } else { 1 })
}

fn find_up(file: &Path, project: &Path, names: &[&str]) -> Option<PathBuf> {
    ancestors_within(file, project).find_map(|dir| names.iter().map(|n| dir.join(n)).find(|p| p.is_file()))
}

/// The Rust edition of the crate `file` belongs to (its `Cargo.toml`), 2021 when unknown.
fn rust_edition(file: &Path, project: &Path) -> String {
    let edition = find_up(file, project, &["Cargo.toml"]).and_then(|manifest| {
        let text = std::fs::read_to_string(manifest).ok()?;
        text.lines().find_map(|line| {
            let value = line.trim().strip_prefix("edition")?.trim_start().strip_prefix('=')?.trim().trim_matches('"');
            (value.len() == 4 && value.chars().all(|c| c.is_ascii_digit())).then(|| value.to_string())
        })
    });
    edition.unwrap_or_else(|| "2021".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_project(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("agentty-format-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        dir
    }

    fn args(invocation: &Invocation) -> Vec<String> {
        invocation.args.iter().map(|a| a.to_string_lossy().to_string()).collect()
    }

    #[test]
    fn each_formatter_reads_stdin_and_gets_the_path_only_as_an_argument() {
        let project = temp_project("args");
        let file = project.join("src/a b;$(rm -rf x).ts");
        let program = PathBuf::from("/usr/local/bin/tool");
        let prettier = plan(Formatter::Prettier, program.clone(), &file, &project).unwrap();
        assert_eq!(args(&prettier), ["--no-config", "--stdin-filepath", &file.to_string_lossy()]);
        assert_eq!(args(&plan(Formatter::GoogleJavaFormat, program.clone(), &file, &project).unwrap()), ["-"]);
        assert_eq!(
            args(&plan(Formatter::Ktlint, program.clone(), &file, &project).unwrap()),
            ["--stdin", "--format", "--stdin-path", &file.to_string_lossy()]
        );
        assert!(args(&plan(Formatter::Gofmt, program.clone(), &file, &project).unwrap()).is_empty());
        assert_eq!(
            args(&plan(Formatter::Black, program.clone(), &file, &project).unwrap()),
            ["--quiet", "--stdin-filename", &file.to_string_lossy(), "-"]
        );
        assert_eq!(args(&plan(Formatter::Rustfmt, program.clone(), &file, &project).unwrap()), ["--edition", "2021"]);
        // None of them starts inside the project.
        for formatter in
            [Formatter::Prettier, Formatter::GoogleJavaFormat, Formatter::Ktlint, Formatter::Rustfmt, Formatter::Gofmt, Formatter::Black]
        {
            assert!(!plan(formatter, program.clone(), &file, &project).unwrap().cwd.starts_with(&project), "{formatter:?}");
        }
        std::fs::remove_dir_all(project).ok();
    }

    #[test]
    fn prettier_gets_only_plain_options_and_never_the_projects_file() {
        let project = temp_project("prettier");
        let file = project.join("src/app.ts");
        let program = PathBuf::from("/usr/local/bin/prettier");
        let config_of = |invocation: &Invocation| -> serde_json::Value {
            assert_eq!(args(invocation)[0], "--config");
            serde_json::from_str(&std::fs::read_to_string(&args(invocation)[1]).unwrap()).unwrap()
        };
        std::fs::write(project.join(".prettierrc"), r#"{ "singleQuote": true, "plugins": ["./evil.mjs"], "overrides": [] }"#).unwrap();
        std::fs::write(project.join(".prettierignore"), "dist\n").unwrap();
        let data = plan(Formatter::Prettier, program.clone(), &file, &project).unwrap();
        assert_eq!(config_of(&data), serde_json::json!({ "singleQuote": true }));
        assert_eq!(args(&data)[2..4], ["--ignore-path".to_string(), project.join(".prettierignore").to_string_lossy().to_string()]);
        // Escaped keys are decoded before they are looked at.
        std::fs::write(project.join(".prettierrc"), r#"{"\u0070lugins": ["./evil.mjs"], "semi": false}"#).unwrap();
        assert_eq!(config_of(&plan(Formatter::Prettier, program.clone(), &file, &project).unwrap()), serde_json::json!({ "semi": false }));
        // Plain YAML and TOML.
        std::fs::write(project.join(".prettierrc"), "# style\ntabWidth: 2\nsingleQuote: true\nparser: ./evil.cjs\n").unwrap();
        assert_eq!(
            config_of(&plan(Formatter::Prettier, program.clone(), &file, &project).unwrap()),
            serde_json::json!({ "tabWidth": 2, "singleQuote": true })
        );
        std::fs::remove_file(project.join(".prettierrc")).unwrap();
        std::fs::write(project.join(".prettierrc.toml"), "semi = false\nplugins = [\"x\"]\n").unwrap();
        assert_eq!(config_of(&plan(Formatter::Prettier, program.clone(), &file, &project).unwrap()), serde_json::json!({ "semi": false }));
        std::fs::remove_file(project.join(".prettierrc.toml")).unwrap();

        // A YAML string names a shared config module: refused, as is anything but flat data.
        for text in ["# settings\n./evil.cjs\n", "\"./a=b.cjs\"", "overrides:\n  - files: x\n"] {
            std::fs::write(project.join(".prettierrc"), text).unwrap();
            assert_eq!(
                plan(Formatter::Prettier, program.clone(), &file, &project),
                Err(FormatError::ProjectCode(project.join(".prettierrc"))),
                "{text}"
            );
        }
        std::fs::remove_file(project.join(".prettierrc")).unwrap();

        // Settings in package.json go through the same copy, removed afterwards.
        std::fs::write(project.join("package.json"), r#"{ "name": "x", "prettier": { "semi": false, "plugins": ["y"] } }"#).unwrap();
        let inline = plan(Formatter::Prettier, program.clone(), &file, &project).unwrap();
        let temp = PathBuf::from(&args(&inline)[1]);
        assert_eq!(config_of(&inline), serde_json::json!({ "semi": false }));
        drop(inline);
        assert!(!temp.exists());
        std::fs::write(project.join("package.json"), r#"{ "prettier": "@company/prettier-config" }"#).unwrap();
        assert_eq!(
            plan(Formatter::Prettier, program.clone(), &file, &project),
            Err(FormatError::ProjectCode(project.join("package.json")))
        );
        std::fs::write(project.join("package.json"), "{}").unwrap();
        std::fs::write(project.join("src/prettier.config.js"), "module.exports = {}").unwrap();
        assert_eq!(
            plan(Formatter::Prettier, program, &file, &project),
            Err(FormatError::ProjectCode(project.join("src/prettier.config.js")))
        );
        std::fs::remove_dir_all(project).ok();
    }

    #[test]
    fn rustfmt_follows_the_crate_edition_and_settings() {
        let project = temp_project("rust");
        std::fs::write(project.join("Cargo.toml"), "[package]\nname = \"x\"\nedition = \"2024\"\n").unwrap();
        std::fs::write(project.join("rustfmt.toml"), "max_width = 140\n").unwrap();
        let invocation = plan(Formatter::Rustfmt, PathBuf::from("/bin/rustfmt"), &project.join("src/main.rs"), &project).unwrap();
        assert_eq!(args(&invocation), ["--edition", "2024", "--config-path", &project.join("rustfmt.toml").to_string_lossy()]);
        std::fs::remove_dir_all(project).ok();
    }

    #[test]
    fn a_formatter_inside_the_project_is_not_used() {
        let project = temp_project("path");
        // Whatever is found, it is never inside the project folder.
        if let Some(found) = find_program("git", &project) {
            assert!(!found.starts_with(&project));
        }
        assert_eq!(find_program("agentty-no-such-formatter", &project), None);
        std::fs::remove_dir_all(project).ok();
    }

    #[cfg(unix)]
    #[test]
    fn output_replaces_the_text_and_failures_are_reported() {
        let project = temp_project("run");
        // `cat` stands in for a formatter: stdin back on stdout.
        let cat = Invocation { program: "/bin/cat".into(), args: Vec::new(), cwd: project.clone(), temp_config: None };
        assert_eq!(run(&cat, Formatter::Gofmt, "a\r\nb\n"), Ok("a\nb\n".into()));
        let failing = Invocation {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "echo 'syntax error at 3:1' >&2; exit 2".into()],
            cwd: project.clone(),
            temp_config: None,
        };
        assert_eq!(run(&failing, Formatter::Gofmt, "x"), Err(FormatError::Failed("syntax error at 3:1".into())));
        std::fs::remove_dir_all(project).ok();
    }
}
