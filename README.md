# sqlite-mcp-rs

Rust SQLite MCP server over Streamable HTTP.

Use it when an MCP client needs to inspect or modify one SQLite database file through a single `execute_sql` tool.

## Quick Start

Build from source:

```bash
cargo build --release
```

Start a local read-write server:

```bash
./target/release/sqlite-mcp-rs \
  --db ./app.db \
  --host 127.0.0.1 \
  --port 3000 \
  --mode readwrite
```

The MCP endpoint is:

```text
http://127.0.0.1:3000/mcp
```

Then configure your MCP client to use Streamable HTTP with that URL. The server exposes one tool:

```text
execute_sql
```

## Install

### From GitHub Releases

Download the matching Linux asset from:

```text
https://github.com/jians1/rust-mcp-sqlite/releases
```

Release assets are packaged as tarballs such as:

```text
sqlite-mcp-rs-v0.1-linux-amd64.tar.gz
sqlite-mcp-rs-v0.1-linux-arm64.tar.gz
sqlite-mcp-rs-v0.1-linux-amd64-musl.tar.gz
sqlite-mcp-rs-v0.1-linux-arm64-musl.tar.gz
```

Extract the archive and run the `sqlite-mcp-rs` binary inside it.

### From Source

Requirements:

- Rust stable
- Cargo

Build:

```bash
cargo build --release
```

Run:

```bash
./target/release/sqlite-mcp-rs --db ./app.db
```

Or install into Cargo's bin directory:

```bash
cargo install --path .
sqlite-mcp-rs --db ./app.db
```

SQLite is bundled through `rusqlite`, so a system SQLite install is not required for normal builds.

## Run

Minimal local server:

```bash
sqlite-mcp-rs --db ./app.db
```

Production-style local backend with auth:

```bash
export MCP_AUTH_TOKEN='change-me'

sqlite-mcp-rs \
  --db /data/app.db \
  --host 127.0.0.1 \
  --port 3000 \
  --mode readwrite \
  --auth-token "$MCP_AUTH_TOKEN" \
  --max-rows 500 \
  --timeout-ms 10000
```

Read-only server:

```bash
sqlite-mcp-rs \
  --db /data/app.db \
  --mode readonly \
  --auth-token "$MCP_AUTH_TOKEN"
```

Command line options:

| Option | Default | Description |
| --- | --- | --- |
| `--db <path>` | required | SQLite database file. `readwrite` may create it; `readonly` requires an existing readable file. |
| `--host <ip>` | `127.0.0.1` | Listen address. Keep localhost when running behind a reverse proxy. |
| `--port <port>` | `3000` | Listen port. |
| `--mode <mode>` | `readwrite` | `readonly` or `readwrite`. |
| `--auth-token <token>` | none | Enables Bearer token auth for every HTTP request. |
| `--max-rows <n>` | `500` | Maximum returned rows per statement that produces rows. |
| `--timeout-ms <n>` | `10000` | Timeout for the whole `execute_sql` call. |

## MCP Client Configuration

Use Streamable HTTP transport and point the client at `/mcp`:

```json
{
  "mcpServers": {
    "sqlite": {
      "type": "http",
      "url": "http://127.0.0.1:3000/mcp"
    }
  }
}
```

If `--auth-token` is enabled, send:

```http
Authorization: Bearer change-me
```

Some MCP clients support headers in their config:

```json
{
  "mcpServers": {
    "sqlite": {
      "type": "http",
      "url": "http://127.0.0.1:3000/mcp",
      "headers": {
        "Authorization": "Bearer change-me"
      }
    }
  }
}
```

Exact config keys vary by MCP client. The important pieces are:

- transport: Streamable HTTP
- URL: `http://<host>:<port>/mcp`
- optional header: `Authorization: Bearer <token>`

## Smoke Test With curl

Start the server first:

```bash
sqlite-mcp-rs --db /tmp/sqlite-mcp-smoke.db --port 3000 --mode readwrite
```

Initialize an MCP session:

```bash
curl -sS \
  -H 'accept: application/json, text/event-stream' \
  -H 'content-type: application/json' \
  --data-binary @- \
  http://127.0.0.1:3000/mcp <<'JSON'
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "initialize",
  "params": {
    "protocolVersion": "2025-06-18",
    "capabilities": {},
    "clientInfo": {"name": "curl", "version": "0.1.0"}
  }
}
JSON
```

Call `execute_sql`:

