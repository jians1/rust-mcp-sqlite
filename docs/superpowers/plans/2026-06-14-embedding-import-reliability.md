# Embedding Import Reliability Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `upsert_texts` reliable for bulk writing-reference imports by adding automatic embedding batching, transient 429/5xx retry with backoff, batch/item-level error context, and safe embedding request latency logs.

**Architecture:** Keep the existing SQLite + sqlite-vec + FTS5 schema and MCP tool surface. Add low-level HTTP retry/logging inside `EmbeddingClient`, expose a configured embedding batch size, and change `SqliteMcpServer::upsert_texts` to embed items in chunks while preserving all-or-nothing SQLite writes after every embedding has been generated and validated.

**Tech Stack:** Rust 2024, clap, reqwest, tokio, tracing, axum test servers, rmcp, rusqlite, sqlite-vec.

---

## File Structure

- Modify `src/config.rs`
  - Add `DEFAULT_EMBEDDING_BATCH_SIZE`.
  - Add `--embedding-batch-size` to `Cli`.
  - Add `batch_size` to `EmbeddingRuntimeConfig`.
- Modify `src/main.rs`
  - Include `embedding_batch_size` in startup logs.
- Modify `src/embedding.rs`
  - Add retry/backoff constants.
  - Add `EmbeddingClient::batch_size()`.
  - Route embedding HTTP sends through a retry-aware helper.
  - Log model, input count, attempt, status/error class, and elapsed time without logging text or API keys.
- Modify `src/mcp.rs`
  - Split `upsert_texts` embedding calls into configured batches.
  - Preserve DB write after all batches succeed and validate.
  - Add batch range and item id context for embedding failures, response count mismatches, and dimension mismatches.
- Modify `tests/config_cli.rs`
  - Cover default, override, help text, and zero rejection for `--embedding-batch-size`.
- Modify `tests/embedding_client.rs`
  - Cover 429 retry, 5xx retry, no retry for non-transient status, and existing dimensions fallback behavior.
- Modify `tests/mcp_http.rs`
  - Cover configured batching across `upsert_texts`.
  - Cover item-level dimension mismatch error context.
- Modify `README.md` and `README_ZH.md`
  - Document `--embedding-batch-size`, retry behavior, batching, and safe logs.

## Implementation Decisions

- Default embedding batch size: `64`.
- Valid batch size range: `>= 1`; reject `0` at CLI parse time.
- Retry policy: max `3` total attempts per HTTP embedding request payload.
- Retry statuses: `429 Too Many Requests` and any `5xx`.
- Do not retry `400` except the existing dimensions-unsupported fallback path.
- Do not retry `401`, `403`, or malformed successful responses.
- Backoff: deterministic exponential backoff, `100ms`, then `200ms`, capped at `1000ms`. No jitter in this phase so tests stay simple and deterministic.
- Do not add import concurrency in this phase. Sequential batches are enough for a personal VPS and keep rate-limit behavior predictable.
- Do not log input text, metadata, full response bodies, or API keys.

---

### Task 1: Add Embedding Batch Size Configuration

**Files:**
- Modify: `src/config.rs`
- Modify: `src/main.rs`
- Test: `tests/config_cli.rs`

- [ ] **Step 1: Write failing CLI/config tests**

Edit `tests/config_cli.rs`:

```rust
use clap::Parser;
use sqlite_mcp_rs::config::{Cli, DEFAULT_EMBEDDING_BATCH_SIZE, RunMode};
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    process::Stdio,
    thread,
    time::{Duration, Instant},
};

#[test]
fn cli_defaults_match_spec() {
    let cli = Cli::parse_from(["sqlite-mcp-rs", "--db", "/tmp/app.db"]);

    assert_eq!(cli.db.to_string_lossy(), "/tmp/app.db");
    assert_eq!(cli.host.to_string(), "127.0.0.1");
    assert_eq!(cli.port, 3000);
    assert_eq!(cli.mode, RunMode::Readwrite);
    assert_eq!(cli.auth_token, None);
    assert_eq!(cli.max_rows, 500);
    assert_eq!(cli.max_top_k, 100);
    assert_eq!(cli.timeout_ms, 10_000);
    assert_eq!(cli.embedding_base_url, "https://api.openai.com/v1");
    assert_eq!(cli.embedding_api_key, None);
    assert_eq!(cli.embedding_model, None);
    assert_eq!(cli.embedding_dimensions, None);
    assert_eq!(cli.embedding_timeout_ms, 30_000);
    assert_eq!(cli.embedding_batch_size, DEFAULT_EMBEDDING_BATCH_SIZE);
}
```

