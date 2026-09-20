//! A project's connections as the database page and agents see them: the ones its configuration
//! names ([`crate::detect`]), with what the configuration hides filled from the keychain once the
//! user entered it, and the ones the user added by hand.
//!
//! Only non-secret fields are written to `<data dir>/db-connections.json` (`0600`); passwords go to
//! the operating system's credential store (Keychain, Credential Manager, Secret Service) under a
//! service scoped to the data folder, so a development build never touches the app's.

use crate::detect::{detect, Detected};
use crate::model::{ConnectionConfig, Engine, Missing, Secret};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

const KEYCHAIN_SERVICE: &str = "run.agentty.database";

fn keychain_service() -> String {
    agentty_bridge::connectors::scoped_service(KEYCHAIN_SERVICE, std::env::var_os("AGENTTY_DATA_DIR").as_deref())
}

/// Where a connection comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "file")]
pub enum Source {
    /// The project's configuration (`.env`, `application.yml`, …).
    Detected(String),
    /// Added by the user on the database page.
    Manual,
}

/// One connection of a project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connection {
    /// Stable across restarts (a hash of project and address for detected ones).
    pub id: String,
    pub name: String,
    pub source: Source,
    pub config: ConnectionConfig,
    /// What the user still has to enter before it can connect.
    pub missing: Vec<Missing>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct Saved {
    id: String,
    project: PathBuf,
    name: String,
    engine: Engine,
    host: String,
    port: u16,
    #[serde(default)]
    database: Option<String>,
    #[serde(default)]
    user: Option<String>,
    #[serde(default)]
    tls_off: bool,
}

fn saved_path() -> PathBuf {
    agentty_bridge::fsutil::data_dir().join("db-connections.json")
}

