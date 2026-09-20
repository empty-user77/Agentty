//! Talking to the databases: one [`Session`] per connection, the same calls for every engine.
//!
//! Reads run inside a read-only transaction (`START TRANSACTION READ ONLY` on MySQL / MariaDB,
//! `BEGIN READ ONLY` on PostgreSQL, `SET TRANSACTION READ ONLY` on Oracle) and are rolled back, so
//! the database refuses a write even if one slipped past [`crate::guard`]. MongoDB reads only ever
//! call read operations. Writes run only through [`Session::write`] / [`Session::mongo_write`],
//! which the app calls after the user approved the exact statement.
//!
//! Results are capped (rows and cell length) so a large table never floods the page or an agent.

use crate::guard::MongoOp;
use crate::model::{ConnectionConfig, Engine};
use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use serde_json::Value;
use std::time::Duration;

/// Longest cell text kept (the rest is cut with `…`).
const MAX_CELL: usize = 4_000;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const QUERY_TIMEOUT: Duration = Duration::from_secs(60);

/// Amazon RDS / Aurora root certificates (public, from truststore.pki.rds.amazonaws.com).
const RDS_BUNDLE: &[u8] = include_bytes!("../certs/rds-global-bundle.crt");

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    /// More rows existed than were returned.
    pub truncated: bool,
    /// Rows a write changed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub affected: Option<u64>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TableInfo {
    pub name: String,
    /// `table`, `view` or `collection`.
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
}

/// How to use TLS for a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tls {
    Off,
    Required,
}

/// TLS for `config`: off on this machine or when the configuration turns it off, required otherwise.
pub fn tls_for(config: &ConnectionConfig) -> Tls {
    let off = config.options.iter().any(|(k, v)| {
        let (k, v) = (k.to_ascii_lowercase(), v.to_ascii_lowercase());
        (k == "sslmode" && v == "disable")
            || (k == "ssl-mode" && v == "disabled")
            || (k == "sslmode" && v == "disabled")
            || ((k == "usessl" || k == "ssl" || k == "tls") && v == "false")
    });
    if config.is_local() || off {
        Tls::Off
    } else {
        Tls::Required
    }
}

/// An open connection.
pub enum Session {
    MySql(Box<mysql::Conn>),
    Postgres(Box<postgres::Client>),
    Oracle(Box<oracle::Connection>),
    Mongo { client: mongodb::sync::Client, database: Option<String> },
}

impl Session {
    /// Connects with `config` (its password must be filled in).
    pub fn connect(config: &ConnectionConfig) -> Result<Session> {
        match config.engine {
            Engine::MySql | Engine::MariaDb => connect_mysql(config),
            Engine::Postgres => connect_postgres(config),
            Engine::Oracle => connect_oracle(config),
            Engine::MongoDb => connect_mongo(config),
        }
    }

    /// Tables and views (collections for MongoDB) of the connection's database.
    pub fn tables(&mut self) -> Result<Vec<TableInfo>> {
        let rows = match self {
            Session::MySql(_) => self.read_rows(
                "SELECT TABLE_NAME, TABLE_TYPE FROM information_schema.TABLES WHERE TABLE_SCHEMA = DATABASE() ORDER BY TABLE_NAME",
                5_000,
            )?,
            Session::Postgres(_) => self.read_rows(
                "SELECT table_schema || '.' || table_name, table_type FROM information_schema.tables \
                 WHERE table_schema NOT IN ('pg_catalog', 'information_schema') ORDER BY 1",
                5_000,
            )?,
            Session::Oracle(_) => self.read_rows(
                "SELECT table_name, 'TABLE' FROM user_tables UNION ALL SELECT view_name, 'VIEW' FROM user_views ORDER BY 1",
                5_000,
            )?,
            Session::Mongo { client, database } => {
                let db = client.database(database.as_deref().ok_or_else(|| anyhow!("no database in this connection"))?);
                let mut names = db.list_collection_names().run()?;
                names.sort();
                return Ok(names.into_iter().map(|name| TableInfo { name, kind: "collection".into() }).collect());
            }
        };
        Ok(rows
            .rows
            .into_iter()
            .filter_map(|row| {
                let name = row.first()?.as_str()?.to_string();
                let kind =
                    if row.get(1).and_then(Value::as_str).unwrap_or("").to_ascii_uppercase().contains("VIEW") { "view" } else { "table" };
                Some(TableInfo { name, kind: kind.into() })
            })
            .collect())
    }

