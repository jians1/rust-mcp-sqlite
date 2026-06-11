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
