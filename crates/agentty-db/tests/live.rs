//! Against real databases, when `AGENTTY_DB_LIVE=1` (skipped otherwise). Start them with:
//!
//! ```sh
//! P=example-not-a-real-pass
//! docker run -d --name adb-mysql -e MYSQL_ROOT_PASSWORD=$P -e MYSQL_DATABASE=shop -p 127.0.0.1:13306:3306 mysql:8.4
//! docker run -d --name adb-maria -e MARIADB_ROOT_PASSWORD=$P -e MARIADB_DATABASE=shop -p 127.0.0.1:13307:3306 mariadb:11
//! docker run -d --name adb-pg -e POSTGRES_PASSWORD=$P -e POSTGRES_DB=shop -p 127.0.0.1:15432:5432 postgres:16-alpine
//! docker run -d --name adb-mongo -e MONGO_INITDB_ROOT_USERNAME=root -e MONGO_INITDB_ROOT_PASSWORD=$P -p 127.0.0.1:17017:27017 mongo:7
//! ```

use agentty_db::engine::Session;
use agentty_db::guard::{classify_sql, MongoOp, Verdict};
use agentty_db::model::{ConnectionConfig, Engine, Secret};
use agentty_db::schema::relations;

const PASSWORD: &str = "example-not-a-real-pass";

fn live() -> bool {
    std::env::var("AGENTTY_DB_LIVE").as_deref() == Ok("1")
}

/// The port of a test database: the one above, or `AGENTTY_DB_LIVE_<ENGINE>_PORT` when that one is
/// taken on this machine.
fn port(engine: &str, default: u16) -> u16 {
    std::env::var(format!("AGENTTY_DB_LIVE_{engine}_PORT")).ok().and_then(|p| p.parse().ok()).unwrap_or(default)
}

fn config(engine: Engine, port: u16, user: &str, database: &str) -> ConnectionConfig {
    ConnectionConfig {
        engine,
        host: "127.0.0.1".into(),
        port,
        database: Some(database.into()),
        user: Some(user.into()),
        password: Some(Secret::new(PASSWORD)),
        srv: false,
        options: Vec::new(),
    }
}

