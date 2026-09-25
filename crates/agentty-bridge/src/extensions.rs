//! Discovers what each agent can use: skills, subagents, slash commands, plugins and MCP servers,
//! from user-level and project-level configuration. Read-only.

use crate::fsutil;
use crate::model::Agent;
use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ExtensionKind {
    Skill,
    Agent,
    Command,
    Plugin,
    Mcp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase", tag = "type", content = "name")]
pub enum Scope {
    User,
    Project,
    System,
    Plugin(String),
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Extension {
    pub agent: Agent,
    pub kind: ExtensionKind,
    pub name: String,
    pub description: String,
    pub scope: Scope,
    pub path: Option<PathBuf>,
    /// Extra facts: model for subagents, command/URL for MCP servers, version for plugins.
    pub detail: Option<String>,
    pub enabled: bool,
    /// Text to type into the agent to use it (`/skill`, `$skill`, …), when it can be invoked.
    pub invocation: Option<String>,
}

/// Minimal YAML front matter reader: top-level `key: value` pairs, with `>`/`|` blocks folded.
pub fn front_matter(text: &str) -> Vec<(String, String)> {
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return Vec::new();
    }
    let mut pairs: Vec<(String, String)> = Vec::new();
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        let indented = line.starts_with(' ') || line.starts_with('\t');
        match (indented, line.split_once(':')) {
            (false, Some((key, value))) if !key.contains(' ') => {
                let value = value.trim();
                let value = if matches!(value, ">" | "|" | ">-" | "|-") { "" } else { value };
                pairs.push((key.trim().to_string(), unquote(value)));
            }
            (true, _) => {
                if let Some((_, value)) = pairs.last_mut() {
                    if !value.is_empty() {
                        value.push(' ');
                    }
                    value.push_str(line.trim());
                }
            }
            _ => {}
        }
    }
    pairs
}

fn unquote(value: &str) -> String {
    let v = value.trim();
    if v.len() >= 2 && ((v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\''))) {
        v[1..v.len() - 1].to_string()
    } else {
        v.to_string()
    }
}

fn field(pairs: &[(String, String)], key: &str) -> Option<String> {
    pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()).filter(|v| !v.is_empty())
}

fn read_markdown(path: &Path) -> Option<Vec<(String, String)>> {
    let text = std::fs::read_to_string(path).ok()?;
    Some(front_matter(&text))
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> =
        std::fs::read_dir(dir).map(|e| e.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect()).unwrap_or_default();
    dirs.sort();
    dirs
}

fn markdown_files(dir: &Path, recursive: bool) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return out };
    for path in entries.flatten().map(|e| e.path()) {
        if path.is_dir() && recursive {
            out.extend(markdown_files(&path, true));
        } else if path.extension().is_some_and(|e| e == "md") {
            out.push(path);
        }
    }
    out.sort();
    out
}

/// `<dir>/<skill>/SKILL.md` entries.
fn skills_in(agent: Agent, dir: &Path, scope: Scope, invoke: impl Fn(&str) -> String) -> Vec<Extension> {
    subdirs(dir)
        .into_iter()
        .filter_map(|skill_dir| {
            let file = skill_dir.join("SKILL.md");
            let meta = read_markdown(&file)?;
            let name = field(&meta, "name").unwrap_or_else(|| skill_dir.file_name().unwrap_or_default().to_string_lossy().to_string());
            Some(Extension {
                agent,
                kind: ExtensionKind::Skill,
                invocation: Some(invoke(&name)),
                description: field(&meta, "description").unwrap_or_default(),
                name,
                scope: scope.clone(),
                path: Some(file),
                detail: None,
                enabled: true,
            })
        })
        .collect()
}

pub fn discover(agent: Agent, project: Option<&Path>) -> Vec<Extension> {
    let mut items = match agent {
        Agent::Claude => claude(project),
        Agent::Codex => codex(project),
        _ => Vec::new(),
    };
    items.sort_by(|a, b| {
        (a.kind as u8, scope_rank(&a.scope), a.name.to_lowercase()).cmp(&(b.kind as u8, scope_rank(&b.scope), b.name.to_lowercase()))
    });
    items
}

/// Only what a project itself defines (no user, plugin or system items): cheap enough to run on
/// every `cd`.
pub fn project_only(agent: Agent, project: &Path) -> Vec<Extension> {
    let mut items = match agent {
        Agent::Claude => {
            let root = project.join(".claude");
            let mut items = skills_in(Agent::Claude, &root.join("skills"), Scope::Project, |n| format!("/{n}"));
            items.extend(claude_agents(&root.join("agents"), Scope::Project, None));
            items.extend(claude_commands(&root.join("commands"), Scope::Project, None));
            items
        }
        Agent::Codex => {
            let invoke = |n: &str| format!("${n}");
            let mut items = skills_in(Agent::Codex, &project.join(".agents").join("skills"), Scope::Project, invoke);
            items.extend(skills_in(Agent::Codex, &project.join(".codex").join("skills"), Scope::Project, invoke));
            items
        }
        _ => Vec::new(),
    };
    items.sort_by_key(|a| (a.kind as u8, a.name.to_lowercase()));
    items
}

fn scope_rank(scope: &Scope) -> u8 {
    match scope {
        Scope::Project => 0,
        Scope::User => 1,
        Scope::Plugin(_) => 2,
        Scope::System => 3,
    }
}

// ---------------------------------------------------------------------------------------------
// Claude Code
// ---------------------------------------------------------------------------------------------

