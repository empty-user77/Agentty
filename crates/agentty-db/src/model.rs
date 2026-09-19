//! Connections: which database, where, as whom. Passwords live in [`Secret`], which never prints.

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Engine {
    MySql,
    MariaDb,
    Postgres,
    Oracle,
    MongoDb,
}

impl Engine {
    pub fn label(self) -> &'static str {
        match self {
            Engine::MySql => "MySQL",
            Engine::MariaDb => "MariaDB",
            Engine::Postgres => "PostgreSQL",
            Engine::Oracle => "Oracle",
            Engine::MongoDb => "MongoDB",
        }
    }

    pub fn default_port(self) -> u16 {
        match self {
            Engine::MySql | Engine::MariaDb => 3306,
            Engine::Postgres => 5432,
            Engine::Oracle => 1521,
            Engine::MongoDb => 27017,
        }
    }

    /// From a URL scheme, a JDBC sub-protocol, a Prisma provider or a Laravel driver name.
    pub fn from_name(name: &str) -> Option<Engine> {
        Some(match name.to_ascii_lowercase().as_str() {
            "mysql" | "mysqlx" | "aurora-mysql" => Engine::MySql,
            "mariadb" => Engine::MariaDb,
            "postgres" | "postgresql" | "pgsql" | "aurora-postgresql" => Engine::Postgres,
            "oracle" => Engine::Oracle,
            "mongodb" | "mongodb+srv" | "mongo" => Engine::MongoDb,
            _ => return None,
        })
    }
}

/// A password or other credential. Its `Debug` and `Display` never show the value.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Secret(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

/// Where to connect and how. `password` is `None` when it is still to be entered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionConfig {
    pub engine: Engine,
    pub host: String,
    pub port: u16,
    /// Database (MySQL, PostgreSQL, MongoDB) or service name / SID (Oracle).
    pub database: Option<String>,
    pub user: Option<String>,
    pub password: Option<Secret>,
    /// MongoDB `mongodb+srv://` (DNS seed list): `host` is the SRV name and `port` is unused.
    pub srv: bool,
    /// Extra URL query options kept as given (MongoDB `authSource`, `replicaSet`, …), never secrets.
    pub options: Vec<(String, String)>,
}

impl ConnectionConfig {
    /// Amazon RDS / Aurora endpoints (they require or prefer TLS).
    pub fn is_rds(&self) -> bool {
        let host = self.host.to_ascii_lowercase();
        host.ends_with(".rds.amazonaws.com") || host.ends_with(".rds.amazonaws.com.cn")
    }

    /// A host on this machine (compose services, local installs): TLS is not attempted.
    pub fn is_local(&self) -> bool {
        matches!(self.host.as_str(), "localhost" | "127.0.0.1" | "::1" | "0.0.0.0" | "host.docker.internal")
    }

    /// What the user sees: `MySQL · user@host:3306/shop` — never the password.
    pub fn summary(&self) -> String {
        let user = self.user.as_deref().map(|u| format!("{u}@")).unwrap_or_default();
        let db = self.database.as_deref().map(|d| format!("/{d}")).unwrap_or_default();
        let place = if self.srv { self.host.clone() } else { format!("{}:{}", self.host, self.port) };
        format!("{} · {user}{place}{db}", if self.is_rds() { format!("{} (RDS)", self.engine.label()) } else { self.engine.label().into() })
    }
}

/// A field a connection still needs from the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Missing {
    Host,
    User,
    Password,
    Database,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_never_print() {
        let secret = Secret::new("example-not-a-real-password");
        assert_eq!(format!("{secret:?}"), "Secret(***)");
        let config = ConnectionConfig {
            engine: Engine::MySql,
            host: "db.abc.ap-northeast-2.rds.amazonaws.com".into(),
            port: 3306,
            database: Some("shop".into()),
            user: Some("app".into()),
            password: Some(secret),
            srv: false,
            options: Vec::new(),
        };
        assert!(!format!("{config:?}").contains("example-not-a-real-password"));
        assert!(config.is_rds());
        assert_eq!(config.summary(), "MySQL (RDS) · app@db.abc.ap-northeast-2.rds.amazonaws.com:3306/shop");
    }
}
