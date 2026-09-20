//! Which statements may run without asking. Agents (and the query box) may read freely; anything
//! that could change data, schema or server state waits for the user's explicit "Execute" in a
//! dialog that shows the statement. This module decides which is which — conservatively: only
//! statements recognized as plain reads are reads, everything else asks.
//!
//! A read is also run inside a read-only transaction by the engines, so the database itself refuses
//! a write that slipped through (a function with side effects, a view with a trigger, …). DDL is the
//! exception there — MySQL commits implicitly before DDL, even in a read-only transaction — which is
//! why DDL never classifies as a read.

use crate::model::Engine;
use serde::Serialize;

/// What a statement would do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Verdict {
    /// Only reads: runs at once, in a read-only transaction.
    Read,
    /// Changes rows (INSERT, UPDATE, DELETE, …) or takes locks: asks first.
    Write,
    /// Changes schema or server state (CREATE, DROP, ALTER, TRUNCATE, GRANT, …): asks first.
    Ddl,
}

impl Verdict {
    pub fn needs_approval(self) -> bool {
        self != Verdict::Read
    }
}

/// Words that start a statement that only reads.
const READ_STARTS: &[&str] = &["SELECT", "SHOW", "DESCRIBE", "DESC", "EXPLAIN", "WITH", "VALUES", "TABLE"];

/// Words that start a statement that changes schema, users or server state.
const DDL_STARTS: &[&str] = &[
    "CREATE",
    "ALTER",
    "DROP",
    "TRUNCATE",
    "RENAME",
    "GRANT",
    "REVOKE",
    "COMMENT",
    "ANALYZE",
    "OPTIMIZE",
    "REPAIR",
    "VACUUM",
    "REINDEX",
    "CLUSTER",
    "FLUSH",
    "RESET",
    "PURGE",
    "INSTALL",
    "UNINSTALL",
    "KILL",
    "SHUTDOWN",
    "AUDIT",
    "NOAUDIT",
];

/// Words that anywhere in a would-be read make it a write: data changes, results written somewhere,
/// row locks, sequence changes and calls into code.
const WRITE_WORDS: &[&str] = &[
    "INSERT",
    "UPDATE",
    "DELETE",
    "MERGE",
    "REPLACE",
    "UPSERT",
    "INTO",
    "OUTFILE",
    "DUMPFILE",
    "LOCK",
    "FOR UPDATE",
    "FOR SHARE",
    "FOR NO KEY UPDATE",
    "FOR KEY SHARE",
    "NEXTVAL",
    "SETVAL",
    "CALL",
    "EXEC",
    "EXECUTE",
    "SLEEP",
    "BENCHMARK",
    "GET_LOCK",
    "RELEASE_LOCK",
    "PG_SLEEP",
    "PG_TERMINATE_BACKEND",
    "PG_CANCEL_BACKEND",
    "LO_IMPORT",
    "LO_EXPORT",
    "PG_READ_FILE",
    "PG_WRITE_FILE",
    "LOAD_FILE",
    "DBMS_",
    "UTL_",
];

/// Words that may stand before `(` in a read without being a function call: keywords, and type
/// names in casts (`CAST(x AS DECIMAL(10, 2))`).
const PAREN_KEYWORDS: &[&str] = &[
    "SELECT",
    "FROM",
    "WHERE",
    "AND",
    "OR",
    "NOT",
    "IN",
    "EXISTS",
    "AS",
    "ON",
    "JOIN",
    "USING",
    "VALUES",
    "OVER",
    "FILTER",
    "GROUP",
    "WITHIN",
    "BY",
    "HAVING",
    "WHEN",
    "THEN",
    "ELSE",
    "CASE",
    "END",
    "ANY",
    "ALL",
    "SOME",
    "ROW",
    "ARRAY",
    "LATERAL",
    "UNION",
    "INTERSECT",
    "EXCEPT",
    "MINUS",
    "DISTINCT",
    "WITH",
    "RECURSIVE",
    "LIMIT",
    "OFFSET",
    "BETWEEN",
    "LIKE",
    "ILIKE",
    "IS",
    "INTERVAL",
    "TABLE",
    "PARTITION",
    "ORDER",
    "ASC",
    "DESC",
    "NULLS",
    "FIRST",
    "LAST",
    "EXPLAIN",
    "FORMAT",
    "ROWS",
    "RANGE",
    "PRECEDING",
    "FOLLOWING",
    "CURRENT",
    "UNBOUNDED",
    "FETCH",
    "NEXT",
    "ONLY",
    "TOP",
    "IF",
    "DECIMAL",
    "NUMERIC",
    "NUMBER",
    "VARCHAR",
    "VARCHAR2",
    "NVARCHAR",
    "NVARCHAR2",
    "CHAR",
    "NCHAR",
    "CHARACTER",
    "VARYING",
    "BINARY",
    "VARBINARY",
    "FLOAT",
    "DOUBLE",
    "PRECISION",
    "REAL",
    "BIT",
    "INT",
    "INTEGER",
    "BIGINT",
    "SMALLINT",
    "TIME",
    "TIMESTAMP",
    "DATETIME",
    "SIGNED",
    "UNSIGNED",
    "RAW",
];

