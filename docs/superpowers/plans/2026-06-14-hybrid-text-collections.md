# Hybrid Text Collections Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a generic SQLite + sqlite-vec + FTS5 trigram hybrid recall tool without locking the project into a novel-specific schema.

**Architecture:** Keep the existing text collection API as the base storage abstraction. Each collection keeps its current sqlite-vec table and gains two sidecars: an FTS5 trigram table for lexical Chinese/full-text candidate filtering and a normal tags table populated from `metadata.tags`. Add a new `search_text_hybrid` MCP tool so existing vector-only `search_text` behavior remains stable.

**Tech Stack:** Rust 2024, rusqlite, sqlite-vec, SQLite FTS5 trigram, serde JSON metadata, rmcp tool macros, tokio tests.

---

## File Structure

- Modify `src/vector.rs`: collection sidecar schema, tag extraction, FTS query escaping, generated-vector hybrid search operation.
- Modify `src/sqlite.rs`: executor wrapper method for the new vector operation.
- Modify `src/mcp.rs`: new `search_text_hybrid` MCP tool that embeds the query and calls the generated-vector hybrid search.
- Modify `src/lib.rs`: no new module required unless `src/vector.rs` grows too large during implementation.
- Modify `tests/vector_collections.rs`: storage and search behavior tests for FTS sidecars, tag extraction, and hybrid filtering.
- Modify `tests/mcp_http.rs`: tool list and one end-to-end MCP hybrid search test.
- Modify `README.md` and `README_ZH.md`: document sidecar tables, `metadata.tags`, FTS trigram behavior, and the new tool.

The implementation intentionally does not create fixed business tables such as `analysis_items`. Users store flexible text plus JSON metadata first; stable business fields can later be promoted into explicit tables or generated columns.

---

### Task 1: Add Storage Tests For FTS And Tags Sidecars

**Files:**
- Modify: `tests/vector_collections.rs`

- [ ] **Step 1: Write the failing sidecar creation and indexing test**

Add this test near `create_collection_writes_registry`:

```rust
#[tokio::test]
async fn create_collection_adds_fts_and_tag_sidecars() {
    let (_dir, path) = temp_db_path("hybrid_sidecars.db");
    let exec = executor(path, RunMode::Readwrite, 500, 100).await;

    let create = exec
        .create_text_collection_with_dimension(CreateTextCollectionStorageInput {
            collection: "docs".to_string(),
            dimension: 2,
        })
        .await;
    assert!(create.success, "{create:?}");

    let sidecars = exec
        .execute(
            "SELECT name, type
             FROM sqlite_master
             WHERE name IN ('fts_docs', 'tags_docs')
             ORDER BY name;"
                .to_string(),
        )
        .await;
    assert!(sidecars.success, "{sidecars:?}");
    let StatementResult::Query(query) = &sidecars.results[0] else {
        panic!("expected query result");
    };
    assert_eq!(query.row_count, 2);
    assert_eq!(query.rows[0]["name"], json!("fts_docs"));
    assert_eq!(query.rows[1]["name"], json!("tags_docs"));
}
```

- [ ] **Step 2: Write the failing upsert indexing test**

Add this test near `upsert_generated_texts_replaces_existing_records`:

```rust
#[tokio::test]
async fn upsert_generated_texts_indexes_fts_and_metadata_tags() {
    let (_dir, path) = temp_db_path("hybrid_indexing.db");
    let exec = executor(path, RunMode::Readwrite, 500, 100).await;

    let create = exec
        .create_text_collection_with_dimension(CreateTextCollectionStorageInput {
            collection: "docs".to_string(),
            dimension: 2,
        })
        .await;
    assert!(create.success, "{create:?}");

    let upsert = exec
        .upsert_generated_texts(UpsertGeneratedTextsInput {
            collection: "docs".to_string(),
            items: vec![GeneratedTextItemInput {
                id: "doc-a".to_string(),
                vector: vec![1.0, 0.0],
                text: "她没有拔剑，只是抬眼看过去，殿中喧哗便像被霜压住。".to_string(),
                metadata: Some(json!({
                    "tenant": "novel",
                    "tags": ["女主", "克制", "压迫感"]
                })),
            }],
        })
        .await;
    assert!(upsert.success, "{upsert:?}");

    let fts = exec
        .execute(
            "SELECT id
             FROM fts_docs
             WHERE fts_docs MATCH '\"殿中喧哗\"';"
                .to_string(),
        )
        .await;
    assert!(fts.success, "{fts:?}");
    let StatementResult::Query(fts_query) = &fts.results[0] else {
        panic!("expected query result");
    };
    assert_eq!(fts_query.row_count, 1);
    assert_eq!(fts_query.rows[0]["id"], json!("doc-a"));

    let tags = exec
        .execute(
            "SELECT tag
             FROM tags_docs
             WHERE item_id = 'doc-a'
             ORDER BY tag;"
                .to_string(),
        )
        .await;
    assert!(tags.success, "{tags:?}");
    let StatementResult::Query(tags_query) = &tags.results[0] else {
        panic!("expected query result");
    };
    assert_eq!(tags_query.row_count, 3);
    assert_eq!(tags_query.rows[0]["tag"], json!("克制"));
    assert_eq!(tags_query.rows[1]["tag"], json!("压迫感"));
    assert_eq!(tags_query.rows[2]["tag"], json!("女主"));
}
```

- [ ] **Step 3: Run the storage tests and verify they fail**

Run:

```bash
cargo test --test vector_collections create_collection_adds_fts_and_tag_sidecars upsert_generated_texts_indexes_fts_and_metadata_tags
```

Expected: fail because `fts_docs` and `tags_docs` do not exist.

---

### Task 2: Implement Collection Sidecar Tables

**Files:**
- Modify: `src/vector.rs`

- [ ] **Step 1: Add sidecar table-name helpers**

Add near `collection_table_name`:

```rust
fn fts_table_name(collection: &str) -> String {
    format!("fts_{collection}")
}

fn tags_table_name(collection: &str) -> String {
    format!("tags_{collection}")
}
```

- [ ] **Step 2: Create FTS and tags sidecars with each collection**

Add this helper near `create_vec0_table`:

```rust
fn create_collection_sidecars(conn: &Connection, collection: &str) -> Result<(), String> {
    let fts_table = fts_table_name(collection);
    let tags_table = tags_table_name(collection);
    let sql = format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS {fts_table}
         USING fts5(id UNINDEXED, text, tokenize='trigram');
         CREATE TABLE IF NOT EXISTS {tags_table} (
           item_id TEXT NOT NULL,
           tag TEXT NOT NULL,
           PRIMARY KEY (item_id, tag)
         );
         CREATE INDEX IF NOT EXISTS {tags_table}_tag_item_idx
           ON {tags_table}(tag, item_id);"
    );
    conn.execute_batch(&sql).map_err(|error| error.to_string())
}
```

Update `create_collection` so both the new and idempotent-existing paths call sidecar creation after the collection name is validated and the registry is ensured:

```rust
if let Some(existing) = find_collection(conn, collection)? {
    if existing.dimension != input.dimension || existing.distance_metric != DISTANCE_METRIC {
        return Err(format!(
            "collection already exists with dimension {} and distance metric {}",
            existing.dimension, existing.distance_metric
        ));
    }
    create_collection_sidecars(conn, collection)?;
    return Ok(collection_response(
        collection,
        &existing.table_name,
        existing.dimension,
        false,
    ));
}

let table_name = collection_table_name(collection);
create_vec0_table(conn, &table_name, input.dimension)?;
create_collection_sidecars(conn, collection)?;
```

- [ ] **Step 3: Extract tags from metadata**

Add near `metadata_to_json`:

```rust
fn metadata_tags(metadata: Option<&Value>) -> Result<Vec<String>, String> {
    let Some(Value::Object(object)) = metadata else {
        return Ok(Vec::new());
    };
    let Some(tags) = object.get("tags") else {
        return Ok(Vec::new());
    };
    let Value::Array(values) = tags else {
        return Err("metadata.tags must be an array of strings".to_string());
    };

    let mut collected = Vec::new();
    for value in values {
        let Some(tag) = value.as_str() else {
            return Err("metadata.tags must be an array of strings".to_string());
        };
        let tag = tag.trim();
        if tag.is_empty() {
            return Err("metadata.tags must not contain empty strings".to_string());
        }
        if !collected.iter().any(|existing| existing == tag) {
            collected.push(tag.to_string());
        }
    }
    Ok(collected)
}
```

- [ ] **Step 4: Upsert FTS rows and tags in the existing transaction**

In `upsert_generated_texts`, after inserting into the vec table, add sidecar writes. Build these SQL strings once before the loop:

```rust
let fts_table = fts_table_name(collection);
let tags_table = tags_table_name(collection);
let delete_fts_sql = format!("DELETE FROM {fts_table} WHERE id = ?1");
let insert_fts_sql = format!("INSERT INTO {fts_table}(id, text) VALUES (?1, ?2)");
let delete_tags_sql = format!("DELETE FROM {tags_table} WHERE item_id = ?1");
let insert_tag_sql = format!("INSERT INTO {tags_table}(item_id, tag) VALUES (?1, ?2)");
create_collection_sidecars(conn, collection)?;
```

Inside the loop, after the vec insert:

```rust
let tags = metadata_tags(item.metadata.as_ref())?;
conn.execute(&delete_fts_sql, params![item.id])
    .map_err(|error| error.to_string())?;
conn.execute(&insert_fts_sql, params![item.id, item.text.as_str()])
    .map_err(|error| error.to_string())?;
conn.execute(&delete_tags_sql, params![item.id])
    .map_err(|error| error.to_string())?;
for tag in tags {
    conn.execute(&insert_tag_sql, params![item.id, tag])
        .map_err(|error| error.to_string())?;
}
```

- [ ] **Step 5: Delete and drop sidecar data**

In `delete_texts`, create these SQL strings next to the vec delete SQL:

```rust
let fts_table = fts_table_name(collection);
let tags_table = tags_table_name(collection);
let delete_fts_sql = format!("DELETE FROM {fts_table} WHERE id = ?1");
let delete_tags_sql = format!("DELETE FROM {tags_table} WHERE item_id = ?1");
```

For each requested id, after deleting from the vec table, also run:

```rust
let _ = conn.execute(&delete_fts_sql, params![id]);
let _ = conn.execute(&delete_tags_sql, params![id]);
```

In `drop_text_collection`, drop sidecars before deleting the registry row:

```rust
let fts_table = fts_table_name(collection);
let tags_table = tags_table_name(collection);
let sql = format!(
    "DROP TABLE IF EXISTS {};
     DROP TABLE IF EXISTS {};
     DROP TABLE IF EXISTS {};",
    existing.table_name, fts_table, tags_table
);
```

- [ ] **Step 6: Run the storage tests and verify they pass**

Run:

```bash
cargo test --test vector_collections create_collection_adds_fts_and_tag_sidecars upsert_generated_texts_indexes_fts_and_metadata_tags
```