    /// Columns of `table` (name, type, nullable).
    pub fn columns(&mut self, table: &str) -> Result<Vec<ColumnInfo>> {
        check_name(table)?;
        let sql = match self {
            Session::MySql(_) => format!(
                "SELECT COLUMN_NAME, COLUMN_TYPE, IS_NULLABLE FROM information_schema.COLUMNS WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = '{table}' ORDER BY ORDINAL_POSITION"
            ),
            Session::Postgres(_) => {
                let (schema, name) = table.split_once('.').unwrap_or(("public", table));
                format!(
                    "SELECT column_name, data_type, is_nullable FROM information_schema.columns WHERE table_schema = '{schema}' AND table_name = '{name}' ORDER BY ordinal_position"
                )
            }
            Session::Oracle(_) => format!(
                "SELECT column_name, data_type, nullable FROM user_tab_columns WHERE table_name = '{table}' ORDER BY column_id"
            ),
            Session::Mongo { .. } => {
                // No schema: the fields of a sample document.
                let sample = self.mongo_read(MongoOp::Find, table, &serde_json::json!({}), 1)?;
                return Ok(sample.columns.into_iter().map(|name| ColumnInfo { name, data_type: String::new(), nullable: true }).collect());
            }
        };
        let rows = self.read_rows(&sql, 5_000)?;
        Ok(rows
            .rows
            .into_iter()
            .filter_map(|row| {
                Some(ColumnInfo {
                    name: row.first()?.as_str()?.to_string(),
                    data_type: row.get(1).and_then(Value::as_str).unwrap_or("").to_string(),
                    nullable: matches!(row.get(2).and_then(Value::as_str), Some("YES" | "Y")),
                })
            })
            .collect())
    }

    /// The first `limit` rows (documents) of a table (collection).
    pub fn preview(&mut self, table: &str, limit: usize) -> Result<QueryResult> {
        check_name(table)?;
        match self {
            Session::Mongo { .. } => self.mongo_read(MongoOp::Find, table, &serde_json::json!({}), limit),
            Session::MySql(_) => self.read(&format!("SELECT * FROM `{}`", table.replace('`', "")), limit),
            Session::Postgres(_) => {
                let quoted = table.split('.').map(|p| format!("\"{}\"", p.replace('"', ""))).collect::<Vec<_>>().join(".");
                self.read(&format!("SELECT * FROM {quoted}"), limit)
            }
            Session::Oracle(_) => self.read(&format!("SELECT * FROM \"{}\"", table.replace('"', "")), limit),
        }
    }

    /// Runs a read (checked by the caller with [`crate::guard::classify_sql`]) in a read-only
    /// transaction, keeping at most `limit` rows.
    pub fn read(&mut self, sql: &str, limit: usize) -> Result<QueryResult> {
        self.read_rows(sql, limit)
    }

    fn read_rows(&mut self, sql: &str, limit: usize) -> Result<QueryResult> {
        let sql = sql.trim().trim_end_matches(';');
        match self {
            Session::MySql(conn) => {
                use mysql::prelude::Queryable;
                conn.query_drop("START TRANSACTION READ ONLY")?;
                let result = mysql_rows(conn, sql, limit, true);
                let _ = conn.query_drop("ROLLBACK");
                result
            }
            Session::Postgres(client) => {
                let mut tx = client.build_transaction().read_only(true).start()?;
                // Parse first: the extended protocol refuses a string holding several statements,
                // so nothing the guard never classified can ride along with the read below.
                let result = tx.prepare(sql).map_err(anyhow::Error::from).and_then(|_| postgres_rows(&mut tx, sql, limit));
                let _ = tx.rollback();
                result
            }
            Session::Oracle(conn) => {
                conn.execute("SET TRANSACTION READ ONLY", &[])?;
                let result = oracle_rows(conn, sql, limit);
                let _ = conn.rollback();
                result
            }
            Session::Mongo { .. } => bail!("MongoDB takes operations, not SQL"),
        }
    }