/// Built-in functions that only compute (no side effects, no access outside the query). A read
/// that calls anything else — a stored function, an extension, a package — asks first: such code
/// can write despite a read-only transaction (Oracle autonomous transactions, `dblink_exec`, UDFs).
const READ_FUNCTIONS: &[&str] = &[
    // aggregates and windows
    "COUNT",
    "SUM",
    "AVG",
    "MIN",
    "MAX",
    "STDDEV",
    "STDDEV_POP",
    "STDDEV_SAMP",
    "VARIANCE",
    "VAR_POP",
    "VAR_SAMP",
    "GROUP_CONCAT",
    "STRING_AGG",
    "LISTAGG",
    "ARRAY_AGG",
    "JSON_AGG",
    "JSONB_AGG",
    "JSON_ARRAYAGG",
    "JSON_OBJECTAGG",
    "BOOL_AND",
    "BOOL_OR",
    "EVERY",
    "BIT_AND",
    "BIT_OR",
    "ROW_NUMBER",
    "RANK",
    "DENSE_RANK",
    "PERCENT_RANK",
    "CUME_DIST",
    "NTILE",
    "LAG",
    "LEAD",
    "FIRST_VALUE",
    "LAST_VALUE",
    "NTH_VALUE",
    "PERCENTILE_CONT",
    "PERCENTILE_DISC",
    "MEDIAN",
    "MODE",
    // conditionals and conversion
    "COALESCE",
    "NULLIF",
    "IFNULL",
    "ISNULL",
    "NVL",
    "NVL2",
    "DECODE",
    "GREATEST",
    "LEAST",
    "CAST",
    "CONVERT",
    "TRY_CAST",
    "TO_CHAR",
    "TO_NUMBER",
    "TO_DATE",
    "TO_TIMESTAMP",
    "TO_JSON",
    "TO_JSONB",
    "TO_HEX",
    // strings
    "LOWER",
    "UPPER",
    "INITCAP",
    "LENGTH",
    "CHAR_LENGTH",
    "CHARACTER_LENGTH",
    "OCTET_LENGTH",
    "BIT_LENGTH",
    "CONCAT",
    "CONCAT_WS",
    "SUBSTR",
    "SUBSTRING",
    "SUBSTRING_INDEX",
    "LEFT",
    "RIGHT",
    "LPAD",
    "RPAD",
    "TRIM",
    "LTRIM",
    "RTRIM",
    "BTRIM",
    "INSTR",
    "LOCATE",
    "POSITION",
    "STRPOS",
    "SPLIT_PART",
    "REVERSE",
    "REPEAT",
    "CHR",
    "ASCII",
    "HEX",
    "UNHEX",
    "MD5",
    "SHA1",
    "SHA2",
    "REGEXP_LIKE",
    "REGEXP_REPLACE",
    "REGEXP_SUBSTR",
    "REGEXP_INSTR",
    "REGEXP_COUNT",
    "REGEXP_MATCHES",
    "TRANSLATE",
    "QUOTE_IDENT",
    "QUOTE_LITERAL",
    "FORMAT_TYPE",
    "STARTS_WITH",
    "FIELD",
    "FIND_IN_SET",
    "ELT",
    "SOUNDEX",
    // numbers
    "ABS",
    "CEIL",
    "CEILING",
    "FLOOR",
    "ROUND",
    "TRUNC",
    "MOD",
    "POWER",
    "POW",
    "SQRT",
    "EXP",
    "LN",
    "LOG",
    "LOG10",
    "LOG2",
    "SIGN",
    "PI",
    "DIV",
    "WIDTH_BUCKET",
    // dates
    "NOW",
    "CURRENT_DATE",
    "CURRENT_TIME",
    "CURRENT_TIMESTAMP",
    "LOCALTIME",
    "LOCALTIMESTAMP",
    "SYSDATE",
    "SYSTIMESTAMP",
    "UTC_DATE",
    "UTC_TIMESTAMP",
    "DATE",
    "YEAR",
    "MONTH",
    "DAY",
    "DAYOFWEEK",
    "DAYOFMONTH",
    "DAYOFYEAR",
    "WEEK",
    "WEEKDAY",
    "QUARTER",
    "HOUR",
    "MINUTE",
    "SECOND",
    "DATE_FORMAT",
    "DATE_ADD",
    "DATE_SUB",
    "ADDDATE",
    "SUBDATE",
    "DATEDIFF",
    "TIMEDIFF",
    "TIMESTAMPDIFF",
    "TIMESTAMPADD",
    "STR_TO_DATE",
    "UNIX_TIMESTAMP",
    "FROM_UNIXTIME",
    "LAST_DAY",
    "ADD_MONTHS",
    "MONTHS_BETWEEN",
    "DATE_TRUNC",
    "DATE_PART",
    "EXTRACT",
    "AGE",
    "MAKE_DATE",
    "MAKE_TIMESTAMP",
    // JSON
    "JSON_EXTRACT",
    "JSON_UNQUOTE",
    "JSON_VALUE",
    "JSON_QUERY",
    "JSON_CONTAINS",
    "JSON_CONTAINS_PATH",
    "JSON_KEYS",
    "JSON_LENGTH",
    "JSON_TYPE",
    "JSON_VALID",
    "JSON_OBJECT",
    "JSON_ARRAY",
    "JSON_BUILD_OBJECT",
    "JSON_BUILD_ARRAY",
    "JSONB_BUILD_OBJECT",
    "JSONB_BUILD_ARRAY",
    "JSON_EXTRACT_PATH",
    "JSON_EXTRACT_PATH_TEXT",
    "JSONB_EXTRACT_PATH",
    "JSONB_EXTRACT_PATH_TEXT",
    "JSON_ARRAY_LENGTH",
    "JSONB_ARRAY_LENGTH",
    "JSON_EACH",
    "JSONB_EACH",
    "JSON_ARRAY_ELEMENTS",
    "JSONB_ARRAY_ELEMENTS",
    "JSON_TABLE",
    "JSONB_PRETTY",
    // sets and system information
    "UNNEST",
    "GENERATE_SERIES",
    "VERSION",
    "DATABASE",
    "SCHEMA",
    "CURRENT_USER",
    "SESSION_USER",
    "USER",
    "CURRENT_SCHEMA",
    "CURRENT_DATABASE",
    "FOUND_ROWS",
    "ROW_COUNT",
    "PG_TYPEOF",
    "PG_SIZE_PRETTY",
    "PG_TOTAL_RELATION_SIZE",
    "PG_RELATION_SIZE",
    "PG_TABLE_SIZE",
    "PG_DATABASE_SIZE",
    "OBJ_DESCRIPTION",
    "COL_DESCRIPTION",
];

