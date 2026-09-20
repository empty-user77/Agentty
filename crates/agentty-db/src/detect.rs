//! Finds the databases a project is configured for, so the database page appears only for projects
//! that have one and connects without extra setup when the configuration holds everything.
//!
//! Looked at (at the project root; nothing is written, nothing leaves the machine):
//! - `.env`, `.env.local`, `.env.development`, `.env.development.local`: `DATABASE_URL` and friends,
//!   `DB_*` (Laravel style), `MYSQL_*`, `POSTGRES_*` / `PG*`, `MONGODB_URI`, `ORACLE_URL`
//! - Spring: `src/main/resources/application(-<profile>).properties|yml|yaml` —
//!   `spring.datasource.*`, `spring.data.mongodb.*`
//! - Prisma: `prisma/schema.prisma` / `schema.prisma` — the `datasource` block
//! - Compose files: MySQL, MariaDB, PostgreSQL, MongoDB and Oracle services published on this machine
//!
//! `${VAR}`, `${VAR:default}`, `env("VAR")` and `$VAR` are filled from those `.env` files. A value
//! that stays a reference (a secret manager, an environment variable set elsewhere, an encrypted
//! value) leaves the field to be entered by the user.

use crate::model::{ConnectionConfig, Engine, Missing, Secret};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A database found in the project's configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detected {
    /// Where it was found, relative to the project root (`.env`, `src/main/resources/application.yml`, …).
    pub source: String,
    pub config: ConnectionConfig,
    /// Fields the configuration refers to but doesn't hold (the user enters them).
    pub missing: Vec<Missing>,
}

/// Files read at most (a `.env` or YAML is small; anything bigger is not a config file).
const MAX_FILE: u64 = 256 * 1024;

fn read(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_FILE {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

/// Every database configured in the project at `root`.
pub fn detect(root: &Path) -> Vec<Detected> {
    let mut env = BTreeMap::new();
    let mut found = Vec::new();
    for name in [".env", ".env.local", ".env.development", ".env.development.local"] {
        if let Some(text) = read(&root.join(name)) {
            let vars = parse_env(&text);
            found.extend(from_env(&vars, name));
            env.extend(vars);
        }
    }
    let resources = root.join("src").join("main").join("resources");
    if let Ok(entries) = std::fs::read_dir(&resources) {
        let mut files: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                name.starts_with("application") && (name.ends_with(".properties") || name.ends_with(".yml") || name.ends_with(".yaml"))
            })
            .collect();
        files.sort();
        for path in files {
            let Some(text) = read(&path) else { continue };
            let keys = if path.extension().is_some_and(|e| e == "properties") { parse_properties(&text) } else { flatten_yaml(&text) };
            let source = path.strip_prefix(root).unwrap_or(&path).display().to_string();
            found.extend(from_spring(&keys, &env, &source));
        }
    }
    for name in ["prisma/schema.prisma", "schema.prisma"] {
        if let Some(text) = read(&root.join(name)) {
            found.extend(from_prisma(&text, &env, name));
        }
    }
    for name in ["compose.yaml", "compose.yml", "docker-compose.yml", "docker-compose.yaml"] {
        if let Some(text) = read(&root.join(name)) {
            found.extend(from_compose(&text, &env, name));
        }
    }
    let mut unique: Vec<Detected> = Vec::new();
    for item in found {
        let same = |d: &Detected| {
            d.config.engine == item.config.engine
                && d.config.host == item.config.host
                && d.config.port == item.config.port
                && d.config.database == item.config.database
                && d.config.user == item.config.user
        };
        match unique.iter_mut().find(|d| same(d)) {
            // Keep the one that knows more (a password over none).
            Some(existing) if existing.missing.len() > item.missing.len() => *existing = item,
            Some(_) => {}
            None => unique.push(item),
        }
    }
    unique
}

// -- values ------------------------------------------------------------------------------------

/// `KEY=value` lines (`export`, quotes and `#` comments handled).
pub fn parse_env(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((key, value)) = line.split_once('=') else { continue };
        let key = key.trim();
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.') {
            continue;
        }
        out.insert(key.to_string(), unquote(value.trim()));
    }
    out
}