```bash
curl -sS \
  -H 'accept: application/json, text/event-stream' \
  -H 'content-type: application/json' \
  --data-binary @- \
  http://127.0.0.1:3000/mcp <<'JSON'
{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "tools/call",
  "params": {
    "name": "execute_sql",
    "arguments": {
      "sql": "CREATE TABLE IF NOT EXISTS smoke(id INTEGER PRIMARY KEY, name TEXT); INSERT INTO smoke(name) VALUES ('alpha'), ('beta'); SELECT id, name FROM smoke ORDER BY id;"
    }
  }
}
JSON
```

With auth enabled, add:

```bash
-H "Authorization: Bearer $MCP_AUTH_TOKEN"
```

## Tool: execute_sql

Input schema:

```json
{
  "sql": "string, required"
}
```

Single statement:

```json
{"sql": "SELECT 1 AS value"}
```

Multiple statements go in the same `sql` string:

```json
{
  "sql": "CREATE TABLE users(id INTEGER PRIMARY KEY, name TEXT); INSERT INTO users(name) VALUES ('alice'); SELECT id, name FROM users;"
}
```

Supported SQL includes:

- `SELECT`, `EXPLAIN`, and read-style `PRAGMA`
- `INSERT`, `UPDATE`, `DELETE`, and `REPLACE` in `readwrite` mode
- `CREATE`, `DROP`, and `ALTER` in `readwrite` mode
- common table expressions with `WITH`
- statements with `RETURNING`
- FTS5

Do not include explicit transaction control statements:

- `BEGIN`
- `COMMIT`
- `ROLLBACK`
- `SAVEPOINT`
- `RELEASE`

Each `execute_sql` call is wrapped in one transaction by the server. If any statement in the call fails, the whole call rolls back and `results` is empty.

## Response Format

The MCP tool returns a text content item. The text is a JSON envelope.

Successful response:

```json
{
  "success": true,
  "results": [
    {
      "statement_index": 0,
      "statement_type": "SELECT",
      "columns": ["value"],
      "rows": [{"value": 1}],
      "row_count": 1,
      "truncated": false
    }
  ],
  "elapsed_ms": 0
}
```

Failed response:

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

Result shapes:

- queries return `columns`, `rows`, `row_count`, and `truncated`
- `INSERT` returns `affected_rows` and `last_insert_rowid`
- `UPDATE` and `DELETE` return `affected_rows`
- schema changes return `success` and `schema_changed`
- other legal non-query statements return a generic success result

SQLite value mapping:

- `NULL` -> `null`
- `INTEGER` and `REAL` -> JSON numbers
- `TEXT` -> JSON string
- `BLOB` -> `{"type":"blob","encoding":"base64","data":"..."}`

## Modes And Safety

### readonly

`readonly` opens the SQLite database read-only and rejects mutating statements. Use it when the MCP client should inspect data but not change it.

Allowed examples:

```sql
SELECT * FROM users LIMIT 10;
PRAGMA table_info(users);
```

Rejected examples:

```sql
INSERT INTO users(name) VALUES ('alice');
CREATE TABLE t(id INTEGER);
PRAGMA user_version = 1;
```

### readwrite

`readwrite` allows legal SQLite SQL except explicit transaction control statements. Use it only for trusted clients.

The server serializes SQL execution through one SQLite connection. Concurrent HTTP requests are accepted, but database work is executed in order.

## Deployment Notes

Recommended deployment shape:

```text
MCP client -> HTTPS reverse proxy -> sqlite-mcp-rs on 127.0.0.1:<port> -> SQLite file
```

Use Nginx or Caddy for:

- HTTPS
- domain routing
- IP allowlists
- extra authentication or access policy

Use `--auth-token` for backend Bearer auth even when a reverse proxy is present.

Keep the database file and its directory permissions restricted to the service user.

## Troubleshooting

`401 Unauthorized`

- `--auth-token` is enabled.
- Add `Authorization: Bearer <token>` to the MCP client or curl request.

`readonly mode forbids ... statements`

- The server is running with `--mode readonly`.
- Restart with `--mode readwrite` only if writes are intended.

`transaction control statements are not allowed`

- Remove `BEGIN`, `COMMIT`, `ROLLBACK`, `SAVEPOINT`, or `RELEASE`.
- Send the SQL statements together; the server handles the transaction.

`query timed out after ... ms`

- Increase `--timeout-ms`, reduce the query cost, or add indexes.

Too many rows missing from a query result

- Increase `--max-rows`.
- Check whether the result has `"truncated": true`.

## Development

Run tests:

```bash
cargo test
```

Run with logs:

```bash
RUST_LOG=info cargo run -- --db ./app.db --mode readwrite
```
