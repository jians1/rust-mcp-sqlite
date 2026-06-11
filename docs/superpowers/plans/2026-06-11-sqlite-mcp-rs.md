# SQLite MCP RS Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `sqlite-mcp-rs`, a Rust SQLite MCP server using Streamable HTTP, optional Bearer auth, one SQLite database file, and readonly/readwrite modes.

**Architecture:** The crate exposes a small library plus one binary. HTTP and MCP concerns stay in `mcp.rs` and `auth.rs`; all SQL work goes through one worker-thread SQLite connection owned by `SqliteExecutor`. SQL calls are serialized through a channel, wrapped in a service-owned transaction, and shaped into a stable JSON envelope.

**Tech Stack:** Rust stable, `rmcp` 1.7 Streamable HTTP server transport, Axum 0.8, Tokio 1.x, `rusqlite` 0.40 with bundled SQLite, `clap`, `serde`, `tracing`, `thiserror`, `base64`, `fallible-iterator`.

---

## Current Context

The repository currently contains only the approved design spec:

- `docs/superpowers/specs/2026-06-11-sqlite-mcp-rs-design.md`

The current shell does not have `cargo` or `rustc` installed. Execution must begin by installing or activating a Rust toolchain before running tests.

## File Structure

Create or modify these files:

- `Cargo.toml`: crate metadata and dependencies.
- `src/lib.rs`: module exports for tests and binary.
- `src/main.rs`: CLI entrypoint, logging, executor startup, HTTP server startup.
- `src/config.rs`: `Cli`, `RunMode`, runtime config conversion.
- `src/error.rs`: `AppError` and conversions.
- `src/response.rs`: `ExecuteSqlRequest`, `ExecuteSqlResponse`, `StatementResult`, `SqlErrorBody`.
- `src/sql_classify.rs`: comment-aware SQL keyword classifier, transaction-control detection, readonly helpers.
- `src/sqlite.rs`: `SqliteExecutor`, worker thread, transaction execution, multi-statement parsing, SQLite value mapping, timeout, FTS5 check.
- `src/auth.rs`: optional Bearer token middleware and tests.
- `src/mcp.rs`: `SqliteMcpServer`, `execute_sql` tool, Streamable HTTP service builder.
- `tests/config_cli.rs`: CLI behavior tests.
- `tests/sql_classify.rs`: classifier behavior tests.
- `tests/sqlite_executor.rs`: executor integration tests with temp databases.
- `tests/auth_http.rs`: auth middleware tests.
- `tests/mcp_http.rs`: Streamable HTTP MCP tests.

Public API contracts to keep stable across tasks:

```rust
// src/config.rs
#[derive(Clone, Copy, Debug, Eq, PartialEq, clap::ValueEnum)]
pub enum RunMode {
    Readonly,
    Readwrite,
}

#[derive(Clone, Debug, clap::Parser)]
#[command(name = "sqlite-mcp-rs")]
pub struct Cli {
    #[arg(long)]
    pub db: std::path::PathBuf,
    #[arg(long, default_value = "127.0.0.1")]
    pub host: std::net::IpAddr,
    #[arg(long, default_value_t = 3000)]
    pub port: u16,
    #[arg(long, value_enum, default_value_t = RunMode::Readwrite)]
    pub mode: RunMode,
    #[arg(long)]
    pub auth_token: Option<String>,
    #[arg(long, default_value_t = 500)]
    pub max_rows: usize,
    #[arg(long, default_value_t = 10_000)]
    pub timeout_ms: u64,
}
```

```rust
// src/response.rs
#[derive(Debug, serde::Deserialize)]
pub struct ExecuteSqlRequest {
    pub sql: String,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct ExecuteSqlResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<SqlErrorBody>,
    pub results: Vec<StatementResult>,
    pub elapsed_ms: u128,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct SqlErrorBody {
    pub message: String,
    pub statement_index: usize,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(untagged)]
pub enum StatementResult {
    Query(QueryResult),
    Insert(InsertResult),
    Affected(AffectedResult),
    Schema(SchemaResult),
    Success(SuccessResult),
}
```

```rust
// src/sqlite.rs
#[derive(Clone)]
pub struct SqliteExecutor {
    tx: tokio::sync::mpsc::Sender<ExecuteJob>,
}

#[derive(Clone, Debug)]
pub struct ExecutorConfig {
    pub db_path: std::path::PathBuf,
    pub mode: crate::config::RunMode,
    pub max_rows: usize,
    pub timeout_ms: u64,
}

impl SqliteExecutor {
    pub fn open(config: ExecutorConfig) -> Result<Self, crate::error::AppError>;
    pub async fn execute(&self, sql: String) -> crate::response::ExecuteSqlResponse;
}
```

## Task 1: Toolchain And Crate Scaffold

**Files:**
- Create: `Cargo.toml`
- Create: `src/lib.rs`
- Create: `src/main.rs`
- Test: toolchain commands

- [ ] **Step 1: Verify or install Rust**

Run:

```bash
command -v cargo || command -v rustup || true
```

Expected in the current environment before installing Rust: the command exits successfully and prints nothing.

Install Rust stable when absent:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
. "$HOME/.cargo/env"
rustup default stable
```

Expected after installation:

```text
stable-x86_64-unknown-linux-gnu installed
```

- [ ] **Step 2: Create `Cargo.toml`**

Use this dependency set:

```toml
[package]
name = "sqlite-mcp-rs"
version = "0.1.0"
edition = "2024"
license = "MIT"
publish = false