/// Whether a statement returns rows (starts like a read), whatever else it may do: an approved
/// `SELECT my_function()` still shows its result.
pub fn returns_rows(engine: Engine, sql: &str) -> bool {
    normalize(engine, sql).and_then(|text| text.split_whitespace().next().map(|w| READ_STARTS.contains(&w))).unwrap_or(false)
}

/// Classifies one SQL statement. The engine decides how the text is read: a comment in one dialect
/// is an operator in another, and a statement the reader mis-lexes is a statement it mis-classifies.
pub fn classify_sql(engine: Engine, sql: &str) -> Verdict {
    let Some(text) = normalize(engine, sql) else { return Verdict::Write };
    let words: Vec<&str> = text.split_whitespace().collect();
    let Some(first) = words.first().copied() else { return Verdict::Write };
    if DDL_STARTS.contains(&first) {
        return Verdict::Ddl;
    }
    if !READ_STARTS.contains(&first) {
        return Verdict::Write;
    }
    // EXPLAIN ANALYZE runs the statement it explains — in PostgreSQL also as `EXPLAIN (ANALYZE) …`,
    // and Oracle's `EXPLAIN PLAN FOR …` writes its rows into PLAN_TABLE.
    if first == "EXPLAIN" && (words.iter().any(|w| w.starts_with("ANALY")) || words.get(1) == Some(&"PLAN")) {
        return Verdict::Write;
    }
    // A call into code the reader doesn't know: `name (` where `name` is neither a keyword nor a
    // built-in that only computes. `(` and `)` are tokens of their own, so a call is always the
    // pair `name (`. Schema-qualified (`pkg.fn(`) and quoted names count as unknown.
    for pair in words.windows(2) {
        if pair[1] == "(" {
            let name = pair[0];
            let known = PAREN_KEYWORDS.contains(&name) || READ_FUNCTIONS.contains(&name);
            let is_name = name.starts_with(|c: char| c.is_alphabetic() || c == '_' || c == '.');
            if is_name && !known {
                return Verdict::Write;
            }
        }
    }
    // Write words are matched against whole tokens, not as substrings of the joined text: a word
    // glued to a `.` (`orders_seq.NEXTVAL`) must still count.
    let tokens: Vec<&str> = words.iter().map(|w| w.trim_start_matches('.')).collect();
    if WRITE_WORDS.iter().any(|w| match w.split_once(' ') {
        // Two-word entries (`FOR UPDATE`) are adjacent tokens.
        Some((a, b)) => tokens.windows(2).any(|pair| pair[0] == a && pair[1] == b),
        // A prefix entry (`DBMS_`, `UTL_`) matches any token that starts with it.
        None if w.ends_with('_') => tokens.iter().any(|t| t.starts_with(w)),
        None => tokens.contains(w),
    }) {
        return Verdict::Write;
    }
    Verdict::Read
}