fn unquote(value: &str) -> String {
    let value = value.trim();
    for quote in ['"', '\''] {
        if value.len() >= 2 && value.starts_with(quote) && value.ends_with(quote) {
            return value[1..value.len() - 1].to_string();
        }
    }
    // An unquoted value ends at ` #` (a trailing comment).
    value.split(" #").next().unwrap_or(value).trim().to_string()
}

/// A value with its references filled from `env`, or `None` when it still refers to something
/// elsewhere (an unset variable, a secret manager, an encrypted value).
fn resolve(value: &str, env: &BTreeMap<String, String>) -> Option<String> {
    let value = value.trim();
    let lower = value.to_ascii_lowercase();
    if lower.starts_with("{cipher}")
        || lower.contains("secretsmanager")
        || lower.contains("{{resolve:")
        || lower.starts_with("vault:")
        || lower.starts_with("sm://")
        || lower.starts_with("${sm")
    {
        return None;
    }
    if let Some(name) = value.strip_prefix("env(\"").and_then(|v| v.strip_suffix("\")")) {
        return env.get(name).cloned();
    }
    let mut out = String::new();
    let mut rest = value;
    while let Some(start) = rest.find('$') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        if let Some(inner) = after.strip_prefix('{') {
            let end = inner.find('}')?;
            let (name, default) = match inner[..end].split_once(':') {
                Some((n, d)) => (n, Some(d.trim_start_matches('-'))),
                None => (&inner[..end], None),
            };
            out.push_str(&env.get(name).cloned().or_else(|| default.map(str::to_string))?);
            rest = &inner[end + 1..];
        } else {
            let len = after.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').count();
            if len == 0 {
                out.push('$');
                rest = after;
                continue;
            }
            out.push_str(env.get(&after[..len])?);
            rest = &after[len..];
        }
    }
    out.push_str(rest);
    Some(out)
}

// -- URLs --------------------------------------------------------------------------------------

/// Parses `mysql://`, `mariadb://`, `postgres(ql)://`, `mongodb(+srv)://` URLs and JDBC URLs
/// (`jdbc:mysql://…`, `jdbc:mariadb://…`, `jdbc:postgresql://…`, `jdbc:oracle:thin:@…`).
pub fn parse_url(raw: &str) -> Option<ConnectionConfig> {
    let raw = raw.trim();
    if let Some(oracle) = raw.strip_prefix("jdbc:oracle:thin:") {
        return parse_oracle(oracle);
    }
    let raw = raw.strip_prefix("jdbc:").unwrap_or(raw);
    let (scheme, _) = raw.split_once("://")?;
    let engine = Engine::from_name(scheme)?;
    let url = url::Url::parse(raw).ok()?;
    let host = url.host_str()?.trim_matches(['[', ']']).to_string();
    let user = Some(percent_decode(url.username())).filter(|u| !u.is_empty());
    let password = url.password().map(percent_decode).filter(|p| !p.is_empty()).map(Secret::new);
    let database = Some(url.path().trim_start_matches('/').to_string()).filter(|d| !d.is_empty());
    let options: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| {
            let key = k.to_ascii_lowercase();
            !key.contains("password") && !key.contains("secret") && !key.contains("token")
        })
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let srv = scheme == "mongodb+srv";
    Some(ConnectionConfig { engine, port: url.port().unwrap_or(engine.default_port()), host, database, user, password, srv, options })
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Some(byte) = std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
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

/// `@host:port:SID`, `@//host:port/service`, `@host:port/service` (after `jdbc:oracle:thin:`),
/// optionally with `user/password` before the `@`.
fn parse_oracle(rest: &str) -> Option<ConnectionConfig> {
    let (credentials, address) = rest.split_once('@')?;
    let (user, password) = match credentials.split_once('/') {
        Some((u, p)) if !u.is_empty() => (Some(u.to_string()), Some(Secret::new(p))),
        _ => (None, None),
    };
    let address = address.trim_start_matches("//");
    if address.starts_with('(') {
        // A TNS descriptor: not parsed; the user enters the parts.
        return None;
    }
    let (host_port, database) = match address.split_once('/') {
        Some((hp, service)) => (hp, Some(service.to_string())),
        None => {
            let mut parts = address.rsplitn(2, ':');
            let sid = parts.next()?;
            (parts.next()?, Some(sid.to_string()))
        }
    };
    let (host, port) = match host_port.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().ok()?),
        None => (host_port.to_string(), Engine::Oracle.default_port()),
    };
    Some(ConnectionConfig {
        engine: Engine::Oracle,
        host,
        port,
        database,
        user,
        password: password.filter(|p| !p.is_empty()),
        srv: false,
        options: Vec::new(),
    })
}