[dependencies]
axum = "0.8"
base64 = "0.22"
clap = { version = "4.5", features = ["derive"] }
fallible-iterator = "0.3"
http = "1"
rmcp = { version = "1.7", features = ["server", "macros", "transport-streamable-http-server"] }
rusqlite = { version = "0.40", features = ["bundled", "functions", "modern_sqlite"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
tokio = { version = "1", features = ["macros", "rt-multi-thread", "signal", "sync", "net"] }
tokio-util = "0.7"
tower = "0.5"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "fmt"] }

[dev-dependencies]
assert_cmd = "2"
predicates = "3"
reqwest = { version = "0.12", features = ["json", "stream"] }
tempfile = "3"
tower = { version = "0.5", features = ["util"] }
```

- [ ] **Step 3: Create module skeleton**

Create `src/lib.rs`:

```rust
pub mod auth;
pub mod config;
pub mod error;
pub mod mcp;
pub mod response;
pub mod sql_classify;
pub mod sqlite;
```

Create `src/main.rs`:

```rust
fn main() {
    eprintln!("sqlite-mcp-rs scaffold is not wired yet");
}
```

- [ ] **Step 4: Run scaffold verification**

Run:

```bash
cargo metadata --format-version 1 >/tmp/sqlite-mcp-rs-metadata.json
cargo check
```

Expected after only skeleton files exist:

```text
Finished `dev` profile
```

- [ ] **Step 5: Commit scaffold**

```bash
git add Cargo.toml src/lib.rs src/main.rs
git commit -m "chore: scaffold rust crate"
```

## Task 2: Config, Response, And Error Types

**Files:**
- Create: `src/config.rs`
- Create: `src/error.rs`
- Create: `src/response.rs`
- Create: `tests/config_cli.rs`
- Modify: `src/main.rs`
- Test: `cargo test --test config_cli`

- [ ] **Step 1: Write failing CLI tests**

Create `tests/config_cli.rs`:

```rust
use clap::Parser;
use sqlite_mcp_rs::config::{Cli, RunMode};

#[test]
fn cli_defaults_match_spec() {
    let cli = Cli::parse_from(["sqlite-mcp-rs", "--db", "/tmp/app.db"]);

    assert_eq!(cli.db.to_string_lossy(), "/tmp/app.db");
    assert_eq!(cli.host.to_string(), "127.0.0.1");
    assert_eq!(cli.port, 3000);
    assert_eq!(cli.mode, RunMode::Readwrite);
    assert_eq!(cli.auth_token, None);
    assert_eq!(cli.max_rows, 500);
    assert_eq!(cli.timeout_ms, 10_000);
}

#[test]
fn cli_accepts_readonly_and_overrides() {
    let cli = Cli::parse_from([
        "sqlite-mcp-rs",
        "--db",
        "/data/app.db",
        "--host",
        "0.0.0.0",
        "--port",
        "3100",
        "--mode",
        "readonly",
        "--auth-token",
        "secret-value",
        "--max-rows",
        "25",
        "--timeout-ms",
        "1500",
    ]);

    assert_eq!(cli.host.to_string(), "0.0.0.0");
    assert_eq!(cli.port, 3100);
    assert_eq!(cli.mode, RunMode::Readonly);
    assert_eq!(cli.auth_token.as_deref(), Some("secret-value"));
    assert_eq!(cli.max_rows, 25);
    assert_eq!(cli.timeout_ms, 1500);
}