/// The statement upper-cased with comments, string literals and quoted names blanked, so only its
/// keywords remain — or `None` when it can't be read safely: several statements, an unterminated
/// quote or comment, or an executable comment (`/*! … */`, `/*M! … */`, which the server runs).
///
/// Read with the target engine's own rules. Taking the union of every dialect's comment syntax
/// would hide statements from the reader that the server still runs: MySQL needs whitespace after
/// `--` (`1--1` is arithmetic), and `#` starts a comment only there — in PostgreSQL it is an
/// operator, in Oracle an identifier character.
fn normalize(engine: Engine, sql: &str) -> Option<String> {
    let mysql_family = matches!(engine, Engine::MySql | Engine::MariaDb);
    let chars: Vec<char> = sql.chars().collect();
    let mut out = String::with_capacity(sql.len());
    let mut i = 0;
    let mut ended = false;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        // MySQL and MariaDB only treat `--` as a comment when whitespace (or the end) follows.
        let dash_comment = c == '-' && next == Some('-') && (!mysql_family || chars.get(i + 2).is_none_or(|c| c.is_whitespace()));
        match c {
            _ if dash_comment => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                out.push(' ');
                continue;
            }
            '#' if mysql_family => {
                // MySQL line comment. Elsewhere `#` falls through to the identifier branch below.
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                out.push(' ');
                continue;
            }
            '/' if next == Some('*') => {
                let executable = matches!(chars.get(i + 2), Some('!') | Some('+'))
                    // MariaDB also runs `/*M! … */`.
                    || (matches!(chars.get(i + 2), Some('M') | Some('m')) && chars.get(i + 3) == Some(&'!'));
                if executable {
                    // Executed by MySQL / MariaDB, or an Oracle hint: never treated as a comment.
                    return None;
                }
                let end = (i + 2..chars.len().saturating_sub(1)).find(|&j| chars[j] == '*' && chars[j + 1] == '/')?;
                i = end + 2;
                out.push(' ');
                continue;
            }
            '\'' | '"' | '`' => {
                let quote = c;
                i += 1;
                loop {
                    let ch = *chars.get(i)?;
                    if ch == '\\' {
                        // Whether a backslash escapes depends on server settings (MySQL's
                        // NO_BACKSLASH_ESCAPES, PostgreSQL's standard strings): don't guess.
                        return None;
                    }
                    if ch == quote {
                        if chars.get(i + 1) == Some(&quote) {
                            i += 2;
                            continue;
                        }
                        break;
                    }
                    i += 1;
                }
                i += 1;
                out.push_str(" q ");
                continue;
            }
            '$' if next == Some('$') => {
                // PostgreSQL dollar quoting: a body of code.
                return None;
            }
            ';' => {
                ended = true;
                i += 1;
                continue;
            }
            _ => {}
        }
        if ended && !c.is_whitespace() {
            // A second statement after `;`.
            return None;
        }
        // `$` and `#` are identifier characters in these engines (`evil$fn`, Oracle's `sys#fn`):
        // blanking them would leave only the tail of a name, which can be an allowed one.
        if c.is_alphanumeric() || c == '_' || c == '$' || c == '#' {
            out.extend(c.to_uppercase());
        } else if c == '.' {
            out.push(' ');
            out.push(c);
        } else if c == '(' || c == ')' || "=<>+-*/%|&^~!,:".contains(c) {
            // Parens and operators stay as words of their own: `x = (SELECT …)` is not a call of
            // `x`, and `(evil(1))` is still a call of `evil`.
            out.push(' ');
            out.push(c);
            out.push(' ');
        } else {
            out.push(' ');
        }
        i += 1;
    }
    Some(out)
}

