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

Multiple statements should be passed in the same `sql` string:

```json
{"sql": "CREATE TABLE smoke(id INTEGER PRIMARY KEY, name TEXT); INSERT INTO smoke(name) VALUES ('alpha'); INSERT INTO smoke(name) VALUES ('beta'); SELECT id, name FROM smoke ORDER BY id;"}
```

The response is a JSON envelope with `success`, `results`, and `elapsed_ms`.
Each successful statement produces one entry in `results`.

## Modes

`readonly` opens SQLite read-only and rejects mutating statements.

`readwrite` allows legal SQLite SQL except explicit transaction control statements. Each tool call is wrapped in one transaction, so multiple statements in one `execute_sql` call are atomic: if any statement fails, the whole call rolls back. Do not include explicit transaction control statements such as `BEGIN`, `COMMIT`, or `ROLLBACK`.
