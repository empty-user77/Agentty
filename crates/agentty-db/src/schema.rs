//! A database's shape for the relationship diagram: every table with its comment, its columns
//! (type, primary key, comment) and the foreign keys it declares — read in a handful of catalog
//! queries for the whole database, not one per table.
//!
//! Plenty of databases declare no foreign keys at all: the application keeps `user_settings.user_id`
//! pointing at `users.id` on its own. [`relations`] draws those too, from the names and types alone,
//! and marks them as inferred so they never pass for a constraint the database enforces.

use crate::engine::Session;
use crate::guard::MongoOp;
use anyhow::{anyhow, Result};
use serde::Serialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// More tables than this are left out of the diagram (it would be unreadable anyway).
pub const MAX_TABLES: usize = 300;
/// Columns read for the whole database.
const MAX_COLUMNS: usize = 30_000;
/// Documents sampled per MongoDB collection to find its fields.
const MONGO_SAMPLE: usize = 20;

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Schema {
    pub tables: Vec<TableShape>,
    /// Foreign keys the database declares.
    pub foreign_keys: Vec<Relation>,
    /// The database had more than [`MAX_TABLES`] tables; the rest are not here.
    pub truncated: bool,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct TableShape {
    pub name: String,
    /// `table`, `view` or `collection`.
    pub kind: String,
    pub comment: String,
    pub columns: Vec<ColumnShape>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct ColumnShape {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub primary: bool,
    pub comment: String,
}

/// `from_table.from_column` holds values of `to_table.to_column`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, Hash)]
pub struct Relation {
    pub from_table: String,
    pub from_column: String,
    pub to_table: String,
    pub to_column: String,
    /// A foreign key the database declares; otherwise inferred from names and types.
    pub declared: bool,
}

impl Session {
    /// The whole database's tables, columns, comments and declared foreign keys.
    pub fn schema(&mut self) -> Result<Schema> {
        if let Session::Mongo { .. } = self {
            return self.mongo_schema();
        }
        let (tables_sql, columns_sql, keys_sql) = match self {
            Session::MySql(_) => (MYSQL_TABLES, MYSQL_COLUMNS, MYSQL_KEYS),
            Session::Postgres(_) => (POSTGRES_TABLES, POSTGRES_COLUMNS, POSTGRES_KEYS),
            Session::Oracle(_) => (ORACLE_TABLES, ORACLE_COLUMNS, ORACLE_KEYS),
            Session::Mongo { .. } => unreachable!(),
        };
        let mut schema = Schema::default();
        let listed = self.read_rows(tables_sql, MAX_TABLES + 1)?;
        schema.truncated = listed.truncated || listed.rows.len() > MAX_TABLES;
        let mut index = HashMap::new();
        for row in listed.rows.iter().take(MAX_TABLES) {
            let name = text(row, 0);
            if name.is_empty() || index.contains_key(&name) {
                continue;
            }
            let kind = if text(row, 1).to_ascii_uppercase().contains("VIEW") || text(row, 1) == "v" || text(row, 1) == "m" {
                "view"
            } else {
                "table"
            };
            let comment = text(row, 2);
            // MySQL says "VIEW" as the comment of every view.
            let comment = if kind == "view" && comment.eq_ignore_ascii_case("view") { String::new() } else { comment };
            index.insert(name.clone(), schema.tables.len());
            schema.tables.push(TableShape { name, kind: kind.into(), comment, columns: Vec::new() });
        }
        for row in self.read_rows(columns_sql, MAX_COLUMNS)?.rows {
            let Some(&i) = index.get(&text(&row, 0)) else { continue };
            schema.tables[i].columns.push(ColumnShape {
                name: text(&row, 1),
                data_type: text(&row, 2),
                nullable: matches!(text(&row, 3).as_str(), "YES" | "Y"),
                primary: text(&row, 4) == "PRI",
                comment: text(&row, 5),
            });
        }
        let mut seen = HashSet::new();
        for row in self.read_rows(keys_sql, MAX_COLUMNS)?.rows {
            let relation = Relation {
                from_table: text(&row, 0),
                from_column: text(&row, 1),
                to_table: text(&row, 2),
                to_column: text(&row, 3),
                declared: true,
            };
            if index.contains_key(&relation.from_table) && index.contains_key(&relation.to_table) && seen.insert(relation.clone()) {
                schema.foreign_keys.push(relation);
            }
        }
        Ok(schema)
    }

    /// MongoDB has no schema: the fields of a few documents of each collection, typed by the first
    /// value seen. `_id` is the key; links between collections are all inferred.
    fn mongo_schema(&mut self) -> Result<Schema> {
        let Session::Mongo { client, database } = self else { return Err(anyhow!("not a MongoDB connection")) };
        let db = client.database(database.as_deref().ok_or_else(|| anyhow!("no database in this connection"))?);
        let mut names = db.list_collection_names().run()?;
        names.retain(|n| !n.starts_with("system."));
        names.sort();
        let truncated = names.len() > MAX_TABLES;
        names.truncate(MAX_TABLES);
        let mut tables = Vec::new();
        for name in names {
            let sample = self.mongo_read(MongoOp::Find, &name, &serde_json::json!({}), MONGO_SAMPLE)?;
            let columns = sample
                .columns
                .iter()
                .enumerate()
                .map(|(i, field)| ColumnShape {
                    name: field.clone(),
                    data_type: sample.rows.iter().filter_map(|r| r.get(i)).find(|v| !v.is_null()).map(json_type).unwrap_or_default(),
                    nullable: sample.rows.iter().any(|r| r.get(i).is_none_or(Value::is_null)),
                    primary: field == "_id",
                    comment: String::new(),
                })
                .collect();
            tables.push(TableShape { name, kind: "collection".into(), comment: String::new(), columns });
        }
        Ok(Schema { tables, foreign_keys: Vec::new(), truncated })
    }
}

fn text(row: &[Value], i: usize) -> String {
    match row.get(i) {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

fn json_type(value: &Value) -> String {
    match value {
        Value::Object(map) if map.contains_key("$oid") => "objectId",
        Value::Object(map) if map.contains_key("$date") => "date",
        Value::Object(map) if map.contains_key("$uuid") || map.contains_key("$binary") => "binary",
        Value::Object(_) => "object",
        Value::Array(_) => "array",
        Value::String(_) => "string",
        Value::Number(_) => "number",
        Value::Bool(_) => "bool",
        Value::Null => "",
    }
    .into()
}

// -- catalog queries -----------------------------------------------------------------------------
// Each reads the whole database at once: (table, kind, comment); (table, column, type, nullable,
// "PRI" for a primary key column, comment); (table, column, referenced table, referenced column).

const MYSQL_TABLES: &str = "SELECT TABLE_NAME, TABLE_TYPE, TABLE_COMMENT FROM information_schema.TABLES \
     WHERE TABLE_SCHEMA = DATABASE() ORDER BY TABLE_NAME";
const MYSQL_COLUMNS: &str = "SELECT TABLE_NAME, COLUMN_NAME, COLUMN_TYPE, IS_NULLABLE, COLUMN_KEY, COLUMN_COMMENT \
     FROM information_schema.COLUMNS WHERE TABLE_SCHEMA = DATABASE() ORDER BY TABLE_NAME, ORDINAL_POSITION";
const MYSQL_KEYS: &str = "SELECT TABLE_NAME, COLUMN_NAME, REFERENCED_TABLE_NAME, REFERENCED_COLUMN_NAME \
     FROM information_schema.KEY_COLUMN_USAGE WHERE TABLE_SCHEMA = DATABASE() \
     AND REFERENCED_TABLE_SCHEMA = DATABASE() AND REFERENCED_TABLE_NAME IS NOT NULL";

macro_rules! postgres_schemas {
    () => {
        "n.nspname NOT IN ('pg_catalog', 'information_schema') AND n.nspname NOT LIKE 'pg_toast%'"
    };
}
const POSTGRES_TABLES: &str = concat!(
    "SELECT n.nspname || '.' || c.relname, c.relkind::text, coalesce(obj_description(c.oid, 'pg_class'), '') \
     FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
     WHERE c.relkind IN ('r', 'p', 'v', 'm', 'f') AND NOT c.relispartition AND ",
    postgres_schemas!(),
    " ORDER BY 1"
);
const POSTGRES_COLUMNS: &str = concat!(
    "SELECT n.nspname || '.' || c.relname, a.attname, format_type(a.atttypid, a.atttypmod), \
     CASE WHEN a.attnotnull THEN 'NO' ELSE 'YES' END, \
     CASE WHEN EXISTS (SELECT 1 FROM pg_index i WHERE i.indrelid = c.oid AND i.indisprimary AND a.attnum = ANY(i.indkey)) \
     THEN 'PRI' ELSE '' END, coalesce(col_description(c.oid, a.attnum), '') \
     FROM pg_attribute a JOIN pg_class c ON c.oid = a.attrelid JOIN pg_namespace n ON n.oid = c.relnamespace \
     WHERE a.attnum > 0 AND NOT a.attisdropped AND c.relkind IN ('r', 'p', 'v', 'm', 'f') AND ",
    postgres_schemas!(),
    " ORDER BY 1, a.attnum"
);
const POSTGRES_KEYS: &str = concat!(
    "SELECT n.nspname || '.' || c.relname, a.attname, fn.nspname || '.' || fc.relname, fa.attname \
     FROM pg_constraint co \
     JOIN pg_class c ON c.oid = co.conrelid JOIN pg_namespace n ON n.oid = c.relnamespace \
     JOIN pg_class fc ON fc.oid = co.confrelid JOIN pg_namespace fn ON fn.oid = fc.relnamespace \
     CROSS JOIN LATERAL unnest(co.conkey, co.confkey) AS k(col, fcol) \
     JOIN pg_attribute a ON a.attrelid = co.conrelid AND a.attnum = k.col \
     JOIN pg_attribute fa ON fa.attrelid = co.confrelid AND fa.attnum = k.fcol \
     WHERE co.contype = 'f' AND ",
    postgres_schemas!()
);

const ORACLE_TABLES: &str = "SELECT t.table_name, 'TABLE', c.comments FROM user_tables t \
     LEFT JOIN user_tab_comments c ON c.table_name = t.table_name \
     UNION ALL SELECT v.view_name, 'VIEW', c.comments FROM user_views v \
     LEFT JOIN user_tab_comments c ON c.table_name = v.view_name ORDER BY 1";
const ORACLE_COLUMNS: &str = "SELECT c.table_name, c.column_name, c.data_type, c.nullable, \
     CASE WHEN EXISTS (SELECT 1 FROM user_constraints k JOIN user_cons_columns kc ON kc.constraint_name = k.constraint_name \
     WHERE k.constraint_type = 'P' AND k.table_name = c.table_name AND kc.column_name = c.column_name) THEN 'PRI' ELSE 'NO' END, \
     m.comments FROM user_tab_columns c \
     LEFT JOIN user_col_comments m ON m.table_name = c.table_name AND m.column_name = c.column_name \
     ORDER BY c.table_name, c.column_id";
const ORACLE_KEYS: &str = "SELECT a.table_name, a.column_name, b.table_name, b.column_name FROM user_constraints c \
     JOIN user_cons_columns a ON a.constraint_name = c.constraint_name \
     JOIN user_cons_columns b ON b.constraint_name = c.r_constraint_name AND b.position = a.position \
     WHERE c.constraint_type = 'R'";

// -- relationships -------------------------------------------------------------------------------

/// Every relationship worth drawing: the declared foreign keys, then the ones the names say — a
/// column named after another table (`user_id`, `userId` → `users`), or carrying another table's
/// own key name (`emp_no` → `employees.emp_no`) — where the referenced column exists and the two
/// types could hold the same values.
pub fn relations(schema: &Schema) -> Vec<Relation> {
    let mut out: Vec<Relation> = schema.foreign_keys.clone();
    let declared: HashSet<(&str, &str)> = out.iter().map(|r| (r.from_table.as_str(), r.from_column.as_str())).collect();
    let mut inferred = Vec::new();

    // Tables by the bare name they would be referred to by.
    let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, table) in schema.tables.iter().enumerate() {
        if table.kind != "view" {
            by_name.entry(bare(&table.name)).or_default().push(i);
        }
    }
    // Tables by their one primary key column, when that name says whose key it is (not just `id`).
    let mut by_key: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, table) in schema.tables.iter().enumerate() {
        if let [key] = primary_keys(table).as_slice() {
            let name = snake(&key.name);
            if !GENERIC_KEYS.contains(&name.as_str()) && name.len() > 3 {
                by_key.entry(name).or_default().push(i);
            }
        }
    }

    for (from, table) in schema.tables.iter().enumerate() {
        if table.kind == "view" {
            continue;
        }
        for column in &table.columns {
            if declared.contains(&(table.name.as_str(), column.name.as_str())) {
                continue;
            }
            let name = snake(&column.name);
            if GENERIC_KEYS.contains(&name.as_str()) {
                continue;
            }
            let found = by_stem(schema, &by_name, from, &name).or_else(|| by_key_name(schema, &by_key, from, column, &name));
            let Some((to, to_column)) = found else { continue };
            let target = &schema.tables[to].columns[to_column];
            if to == from && target.name == column.name {
                continue;
            }
            if !compatible(&column.data_type, &target.data_type) {
                continue;
            }
            inferred.push(Relation {
                from_table: table.name.clone(),
                from_column: column.name.clone(),
                to_table: schema.tables[to].name.clone(),
                to_column: target.name.clone(),
                declared: false,
            });
        }
    }
    out.extend(inferred);
    out
}

/// Key names that say nothing about whose key they are.
const GENERIC_KEYS: &[&str] = &["id", "_id", "no", "seq", "code", "key", "uuid", "idx"];

/// `user_id` / `userId` / `userid` / `owner_user_id` → the table it names, and the column of that
/// table it points at. A longer stem wins over its tail (`order_item_id` → `order_items` before
/// `items`); `parent_id` is the table's own key.
fn by_stem(schema: &Schema, by_name: &HashMap<String, Vec<usize>>, from: usize, name: &str) -> Option<(usize, usize)> {
    let stem = name.strip_suffix("_id").or_else(|| (name.len() >= 6 && !name.contains('_')).then(|| name.strip_suffix("id")).flatten())?;
    if stem.is_empty() {
        return None;
    }
    if stem == "parent" {
        return referenced_column(&schema.tables[from], name).map(|c| (from, c));
    }
    let parts: Vec<&str> = stem.split('_').filter(|p| !p.is_empty()).collect();
    for start in 0..parts.len() {
        let tail = parts[start..].join("_");
        for candidate in plurals(&tail) {
            let Some(found) = by_name.get(&candidate) else { continue };
            let to = pick(schema, found, from);
            if let Some(column) = referenced_column(&schema.tables[to], name) {
                return Some((to, column));
            }
        }
    }
    None
}

/// A column carrying the exact name of another table's one, specific primary key (`emp_no`).
fn by_key_name(
    schema: &Schema,
    by_key: &HashMap<String, Vec<usize>>,
    from: usize,
    column: &ColumnShape,
    name: &str,
) -> Option<(usize, usize)> {
    let owners: Vec<usize> = by_key.get(name)?.iter().copied().filter(|&i| i != from).collect();
    // Two tables keyed the same way, or both sides keyed by it: no telling which way it points.
    if owners.len() != 1 || (column.primary && primary_keys(&schema.tables[from]).len() == 1) {
        return None;
    }
    let to = owners[0];
    let key = schema.tables[to].columns.iter().position(|c| c.primary)?;
    Some((to, key))
}

/// Of several tables with the same bare name (one per PostgreSQL schema), the one in the
/// referencing table's own schema.
fn pick(schema: &Schema, found: &[usize], from: usize) -> usize {
    let home = schema_of(&schema.tables[from].name);
    found.iter().copied().find(|&i| schema_of(&schema.tables[i].name) == home).unwrap_or(found[0])
}

/// The column of `table` a reference named `name` points at: its one primary key, else `id` /
/// `_id`, else a column of the very same name.
fn referenced_column(table: &TableShape, name: &str) -> Option<usize> {
    let keys: Vec<usize> = table.columns.iter().enumerate().filter(|(_, c)| c.primary).map(|(i, _)| i).collect();
    if let [key] = keys.as_slice() {
        return Some(*key);
    }
    table
        .columns
        .iter()
        .position(|c| matches!(c.name.to_ascii_lowercase().as_str(), "id" | "_id"))
        .or_else(|| table.columns.iter().position(|c| snake(&c.name) == name))
}

fn primary_keys(table: &TableShape) -> Vec<&ColumnShape> {
    table.columns.iter().filter(|c| c.primary).collect()
}

/// `public.tb_users` → `users`: lower case, without the schema or a `tb_` style prefix.
fn bare(table: &str) -> String {
    let name = snake(table.rsplit('.').next().unwrap_or(table));
    for prefix in ["tbl_", "tb_", "t_"] {
        if let Some(rest) = name.strip_prefix(prefix) {
            if !rest.is_empty() {
                return rest.to_string();
            }
        }
    }
    name
}

fn schema_of(table: &str) -> &str {
    table.rsplit_once('.').map(|(s, _)| s).unwrap_or("")
}

/// `userId` / `UserID` / `USER_ID` → `user_id`.
fn snake(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    let chars: Vec<char> = name.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c.is_uppercase() {
            let prev = i.checked_sub(1).map(|p| chars[p]);
            let next = chars.get(i + 1).copied();
            let starts_word = prev.is_some_and(|p| p.is_lowercase() || p.is_ascii_digit())
                || (prev.is_some_and(char::is_uppercase) && next.is_some_and(char::is_lowercase));
            if starts_word && !out.ends_with('_') {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// The names a table holding `noun`s may have: `user`, `users`, `boxes`, `categories`, `people`.
fn plurals(noun: &str) -> Vec<String> {
    let mut out = vec![noun.to_string(), format!("{noun}s"), format!("{noun}es")];
    if let Some(stem) = noun.strip_suffix('y') {
        out.push(format!("{stem}ies"));
    }
    // Irregular ones, as the last word too (`site_person` → `site_people`).
    for (one, many) in [("person", "people"), ("child", "children")] {
        if let Some(head) = noun.strip_suffix(one) {
            out.push(format!("{head}{many}"));
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Number,
    Text,
    Uuid,
    Binary,
    ObjectId,
    Other,
}

fn family(data_type: &str) -> Option<Family> {
    let t = data_type.trim().to_ascii_lowercase();
    if t.is_empty() {
        return None;
    }
    let starts = |prefixes: &[&str]| prefixes.iter().any(|p| t.starts_with(p));
    Some(
        if starts(&[
            "int",
            "bigint",
            "smallint",
            "tinyint",
            "mediumint",
            "serial",
            "bigserial",
            "smallserial",
            "number",
            "numeric",
            "decimal",
            "long",
            "double",
        ]) {
            Family::Number
        } else if t == "uuid" || t == "uniqueidentifier" {
            Family::Uuid
        } else if starts(&[
            "char",
            "varchar",
            "nvarchar",
            "nchar",
            "text",
            "character",
            "string",
            "citext",
            "tinytext",
            "mediumtext",
            "longtext",
            "varchar2",
            "nvarchar2",
        ]) {
            Family::Text
        } else if starts(&["binary", "varbinary", "bytea", "raw", "blob"]) {
            Family::Binary
        } else if t == "objectid" {
            Family::ObjectId
        } else {
            Family::Other
        },
    )
}

/// Whether a column of type `a` could hold values of a column of type `b`. Unknown types (a
/// MongoDB field never seen with a value) pass; a UUID kept as text on one side passes too.
fn compatible(a: &str, b: &str) -> bool {
    match (family(a), family(b)) {
        (Some(x), Some(y)) => x == y || matches!((x, y), (Family::Text, Family::Uuid) | (Family::Uuid, Family::Text)),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column(name: &str, data_type: &str, primary: bool) -> ColumnShape {
        ColumnShape { name: name.into(), data_type: data_type.into(), nullable: false, primary, comment: String::new() }
    }

    fn table(name: &str, columns: Vec<ColumnShape>) -> TableShape {
        TableShape { name: name.into(), kind: "table".into(), comment: String::new(), columns }
    }

    fn pairs(relations: &[Relation]) -> Vec<String> {
        let mut out: Vec<String> = relations
            .iter()
            .map(|r| format!("{}.{} -> {}.{}{}", r.from_table, r.from_column, r.to_table, r.to_column, if r.declared { "" } else { " ?" }))
            .collect();
        out.sort();
        out
    }

    #[test]
    fn names_point_at_tables() {
        let schema = Schema {
            tables: vec![
                table("users", vec![column("id", "bigint", true), column("name", "varchar(80)", false)]),
                table("categories", vec![column("id", "int", true)]),
                table("user_settings", vec![column("id", "bigint", true), column("user_id", "bigint", false)]),
                table(
                    "posts",
                    vec![column("id", "bigint", true), column("authorUserId", "bigint", false), column("category_id", "int", false)],
                ),
                table(
                    "comments",
                    vec![column("id", "bigint", true), column("post_id", "bigint", false), column("parent_id", "bigint", false)],
                ),
            ],
            ..Default::default()
        };
        assert_eq!(
            pairs(&relations(&schema)),
            vec![
                "comments.parent_id -> comments.id ?",
                "comments.post_id -> posts.id ?",
                "posts.authorUserId -> users.id ?",
                "posts.category_id -> categories.id ?",
                "user_settings.user_id -> users.id ?",
            ]
        );
    }

    #[test]
    fn declared_keys_come_first_and_are_not_guessed_again() {
        let schema = Schema {
            tables: vec![
                table("users", vec![column("id", "bigint", true)]),
                table("orders", vec![column("id", "bigint", true), column("user_id", "bigint", false)]),
            ],
            foreign_keys: vec![Relation {
                from_table: "orders".into(),
                from_column: "user_id".into(),
                to_table: "users".into(),
                to_column: "id".into(),
                declared: true,
            }],
            ..Default::default()
        };
        assert_eq!(pairs(&relations(&schema)), vec!["orders.user_id -> users.id"]);
    }

    #[test]
    fn a_name_alone_is_not_enough() {
        let schema = Schema {
            tables: vec![
                table("users", vec![column("id", "bigint", true)]),
                // Types that cannot hold the same values, a table that does not exist, a view.
                table(
                    "logs",
                    vec![column("id", "bigint", true), column("user_id", "timestamp", false), column("device_id", "bigint", false)],
                ),
                TableShape { kind: "view".into(), ..table("user_view", vec![column("user_id", "bigint", false)]) },
            ],
            ..Default::default()
        };
        assert!(relations(&schema).is_empty());
    }

    #[test]
    fn a_table_key_name_links_without_an_id_suffix() {
        let schema = Schema {
            tables: vec![
                table("employees", vec![column("emp_no", "int", true), column("name", "varchar(40)", false)]),
                table("salaries", vec![column("emp_no", "int", true), column("from_date", "date", true), column("salary", "int", false)]),
                table("tb_member", vec![column("member_seq", "bigint", true)]),
                table("tb_point", vec![column("point_seq", "bigint", true), column("member_seq", "bigint", false)]),
            ],
            ..Default::default()
        };
        assert_eq!(
            pairs(&relations(&schema)),
            vec!["salaries.emp_no -> employees.emp_no ?", "tb_point.member_seq -> tb_member.member_seq ?"]
        );
    }

    #[test]
    fn schemas_prefixes_and_mongo_fields() {
        let schema = Schema {
            tables: vec![
                table("public.tb_users", vec![column("id", "uuid", true)]),
                table("billing.users", vec![column("id", "uuid", true)]),
                table("billing.invoices", vec![column("id", "uuid", true), column("user_id", "character varying", false)]),
                TableShape { kind: "collection".into(), ..table("people", vec![column("_id", "objectId", true)]) },
                TableShape {
                    kind: "collection".into(),
                    ..table("visits", vec![column("_id", "objectId", true), column("personId", "objectId", false)])
                },
            ],
            ..Default::default()
        };
        assert_eq!(pairs(&relations(&schema)), vec!["billing.invoices.user_id -> billing.users.id ?", "visits.personId -> people._id ?"]);
    }

    #[test]
    fn names_are_normalised() {
        assert_eq!(snake("userId"), "user_id");
        assert_eq!(snake("UserID"), "user_id");
        assert_eq!(snake("HTTPServerId"), "http_server_id");
        assert_eq!(bare("public.TB_ORDERS"), "orders");
        assert!(compatible("int(11) unsigned", "bigint") && compatible("varchar(36)", "uuid") && !compatible("int", "varchar(10)"));
    }
}
