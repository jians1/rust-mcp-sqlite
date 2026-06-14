# Text Embedding Collections Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the public manual-vector MCP tools with text-first collection tools that generate embeddings through an OpenAI-compatible embeddings API.

**Architecture:** Add an async `EmbeddingClient` used by the MCP handler before work is sent to the serialized SQLite worker. Keep sqlite-vec storage behind the existing worker, but change the public MCP inputs from vectors to text, and expose only text collection tool names. Keep `execute_sql` unchanged.

**Tech Stack:** Rust 2024, tokio, axum, rmcp, rusqlite, sqlite-vec, reqwest with rustls, serde, schemars, clap.

---

## File Structure

- `Cargo.toml`: move `reqwest` into production dependencies and keep test dependencies unchanged otherwise.
- `src/config.rs`: add embedding CLI/runtime settings and environment fallback for `OPENAI_API_KEY`.
- `src/embedding.rs`: new OpenAI-compatible embeddings client with request/response parsing and validation.
- `src/lib.rs`: export the `embedding` module.
- `src/vector.rs`: replace public manual-vector input types with text-facing input types and internal generated-vector storage types.
- `src/sqlite.rs`: update `VectorOperation` usage, add describe/create/upsert/search/delete/drop methods for text collections, and store `RunMode` on `SqliteExecutor`.
- `src/mcp.rs`: inject optional `EmbeddingClient`, expose text tools, remove manual-vector tools, and include embedding time in `elapsed_ms`.
- `src/main.rs`: build embedding config/client from CLI and pass it to the MCP router.
- `tests/config_cli.rs`: cover new CLI defaults, overrides, and help text.
- `tests/embedding_client.rs`: new focused tests for OpenAI-compatible HTTP behavior and error cases.
- `tests/vector_collections.rs`: update storage-level tests to generated text vectors and text tool naming.
- `tests/mcp_http.rs`: update tool list and end-to-end MCP tool calls to text-first behavior.
- `README.md` and `README_ZH.md`: update examples and tool descriptions.

---

### Task 1: Add Embedding Runtime Configuration

**Files:**
- Modify: `src/config.rs`
- Modify: `tests/config_cli.rs`
- Modify: `Cargo.toml`

- [ ] **Step 1: Write failing CLI defaults test**

Replace the `cli_defaults_match_spec` assertions in `tests/config_cli.rs` with assertions that include embedding defaults:

```rust
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
}
```

- [ ] **Step 2: Write failing CLI override test**

Extend `cli_accepts_readonly_and_overrides` with the new flags:

```rust
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
        "--max-top-k",
        "7",
        "--timeout-ms",
        "1500",
        "--embedding-base-url",
        "http://127.0.0.1:8080/v1",
        "--embedding-api-key",
        "embedding-secret",
        "--embedding-model",
        "text-embedding-3-small",
        "--embedding-dimensions",
        "512",
        "--embedding-timeout-ms",
        "2500",
    ]);

    assert_eq!(cli.host.to_string(), "0.0.0.0");
    assert_eq!(cli.port, 3100);
    assert_eq!(cli.mode, RunMode::Readonly);
    assert_eq!(cli.auth_token.as_deref(), Some("secret-value"));
    assert_eq!(cli.max_rows, 25);
    assert_eq!(cli.max_top_k, 7);
    assert_eq!(cli.timeout_ms, 1500);
    assert_eq!(cli.embedding_base_url, "http://127.0.0.1:8080/v1");
    assert_eq!(cli.embedding_api_key.as_deref(), Some("embedding-secret"));
    assert_eq!(cli.embedding_model.as_deref(), Some("text-embedding-3-small"));
    assert_eq!(cli.embedding_dimensions, Some(512));
    assert_eq!(cli.embedding_timeout_ms, 2500);
}
```

- [ ] **Step 3: Write failing RuntimeConfig fallback test**

Add this test to `tests/config_cli.rs`:

```rust
#[test]
fn runtime_config_reads_openai_api_key_when_embedding_api_key_is_absent() {
    let original = std::env::var("OPENAI_API_KEY").ok();
    unsafe {
        std::env::set_var("OPENAI_API_KEY", "env-secret");
    }

    let cli = Cli::parse_from([
        "sqlite-mcp-rs",
        "--db",
        "/tmp/app.db",
        "--embedding-model",
        "text-embedding-3-small",
    ]);
    let config = sqlite_mcp_rs::config::RuntimeConfig::from(cli);

    assert_eq!(config.embedding.api_key.as_deref(), Some("env-secret"));

    unsafe {
        match original {
            Some(value) => std::env::set_var("OPENAI_API_KEY", value),
            None => std::env::remove_var("OPENAI_API_KEY"),
        }
    }
}
```

- [ ] **Step 4: Write failing help text test assertions**

Add these `.stdout` checks to `binary_help_mentions_expected_flags`:

```rust
.stdout(predicates::str::contains("--embedding-base-url"))
.stdout(predicates::str::contains("--embedding-api-key"))
.stdout(predicates::str::contains("--embedding-model"))
.stdout(predicates::str::contains("--embedding-dimensions"))
.stdout(predicates::str::contains("--embedding-timeout-ms"));
```

- [ ] **Step 5: Run config tests and verify RED**

Run:

```bash
cargo test --test config_cli
```

Expected: compile failure or test failure mentioning missing `embedding_base_url`, `embedding_api_key`, `embedding_model`, `embedding_dimensions`, `embedding_timeout_ms`, or `embedding` fields.

- [ ] **Step 6: Implement config structs and CLI fields**

Modify `src/config.rs` to include:

```rust
use std::{env, net::IpAddr, path::PathBuf};

use clap::{Parser, ValueEnum};

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum RunMode {
    Readonly,
    Readwrite,
}

#[derive(Clone, Debug, Parser)]
#[command(name = "sqlite-mcp-rs")]
pub struct Cli {
    #[arg(long)]
    pub db: PathBuf,
    #[arg(long, default_value = "127.0.0.1")]
    pub host: IpAddr,
    #[arg(long, default_value_t = 3000)]
    pub port: u16,
    #[arg(long, value_enum, default_value_t = RunMode::Readwrite)]
    pub mode: RunMode,
    #[arg(long)]
    pub auth_token: Option<String>,
    #[arg(long, default_value_t = 500)]
    pub max_rows: usize,
    #[arg(long, default_value_t = 100)]
    pub max_top_k: usize,
    #[arg(long, default_value_t = 10_000)]
    pub timeout_ms: u64,
    #[arg(long, default_value = "https://api.openai.com/v1")]
    pub embedding_base_url: String,
    #[arg(long)]
    pub embedding_api_key: Option<String>,
    #[arg(long)]
    pub embedding_model: Option<String>,
    #[arg(long)]
    pub embedding_dimensions: Option<usize>,
    #[arg(long, default_value_t = 30_000)]
    pub embedding_timeout_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeConfig {
    pub db: PathBuf,
    pub host: IpAddr,
    pub port: u16,
    pub mode: RunMode,
    pub auth_token: Option<String>,
    pub max_rows: usize,
    pub max_top_k: usize,
    pub timeout_ms: u64,
    pub embedding: EmbeddingRuntimeConfig,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmbeddingRuntimeConfig {
    pub base_url: String,
    pub api_key: Option<String>,
    pub model: Option<String>,
    pub dimensions: Option<usize>,
    pub timeout_ms: u64,
}

impl From<Cli> for RuntimeConfig {
    fn from(cli: Cli) -> Self {
        let embedding_api_key = cli
            .embedding_api_key
            .or_else(|| env::var("OPENAI_API_KEY").ok());

        Self {
            db: cli.db,
            host: cli.host,
            port: cli.port,
            mode: cli.mode,
            auth_token: cli.auth_token,
            max_rows: cli.max_rows,
            max_top_k: cli.max_top_k,
            timeout_ms: cli.timeout_ms,
            embedding: EmbeddingRuntimeConfig {
                base_url: cli.embedding_base_url,
                api_key: embedding_api_key,
                model: cli.embedding_model,
                dimensions: cli.embedding_dimensions,
                timeout_ms: cli.embedding_timeout_ms,
            },
        }
    }
}
```