    /// Runs a statement the user approved: one that returns rows (a `SELECT` calling a function the
    /// guard doesn't know) shows them; anything else runs as [`Session::write`]. Committed either way.
    pub fn approved(&mut self, engine: Engine, sql: &str, limit: usize) -> Result<QueryResult> {
        if !crate::guard::returns_rows(engine, sql) {
            return self.write(sql);
        }
        let sql = sql.trim().trim_end_matches(';');
        match self {
            // The user approved this exact text, so it runs as written (batches included).
            Session::MySql(conn) => mysql_rows(conn, sql, limit, false),
            Session::Postgres(client) => {
                let mut tx = client.transaction()?;
                let result = postgres_rows(&mut tx, sql, limit)?;
                tx.commit()?;
                Ok(result)
            }
            Session::Oracle(conn) => {
                let result = oracle_rows(conn, sql, limit)?;
                conn.commit()?;
                Ok(result)
            }
            Session::Mongo { .. } => bail!("MongoDB takes operations, not SQL"),
        }
    }

    /// Runs a statement the user approved (a write or DDL), committing it.
    pub fn write(&mut self, sql: &str) -> Result<QueryResult> {
        let sql = sql.trim().trim_end_matches(';');
        match self {
            Session::MySql(conn) => {
                use mysql::prelude::Queryable;
                let result = conn.query_iter(sql)?;
                let affected = result.affected_rows();
                drop(result);
                Ok(QueryResult { affected: Some(affected), ..Default::default() })
            }
            Session::Postgres(client) => {
                let affected = client.execute(sql, &[])?;
                Ok(QueryResult { affected: Some(affected), ..Default::default() })
            }
            Session::Oracle(conn) => {
                let statement = conn.execute(sql, &[])?;
                let affected = statement.row_count()?;
                conn.commit()?;
                Ok(QueryResult { affected: Some(affected), ..Default::default() })
            }
            Session::Mongo { .. } => bail!("MongoDB takes operations, not SQL"),
        }
    }

    /// A MongoDB read: `find` (args = filter), `aggregate` (args = pipeline), `count` (filter),
    /// `distinct` (`{"field": …, "filter": …}`), `collections`.
    pub fn mongo_read(&mut self, op: MongoOp, collection: &str, args: &Value, limit: usize) -> Result<QueryResult> {
        let Session::Mongo { client, database } = self else { bail!("not a MongoDB connection") };
        let db = client.database(database.as_deref().ok_or_else(|| anyhow!("no database in this connection"))?);
        let coll = db.collection::<mongodb::bson::Document>(collection);
        let documents: Vec<mongodb::bson::Document> = match op {
            MongoOp::Find => {
                let filter = to_document(args)?;
                coll.find(filter).limit(limit as i64 + 1).run()?.collect::<Result<_, _>>()?
            }
            MongoOp::Aggregate => {
                let pipeline = args
                    .as_array()
                    .ok_or_else(|| anyhow!("a pipeline is a JSON array"))?
                    .iter()
                    .map(to_document)
                    .collect::<Result<Vec<_>>>()?;
                coll.aggregate(pipeline).run()?.take(limit + 1).collect::<Result<_, _>>()?
            }
            MongoOp::Count => {
                let count = coll.count_documents(to_document(args)?).run()?;
                return Ok(QueryResult { columns: vec!["count".into()], rows: vec![vec![Value::from(count)]], ..Default::default() });
            }
            MongoOp::Distinct => {
                let field = args["field"].as_str().ok_or_else(|| anyhow!("distinct needs {{\"field\": …}}"))?;
                let values = coll.distinct(field, to_document(&args["filter"])?).run()?;
                let rows: Vec<Vec<Value>> = values.into_iter().take(limit).map(|v| vec![v.into_relaxed_extjson()]).collect();
                return Ok(QueryResult { columns: vec![field.to_string()], rows, ..Default::default() });
            }
            MongoOp::ListCollections => {
                let names = db.list_collection_names().run()?;
                return Ok(QueryResult {
                    columns: vec!["collection".into()],
                    rows: names.into_iter().map(|n| vec![Value::from(n)]).collect(),
                    ..Default::default()
                });
            }
            _ => bail!("not a read"),
        };
        Ok(documents_to_result(documents, limit))
    }