/// A MongoDB operation an agent or the UI asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MongoOp {
    Find,
    Aggregate,
    Count,
    Distinct,
    ListCollections,
    InsertOne,
    InsertMany,
    UpdateOne,
    UpdateMany,
    ReplaceOne,
    DeleteOne,
    DeleteMany,
    CreateIndex,
    DropIndex,
    DropCollection,
}

impl MongoOp {
    /// The name as the MongoDB shell spells it.
    pub fn name(self) -> &'static str {
        match self {
            MongoOp::Find => "find",
            MongoOp::Aggregate => "aggregate",
            MongoOp::Count => "countDocuments",
            MongoOp::Distinct => "distinct",
            MongoOp::ListCollections => "listCollections",
            MongoOp::InsertOne => "insertOne",
            MongoOp::InsertMany => "insertMany",
            MongoOp::UpdateOne => "updateOne",
            MongoOp::UpdateMany => "updateMany",
            MongoOp::ReplaceOne => "replaceOne",
            MongoOp::DeleteOne => "deleteOne",
            MongoOp::DeleteMany => "deleteMany",
            MongoOp::CreateIndex => "createIndex",
            MongoOp::DropIndex => "dropIndex",
            MongoOp::DropCollection => "drop",
        }
    }

    pub fn parse(name: &str) -> Option<MongoOp> {
        Some(match name.to_ascii_lowercase().as_str() {
            "find" => MongoOp::Find,
            "aggregate" => MongoOp::Aggregate,
            "count" | "countdocuments" => MongoOp::Count,
            "distinct" => MongoOp::Distinct,
            "collections" | "listcollections" => MongoOp::ListCollections,
            "insertone" => MongoOp::InsertOne,
            "insertmany" => MongoOp::InsertMany,
            "updateone" => MongoOp::UpdateOne,
            "updatemany" => MongoOp::UpdateMany,
            "replaceone" => MongoOp::ReplaceOne,
            "deleteone" => MongoOp::DeleteOne,
            "deletemany" => MongoOp::DeleteMany,
            "createindex" => MongoOp::CreateIndex,
            "dropindex" => MongoOp::DropIndex,
            "drop" | "dropcollection" => MongoOp::DropCollection,
            _ => return None,
        })
    }
}

/// Whether a filter or document holds an operator that runs code on the server.
fn runs_code(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(map) => map
            .iter()
            .any(|(key, value)| matches!(key.as_str(), "$where" | "$function" | "$accumulator" | "$expr_code") || runs_code(value)),
        serde_json::Value::Array(items) => items.iter().any(runs_code),
        _ => false,
    }
}