- [ ] **Step 7: Update binary startup to use RuntimeConfig**

Modify `src/main.rs` so it converts `Cli` once:

```rust
use clap::Parser;
use sqlite_mcp_rs::{
    config::{Cli, RuntimeConfig},
    error::AppError,
    sqlite::{ExecutorConfig, SqliteExecutor},
};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), AppError> {
    let cli = Cli::parse();
    let runtime = RuntimeConfig::from(cli);
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let executor = SqliteExecutor::open(ExecutorConfig {
        db_path: runtime.db.clone(),
        mode: runtime.mode,
        max_rows: runtime.max_rows,
        max_top_k: runtime.max_top_k,
        timeout_ms: runtime.timeout_ms,
    })?;

    tracing::info!("SQLite FTS5: enabled");
    tracing::info!(
        db = %runtime.db.display(),
        host = %runtime.host,
        port = runtime.port,
        mode = ?runtime.mode,
        auth_enabled = runtime.auth_token.is_some(),
        max_rows = runtime.max_rows,
        max_top_k = runtime.max_top_k,
        timeout_ms = runtime.timeout_ms,
        embedding_model = ?runtime.embedding.model,
        embedding_base_url = %runtime.embedding.base_url,
        embedding_dimensions = ?runtime.embedding.dimensions,
        embedding_timeout_ms = runtime.embedding.timeout_ms,
        "starting sqlite-mcp-rs"
    );

    let app = sqlite_mcp_rs::mcp::router(executor, runtime.auth_token)?;
    let addr = std::net::SocketAddr::new(runtime.host, runtime.port);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}
```

- [ ] **Step 8: Run config tests and verify GREEN**

Run:

```bash
cargo test --test config_cli
```

Expected: all `config_cli` tests pass.

- [ ] **Step 9: Commit config work**

Run:

```bash
git add Cargo.toml Cargo.lock src/config.rs src/main.rs tests/config_cli.rs
git commit -m "feat: add embedding runtime config"
```

Expected: commit succeeds.

---

### Task 2: Add OpenAI-Compatible Embedding Client

**Files:**
- Modify: `Cargo.toml`
- Modify: `Cargo.lock`
- Create: `src/embedding.rs`
- Modify: `src/lib.rs`
- Create: `tests/embedding_client.rs`

- [ ] **Step 1: Move reqwest into production dependencies**

Modify `Cargo.toml` so `[dependencies]` includes:

```toml
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }
```

Remove the duplicate `reqwest` line from `[dev-dependencies]`.

- [ ] **Step 2: Write failing success test**

Create `tests/embedding_client.rs` with this initial content:

```rust
use axum::{Json, Router, routing::post};
use serde_json::{Value, json};
use sqlite_mcp_rs::{
    config::EmbeddingRuntimeConfig,
    embedding::{EmbeddingClient, EMBEDDINGS_PATH},
};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn embedding_client_posts_openai_compatible_request() {
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let server = spawn_embedding_server(requests.clone(), json!({
        "data": [
            {"index": 0, "embedding": [0.1, 0.2]},
            {"index": 1, "embedding": [0.3, 0.4]}
        ]
    }))
    .await;

    let client = EmbeddingClient::new(EmbeddingRuntimeConfig {
        base_url: server.base_url(),
        api_key: Some("secret".to_string()),
        model: Some("text-embedding-3-small".to_string()),
        dimensions: Some(2),
        timeout_ms: 5_000,
    })
    .unwrap();

    let embeddings = client
        .embed(&["alpha".to_string(), "beta".to_string()])
        .await
        .unwrap();

    assert_eq!(embeddings, vec![vec![0.1, 0.2], vec![0.3, 0.4]]);
    let captured = requests.lock().unwrap();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0]["model"], "text-embedding-3-small");
    assert_eq!(captured[0]["input"], json!(["alpha", "beta"]));
    assert_eq!(captured[0]["dimensions"], 2);
}

struct TestEmbeddingServer {
    base_url: String,
    shutdown: CancellationToken,
}

impl TestEmbeddingServer {
    fn base_url(&self) -> String {
        self.base_url.clone()
    }
}

impl Drop for TestEmbeddingServer {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

async fn spawn_embedding_server(
    requests: Arc<Mutex<Vec<Value>>>,
    response: Value,
) -> TestEmbeddingServer {
    let shutdown = CancellationToken::new();
    let app = Router::new().route(
        EMBEDDINGS_PATH,
        post({
            let requests = requests.clone();
            move |Json(body): Json<Value>| {
                let requests = requests.clone();
                let response = response.clone();
                async move {
                    requests.lock().unwrap().push(body);
                    Json(response)
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

- [ ] **Step 3: Run embedding client test and verify RED**

Run:

```bash
cargo test --test embedding_client embedding_client_posts_openai_compatible_request
```

Expected: compile failure because `sqlite_mcp_rs::embedding` does not exist.

- [ ] **Step 4: Implement minimal embedding client**

Create `src/embedding.rs`:

```rust
use std::time::Duration;

use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

use crate::config::EmbeddingRuntimeConfig;

pub const EMBEDDINGS_PATH: &str = "/embeddings";

#[derive(Clone)]
pub struct EmbeddingClient {
    config: EmbeddingRuntimeConfig,
    client: reqwest::Client,
    endpoint: String,
}

#[derive(Debug, Serialize)]
struct EmbeddingRequest<'a> {
    model: &'a str,
    input: &'a [String],
    #[serde(skip_serializing_if = "Option::is_none")]
    dimensions: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingData {
    embedding: Vec<f64>,
}

impl EmbeddingClient {
    pub fn new(config: EmbeddingRuntimeConfig) -> Result<Self, String> {
        if config.model.as_deref().is_none_or(str::is_empty) {
            return Err("embedding is not configured; set --embedding-model".to_string());
        }

        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(config.timeout_ms))
            .build()
            .map_err(|error| error.to_string())?;
        let endpoint = format!(
            "{}{}",
            config.base_url.trim_end_matches('/'),
            EMBEDDINGS_PATH
        );

