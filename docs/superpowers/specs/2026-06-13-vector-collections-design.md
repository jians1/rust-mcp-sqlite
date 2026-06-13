# SQLite Vector Collections Design

## Goal

Add vector search capability to `sqlite-mcp-rs` while preserving the current
single-file SQLite model, the existing `execute_sql` tool, readonly/readwrite
modes, and the serialized SQLite executor.

The first version targets hundreds of thousands of client-provided embeddings.
The service does not generate embeddings and does not call external model APIs.

## Decisions

- Use SQLite with `sqlite-vec`, not libSQL.
- Keep SQL access and add MCP convenience tools; the tools are thin wrappers over
  SQLite state.
- Use collection-level vector tables.
- Use cosine distance.
- Accept vectors as JSON number arrays.
- Store optional `text` and optional JSON object `metadata` with each vector.
- Support simple top-level JSON metadata equality filters in `search_vectors`.
- Use covering upsert semantics: an existing `id` is replaced with the new
  `vector`, `text`, and `metadata`.
- Do not return stored vectors from `search_vectors` by default.
- Expose `drop_vector_collection` directly in `tools/list`.
- Restrict collection names to simple identifiers.

## Architecture

The existing `SqliteExecutor` remains the only owner of the SQLite connection.
Vector tools enqueue jobs to the same worker thread used by `execute_sql`.
This keeps SQL execution serialized, preserves the existing timeout strategy,
and makes `execute_sql` and vector tools observe the same database state.

Startup registers `sqlite-vec` on the bundled SQLite connection and runs a
self-check, similar to the current FTS5 check. Startup fails with a clear error
if `sqlite-vec` is unavailable.

Add a vector service layer behind the MCP handler. Its responsibilities are:

- validate MCP inputs;
- enforce readonly behavior for vector write operations;
- create and inspect collection metadata;
- build safe SQL for service-owned collection tables;
- map SQLite rows to stable JSON tool responses.

## Storage

Create one registry table:

```sql
CREATE TABLE IF NOT EXISTS __vector_collections (
  name TEXT PRIMARY KEY,
  table_name TEXT NOT NULL UNIQUE,
  dimension INTEGER NOT NULL,
  distance_metric TEXT NOT NULL,
  created_at TEXT NOT NULL
);
```

Collection names must match `[A-Za-z0-9_]+` and must not start with `__`.
The service derives table names as `vec_<collection>`.
`created_at` is stored as a UTC ISO-8601 string.

Each collection uses one `vec0` virtual table. The intended shape is:

```sql
CREATE VIRTUAL TABLE vec_<collection> USING vec0(
  id TEXT PRIMARY KEY,
  embedding float[<dimension>] distance_metric=cosine,
  +text TEXT,
  +metadata TEXT
);
```

`text` and `metadata` are `vec0` auxiliary columns. `metadata` stores the JSON
object as canonical JSON text. The tool layer validates that metadata is an
object before storage.

Advanced users may query these tables through `execute_sql`. They may also
inspect `__vector_collections`.

## MCP Tools

The server exposes these tools:

- `execute_sql`
- `create_vector_collection`
- `upsert_vectors`
- `search_vectors`
- `delete_vectors`
- `drop_vector_collection`

All vector tools return JSON text with `success`, optional `error`, and
`elapsed_ms`.

### create_vector_collection

Input:

```json
{
  "collection": "docs",
  "dimension": 1536
}
```

Behavior:

- validates the collection name and positive dimension;
- creates the registry table if needed;
- creates the `vec0` table with cosine distance;
- inserts the registry row;
- if the collection already exists with the same dimension and metric, returns
  success with `created: false`;
- if the collection exists with a different dimension or metric, returns an
  error;
- rejects in readonly mode.

### upsert_vectors

Input:

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

Behavior:

- validates the collection exists;
- validates every `id` is non-empty;
- validates each vector length equals the collection dimension;
- validates every vector value is finite;
- validates `metadata`, when present, is a JSON object;
- serializes each vector as a JSON array and stores it through `vec_f32(?)`;
- replaces the whole record when `id` already exists;
- executes the whole batch atomically;
- rejects in readonly mode.