Expected: both tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/vector.rs tests/vector_collections.rs
git commit -m "feat: add hybrid collection sidecars"
```

---

### Task 3: Add Generated-Vector Hybrid Search

**Files:**
- Modify: `src/vector.rs`
- Modify: `tests/vector_collections.rs`

- [ ] **Step 1: Add the failing hybrid search test**

Add this test near `search_generated_text_filters_metadata`:

```rust
#[tokio::test]
async fn search_generated_text_hybrid_filters_fts_tags_and_metadata_then_sorts_by_vector() {
    let (_dir, path) = temp_db_path("hybrid_search.db");
    let exec = executor(path, RunMode::Readwrite, 500, 100).await;

    let create = exec
        .create_text_collection_with_dimension(CreateTextCollectionStorageInput {
            collection: "docs".to_string(),
            dimension: 4,
        })
        .await;
    assert!(create.success, "{create:?}");

    let upsert = exec
        .upsert_generated_texts(UpsertGeneratedTextsInput {
            collection: "docs".to_string(),
            items: vec![
                GeneratedTextItemInput {
                    id: "a1".to_string(),
                    vector: vec![0.98, 0.02, 0.04, 0.10],
                    text: "她没有拔剑，只是抬眼看过去，殿中喧哗便像被霜压住。".to_string(),
                    metadata: Some(json!({
                        "tenant": "novel",
                        "dimension": "高潮燃点",
                        "score": 9,
                        "tags": ["女主", "克制", "压迫感"]
                    })),
                },
                GeneratedTextItemInput {
                    id: "a2".to_string(),
                    vector: vec![0.90, 0.05, 0.02, 0.02],
                    text: "他站在雨里，眉眼淡得像旧雪，偏让人不敢靠近。".to_string(),
                    metadata: Some(json!({
                        "tenant": "novel",
                        "dimension": "人物塑造",
                        "score": 9,
                        "tags": ["男主", "克制", "压迫感"]
                    })),
                },
                GeneratedTextItemInput {
                    id: "a3".to_string(),
                    vector: vec![0.04, 0.97, 0.01, 0.02],
                    text: "她把杯子往前一推，笑说这次轮到你认输。".to_string(),
                    metadata: Some(json!({
                        "tenant": "novel",
                        "dimension": "日常互动",
                        "score": 8,
                        "tags": ["女主", "轻松"]
                    })),
                },
            ],
        })
        .await;
    assert!(upsert.success, "{upsert:?}");

    let search = exec
        .search_generated_text_hybrid(SearchGeneratedHybridTextInput {
            collection: "docs".to_string(),
            vector: vec![1.0, 0.0, 0.0, 0.0],
            top_k: 5,
            filter: Some(json!({"tenant": "novel"})),
            fts_query: Some("殿中喧哗".to_string()),
            tags: vec!["女主".to_string(), "克制".to_string()],
        })
        .await;

    assert!(search.success, "{search:?}");
    let results = search.data["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["id"], json!("a1"));
    assert_eq!(results[0]["metadata"]["dimension"], json!("高潮燃点"));
}
```

Update the imports at the top of `tests/vector_collections.rs`:

```rust
use sqlite_mcp_rs::vector::{
    CreateTextCollectionStorageInput, DeleteTextsInput, DropTextCollectionInput,
    GeneratedTextItemInput, SearchGeneratedHybridTextInput, SearchGeneratedTextInput,
    UpsertGeneratedTextsInput,
};
```

- [ ] **Step 2: Run the test and verify it fails**

Run:

```bash
cargo test --test vector_collections search_generated_text_hybrid_filters_fts_tags_and_metadata_then_sorts_by_vector
```

Expected: compile failure because `SearchGeneratedHybridTextInput` and `search_generated_text_hybrid` do not exist.

- [ ] **Step 3: Add input type and operation variant**

In `src/vector.rs`, add this variant:

```rust
pub enum VectorOperation {
    DescribeCollection(DescribeTextCollectionInput),
    CreateCollection(CreateTextCollectionStorageInput),
    UpsertGeneratedTexts(UpsertGeneratedTextsInput),
    SearchGeneratedText(SearchGeneratedTextInput),
    SearchGeneratedTextHybrid(SearchGeneratedHybridTextInput),
    DeleteTexts(DeleteTextsInput),
    DropTextCollection(DropTextCollectionInput),
}
```

Add this input struct near `SearchGeneratedTextInput`:

```rust
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize, PartialEq)]
pub struct SearchGeneratedHybridTextInput {
    pub collection: String,
    pub vector: Vec<f64>,
    pub top_k: usize,
    pub filter: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fts_query: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}