    /// A MongoDB write the user approved: `insertOne` (args = document), `insertMany` (array),
    /// `updateOne` / `updateMany` (`{"filter", "update"}`), `replaceOne` (`{"filter", "replacement"}`),
    /// `deleteOne` / `deleteMany` (filter), `createIndex` (keys), `dropIndex` (name), `drop`.
    pub fn mongo_write(&mut self, op: MongoOp, collection: &str, args: &Value) -> Result<QueryResult> {
        let Session::Mongo { client, database } = self else { bail!("not a MongoDB connection") };
        let db = client.database(database.as_deref().ok_or_else(|| anyhow!("no database in this connection"))?);
        let coll = db.collection::<mongodb::bson::Document>(collection);
        let affected: u64 = match op {
            MongoOp::InsertOne => {
                coll.insert_one(to_document(args)?).run()?;
                1
            }
            MongoOp::InsertMany => {
                let docs = args
                    .as_array()
                    .ok_or_else(|| anyhow!("insertMany takes a JSON array"))?
                    .iter()
                    .map(to_document)
                    .collect::<Result<Vec<_>>>()?;
                coll.insert_many(docs).run()?.inserted_ids.len() as u64
            }
            MongoOp::UpdateOne => coll.update_one(to_document(&args["filter"])?, to_document(&args["update"])?).run()?.modified_count,
            MongoOp::UpdateMany => coll.update_many(to_document(&args["filter"])?, to_document(&args["update"])?).run()?.modified_count,
            MongoOp::ReplaceOne => {
                coll.replace_one(to_document(&args["filter"])?, to_document(&args["replacement"])?).run()?.modified_count
            }
            MongoOp::DeleteOne => coll.delete_one(to_document(args)?).run()?.deleted_count,
            MongoOp::DeleteMany => coll.delete_many(to_document(args)?).run()?.deleted_count,
            MongoOp::CreateIndex => {
                let model = mongodb::IndexModel::builder().keys(to_document(args)?).build();
                coll.create_index(model).run()?;
                0
            }
            MongoOp::DropIndex => {
                coll.drop_index(args.as_str().ok_or_else(|| anyhow!("dropIndex takes the index name"))?).run()?;
                0
            }
            MongoOp::DropCollection => {
                coll.drop().run()?;
                0
            }
            _ => bail!("not a write"),
        };
        Ok(QueryResult { affected: Some(affected), ..Default::default() })
    }
}

/// Table names come from listings or the user: letters, digits, `_`, `$`, `#`, `.`, `-` and spaces only
/// (they are quoted, but never trusted with quotes of their own).
fn check_name(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > 200 || !name.chars().all(|c| c.is_alphanumeric() || "_$#.- ".contains(c)) {
        bail!("not a table name: {name}");
    }
    Ok(())
}

fn cell(text: String) -> Value {
    if text.chars().count() > MAX_CELL {
        Value::String(format!("{}…", text.chars().take(MAX_CELL).collect::<String>()))
    } else {
        Value::String(text)
    }
}

// -- MySQL / MariaDB -----------------------------------------------------------------------------

fn connect_mysql(config: &ConnectionConfig) -> Result<Session> {
    let password = config.password.as_ref().map(|p| p.expose().to_string());
    let mut opts = mysql::OptsBuilder::new()
        .ip_or_hostname(Some(config.host.clone()))
        .tcp_port(config.port)
        .user(config.user.clone())
        .pass(password)
        .db_name(config.database.clone())
        .tcp_connect_timeout(Some(CONNECT_TIMEOUT))
        .read_timeout(Some(QUERY_TIMEOUT))
        .write_timeout(Some(QUERY_TIMEOUT));
    if tls_for(config) == Tls::Required {
        let mut ssl = mysql::SslOpts::default();
        if config.is_rds() {
            ssl = ssl.with_root_cert_path(Some(rds_bundle_file()?));
        }
        opts = opts.ssl_opts(Some(ssl));
    }
    let conn = mysql::Conn::new(opts).with_context(|| format!("could not connect to {}", config.summary()))?;
    Ok(Session::MySql(Box::new(conn)))
}