        Ok(Self {
            config,
            client,
            endpoint,
        })
    }

    pub fn from_runtime_config(config: &EmbeddingRuntimeConfig) -> Option<Result<Self, String>> {
        config.model.as_ref().map(|_| Self::new(config.clone()))
    }

    pub async fn embed(&self, input: &[String]) -> Result<Vec<Vec<f64>>, String> {
        if input.is_empty() {
            return Err("embedding input must not be empty".to_string());
        }

        let model = self
            .config
            .model
            .as_deref()
            .ok_or_else(|| "embedding is not configured; set --embedding-model".to_string())?;
        let request = EmbeddingRequest {
            model,
            input,
            dimensions: self.config.dimensions,
        };
        let mut builder = self.client.post(&self.endpoint).json(&request);
        if let Some(api_key) = &self.config.api_key {
            builder = builder.bearer_auth(api_key);
        }

        let response = builder.send().await.map_err(|error| {
            if error.is_timeout() {
                "embedding HTTP request timed out".to_string()
            } else {
                format!("embedding HTTP request failed: {error}")
            }
        })?;

        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|error| format!("embedding HTTP response read failed: {error}"))?;
        if !status.is_success() {
            return Err(format_embedding_status_error(status, &text));
        }

        let parsed: EmbeddingResponse = serde_json::from_str(&text)
            .map_err(|error| format!("embedding response JSON is malformed: {error}"))?;
        if parsed.data.len() != input.len() {
            return Err(format!(
                "embedding response count mismatch: expected {}, got {}",
                input.len(),
                parsed.data.len()
            ));
        }

        Ok(parsed
            .data
            .into_iter()
            .map(|item| item.embedding)
            .collect())
    }
}

fn format_embedding_status_error(status: StatusCode, body: &str) -> String {
    let excerpt: String = body.chars().take(300).collect();
    if excerpt.is_empty() {
        format!("embedding HTTP response was not successful: {status}")
    } else {
        format!("embedding HTTP response was not successful: {status}: {excerpt}")
    }
}
```

Modify `src/lib.rs`:

```rust
pub mod auth;
pub mod config;
pub mod embedding;
pub mod error;
pub mod mcp;
pub mod response;
pub mod sql_classify;
pub mod sqlite;
pub mod vector;
```

- [ ] **Step 5: Run success test and verify GREEN**

Run:

```bash
cargo test --test embedding_client embedding_client_posts_openai_compatible_request
```

Expected: test passes.

- [ ] **Step 6: Add failing malformed/count/status tests**

Append these tests to `tests/embedding_client.rs`:

```rust
#[tokio::test]
async fn embedding_client_rejects_malformed_json() {
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let shutdown = CancellationToken::new();
    let app = Router::new().route(
        EMBEDDINGS_PATH,
        post(|| async { (http::StatusCode::OK, "not-json") }),
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

    let client = EmbeddingClient::new(EmbeddingRuntimeConfig {
        base_url: format!("http://{addr}/v1"),
        api_key: None,
        model: Some("model".to_string()),
        dimensions: None,
        timeout_ms: 5_000,
    })
    .unwrap();

    let err = client.embed(&["alpha".to_string()]).await.unwrap_err();
    assert!(err.contains("malformed"), "{err}");
    assert!(requests.lock().unwrap().is_empty());
    shutdown.cancel();
}

#[tokio::test]
async fn embedding_client_rejects_wrong_response_count() {
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let server = spawn_embedding_server(requests, json!({
        "data": [
            {"index": 0, "embedding": [0.1, 0.2]}
        ]
    }))
    .await;

    let client = EmbeddingClient::new(EmbeddingRuntimeConfig {
        base_url: server.base_url(),
        api_key: None,
        model: Some("model".to_string()),
        dimensions: None,
        timeout_ms: 5_000,
    })
    .unwrap();

    let err = client
        .embed(&["alpha".to_string(), "beta".to_string()])
        .await
        .unwrap_err();
    assert!(err.contains("count mismatch"), "{err}");
}

#[tokio::test]
async fn embedding_client_reports_non_success_status_with_body_excerpt() {
    let shutdown = CancellationToken::new();
    let app = Router::new().route(
        EMBEDDINGS_PATH,
        post(|| async { (http::StatusCode::UNAUTHORIZED, "invalid api key") }),
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

    let client = EmbeddingClient::new(EmbeddingRuntimeConfig {
        base_url: format!("http://{addr}/v1"),
        api_key: Some("secret".to_string()),
        model: Some("model".to_string()),
        dimensions: None,
        timeout_ms: 5_000,
    })
    .unwrap();

    let err = client.embed(&["alpha".to_string()]).await.unwrap_err();
    assert!(err.contains("401 Unauthorized"), "{err}");
    assert!(err.contains("invalid api key"), "{err}");
    assert!(!err.contains("secret"), "{err}");
    shutdown.cancel();
}
```

- [ ] **Step 7: Run new tests and verify RED or compile issues**

Run:

```bash
cargo test --test embedding_client
```

Expected: tests compile after adding `use http;` if needed and fail only if implementation does not yet handle the covered cases.

- [ ] **Step 8: Adjust tests imports if required**

At the top of `tests/embedding_client.rs`, make imports explicit:

```rust
use axum::{Json, Router, routing::post};
use http::StatusCode;
use serde_json::{Value, json};
use sqlite_mcp_rs::{
    config::EmbeddingRuntimeConfig,
    embedding::{EMBEDDINGS_PATH, EmbeddingClient},
};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;
```

Use `StatusCode::OK` and `StatusCode::UNAUTHORIZED` in the test handlers.

- [ ] **Step 9: Run embedding client tests and verify GREEN**

Run:

```bash
cargo test --test embedding_client
```

Expected: all embedding client tests pass.

- [ ] **Step 10: Commit embedding client**

Run:

```bash
git add Cargo.toml Cargo.lock src/embedding.rs src/lib.rs tests/embedding_client.rs
git commit -m "feat: add OpenAI compatible embedding client"
```

Expected: commit succeeds.

---

### Task 3: Refactor Vector Storage to Text Collection Operations

**Files:**
- Modify: `src/vector.rs`
- Modify: `src/sqlite.rs`
- Modify: `tests/vector_collections.rs`

- [ ] **Step 1: Write failing storage test for generated text upsert/search**

In `tests/vector_collections.rs`, replace imports of manual vector input types with text storage types:

```rust
use sqlite_mcp_rs::vector::{
    CreateTextCollectionStorageInput, DeleteTextsInput, DropTextCollectionInput,
    GeneratedTextItemInput, SearchGeneratedTextInput, UpsertGeneratedTextsInput,
};
```

Add this test near the existing upsert/search tests:

```rust
#[tokio::test]
async fn generated_text_vectors_store_text_and_search_without_vectors() {
    let (_dir, path) = temp_db_path("generated_text_vectors.db");
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
                text: "alpha".to_string(),
                metadata: Some(json!({"tenant": "a"})),
            }],
        })
        .await;
    assert!(upsert.success, "{upsert:?}");
    assert_eq!(upsert.data["upserted_count"], json!(1));

    let search = exec
        .search_generated_text(SearchGeneratedTextInput {
            collection: "docs".to_string(),
            vector: vec![1.0, 0.0],
            top_k: 1,
            filter: None,
        })
        .await;
    assert!(search.success, "{search:?}");
    let results = search.data["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["id"], json!("doc-a"));
    assert_eq!(results[0]["text"], json!("alpha"));
    assert_eq!(results[0]["metadata"], json!({"tenant": "a"}));
    assert!(results[0].get("vector").is_none());
}
```

- [ ] **Step 2: Run storage test and verify RED**

Run:

```bash
cargo test --test vector_collections generated_text_vectors_store_text_and_search_without_vectors
```

Expected: compile failure because the new text storage types and executor methods do not exist.

- [ ] **Step 3: Replace public vector types with text storage types**

In `src/vector.rs`, replace the manual public input structs and `VectorOperation` variants with:

```rust
#[derive(Clone, Debug)]
pub enum VectorOperation {
    DescribeCollection(DescribeTextCollectionInput),
    CreateCollection(CreateTextCollectionStorageInput),
    UpsertGeneratedTexts(UpsertGeneratedTextsInput),
    SearchGeneratedText(SearchGeneratedTextInput),
    DeleteTexts(DeleteTextsInput),
    DropTextCollection(DropTextCollectionInput),
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize, PartialEq)]
pub struct CreateTextCollectionInput {
    pub collection: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize, PartialEq)]
