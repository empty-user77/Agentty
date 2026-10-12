---
title: Databases
description: The databases your project already configures, browsable from Agentty — and queryable by your agents, with writes gated behind your approval.
---

Agentty finds the databases a project is configured for, shows them on a page beside your terminals, and lets agents query them. Reads run immediately; anything that could change data waits for you to press **Execute**.

## How connections are found

Agentty reads the project's **own** configuration — it never writes anything into your project:

| Source | What is read |
|---|---|
| `.env` files | Connection URLs and host / user / password / database variables |
| Spring | `application.properties` / `application-<profile>.yml` |
| Prisma | The `datasource` block |
| Compose | Services using MySQL, MariaDB, PostgreSQL, MongoDB or Oracle images |

References like `${VAR}`, `${VAR:default}`, `env("VAR")` and `$VAR` are resolved from the project's `.env` files. If a value stays a reference — it comes from a secret manager, or an environment variable set elsewhere — that field is simply shown as missing, and you fill it in once.

Supported engines are MySQL, MariaDB, PostgreSQL, MongoDB and Oracle. Oracle additionally needs Oracle Instant Client installed.

## Filling in what's missing

- **Enter password** on the database page stores it in the operating system's credential store, never in a file. The connection id is derived from the project and the address, so the password follows the connection even if the configuration file changes.
- **Add** defines a connection by hand (engine, host, port, database, user, password). It is saved to `~/.agentty/db-connections.json` with owner-only permissions — **without** the password.

## What agents may run, and what they may not

This is the part worth understanding, because it is what makes handing a database to an agent reasonable.

Every statement is classified before it runs:

- A **read** — a single statement starting with `SELECT`, `SHOW`, `DESCRIBE`, `EXPLAIN`, `WITH … SELECT`, `VALUES` or `TABLE`, containing no write keyword — runs immediately and shows its rows.
- **Everything else queues for approval.** The page shows who asked, which connection, what kind of statement and the exact text. It runs only when you press **Execute**. The agent's command waits for your answer for up to 15 minutes.

The classifier is deliberately suspicious: several statements at once, executable comments, dollar quoting, an unterminated quote, or a call to anything but a known pure built-in (`COUNT`, `COALESCE`, `DATE_FORMAT`, …) all need approval. Stored functions and extensions can write even inside a read-only transaction, so they are never auto-approved.

For MongoDB, `find`, `count`, `distinct`, `listCollections` and aggregations built only from known read stages count as reads; `$out`, `$merge` or an unknown stage need approval.

### A second layer

Reads also run **inside a read-only transaction that is rolled back** (`START TRANSACTION READ ONLY`, `BEGIN READ ONLY`, `SET TRANSACTION READ ONLY`). If the classifier ever got something wrong, the database itself still refuses the write.

Results are capped — 200 rows on the page, 500 for agents, cells cut at 4000 characters — so a stray `SELECT *` cannot flood a pane.

## Connections and TLS

TLS is off for `localhost`, `127.0.0.1` and `::1`, or when the configuration explicitly disables it (`sslmode=disable`, `useSSL=false`). Otherwise it is required and verified against the system roots plus a bundled Amazon RDS certificate bundle, so RDS endpoints work without extra setup. Use the instance endpoint as the host.

Passwords never appear in a reply, on the page, or in a log. Connection errors name only the host, user and database.

## Using it

The database chip and page follow the working tree of the active pane, like the Docker panel. Run a query in the box on the page, or let an agent use `agentty db` — both go through the same classification and approval path.

## Relationship diagram

**Diagram** above the rows draws every table of the connection as a card: its comment, and each column with its type and comment (a key icon marks the primary key). Lines join a column to the column it points at.

- **Foreign keys** the database declares are solid blue lines.
- **Inferred** relations are dashed orange lines: references the database does not declare but the names show — `user_settings.user_id` → `users.id`, `userId` → `users`, `parent_id` → the table itself, `emp_no` → the table whose key is `emp_no`. A line is drawn only when the referenced table and column exist and the two types could hold the same values. Click **Inferred** to hide them.

Referenced tables sit left of the tables that point at them. Drag a card by its name to move it, click it to bring out its lines, double-click it to open its rows. Zoom with the buttons at the bottom right or ⌘ + scroll; **Fit** shows every table. Clicking a table in the list finds it in the diagram. MongoDB collections are read from a few sample documents, so their links are always inferred.