/// The RDS bundle as a file (the MySQL driver takes a path), in Agentty's own data folder — never a
/// shared temp folder, where another account could put its own certificate first.
fn rds_bundle_file() -> Result<std::path::PathBuf> {
    let path = agentty_bridge::fsutil::data_dir().join("certs").join("rds-global-bundle.pem");
    if std::fs::read(&path).ok().as_deref() != Some(RDS_BUNDLE) {
        std::fs::create_dir_all(path.parent().expect("has a parent"))?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, RDS_BUNDLE)?;
        std::fs::rename(tmp, &path)?;
    }
    Ok(path)
}

fn mysql_value(value: mysql::Value) -> Value {
    use mysql::Value as V;
    match value {
        V::NULL => Value::Null,
        V::Bytes(bytes) => cell(String::from_utf8_lossy(&bytes).into_owned()),
        V::Int(n) => Value::from(n),
        V::UInt(n) => Value::from(n),
        V::Float(f) => serde_json::Number::from_f64(f as f64).map(Value::Number).unwrap_or(Value::Null),
        V::Double(f) => serde_json::Number::from_f64(f).map(Value::Number).unwrap_or(Value::Null),
        V::Date(y, m, d, h, mi, s, us) => Value::String(if h == 0 && mi == 0 && s == 0 && us == 0 {
            format!("{y:04}-{m:02}-{d:02}")
        } else {
            format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02}:{s:02}")
        }),
        V::Time(neg, days, h, mi, s, _) => Value::String(format!("{}{}:{mi:02}:{s:02}", if neg { "-" } else { "" }, days * 24 + h as u32)),
    }
}

/// `prepared` sends the statement over the binary protocol, which runs exactly one statement. Reads
/// use it: the text protocol takes a batch (the driver always negotiates `CLIENT_MULTI_STATEMENTS`
/// and cannot turn it off), so a statement the guard never classified could ride along with a read.
fn mysql_rows(conn: &mut mysql::Conn, sql: &str, limit: usize, prepared: bool) -> Result<QueryResult> {
    use mysql::prelude::Queryable;
    if prepared {
        mysql_collect(conn.exec_iter(sql, ())?, limit)
    } else {
        mysql_collect(conn.query_iter(sql)?, limit)
    }
}

fn mysql_collect<P: mysql::prelude::Protocol>(mut result: mysql::QueryResult<'_, '_, '_, P>, limit: usize) -> Result<QueryResult> {
    let mut out = QueryResult::default();
    if let Some(set) = result.iter() {
        out.columns = set.columns().as_ref().iter().map(|c| c.name_str().to_string()).collect();
        for row in set {
            if out.rows.len() == limit {
                out.truncated = true;
                break;
            }
            let row = row?;
            out.rows.push(row.unwrap().into_iter().map(mysql_value).collect());
        }
    }
    Ok(out)
}

// -- PostgreSQL ----------------------------------------------------------------------------------

fn rustls_config(config: &ConnectionConfig) -> Result<rustls::ClientConfig> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    if config.is_rds() {
        for cert in rustls_pemfile::certs(&mut &RDS_BUNDLE[..]).flatten() {
            let _ = roots.add(cert);
        }
    }
    Ok(rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots)
        .with_no_client_auth())
}