In `cli_accepts_readonly_and_overrides`, add the flag and assertion:

```rust
        "--embedding-timeout-ms",
        "2500",
        "--embedding-batch-size",
        "7",
```

```rust
    assert_eq!(cli.embedding_timeout_ms, 2500);
    assert_eq!(cli.embedding_batch_size, 7);
```

In `runtime_config_reads_openai_api_key_when_embedding_api_key_is_absent`, add:

```rust
    assert_eq!(config.embedding.batch_size, DEFAULT_EMBEDDING_BATCH_SIZE);
```

In `binary_help_mentions_expected_flags`, extend the assertion chain:

```rust
        .stdout(predicates::str::contains("--embedding-timeout-ms"))
        .stdout(predicates::str::contains("--embedding-batch-size"));
```

Add a new parse rejection test:

```rust
#[test]
fn cli_rejects_zero_embedding_batch_size() {
    let err = Cli::try_parse_from([
        "sqlite-mcp-rs",
        "--db",
        "/tmp/app.db",
        "--embedding-batch-size",
        "0",
    ])
    .unwrap_err();

    assert!(err.to_string().contains("0"), "{err}");
}
```

- [ ] **Step 2: Run config tests and verify failure**

Run:

```bash
cargo test --test config_cli
```

Expected: FAIL because `DEFAULT_EMBEDDING_BATCH_SIZE`, `Cli::embedding_batch_size`, and `EmbeddingRuntimeConfig::batch_size` do not exist yet.

- [ ] **Step 3: Implement config field**

Edit `src/config.rs`:

```rust
pub const DEFAULT_EMBEDDING_BATCH_SIZE: usize = 64;
```

Add this field to `Cli` after `embedding_timeout_ms`:

```rust
    #[arg(
        long,
        default_value_t = DEFAULT_EMBEDDING_BATCH_SIZE,
        value_parser = clap::value_parser!(usize).range(1..)
    )]
    pub embedding_batch_size: usize,
```

Add this field to `EmbeddingRuntimeConfig`:

```rust
    pub batch_size: usize,
```

Set it in `impl From<Cli> for RuntimeConfig`:

```rust
                batch_size: cli.embedding_batch_size,
```

Edit `src/main.rs` startup log fields:

```rust
        embedding_timeout_ms = runtime.embedding.timeout_ms,
        embedding_batch_size = runtime.embedding.batch_size,
```

- [ ] **Step 4: Update compile call sites**

Every `EmbeddingRuntimeConfig { ... }` literal in tests must include `batch_size`. Use the default unless the test is specifically testing batching:

```rust
use sqlite_mcp_rs::config::{DEFAULT_EMBEDDING_BATCH_SIZE, EmbeddingRuntimeConfig};
```

```rust
        batch_size: DEFAULT_EMBEDDING_BATCH_SIZE,
```

- [ ] **Step 5: Run config tests and commit**

Run:

```bash
cargo test --test config_cli
```

Expected: PASS.

Commit:

```bash
git add src/config.rs src/main.rs tests/config_cli.rs tests/embedding_client.rs tests/mcp_http.rs
git commit -m "feat: configure embedding batch size"
```

---

### Task 2: Add 429/5xx Retry and Safe Latency Logs

**Files:**
- Modify: `src/embedding.rs`
- Test: `tests/embedding_client.rs`

- [ ] **Step 1: Add failing retry tests**

Edit `tests/embedding_client.rs` imports:

```rust
use sqlite_mcp_rs::{
    config::{DEFAULT_EMBEDDING_BATCH_SIZE, EmbeddingRuntimeConfig},
    embedding::{EMBEDDINGS_PATH, EmbeddingClient},
};
```

Add these tests before the helper structs:

```rust
#[tokio::test]
async fn embedding_client_retries_429_then_succeeds() {
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let server = spawn_embedding_status_sequence_server(
        requests.clone(),
        vec![
            (StatusCode::TOO_MANY_REQUESTS, json!({"error": {"message": "rate limited"}})),
            (StatusCode::OK, json!({"data": [{"index": 0, "embedding": [0.1, 0.2]}]})),
        ],
    )
    .await;

    let client = EmbeddingClient::new(EmbeddingRuntimeConfig {
        base_url: server.base_url(),
        api_key: None,
        model: Some("model".to_string()),
        dimensions: None,
        timeout_ms: 5_000,
        batch_size: DEFAULT_EMBEDDING_BATCH_SIZE,
    })
    .unwrap();

    let embeddings = client.embed(&["alpha".to_string()]).await.unwrap();

    assert_eq!(embeddings, vec![vec![0.1, 0.2]]);
    assert_eq!(requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn embedding_client_retries_5xx_then_succeeds() {
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let server = spawn_embedding_status_sequence_server(
        requests.clone(),
        vec![
            (StatusCode::INTERNAL_SERVER_ERROR, json!({"error": {"message": "temporary"}})),
            (StatusCode::BAD_GATEWAY, json!({"error": {"message": "gateway"}})),
            (StatusCode::OK, json!({"data": [{"index": 0, "embedding": [0.3, 0.4]}]})),
        ],
    )
    .await;

    let client = EmbeddingClient::new(EmbeddingRuntimeConfig {
        base_url: server.base_url(),
        api_key: None,
        model: Some("model".to_string()),
        dimensions: None,
        timeout_ms: 5_000,
        batch_size: DEFAULT_EMBEDDING_BATCH_SIZE,
    })
    .unwrap();

    let embeddings = client.embed(&["alpha".to_string()]).await.unwrap();

    assert_eq!(embeddings, vec![vec![0.3, 0.4]]);
    assert_eq!(requests.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn embedding_client_does_not_retry_401() {
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let server = spawn_embedding_status_sequence_server(
        requests.clone(),
        vec![(StatusCode::UNAUTHORIZED, json!({"error": {"message": "invalid api key"}}))],
    )
    .await;

    let client = EmbeddingClient::new(EmbeddingRuntimeConfig {
        base_url: server.base_url(),
        api_key: Some("secret".to_string()),
        model: Some("model".to_string()),
        dimensions: None,
        timeout_ms: 5_000,
        batch_size: DEFAULT_EMBEDDING_BATCH_SIZE,
    })
    .unwrap();

    let err = client.embed(&["alpha".to_string()]).await.unwrap_err();

    assert!(err.contains("401 Unauthorized"), "{err}");
    assert!(err.contains("invalid api key"), "{err}");
    assert!(!err.contains("secret"), "{err}");
    assert_eq!(requests.lock().unwrap().len(), 1);
}
```

Add this helper near `spawn_embedding_server`:

```rust
async fn spawn_embedding_status_sequence_server(
    requests: Arc<Mutex<Vec<Value>>>,
    responses: Vec<(StatusCode, Value)>,
) -> TestEmbeddingServer {
    let shutdown = CancellationToken::new();
    let responses = Arc::new(Mutex::new(responses));
    let app = Router::new().route(
        TEST_EMBEDDINGS_ROUTE,
        post({
            let requests = requests.clone();
            let responses = responses.clone();
            move |Json(body): Json<Value>| {
                let requests = requests.clone();
                let responses = responses.clone();
                async move {
                    requests.lock().unwrap().push(body);
                    let mut responses = responses.lock().unwrap();
                    let (status, body) = responses.remove(0);
                    (status, Json(body)).into_response()
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_shutdown = shutdown.clone();

    tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                server_shutdown.cancelled_owned().await;
            })
            .await;
    });

    TestEmbeddingServer {
        base_url: format!("http://{addr}/v1"),
        shutdown,
    }
}
```

- [ ] **Step 2: Run embedding client tests and verify failure**

Run:

```bash
cargo test --test embedding_client
```

Expected: FAIL because `EmbeddingClient` returns immediately on 429/5xx.

- [ ] **Step 3: Implement retry constants and batch accessor**

Edit `src/embedding.rs` imports:

```rust
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
```

Add constants after `EMBEDDINGS_PATH`:

```rust
const EMBEDDING_MAX_ATTEMPTS: usize = 3;
const EMBEDDING_RETRY_BASE_DELAY_MS: u64 = 100;
const EMBEDDING_RETRY_MAX_DELAY_MS: u64 = 1_000;
```

Add a public accessor:

```rust
    pub fn batch_size(&self) -> usize {
        self.config.batch_size
    }
```

- [ ] **Step 4: Route HTTP sends through retry helper**

Replace calls to `post_embedding_request` in `embed()` with `post_embedding_request_with_retry()`:

```rust
        let (status, text) = self.post_embedding_request_with_retry(&request).await?;
```

```rust
            let (retry_status, retry_text) = self
                .post_embedding_request_with_retry(&retry_request)
                .await?;
```

Rename the existing one-shot method:

```rust
    async fn post_embedding_request_once(
        &self,
        request: &EmbeddingRequest<'_>,
    ) -> Result<(StatusCode, String), String> {
```

Add retry helper methods inside `impl EmbeddingClient`:

```rust
    async fn post_embedding_request_with_retry(
        &self,
        request: &EmbeddingRequest<'_>,
    ) -> Result<(StatusCode, String), String> {
        let mut attempt = 1;
        loop {
            let started = Instant::now();
            let response = self.post_embedding_request_once(request).await;
            let elapsed_ms = started.elapsed().as_millis() as u64;

            match response {
                Ok((status, text)) => {
                    tracing::info!(
                        model = %request.model,
                        input_count = request.input.len(),
                        attempt,
                        status = %status,
                        elapsed_ms,
                        retryable = should_retry_status(status),
                        "embedding HTTP request completed"
                    );
                    if should_retry_status(status) && attempt < EMBEDDING_MAX_ATTEMPTS {
                        tokio::time::sleep(retry_delay(attempt)).await;
                        attempt += 1;
                        continue;
                    }
                    return Ok((status, text));
                }
                Err(message) => {
                    tracing::info!(
                        model = %request.model,
                        input_count = request.input.len(),
                        attempt,
                        elapsed_ms,
                        error = %message,
                        "embedding HTTP request failed"
                    );
                    return Err(message);
                }
            }
        }
    }
```

Add helper functions after the `impl` block:

```rust
fn should_retry_status(status: StatusCode) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

fn retry_delay(attempt: usize) -> Duration {
    let shift = attempt.saturating_sub(1).min(20) as u32;
    let multiplier = 1_u64 << shift;
    let delay_ms = EMBEDDING_RETRY_BASE_DELAY_MS
        .saturating_mul(multiplier)
        .min(EMBEDDING_RETRY_MAX_DELAY_MS);
    Duration::from_millis(delay_ms)
}
```

- [ ] **Step 5: Keep dimensions fallback behavior covered**

Run:

```bash
cargo test --test embedding_client embedding_client_retries_without_dimensions_after_bad_request
cargo test --test embedding_client embedding_client_caches_dimensions_bad_request_fallback_across_clones
```

Expected: PASS. Request counts stay `2` and `3`; `400 Bad Request` is still not retried, it only triggers the dimensions fallback when dimensions were present.

- [ ] **Step 6: Run embedding client tests and commit**

Run:

```bash
cargo test --test embedding_client
```

Expected: PASS.

Commit:

```bash
git add src/embedding.rs tests/embedding_client.rs
git commit -m "feat: retry transient embedding failures"
```

---

### Task 3: Batch `upsert_texts` and Add Failure Context

**Files:**
- Modify: `src/mcp.rs`
- Modify: `tests/mcp_http.rs`

- [ ] **Step 1: Add failing HTTP batching test**

Edit `tests/mcp_http.rs` imports:

```rust
use sqlite_mcp_rs::config::{DEFAULT_EMBEDDING_BATCH_SIZE, EmbeddingRuntimeConfig, RunMode};
```

Add a configurable client helper:

```rust
impl TestEmbeddingServer {
    fn client(&self) -> EmbeddingClient {
        self.client_with_batch_size(DEFAULT_EMBEDDING_BATCH_SIZE)
    }

    fn client_with_batch_size(&self, batch_size: usize) -> EmbeddingClient {
        EmbeddingClient::new(EmbeddingRuntimeConfig {
            base_url: self.base_url.clone(),
            api_key: None,
            model: Some("test-embedding".to_string()),
            dimensions: Some(2),
            timeout_ms: 5_000,
            batch_size,
        })
        .unwrap()
    }
}
```

Replace the old `client()` method body with the two methods above.

Add this test before the helper structs:

```rust
#[tokio::test]
async fn upsert_texts_embeds_in_configured_batches_and_writes_all_items() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("mcp_batch_upsert.db");
    let executor = SqliteExecutor::open(ExecutorConfig {
        db_path,
        mode: RunMode::Readwrite,
        max_rows: 500,
        max_top_k: 100,
        timeout_ms: 10_000,
    })
    .unwrap();
    let embedding_server = spawn_test_embedding_server().await;

    let server = spawn_test_server(
        executor,
        None,
        Some(embedding_server.client_with_batch_size(2)),
    )
    .await
    .unwrap();
    let client = Client::new();
    initialize(&client, &server.url()).await;

    let create = call_tool(
        &client,
        &server.url(),
        2,
        "create_text_collection",
        json!({"collection": "docs"}),
    )
    .await;
    assert_eq!(create["success"], true);

    let upsert = call_tool(
        &client,
        &server.url(),
        3,
        "upsert_texts",
        json!({
            "collection": "docs",
            "items": [
                {"id": "doc-a", "text": "alpha"},
                {"id": "doc-b", "text": "beta"},
                {"id": "doc-c", "text": "gamma"},
                {"id": "doc-d", "text": "delta"},
                {"id": "doc-e", "text": "epsilon"}
            ]
        }),
    )
    .await;

    assert_eq!(upsert["success"], true);
    assert_eq!(upsert["upserted_count"], 5);

    let count = call_tool(
        &client,
        &server.url(),
        4,
        "execute_sql",
        json!({"sql": "SELECT COUNT(*) AS count FROM vec_docs"}),
    )
    .await;
    assert_eq!(count["success"], true);
    assert_eq!(count["results"][0]["rows"][0]["count"], 5);

    let requests = embedding_server.requests.lock().unwrap();
    assert_eq!(requests.len(), 4);
    assert_eq!(requests[0]["input"], json!(["sqlite-mcp-rs embedding dimension probe"]));
    assert_eq!(requests[1]["input"], json!(["alpha", "beta"]));
    assert_eq!(requests[2]["input"], json!(["gamma", "delta"]));
    assert_eq!(requests[3]["input"], json!(["epsilon"]));
}
```

- [ ] **Step 2: Add failing item-context test**

Add this test:

```rust
#[tokio::test]
async fn upsert_texts_reports_batch_and_item_id_for_dimension_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("mcp_batch_error_context.db");
    let executor = SqliteExecutor::open(ExecutorConfig {
        db_path,
        mode: RunMode::Readwrite,
        max_rows: 500,
        max_top_k: 100,
        timeout_ms: 10_000,
    })
    .unwrap();
    let embedding_server = spawn_sequence_embedding_server(vec![
        json!({"data": [{"index": 0, "embedding": [1.0, 0.0]}]}),
        json!({
            "data": [
                {"index": 0, "embedding": [1.0, 0.0]},
                {"index": 1, "embedding": [1.0, 0.0, 0.0]}
            ]
        }),
    ])
    .await;

    let server = spawn_test_server(
        executor,
        None,
        Some(embedding_server.client_with_batch_size(2)),
    )
    .await
    .unwrap();
    let client = Client::new();
    initialize(&client, &server.url()).await;

    let create = call_tool(
        &client,
        &server.url(),
        2,
        "create_text_collection",
        json!({"collection": "docs"}),
    )
    .await;
    assert_eq!(create["success"], true);

    let upsert = call_tool(
        &client,
        &server.url(),
        3,
        "upsert_texts",
        json!({
            "collection": "docs",
            "items": [
                {"id": "doc-a", "text": "alpha"},
                {"id": "doc-b", "text": "beta"}
            ]
        }),
    )
    .await;

    assert_eq!(upsert["success"], false);
    let message = upsert["error"]["message"].as_str().unwrap();
    assert!(message.contains("upsert_texts batch 1"), "{message}");
    assert!(message.contains("items 1-2"), "{message}");
    assert!(message.contains("doc-b"), "{message}");
    assert!(message.contains("embedding dimension mismatch"), "{message}");
}
```

- [ ] **Step 3: Run HTTP tests and verify failure**

Run:

```bash
cargo test --test mcp_http upsert_texts_embeds_in_configured_batches_and_writes_all_items
cargo test --test mcp_http upsert_texts_reports_batch_and_item_id_for_dimension_mismatch
```

