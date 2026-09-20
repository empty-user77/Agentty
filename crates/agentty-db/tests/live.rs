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

const PASSWORD: &str = "example-not-a-real-pass";

fn live() -> bool {
    std::env::var("AGENTTY_DB_LIVE").as_deref() == Ok("1")
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
        sql_engine(Engine::MySql, 13306, "root");
    }
}

#[test]
fn mariadb() {
    if live() {
        sql_engine(Engine::MariaDb, 13307, "root");
    }
}

#[test]
fn postgres() {
    if live() {
        sql_engine(Engine::Postgres, 15432, "postgres");
    }
}

#[test]
fn mongodb() {
    if !live() {
        return;
    }
    let mut session = connect(&config(Engine::MongoDb, 17017, "root", "shop"));
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
    let mut bad = config(Engine::MySql, 13306, "root", "shop");
    bad.password = Some(Secret::new("example-wrong"));
    let error = Session::connect(&bad).err().expect("refused");
    let text = format!("{error:#}");
    assert!(text.contains("could not connect"), "{text}");
    assert!(!text.contains("example-wrong"), "the password never shows in errors");
}