/// Connects, retrying while the container starts.
fn connect(config: &ConnectionConfig) -> Session {
    let mut last = None;
    for _ in 0..60 {
        match Session::connect(config) {
            Ok(session) => return session,
            Err(err) => last = Some(err),
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    panic!("{:?} never answered: {:#}", config.engine, last.unwrap());
}

fn sql_engine(engine: Engine, port: u16, user: &str) {
    let mut session = connect(&config(engine, port, user, "shop"));
    let setup = [
        "DROP TABLE IF EXISTS items",
        "CREATE TABLE items (id INT PRIMARY KEY, name VARCHAR(20))",
        "INSERT INTO items VALUES (1, 'apple'), (2, 'pear'), (3, 'plum')",
    ];
    for statement in setup {
        assert!(classify_sql(engine, statement).needs_approval(), "{statement}");
        session.write(statement).unwrap_or_else(|e| panic!("{statement}: {e:#}"));
    }
    let tables = session.tables().unwrap();
    assert!(tables.iter().any(|t| t.name.ends_with("items")), "{tables:?}");
    let columns = session.columns(if engine == Engine::Postgres { "public.items" } else { "items" }).unwrap();
    assert_eq!(columns.iter().map(|c| c.name.to_lowercase()).collect::<Vec<_>>(), vec!["id", "name"]);
    let preview = session.preview("items", 2).unwrap();
    assert!(preview.truncated && preview.rows.len() == 2);
    let read = "SELECT name FROM items WHERE id = 3";
    assert_eq!(classify_sql(engine, read), Verdict::Read);
    let result = session.read(read, 100).unwrap();
    assert_eq!(result.rows[0][0].as_str(), Some("plum"));
    // A batch never reaches the server on the read path, whatever the classifier made of it.
    let batch = "SELECT 1; DELETE FROM items";
    assert!(session.read(batch, 10).is_err(), "{engine:?}: the read path must refuse several statements");
    assert_eq!(session.read("SELECT count(*) AS n FROM items", 10).unwrap().rows.len(), 1);
    // Reads that are not plain SELECTs still work through the single-statement gate.
    assert!(session.read("WITH t AS (SELECT 1 AS a) SELECT * FROM t", 10).is_ok(), "{engine:?}: CTE");
    match engine {
        Engine::Postgres => {
            assert!(session.read("SHOW server_version", 10).is_ok(), "SHOW");
            assert!(session.read("EXPLAIN SELECT 1", 10).is_ok(), "EXPLAIN");
        }
        Engine::MySql | Engine::MariaDb => {
            assert!(session.read("SHOW TABLES", 10).is_ok(), "SHOW TABLES");
            assert!(session.read("DESCRIBE items", 10).is_ok(), "DESCRIBE");
            assert!(session.read("EXPLAIN SELECT 1", 10).is_ok(), "EXPLAIN");
        }
        _ => {}
    }
    // The second line of defense: even called directly, a read can't change anything.
    assert!(session.read("DELETE FROM items", 100).is_err(), "{engine:?}: a read-only transaction refuses writes");
    assert!(session.read("UPDATE items SET name = 'x'", 100).is_err());
    assert_eq!(session.read("SELECT count(*) FROM items", 10).unwrap().rows[0][0].as_str().or(Some("")).map(|_| ()), Some(()));
    let count = session.read("SELECT count(*) AS n FROM items", 10).unwrap();
    let n = &count.rows[0][0];
    assert!(n.as_i64() == Some(3) || n.as_u64() == Some(3) || n.as_str() == Some("3"), "{n:?}");
    // An approved query shows its rows.
    let approved = session.approved(engine, "SELECT name FROM items ORDER BY id", 10).unwrap();
    assert_eq!(approved.rows.len(), 3);
    let changed = session.write("DELETE FROM items WHERE id = 1").unwrap();
    assert_eq!(changed.affected, Some(1));
    session.write("DROP TABLE items").unwrap();
}

#[test]
fn mysql() {
    if live() {
        sql_engine(Engine::MySql, port("MYSQL", 13306), "root");
    }
}

#[test]
fn mariadb() {
    if live() {
        sql_engine(Engine::MariaDb, port("MARIADB", 13307), "root");
    }
}

#[test]
fn postgres() {
    if live() {
        sql_engine(Engine::Postgres, port("POSTGRES", 15432), "postgres");
    }
}

#[test]
fn mongodb() {
    if !live() {
        return;
    }
    let mut session = connect(&config(Engine::MongoDb, port("MONGO", 17017), "root", "shop"));
    let _ = session.mongo_write(MongoOp::DropCollection, "items", &serde_json::Value::Null);
    let docs = serde_json::json!([{ "_id": 1, "name": "apple" }, { "_id": 2, "name": "pear" }, { "_id": 3, "name": "plum" }]);
    assert_eq!(session.mongo_write(MongoOp::InsertMany, "items", &docs).unwrap().affected, Some(3));
    assert!(session.tables().unwrap().iter().any(|t| t.name == "items"));
    let found = session.mongo_read(MongoOp::Find, "items", &serde_json::json!({ "name": "plum" }), 10).unwrap();
    assert_eq!(found.rows.len(), 1);
    let preview = session.preview("items", 2).unwrap();
    assert!(preview.truncated && preview.columns == vec!["_id", "name"]);
    let pipeline = serde_json::json!([{ "$group": { "_id": null, "n": { "$sum": 1 } } }]);
    assert_eq!(session.mongo_read(MongoOp::Aggregate, "items", &pipeline, 10).unwrap().rows.len(), 1);
    assert_eq!(session.mongo_read(MongoOp::Count, "items", &serde_json::json!({}), 10).unwrap().rows[0][0], 3);
    // Reads refuse write operations.
    assert!(session.mongo_read(MongoOp::DeleteMany, "items", &serde_json::json!({}), 10).is_err());
    assert_eq!(session.mongo_write(MongoOp::DeleteMany, "items", &serde_json::json!({ "_id": 1 })).unwrap().affected, Some(1));
    session.mongo_write(MongoOp::DropCollection, "items", &serde_json::Value::Null).unwrap();
}

#[test]
fn wrong_password_says_so() {
    if !live() {
        return;
    }
    let mut bad = config(Engine::MySql, port("MYSQL", 13306), "root", "shop");
    bad.password = Some(Secret::new("example-wrong"));
    let error = Session::connect(&bad).err().expect("refused");
    let text = format!("{error:#}");
    assert!(text.contains("could not connect"), "{text}");
    assert!(!text.contains("example-wrong"), "the password never shows in errors");
}

/// The relationship diagram's reading of a database: comments, primary keys, declared foreign keys,
/// and a reference the database does not declare.
fn sql_schema(engine: Engine, port: u16, user: &str) {
    let mut session = connect(&config(engine, port, user, "shop"));
    let (users, settings, orders) = if engine == Engine::Postgres {
        ("public.erd_users", "public.erd_user_settings", "public.erd_orders")
    } else {
        ("erd_users", "erd_user_settings", "erd_orders")
    };
    for statement in ["DROP TABLE IF EXISTS erd_orders", "DROP TABLE IF EXISTS erd_user_settings", "DROP TABLE IF EXISTS erd_users"] {
        session.write(statement).unwrap();
    }
    let setup: &[&str] = match engine {
        Engine::Postgres => &[
            "CREATE TABLE erd_users (id BIGINT PRIMARY KEY, name VARCHAR(40))",
            "COMMENT ON TABLE erd_users IS 'members'",
            "COMMENT ON COLUMN erd_users.name IS 'display name'",
            "CREATE TABLE erd_user_settings (id BIGINT PRIMARY KEY, erd_user_id BIGINT, theme TEXT)",
            "CREATE TABLE erd_orders (id BIGINT PRIMARY KEY, erd_user_id BIGINT REFERENCES erd_users(id))",
        ],
        _ => &[
            "CREATE TABLE erd_users (id BIGINT PRIMARY KEY, name VARCHAR(40) COMMENT 'display name') COMMENT 'members'",
            "CREATE TABLE erd_user_settings (id BIGINT PRIMARY KEY, erd_user_id BIGINT, theme TEXT)",
            "CREATE TABLE erd_orders (id BIGINT PRIMARY KEY, erd_user_id BIGINT, FOREIGN KEY (erd_user_id) REFERENCES erd_users(id))",
        ],
    };
    for statement in setup {
        session.write(statement).unwrap_or_else(|e| panic!("{statement}: {e:#}"));
    }
    let schema = session.schema().unwrap();
    let user_table = schema.tables.iter().find(|t| t.name == users).unwrap_or_else(|| panic!("{schema:?}"));
    assert_eq!(user_table.comment, "members");
    assert!(user_table.columns[0].primary && user_table.columns[0].name == "id");
    assert_eq!(user_table.columns[1].comment, "display name");
    let found: Vec<(String, String, bool)> =
        relations(&schema).into_iter().filter(|r| r.to_table == users).map(|r| (r.from_table, r.from_column, r.declared)).collect();
    assert!(found.contains(&(orders.to_string(), "erd_user_id".to_string(), true)), "{found:?}");
    assert!(found.contains(&(settings.to_string(), "erd_user_id".to_string(), false)), "{found:?}");
    for statement in ["DROP TABLE erd_orders", "DROP TABLE erd_user_settings", "DROP TABLE erd_users"] {
        session.write(statement).unwrap();
    }
}

#[test]
fn mysql_schema() {
    if live() {
        sql_schema(Engine::MySql, port("MYSQL", 13306), "root");
    }
}

#[test]
fn mariadb_schema() {
    if live() {
        sql_schema(Engine::MariaDb, port("MARIADB", 13307), "root");
    }
}

#[test]
fn postgres_schema() {
    if live() {
        sql_schema(Engine::Postgres, port("POSTGRES", 15432), "postgres");
    }
}

#[test]
fn mongodb_schema() {
    if !live() {
        return;
    }
    let mut session = connect(&config(Engine::MongoDb, port("MONGO", 17017), "root", "shop"));
    for name in ["erd_people", "erd_visits"] {
        let _ = session.mongo_write(MongoOp::DropCollection, name, &serde_json::Value::Null);
    }
    let person = serde_json::json!({ "_id": { "$oid": "65f000000000000000000001" }, "name": "a" });
    session.mongo_write(MongoOp::InsertOne, "erd_people", &person).unwrap();
    let visit = serde_json::json!({ "erdPersonId": { "$oid": "65f000000000000000000001" }, "at": 1 });
    session.mongo_write(MongoOp::InsertOne, "erd_visits", &visit).unwrap();
    let schema = session.schema().unwrap();
    let visits = schema.tables.iter().find(|t| t.name == "erd_visits").unwrap();
    assert_eq!(visits.columns.iter().find(|c| c.name == "erdPersonId").unwrap().data_type, "objectId");
    let found = relations(&schema);
    assert!(found.iter().any(|r| r.from_table == "erd_visits" && r.to_table == "erd_people" && r.to_column == "_id"), "{found:?}");
    for name in ["erd_people", "erd_visits"] {
        session.mongo_write(MongoOp::DropCollection, name, &serde_json::Value::Null).unwrap();
    }
}
