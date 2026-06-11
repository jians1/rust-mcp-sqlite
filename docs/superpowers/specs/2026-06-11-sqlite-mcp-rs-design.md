# sqlite-mcp-rs Design

## Goal

Build a Rust SQLite MCP server that runs on a server, exposes MCP over Streamable HTTP, and provides a single `execute_sql` tool for local or remote clients through an HTTPS reverse proxy.

The service is intended for a single SQLite database file per process. It defaults to listening on `127.0.0.1`, leaves TLS, domain routing, and public access control to Nginx or Caddy, and optionally enforces Bearer token authentication in the backend.

## Command Line Interface

The binary name is `sqlite-mcp-rs`.

Example:

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

Supported arguments:

```text
--db <path>                 required
--host <ip>                 default: 127.0.0.1
--port <port>               default: 3000
--mode <readonly|readwrite> default: readwrite
--auth-token <token>        optional
--max-rows <n>              default: 500
--timeout-ms <ms>           default: 10000
```

`--auth-token` must never be written to logs.

## Architecture

Use the Rust MCP SDK for MCP protocol handling and Streamable HTTP support. Use Axum and Tokio for the HTTP runtime if required by the selected MCP SDK integration. Use `rusqlite` with bundled SQLite and FTS5 support for database access.

The service has these responsibilities:

- `main.rs`: process entrypoint, configuration loading, startup checks, server startup.
- `config.rs`: CLI parsing and runtime configuration.
- `auth.rs`: optional Bearer token HTTP middleware.
- `mcp.rs`: MCP server and `execute_sql` tool registration.
- `sqlite.rs`: single-connection SQLite executor, transactions, multi-statement execution, value mapping.
- `sql_classify.rs`: statement classification, transaction-control rejection, readonly rejection.
- `response.rs`: stable JSON response structs.
- `error.rs`: typed internal errors and public error conversion.

HTTP may accept concurrent requests, but SQL execution is serialized through one SQLite executor connection. This preserves predictable transaction behavior and `last_insert_rowid` semantics for each `execute_sql` call.

## Transport And Deployment

The MCP transport is Streamable HTTP. The backend default bind address is `127.0.0.1`.

Recommended production deployment:

- `sqlite-mcp-rs` listens on `127.0.0.1:<port>`.
- Nginx or Caddy terminates HTTPS.
- Nginx or Caddy handles domain routing and may add IP allowlists or additional authentication.
- Backend Bearer token auth remains available as a defense-in-depth control.

The implementation only needs to conform to the MCP Streamable HTTP protocol. It does not target private behavior from a specific client.

## Authentication

If `--auth-token` is configured, every MCP HTTP request must include:

```http
Authorization: Bearer <token>
```

Requests with a missing header, malformed header, or mismatched token return HTTP `401`.

If `--auth-token` is not configured, backend authentication is disabled.

The token and full `Authorization` header must not appear in logs, traces, panic messages, or structured error fields.

## MCP Tool

Expose exactly one MCP tool:

```text
execute_sql
```

Input schema:

```json
{
  "sql": "string, required"
}
```

The tool accepts any legal SQLite SQL except explicit transaction-control statements. Supported SQLite features include:

- `SELECT`
- `EXPLAIN`
- `INSERT`
- `UPDATE`
- `DELETE`
- `CREATE`
- `DROP`
- `ALTER`
- `PRAGMA`
- common table expressions with `WITH`
- statements with `RETURNING`
- FTS5
- Multiple statements in one `sql` string

## Response Format

All successful calls return:

```json
{
  "success": true,
  "results": [],
  "elapsed_ms": 12
}
```

All failed calls return:

```json
{
  "success": false,
  "error": {
    "message": "no such table: users",
    "statement_index": 0
  },
  "results": [],
  "elapsed_ms": 3
}
```

If failure happens before a statement can be parsed, `statement_index` is `0`.

### Query Results

Statements that produce columns return rows. This includes `SELECT`, `EXPLAIN`, read-style `PRAGMA`, CTE queries that begin with `WITH`, and mutating statements with `RETURNING`.

Example query result:

```json
{
  "statement_index": 0,
  "statement_type": "SELECT",
  "columns": ["id", "name"],
  "rows": [
    {"id": 1, "name": "foo"}
  ],
  "row_count": 1,
  "truncated": false
}
```

### Insert Results

`INSERT` returns:

```json
{
  "statement_index": 0,
  "statement_type": "INSERT",
  "affected_rows": 1,
  "last_insert_rowid": 42
}
```

### Update And Delete Results

`UPDATE` and `DELETE` return:

```json
{
  "statement_index": 0,
  "statement_type": "UPDATE",
  "affected_rows": 3
}
```

### Schema Change Results

`CREATE`, `DROP`, and `ALTER` return:

```json
{
  "statement_index": 0,
  "statement_type": "CREATE",
  "success": true,
  "schema_changed": true
}
```

Other legal non-query statements may return a generic success result with `statement_index`, `statement_type`, `success`, and any relevant affected row information.

## Transaction Strategy

Every `execute_sql` call is wrapped in one transaction by default.

Execution flow:

1. Validate that `sql` is non-empty.
2. Start a transaction on the executor connection.
3. Parse and execute each statement in order.
4. Commit only after all statements succeed.
5. Roll back if parsing, execution, row collection, timeout, or interruption fails.

Multiple statements in one call are one atomic unit. If any statement fails, the whole call rolls back and `results` is returned as an empty array.

Explicit transaction-control statements are forbidden because they would conflict with the service-owned transaction boundary. The forbidden set includes:

- `BEGIN`
- `COMMIT`
- `ROLLBACK`
- `SAVEPOINT`
- `RELEASE`

When one of these statements is found, execution fails with a clear message:

```text
transaction control statements are not allowed; execute_sql wraps each call in a transaction
```

## Multi-Statement Parsing

The implementation must not split SQL by semicolon. It must use SQLite prepare and consume behavior to parse one statement at a time.

Each parsed statement produces one result entry on success. If a multi-statement call contains multiple queries, each query returns its own columns and rows.

Empty statements and whitespace between statements are ignored.

## Statement Classification

Statement classification is based on the first effective SQL keyword after whitespace and comments. CTE statements that begin with `WITH` must first try to identify the main statement after the CTE prefix as `SELECT`, `INSERT`, `UPDATE`, or `DELETE`. If that cannot be determined without a full SQL parser, the classifier must use `WITH` as the public `statement_type`, and execution must still rely on SQLite prepared statement metadata to decide whether rows are returned.

Required categories:

- `SELECT`
- `EXPLAIN`
- `WITH`
- `INSERT`
- `UPDATE`
- `DELETE`
- `CREATE`
- `DROP`
- `ALTER`
- `PRAGMA`
- `OTHER`

Classification is used for public `statement_type`, readonly preflight checks, and transaction-control rejection.

Result shaping must not rely only on classification. The executor must inspect the prepared SQLite statement metadata. If the prepared statement has result columns, collect rows using the query result format. If it has no result columns, execute it as a non-query statement and return the relevant affected row or success result.

## SQLite Type Mapping

SQLite values map to JSON as follows:

- `NULL` to `null`
- `INTEGER` to JSON number
- `REAL` to JSON number
- `TEXT` to JSON string
- `BLOB` to a base64 object

BLOB format:

```json
{
  "type": "blob",
  "encoding": "base64",
  "data": "..."
}
```

## Resource Limits

`--max-rows` limits returned rows for every statement that produces result columns. This includes `SELECT`, `EXPLAIN`, read-style `PRAGMA`, CTE queries, and statements with `RETURNING`.

Rules:

- Return at most `max_rows` rows per query statement.
- If more rows are available, set `truncated: true`.
- Do not keep collecting all rows after the response limit is reached.

`--timeout-ms` applies to the entire `execute_sql` call.

Use SQLite progress handling with a deadline to interrupt long-running work. If the deadline is exceeded:

- interrupt execution,
- roll back the transaction,
- return `success: false`,
- return an error message such as `query timed out after 10000 ms`.

Timeout handling must prevent a slow query from blocking the service indefinitely.

## Run Modes

### readonly

Open SQLite with read-only flags.

Also reject obvious mutating statements before execution so users get clear errors. Rejected statements include:

- `INSERT`
- `UPDATE`
- `DELETE`
- `REPLACE`
- `CREATE`
- `DROP`
- `ALTER`
- `VACUUM`
- `ANALYZE`
- `ATTACH`
- `DETACH`
- explicit transaction-control statements
- write-style `PRAGMA` statements

Read-only `SELECT`, `EXPLAIN`, and read-style `PRAGMA` statements are allowed.

### readwrite

Open SQLite with read-write flags and allow legal SQLite SQL, except explicit transaction-control statements.

This mode is appropriate for trusted self-hosted usage. Production access should combine backend auth with reverse proxy controls.

## FTS5

Use `rusqlite` with bundled SQLite and ensure FTS5 is enabled at build time.

On startup, run:

```sql
CREATE VIRTUAL TABLE temp.__fts5_check USING fts5(x);
DROP TABLE temp.__fts5_check;
```

If the check succeeds, log:

```text
SQLite FTS5: enabled
```

If the check fails, startup fails with a clear error explaining that SQLite FTS5 is unavailable.

## Error Handling

Errors returned through the MCP tool must be stable and concise. They should include:

- human-readable `message`,
- `statement_index` when applicable,
- no authentication secrets,
- no internal debug dumps.

The service should log operational errors with enough context for diagnosis, but SQL result values and auth tokens should not be logged by default.

## Testing And Acceptance Criteria

Automated tests should cover:

- CLI defaults and required `--db`.
- Auth disabled when no token is configured.
- Auth enabled with missing, malformed, and mismatched `Authorization` headers returning `401`.
- Transaction commit on success.
- Rollback on any failed statement.
- Multiple statements executed in order.
- Correct `last_insert_rowid` after `INSERT`.
- Rejection of `BEGIN`, `COMMIT`, `ROLLBACK`, `SAVEPOINT`, and `RELEASE`.
- JSON mapping for NULL, INTEGER, REAL, TEXT, and BLOB.
- `max_rows` truncation and `truncated: true`.
- Timeout interruption and rollback.
- `readonly` allowing reads and rejecting writes, DDL, `VACUUM`, and write-style `PRAGMA`.
- FTS5 startup self-check.
- FTS5 table creation and query in readwrite mode.
- MCP `tools/list` exposing only `execute_sql`.
- MCP `tools/call` returning the unified response envelope.

Manual smoke test:

```bash
sqlite-mcp-rs \
  --db /tmp/app.db \
  --host 127.0.0.1 \
  --port 3000 \
  --mode readwrite \
  --auth-token "$MCP_AUTH_TOKEN" \
  --max-rows 500 \
  --timeout-ms 10000
```

The service is acceptable when the automated suite passes, startup confirms FTS5 support, and a Streamable HTTP MCP client can list and call `execute_sql`.
