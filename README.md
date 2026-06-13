# sqlite-mcp-rs

Rust SQLite MCP server over Streamable HTTP.

Use it when an MCP client needs to inspect or modify one SQLite database file through SQL and optional vector collection tools.

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

Then configure your MCP client to use Streamable HTTP with that URL. The server exposes these tools:

```text
execute_sql
create_vector_collection
upsert_vectors
search_vectors
delete_vectors
drop_vector_collection
```

Tool summary:

| Tool | Purpose | Mode |
| --- | --- | --- |
| `execute_sql` | Run SQLite SQL against the configured database. | Reads in `readonly`; reads and writes in `readwrite`. |
| `create_vector_collection` | Create a named sqlite-vec collection. | `readwrite` only. |
| `upsert_vectors` | Insert or replace client-provided embeddings. | `readwrite` only. |
| `search_vectors` | Search a collection by cosine distance. | `readonly` and `readwrite`. |
| `delete_vectors` | Delete vector records by id. | `readwrite` only. |
| `drop_vector_collection` | Drop a vector collection and registry metadata. | `readwrite` only. |

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

The response is an MCP `content` item whose `text` field is a JSON string. Parse that text to read the tool result.

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
- sqlite-vec functions and `vec0` virtual tables

Do not include explicit transaction control statements:

- `BEGIN`
- `COMMIT`
- `ROLLBACK`
- `SAVEPOINT`
- `RELEASE`

Each `execute_sql` call is wrapped in one transaction by the server. If any statement in the call fails, the whole call rolls back and `results` is empty.

## execute_sql Response Format

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

## Vector Collections

Vector support uses SQLite with `sqlite-vec`. Embeddings are supplied by the client as JSON number arrays; this server does not generate embeddings or call model APIs. Collections use cosine distance and are stored as `vec0` virtual tables named `vec_<collection>`.

Collection names must contain only ASCII letters, digits, and underscores, and must not start with `__`. Each record has:

- `id`: non-empty string
- `vector`: JSON number array matching the collection dimension
- `text`: optional string
- `metadata`: optional JSON object, stored as `{}` when omitted

Vector tools return the same MCP shape as `execute_sql`: a text content item containing a JSON envelope. Successful vector envelopes include:

- `success`: `true`
- `collection`: the collection name, when relevant
- operation-specific fields such as `created`, `upserted_count`, `results`, `requested_count`, `deleted_count`, or `existed`
- `elapsed_ms`

Failed vector envelopes include:

```json
{
  "success": false,
  "error": {
    "message": "vector dimension mismatch: expected 1536, got 768"
  },
  "elapsed_ms": 0
}
```

### create_vector_collection

```json
{
  "collection": "docs",
  "dimension": 1536
}
```

Creates `vec_docs` and records metadata in `__vector_collections`. Calling it again with the same dimension succeeds with `"created": false`; a different dimension returns an error.

Example success body:

```json
{
  "success": true,
  "collection": "docs",
  "table_name": "vec_docs",
  "dimension": 1536,
  "distance_metric": "cosine",
  "created": true,
  "elapsed_ms": 3
}
```

### upsert_vectors

```json
{
  "collection": "docs",
  "items": [
    {
      "id": "doc-1",
      "vector": [0.12, -0.03, 0.88],
      "text": "chunk text",
      "metadata": {"source": "manual", "tenant": "a"}
    }
  ]
}
```

Upsert replaces the whole record for the same `id`: vector, text, and metadata. Batches are atomic.

Validation rules:

- `items` may contain one or more records.
- `id` must be non-empty.
- `vector` length must match the collection dimension.
- vector values must be finite JSON numbers.
- `metadata`, when present, must be a JSON object.

### search_vectors

```json
{
  "collection": "docs",
  "vector": [0.12, -0.03, 0.88],
  "top_k": 5,
  "filter": {"tenant": "a", "source": "manual"}
}
```

Results include `id`, `distance`, `text`, and `metadata`. Stored vectors are not returned by default. `top_k` must be positive and no larger than `--max-rows`.