fn connect_postgres(config: &ConnectionConfig) -> Result<Session> {
    let mut pg = postgres::Config::new();
    pg.host(&config.host).port(config.port).connect_timeout(CONNECT_TIMEOUT);
    if let Some(user) = &config.user {
        pg.user(user);
    }
    if let Some(password) = &config.password {
        pg.password(password.expose());
    }
    if let Some(database) = &config.database {
        pg.dbname(database);
    }
    let client = match tls_for(config) {
        Tls::Off => pg.connect(postgres::NoTls),
        Tls::Required => {
            pg.ssl_mode(postgres::config::SslMode::Require);
            pg.connect(tokio_postgres_rustls::MakeRustlsConnect::new(rustls_config(config)?))
        }
    }
    .with_context(|| format!("could not connect to {}", config.summary()))?;
    Ok(Session::Postgres(Box::new(client)))
}

fn postgres_rows(tx: &mut postgres::Transaction<'_>, sql: &str, limit: usize) -> Result<QueryResult> {
    let mut out = QueryResult::default();
    for message in tx.simple_query(sql)? {
        match message {
            postgres::SimpleQueryMessage::RowDescription(columns) if out.columns.is_empty() => {
                out.columns = columns.iter().map(|c| c.name().to_string()).collect();
            }
            postgres::SimpleQueryMessage::Row(row) => {
                if out.columns.is_empty() {
                    out.columns = row.columns().iter().map(|c| c.name().to_string()).collect();
                }
                if out.rows.len() == limit {
                    out.truncated = true;
                    continue;
                }
                out.rows.push((0..row.len()).map(|i| row.get(i).map(|t| cell(t.to_string())).unwrap_or(Value::Null)).collect());
            }
            _ => {}
        }
    }
    Ok(out)
}

// -- Oracle --------------------------------------------------------------------------------------

fn connect_oracle(config: &ConnectionConfig) -> Result<Session> {
    let user = config.user.as_deref().ok_or_else(|| anyhow!("Oracle needs a user"))?;
    let password = config.password.as_ref().map(|p| p.expose()).unwrap_or("");
    let service = config.database.as_deref().unwrap_or("");
    let address = format!("//{}:{}/{service}", config.host, config.port);
    let conn = oracle::Connection::connect(user, password, &address).map_err(|err| {
        let text = err.to_string();
        if text.contains("DPI-1047") || text.to_ascii_lowercase().contains("cannot locate") {
            anyhow!("Oracle Instant Client is not installed (https://www.oracle.com/database/technologies/instant-client.html)")
        } else {
            anyhow!("could not connect to {}: {text}", config.summary())
        }
    })?;
    conn.set_call_timeout(Some(QUERY_TIMEOUT))?;
    Ok(Session::Oracle(Box::new(conn)))
}

fn oracle_rows(conn: &oracle::Connection, sql: &str, limit: usize) -> Result<QueryResult> {
    let rows = conn.query(sql, &[])?;
    let mut out = QueryResult { columns: rows.column_info().iter().map(|c| c.name().to_string()).collect(), ..Default::default() };
    for row in rows {
        if out.rows.len() == limit {
            out.truncated = true;
            break;
        }
        let row = row?;
        out.rows.push(
            (0..out.columns.len()).map(|i| row.get::<usize, Option<String>>(i).ok().flatten().map(cell).unwrap_or(Value::Null)).collect(),
        );
    }
    Ok(out)
}

// -- MongoDB -------------------------------------------------------------------------------------

fn connect_mongo(config: &ConnectionConfig) -> Result<Session> {
    use mongodb::options::{ClientOptions, Credential};
    // The URI carries no credential: it is set on the options, so it never appears in a string.
    let scheme = if config.srv { "mongodb+srv" } else { "mongodb" };
    let place = if config.srv { config.host.clone() } else { format!("{}:{}", config.host, config.port) };
    let query: Vec<String> = config.options.iter().map(|(k, v)| format!("{k}={v}")).collect();
    let uri = format!(
        "{scheme}://{place}/{}{}",
        config.database.as_deref().unwrap_or(""),
        if query.is_empty() { String::new() } else { format!("?{}", query.join("&")) }
    );
    let mut options = ClientOptions::parse(&uri).run().context("MongoDB address")?;
    options.connect_timeout = Some(CONNECT_TIMEOUT);
    options.server_selection_timeout = Some(CONNECT_TIMEOUT);
    if let Some(user) = &config.user {
        let source = config.options.iter().find(|(k, _)| k.eq_ignore_ascii_case("authSource")).map(|(_, v)| v.clone());
        options.credential = Some(
            Credential::builder()
                .username(user.clone())
                .password(config.password.as_ref().map(|p| p.expose().to_string()))
                .source(source.or_else(|| Some("admin".into())))
                .build(),
        );
    }
    let client = mongodb::sync::Client::with_options(options)?;
    // Connect now, so a wrong password shows here and not on the first query.
    client
        .database("admin")
        .run_command(mongodb::bson::doc! { "ping": 1 })
        .run()
        .with_context(|| format!("could not connect to {}", config.summary()))?;
    Ok(Session::Mongo { client, database: config.database.clone() })
}