fn claude(project: Option<&Path>) -> Vec<Extension> {
    let home = fsutil::home().join(".claude");
    let mut items = Vec::new();
    let mut roots: Vec<(PathBuf, Scope)> = vec![(home.clone(), Scope::User)];
    if let Some(project) = project {
        roots.insert(0, (project.join(".claude"), Scope::Project));
    }
    for (root, scope) in &roots {
        items.extend(skills_in(Agent::Claude, &root.join("skills"), scope.clone(), |n| format!("/{n}")));
        items.extend(claude_agents(&root.join("agents"), scope.clone(), None));
        items.extend(claude_commands(&root.join("commands"), scope.clone(), None));
    }
    items.extend(claude_plugins(&home));
    items.extend(claude_mcp(project));
    items
}

fn claude_agents(dir: &Path, scope: Scope, plugin: Option<&str>) -> Vec<Extension> {
    markdown_files(dir, false)
        .into_iter()
        .filter_map(|file| {
            let meta = read_markdown(&file)?;
            let name = field(&meta, "name").unwrap_or_else(|| file.file_stem().unwrap_or_default().to_string_lossy().to_string());
            let qualified = plugin.map(|p| format!("{p}:{name}")).unwrap_or_else(|| name.clone());
            Some(Extension {
                agent: Agent::Claude,
                kind: ExtensionKind::Agent,
                invocation: Some(format!("Use the {qualified} agent to ")),
                description: field(&meta, "description").unwrap_or_default(),
                detail: field(&meta, "model"),
                name,
                scope: scope.clone(),
                path: Some(file),
                enabled: true,
            })
        })
        .collect()
}

fn claude_commands(dir: &Path, scope: Scope, plugin: Option<&str>) -> Vec<Extension> {
    markdown_files(dir, true)
        .into_iter()
        .map(|file| {
            let meta = read_markdown(&file).unwrap_or_default();
            // Nested directories namespace the command: commands/git/commit.md → /git:commit.
            let relative = file.strip_prefix(dir).unwrap_or(&file).with_extension("");
            let local = relative.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect::<Vec<_>>().join(":");
            let name = plugin.map(|p| format!("{p}:{local}")).unwrap_or(local);
            Extension {
                agent: Agent::Claude,
                kind: ExtensionKind::Command,
                invocation: Some(format!("/{name}")),
                description: field(&meta, "description").unwrap_or_default(),
                detail: field(&meta, "argument-hint"),
                name,
                scope: scope.clone(),
                path: Some(file),
                enabled: true,
            }
        })
        .collect()
}