Expected: first test FAILS because current `upsert_texts` sends all items in one embedding request; second test FAILS because the current dimension mismatch error lacks batch/id context.

- [ ] **Step 4: Add embedding client accessors in MCP server**

Edit `src/mcp.rs`:

```rust
impl SqliteMcpServer {
    fn embedding_client(&self) -> Result<&EmbeddingClient, String> {
        self.embeddings
            .as_ref()
            .ok_or_else(|| "embedding is not configured; set --embedding-model".to_string())
    }

    async fn embed(&self, input: &[String]) -> Result<Vec<Vec<f64>>, String> {
        self.embedding_client()?.embed(input).await
    }

    fn embedding_batch_size(&self) -> Result<usize, String> {
        Ok(self.embedding_client()?.batch_size())
    }
}
```

- [ ] **Step 5: Replace one-shot upsert embedding with batches**

Replace the start of `upsert_texts` after readonly check with destructuring:

```rust
        let UpsertTextsInput { collection, items } = input;
        if let Err(message) = validate_text_items(&items) {
            return vector_failure(start, message);
        }
```

Update collection description to use `collection.clone()`:

```rust
                collection: collection.clone(),
```

Replace the one-shot `texts`/`embeddings` block with:

```rust
        let batch_size = match self.embedding_batch_size() {
            Ok(batch_size) => batch_size,
            Err(message) => return vector_failure(start, message),
        };

        let mut generated_items = Vec::with_capacity(items.len());
        for (batch_index, batch) in items.chunks(batch_size).enumerate() {
            let batch_start_index = batch_index * batch_size;
            let texts = batch.iter().map(|item| item.text.clone()).collect::<Vec<_>>();
            let embeddings = match self.embed(&texts).await {
                Ok(embeddings) => embeddings,
                Err(message) => {
                    return vector_failure(
                        start,
                        upsert_batch_error(batch_index, batch_start_index, batch, &message),
                    );
                }
            };
            if embeddings.len() != batch.len() {
                return vector_failure(
                    start,
                    upsert_batch_error(
                        batch_index,
                        batch_start_index,
                        batch,
                        &format!(
                            "embedding response count mismatch: expected {}, got {}",
                            batch.len(),
                            embeddings.len()
                        ),
                    ),
                );
            }

            for (item_offset, (item, embedding)) in batch.iter().zip(embeddings).enumerate() {
                if let Err(message) = validate_embedding_dimension(&embedding, dimension) {
                    return vector_failure(
                        start,
                        upsert_item_error(
                            batch_index,
                            batch_start_index,
                            batch,
                            item_offset,
                            &message,
                        ),
                    );
                }
                generated_items.push(GeneratedTextItemInput {
                    id: item.id.clone(),
                    text: item.text.clone(),
                    vector: embedding,
                    metadata: item.metadata.clone(),
                });
            }
        }
```

Update the final executor call to use the destructured collection:

```rust
                collection,
```

- [ ] **Step 6: Add batch/id error helpers**

Add these helpers near `validate_text_items` in `src/mcp.rs`:

```rust
fn upsert_batch_error(
    batch_index: usize,
    batch_start_index: usize,
    batch: &[TextItemInput],
    message: &str,
) -> String {
    format!(
        "upsert_texts batch {} failed (items {}, ids: {}): {}",
        batch_index + 1,
        item_range(batch_start_index, batch),
        summarize_item_ids(batch),
        message
    )
}

fn upsert_item_error(
    batch_index: usize,
    batch_start_index: usize,
    batch: &[TextItemInput],
    item_offset: usize,
    message: &str,
) -> String {
    let item = &batch[item_offset];
    format!(
        "upsert_texts batch {} failed (items {}, item id: {}): {}",
        batch_index + 1,
        item_range(batch_start_index, batch),
        item.id,
        message
    )
}

fn item_range(batch_start_index: usize, batch: &[TextItemInput]) -> String {
    let start = batch_start_index + 1;
    let end = batch_start_index + batch.len();
    format!("{start}-{end}")
}

fn summarize_item_ids(batch: &[TextItemInput]) -> String {
    let mut ids = batch
        .iter()
        .take(5)
        .map(|item| item.id.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    if batch.len() > 5 {
        ids.push_str(", ...");
    }
    ids
}
```