pub struct TextItemInput {
    pub id: String,
    pub text: String,
    pub metadata: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize, PartialEq)]
pub struct UpsertTextsInput {
    pub collection: String,
    pub items: Vec<TextItemInput>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize, PartialEq)]
pub struct SearchTextInput {
    pub collection: String,
    pub query: String,
    pub top_k: usize,
    pub filter: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize, PartialEq)]
pub struct DeleteTextsInput {
    pub collection: String,
    pub ids: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize, PartialEq)]
pub struct DropTextCollectionInput {
    pub collection: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize, PartialEq)]
pub struct DescribeTextCollectionInput {
    pub collection: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize, PartialEq)]
pub struct CreateTextCollectionStorageInput {
    pub collection: String,
    pub dimension: usize,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize, PartialEq)]
pub struct UpsertGeneratedTextsInput {
    pub collection: String,
    pub items: Vec<GeneratedTextItemInput>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize, PartialEq)]
pub struct GeneratedTextItemInput {
    pub id: String,
    pub vector: Vec<f64>,
    pub text: String,
    pub metadata: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize, PartialEq)]
pub struct SearchGeneratedTextInput {
    pub collection: String,
    pub vector: Vec<f64>,
    pub top_k: usize,
    pub filter: Option<Value>,
}
```

- [ ] **Step 4: Update operation dispatch**

In `src/vector.rs`, update `execute_vector_operation`:

```rust
pub fn execute_vector_operation(
    conn: &Connection,
    mode: RunMode,
    max_top_k: usize,
    operation: VectorOperation,
) -> Result<Map<String, Value>, String> {
    match operation {
        VectorOperation::DescribeCollection(input) => describe_collection(conn, input),
        VectorOperation::CreateCollection(input) => create_collection(conn, mode, input),
        VectorOperation::UpsertGeneratedTexts(input) => upsert_generated_texts(conn, mode, input),
        VectorOperation::SearchGeneratedText(input) => search_generated_text(conn, max_top_k, input),
        VectorOperation::DeleteTexts(input) => delete_texts(conn, mode, input),
        VectorOperation::DropTextCollection(input) => drop_text_collection(conn, mode, input),
    }
}
```

- [ ] **Step 5: Rename storage functions and validation messages**

In `src/vector.rs`, rename functions and update signatures:

```rust
fn create_collection(
    conn: &Connection,
    mode: RunMode,
    input: CreateTextCollectionStorageInput,
) -> Result<Map<String, Value>, String> {
    if mode == RunMode::Readonly {
        return Err("readonly mode forbids create_text_collection".to_string());
    }
    // keep the existing collection validation, registry creation, and vec0 creation body
}

fn upsert_generated_texts(
    conn: &Connection,
    mode: RunMode,
    input: UpsertGeneratedTextsInput,
) -> Result<Map<String, Value>, String> {
    if mode == RunMode::Readonly {
        return Err("readonly mode forbids upsert_texts".to_string());
    }
    // keep the existing upsert body, but read item.text as a required String
}

fn search_generated_text(
    conn: &Connection,
    max_top_k: usize,
    input: SearchGeneratedTextInput,
) -> Result<Map<String, Value>, String> {
    // keep the existing search body using input.vector
}

fn delete_texts(
    conn: &Connection,
    mode: RunMode,
    input: DeleteTextsInput,
) -> Result<Map<String, Value>, String> {
    if mode == RunMode::Readonly {
        return Err("readonly mode forbids delete_texts".to_string());
    }
    // keep the existing delete body
}

fn drop_text_collection(
    conn: &Connection,
    mode: RunMode,
    input: DropTextCollectionInput,
) -> Result<Map<String, Value>, String> {
    if mode == RunMode::Readonly {
        return Err("readonly mode forbids drop_text_collection".to_string());
    }
    // keep the existing drop body
}
```

The copied bodies must use `item.text` instead of `item.text: Option<String>`, store text through `params![item.id, vector_json, item.text, metadata_json]`, and keep metadata defaulting to `{}`.

- [ ] **Step 6: Add collection description operation**

Add this function in `src/vector.rs`:

```rust
fn describe_collection(
    conn: &Connection,
    input: DescribeTextCollectionInput,
) -> Result<Map<String, Value>, String> {
    let collection = validate_collection_name(&input.collection)?;
    let existing = find_collection(conn, collection)?
        .ok_or_else(|| format!("collection not found: {collection}"))?;

    Ok(Map::from_iter([
        ("collection".to_string(), json!(collection)),
        ("table_name".to_string(), json!(existing.table_name)),
        ("dimension".to_string(), json!(existing.dimension)),
        ("distance_metric".to_string(), json!(existing.distance_metric)),
    ]))
}
```

- [ ] **Step 7: Update SqliteExecutor vector methods**

In `src/sqlite.rs`, update imports and methods:

```rust
use crate::vector::{
    CreateTextCollectionStorageInput, DeleteTextsInput, DescribeTextCollectionInput,
    DropTextCollectionInput, SearchGeneratedTextInput, UpsertGeneratedTextsInput, VectorOperation,
    VectorToolResponse, execute_vector_operation,
};
```

Add `mode` to `SqliteExecutor`:

```rust
#[derive(Clone)]
pub struct SqliteExecutor {
    tx: mpsc::Sender<WorkerJob>,
    mode: RunMode,
}
```

Return it from `open`:

```rust
Ok(Self {
    tx,
    mode: config.mode,
})
```

Add methods:

```rust
pub fn mode(&self) -> RunMode {
    self.mode
}

pub async fn describe_text_collection(
    &self,
    input: DescribeTextCollectionInput,
) -> VectorToolResponse {
    self.execute_vector(VectorOperation::DescribeCollection(input))
        .await
}

pub async fn create_text_collection_with_dimension(
    &self,
    input: CreateTextCollectionStorageInput,
) -> VectorToolResponse {
    self.execute_vector(VectorOperation::CreateCollection(input))
        .await
}

pub async fn upsert_generated_texts(&self, input: UpsertGeneratedTextsInput) -> VectorToolResponse {
    self.execute_vector(VectorOperation::UpsertGeneratedTexts(input))
        .await
}

pub async fn search_generated_text(&self, input: SearchGeneratedTextInput) -> VectorToolResponse {
    self.execute_vector(VectorOperation::SearchGeneratedText(input))
        .await
}

pub async fn delete_texts(&self, input: DeleteTextsInput) -> VectorToolResponse {
    self.execute_vector(VectorOperation::DeleteTexts(input)).await
}

