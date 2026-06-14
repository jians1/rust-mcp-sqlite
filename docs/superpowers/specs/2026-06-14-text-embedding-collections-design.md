# Text Embedding Collections Design

## Goal

Replace the public vector MCP tool surface with text-first collection tools that
generate embeddings inside `sqlite-mcp-rs`. This removes the need for clients or
LLMs to pass large float arrays through MCP calls while keeping SQLite and
`sqlite-vec` as the local storage and search engine.

This is a breaking redesign. The service has not been deployed yet, so the
implementation does not need to preserve the old manual-vector MCP tools.

## Decisions

- Use an OpenAI-compatible embeddings HTTP API.
- Expose only text-oriented MCP tools for vector search workflows.
- Remove these MCP tools from `tools/list`: `create_vector_collection`,
  `upsert_vectors`, and `search_vectors`.
- Rename management tools to match the text-oriented surface:
  `delete_texts` and `drop_text_collection`.
- Keep `execute_sql` unchanged for general SQLite access and advanced
  inspection.
- Continue using `sqlite-vec` `vec0` tables internally.
- Do not return generated or stored embedding vectors to MCP clients.
- Store the original text and optional JSON object metadata with each item.
- Keep cosine distance and simple top-level JSON metadata equality filters.

## Embedding Configuration

Add runtime configuration:

- `--embedding-base-url <url>` defaults to `https://api.openai.com/v1`.
- `--embedding-api-key <key>` is optional and falls back to `OPENAI_API_KEY`.
- `--embedding-model <model>` enables text embedding tools.
- `--embedding-dimensions <n>` is optional and is sent as the OpenAI-compatible
  `dimensions` request field when present.
- `--embedding-timeout-ms <n>` defaults to `30000`.

The server may run SQL-only with no `--embedding-model`. In that mode,
text-embedding tools are still listed but return a clear JSON error stating that
embedding is not configured. This keeps startup simple and lets users discover
the required configuration from tool errors.

The embedding client sends:

```json
{
  "model": "text-embedding-3-small",
  "input": ["first text", "second text"],
  "dimensions": 1536
}
```

`dimensions` is omitted when not configured. The client expects a response with
a `data` array containing one embedding per input item.

## MCP Tools

The server exposes these tools:

- `execute_sql`
- `create_text_collection`
- `upsert_texts`
- `search_text`
- `delete_texts`
- `drop_text_collection`

All text collection tools return JSON text with `success`, optional `error`,
operation data, and `elapsed_ms`.

### create_text_collection

Input:

```json
{
  "collection": "docs"
}
```

Behavior:

- rejects in readonly mode;
- validates the collection name;
- requires embedding configuration;
- embeds a small probe string to determine the model dimension;
- creates the internal vector registry and `vec0` table;
- if the collection already exists with the same dimension and metric, returns
  success with `created: false`;
- if the collection exists with a different dimension or metric, returns an
  error.

Output includes `collection`, `table_name`, `dimension`, `distance_metric`, and
`created`. The dimension is returned for observability, not because clients need
to send vectors.

### upsert_texts

Input:

```json
{
  "collection": "docs",
  "items": [
    {
      "id": "doc-1",
      "text": "chunk text",
      "metadata": {"source": "manual", "tenant": "a"}
    }
  ]
}
```

Behavior:

- rejects in readonly mode;
- validates the collection exists;
- validates every `id` is non-empty;
- validates every `text` is non-empty;
- validates `metadata`, when present, is a JSON object;
- calls the embedding API once for the batch of item texts;
- validates the response count equals the item count;
- validates every returned embedding has the collection dimension and contains
  only finite values;
- replaces the whole record when `id` already exists;
- executes the SQLite batch atomically.

Missing `metadata` is stored as `{}`. The original text is stored in the
collection table and returned by search results.

### search_text

Input:

```json
{
  "collection": "docs",
  "query": "search query",
  "top_k": 5,
  "filter": {"tenant": "a", "source": "manual"}
}
```

Behavior:

- validates the collection exists;
- validates `query` is non-empty;
- validates `top_k` is positive and does not exceed `max_top_k`;
- validates `filter`, when present, is a JSON object with simple identifier
  keys and scalar values;
- embeds the query string;
- validates the query embedding dimension against the collection dimension;
- reuses the current unfiltered and filtered sqlite-vec search behavior.

Output:

```json
{
  "success": true,
  "collection": "docs",
  "results": [
    {
      "id": "doc-1",
      "distance": 0.123,
      "text": "chunk text",
      "metadata": {"source": "manual", "tenant": "a"}
    }
  ],
  "elapsed_ms": 4
}
```

Vectors are not returned.

### delete_texts

Input:

```json
{
  "collection": "docs",
  "ids": ["doc-1", "doc-2"]
}
```

Behavior:

- rejects in readonly mode;
- validates the collection exists;
- validates ids are non-empty;
- deletes matching ids;
- missing ids are not errors;
- returns requested and deleted counts.

### drop_text_collection

Input:

```json
{
  "collection": "docs"
}
```

Behavior:

- rejects in readonly mode;
- validates the collection name;
- drops the internal `vec0` table when present;
- removes the registry row;
- returns success with `existed: false` when the collection did not exist.

## Architecture

Keep `SqliteExecutor` as the only owner of the SQLite connection. Embedding HTTP
requests must not run inside the SQLite worker thread.

The MCP handler performs the async embedding step first, then sends a normal
SQLite vector operation to the worker with generated vectors. This keeps remote
model latency from blocking unrelated SQL and vector database jobs.

Internally, keep a small vector storage layer that understands dimensions,
metadata validation, upsert, search, delete, and drop. The public MCP layer uses
text-oriented input types, while the storage layer can still use generated
vectors as implementation details.

Add an `EmbeddingClient` boundary with one responsibility: convert one or more
texts into embeddings using the configured OpenAI-compatible endpoint. Tests can
exercise this boundary with a local HTTP server.

## Storage

Keep the registry table:

```sql
CREATE TABLE IF NOT EXISTS __vector_collections (
  name TEXT PRIMARY KEY,
  table_name TEXT NOT NULL UNIQUE,
  dimension INTEGER NOT NULL,
  distance_metric TEXT NOT NULL,
  created_at TEXT NOT NULL
);
```

Keep one internal `vec0` table per collection:

```sql
CREATE VIRTUAL TABLE vec_<collection> USING vec0(
  id TEXT PRIMARY KEY,
  embedding float[<dimension>] distance_metric=cosine,
  +text TEXT,
  +metadata TEXT
);
```

Collection names must contain only ASCII letters, digits, and underscores, and
must not start with `__`.

Advanced users may still inspect these tables through `execute_sql`, but normal
MCP clients should not depend on the internal vector representation.

## Error Handling

Text collection tools return ordinary failures as JSON tool responses, not MCP
protocol errors:

```json
{
  "success": false,
  "error": {
    "message": "embedding is not configured; set --embedding-model"
  },
  "elapsed_ms": 2
}
```

Representative errors:

- embedding is not configured;
- embedding API key is missing when the provider requires one;
- embedding HTTP request timed out;
- embedding HTTP response is not 2xx;
- embedding response JSON is malformed;
- embedding response count does not match input count;
- embedding dimension mismatch;
- embedding contains a non-finite value;
- invalid collection name;
- collection not found;
- collection already exists with a different dimension;
- metadata must be a JSON object;
- filter must be a JSON object;
- unsupported filter value type;
- readonly mode forbids the requested operation.

HTTP errors should include status code and a short response body excerpt when
available. Error messages must not include API keys.

## Readonly Mode

Readonly mode allows:

- `execute_sql` read statements, as today;
- `search_text`, because it only reads SQLite state after embedding the query.

Readonly mode rejects:

- `create_text_collection`;
- `upsert_texts`;
- `delete_texts`;
- `drop_text_collection`.

## Testing

Add or update focused tests for:

- CLI defaults and overrides for embedding flags;
- `tools/list` exposing `execute_sql` plus the five text collection tools;
- SQL-only configuration returning clear errors from text embedding tools;
- create collection probing embedding dimension;
- create collection idempotency and dimension conflict;
- upsert text storing original text and metadata;
- upsert replacing an existing id;
- upsert batch rollback when one generated embedding is invalid;
- search text embedding the query and returning results without vectors;
- search text metadata filters;
- delete text ids reporting requested and deleted counts;
- drop text collection removing the table and registry row;
- readonly allowing `search_text` and rejecting text write tools;
- embedding HTTP non-2xx, malformed JSON, wrong count, wrong dimension, and
  timeout behavior.

## Documentation

Update `README.md` and `README_ZH.md` to show text-first usage. The examples
should explain that users pass text, not vectors, and that embedding tokens are
spent only by the configured embedding model instead of by the chat model moving
float arrays through MCP.