/// What a partly known connection still needs.
fn missing_of(config: &ConnectionConfig, password_hidden: bool) -> Vec<Missing> {
    let mut missing = Vec::new();
    if config.host.is_empty() {
        missing.push(Missing::Host);
    }
    if config.user.is_none() {
        missing.push(Missing::User);
    }
    if config.password.is_none() && (password_hidden || config.user.is_some()) {
        missing.push(Missing::Password);
    }
    missing
}

fn detected(source: &str, config: ConnectionConfig, password_hidden: bool) -> Detected {
    let missing = missing_of(&config, password_hidden);
    Detected { source: source.to_string(), config, missing }
}

/// A connection URL whose parts may be references: filled where possible; a password that stays a
/// reference is left for the user.
fn from_url_value(value: &str, env: &BTreeMap<String, String>, source: &str) -> Option<Detected> {
    if let Some(resolved) = resolve(value, env) {
        return parse_url(&resolved).map(|c| detected(source, c, false));
    }
    // A part (usually the password) is a reference: blank it and ask for it.
    let blanked = blank_references(value);
    parse_url(&blanked).map(|mut c| {
        c.password = None;
        detected(source, c, true)
    })
}

fn blank_references(value: &str) -> String {
    let mut out = String::new();
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        rest = match rest[start..].find('}') {
            Some(end) => &rest[start + end + 1..],
            None => "",
        };
    }
    out.push_str(rest);
    out
}

// -- .env ----------------------------------------------------------------------------------------

type PartKeys = (
    Option<Engine>,
    &'static [&'static str],
    &'static [&'static str],
    &'static [&'static str],
    &'static [&'static str],
    &'static [&'static str],
);

fn from_env(vars: &BTreeMap<String, String>, source: &str) -> Vec<Detected> {
    let mut out = Vec::new();
    for key in
        ["DATABASE_URL", "DB_URL", "MYSQL_URL", "POSTGRES_URL", "POSTGRESQL_URL", "MONGODB_URI", "MONGO_URI", "MONGO_URL", "ORACLE_URL"]
    {
        if let Some(value) = vars.get(key) {
            out.extend(from_url_value(value, vars, source));
        }
    }
    // Separate parts: DB_* (with DB_CONNECTION naming the engine), MYSQL_*, POSTGRES_* / PG*.
    let get = |keys: &[&str]| keys.iter().find_map(|k| vars.get(*k)).map(|v| resolve(v, vars));
    let groups: [PartKeys; 3] = [
        (
            vars.get("DB_CONNECTION").and_then(|c| Engine::from_name(c)),
            &["DB_HOST"],
            &["DB_PORT"],
            &["DB_USERNAME", "DB_USER"],
            &["DB_PASSWORD", "DB_PASS"],
            &["DB_DATABASE", "DB_NAME"],
        ),
        (Some(Engine::MySql), &["MYSQL_HOST"], &["MYSQL_PORT"], &["MYSQL_USER"], &["MYSQL_PASSWORD"], &["MYSQL_DATABASE"]),
        (
            Some(Engine::Postgres),
            &["POSTGRES_HOST", "PGHOST"],
            &["POSTGRES_PORT", "PGPORT"],
            &["POSTGRES_USER", "PGUSER"],
            &["POSTGRES_PASSWORD", "PGPASSWORD"],
            &["POSTGRES_DB", "PGDATABASE"],
        ),
    ];
    for (engine, host, port, user, password, database) in groups {
        let Some(Some(host)) = get(host) else { continue };
        let engine = engine.unwrap_or(Engine::MySql);
        let port = get(port).flatten().and_then(|p| p.parse().ok()).unwrap_or(engine.default_port());
        let password_value = get(password);
        let config = ConnectionConfig {
            engine,
            host,
            port,
            database: get(database).flatten().filter(|d| !d.is_empty()),
            user: get(user).flatten().filter(|u| !u.is_empty()),
            password: password_value.clone().flatten().filter(|p| !p.is_empty()).map(Secret::new),
            srv: false,
            options: Vec::new(),
        };
        out.push(detected(source, config, matches!(password_value, Some(None))));
    }
    out
}