fn claude_plugins(home: &Path) -> Vec<Extension> {
    let installed: Value = std::fs::read(home.join("plugins").join("installed_plugins.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let settings: Value = std::fs::read(home.join("settings.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let mut items = Vec::new();
    let Some(plugins) = installed["plugins"].as_object() else { return items };
    for (id, installs) in plugins {
        let Some(install) = installs.as_array().and_then(|a| a.last()) else { continue };
        let short = id.split('@').next().unwrap_or(id).to_string();
        let enabled = settings["enabledPlugins"][id].as_bool().unwrap_or(false);
        let path = install["installPath"].as_str().map(PathBuf::from);
        let manifest: Value = path
            .as_ref()
            .and_then(|p| std::fs::read(p.join(".claude-plugin").join("plugin.json")).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        items.push(Extension {
            agent: Agent::Claude,
            kind: ExtensionKind::Plugin,
            name: id.clone(),
            description: manifest["description"].as_str().unwrap_or("").to_string(),
            scope: Scope::User,
            detail: install["version"].as_str().map(|v| format!("v{v}")),
            path: path.clone(),
            enabled,
            invocation: None,
        });
        if let (Some(path), true) = (path, enabled) {
            let scope = Scope::Plugin(short.clone());
            items.extend(skills_in(Agent::Claude, &path.join("skills"), scope.clone(), |n| format!("/{short}:{n}")));
            items.extend(claude_agents(&path.join("agents"), scope.clone(), Some(&short)));
            items.extend(claude_commands(&path.join("commands"), scope, Some(&short)));
        }
    }
    items
}

fn mcp_detail(server: &Value) -> String {
    if let Some(url) = server["url"].as_str() {
        return format!("{} {}", server["type"].as_str().unwrap_or("http"), redact_url(url));
    }
    let command = server["command"].as_str().unwrap_or("");
    let args: Vec<&str> = server["args"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    format!("stdio {command} {}", redact_args(&args)).trim().to_string()
}

const MASK: &str = "••••";

/// Substrings of a name that say its value is a credential…
const SECRET_WORDS: &[&str] =
    &["key", "token", "secret", "password", "passwd", "auth", "credential", "bearer", "cookie", "session", "signature", "jwt"];
/// …and short ones that only count as a whole part of the name (`FIGMA_PAT`, not `--path`).
const SECRET_PARTS: &[&str] = &["pat", "pwd", "sig", "pass"];
/// How well-known credentials start, whatever they are called.
const TOKEN_PREFIXES: &[&str] = &[
    "sk-",
    "sk_live_",
    "rk_live_",
    "pk_live_",
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
    "github_pat_",
    "glpat-",
    "figd_",
    "xoxb-",
    "xoxp-",
    "xoxa-",
    "AKIA",
    "ASIA",
    "AIza",
    "eyJ",
    "sbp_",
    "sb_secret_",
    "npm_",
    "hf_",
    "ntn_",
    "lin_api_",
    "gsk_",
    "xai-",
    "pplx-",
    "r8_",
    "dop_v1_",
    "shpat_",
    "dckr_pat_",
    "ATATT3",
    "GOCSPX-",
    "sntrys_",
];

fn looks_secret_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    SECRET_WORDS.iter().any(|w| lower.contains(w))
        || lower.split(|c: char| !c.is_ascii_alphanumeric()).any(|part| SECRET_PARTS.contains(&part))
}

/// Credentials by their looks: a known prefix, a UUID, or a long run of letters and digits.
fn looks_like_token(value: &str) -> bool {
    let value = value.trim_matches(|c| c == '"' || c == '\'');
    if value.len() >= 12 && TOKEN_PREFIXES.iter().any(|prefix| value.starts_with(prefix)) {
        return true;
    }
    let plain = value.chars().all(|c| c.is_ascii_alphanumeric() || "-_.~+=".contains(c));
    let mixed = value.chars().any(|c| c.is_ascii_digit()) && value.chars().any(|c| c.is_ascii_alphabetic());
    let uuid = value.len() == 36
        && value.split('-').map(str::len).eq([8, 4, 4, 4, 12])
        && value.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
    uuid || (plain && mixed && value.len() >= 24)
        // Shorter, but upper case, lower case and digits together: nobody names things that way.
        || (plain && mixed && value.len() >= 20 && value.chars().any(|c| c.is_ascii_uppercase()) && value.chars().any(|c| c.is_ascii_lowercase()))
}

/// Hides credentials in command arguments shown in the UI. First the shapes that name a secret
/// (`--token x`), then — inside every argument — anything that is one whatever it is called:
/// `FIGMA_PAT=figd_…`, `Cookie: session=…`, `Bearer …`, a URL with a token in its user, path or query.
///
/// Best effort, not a boundary: a short or plain-looking secret under an unremarkable name gets
/// through. Secrets are kept out of arguments in the first place (the Keychain, stdin, the
/// environment); this only catches what a user typed there anyway.
pub fn redact_args(args: &[&str]) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut hide_next = false;
    for arg in args {
        if hide_next {
            out.push(MASK.into());
            hide_next = false;
        } else if arg.starts_with('-') && !arg.contains('=') && looks_secret_name(arg) {
            out.push(arg.to_string());
            hide_next = true;
        } else {
            out.push(mask_inline(arg));
        }
    }
    out.join(" ")
}

/// One argument: `name=value`, `Header: value`, or words of which any may be a credential or a URL.
fn mask_inline(arg: &str) -> String {
    if let Some((name, value)) = arg.split_once('=').filter(|(name, _)| is_name(name)) {
        return if looks_secret_name(name) { format!("{name}={MASK}") } else { format!("{name}={}", mask_inline(value)) };
    }
    if let Some((name, value)) = arg.split_once(": ").filter(|(name, _)| is_name(name)) {
        // HTTP headers: `Authorization: Bearer …`, `X-Api-Key: …`, `Cookie: a=b; c=d`.
        return if looks_secret_name(name) { format!("{name}: {MASK}") } else { format!("{name}: {}", mask_words(value)) };
    }
    mask_words(arg)
}

fn is_name(name: &str) -> bool {
    !name.is_empty() && !name.contains(['/', ' ', ':', '"', '\'', '{'])
}

/// Words separated by spaces, `;`, `,`, quotes and brackets; the separators stay as they are.
///
/// Also used for text that may carry a URL with a token in it (a plugin's network error, say).
/// A word is masked for what it is, after `Bearer` / `Basic` / `Token`, or as the value of a name
/// that says so with the quotes of JSON in between (`"apiKey": "…"`).
pub fn mask_words(text: &str) -> String {
    #[derive(PartialEq)]
    enum Next {
        Anything,
        /// The word before named a secret; a `:` or `=` on its own arms the mask.
        AfterSecretName,
        Secret,
    }
    let mut out = String::new();
    let mut word = String::new();
    let mut next = Next::Anything;
    let flush = |word: &mut String, out: &mut String, next: &mut Next| {
        if word.is_empty() {
            return;
        }
        if matches!(word.as_str(), ":" | "=") {
            out.push_str(word);
            if *next == Next::AfterSecretName {
                *next = Next::Secret;
            }
        } else if *next == Next::Secret {
            out.push_str(MASK);
            *next = Next::Anything;
        } else {
            out.push_str(&mask_word(word));
            *next = if matches!(word.to_lowercase().as_str(), "bearer" | "basic" | "token") {
                Next::Secret
            } else if is_name(word) && looks_secret_name(word) {
                Next::AfterSecretName
            } else {
                Next::Anything
            };
        }
        word.clear();
    };
    for c in text.chars() {
        if c.is_whitespace() || matches!(c, ';' | ',' | '"' | '\'' | '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>') {
            flush(&mut word, &mut out, &mut next);
            out.push(c);
        } else {
            word.push(c);
        }
    }
    flush(&mut word, &mut out, &mut next);
    out
}

fn mask_word(word: &str) -> String {
    if word.contains("://") {
        return redact_url(word);
    }
    if let Some((name, value)) = word.split_once('=').or_else(|| word.split_once(':')).filter(|(name, _)| is_name(name)) {
        let separator = &word[name.len()..name.len() + 1];
        return if looks_secret_name(name) || looks_like_token(value) { format!("{name}{separator}{MASK}") } else { word.to_string() };
    }
    if looks_like_token(word) {
        MASK.into()
    } else {
        word.to_string()
    }
}

/// A URL with everything that can carry a credential masked: user and password, token-like path
/// segments, query values (by name or by looks) and the fragment.
/// A URL fit to be written to disk: as it is, unless it carries something that looks like a
/// credential (a token in the path or query, a password), when only its origin is kept.
pub fn url_to_keep(url: &str) -> String {
    if redact_url(url) == url {
        return url.to_string();
    }
    match url::Url::parse(url) {
        Ok(parsed) if parsed.host_str().is_some() => parsed.origin().ascii_serialization() + "/",
        _ => String::new(),
    }
}

pub fn redact_url(url: &str) -> String {
    let Ok(parsed) = url::Url::parse(url) else { return if looks_like_token(url) { MASK.into() } else { url.to_string() } };
    let Some(host) = parsed.host_str() else { return url.to_string() };
    let mut out = format!("{}://", parsed.scheme());
    if !parsed.username().is_empty() || parsed.password().is_some() {
        out.push_str(MASK);
        out.push('@');
    }
    out.push_str(host);
    if let Some(port) = parsed.port() {
        out.push_str(&format!(":{port}"));
    }
    let path: Vec<String> =
        parsed.path().split('/').map(|part| if looks_like_token(part) { MASK.to_string() } else { part.to_string() }).collect();
    out.push_str(&path.join("/"));
    if let Some(query) = parsed.query() {
        let pairs: Vec<String> = query
            .split('&')
            .map(|pair| match pair.split_once('=') {
                Some((key, value)) if looks_secret_name(key) || looks_like_token(value) => format!("{key}={MASK}"),
                _ if looks_like_token(pair) => MASK.to_string(),
                _ => pair.to_string(),
            })
            .collect();
        out.push('?');
        out.push_str(&pairs.join("&"));
    }
    if let Some(fragment) = parsed.fragment() {
        out.push('#');
        out.push_str(if looks_like_token(fragment) { MASK } else { fragment });
    }
    out
}

fn claude_mcp(project: Option<&Path>) -> Vec<Extension> {
    let mut items = Vec::new();
    let mut push = |servers: &Value, scope: Scope, path: PathBuf| {
        if let Some(map) = servers.as_object() {
            for (name, server) in map {
                items.push(Extension {
                    agent: Agent::Claude,
                    kind: ExtensionKind::Mcp,
                    name: name.clone(),
                    description: String::new(),
                    scope: scope.clone(),
                    path: Some(path.clone()),
                    detail: Some(mcp_detail(server)),
                    enabled: server["disabled"].as_bool() != Some(true),
                    invocation: None,
                });
            }
        }
    };
    let settings_path = fsutil::home().join(".claude").join("settings.json");
    let settings: Value = std::fs::read(&settings_path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    push(&settings["mcpServers"], Scope::User, settings_path);
    let config_path = fsutil::home().join(".claude.json");
    let config: Value = std::fs::read(&config_path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    push(&config["mcpServers"], Scope::User, config_path.clone());
    if let Some(project) = project {
        let key = project.to_string_lossy().to_string();
        push(&config["projects"][&key]["mcpServers"], Scope::Project, config_path);
        let project_file = project.join(".mcp.json");
        let shared: Value = std::fs::read(&project_file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        push(&shared["mcpServers"], Scope::Project, project_file);
    }
    items
}

// ---------------------------------------------------------------------------------------------
// Codex
// ---------------------------------------------------------------------------------------------

fn codex(project: Option<&Path>) -> Vec<Extension> {
    let home = fsutil::home();
    let codex_home = home.join(".codex");
    let invoke = |n: &str| format!("${n}");
    let mut items = Vec::new();
    if let Some(project) = project {
        items.extend(skills_in(Agent::Codex, &project.join(".agents").join("skills"), Scope::Project, invoke));
        items.extend(skills_in(Agent::Codex, &project.join(".codex").join("skills"), Scope::Project, invoke));
    }
    items.extend(skills_in(Agent::Codex, &home.join(".agents").join("skills"), Scope::User, invoke));
    items.extend(skills_in(Agent::Codex, &codex_home.join("skills"), Scope::User, invoke));
    items.extend(skills_in(Agent::Codex, &codex_home.join("skills").join(".system"), Scope::System, invoke));

    for file in markdown_files(&codex_home.join("prompts"), false) {
        let meta = read_markdown(&file).unwrap_or_default();
        let name = file.file_stem().unwrap_or_default().to_string_lossy().to_string();
        items.push(Extension {
            agent: Agent::Codex,
            kind: ExtensionKind::Command,
            invocation: Some(format!("/prompts:{name}")),
            description: field(&meta, "description").unwrap_or_default(),
            detail: field(&meta, "argument-hint"),
            name,
            scope: Scope::User,
            path: Some(file),
            enabled: true,
        });
    }

    let config_path = codex_home.join("config.toml");
    let config: toml::Table = std::fs::read_to_string(&config_path).ok().and_then(|t| t.parse().ok()).unwrap_or_default();
    if let Some(servers) = config.get("mcp_servers").and_then(|v| v.as_table()) {
        for (name, server) in servers {
            let url = server.get("url").and_then(|v| v.as_str());
            let command = server.get("command").and_then(|v| v.as_str()).unwrap_or("");
            let args: Vec<&str> =
                server.get("args").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|v| v.as_str()).collect()).unwrap_or_default();
            items.push(Extension {
                agent: Agent::Codex,
                kind: ExtensionKind::Mcp,
                name: name.clone(),
                description: String::new(),
                scope: Scope::User,
                path: Some(config_path.clone()),
                detail: Some(match url {
                    Some(url) => format!("http {}", redact_url(url)),
                    None => format!("stdio {command} {}", redact_args(&args)).trim().to_string(),
                }),
                enabled: server.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true),
                invocation: None,
            });
        }
    }
    if let Some(plugins) = config.get("plugins").and_then(|v| v.as_table()) {
        let cache = codex_home.join("plugins").join("cache");
        for (id, plugin) in plugins {
            let (name, marketplace) = id.split_once('@').unwrap_or((id, ""));
            let version_dir = subdirs(&cache.join(marketplace).join(name)).pop();
            let manifest: Value = version_dir
                .as_ref()
                .and_then(|d| std::fs::read(d.join(".codex-plugin").join("plugin.json")).ok())
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default();
            let enabled = plugin.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
            items.push(Extension {
                agent: Agent::Codex,
                kind: ExtensionKind::Plugin,
                name: id.clone(),
                description: manifest["description"].as_str().unwrap_or("").to_string(),
                scope: Scope::User,
                detail: manifest["version"].as_str().map(|v| format!("v{v}")),
                path: version_dir.clone(),
                enabled,
                invocation: None,
            });
            if let (Some(dir), true) = (version_dir, enabled) {
                items.extend(skills_in(Agent::Codex, &dir.join("skills"), Scope::Plugin(name.to_string()), |n| format!("${n}")));
            }
        }
    }
    items
}

// ---------------------------------------------------------------------------------------------
// MCP management (through each agent's own CLI, so their config formats stay authoritative)
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpTransport {
    Stdio,
    Http,
    /// Legacy server-sent events transport (Claude Code only).
    Sse,
}

/// Well-known MCP servers from the official Claude Code and Codex documentation
/// (code.claude.com/docs/en/mcp, learn.chatgpt.com/docs/extend/mcp), installable in one click.
pub struct CatalogEntry {
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub transport: McpTransport,
    /// URL for remote servers, command for stdio servers.
    pub target: &'static str,
    pub args: &'static [&'static str],
    pub agents: &'static [Agent],
    /// Header (Claude) needed for a personal token, when the server doesn't use OAuth.
    pub token_header: Option<&'static str>,
}

pub const CATALOG: &[CatalogEntry] = &[
    CatalogEntry {
        name: "github",
        title: "GitHub",
        description: "Pull requests, issues and repositories",
        transport: McpTransport::Http,
        target: "https://api.githubcopilot.com/mcp/",
        args: &[],
        agents: &[Agent::Claude, Agent::Codex],
        token_header: Some("Authorization"),
    },
    CatalogEntry {
        name: "sentry",
        title: "Sentry",
        description: "Errors, issues and performance data",
        transport: McpTransport::Http,
        target: "https://mcp.sentry.dev/mcp",
        args: &[],
        agents: &[Agent::Claude, Agent::Codex],
        token_header: None,
    },
    CatalogEntry {
        name: "notion",
        title: "Notion",
        description: "Pages, databases and docs",
        transport: McpTransport::Http,
        target: "https://mcp.notion.com/mcp",
        args: &[],
        agents: &[Agent::Claude],
        token_header: None,
    },
    CatalogEntry {
        name: "stripe",
        title: "Stripe",
        description: "Payments, customers and subscriptions",
        transport: McpTransport::Http,
        target: "https://mcp.stripe.com",
        args: &[],
        agents: &[Agent::Claude],
        token_header: None,
    },
    CatalogEntry {
        name: "hubspot",
        title: "HubSpot",
        description: "CRM contacts, deals and tickets",
        transport: McpTransport::Http,
        target: "https://mcp.hubspot.com/anthropic",
        args: &[],
        agents: &[Agent::Claude],
        token_header: None,
    },
    CatalogEntry {
        name: "asana",
        title: "Asana",
        description: "Projects and tasks",
        transport: McpTransport::Sse,
        target: "https://mcp.asana.com/sse",
        args: &[],
        agents: &[Agent::Claude],
        token_header: None,
    },
    CatalogEntry {
        name: "figma",
        title: "Figma",
        description: "Design files and components",
        transport: McpTransport::Http,
        target: "https://mcp.figma.com/mcp",
        args: &[],
        agents: &[Agent::Codex],
        token_header: None,
    },
    CatalogEntry {
        name: "context7",
        title: "Context7",
        description: "Up-to-date library documentation",
        transport: McpTransport::Stdio,
        target: "npx",
        args: &["-y", "@upstash/context7-mcp"],
        agents: &[Agent::Claude, Agent::Codex],
        token_header: None,
    },
    CatalogEntry {
        name: "playwright",
        title: "Playwright",
        description: "Control and inspect a browser",
        transport: McpTransport::Stdio,
        target: "npx",
        args: &["-y", "@playwright/mcp"],
        agents: &[Agent::Claude, Agent::Codex],
        token_header: None,
    },
];

impl CatalogEntry {
    pub fn spec(&self, agent: Agent, token: Option<&str>) -> McpServerSpec {
        let env = match (self.token_header, token.filter(|t| !t.trim().is_empty()), agent) {
            (Some(header), Some(token), Agent::Claude) => vec![(header.to_string(), format!("Bearer {}", token.trim()))],
            _ => Vec::new(),
        };
        McpServerSpec {
            name: self.name.to_string(),
            transport: self.transport,
            target: self.target.to_string(),
            args: self.args.iter().map(|a| a.to_string()).collect(),
            env,
            scope: McpScope::User,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpScope {
    User,
    Project,
}

#[derive(Debug, Clone)]
pub struct McpServerSpec {
    pub name: String,
    pub transport: McpTransport,
    /// Command for stdio servers, URL for HTTP servers.
    pub target: String,
    pub args: Vec<String>,
    /// `KEY=VALUE` environment variables (stdio) or headers (HTTP, Claude only).
    pub env: Vec<(String, String)>,
    pub scope: McpScope,
}

pub fn is_valid_server_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && name.chars().all(|c| c.is_ascii_alphanumeric() || "_-".contains(c))
}

/// Argument vector (program first) that registers the server with the agent.
pub fn mcp_add_command(agent: Agent, spec: &McpServerSpec) -> anyhow::Result<Vec<String>> {
    anyhow::ensure!(is_valid_server_name(&spec.name), "server names may only contain letters, numbers, - and _");
    anyhow::ensure!(!spec.target.trim().is_empty(), "a command or URL is required");
    if spec.transport != McpTransport::Stdio {
        anyhow::ensure!(
            spec.target.starts_with("https://")
                || spec.target.starts_with("http://localhost")
                || spec.target.starts_with("http://127.0.0.1"),
            "HTTP servers must use https (or localhost)"
        );
    }
    let mut argv: Vec<String> = Vec::new();
    match agent {
        Agent::Claude => {
            argv.extend(["claude", "mcp", "add"].map(String::from));
            argv.extend([
                "--scope".into(),
                match spec.scope {
                    McpScope::User => "user",
                    McpScope::Project => "project",
                }
                .into(),
            ]);
            match spec.transport {
                McpTransport::Stdio => {
                    argv.extend(["--transport".into(), "stdio".into()]);
                    for (k, v) in &spec.env {
                        argv.extend(["-e".into(), format!("{k}={v}")]);
                    }
                    argv.push(spec.name.clone());
                    argv.push("--".into());
                    argv.push(spec.target.clone());
                    argv.extend(spec.args.iter().cloned());
                }
                McpTransport::Http | McpTransport::Sse => {
                    argv.extend(["--transport".into(), if spec.transport == McpTransport::Sse { "sse".into() } else { "http".into() }]);
                    for (k, v) in &spec.env {
                        argv.extend(["--header".into(), format!("{k}: {v}")]);
                    }
                    argv.push(spec.name.clone());
                    argv.push(spec.target.clone());
                }
            }
        }
        Agent::Codex => {
            anyhow::ensure!(spec.scope == McpScope::User, "Codex only supports user-level MCP servers");
            argv.extend(["codex", "mcp", "add"].map(String::from));
            match spec.transport {
                McpTransport::Stdio => {
                    for (k, v) in &spec.env {
                        argv.extend(["--env".into(), format!("{k}={v}")]);
                    }
                    argv.push(spec.name.clone());
                    argv.push("--".into());
                    argv.push(spec.target.clone());
                    argv.extend(spec.args.iter().cloned());
                }
                McpTransport::Sse => anyhow::bail!("Codex does not support SSE MCP servers"),
                McpTransport::Http => {
                    anyhow::ensure!(spec.env.is_empty(), "Codex HTTP servers take credentials from environment variables, not headers");
                    argv.push(spec.name.clone());
                    argv.extend(["--url".into(), spec.target.clone()]);
                }
            }
        }
        other => anyhow::bail!("{} MCP servers are not managed here", other.display_name()),
    }
    Ok(argv)
}

pub fn mcp_remove_command(agent: Agent, name: &str, scope: &Scope) -> anyhow::Result<Vec<String>> {
    anyhow::ensure!(is_valid_server_name(name), "invalid server name");
    Ok(match agent {
        Agent::Claude => {
            let scope = match scope {
                Scope::Project => "project",
                _ => "user",
            };
            ["claude", "mcp", "remove", name, "--scope", scope].map(String::from).to_vec()
        }
        Agent::Codex => ["codex", "mcp", "remove", name].map(String::from).to_vec(),
        other => anyhow::bail!("{} MCP servers are not managed here", other.display_name()),
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn urls_with_credentials_keep_only_their_origin() {
        assert_eq!(super::url_to_keep("http://localhost:3000/dashboard?tab=2"), "http://localhost:3000/dashboard?tab=2");
        assert_eq!(super::url_to_keep("https://example.com/cb?code=x&access_token=example_not_a_real_token"), "https://example.com/");
        assert_eq!(super::url_to_keep("https://user:pw@example.com/"), "https://example.com/");
    }

    use super::*;

    #[test]
    fn catalog_entries_build_valid_commands() {
        for entry in CATALOG {
            for agent in entry.agents {
                let argv = mcp_add_command(*agent, &entry.spec(*agent, Some("tok")))
                    .unwrap_or_else(|e| panic!("{} for {agent:?}: {e}", entry.name));
                assert!(argv.contains(&entry.name.to_string()));
            }
        }
        let github = CATALOG.iter().find(|e| e.name == "github").unwrap();
        let argv = mcp_add_command(Agent::Claude, &github.spec(Agent::Claude, Some("ghp_x"))).unwrap();
        assert!(argv.windows(2).any(|w| w[0] == "--header" && w[1] == "Authorization: Bearer ghp_x"));
    }

    #[test]
    fn redacts_credentials_in_details() {
        assert_eq!(
            redact_args(&["-y", "figma-developer-mcp", "--figma-api-key=figd_example_not_a_real_key", "--stdio"]),
            "-y figma-developer-mcp --figma-api-key=•••• --stdio"
        );
        assert_eq!(redact_args(&["--token", "abc", "--port", "8080"]), "--token •••• --port 8080");
        assert_eq!(
            redact_args(&["-H", "Authorization: Bearer example-not-a-real-token", "-H", "Accept: text/plain"]),
            "-H Authorization: •••• -H Accept: text/plain"
        );
        assert_eq!(redact_args(&["sk1234567890abcdefghijklmnop"]), "••••");
        assert_eq!(
            redact_args(&["@modelcontextprotocol/server-filesystem", "/Users/me/projects"]),
            "@modelcontextprotocol/server-filesystem /Users/me/projects"
        );
        assert_eq!(redact_url("https://x.dev/mcp?api_key=secret&team=a"), "https://x.dev/mcp?api_key=••••&team=a");
    }

    /// A value is a credential by what it is, not only by what it is called.
    #[test]
    fn redacts_credentials_whatever_they_are_called() {
        // A name without "key" or "token" in it.
        assert_eq!(redact_args(&["FIGMA_PAT=figd_example_not_a_real_key", "--path=/Users/me/app"]), "FIGMA_PAT=•••• --path=/Users/me/app");
        assert_eq!(redact_args(&["REGION=eu", "BUILD=a1B2c3D4e5F6g7H8i9J0k1L2"]), "REGION=eu BUILD=••••");
        // Headers that are not called Authorization, and values with pairs inside.
        assert_eq!(redact_args(&["-H", "Cookie: session=example-not-a-real-session; theme=dark"]), "-H Cookie: ••••");
        assert_eq!(redact_args(&["--header=X-Request: id=1; sid=a1B2c3D4e5F6g7H8i9J0k1L2"]), "--header=X-Request: id=1; sid=••••");
        assert_eq!(redact_args(&["-H", "X-Custom: Bearer example-not-a-real-token"]), "-H X-Custom: Bearer ••••");
        // URLs as arguments: user, password, a token in the path, a token in the query.
        assert_eq!(redact_args(&["--url", "https://me:example-not-real@x.dev/mcp"]), "--url https://••••@x.dev/mcp");
        assert_eq!(
            redact_args(&["https://ghp_exampleNotARealToken0000000000000000@github.com/me/app.git"]),
            "https://••••@github.com/me/app.git"
        );
        assert_eq!(redact_url("https://mcp.x.dev/a1B2c3D4e5F6g7H8i9J0k1L2/sse?team=a"), "https://mcp.x.dev/••••/sse?team=a");
        assert_eq!(
            redact_url("https://x.dev/mcp?sid=a1B2c3D4e5F6g7H8i9J0k1L2#a1B2c3D4e5F6g7H8i9J0k1L2"),
            "https://x.dev/mcp?sid=••••#••••"
        );
        // JSON handed over as one argument.
        assert_eq!(redact_args(&[r#"{"apiKey":"example-not-a-real-key","region":"eu"}"#]), r#"{"apiKey":"••••","region":"eu"}"#);
        // Identifiers that are secrets as often as not.
        assert_eq!(redact_args(&["--project", "123e4567-e89b-12d3-a456-426614174000"]), "--project ••••");
        // What is not a credential stays readable.
        assert_eq!(
            redact_args(&["-y", "@scope/mcp-server-postgres", "--port=8080", "--mode", "read-only"]),
            "-y @scope/mcp-server-postgres --port=8080 --mode read-only"
        );
    }

    #[test]
    fn builds_mcp_commands() {
        let stdio = McpServerSpec {
            name: "files".into(),
            transport: McpTransport::Stdio,
            target: "npx".into(),
            args: vec!["-y".into(), "server".into()],
            env: vec![("TOKEN".into(), "x y".into())],
            scope: McpScope::User,
        };
        assert_eq!(
            mcp_add_command(Agent::Claude, &stdio).unwrap(),
            ["claude", "mcp", "add", "--scope", "user", "--transport", "stdio", "-e", "TOKEN=x y", "files", "--", "npx", "-y", "server"]
        );
        assert_eq!(
            mcp_add_command(Agent::Codex, &stdio).unwrap(),
            ["codex", "mcp", "add", "--env", "TOKEN=x y", "files", "--", "npx", "-y", "server"]
        );

        let http = McpServerSpec {
            name: "docs".into(),
            transport: McpTransport::Http,
            target: "https://x.dev/mcp".into(),
            args: vec![],
            env: vec![],
            scope: McpScope::Project,
        };
        assert_eq!(
            mcp_add_command(Agent::Claude, &http).unwrap(),
            ["claude", "mcp", "add", "--scope", "project", "--transport", "http", "docs", "https://x.dev/mcp"]
        );
        assert!(mcp_add_command(Agent::Codex, &http).is_err(), "codex has no project scope");

        let insecure = McpServerSpec { target: "http://evil.example/mcp".into(), scope: McpScope::User, ..http.clone() };
        assert!(mcp_add_command(Agent::Claude, &insecure).is_err());
        let bad_name = McpServerSpec { name: "a; rm -rf".into(), ..stdio };
        assert!(mcp_add_command(Agent::Claude, &bad_name).is_err());
        assert!(mcp_remove_command(Agent::Codex, "../x", &Scope::User).is_err());
    }

    #[test]
    fn parses_front_matter() {
        let text = "---\nname: team-lead\ndescription: \"Plans work\"\nmodel: opus\nnotes: >\n  folded\n  text\n---\n# Body\nname: ignored";
        let meta = front_matter(text);
        assert_eq!(field(&meta, "name").as_deref(), Some("team-lead"));
        assert_eq!(field(&meta, "description").as_deref(), Some("Plans work"));
        assert_eq!(field(&meta, "notes").as_deref(), Some("folded text"));
        assert!(front_matter("# no front matter").is_empty());
    }

    #[test]
    fn discovers_project_items() {
        let dir = std::env::temp_dir().join(format!("agentty-ext-{}", std::process::id()));
        let skill = dir.join(".claude/skills/deploy");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(skill.join("SKILL.md"), "---\nname: deploy\ndescription: Ship it\n---\n").unwrap();
        std::fs::create_dir_all(dir.join(".claude/commands/git")).unwrap();
        std::fs::write(dir.join(".claude/commands/git/commit.md"), "---\ndescription: Commit\n---\n").unwrap();
        std::fs::write(dir.join(".mcp.json"), r#"{"mcpServers":{"docs":{"type":"http","url":"https://example.com/mcp"}}}"#).unwrap();

        let items = claude(Some(&dir));
        let find = |kind, name: &str| items.iter().find(|e| e.kind == kind && e.name == name && e.scope == Scope::Project);
        assert_eq!(find(ExtensionKind::Skill, "deploy").unwrap().invocation.as_deref(), Some("/deploy"));
        assert_eq!(find(ExtensionKind::Command, "git:commit").unwrap().invocation.as_deref(), Some("/git:commit"));
        assert_eq!(find(ExtensionKind::Mcp, "docs").unwrap().detail.as_deref(), Some("http https://example.com/mcp"));
        std::fs::remove_dir_all(dir).ok();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum McpHealth {
    Connected,
    Failed,
    NeedsAuth,
    Enabled,
    Disabled,
    Unknown,
}

/// One server from `claude mcp list` / `codex mcp list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct McpStatus {
    pub name: String,
    pub health: McpHealth,
}

/// `claude mcp list`: `name: target - ✔ Connected` (targets can contain " - ", so split from the right).
pub fn parse_claude_mcp_list(output: &str) -> Vec<McpStatus> {
    output
        .lines()
        .filter_map(|line| {
            let (head, state) = line.rsplit_once(" - ")?;
            let (name, _) = head.split_once(": ")?;
            let state = state.to_lowercase();
            let health = if state.contains("connected") && !state.contains("failed") {
                McpHealth::Connected
            } else if state.contains("auth") {
                McpHealth::NeedsAuth
            } else if state.contains("fail") || state.contains('✘') || state.contains('✗') {
                McpHealth::Failed
            } else {
                McpHealth::Unknown
            };
            Some(McpStatus { name: name.trim().to_string(), health })
        })
        .collect()
}

/// `codex mcp list`: a table whose `Status` column says enabled / disabled.
pub fn parse_codex_mcp_list(output: &str) -> Vec<McpStatus> {
    let mut lines = output.lines().skip_while(|l| !l.trim_start().starts_with("Name"));
    let Some(header) = lines.next() else { return Vec::new() };
    let Some(status_col) = header.find("Status") else { return Vec::new() };
    lines
        .filter(|l| !l.trim().is_empty())
        .filter_map(|line| {
            let name = line.split_whitespace().next()?.to_string();
            let status = line.get(status_col..).unwrap_or_default().split_whitespace().next().unwrap_or_default();
            let health = match status {
                "enabled" => McpHealth::Enabled,
                "disabled" => McpHealth::Disabled,
                _ => McpHealth::Unknown,
            };
            Some(McpStatus { name, health })
        })
        .collect()
}

#[cfg(test)]
mod mcp_status_tests {
    use super::*;

    #[test]
    fn parses_claude_health() {
        let out = "Checking MCP server health…\n\nclaude.ai Gmail: https://gmailmcp.googleapis.com/mcp/v1 - ✔ Connected\nlocal: npx -y some-mcp --flag - x - ✘ Failed to connect\nlinear: https://mcp.linear.app/sse - ⚠ Needs authentication\n";
        let list = parse_claude_mcp_list(out);
        assert_eq!(list.len(), 3);
        assert_eq!((list[0].name.as_str(), list[0].health), ("claude.ai Gmail", McpHealth::Connected));
        assert_eq!(list[1].health, McpHealth::Failed);
        assert_eq!(list[2].health, McpHealth::NeedsAuth);
    }

    #[test]
    fn parses_codex_table() {
        let out = "Name      Command   Args  Env  Cwd  Status    Auth\ncodex_app /x/launch  ./s   -    /a   disabled  Unsupported\nnode_repl /x/node    -     -    -    enabled   Unsupported\n";
        let list = parse_codex_mcp_list(out);
        assert_eq!(
            list.iter().map(|s| (s.name.as_str(), s.health)).collect::<Vec<_>>(),
            [("codex_app", McpHealth::Disabled), ("node_repl", McpHealth::Enabled)]
        );
    }
}