/// Classifies a MongoDB operation. An aggregation that writes its result (`$out`, `$merge`) is a
/// write; so is any stage the reader doesn't know, since new stages may write.
pub fn classify_mongo(op: MongoOp, pipeline: Option<&serde_json::Value>) -> Verdict {
    match op {
        MongoOp::Find | MongoOp::Count | MongoOp::Distinct | MongoOp::ListCollections => {
            // A filter can carry code the server runs ($where, $function, $accumulator): that is
            // not a plain read, whatever the operation around it is.
            if pipeline.is_some_and(runs_code) {
                return Verdict::Write;
            }
            Verdict::Read
        }
        MongoOp::Aggregate => {
            let known = [
                "$match",
                "$project",
                "$group",
                "$sort",
                "$limit",
                "$skip",
                "$unwind",
                "$lookup",
                "$count",
                "$addFields",
                "$set",
                "$unset",
                "$facet",
                "$bucket",
                "$bucketAuto",
                "$sortByCount",
                "$replaceRoot",
                "$replaceWith",
                "$sample",
                "$graphLookup",
                "$densify",
                "$fill",
                "$redact",
                "$geoNear",
                "$unionWith",
                "$setWindowFields",
            ];
            let stages = pipeline.and_then(|p| p.as_array());
            let Some(stages) = stages else { return Verdict::Write };
            let reads =
                stages.iter().all(|stage| stage.as_object().is_some_and(|o| o.len() == 1 && o.keys().all(|k| known.contains(&k.as_str()))));
            if reads {
                Verdict::Read
            } else {
                Verdict::Write
            }
        }
        MongoOp::InsertOne
        | MongoOp::InsertMany
        | MongoOp::UpdateOne
        | MongoOp::UpdateMany
        | MongoOp::ReplaceOne
        | MongoOp::DeleteOne
        | MongoOp::DeleteMany => Verdict::Write,
        MongoOp::CreateIndex | MongoOp::DropIndex | MongoOp::DropCollection => Verdict::Ddl,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SQL_ENGINES: [Engine; 4] = [Engine::MySql, Engine::MariaDb, Engine::Postgres, Engine::Oracle];

    /// A verdict that must be the same whichever engine reads the statement.
    fn verdict(sql: &str) -> Verdict {
        let mut verdicts = SQL_ENGINES.into_iter().map(|engine| (engine, classify_sql(engine, sql)));
        let (first_engine, first) = verdicts.next().expect("one engine");
        for (engine, other) in verdicts {
            assert_eq!(other, first, "{engine:?} and {first_engine:?} disagree on {sql}");
        }
        first
    }

    #[test]
    fn calls_into_unknown_code_ask_first() {
        for read in [
            "SELECT COUNT(*), MAX(id), DATE_FORMAT(created_at, '%Y') FROM orders GROUP BY 3",
            "SELECT * FROM users WHERE id = (SELECT MAX(id) FROM users)",
            "SELECT a, (b + 1) * 2 FROM t WHERE x IN (1, 2) AND EXISTS (SELECT 1 FROM u)",
            "SELECT CAST(price AS DECIMAL(10, 2)), COALESCE(name, '') FROM items",
            "WITH recent AS (SELECT * FROM orders) SELECT * FROM recent",
            "SELECT ROW_NUMBER() OVER (PARTITION BY a ORDER BY b) FROM t",
            "SELECT o.id FROM orders o JOIN (SELECT id FROM users) u ON (o.user_id = u.id)",
        ] {
            assert_eq!(verdict(read), Verdict::Read, "{read}");
        }
        for call in [
            "SELECT my_pkg.do_things(1) FROM dual",
            "SELECT audit_and_return(id) FROM users",
            "SELECT dblink_exec('dbname=x', 'DELETE FROM t')",
            "SELECT pg_reload_conf()",
            "SELECT sys_exec('touch /tmp/x')",
            "SELECT \"weird\"(1)",
            "SELECT * FROM users WHERE id = my_function (1)",
        ] {
            assert_eq!(verdict(call), Verdict::Write, "{call}");
        }
    }

    #[test]
    fn plain_reads_are_reads() {
        for sql in [
            "select * from users",
            "SELECT id, name FROM users WHERE email = 'a@example.com' LIMIT 10",
            "  -- recent orders\nSELECT * FROM orders ORDER BY created_at DESC",
            "show tables",
            "SHOW CREATE TABLE users",
            "describe users",
            "desc users;",
            "EXPLAIN SELECT * FROM users",
            "WITH recent AS (SELECT * FROM orders) SELECT count(*) FROM recent",
            "select 'insert into x' as text",
            "select \"delete\" from `update`",
            "SELECT * FROM users /* no writes here */ WHERE id = 1",
            "TABLE users",
        ] {
            assert_eq!(verdict(sql), Verdict::Read, "{sql}");
        }
    }

    #[test]
    fn writes_ask() {
        for sql in [
            "insert into users(name) values ('x')",
            "UPDATE users SET name = 'x'",
            "delete from users",
            "REPLACE INTO users VALUES (1)",
            "MERGE INTO t USING s ON (t.id = s.id) WHEN MATCHED THEN UPDATE SET t.x = s.x",
            "SELECT * FROM users FOR UPDATE",
            "select * from users lock in share mode",
            "SELECT * INTO new_table FROM users",
            "SELECT * FROM users INTO OUTFILE '/tmp/x'",
            "WITH gone AS (DELETE FROM users RETURNING *) SELECT * FROM gone",
            "SELECT nextval('seq')",
            "SELECT sleep(100)",
            "SELECT pg_sleep(5)",
            "call cleanup()",
            "BEGIN DELETE FROM t; END;",
            "SET GLOBAL max_connections = 1",
            "EXPLAIN ANALYZE DELETE FROM users",
            "SELECT DBMS_RANDOM.VALUE FROM dual",
            // Where the string ends depends on the server: never a read.
            "SELECT 'a\\'; DROP TABLE t; -- '",
            "",
        ] {
            assert!(verdict(sql).needs_approval(), "{sql}");
        }
    }

    #[test]
    fn schema_changes_are_ddl() {
        for sql in ["CREATE TABLE t (id int)", "drop table users", "ALTER TABLE users ADD x int", "truncate users", "GRANT ALL ON *.* TO x"]
        {
            assert_eq!(verdict(sql), Verdict::Ddl, "{sql}");
        }
    }

    #[test]
    fn tricks_never_pass_as_reads() {
        for sql in [
            // A second statement.
            "SELECT 1; DROP TABLE users",
            "select 1;delete from users",
            // MySQL runs /*! … */; an Oracle hint is code for the optimizer.
            "SELECT 1 /*! ; DROP TABLE users */",
            "SELECT /*+ PARALLEL */ * FROM t",
            // Unterminated quote or comment.
            "SELECT 'abc",
            "SELECT 1 /* never closed",
            // Dollar-quoted code.
            "SELECT $$ DELETE FROM t $$",
            // Case games.
            "SeLeCt * FrOm t FoR uPdAtE",
        ] {
            assert!(verdict(sql).needs_approval(), "{sql}");
        }
        // Keywords inside strings and quoted names don't count either way.
        assert_eq!(verdict("SELECT 'DROP TABLE x' FROM t"), Verdict::Read);
    }

    /// A comment in one dialect is arithmetic or an operator in another: reading a statement with
    /// the wrong dialect's rules hides whatever follows from the guard, while the server still runs it.
    #[test]
    fn a_comment_the_engine_ignores_cannot_hide_a_statement() {
        for engine in [Engine::MySql, Engine::MariaDb] {
            // The server needs whitespace after `--`; `1--1` is `1 - (-1)`, so the rest still runs.
            assert!(classify_sql(engine, "SELECT 1--1;COMMIT;DELETE FROM users").needs_approval());
            assert!(classify_sql(engine, "SELECT 1--1;DROP TABLE users").needs_approval());
            // MariaDB runs `/*M! … */` the way MySQL runs `/*! … */`.
            assert!(classify_sql(engine, "SELECT 1 /*M!99999 ; DROP TABLE users */").needs_approval());
            // With whitespace it really is a comment.
            assert_eq!(classify_sql(engine, "SELECT 1 -- 1;DELETE FROM users"), Verdict::Read);
            // `#` is a comment here.
            assert_eq!(classify_sql(engine, "SELECT 1#2"), Verdict::Read);
        }
        for engine in [Engine::Postgres, Engine::Oracle] {
            // `#` is an operator (PostgreSQL) or a name character (Oracle), never a comment.
            assert!(classify_sql(engine, "SELECT 1#2;COMMIT;DROP TABLE users").needs_approval());
            // `--` is a comment with or without whitespace.
            assert_eq!(classify_sql(engine, "SELECT 1--1"), Verdict::Read);
        }
    }

    /// One more paren used to make a call invisible to the reader.
    #[test]
    fn a_paren_does_not_hide_a_call() {
        for sql in [
            "SELECT (sys_eval('id'))",
            "SELECT ((my_writer(1)))",
            "SELECT COUNT(dblink_exec('dbname=x', 'DELETE FROM t'))",
            "SELECT MAX(my_autonomous_writer(id)) FROM t",
        ] {
            assert!(verdict(sql).needs_approval(), "{sql}");
        }
    }

    /// `$` and `#` belong to the name around them: blanking them leaves an allowed name behind.
    #[test]
    fn name_characters_are_not_blanked() {
        for engine in SQL_ENGINES {
            assert!(classify_sql(engine, "SELECT evil$count(1)").needs_approval(), "{engine:?}");
        }
        assert!(classify_sql(Engine::Oracle, "SELECT sys#evil_fn(1) FROM dual").needs_approval());
        assert!(classify_sql(Engine::Postgres, "SELECT evil#fn(1)").needs_approval());
    }

    /// A write word glued to a `.` is still a write word.
    #[test]
    fn write_words_count_after_a_dot() {
        assert!(verdict("SELECT orders_seq.NEXTVAL FROM dual").needs_approval());
        assert!(verdict("SELECT my_pkg.EXECUTE FROM dual").needs_approval());
        // And a word that only contains one isn't: `nextvalue` is a column.
        assert_eq!(verdict("SELECT nextvalue FROM t"), Verdict::Read);
    }

    /// EXPLAIN that really runs the statement (or writes a plan) asks first.
    #[test]
    fn explain_that_runs_the_statement_asks() {
        assert!(classify_sql(Engine::Postgres, "EXPLAIN (ANALYZE) SELECT 1").needs_approval());
        assert!(classify_sql(Engine::Postgres, "EXPLAIN (ANALYZE, BUFFERS) SELECT 1").needs_approval());
        assert!(classify_sql(Engine::Oracle, "EXPLAIN PLAN FOR SELECT 1 FROM dual").needs_approval());
        assert_eq!(classify_sql(Engine::Postgres, "EXPLAIN SELECT 1"), Verdict::Read);
    }

    #[test]
    fn mongo_reads_and_writes() {
        assert_eq!(classify_mongo(MongoOp::Find, None), Verdict::Read);
        let read = serde_json::json!([{ "$match": { "a": 1 } }, { "$group": { "_id": "$a" } }]);
        assert_eq!(classify_mongo(MongoOp::Aggregate, Some(&read)), Verdict::Read);
        let out = serde_json::json!([{ "$match": {} }, { "$out": "copy" }]);
        assert_eq!(classify_mongo(MongoOp::Aggregate, Some(&out)), Verdict::Write);
        let merge = serde_json::json!([{ "$merge": { "into": "x" } }]);
        assert_eq!(classify_mongo(MongoOp::Aggregate, Some(&merge)), Verdict::Write);
        let unknown = serde_json::json!([{ "$someNewStage": {} }]);
        assert_eq!(classify_mongo(MongoOp::Aggregate, Some(&unknown)), Verdict::Write);
        // A filter that runs code on the server is not a plain read.
        let js = serde_json::json!({ "$where": "this.a == 1" });
        assert_eq!(classify_mongo(MongoOp::Find, Some(&js)), Verdict::Write);
        let nested = serde_json::json!({ "a": { "$function": { "body": "…" } } });
        assert_eq!(classify_mongo(MongoOp::Count, Some(&nested)), Verdict::Write);
        let plain = serde_json::json!({ "age": 30 });
        assert_eq!(classify_mongo(MongoOp::Find, Some(&plain)), Verdict::Read);
        assert_eq!(classify_mongo(MongoOp::DeleteMany, None), Verdict::Write);
        assert_eq!(classify_mongo(MongoOp::DropCollection, None), Verdict::Ddl);
        assert_eq!(MongoOp::parse("insertMany"), Some(MongoOp::InsertMany));
        assert_eq!(MongoOp::parse("eval"), None);
    }
}