// -- Spring --------------------------------------------------------------------------------------

/// `key=value` / `key: value` lines of a `.properties` file.
pub fn parse_properties(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
            continue;
        }
        if let Some(at) = line.find(['=', ':']) {
            out.insert(line[..at].trim().to_string(), unquote(&line[at + 1..]));
        }
    }
    out
}

/// Nested `key: value` maps of a simple YAML file as dotted keys (`spring.datasource.url`). List
/// items are skipped; a datasource needs none.
pub fn flatten_yaml(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut stack: Vec<(usize, String)> = Vec::new();
    for raw in text.lines() {
        let trimmed = raw.trim();
        if trimmed == "---" {
            stack.clear();
            continue;
        }
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('-') {
            continue;
        }
        let indent = raw.len() - raw.trim_start().len();
        let Some((key, value)) = trimmed.split_once(':') else { continue };
        let key = key.trim().trim_matches(['"', '\'']).to_string();
        while stack.last().is_some_and(|(i, _)| *i >= indent) {
            stack.pop();
        }
        let path = stack.iter().map(|(_, k)| k.as_str()).chain(std::iter::once(key.as_str())).collect::<Vec<_>>().join(".");
        let value = value.trim();
        if value.is_empty() {
            stack.push((indent, key));
        } else {
            out.insert(path, unquote(value));
        }
    }
    out
}

fn from_spring(keys: &BTreeMap<String, String>, env: &BTreeMap<String, String>, source: &str) -> Vec<Detected> {
    let mut out = Vec::new();
    let get = |k: &str| keys.get(k).map(|v| (v.clone(), resolve(v, env)));
    if let Some((raw_url, _)) = get("spring.datasource.url") {
        if let Some(mut found) = from_url_value(&raw_url, env, source) {
            if let Some((_, user)) = get("spring.datasource.username") {
                found.config.user = user.filter(|u| !u.is_empty());
            }
            let mut hidden = found.missing.contains(&Missing::Password);
            if let Some((_, password)) = get("spring.datasource.password") {
                hidden = password.is_none();
                found.config.password = password.filter(|p| !p.is_empty()).map(Secret::new);
            }
            found.missing = missing_of(&found.config, hidden);
            out.push(found);
        }
    }
    if let Some((raw_uri, _)) = get("spring.data.mongodb.uri") {
        out.extend(from_url_value(&raw_uri, env, source));
    } else if let Some((_, Some(host))) = get("spring.data.mongodb.host") {
        let port = get("spring.data.mongodb.port").and_then(|(_, p)| p).and_then(|p| p.parse().ok()).unwrap_or(27017);
        let password = get("spring.data.mongodb.password");
        let config = ConnectionConfig {
            engine: Engine::MongoDb,
            host,
            port,
            database: get("spring.data.mongodb.database").and_then(|(_, d)| d),
            user: get("spring.data.mongodb.username").and_then(|(_, u)| u),
            password: password.clone().and_then(|(_, p)| p).filter(|p| !p.is_empty()).map(Secret::new),
            srv: false,
            options: Vec::new(),
        };
        out.push(detected(source, config, password.is_some_and(|(_, p)| p.is_none())));
    }
    out
}

// -- Prisma --------------------------------------------------------------------------------------

fn from_prisma(text: &str, env: &BTreeMap<String, String>, source: &str) -> Vec<Detected> {
    let Some(start) = text.find("datasource") else { return Vec::new() };
    let block = &text[start..];
    let Some(end) = block.find('}') else { return Vec::new() };
    let url = block[..end]
        .lines()
        .map(str::trim)
        .find_map(|l| l.strip_prefix("url").map(str::trim))
        .and_then(|v| v.strip_prefix('='))
        .map(|v| unquote(v.trim()));
    let Some(url) = url else { return Vec::new() };
    from_url_value(&url, env, source).into_iter().collect()
}