pub async fn drop_text_collection(&self, input: DropTextCollectionInput) -> VectorToolResponse {
    self.execute_vector(VectorOperation::DropTextCollection(input))
        .await
}
```

Remove the old `create_vector_collection`, `upsert_vectors`, `search_vectors`, `delete_vectors`, and `drop_vector_collection` public methods.

- [ ] **Step 8: Run new storage test and verify GREEN**

Run:

```bash
cargo test --test vector_collections generated_text_vectors_store_text_and_search_without_vectors
```

Expected: the new test passes.

- [ ] **Step 9: Convert remaining vector collection tests**

Update the rest of `tests/vector_collections.rs` mechanically:

```rust
CreateVectorCollectionInput -> CreateTextCollectionStorageInput
UpsertVectorsInput -> UpsertGeneratedTextsInput
VectorItemInput -> GeneratedTextItemInput
SearchVectorsInput -> SearchGeneratedTextInput
DeleteVectorsInput -> DeleteTextsInput
DropVectorCollectionInput -> DropTextCollectionInput
create_vector_collection -> create_text_collection_with_dimension
upsert_vectors -> upsert_generated_texts
search_vectors -> search_generated_text
delete_vectors -> delete_texts
drop_vector_collection -> drop_text_collection
```

For every `GeneratedTextItemInput`, change `text: Some("alpha".to_string())` to `text: "alpha".to_string()`. Where old tests used `text: None`, use `text: "alpha".to_string()` or another non-empty string that matches the assertion.

- [ ] **Step 10: Update validation assertions**

In `tests/vector_collections.rs`, update readonly error assertions:

```rust
assert!(vector_error_message(&create).contains("create_text_collection"));
assert!(vector_error_message(&upsert).contains("upsert_texts"));
assert!(vector_error_message(&delete).contains("delete_texts"));
assert!(vector_error_message(&drop).contains("drop_text_collection"));
```

Keep dimension mismatch, non-finite vector, metadata, filter, and `max_top_k` assertions unchanged because storage still validates generated vectors internally.

- [ ] **Step 11: Run storage tests and verify GREEN**

Run:

```bash
cargo test --test vector_collections
```

Expected: all vector collection storage tests pass.

- [ ] **Step 12: Commit storage refactor**

Run:

```bash
git add src/vector.rs src/sqlite.rs tests/vector_collections.rs
git commit -m "refactor: make vector storage text collection oriented"
```

Expected: commit succeeds.

---

### Task 4: Replace MCP Vector Tools With Text Tools

**Files:**
- Modify: `src/mcp.rs`
- Modify: `src/main.rs`
- Modify: `tests/mcp_http.rs`

- [ ] **Step 1: Write failing tools/list test**

In `tests/mcp_http.rs`, update the expected tool names:

```rust
assert_eq!(
    tool_names,
    vec![
        "create_text_collection",
        "delete_texts",
        "drop_text_collection",
        "execute_sql",
        "search_text",
        "upsert_texts",
    ]
);
```

- [ ] **Step 2: Run MCP test and verify RED**

Run:

```bash
cargo test --test mcp_http mcp_lists_execute_sql_and_vector_tools
```

Expected: test fails because the server still lists old vector tool names or because router signatures have not been updated.

- [ ] **Step 3: Inject embedding client into MCP server**

Modify `src/mcp.rs` imports:

```rust
use std::{net::SocketAddr, time::Instant};

use crate::{
    auth::{AuthState, require_auth},
    embedding::EmbeddingClient,
    error::AppError,
    sqlite::SqliteExecutor,
    vector::{
        CreateTextCollectionInput, CreateTextCollectionStorageInput, DeleteTextsInput,
        DescribeTextCollectionInput, DropTextCollectionInput, GeneratedTextItemInput,
        SearchGeneratedTextInput, SearchTextInput, TextItemInput, UpsertGeneratedTextsInput,
        UpsertTextsInput, VectorToolResponse,
    },
};
```

Update the server struct and constructor:

```rust
#[derive(Clone)]
pub struct SqliteMcpServer {
    executor: SqliteExecutor,
    embeddings: Option<EmbeddingClient>,
    tool_router: ToolRouter<Self>,
}

impl SqliteMcpServer {
    pub fn new(executor: SqliteExecutor, embeddings: Option<EmbeddingClient>) -> Self {
        Self {
            executor,
            embeddings,
            tool_router: Self::tool_router(),
        }
    }
}
```

- [ ] **Step 4: Add MCP helper methods**

Add these helpers in `src/mcp.rs`:

```rust
const EMBEDDING_DIMENSION_PROBE: &str = "sqlite-mcp-rs embedding dimension probe";

impl SqliteMcpServer {
    async fn embed(&self, input: &[String]) -> Result<Vec<Vec<f64>>, String> {
        let client = self
            .embeddings
            .as_ref()
            .ok_or_else(|| "embedding is not configured; set --embedding-model".to_string())?;
        client.embed(input).await
    }
}

fn timed_vector_result(start: Instant, mut response: VectorToolResponse) -> CallToolResult {
    response.elapsed_ms = start.elapsed().as_millis();
    vector_result(response)
}

fn vector_failure(start: Instant, message: impl Into<String>) -> CallToolResult {
    vector_result(VectorToolResponse::failure(
        message,
        start.elapsed().as_millis(),
    ))
}

fn first_embedding(mut embeddings: Vec<Vec<f64>>) -> Result<Vec<f64>, String> {
    if embeddings.len() != 1 {
        return Err(format!(
            "embedding response count mismatch: expected 1, got {}",
            embeddings.len()
        ));
    }
    Ok(embeddings.remove(0))
}

fn validate_text_items(items: &[TextItemInput]) -> Result<(), String> {
    if items.is_empty() {
        return Err("items must not be empty".to_string());
    }
    for item in items {
        if item.id.is_empty() {
            return Err("text id must not be empty".to_string());
        }
        if item.text.trim().is_empty() {
            return Err("text must not be empty".to_string());
        }
        if let Some(metadata) = &item.metadata {
            if !metadata.is_object() {
                return Err("metadata must be a JSON object".to_string());
            }
        }
    }
    Ok(())
}

fn dimension_from_response(response: &VectorToolResponse) -> Result<usize, String> {
    response
        .data
        .get("dimension")
        .and_then(|value| value.as_u64())
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| "collection response did not include a valid dimension".to_string())
}

fn validate_embedding_dimension(vector: &[f64], expected_dimension: usize) -> Result<(), String> {
    if vector.len() != expected_dimension {
        return Err(format!(
            "embedding dimension mismatch: expected {}, got {}",
            expected_dimension,
            vector.len()
        ));
    }
    if vector.iter().any(|value| !value.is_finite()) {
        return Err("embedding contains a non-finite value".to_string());
    }
    Ok(())
}
```

- [ ] **Step 5: Replace MCP tool functions**

In the `#[tool_router] impl SqliteMcpServer` block, remove old vector tools and add:

```rust
#[tool(
    name = "create_text_collection",
    description = "Create a text embedding collection"
)]
async fn create_text_collection(
    &self,
    Parameters(input): Parameters<CreateTextCollectionInput>,
) -> CallToolResult {
    let start = Instant::now();
    if self.executor.mode() == crate::config::RunMode::Readonly {
        return vector_failure(start, "readonly mode forbids create_text_collection");
    }
    let probe = vec![EMBEDDING_DIMENSION_PROBE.to_string()];
    let embedding = match self.embed(&probe).await.and_then(first_embedding) {
        Ok(embedding) => embedding,
        Err(message) => return vector_failure(start, message),
    };
    if let Err(message) = validate_embedding_dimension(&embedding, embedding.len()) {
        return vector_failure(start, message);
    }

    let response = self
        .executor
        .create_text_collection_with_dimension(CreateTextCollectionStorageInput {
            collection: input.collection,
            dimension: embedding.len(),
        })
        .await;
    timed_vector_result(start, response)
}

#[tool(name = "upsert_texts", description = "Insert or replace texts in a collection")]
async fn upsert_texts(&self, Parameters(input): Parameters<UpsertTextsInput>) -> CallToolResult {
    let start = Instant::now();
    if self.executor.mode() == crate::config::RunMode::Readonly {
        return vector_failure(start, "readonly mode forbids upsert_texts");
    }
    if let Err(message) = validate_text_items(&input.items) {
        return vector_failure(start, message);
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

    let texts = input
        .items
        .iter()
        .map(|item| item.text.clone())
        .collect::<Vec<_>>();
    let embeddings = match self.embed(&texts).await {
        Ok(embeddings) => embeddings,
        Err(message) => return vector_failure(start, message),
    };
    if embeddings.len() != input.items.len() {
        return vector_failure(
            start,
            format!(
                "embedding response count mismatch: expected {}, got {}",
                input.items.len(),
                embeddings.len()
            ),
        );
    }

    let mut generated_items = Vec::with_capacity(input.items.len());
    for (item, vector) in input.items.into_iter().zip(embeddings) {
        if let Err(message) = validate_embedding_dimension(&vector, dimension) {
            return vector_failure(start, message);
        }
        generated_items.push(GeneratedTextItemInput {
            id: item.id,
            vector,
            text: item.text,
            metadata: item.metadata,
        });
    }

    let response = self
        .executor
        .upsert_generated_texts(UpsertGeneratedTextsInput {
            collection: input.collection,
            items: generated_items,
        })
        .await;
    timed_vector_result(start, response)
}

#[tool(name = "search_text", description = "Search a text embedding collection")]
async fn search_text(&self, Parameters(input): Parameters<SearchTextInput>) -> CallToolResult {
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
        .search_generated_text(SearchGeneratedTextInput {
            collection: input.collection,
            vector: embedding,
            top_k: input.top_k,
            filter: input.filter,
        })
        .await;
    timed_vector_result(start, response)
}

#[tool(name = "delete_texts", description = "Delete texts from a collection by id")]
async fn delete_texts(&self, Parameters(input): Parameters<DeleteTextsInput>) -> CallToolResult {
    vector_result(self.executor.delete_texts(input).await)
}

#[tool(
    name = "drop_text_collection",
    description = "Drop a text embedding collection and remove its registry row"
)]
async fn drop_text_collection(
    &self,
    Parameters(input): Parameters<DropTextCollectionInput>,
) -> CallToolResult {
    vector_result(self.executor.drop_text_collection(input).await)
}
```

- [ ] **Step 6: Update router constructors**

Change router signatures in `src/mcp.rs`:

```rust
pub fn router(
    executor: SqliteExecutor,
    auth_token: Option<String>,
    embeddings: Option<EmbeddingClient>,
) -> Result<Router, AppError> {
    router_with_cancellation(executor, auth_token, embeddings, CancellationToken::new())
}

fn router_with_cancellation(
    executor: SqliteExecutor,
    auth_token: Option<String>,
    embeddings: Option<EmbeddingClient>,
    cancellation_token: CancellationToken,
) -> Result<Router, AppError> {
    let service: StreamableHttpService<SqliteMcpServer, LocalSessionManager> =
        StreamableHttpService::new(
            move || Ok(SqliteMcpServer::new(executor.clone(), embeddings.clone())),
            Default::default(),
            StreamableHttpServerConfig::default()
                .with_stateful_mode(false)
                .with_json_response(true)
                .with_sse_keep_alive(None)
                .disable_allowed_hosts()
                .with_cancellation_token(cancellation_token),
        );
    // keep the existing Router body
}

pub async fn spawn_test_server(
    executor: SqliteExecutor,
    auth_token: Option<String>,
    embeddings: Option<EmbeddingClient>,
) -> Result<TestServer, AppError> {
    let shutdown = CancellationToken::new();
    let app =
        router_with_cancellation(executor, auth_token, embeddings, shutdown.child_token())?;
    // keep the existing listener body
}
```

- [ ] **Step 7: Update main to build embedding client**

Modify `src/main.rs` imports:

```rust
use sqlite_mcp_rs::{
    config::{Cli, RuntimeConfig},
    embedding::EmbeddingClient,
    error::AppError,
    sqlite::{ExecutorConfig, SqliteExecutor},
};
```

Before calling `router`, build the optional client:

```rust
let embeddings = match EmbeddingClient::from_runtime_config(&runtime.embedding) {
    Some(Ok(client)) => Some(client),
    Some(Err(message)) => return Err(AppError::Config(message)),
    None => None,
};

let app = sqlite_mcp_rs::mcp::router(executor, runtime.auth_token, embeddings)?;
```

- [ ] **Step 8: Update MCP test setup with mock embedding server**

In `tests/mcp_http.rs`, add imports:

```rust
use axum::{Json, Router, routing::post};
use sqlite_mcp_rs::config::EmbeddingRuntimeConfig;
use sqlite_mcp_rs::embedding::{EMBEDDINGS_PATH, EmbeddingClient};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;
```

Add a reusable test server:

```rust
struct TestEmbeddingServer {
    base_url: String,
    requests: Arc<Mutex<Vec<Value>>>,
    shutdown: CancellationToken,
}

impl TestEmbeddingServer {
    fn client(&self) -> EmbeddingClient {
        EmbeddingClient::new(EmbeddingRuntimeConfig {
            base_url: self.base_url.clone(),
            api_key: None,
            model: Some("test-embedding".to_string()),
            dimensions: Some(2),
            timeout_ms: 5_000,
        })
        .unwrap()
    }
}

impl Drop for TestEmbeddingServer {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

async fn spawn_test_embedding_server() -> TestEmbeddingServer {
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let shutdown = CancellationToken::new();
    let app = Router::new().route(
        EMBEDDINGS_PATH,
        post({
            let requests = requests.clone();
            move |Json(body): Json<Value>| {
                let requests = requests.clone();
                async move {
                    requests.lock().unwrap().push(body.clone());
                    let input = body["input"].as_array().unwrap();
                    let data = input
                        .iter()
                        .enumerate()
                        .map(|(index, value)| {
                            let text = value.as_str().unwrap();
                            let embedding = if text.contains("beta") {
                                json!([0.0, 1.0])
                            } else {
                                json!([1.0, 0.0])
                            };
                            json!({"index": index, "embedding": embedding})
                        })
                        .collect::<Vec<_>>();
                    Json(json!({"data": data}))
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
        requests,
        shutdown,
    }
}
```

In `mcp_lists_execute_sql_and_vector_tools`, create the embedding server before spawning MCP:

```rust
let embedding_server = spawn_test_embedding_server().await;
let server = spawn_test_server(executor, None, Some(embedding_server.client()))
    .await
    .unwrap();
```

- [ ] **Step 9: Update MCP tool calls to text-first arguments**

In `tests/mcp_http.rs`, replace create/upsert/search/delete/drop calls with:

```rust
let create = call_tool(
    &client,
    &server.url(),
    4,
    "create_text_collection",
    json!({"collection": "docs"}),
)
.await;
assert_eq!(create["success"], true);
assert_eq!(create["collection"], "docs");
assert_eq!(create["dimension"], 2);
assert_eq!(create["created"], true);

let upsert = call_tool(
    &client,
    &server.url(),
    5,
    "upsert_texts",
    json!({
        "collection": "docs",
        "items": [
            {
                "id": "doc-a",
                "text": "alpha",
                "metadata": {"tenant": "a"}
            }
        ]
    }),
)
.await;
assert_eq!(upsert["success"], true);
assert_eq!(upsert["upserted_count"], 1);

let search = call_tool(
    &client,
    &server.url(),
    6,
    "search_text",
    json!({
        "collection": "docs",
        "query": "alpha query",
        "top_k": 1,
        "filter": {"tenant": "a"}
    }),
)
.await;
assert_eq!(search["success"], true);
assert_eq!(search["results"][0]["id"], "doc-a");
assert!(search["results"][0].get("vector").is_none());

let deleted = call_tool(
    &client,
    &server.url(),
    7,
    "delete_texts",
    json!({"collection": "docs", "ids": ["doc-a", "missing"]}),
)
.await;
assert_eq!(deleted["success"], true);
assert_eq!(deleted["requested_count"], 2);
assert_eq!(deleted["deleted_count"], 1);

let dropped = call_tool(
    &client,
    &server.url(),
    8,
    "drop_text_collection",
    json!({"collection": "docs"}),
)
.await;
assert_eq!(dropped["success"], true);
assert_eq!(dropped["existed"], true);
```

- [ ] **Step 10: Run MCP test and verify GREEN**

Run:

```bash
cargo test --test mcp_http
```

Expected: MCP HTTP tests pass.

- [ ] **Step 11: Commit MCP text tools**

Run:

```bash
git add src/mcp.rs src/main.rs tests/mcp_http.rs
git commit -m "feat: expose text embedding MCP tools"
```

Expected: commit succeeds.

---

### Task 5: Add MCP Error Coverage for Missing and Bad Embeddings

**Files:**
- Modify: `tests/mcp_http.rs`
- Modify: `src/mcp.rs`

- [ ] **Step 1: Write missing embedding config test**

Add this test to `tests/mcp_http.rs`:

```rust
#[tokio::test]
async fn text_tools_report_missing_embedding_configuration() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("mcp_missing_embedding.db");
    let executor = SqliteExecutor::open(ExecutorConfig {
        db_path,
        mode: RunMode::Readwrite,
        max_rows: 500,
        max_top_k: 100,
        timeout_ms: 10_000,
    })
    .unwrap();

    let server = spawn_test_server(executor, None, None).await.unwrap();
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

    assert_eq!(create["success"], false);
    assert!(
        create["error"]["message"]
            .as_str()
            .unwrap()
            .contains("embedding is not configured")
    );
}
```

Extract initialization into a helper:

```rust
async fn initialize(client: &Client, url: &str) {
    let initialize: Value = client
        .post(url)
        .header("accept", "application/json, text/event-stream")
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
}
```

Replace the inline initialize block in the existing test with `initialize(&client, &server.url()).await;`.

- [ ] **Step 2: Run missing config test and verify GREEN**

Run:

```bash
cargo test --test mcp_http text_tools_report_missing_embedding_configuration
```

Expected: test passes if Task 4 helper behavior is complete. If it fails, update `vector_failure` and `SqliteMcpServer::embed` so missing client returns `embedding is not configured; set --embedding-model`.

- [ ] **Step 3: Write wrong embedding dimension test**

Add this test to `tests/mcp_http.rs`:

```rust
#[tokio::test]
async fn upsert_texts_rejects_embedding_dimension_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("mcp_dimension_mismatch.db");
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
        json!({"data": [{"index": 0, "embedding": [1.0, 0.0, 0.0]}]}),
    ])
    .await;

    let server = spawn_test_server(executor, None, Some(embedding_server.client()))
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
            "items": [{"id": "doc-a", "text": "alpha"}]
        }),
    )
    .await;
    assert_eq!(upsert["success"], false);
    assert!(
        upsert["error"]["message"]
            .as_str()
            .unwrap()
            .contains("embedding dimension mismatch")
    );
}
```

Add the sequence server helper:

```rust
async fn spawn_sequence_embedding_server(responses: Vec<Value>) -> TestEmbeddingServer {
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let responses = Arc::new(Mutex::new(responses));
    let shutdown = CancellationToken::new();
    let app = Router::new().route(
        EMBEDDINGS_PATH,
        post({
            let requests = requests.clone();
            let responses = responses.clone();
            move |Json(body): Json<Value>| {
                let requests = requests.clone();
                let responses = responses.clone();
                async move {
                    requests.lock().unwrap().push(body);
                    let mut responses = responses.lock().unwrap();
                    Json(responses.remove(0))
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
        requests,
        shutdown,
    }
}
```

- [ ] **Step 4: Run dimension mismatch test and verify GREEN**

Run:

```bash
cargo test --test mcp_http upsert_texts_rejects_embedding_dimension_mismatch
```

Expected: test passes. If it fails, make `upsert_texts` call `validate_embedding_dimension` for each generated vector before sending the operation to SQLite.

- [ ] **Step 5: Write readonly behavior test**

Add this test to `tests/mcp_http.rs`:

```rust
#[tokio::test]
async fn readonly_rejects_text_writes_before_embedding() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("mcp_readonly_text.db");
    {
        let writable = SqliteExecutor::open(ExecutorConfig {
            db_path: db_path.clone(),
            mode: RunMode::Readwrite,
            max_rows: 500,
            max_top_k: 100,
            timeout_ms: 10_000,
        })
        .unwrap();
        let created = writable
            .create_text_collection_with_dimension(
                sqlite_mcp_rs::vector::CreateTextCollectionStorageInput {
                    collection: "docs".to_string(),
                    dimension: 2,
                },
            )
            .await;
        assert!(created.success, "{created:?}");
    }

    let readonly = SqliteExecutor::open(ExecutorConfig {
        db_path,
        mode: RunMode::Readonly,
        max_rows: 500,
        max_top_k: 100,
        timeout_ms: 10_000,
    })
    .unwrap();
    let embedding_server = spawn_test_embedding_server().await;
    let server = spawn_test_server(readonly, None, Some(embedding_server.client()))
        .await
        .unwrap();
    let client = Client::new();
    initialize(&client, &server.url()).await;

    let upsert = call_tool(
        &client,
        &server.url(),
        2,
        "upsert_texts",
        json!({
            "collection": "docs",
            "items": [{"id": "doc-a", "text": "alpha"}]
        }),
    )
    .await;
    assert_eq!(upsert["success"], false);
    assert!(upsert["error"]["message"].as_str().unwrap().contains("readonly"));
    assert_eq!(embedding_server.requests.lock().unwrap().len(), 0);
}
```

- [ ] **Step 6: Run readonly test and verify GREEN**

Run:

```bash
cargo test --test mcp_http readonly_rejects_text_writes_before_embedding
```

Expected: test passes. If it fails, ensure `create_text_collection` and `upsert_texts` check `self.executor.mode()` before calling `self.embed`.

- [ ] **Step 7: Run MCP HTTP tests**

Run:

```bash
cargo test --test mcp_http
```

Expected: all MCP HTTP tests pass.

- [ ] **Step 8: Commit MCP error tests**

Run:

```bash
git add src/mcp.rs tests/mcp_http.rs
git commit -m "test: cover text embedding MCP errors"
```

Expected: commit succeeds.

---

### Task 6: Update Documentation

**Files:**
- Modify: `README.md`
- Modify: `README_ZH.md`