```

In `execute_vector_operation`, add:

```rust
VectorOperation::SearchGeneratedTextHybrid(input) => {
    search_generated_text_hybrid(conn, max_top_k, input)
}
```

- [ ] **Step 4: Add FTS query and tag validation helpers**

Add near `validate_filter_key`:

```rust
fn validate_tag(tag: &str) -> Result<&str, String> {
    let tag = tag.trim();
    if tag.is_empty() {
        return Err("tags must not contain empty strings".to_string());
    }
    Ok(tag)
}

fn fts_match_query(input: &str) -> Result<String, String> {
    let terms = input
        .split_whitespace()
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .map(quote_fts_term)
        .collect::<Vec<_>>();

    if terms.is_empty() {
        return Err("fts_query must not be empty".to_string());
    }

    Ok(terms.join(" AND "))
}

fn quote_fts_term(term: &str) -> String {
    format!("\"{}\"", term.replace('"', "\"\""))
}
```

- [ ] **Step 5: Implement hybrid search SQL**

Add this function after `search_generated_text_filtered`:

```rust
fn search_generated_text_hybrid(
    conn: &Connection,
    max_top_k: usize,
    input: SearchGeneratedHybridTextInput,
) -> Result<Map<String, Value>, String> {
    let collection = validate_collection_name(&input.collection)?;
    let existing = find_collection(conn, collection)?
        .ok_or_else(|| format!("collection not found: {collection}"))?;
    if input.top_k == 0 {
        return Err("top_k must be positive".to_string());
    }
    if input.top_k > max_top_k {
        return Err(format!("top_k must not exceed max_top_k ({max_top_k})"));
    }

    create_collection_sidecars(conn, collection)?;
    let vector_json = vector_to_json(&input.vector, existing.dimension)?;
    let fts_table = fts_table_name(collection);
    let tags_table = tags_table_name(collection);

    let mut clauses = Vec::new();
    let mut values: Vec<Box<dyn ToSql>> = vec![Box::new(vector_json)];

    if let Some(filter) = filter_to_map(input.filter.as_ref())? {
        append_metadata_filter_clauses(filter, &mut clauses, &mut values)?;
    }

    if let Some(fts_query) = input.fts_query.as_deref().map(str::trim)
        && !fts_query.is_empty()
    {
        clauses.push(format!(
            "id IN (SELECT id FROM {fts_table} WHERE {fts_table} MATCH ?)"
        ));
        values.push(Box::new(fts_match_query(fts_query)?));
    }

    let mut normalized_tags = Vec::new();
    for tag in input.tags {
        let tag = validate_tag(&tag)?.to_string();
        if !normalized_tags.iter().any(|existing| existing == &tag) {
            normalized_tags.push(tag);
        }
    }
    for tag in normalized_tags {
        clauses.push(format!(
            "id IN (SELECT item_id FROM {tags_table} WHERE tag = ?)"
        ));
        values.push(Box::new(tag));
    }

    values.push(Box::new(input.top_k as i64));
    let where_clause = if clauses.is_empty() {
        "1".to_string()
    } else {
        clauses.join(" AND ")
    };
    let sql = format!(
        "SELECT id, vec_distance_cosine(embedding, vec_f32(?)) AS distance, text, metadata
         FROM {}
         WHERE {where_clause}
         ORDER BY distance
         LIMIT ?",
        existing.table_name
    );

    let params = params_from_iter(values.iter().map(|value| value.as_ref() as &dyn ToSql));
    let mut statement = conn.prepare(&sql).map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(params, |row| {
            let metadata_text: Option<String> = row.get(3)?;
            let metadata = parse_metadata_text(metadata_text.as_deref());
            Ok(json!({
                "id": row.get::<_, String>(0)?,
                "distance": row.get::<_, f64>(1)?,
                "text": row.get::<_, Option<String>>(2)?,
                "metadata": metadata,
            }))
        })
        .map_err(|error| error.to_string())?;

    let mut results = Vec::new();
    for row in rows {
        results.push(row.map_err(|error| error.to_string())?);
    }

    Ok(Map::from_iter([
        ("collection".to_string(), json!(collection)),
        ("results".to_string(), Value::Array(results)),
    ]))
}
```

Refactor the existing metadata filter loop out of `search_generated_text_filtered` into this helper so both filtered vector search and hybrid search use the same validation:

```rust
fn append_metadata_filter_clauses(
    filter: &Map<String, Value>,
    clauses: &mut Vec<String>,
    values: &mut Vec<Box<dyn ToSql>>,
) -> Result<(), String> {
    for (key, value) in filter {
        validate_filter_key(key)?;
        let path = format!("$.{key}");
        match value {
            Value::String(value) => {
                clauses.push(format!(
                    "json_type(metadata, '{path}') = 'text' AND json_extract(metadata, '{path}') = ?"
                ));
                values.push(Box::new(value.clone()));
            }
            Value::Number(value) => {
                if let Some(value) = value.as_i64() {
                    clauses.push(format!(
                        "json_type(metadata, '{path}') = 'integer' AND json_extract(metadata, '{path}') = ?"
                    ));
                    values.push(Box::new(value));
                } else if let Some(value) = value.as_u64() {
                    let value = i64::try_from(value)
                        .map_err(|_| "filter integer is too large".to_string())?;
                    clauses.push(format!(
                        "json_type(metadata, '{path}') = 'integer' AND json_extract(metadata, '{path}') = ?"
                    ));
                    values.push(Box::new(value));
                } else if let Some(value) = value.as_f64() {
                    clauses.push(format!(
                        "json_type(metadata, '{path}') = 'real' AND json_extract(metadata, '{path}') = ?"
                    ));
                    values.push(Box::new(value));
                }
            }
            Value::Bool(value) => {
                let json_type = if *value { "true" } else { "false" };
                clauses.push(format!("json_type(metadata, '{path}') = '{json_type}'"));
            }
            Value::Null => {
                clauses.push(format!("json_type(metadata, '{path}') = 'null'"));
            }
            Value::Array(_) | Value::Object(_) => {
                return Err("filter values must be scalar JSON values".to_string());
            }
        }
    }
    Ok(())
}
```

- [ ] **Step 6: Run the hybrid vector tests**

Run:

```bash
cargo test --test vector_collections search_generated_text_hybrid_filters_fts_tags_and_metadata_then_sorts_by_vector search_generated_text_filters_metadata
```

Expected: both tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/vector.rs tests/vector_collections.rs
git commit -m "feat: add generated hybrid text search"
```