// -- compose -------------------------------------------------------------------------------------

/// Database services of a compose file that publish their port on this machine.
fn from_compose(text: &str, env: &BTreeMap<String, String>, source: &str) -> Vec<Detected> {
    let mut out = Vec::new();
    for service in compose_services(text) {
        let image = service.image.to_ascii_lowercase();
        let base = image.rsplit('/').next().unwrap_or(&image).split(':').next().unwrap_or("").to_string();
        let engine = match base.as_str() {
            "mysql" | "percona" => Engine::MySql,
            "mariadb" => Engine::MariaDb,
            "postgres" | "postgis" => Engine::Postgres,
            "mongo" => Engine::MongoDb,
            b if b.starts_with("oracle") || image.contains("oracle") => Engine::Oracle,
            _ => continue,
        };
        let inside = match engine {
            Engine::MySql | Engine::MariaDb => 3306,
            Engine::Postgres => 5432,
            Engine::MongoDb => 27017,
            Engine::Oracle => 1521,
        };
        let Some(port) = service.published(inside) else { continue };
        let var = |keys: &[&str]| keys.iter().find_map(|k| service.environment.get(*k)).map(|v| resolve(v, env));
        let (user, password, database) = match engine {
            Engine::MySql | Engine::MariaDb => {
                let user = var(&["MYSQL_USER", "MARIADB_USER"]).flatten();
                let password = if user.is_some() {
                    var(&["MYSQL_PASSWORD", "MARIADB_PASSWORD"])
                } else {
                    var(&["MYSQL_ROOT_PASSWORD", "MARIADB_ROOT_PASSWORD"])
                };
                (user.or(Some("root".into())), password, var(&["MYSQL_DATABASE", "MARIADB_DATABASE"]).flatten())
            }
            Engine::Postgres => (
                var(&["POSTGRES_USER"]).flatten().or(Some("postgres".into())),
                var(&["POSTGRES_PASSWORD"]),
                var(&["POSTGRES_DB"]).flatten().or_else(|| var(&["POSTGRES_USER"]).flatten()),
            ),
            Engine::MongoDb => (
                var(&["MONGO_INITDB_ROOT_USERNAME"]).flatten(),
                var(&["MONGO_INITDB_ROOT_PASSWORD"]),
                var(&["MONGO_INITDB_DATABASE"]).flatten(),
            ),
            Engine::Oracle => (Some("system".into()), var(&["ORACLE_PASSWORD", "ORACLE_PWD"]), Some("FREEPDB1".into())),
        };
        let config = ConnectionConfig {
            engine,
            host: "127.0.0.1".into(),
            port,
            database,
            user,
            password: password.clone().flatten().filter(|p| !p.is_empty()).map(Secret::new),
            srv: false,
            options: Vec::new(),
        };
        out.push(detected(&format!("{source} · {}", service.name), config, matches!(password, Some(None))));
    }
    out
}

#[derive(Debug, Default)]
struct ComposeService {
    name: String,
    image: String,
    ports: Vec<String>,
    environment: BTreeMap<String, String>,
}

impl ComposeService {
    /// The host port a container port is published on (`"3307:3306"`, `"127.0.0.1:3307:3306"`).
    fn published(&self, inside: u16) -> Option<u16> {
        self.ports.iter().find_map(|spec| {
            let spec = spec.trim().trim_matches(['"', '\'']);
            let spec = spec.split('/').next().unwrap_or(spec);
            let parts: Vec<&str> = spec.split(':').collect();
            let (host, container) = match parts.as_slice() {
                [h, c] => (*h, *c),
                [_, h, c] => (*h, *c),
                _ => return None,
            };
            if container.parse::<u16>().ok()? == inside {
                host.parse().ok()
            } else {
                None
            }
        })
    }
}