Filters are optional top-level metadata equality checks. Filter keys must be simple identifiers, and values must be scalar JSON values: string, number, boolean, or null. Nested paths, arrays, objects, ranges, and contains queries are not supported.

Unfiltered search uses sqlite-vec KNN. Filtered search first applies exact JSON metadata filtering, then ranks the filtered rows by cosine distance; filtered search is correct but not KNN-optimized in this version.

Example success body:

```json
{
  "success": true,
  "collection": "docs",
  "results": [
    {
      "id": "doc-1",
      "distance": 0.0,
      "text": "chunk text",
      "metadata": {"source": "manual", "tenant": "a"}
    }
  ],
  "elapsed_ms": 2
}
```

### delete_vectors

```json
{
  "collection": "docs",
  "ids": ["doc-1", "doc-2"]
}
```

Deletes matching ids and returns `requested_count` and `deleted_count`. Missing ids are not errors.

### drop_vector_collection

```json
{
  "collection": "docs"
}
```

Drops the collection table and removes its registry row. Dropping a missing collection succeeds with `"existed": false`.

### SQL Inspection

The vector tools are convenience wrappers over SQLite state. Advanced users can inspect the registry and collection tables through `execute_sql`:

```json
{
  "sql": "SELECT name, table_name, dimension, distance_metric, created_at FROM __vector_collections; SELECT id, text, metadata FROM vec_docs LIMIT 10;"
}
```

Vector tables can also be queried directly with sqlite-vec functions. This works for collections created by the vector tools:

```json
{
  "sql": "SELECT id, distance FROM vec_docs WHERE embedding MATCH vec_f32('[0.12,-0.03,0.88]') ORDER BY distance LIMIT 5;"
}
```

Advanced users may also create sqlite-vec tables directly through `execute_sql`:

```json
{
  "sql": "CREATE VIRTUAL TABLE vec_direct USING vec0(id TEXT PRIMARY KEY, embedding float[2] distance_metric=cosine, +text TEXT, +metadata TEXT); INSERT INTO vec_direct(id, embedding, text, metadata) VALUES ('doc-a', vec_f32('[1.0,0.0]'), 'alpha', '{\"tenant\":\"a\"}'); SELECT id, distance FROM vec_direct WHERE embedding MATCH vec_f32('[1.0,0.0]') ORDER BY distance LIMIT 1;"
}
```

Tables created directly this way are not registered in `__vector_collections`, so the vector convenience tools will not manage them unless you also maintain compatible registry metadata.

### Minimal MCP Workflow

For raw JSON-RPC clients, each vector operation is called with `tools/call`. The `arguments` object is the tool input shown above.

Create a collection:

```json
{
  "jsonrpc": "2.0",
  "id": 10,
  "method": "tools/call",
  "params": {
    "name": "create_vector_collection",
    "arguments": {"collection": "docs", "dimension": 2}
  }
}
```

Upsert and search:

```json
{
  "jsonrpc": "2.0",
  "id": 11,
  "method": "tools/call",
  "params": {
    "name": "upsert_vectors",
    "arguments": {
      "collection": "docs",
      "items": [
        {
          "id": "doc-a",
          "vector": [1.0, 0.0],
          "text": "alpha",
          "metadata": {"tenant": "a"}
        }
      ]
    }
  }
}
```

```json
{
  "jsonrpc": "2.0",
  "id": 12,
  "method": "tools/call",
  "params": {
    "name": "search_vectors",
    "arguments": {
      "collection": "docs",
      "vector": [1.0, 0.0],
      "top_k": 1,
      "filter": {"tenant": "a"}
    }
  }
}
```

## Modes And Safety

### readonly

`readonly` opens the SQLite database read-only and rejects mutating statements. Use it when the MCP client should inspect data but not change it.

Allowed examples:

```sql
SELECT * FROM users LIMIT 10;
PRAGMA table_info(users);
```

`search_vectors` is also allowed in readonly mode.

Rejected examples:

```sql
INSERT INTO users(name) VALUES ('alice');
CREATE TABLE t(id INTEGER);
PRAGMA user_version = 1;
```

`create_vector_collection`, `upsert_vectors`, `delete_vectors`, and `drop_vector_collection` are rejected in readonly mode.

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