fn load_saved() -> Vec<Saved> {
    std::fs::read(saved_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn write_saved(saved: &[Saved]) -> Result<()> {
    let path = saved_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    {
        use std::io::Write;
        options.open(&tmp)?.write_all(&serde_json::to_vec_pretty(saved)?)?;
    }
    std::fs::rename(tmp, path)?;
    Ok(())
}

/// A stable id for a detected connection: the project and the address, hashed.
fn detected_id(project: &Path, config: &ConnectionConfig) -> String {
    let key = format!(
        "{}|{:?}|{}|{}|{}|{}",
        project.display(),
        config.engine,
        config.host,
        config.port,
        config.database.as_deref().unwrap_or(""),
        config.user.as_deref().unwrap_or("")
    );
    let digest = Sha256::digest(key.as_bytes());
    format!("d-{}", digest.iter().take(8).map(|b| format!("{b:02x}")).collect::<String>())
}

fn stored_password(id: &str) -> Option<Secret> {
    agentty_bridge::secret_store::load(&keychain_service(), id).ok().filter(|p| !p.is_empty()).map(Secret::new)
}

/// Every connection of the project at `project`: detected first, then the ones added by hand.
pub fn connections(project: &Path) -> Vec<Connection> {
    let mut out: Vec<Connection> = detect(project).into_iter().map(|d| from_detected(project, d)).collect();
    for saved in load_saved().into_iter().filter(|s| s.project == project) {
        let mut options = Vec::new();
        if saved.tls_off {
            options.push(("sslmode".to_string(), "disable".to_string()));
        }
        let config = ConnectionConfig {
            engine: saved.engine,
            host: saved.host,
            port: saved.port,
            database: saved.database,
            user: saved.user,
            password: stored_password(&saved.id),
            srv: false,
            options,
        };
        let missing = if config.password.is_none() { vec![Missing::Password] } else { Vec::new() };
        out.push(Connection { id: saved.id, name: saved.name, source: Source::Manual, config, missing });
    }
    unique_names(&mut out);
    out
}

/// Two connections named alike (`shop` in MySQL and in PostgreSQL) get the engine, then a number,
/// so `--conn <name>` always means one of them.
fn unique_names(connections: &mut [Connection]) {
    let count = |list: &[Connection], name: &str| list.iter().filter(|c| c.name.eq_ignore_ascii_case(name)).count();
    let alike: Vec<bool> = connections.iter().map(|c| count(connections, &c.name) > 1).collect();
    for (connection, alike) in connections.iter_mut().zip(alike) {
        if alike {
            connection.name = format!("{} ({})", connection.name, connection.config.engine.label());
        }
    }
    let alike: Vec<bool> = connections.iter().map(|c| count(connections, &c.name) > 1).collect();
    for (i, (connection, alike)) in connections.iter_mut().zip(alike).enumerate() {
        if alike {
            connection.name = format!("{} #{}", connection.name, i + 1);
        }
    }
}

fn from_detected(project: &Path, detected: Detected) -> Connection {
    let Detected { source, mut config, mut missing } = detected;
    let id = detected_id(project, &config);
    if missing.contains(&Missing::Password) {
        if let Some(password) = stored_password(&id) {
            config.password = Some(password);
            missing.retain(|m| *m != Missing::Password);
        }
    }
    let name = config.database.clone().unwrap_or_else(|| config.engine.label().to_string());
    Connection { id, name, source: Source::Detected(source), config, missing }
}

/// Keeps the password the user entered for connection `id` (detected or added by hand).
pub fn save_password(id: &str, password: &str) -> Result<()> {
    agentty_bridge::secret_store::store(&keychain_service(), id, password).context("could not save the password")
}

/// Adds a connection by hand; its password (if any) goes to the credential store.
pub fn add_manual(project: &Path, name: &str, config: &ConnectionConfig, password: Option<&str>, tls_off: bool) -> Result<String> {
    let mut saved = load_saved();
    let id = format!("m-{}", uuid_like());
    saved.push(Saved {
        id: id.clone(),
        project: project.to_path_buf(),
        name: name.to_string(),
        engine: config.engine,
        host: config.host.clone(),
        port: config.port,
        database: config.database.clone(),
        user: config.user.clone(),
        tls_off,
    });
    write_saved(&saved)?;
    if let Some(password) = password.filter(|p| !p.is_empty()) {
        save_password(&id, password)?;
    }
    Ok(id)
}

/// Removes a connection added by hand and its password.
pub fn remove_manual(id: &str) -> Result<()> {
    let mut saved = load_saved();
    saved.retain(|s| s.id != id);
    write_saved(&saved)?;
    let _ = agentty_bridge::secret_store::delete(&keychain_service(), id);
    Ok(())
}

fn uuid_like() -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let digest = Sha256::digest(format!("{nanos}-{}", std::process::id()).as_bytes());
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// Finds a connection by id, name or 1-based position (as `agentty db` takes it).
pub fn find<'a>(connections: &'a [Connection], wanted: Option<&str>) -> Option<&'a Connection> {
    match wanted {
        None if connections.len() == 1 => connections.first(),
        None => None,
        Some(w) => connections
            .iter()
            .find(|c| c.id == w || c.name.eq_ignore_ascii_case(w))
            .or_else(|| w.parse::<usize>().ok().and_then(|n| connections.get(n.wrapping_sub(1)))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detected_ids_are_stable_and_distinct() {
        let config = crate::detect::parse_url("mysql://app@localhost:3306/shop").unwrap();
        let a = detected_id(Path::new("/work/a"), &config);
        assert_eq!(a, detected_id(Path::new("/work/a"), &config));
        assert_ne!(a, detected_id(Path::new("/work/b"), &config));
        assert!(a.starts_with("d-") && a.len() == 18);
    }

    #[test]
    fn alike_names_become_distinct() {
        let mysql = crate::detect::parse_url("mysql://app@localhost:3306/shop").unwrap();
        let pg = crate::detect::parse_url("postgres://app@localhost:5432/shop").unwrap();
        let one = Connection { id: "a".into(), name: "shop".into(), source: Source::Manual, config: mysql.clone(), missing: Vec::new() };
        let mut list = vec![
            one.clone(),
            Connection { id: "b".into(), config: pg, ..one.clone() },
            Connection { id: "c".into(), ..one.clone() },
            Connection { id: "d".into(), name: "logs".into(), ..one },
        ];
        unique_names(&mut list);
        let names: Vec<&str> = list.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["shop (MySQL) #1", "shop (PostgreSQL)", "shop (MySQL) #3", "logs"]);
    }

    #[test]
    fn finds_by_name_or_position() {
        let config = crate::detect::parse_url("mysql://app@localhost:3306/shop").unwrap();
        let one = Connection { id: "d-1".into(), name: "shop".into(), source: Source::Manual, config: config.clone(), missing: Vec::new() };
        let two = Connection { id: "m-2".into(), name: "logs".into(), ..one.clone() };
        let list = vec![one, two];
        assert_eq!(find(&list, Some("LOGS")).map(|c| c.id.as_str()), Some("m-2"));
        assert_eq!(find(&list, Some("1")).map(|c| c.id.as_str()), Some("d-1"));
        assert!(find(&list, None).is_none(), "several: say which");
        assert!(find(&list[..1], None).is_some(), "just one: that one");
        assert!(find(&list, Some("9")).is_none());
    }
}