#[test]
fn cli_requires_db() {
    let err = Cli::try_parse_from(["sqlite-mcp-rs"]).unwrap_err();
    assert!(err.to_string().contains("--db"));
}
```

- [ ] **Step 2: Run tests and verify red**

Run:

```bash
cargo test --test config_cli
```

Expected:

```text
error[E0432]: unresolved import `sqlite_mcp_rs::config`
```

- [ ] **Step 3: Implement config, errors, and response structs**

Implement `src/config.rs` with the public API shown in the File Structure section.

Implement `src/error.rs`:

```rust
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Message(String),
}
```

Implement `src/response.rs` with:

```rust
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Deserialize)]
pub struct ExecuteSqlRequest {
    pub sql: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct ExecuteSqlResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<SqlErrorBody>,
    pub results: Vec<StatementResult>,
    pub elapsed_ms: u128,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct SqlErrorBody {
    pub message: String,
    pub statement_index: usize,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum StatementResult {
    Query(QueryResult),
    Insert(InsertResult),
    Affected(AffectedResult),
    Schema(SchemaResult),
    Success(SuccessResult),
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct QueryResult {
    pub statement_index: usize,
    pub statement_type: String,
    pub columns: Vec<String>,
    pub rows: Vec<Map<String, Value>>,
    pub row_count: usize,
    pub truncated: bool,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct InsertResult {
    pub statement_index: usize,
    pub statement_type: String,
    pub affected_rows: usize,
    pub last_insert_rowid: i64,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct AffectedResult {
    pub statement_index: usize,
    pub statement_type: String,
    pub affected_rows: usize,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct SchemaResult {
    pub statement_index: usize,
    pub statement_type: String,
    pub success: bool,
    pub schema_changed: bool,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct SuccessResult {
    pub statement_index: usize,
    pub statement_type: String,
    pub success: bool,
}
```

Update `src/main.rs` to parse CLI and print sanitized startup data:

```rust
use clap::Parser;
use sqlite_mcp_rs::config::Cli;

fn main() {
    let cli = Cli::parse();
    eprintln!(
        "sqlite-mcp-rs db={} host={} port={} mode={:?} max_rows={} timeout_ms={}",
        cli.db.display(),
        cli.host,
        cli.port,
        cli.mode,
        cli.max_rows,
        cli.timeout_ms
    );
}
```

- [ ] **Step 4: Run green verification**

Run:

```bash
cargo test --test config_cli
cargo check
```

Expected:

```text
test result: ok. 3 passed
Finished `dev` profile
```

- [ ] **Step 5: Commit**

```bash
git add src/config.rs src/error.rs src/response.rs src/main.rs tests/config_cli.rs
git commit -m "feat: add config and response contracts"
```

## Task 3: SQL Classification

**Files:**
- Create: `src/sql_classify.rs`
- Create: `tests/sql_classify.rs`
- Test: `cargo test --test sql_classify`

- [ ] **Step 1: Write failing classifier tests**

Create `tests/sql_classify.rs`:

```rust
use sqlite_mcp_rs::config::RunMode;
use sqlite_mcp_rs::sql_classify::{classify, is_forbidden_in_mode, StatementKind};

#[test]
fn classifies_after_whitespace_and_comments() {
    assert_eq!(classify("  -- comment\n SELECT 1"), StatementKind::Select);
    assert_eq!(classify("/* x */\nEXPLAIN QUERY PLAN SELECT 1"), StatementKind::Explain);
    assert_eq!(classify("insert into t values (1)"), StatementKind::Insert);
    assert_eq!(classify("PrAgMa table_info(users)"), StatementKind::Pragma);
}

#[test]
fn rejects_transaction_control() {
    for sql in ["BEGIN", "commit", "ROLLBACK", "savepoint x", "release x"] {
        let kind = classify(sql);
        assert!(kind.is_transaction_control(), "{sql} should be transaction control");
    }
}

#[test]
fn readonly_rejects_mutating_statements() {
    for sql in [
        "INSERT INTO t VALUES (1)",
        "UPDATE t SET x = 1",
        "DELETE FROM t",
        "CREATE TABLE t(x)",
        "DROP TABLE t",
        "ALTER TABLE t ADD COLUMN y",
        "VACUUM",
        "ANALYZE",
        "ATTACH DATABASE ':memory:' AS x",
        "DETACH DATABASE x",
        "PRAGMA user_version = 2",
    ] {
        assert!(
            is_forbidden_in_mode(classify(sql), sql, RunMode::Readonly),
            "{sql} should be forbidden in readonly"
        );
    }
}

#[test]
fn readonly_allows_read_statements() {
    for sql in [
        "SELECT 1",
        "EXPLAIN SELECT 1",
        "PRAGMA table_info(users)",
        "WITH cte AS (SELECT 1 AS x) SELECT x FROM cte",
    ] {
        assert!(
            !is_forbidden_in_mode(classify(sql), sql, RunMode::Readonly),
            "{sql} should be allowed in readonly"
        );
    }
}

#[test]
fn classifies_with_main_statement_when_obvious() {
    assert_eq!(
        classify("WITH cte AS (SELECT 1) SELECT * FROM cte"),
        StatementKind::Select
    );
    assert_eq!(
        classify("WITH cte AS (SELECT 1) INSERT INTO t SELECT * FROM cte"),
        StatementKind::Insert
    );
}
```

- [ ] **Step 2: Run tests and verify red**

Run:

```bash
cargo test --test sql_classify
```

Expected:

```text
error[E0432]: unresolved import `sqlite_mcp_rs::sql_classify`
```

- [ ] **Step 3: Implement classifier**

Implement `StatementKind`:

```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatementKind {
    Select,
    Explain,
    With,
    Insert,
    Update,
    Delete,
    Replace,
    Create,
    Drop,
    Alter,
    Pragma,
    Vacuum,
    Analyze,
    Attach,
    Detach,
    Begin,
    Commit,
    Rollback,
    Savepoint,
    Release,
    Other,
}
```

Implement:

```rust
pub fn classify(sql: &str) -> StatementKind;
pub fn is_forbidden_in_mode(kind: StatementKind, sql: &str, mode: RunMode) -> bool;
pub fn public_statement_type(kind: StatementKind) -> &'static str;
```

Rules:

- Ignore leading whitespace.
- Ignore leading line comments that start with `--` and continue through the next newline.
- Ignore leading block comments that start with `/*` and end at the next `*/`.
- Match the first keyword case-insensitively.
- For a statement starting with `WITH`, scan tokens outside strings and comments and track parenthesis depth. After the CTE prefix closes at depth zero, return the first of `SELECT`, `INSERT`, `UPDATE`, or `DELETE`; return `StatementKind::With` when that scan does not find a main statement.
- `StatementKind::is_transaction_control()` returns true for `Begin`, `Commit`, `Rollback`, `Savepoint`, and `Release`.
- In readonly mode, reject transaction control, write/DDL/maintenance statements, and `PRAGMA` statements containing `=`.

- [ ] **Step 4: Run green verification**

Run:

```bash
cargo test --test sql_classify
cargo test
```

Expected:

```text
test result: ok. 5 passed
```

- [ ] **Step 5: Commit**

```bash
git add src/sql_classify.rs tests/sql_classify.rs
git commit -m "feat: classify sqlite statements"
```

## Task 4: SQLite Executor Basic Query And Type Mapping

**Files:**
- Create: `src/sqlite.rs`
- Create: `tests/sqlite_executor.rs`
- Modify: `src/error.rs`
- Test: `cargo test --test sqlite_executor`

- [ ] **Step 1: Write failing executor tests**

Create `tests/sqlite_executor.rs`:

```rust
use serde_json::json;
use sqlite_mcp_rs::config::RunMode;
use sqlite_mcp_rs::response::StatementResult;
use sqlite_mcp_rs::sqlite::{ExecutorConfig, SqliteExecutor};

fn temp_db_path(name: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(name);
    (dir, path)
}

async fn executor(path: std::path::PathBuf, mode: RunMode, max_rows: usize) -> SqliteExecutor {
    SqliteExecutor::open(ExecutorConfig {
        db_path: path,
        mode,
        max_rows,
        timeout_ms: 10_000,
    })
    .unwrap()
}

#[tokio::test]
async fn basic_select_maps_sqlite_types() {
    let (_dir, path) = temp_db_path("types.db");
    let exec = executor(path, RunMode::Readwrite, 500).await;

    let response = exec
        .execute(
            "SELECT NULL AS n, 7 AS i, 1.5 AS r, 'text' AS t, x'6869' AS b".to_string(),
        )
        .await;

    assert!(response.success, "{response:?}");
    assert_eq!(response.results.len(), 1);
    let StatementResult::Query(result) = &response.results[0] else {
        panic!("expected query result");
    };
    assert_eq!(result.statement_index, 0);
    assert_eq!(result.statement_type, "SELECT");
    assert_eq!(result.columns, vec!["n", "i", "r", "t", "b"]);
    assert_eq!(result.row_count, 1);
    assert!(!result.truncated);
    assert_eq!(
        result.rows[0],
        serde_json::Map::from_iter([
            ("n".to_string(), serde_json::Value::Null),
            ("i".to_string(), json!(7)),
            ("r".to_string(), json!(1.5)),
            ("t".to_string(), json!("text")),
            (
                "b".to_string(),
                json!({"type": "blob", "encoding": "base64", "data": "aGk="}),
            ),
        ])
    );
}

#[tokio::test]
async fn schema_and_insert_and_update_have_expected_shapes() {
    let (_dir, path) = temp_db_path("write.db");
    let exec = executor(path, RunMode::Readwrite, 500).await;

    let response = exec
        .execute(
            "CREATE TABLE users(id INTEGER PRIMARY KEY, name TEXT);
             INSERT INTO users(name) VALUES ('foo');
             UPDATE users SET name = 'bar' WHERE id = 1;"
                .to_string(),
        )
        .await;

    assert!(response.success, "{response:?}");
    assert_eq!(response.results.len(), 3);
    assert!(matches!(response.results[0], StatementResult::Schema(_)));
    let StatementResult::Insert(insert) = &response.results[1] else {
        panic!("expected insert result");
    };
    assert_eq!(insert.affected_rows, 1);
    assert_eq!(insert.last_insert_rowid, 1);
    let StatementResult::Affected(update) = &response.results[2] else {
        panic!("expected affected result");
    };
    assert_eq!(update.statement_type, "UPDATE");
    assert_eq!(update.affected_rows, 1);
}
```

- [ ] **Step 2: Run tests and verify red**

Run:

```bash
cargo test --test sqlite_executor
```

Expected:

```text
error[E0432]: unresolved import `sqlite_mcp_rs::sqlite`
```

- [ ] **Step 3: Implement executor skeleton and value mapping**

Implement `SqliteExecutor::open`:

- Open with `OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE` in readwrite mode.
- Open with `OpenFlags::SQLITE_OPEN_READ_ONLY` in readonly mode.
- Run FTS5 self-check before spawning the worker thread.
- Move `rusqlite::Connection` into one `std::thread::spawn` worker.
- Use `tokio::sync::mpsc` for jobs and `tokio::sync::oneshot` for results.

Inside the worker:

- For each job, record `Instant::now()`.
- Call `execute_on_connection`.
- Return `ExecuteSqlResponse`.

Use `rusqlite::Batch` and `fallible_iterator::FallibleIterator`:

```rust
let mut batch = rusqlite::Batch::new(conn, sql);
while let Some(mut stmt) = batch.next()? {
    let expanded = stmt.expanded_sql().unwrap_or_default();
    let kind = crate::sql_classify::classify(&expanded);
    let column_count = stmt.column_count();
    if column_count > 0 {
        // collect query rows
    } else {
        // execute non-query
    }
}
```

Map row values with `row.get_ref(index)?` and `rusqlite::types::ValueRef`:

- `Null` to `serde_json::Value::Null`
- `Integer(i)` to `json!(i)`
- `Real(f)` to `json!(f)`
- `Text(bytes)` to `String::from_utf8_lossy(bytes)`
- `Blob(bytes)` to base64 using `base64::engine::general_purpose::STANDARD`

- [ ] **Step 4: Run green verification**

Run:

```bash
cargo test --test sqlite_executor
```

Expected:

```text
test result: ok. 2 passed
```

- [ ] **Step 5: Commit**

```bash
git add src/sqlite.rs src/error.rs tests/sqlite_executor.rs
git commit -m "feat: execute sqlite statements"
```

## Task 5: Transactions, Rollback, Multi-Statement Semantics

**Files:**
- Modify: `src/sqlite.rs`
- Modify: `tests/sqlite_executor.rs`
- Test: `cargo test --test sqlite_executor`

- [ ] **Step 1: Add failing transaction tests**

Append to `tests/sqlite_executor.rs`:

```rust
#[tokio::test]
async fn transaction_rolls_back_all_statements_on_error() {
    let (_dir, path) = temp_db_path("rollback.db");
    let exec = executor(path, RunMode::Readwrite, 500).await;

    let create = exec
        .execute("CREATE TABLE users(id INTEGER PRIMARY KEY, name TEXT UNIQUE);".to_string())
        .await;
    assert!(create.success, "{create:?}");

    let failed = exec
        .execute(
            "INSERT INTO users(name) VALUES ('foo');
             INSERT INTO users(name) VALUES ('foo');"
                .to_string(),
        )
        .await;
    assert!(!failed.success);
    assert_eq!(failed.results.len(), 0);
    assert_eq!(failed.error.as_ref().unwrap().statement_index, 1);

    let check = exec.execute("SELECT COUNT(*) AS c FROM users;".to_string()).await;
    let StatementResult::Query(result) = &check.results[0] else {
        panic!("expected query result");
    };
    assert_eq!(result.rows[0]["c"], json!(0));
}

#[tokio::test]
async fn multiple_selects_return_multiple_results() {
    let (_dir, path) = temp_db_path("multi_select.db");
    let exec = executor(path, RunMode::Readwrite, 500).await;

    let response = exec.execute("SELECT 1 AS a; SELECT 2 AS b;".to_string()).await;

    assert!(response.success, "{response:?}");
    assert_eq!(response.results.len(), 2);
    let StatementResult::Query(first) = &response.results[0] else {
        panic!("expected query result");
    };
    let StatementResult::Query(second) = &response.results[1] else {
        panic!("expected query result");
    };
    assert_eq!(first.rows[0]["a"], json!(1));
    assert_eq!(second.rows[0]["b"], json!(2));
}

#[tokio::test]
async fn explicit_transaction_control_is_rejected() {
    let (_dir, path) = temp_db_path("transaction_control.db");
    let exec = executor(path, RunMode::Readwrite, 500).await;

    for sql in ["BEGIN", "COMMIT", "ROLLBACK", "SAVEPOINT x", "RELEASE x"] {
        let response = exec.execute(sql.to_string()).await;
        assert!(!response.success, "{sql} should fail");
        assert_eq!(response.results.len(), 0);
        assert!(
            response
                .error
                .as_ref()
                .unwrap()
                .message
                .contains("transaction control statements are not allowed"),
            "{response:?}"
        );
    }
}
```

- [ ] **Step 2: Run tests and verify red**

Run:

```bash
cargo test --test sqlite_executor
```

Expected before transaction logic:

```text
FAILED transaction_rolls_back_all_statements_on_error
```

- [ ] **Step 3: Implement service-owned transaction**

In `execute_on_connection`:

- Reject empty or whitespace-only SQL with `success: false`.
- Start with `conn.execute_batch("BEGIN")`.
- On every failure, call `conn.execute_batch("ROLLBACK")` before returning.
- On success, call `conn.execute_batch("COMMIT")`.
- Keep results in a temporary vector and discard them on failure.
- Track `statement_index` as the count of non-empty statements yielded by `Batch`.
- Before execution, classify `stmt.expanded_sql().unwrap_or_default()` and reject transaction-control statements with the exact message from the spec.

- [ ] **Step 4: Run green verification**

Run:

```bash
cargo test --test sqlite_executor
```

Expected:

```text
test result: ok
```

- [ ] **Step 5: Commit**

```bash
git add src/sqlite.rs tests/sqlite_executor.rs
git commit -m "feat: wrap sql calls in transactions"
```

## Task 6: Resource Limits, Timeout, Readonly, And FTS5

**Files:**
- Modify: `src/sqlite.rs`
- Modify: `tests/sqlite_executor.rs`
- Test: `cargo test --test sqlite_executor`

- [ ] **Step 1: Add failing resource and mode tests**

Append to `tests/sqlite_executor.rs`:

```rust
#[tokio::test]
async fn max_rows_truncates_query_results() {
    let (_dir, path) = temp_db_path("max_rows.db");
    let exec = executor(path, RunMode::Readwrite, 2).await;

    let response = exec
        .execute(
            "CREATE TABLE nums(n INTEGER);
             INSERT INTO nums(n) VALUES (1), (2), (3);
             SELECT n FROM nums ORDER BY n;"
                .to_string(),
        )
        .await;

    assert!(response.success, "{response:?}");
    let StatementResult::Query(query) = &response.results[2] else {
        panic!("expected query result");
    };
    assert_eq!(query.row_count, 2);
    assert!(query.truncated);
    assert_eq!(query.rows[0]["n"], json!(1));
    assert_eq!(query.rows[1]["n"], json!(2));
}

#[tokio::test]
async fn readonly_allows_reads_and_rejects_writes() {
    let (_dir, path) = temp_db_path("readonly.db");
    {
        let setup = executor(path.clone(), RunMode::Readwrite, 500).await;
        let response = setup
            .execute("CREATE TABLE users(id INTEGER); INSERT INTO users VALUES (1);".to_string())
            .await;
        assert!(response.success, "{response:?}");
    }

    let readonly = executor(path, RunMode::Readonly, 500).await;
    let read = readonly.execute("SELECT id FROM users;".to_string()).await;
    assert!(read.success, "{read:?}");

    let write = readonly.execute("INSERT INTO users VALUES (2);".to_string()).await;
    assert!(!write.success);
    assert!(
        write
            .error
            .as_ref()
            .unwrap()
            .message
            .contains("readonly mode forbids")
    );
}

#[tokio::test]
async fn timeout_interrupts_and_rolls_back() {
    let (_dir, path) = temp_db_path("timeout.db");
    let exec = SqliteExecutor::open(ExecutorConfig {
        db_path: path,
        mode: RunMode::Readwrite,
        max_rows: 500,
        timeout_ms: 1,
    })
    .unwrap();

    let response = exec
        .execute(
            "CREATE TABLE t(n INTEGER);
             INSERT INTO t(n) VALUES (1);
             WITH RECURSIVE cnt(x) AS (
               SELECT 1
               UNION ALL
               SELECT x + 1 FROM cnt WHERE x < 100000000
             )
             SELECT sum(x) FROM cnt;"
                .to_string(),
        )
        .await;

    assert!(!response.success, "{response:?}");
    assert!(
        response
            .error
            .as_ref()
            .unwrap()
            .message
            .contains("query timed out")
    );

    let check = exec
        .execute("SELECT name FROM sqlite_master WHERE type = 'table' AND name = 't';".to_string())
        .await;
    let StatementResult::Query(query) = &check.results[0] else {
        panic!("expected query result");
    };
    assert_eq!(query.row_count, 0);
}

#[tokio::test]
async fn fts5_is_available() {
    let (_dir, path) = temp_db_path("fts5.db");
    let exec = executor(path, RunMode::Readwrite, 500).await;

    let response = exec
        .execute(
            "CREATE VIRTUAL TABLE docs USING fts5(body);
             INSERT INTO docs(body) VALUES ('hello sqlite');
             SELECT rowid, body FROM docs WHERE docs MATCH 'sqlite';"
                .to_string(),
        )
        .await;

    assert!(response.success, "{response:?}");
    let StatementResult::Query(query) = &response.results[2] else {
        panic!("expected query result");
    };
    assert_eq!(query.row_count, 1);
    assert_eq!(query.rows[0]["body"], json!("hello sqlite"));
}
```

- [ ] **Step 2: Run tests and verify red**

Run:

```bash
cargo test --test sqlite_executor
```

Expected before this task:

```text
FAILED max_rows_truncates_query_results
FAILED readonly_allows_reads_and_rejects_writes
FAILED timeout_interrupts_and_rolls_back
```

- [ ] **Step 3: Implement limits and modes**

Implement:

- `collect_query_rows(stmt, max_rows)` reads up to `max_rows` rows, then attempts one additional `rows.next()?` to set `truncated`.
- `SqliteExecutor::open` runs FTS5 self-check:

```sql
CREATE VIRTUAL TABLE temp.__fts5_check USING fts5(x);
DROP TABLE temp.__fts5_check;
```

- Before executing each statement in readonly mode, reject when `is_forbidden_in_mode(kind, &expanded, RunMode::Readonly)` is true or when `!stmt.readonly()`.
- Install `conn.progress_handler(Some(1000), move || Instant::now() >= deadline)` before execution and clear it with `conn.progress_handler(None::<fn() -> bool>)` after each job.
- When deadline is exceeded, return `query timed out after <timeout_ms> ms`.

- [ ] **Step 4: Run green verification**

Run:

```bash
cargo test --test sqlite_executor
```

Expected:

```text
test result: ok
```

- [ ] **Step 5: Commit**

```bash
git add src/sqlite.rs tests/sqlite_executor.rs
git commit -m "feat: enforce sqlite limits and modes"
```

## Task 7: Bearer Auth Middleware

**Files:**
- Create: `src/auth.rs`
- Create: `tests/auth_http.rs`
- Test: `cargo test --test auth_http`

- [ ] **Step 1: Write failing auth tests**

Create `tests/auth_http.rs`:

```rust
use axum::{middleware::from_fn_with_state, routing::post, Router};
use http::{Request, StatusCode};
use sqlite_mcp_rs::auth::{require_auth, AuthState};
use tower::ServiceExt;

async fn ok_handler() -> &'static str {
    "ok"
}

#[tokio::test]
async fn auth_disabled_allows_request() {
    let app = Router::new()
        .route("/mcp", post(ok_handler))
        .layer(from_fn_with_state(AuthState::new(None), require_auth));

    let response = app
        .oneshot(Request::post("/mcp").body(axum::body::Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn auth_enabled_requires_matching_bearer_token() {
    let app = Router::new()
        .route("/mcp", post(ok_handler))
        .layer(from_fn_with_state(
            AuthState::new(Some("secret".to_string())),
            require_auth,
        ));

    for header in [None, Some("Basic secret"), Some("Bearer wrong")] {
        let mut builder = Request::post("/mcp");
        if let Some(value) = header {
            builder = builder.header("authorization", value);
        }
        let response = app
            .clone()
            .oneshot(builder.body(axum::body::Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    let response = app
        .oneshot(
            Request::post("/mcp")
                .header("authorization", "Bearer secret")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}
```

- [ ] **Step 2: Run tests and verify red**

Run:

```bash
cargo test --test auth_http
```

Expected:

```text
error[E0432]: unresolved import `sqlite_mcp_rs::auth::AuthState`
```

- [ ] **Step 3: Implement middleware**

Implement a concrete Axum middleware state and function:

```rust
use axum::body::Body;
use axum::extract::State;
use axum::http::{header::AUTHORIZATION, HeaderValue, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::sync::Arc;

#[derive(Clone)]
pub struct AuthState {
    token: Option<Arc<str>>,
}

impl AuthState {
    pub fn new(token: Option<String>) -> Self {
        Self {
            token: token.map(Arc::<str>::from),
        }
    }
}

pub async fn require_auth(
    State(state): State<AuthState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if let Some(expected) = state.token.as_deref() {
        let header = request.headers().get(AUTHORIZATION);
        if !bearer_matches(header, expected) {
            return StatusCode::UNAUTHORIZED.into_response();
        }
    }

    next.run(request).await
}

fn bearer_matches(header: Option<&HeaderValue>, expected: &str) -> bool {
    let Some(header) = header else {
        return false;
    };
    let Ok(value) = header.to_str() else {
        return false;
    };
    value.strip_prefix("Bearer ") == Some(expected)
}
```

Required behavior:

- If token is `None`, pass requests through.
- If token is `Some`, require `Authorization` exactly in `Bearer <token>` form.
- Return `StatusCode::UNAUTHORIZED` on missing or mismatch.
- Do not log the configured token or received header.

- [ ] **Step 4: Run green verification**

Run:

```bash
cargo test --test auth_http
cargo test
```

Expected:

```text
test result: ok. 2 passed
```

- [ ] **Step 5: Commit**

```bash
git add src/auth.rs tests/auth_http.rs
git commit -m "feat: add optional bearer auth"
```

## Task 8: MCP Tool And Streamable HTTP Server

**Files:**
- Create: `src/mcp.rs`
- Create: `tests/mcp_http.rs`
- Modify: `src/main.rs`
- Test: `cargo test --test mcp_http`

- [ ] **Step 1: Write failing MCP tests**

Create `tests/mcp_http.rs`:

```rust
use reqwest::Client;
use serde_json::{json, Value};
use sqlite_mcp_rs::config::RunMode;
use sqlite_mcp_rs::mcp::spawn_test_server;
use sqlite_mcp_rs::sqlite::{ExecutorConfig, SqliteExecutor};

#[tokio::test]
async fn mcp_lists_only_execute_sql_and_calls_it() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("mcp.db");
    let executor = SqliteExecutor::open(ExecutorConfig {
        db_path,
        mode: RunMode::Readwrite,
        max_rows: 500,
        timeout_ms: 10_000,
    })
    .unwrap();

    let server = spawn_test_server(executor, None).await.unwrap();
    let client = Client::new();

    let initialize: Value = client
        .post(server.url())
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "0.1.0"}
            }
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(initialize.get("result").is_some(), "{initialize}");

    let tools: Value = client
        .post(server.url())
        .json(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let tools_array = tools["result"]["tools"].as_array().unwrap();
    assert_eq!(tools_array.len(), 1);
    assert_eq!(tools_array[0]["name"], "execute_sql");

    let call: Value = client
        .post(server.url())
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "execute_sql",
                "arguments": {"sql": "SELECT 42 AS answer"}
            }
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let text = call["result"]["content"][0]["text"].as_str().unwrap();
    let envelope: Value = serde_json::from_str(text).unwrap();
    assert_eq!(envelope["success"], true);
    assert_eq!(envelope["results"][0]["rows"][0]["answer"], 42);
}
```

- [ ] **Step 2: Run tests and verify red**

Run:

```bash
cargo test --test mcp_http
```

Expected:

```text
error[E0432]: unresolved import `sqlite_mcp_rs::mcp::spawn_test_server`
```

- [ ] **Step 3: Implement MCP server**

Implement `SqliteMcpServer`:

- Holds `SqliteExecutor`.
- Exposes only `execute_sql`.
- Tool input is `ExecuteSqlRequest`.
- Tool output is one text content item containing serialized `ExecuteSqlResponse`.
- `tools/list` returns exactly one tool with name `execute_sql` and an input schema requiring `sql`.

Use `rmcp`:

- Implement `ServerHandler` directly or with `#[tool_router]` / `#[tool_handler]`.
- Build HTTP with `rmcp::transport::StreamableHttpService`.
- Use `StreamableHttpServerConfig::default().with_stateful_mode(false).with_json_response(true)` for simpler request/response tests.
- Mount the service at `/mcp` in production and at a test URL from `spawn_test_server`.

Implement test helper:

```rust
pub struct TestServer {
    addr: std::net::SocketAddr,
    shutdown: tokio_util::sync::CancellationToken,
}

impl TestServer {
    pub fn url(&self) -> String {
        format!("http://{}/mcp", self.addr)
    }
}

pub async fn spawn_test_server(
    executor: crate::sqlite::SqliteExecutor,
    auth_token: Option<String>,
) -> Result<TestServer, crate::error::AppError>;
```

- [ ] **Step 4: Run green verification**

Run:

```bash
cargo test --test mcp_http
cargo test
```

Expected:

```text
test result: ok. 1 passed
```

- [ ] **Step 5: Commit**

```bash
git add src/mcp.rs src/main.rs tests/mcp_http.rs
git commit -m "feat: expose execute_sql over mcp http"
```

## Task 9: Binary Startup, Logging, And Manual Smoke

**Files:**
- Modify: `src/main.rs`
- Modify: `tests/config_cli.rs`
- Test: `cargo test && cargo run -- --db /tmp/sqlite-mcp-rs-smoke.db --host 127.0.0.1 --port 3000 --mode readwrite --max-rows 500 --timeout-ms 10000`

- [ ] **Step 1: Add failing binary smoke tests**

Append to `tests/config_cli.rs`:

```rust
#[test]
fn binary_help_mentions_expected_flags() {
    let mut cmd = assert_cmd::Command::cargo_bin("sqlite-mcp-rs").unwrap();
    cmd.arg("--help")
        .assert()
        .success()
        .stdout(predicates::str::contains("--db"))
        .stdout(predicates::str::contains("--host"))
        .stdout(predicates::str::contains("--port"))
        .stdout(predicates::str::contains("--mode"))
        .stdout(predicates::str::contains("--auth-token"))
        .stdout(predicates::str::contains("--max-rows"))
        .stdout(predicates::str::contains("--timeout-ms"));
}
```

- [ ] **Step 2: Run tests and verify red**

Run:

```bash
cargo test --test config_cli binary_help_mentions_expected_flags
```

Expected before binary wiring is finished:

```text
FAILED binary_help_mentions_expected_flags
```

- [ ] **Step 3: Implement final `main.rs`**

`main.rs` must:

- Parse `Cli`.
- Initialize tracing with `EnvFilter`.
- Build `ExecutorConfig`.
- Call `SqliteExecutor::open`.
- Log `SQLite FTS5: enabled` after open succeeds.
- Build the MCP router with optional auth.
- Bind `TcpListener` to `(cli.host, cli.port)`.
- Serve until `tokio::signal::ctrl_c()`.
- Never log `cli.auth_token`.

Use:

```rust
#[tokio::main]
async fn main() -> Result<(), sqlite_mcp_rs::error::AppError> {
    use clap::Parser;
    use sqlite_mcp_rs::config::Cli;

    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let executor = sqlite_mcp_rs::sqlite::SqliteExecutor::open(
        sqlite_mcp_rs::sqlite::ExecutorConfig {
            db_path: cli.db.clone(),
            mode: cli.mode,
            max_rows: cli.max_rows,
            timeout_ms: cli.timeout_ms,
        },
    )?;

    tracing::info!("SQLite FTS5: enabled");
    tracing::info!(
        db = %cli.db.display(),
        host = %cli.host,
        port = cli.port,
        mode = ?cli.mode,
        max_rows = cli.max_rows,
        timeout_ms = cli.timeout_ms,
        "starting sqlite-mcp-rs"
    );

    let app = sqlite_mcp_rs::mcp::router(executor, cli.auth_token.clone())?;
    let addr = std::net::SocketAddr::new(cli.host, cli.port);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
```

Set `mcp::router` to return `Result<axum::Router, sqlite_mcp_rs::error::AppError>` so `main.rs` can pass the router directly to `axum::serve`.

- [ ] **Step 4: Run full verification**

Run:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

Expected:

```text
Finished `test` profile
test result: ok
```

- [ ] **Step 5: Manual smoke**

Run:

```bash
rm -f /tmp/sqlite-mcp-rs-smoke.db
MCP_AUTH_TOKEN=dev-secret cargo run -- \
  --db /tmp/sqlite-mcp-rs-smoke.db \
  --host 127.0.0.1 \
  --port 3000 \
  --mode readwrite \
  --auth-token "$MCP_AUTH_TOKEN" \
  --max-rows 500 \
  --timeout-ms 10000
```

Expected startup log includes:

```text
SQLite FTS5: enabled
starting sqlite-mcp-rs
```

Stop the server with Ctrl-C after verifying startup.

- [ ] **Step 6: Commit**

```bash
git add src/main.rs src/mcp.rs tests/config_cli.rs
git commit -m "feat: wire binary startup"
```

## Task 10: Documentation And Final Verification

**Files:**
- Create: `README.md`
- Test: full suite and smoke commands

- [ ] **Step 1: Create README**

Create `README.md` with:

````markdown
# sqlite-mcp-rs

Rust SQLite MCP server over Streamable HTTP.

## Run

```bash
sqlite-mcp-rs \
  --db /data/app.db \
  --host 127.0.0.1 \
  --port 3000 \
  --mode readwrite \
  --auth-token "$MCP_AUTH_TOKEN" \
  --max-rows 500 \
  --timeout-ms 10000
```

## Security

Use `--auth-token` for backend Bearer auth. Deploy behind Nginx or Caddy for HTTPS, domains, IP allowlists, and additional access controls. The backend defaults to `127.0.0.1`.

## Tool

The server exposes one MCP tool: `execute_sql`.

Input:

```json
{"sql": "SELECT 1"}
```

The response is a JSON envelope with `success`, `results`, and `elapsed_ms`.

## Modes

`readonly` opens SQLite read-only and rejects mutating statements.

`readwrite` allows legal SQLite SQL except explicit transaction control statements. Each tool call is wrapped in one transaction.
````

- [ ] **Step 2: Run final verification**

Run:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

Expected:

```text
Finished `test` profile
test result: ok
```

- [ ] **Step 3: Commit README**

```bash
git add README.md
git commit -m "docs: add usage guide"
```

- [ ] **Step 4: Check worktree**

Run:

```bash
git status --short
```

Expected: no output.

## Self-Review Checklist

- Spec coverage:
  - CLI flags covered by Tasks 1, 2, and 9.
  - Optional Bearer auth covered by Task 7 and MCP integration in Task 8.
  - Single database file and single connection executor covered by Tasks 4 and 5.
  - Readonly/readwrite covered by Tasks 3 and 6.
  - Multi-statement parsing through `rusqlite::Batch` covered by Tasks 4 and 5.
  - Transactions and rollback covered by Task 5.
  - Type mapping covered by Task 4.
  - `max_rows` and timeout covered by Task 6.
  - FTS5 self-check and usage covered by Task 6 and startup in Task 9.
  - Streamable HTTP MCP `execute_sql` covered by Task 8.
- Type consistency:
  - `RunMode`, `ExecuteSqlResponse`, `StatementResult`, `SqliteExecutor`, and `ExecutorConfig` are defined before use.
  - `AuthState`, `require_auth`, `router`, and `spawn_test_server` are introduced in their owning tasks before external use.
- Verification commands:
  - Each task has a red test command, a green test command, and a commit command.