/// The `services:` of a compose file: each one's image, port specs and environment (map or list).
fn compose_services(text: &str) -> Vec<ComposeService> {
    let mut services: Vec<ComposeService> = Vec::new();
    let mut in_services = false;
    let mut service_indent: Option<usize> = None;
    let (mut field, mut field_indent) = (String::new(), 0);
    for raw in text.lines() {
        if raw.trim().is_empty() || raw.trim_start().starts_with('#') {
            continue;
        }
        let indent = raw.len() - raw.trim_start().len();
        let line = raw.trim();
        if indent == 0 {
            in_services = line == "services:";
            service_indent = None;
            continue;
        }
        if !in_services {
            continue;
        }
        let indent_of_service = *service_indent.get_or_insert(indent);
        if indent == indent_of_service {
            if let Some(name) = line.strip_suffix(':') {
                services.push(ComposeService { name: name.trim_matches(['"', '\'']).to_string(), ..Default::default() });
                field.clear();
            }
            continue;
        }
        let Some(service) = services.last_mut() else { continue };
        if !line.starts_with('-') && (field.is_empty() || indent <= field_indent) {
            let (key, value) = line.split_once(':').map(|(k, v)| (k.trim(), v.trim())).unwrap_or((line, ""));
            field = key.to_string();
            field_indent = indent;
            if key == "image" {
                service.image = unquote(value);
            }
            continue;
        }
        match field.as_str() {
            "ports" => {
                if let Some(item) = line.strip_prefix('-') {
                    service.ports.push(unquote(item));
                }
            }
            "environment" => {
                if let Some(item) = line.strip_prefix('-') {
                    let item = unquote(item);
                    if let Some((k, v)) = item.split_once('=') {
                        service.environment.insert(k.trim().to_string(), v.trim().to_string());
                    }
                } else if let Some((k, v)) = line.split_once(':') {
                    service.environment.insert(k.trim().to_string(), unquote(v));
                }
            }
            _ => {}
        }
    }
    services
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("agentty-db-detect-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (file, text) in files {
            let path = dir.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        dir
    }

    #[test]
    fn urls_of_every_engine() {
        let c = parse_url("mysql://app:example-not-a-real-pass@db.example.com:3307/shop?ssl-mode=REQUIRED").unwrap();
        assert_eq!(
            (c.engine, c.host.as_str(), c.port, c.database.as_deref(), c.user.as_deref()),
            (Engine::MySql, "db.example.com", 3307, Some("shop"), Some("app"))
        );
        assert_eq!(c.password.as_ref().map(Secret::expose), Some("example-not-a-real-pass"));
        let c = parse_url("jdbc:mariadb://localhost/shop").unwrap();
        assert_eq!((c.engine, c.port), (Engine::MariaDb, 3306));
        let c = parse_url("postgresql://u:example%40not-a-real-pass@x.abc.us-east-1.rds.amazonaws.com/app").unwrap();
        assert!(c.is_rds() && c.engine == Engine::Postgres);
        assert_eq!(c.password.as_ref().map(Secret::expose), Some("example@not-a-real-pass"));
        let c = parse_url(
            "mongodb+srv://u:example-not-a-real-password@cluster0.example.mongodb.net/app?authSource=admin&password=example-fake",
        )
        .unwrap();
        assert!(c.srv && c.engine == Engine::MongoDb);
        assert_eq!(c.options, vec![("authSource".to_string(), "admin".to_string())], "password-like options are dropped");
        let c = parse_url("jdbc:oracle:thin:@//ora.example.com:1522/ORCLPDB1").unwrap();
        assert_eq!((c.engine, c.host.as_str(), c.port, c.database.as_deref()), (Engine::Oracle, "ora.example.com", 1522, Some("ORCLPDB1")));
        let c = parse_url("jdbc:oracle:thin:scott/example-fake@ora:1521:XE").unwrap();
        assert_eq!((c.user.as_deref(), c.database.as_deref()), (Some("scott"), Some("XE")));
        assert!(parse_url("jdbc:oracle:thin:@(DESCRIPTION=(ADDRESS=(HOST=x)))").is_none());
        assert!(parse_url("redis://localhost").is_none());
        assert_eq!(percent_decode("a%2"), "a%2");
        assert_eq!(percent_decode("%ED%95%9C"), "한");
    }

    #[test]
    fn env_files_with_parts_urls_and_references() {
        // The Mongo password is a reference to a variable set nowhere here (built at runtime so the
        // test holds no credential-shaped URL).
        let credentials = format!("root:{}", concat!("$", "{MONGO_PASS}"));
        let env = format!(
            "DB_CONNECTION=pgsql\nDB_HOST=127.0.0.1\nDB_PORT=5433\nDB_DATABASE=app\nDB_USERNAME=app\nDB_PASSWORD=\"example-not-a-real-pass\"\n\
             MONGODB_URI=mongodb://{credentials}@localhost:27018/app\n"
        );
        let root = project("env", &[(".env", &env)]);
        let found = detect(&root);
        let pg = found.iter().find(|d| d.config.engine == Engine::Postgres).unwrap();
        assert_eq!((pg.config.port, pg.config.database.as_deref()), (5433, Some("app")));
        assert!(pg.missing.is_empty());
        let mongo = found.iter().find(|d| d.config.engine == Engine::MongoDb).unwrap();
        assert_eq!(mongo.config.user.as_deref(), Some("root"));
        assert!(mongo.config.password.is_none() && mongo.missing == vec![Missing::Password]);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn spring_properties_and_yaml() {
        let root = project(
            "spring",
            &[
                (".env", "SPRING_DB_PASSWORD=example-not-a-real-pass\n"),
                (
                    "src/main/resources/application.yml",
                    "spring:\n  datasource:\n    url: jdbc:mysql://localhost:3306/shop?useSSL=false\n    username: app\n    password: ${SPRING_DB_PASSWORD}\n  data:\n    mongodb:\n      uri: mongodb://localhost/app\n",
                ),
                (
                    "src/main/resources/application-prod.properties",
                    "spring.datasource.url=jdbc:oracle:thin:@//ora.example.com:1521/PROD\nspring.datasource.username=app\nspring.datasource.password={cipher}abc\n",
                ),
            ],
        );
        let found = detect(&root);
        let mysql = found.iter().find(|d| d.config.engine == Engine::MySql).unwrap();
        assert_eq!(mysql.config.password.as_ref().map(Secret::expose), Some("example-not-a-real-pass"));
        assert!(mysql.missing.is_empty() && mysql.source.ends_with("application.yml"));
        let oracle = found.iter().find(|d| d.config.engine == Engine::Oracle).unwrap();
        assert_eq!(oracle.missing, vec![Missing::Password], "an encrypted value is entered by the user");
        assert!(found.iter().any(|d| d.config.engine == Engine::MongoDb));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn prisma_and_compose() {
        let root = project(
            "compose",
            &[
                (".env", "DATABASE_URL=mysql://app:example-not-a-real-pass@localhost:3310/shop\n"),
                ("prisma/schema.prisma", "datasource db {\n  provider = \"mysql\"\n  url      = env(\"DATABASE_URL\")\n}\n"),
                (
                    "compose.yaml",
                    "services:\n  db:\n    image: mariadb:11\n    ports:\n      - \"3311:3306\"\n    environment:\n      MARIADB_DATABASE: shop\n      MARIADB_USER: app\n      MARIADB_PASSWORD: example-not-a-real-pass\n  cache:\n    image: redis:7\n    ports: [\"6379:6379\"]\n  mongo:\n    image: mongo:7\n    ports:\n      - 127.0.0.1:27019:27017\n    environment:\n      - MONGO_INITDB_ROOT_USERNAME=root\n      - MONGO_INITDB_ROOT_PASSWORD=${MONGO_ROOT}\n",
                ),
            ],
        );
        let found = detect(&root);
        // .env and Prisma name the same database: listed once.
        assert_eq!(found.iter().filter(|d| d.config.port == 3310).count(), 1);
        let maria = found.iter().find(|d| d.config.engine == Engine::MariaDb).unwrap();
        assert_eq!((maria.config.port, maria.config.user.as_deref(), maria.config.database.as_deref()), (3311, Some("app"), Some("shop")));
        assert!(maria.missing.is_empty() && maria.source == "compose.yaml · db");
        let mongo = found.iter().find(|d| d.config.engine == Engine::MongoDb).unwrap();
        assert_eq!(mongo.config.port, 27019);
        assert_eq!(mongo.missing, vec![Missing::Password]);
        assert!(!found.iter().any(|d| d.source.contains("cache")));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn nothing_configured_nothing_found() {
        let root = project("none", &[("README.md", "hello")]);
        assert!(detect(&root).is_empty());
        std::fs::remove_dir_all(root).ok();
    }
}