---

### Task 4: Add Executor And MCP Tool

**Files:**
- Modify: `src/sqlite.rs`
- Modify: `src/mcp.rs`
- Modify: `tests/mcp_http.rs`

- [ ] **Step 1: Add executor method**

In `src/sqlite.rs`, import `SearchGeneratedHybridTextInput` with the other vector inputs and add:

```rust
pub async fn search_generated_text_hybrid(
    &self,
    input: SearchGeneratedHybridTextInput,
) -> VectorToolResponse {
    self.execute_vector(VectorOperation::SearchGeneratedTextHybrid(input))
        .await
}
```

- [ ] **Step 2: Add MCP-facing input**

In `src/vector.rs`, add near `SearchTextInput`:

```rust
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize, PartialEq)]
pub struct SearchHybridTextInput {
    pub collection: String,
    pub query: String,
    pub top_k: usize,
    pub filter: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fts_query: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}
```

In `src/mcp.rs`, import `SearchGeneratedHybridTextInput` and `SearchHybridTextInput`.

- [ ] **Step 3: Add `search_text_hybrid` tool**

In `src/mcp.rs`, add this method next to `search_text`:

```rust
#[tool(
    name = "search_text_hybrid",
    description = "Search a text embedding collection using metadata, FTS5 trigram, tags, and cosine distance"
)]
async fn search_text_hybrid(
    &self,
    Parameters(input): Parameters<SearchHybridTextInput>,
) -> CallToolResult {
    let start = Instant::now();
    if input.query.trim().is_empty() {
        return vector_failure(start, "query must not be empty");
    }

    let description = self
        .executor
        .describe_text_collection(DescribeTextCollectionInput {
            collection: input.collection.clone(),
        })
        .await;
    if !description.success {
        return timed_vector_result(start, description);
    }
    let dimension = match dimension_from_response(&description) {
        Ok(dimension) => dimension,
        Err(message) => return vector_failure(start, message),
    };

    let embedding = match self.embed(&[input.query]).await.and_then(first_embedding) {
        Ok(embedding) => embedding,
        Err(message) => return vector_failure(start, message),
    };
    if let Err(message) = validate_embedding_dimension(&embedding, dimension) {
        return vector_failure(start, message);
    }

    let response = self
        .executor
        .search_generated_text_hybrid(SearchGeneratedHybridTextInput {
            collection: input.collection,
            vector: embedding,
            top_k: input.top_k,
            filter: input.filter,
            fts_query: input.fts_query,
            tags: input.tags,
        })
        .await;
    timed_vector_result(start, response)
}
```