- [ ] **Step 7: Run HTTP tests and commit**

Run:

```bash
cargo test --test mcp_http
```

Expected: PASS.

Commit:

```bash
git add src/mcp.rs tests/mcp_http.rs
git commit -m "feat: batch text upsert embeddings"
```

---

### Task 4: Update Documentation

**Files:**
- Modify: `README.md`
- Modify: `README_ZH.md`

- [ ] **Step 1: Update CLI option tables**

In `README.md`, add:

```markdown
| `--embedding-batch-size <n>` | `64` | Maximum number of texts sent to the embedding API in one request. Must be at least `1`. |
```

In `README_ZH.md`, add:

```markdown
| `--embedding-batch-size <n>` | `64` | 单次 embedding API 请求最多发送的文本条数，必须至少为 `1`。 |
```

- [ ] **Step 2: Update `upsert_texts` behavior notes**

In `README.md`, replace the current `upsert_texts` behavior sentence with:

```markdown
The server embeds item text in batches of up to `--embedding-batch-size`, validates the generated dimensions against the collection, and replaces the whole record for the same `id`: embedding, text, and metadata. SQLite writes are issued only after every embedding batch succeeds and validates.
```

In `README_ZH.md`, replace the corresponding sentence with:

```markdown
服务器会按 `--embedding-batch-size` 分批生成 item 文本 embedding，验证生成维度和集合一致，然后替换相同 `id` 的整条记录：embedding、文本和 metadata。只有所有 embedding 批次成功并通过校验后，才会写入 SQLite。
```

- [ ] **Step 3: Document retry/log behavior**

Add a short note near the embedding tools section in `README.md`:

```markdown
Embedding HTTP requests retry transient `429` and `5xx` responses up to three total attempts with exponential backoff. Logs include model, input count, attempt, status or error class, and elapsed milliseconds; they do not include input text, metadata, or API keys.
```

Add the matching note in `README_ZH.md`:

```markdown
Embedding HTTP 请求会对临时性的 `429` 和 `5xx` 响应做最多三次总尝试，并使用指数退避。日志只包含模型名、输入条数、尝试次数、状态或错误类别、耗时毫秒数，不包含输入文本、metadata 或 API key。
```

- [ ] **Step 4: Run docs-adjacent checks and commit**

Run:

```bash
rg -n "embedding-batch-size|Embedding HTTP requests retry|Embedding HTTP 请求" README.md README_ZH.md
```

Expected: finds the new option and retry notes in both docs.

Commit:

```bash
git add README.md README_ZH.md
git commit -m "docs: describe embedding import reliability"
```

---

### Task 5: Final Verification

**Files:**
- No new source files.

- [ ] **Step 1: Format**

Run:

```bash
cargo fmt
```

Expected: no formatting errors.

- [ ] **Step 2: Run focused tests**

Run:

```bash
cargo test --test config_cli
cargo test --test embedding_client
cargo test --test mcp_http
```

Expected: all pass.

- [ ] **Step 3: Run full test suite**

Run:

```bash
cargo test
```

Expected: all tests pass.

- [ ] **Step 4: Inspect changed files**

Run:

```bash
git status --short
git diff --stat
```

Expected: clean if all task commits were made, or only intentional uncommitted edits if execution intentionally skipped commits.

- [ ] **Step 5: Final commit if needed**

If formatting or docs changed after the previous commits:

```bash
git add .
git commit -m "chore: finalize embedding import reliability"
```

---

## Deferred Work

- Concurrent embedding import with a semaphore.
- Configurable retry count/backoff knobs.
- `Retry-After` parsing for rate-limit responses.
- Structured metrics export.

These are intentionally deferred because this MCP is expected to run on a personal VPS with fewer than about 10k novel scene/chapter records. Sequential batches plus retry/backoff should cover the likely failure modes without adding avoidable operational complexity.

## Self-Review Notes

- Spec coverage: Covers batch-size control, 429/5xx retry/backoff, request latency logs, and per-batch/per-item failure traceability. Also documents the deferred concurrency decision.
- Placeholder scan: No unresolved placeholders or unspecified test-writing steps remain.
- Type consistency: `EmbeddingRuntimeConfig::batch_size`, `Cli::embedding_batch_size`, and `EmbeddingClient::batch_size()` are used consistently across config, client, MCP, tests, and docs.