fn to_document(value: &Value) -> Result<mongodb::bson::Document> {
    if value.is_null() {
        return Ok(mongodb::bson::Document::new());
    }
    let bson = mongodb::bson::Bson::try_from(value.clone())?;
    match bson {
        mongodb::bson::Bson::Document(doc) => Ok(doc),
        _ => bail!("expected a JSON object"),
    }
}

fn documents_to_result(documents: Vec<mongodb::bson::Document>, limit: usize) -> QueryResult {
    let truncated = documents.len() > limit;
    let documents: Vec<_> = documents.into_iter().take(limit).collect();
    let mut columns: Vec<String> = Vec::new();
    for doc in &documents {
        for key in doc.keys() {
            if !columns.contains(key) {
                columns.push(key.clone());
            }
        }
    }
    let rows = documents
        .into_iter()
        .map(|mut doc| {
            columns
                .iter()
                .map(|c| match doc.remove(c) {
                    Some(value) => match value.into_relaxed_extjson() {
                        Value::String(s) => cell(s),
                        other => {
                            let text = other.to_string();
                            if text.len() > MAX_CELL {
                                cell(text)
                            } else {
                                other
                            }
                        }
                    },
                    None => Value::Null,
                })
                .collect()
        })
        .collect();
    QueryResult { columns, rows, truncated, affected: None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Secret;

    fn config(host: &str, options: &[(&str, &str)]) -> ConnectionConfig {
        ConnectionConfig {
            engine: Engine::MySql,
            host: host.into(),
            port: 3306,
            database: None,
            user: None,
            password: Some(Secret::new("example-not-a-real-pass")),
            srv: false,
            options: options.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        }
    }

    #[test]
    fn tls_is_required_away_from_this_machine() {
        assert_eq!(tls_for(&config("127.0.0.1", &[])), Tls::Off);
        assert_eq!(tls_for(&config("db.example.com", &[])), Tls::Required);
        assert_eq!(tls_for(&config("db.example.com", &[("sslmode", "disable")])), Tls::Off);
        assert_eq!(tls_for(&config("db.example.com", &[("useSSL", "false")])), Tls::Off);
        assert_eq!(tls_for(&config("x.abc.rds.amazonaws.com", &[])), Tls::Required);
    }

    #[test]
    fn names_are_checked_and_cells_capped() {
        assert!(check_name("users").is_ok() && check_name("public.orders").is_ok());
        assert!(check_name("users`; DROP TABLE x").is_err());
        assert!(check_name("a\"b").is_err() && check_name("").is_err());
        let long = "x".repeat(MAX_CELL + 10);
        assert_eq!(cell(long).as_str().unwrap().chars().count(), MAX_CELL + 1);
    }

    #[test]
    fn rds_bundle_has_certificates() {
        assert!(rustls_pemfile::certs(&mut &RDS_BUNDLE[..]).flatten().count() > 50);
    }

    #[test]
    fn documents_become_rows() {
        let docs = vec![mongodb::bson::doc! { "_id": 1, "name": "a" }, mongodb::bson::doc! { "_id": 2, "tags": ["x"] }];
        let result = documents_to_result(docs, 1);
        assert_eq!(result.columns, vec!["_id", "name"]);
        assert!(result.truncated && result.rows.len() == 1);
    }
}