- [ ] **Step 4: Update MCP tool-list test**

In `tests/mcp_http.rs`, update the expected tool list in `mcp_lists_execute_sql_and_vector_tools` to include:

```rust
"search_text_hybrid",
```

- [ ] **Step 5: Add HTTP smoke test for the new tool**

Add a test that mirrors the existing `upsert_texts` HTTP path but calls `search_text_hybrid` with:

```json
{
  "collection": "docs",
  "query": "克制压迫感",
  "top_k": 5,
  "filter": {"tenant": "novel"},
  "fts_query": "殿中喧哗",
  "tags": ["女主", "克制"]
}
```

Expected response content:

```rust
assert!(body["success"].as_bool().unwrap());
assert_eq!(body["results"].as_array().unwrap()[0]["id"], json!("doc-a"));
```

- [ ] **Step 6: Run MCP tests**

Run:

```bash
cargo test --test mcp_http
```

Expected: all MCP HTTP tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/sqlite.rs src/vector.rs src/mcp.rs tests/mcp_http.rs
git commit -m "feat: expose hybrid text search tool"
```

---

### Task 5: Tighten Validation And Regression Coverage

**Files:**
- Modify: `tests/vector_collections.rs`
- Modify: `src/vector.rs`

- [ ] **Step 1: Add validation regression tests**

Add this test near `validation_rejects_invalid_inputs`:

```rust
#[tokio::test]
async fn hybrid_search_rejects_invalid_tags_and_metadata_tags() {
    let (_dir, path) = temp_db_path("hybrid_validation.db");
    let exec = executor(path, RunMode::Readwrite, 500, 100).await;

    let create = exec
        .create_text_collection_with_dimension(CreateTextCollectionStorageInput {
            collection: "docs".to_string(),
            dimension: 2,
        })
        .await;
    assert!(create.success, "{create:?}");

    let bad_metadata_tags = exec
        .upsert_generated_texts(UpsertGeneratedTextsInput {
            collection: "docs".to_string(),
            items: vec![GeneratedTextItemInput {
                id: "doc-a".to_string(),
                vector: vec![1.0, 0.0],
                text: "text".to_string(),
                metadata: Some(json!({"tags": ["ok", 7]})),
            }],
        })
        .await;
    assert!(!bad_metadata_tags.success, "{bad_metadata_tags:?}");
    assert!(vector_error_message(&bad_metadata_tags).contains("metadata.tags"));

    let bad_search_tag = exec
        .search_generated_text_hybrid(SearchGeneratedHybridTextInput {
            collection: "docs".to_string(),
            vector: vec![1.0, 0.0],
            top_k: 1,
            filter: None,
            fts_query: None,
            tags: vec![" ".to_string()],
        })
        .await;
    assert!(!bad_search_tag.success, "{bad_search_tag:?}");
    assert!(vector_error_message(&bad_search_tag).contains("tags must not contain empty"));
}
```

- [ ] **Step 2: Verify validation tests pass**

Run:

```bash
cargo test --test vector_collections hybrid_search_rejects_invalid_tags_and_metadata_tags validation_rejects_invalid_inputs
```

Expected: both tests pass.

- [ ] **Step 3: Commit**

```bash
git add src/vector.rs tests/vector_collections.rs
git commit -m "test: cover hybrid search validation"
```

---

### Task 6: Document The Generic Hybrid Workflow

**Files:**
- Modify: `README.md`
- Modify: `README_ZH.md`

- [ ] **Step 1: Update tool lists**

Add `search_text_hybrid` to both README tool lists:

```text
search_text_hybrid
```

Add a summary row:

```markdown
| `search_text_hybrid` | Embed a query internally, optionally filter by metadata, FTS5 trigram text, and metadata tags, then rank by cosine distance. | `readonly` and `readwrite`. |
```

- [ ] **Step 2: Document metadata tags**

Add this note near the text collection docs:

Hybrid search reads tags from `metadata.tags` when it is an array of strings. For example:

```json
{
  "tenant": "novel",
  "dimension": "高潮燃点",
  "score": 9,
  "tags": ["女主", "克制", "压迫感"]
}
```

- [ ] **Step 3: Document `search_text_hybrid` input**

Add this JSON example:

```json
{
  "collection": "novel_analysis",
  "query": "克制但有压迫感的女主爆发",
  "top_k": 10,
  "filter": {
    "tenant": "novel",
    "dimension": "高潮燃点"
  },
  "fts_query": "殿中喧哗",
  "tags": ["女主", "克制"]
}
```

State explicitly:

```markdown
`fts_query` is treated as plain text, split on whitespace, quoted for FTS5, and combined with `AND`. FTS5 trigram is useful for Chinese phrase and substring matching, but very short queries may be better expressed as tags or metadata filters.
```

- [ ] **Step 4: Run docs-adjacent checks**

Run:

```bash
cargo test --test mcp_http mcp_lists_execute_sql_and_vector_tools
```

Expected: the documented tool list matches the exposed MCP tool list.

- [ ] **Step 5: Commit**

```bash
git add README.md README_ZH.md tests/mcp_http.rs
git commit -m "docs: document hybrid text search"
```

---

### Task 7: Full Verification

**Files:**
- No edits unless verification exposes a defect.

- [ ] **Step 1: Format**

Run:

```bash
cargo fmt
```

Expected: command exits 0.

- [ ] **Step 2: Run full test suite**

Run:

```bash
cargo test
```

Expected: all unit, integration, and doc tests pass.

- [ ] **Step 3: Inspect git status**

Run:

```bash
git status --short --branch
```

Expected: branch is the implementation branch and only intended files are modified or committed.

- [ ] **Step 4: Commit any final formatting-only changes**

If `cargo fmt` changed files after the previous commits, run:

```bash
git add src/vector.rs src/sqlite.rs src/mcp.rs tests/vector_collections.rs tests/mcp_http.rs README.md README_ZH.md
git commit -m "chore: format hybrid search changes"
```

If `cargo fmt` did not change files, do not create an empty commit.

---

## Self-Review

- Spec coverage: the plan removes the need for libSQL/Turso, keeps SQLite + sqlite-vec, adds FTS5 trigram, supports tags without fixed business fields, and exposes a generic hybrid search tool.
- Placeholder scan: no task depends on undefined table names; sidecar names are `fts_<collection>` and `tags_<collection>`; the new MCP tool is explicitly named `search_text_hybrid`.
- Type consistency: MCP input uses `SearchHybridTextInput`; executor/generated-vector input uses `SearchGeneratedHybridTextInput`; vector operation variant is `SearchGeneratedTextHybrid`.