Missing `text` is stored as `NULL`. Missing `metadata` is stored as `{}`.

### search_vectors

Input:

```json
{
  "collection": "docs",
  "vector": [0.12, -0.03, 0.88],
  "top_k": 5,
  "filter": {"tenant": "a", "source": "manual"}
}
```

Behavior:

- validates the collection exists;
- validates the query vector length and values;
- validates `top_k` is positive and does not exceed the runtime `max_top_k`
  setting;
- accepts an optional JSON object filter;
- filter keys match top-level metadata fields and must be simple identifiers;
- filter values must be scalar JSON values: string, number, boolean, or null;
- filter comparisons are type-sensitive;
- array, object, range, contains, and nested-path filters are out of scope.

With no filter, use `sqlite-vec` KNN query shape:

```sql
SELECT id, distance, text, metadata
FROM vec_<collection>
WHERE embedding MATCH vec_f32(?)
ORDER BY distance
LIMIT ?;
```

With a JSON metadata filter, do exact filtering first and rank the filtered
rows by cosine distance. This preserves correct filter semantics but may scan
the collection because `vec0` KNN queries cannot constrain auxiliary columns.
The first version does not promise KNN-level performance for filtered search.

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

Stored vectors are not returned.

### delete_vectors

Input:

```json
{
  "collection": "docs",
  "ids": ["doc-1", "doc-2"]
}
```

Behavior:

- validates the collection exists;
- deletes matching ids in one transaction;
- missing ids are not errors;
- returns requested and deleted counts;
- rejects in readonly mode.

### drop_vector_collection

Input:

```json
{
  "collection": "docs"
}
```

Behavior:

- validates the collection name;
- drops the `vec0` table if present;
- removes the registry row;
- returns success with `existed: false` when the collection did not exist;
- rejects in readonly mode.

## Readonly Mode

Readonly mode allows:

- `execute_sql` read statements, as today;
- `search_vectors`.

Readonly mode rejects:

- `create_vector_collection`;
- `upsert_vectors`;
- `delete_vectors`;
- `drop_vector_collection`.

Readonly rejections use clear JSON errors consistent with the current
`execute_sql` behavior.

## Errors

Vector tools do not surface ordinary validation or SQLite execution failures as
MCP protocol errors. They return JSON text:

```json
{
  "success": false,
  "error": {
    "message": "vector dimension mismatch: expected 1536, got 768"
  },
  "elapsed_ms": 2
}
```

Representative validation errors:

- invalid collection name;
- collection not found;
- collection already exists with a different dimension;
- vector dimension mismatch;
- vector contains a non-finite value;
- metadata must be a JSON object;
- filter must be a JSON object;
- unsupported filter value type;
- readonly mode forbids the requested operation.

## SQL Compatibility

`execute_sql` remains available for advanced users. It can:

- inspect `__vector_collections`;
- query collection tables directly;
- run custom vector queries;
- drop or repair tables manually if needed.

The service-owned tools remain the recommended path for operations that are
easy for an MCP client to get wrong: collection creation, vector encoding,
dimension checks, upsert, search, batch deletion, and collection dropping.

## Testing

Add focused tests for:

- `sqlite-vec` startup registration and self-check;
- collection creation;
- repeated create with same dimension and different dimension;
- collection name validation;
- JSON vector length validation;
- non-finite vector rejection;
- metadata object validation;
- covering upsert behavior;
- unfiltered cosine search returning top-k and no vector field;
- JSON metadata equality filter behavior;
- filtered search correctness;
- deletion of existing and missing ids;
- collection drop removing the table and registry row;
- readonly allowing search and rejecting write tools;
- `tools/list` exposing `execute_sql` plus the five vector tools;
- direct `execute_sql` access to vector collection tables.

## Future Work

If filtered search needs high performance, add a second collection creation mode
that declares typed metadata columns or partition keys. That would let
`sqlite-vec` apply filters inside KNN queries. The first version keeps the API
simple and treats JSON metadata filtering as convenient but not index-optimized.