- [ ] **Step 1: Update English README tool list**

In `README.md`, replace vector tool names with:

```text
execute_sql
create_text_collection
upsert_texts
search_text
delete_texts
drop_text_collection
```

Update the tool overview rows:

```markdown
| `create_text_collection` | Create a named sqlite-vec collection using the configured embedding model dimension. | `readwrite` only. |
| `upsert_texts` | Embed text items internally and insert or replace them in a collection. | `readwrite` only. |
| `search_text` | Embed a query internally and search by cosine distance. | `readonly` and `readwrite`. |
| `delete_texts` | Delete text records by id. | `readwrite` only. |
| `drop_text_collection` | Drop a text embedding collection and its registry row. | `readwrite` only. |
```

- [ ] **Step 2: Update English README CLI options**

Add these rows to the options table in `README.md`:

```markdown
| `--embedding-base-url <url>` | `https://api.openai.com/v1` | OpenAI-compatible API base URL. |
| `--embedding-api-key <key>` | `OPENAI_API_KEY` env var | Bearer token for the embedding API. |
| `--embedding-model <model>` | None | Enables text embedding tools with this model. |
| `--embedding-dimensions <n>` | None | Optional OpenAI-compatible embedding dimensions override. |
| `--embedding-timeout-ms <n>` | `30000` | HTTP timeout for embedding requests. |
```

- [ ] **Step 3: Add English text-first vector usage example**

Add this section to `README.md` after the `execute_sql` tool section or where vector usage currently appears:

```markdown
## Text Embedding Collections

Text collection tools call the configured embedding API internally. MCP clients
send text, ids, and metadata; they do not send or receive embedding arrays.

Start with embedding enabled:

```bash
export OPENAI_API_KEY='sk-...'

sqlite-mcp-rs \
  --db ./app.db \
  --embedding-model text-embedding-3-small
```

Create a collection:

```json
{
  "collection": "docs"
}
```

Upsert text:

```json
{
  "collection": "docs",
  "items": [
    {
      "id": "doc-1",
      "text": "SQLite is an embedded relational database.",
      "metadata": {"source": "manual", "tenant": "a"}
    }
  ]
}
```

Search:

```json
{
  "collection": "docs",
  "query": "embedded database",
  "top_k": 5,
  "filter": {"tenant": "a"}
}
```

Embedding model tokens are still used to create embeddings. The token saving is
that chat models no longer need to move thousands of floating point numbers
through MCP tool calls.
```
```

- [ ] **Step 4: Update Chinese README equivalently**

In `README_ZH.md`, replace vector tool names with:

```text
execute_sql
create_text_collection
upsert_texts
search_text
delete_texts
drop_text_collection
```

Use these Chinese descriptions in the overview table:

```markdown
| `create_text_collection` | 使用配置的 embedding 模型维度创建命名 sqlite-vec 集合。 | 仅 `readwrite`。 |
| `upsert_texts` | 工具内部生成文本 embedding，并插入或替换集合记录。 | 仅 `readwrite`。 |
| `search_text` | 工具内部生成查询 embedding，并按余弦距离搜索集合。 | `readonly` 和 `readwrite`。 |
| `delete_texts` | 按 id 删除文本记录。 | 仅 `readwrite`。 |
| `drop_text_collection` | 删除文本 embedding 集合及注册表元数据。 | 仅 `readwrite`。 |
```

Add CLI option rows:

```markdown
| `--embedding-base-url <url>` | `https://api.openai.com/v1` | OpenAI-compatible API 基础 URL。 |
| `--embedding-api-key <key>` | `OPENAI_API_KEY` 环境变量 | embedding API Bearer token。 |
| `--embedding-model <model>` | 无 | 使用该模型启用文本 embedding 工具。 |
| `--embedding-dimensions <n>` | 无 | 可选的 OpenAI-compatible embedding 维度覆盖。 |
| `--embedding-timeout-ms <n>` | `30000` | embedding HTTP 请求超时时间。 |
```

Add a short Chinese explanation:

```markdown
文本集合工具会在服务内部调用配置的 embedding API。MCP 客户端只传文本、id 和 metadata，不传也不接收向量数组。

生成 embedding 仍会消耗 embedding 模型 token。节省的是聊天模型不再需要通过 MCP tool call 搬运几千个浮点数。
```

- [ ] **Step 5: Run documentation search**

Run:

```bash
rg -n "create_vector_collection|upsert_vectors|search_vectors|delete_vectors|drop_vector_collection|client-provided embeddings|客户端提供的向量" README.md README_ZH.md
```

Expected: no matches in the README files except historical context that explicitly says old manual-vector tools were removed. Prefer no matches.

- [ ] **Step 6: Commit docs**

Run:

```bash
git add README.md README_ZH.md
git commit -m "docs: document text embedding tools"
```

Expected: commit succeeds.

---

### Task 7: Full Verification and Cleanup

**Files:**
- Potentially modify any file touched in previous tasks only to fix verification failures.

- [ ] **Step 1: Run formatting**

Run:

```bash
cargo fmt
```

Expected: command succeeds and only Rust formatting changes appear.

- [ ] **Step 2: Run all tests**

Run:

```bash
cargo test
```

Expected: all tests pass.

- [ ] **Step 3: Run clippy**

Run:

```bash
cargo clippy --all-targets --all-features -- -D warnings
```

Expected: clippy exits successfully with no warnings.

- [ ] **Step 4: Inspect public tool names**

Run:

```bash
rg -n "create_vector_collection|upsert_vectors|search_vectors|delete_vectors|drop_vector_collection" src tests README.md README_ZH.md
```

Expected: no matches in active source, tests, or README files.

- [ ] **Step 5: Inspect generated-vector exposure**

Run:

```bash
rg -n "\"vector\"|vector:" src/mcp.rs README.md README_ZH.md tests/mcp_http.rs
```

Expected: no text-tool MCP schema or README example asks clients to send a vector. Matches in internal storage tests are acceptable only outside `src/mcp.rs`, `README.md`, and `README_ZH.md`.

- [ ] **Step 6: Commit verification fixes if needed**

If formatting or verification changed files, run:

```bash
git add .
git commit -m "chore: finalize text embedding collection migration"
```

Expected: commit succeeds when there are verification fixes. If `git status --short` is empty, skip this commit.

---

## Self-Review

Spec coverage:

- Embedding CLI settings are covered in Task 1.
- OpenAI-compatible HTTP client and response validation are covered in Task 2.
- Text-only MCP tool names and old tool removal are covered in Task 4 and Task 7.
- sqlite-vec storage, registry, dimension validation, metadata, filters, and readonly behavior are covered in Tasks 3 through 5.
- Documentation updates are covered in Task 6.
- Full verification is covered in Task 7.

Placeholder scan:

- This plan contains no placeholder markers and no intentionally incomplete implementation steps.

Type consistency:

- Public MCP input types are `CreateTextCollectionInput`, `UpsertTextsInput`, `SearchTextInput`, `DeleteTextsInput`, and `DropTextCollectionInput`.
- Internal generated-vector storage types are `CreateTextCollectionStorageInput`, `UpsertGeneratedTextsInput`, `GeneratedTextItemInput`, and `SearchGeneratedTextInput`.
- Executor methods use text names externally and generated names internally: `create_text_collection_with_dimension`, `upsert_generated_texts`, `search_generated_text`, `delete_texts`, and `drop_text_collection`.
