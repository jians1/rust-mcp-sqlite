use std::{net::SocketAddr, time::Instant};

use axum::{Router, middleware::from_fn_with_state};
use rmcp::{
    ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Content, ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use crate::{
    auth::{AuthState, require_auth},
    config::RunMode,
    embedding::EmbeddingClient,
    error::AppError,
    sqlite::SqliteExecutor,
    vector as vector_tools,
};
use vector_tools::{
    CreateTextCollectionInput, CreateTextCollectionStorageInput, DeleteTextsInput,
    DescribeTextCollectionInput, DropTextCollectionInput, GeneratedTextItemInput,
    SearchGeneratedHybridTextInput, SearchGeneratedTextInput, SearchHybridTextInput,
    SearchTextInput, TextItemInput, UpsertGeneratedTextsInput, UpsertTextsInput,
    VectorToolResponse,
};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ExecuteSqlToolInput {
    sql: String,
}

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

const EMBEDDING_DIMENSION_PROBE: &str = "sqlite-mcp-rs embedding dimension probe";

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

#[tool_router]
impl SqliteMcpServer {
    #[tool(name = "execute_sql", description = "Execute SQLite SQL")]
    async fn execute_sql(
        &self,
        Parameters(ExecuteSqlToolInput { sql }): Parameters<ExecuteSqlToolInput>,
    ) -> CallToolResult {
        let response = self.executor.execute(sql).await;
        let text = serialize_tool_response(&response, |error| {
            serde_json::json!({
                "success": false,
                "error": {
                    "message": format!("failed to serialize response: {error}"),
                    "statement_index": 0
                },
                "results": [],
                "elapsed_ms": 0
            })
            .to_string()
        });

        CallToolResult::success(vec![Content::text(text)])
    }

    #[tool(
        name = "create_text_collection",
        description = "Create a text embedding collection"
    )]
    async fn create_text_collection(
        &self,
        Parameters(input): Parameters<CreateTextCollectionInput>,
    ) -> CallToolResult {
        let start = Instant::now();
        if self.executor.mode() == RunMode::Readonly {
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

    #[tool(
        name = "upsert_texts",
        description = "Insert or replace texts in a collection"
    )]
    async fn upsert_texts(
        &self,
        Parameters(input): Parameters<UpsertTextsInput>,
    ) -> CallToolResult {
        let start = Instant::now();
        if self.executor.mode() == RunMode::Readonly {
            return vector_failure(start, "readonly mode forbids upsert_texts");
        }
        let UpsertTextsInput { collection, items } = input;
        if let Err(message) = validate_text_items(&items) {
            return vector_failure(start, message);
        }

        let description = self
            .executor
            .describe_text_collection(DescribeTextCollectionInput {
                collection: collection.clone(),
            })
            .await;
        if !description.success {
            return timed_vector_result(start, description);
        }
        let dimension = match dimension_from_response(&description) {
            Ok(dimension) => dimension,
            Err(message) => return vector_failure(start, message),
        };

        let batch_size = match self.embedding_batch_size() {
            Ok(batch_size) => batch_size,
            Err(message) => return vector_failure(start, message),
        };

        let mut generated_items = Vec::with_capacity(items.len());
        for (batch_index, batch) in items.chunks(batch_size).enumerate() {
            let batch_start_index = batch_index * batch_size;
            let texts = batch
                .iter()
                .map(|item| item.text.clone())
                .collect::<Vec<_>>();
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

        let response = self
            .executor
            .upsert_generated_texts(UpsertGeneratedTextsInput {
                collection,
                items: generated_items,
            })
            .await;
        timed_vector_result(start, response)
    }

    #[tool(
        name = "search_text",
        description = "Search a text embedding collection"
    )]
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

        let vector = embedding;
        let response = self
            .executor
            .search_generated_text(SearchGeneratedTextInput {
                collection: input.collection,
                vector,
                top_k: input.top_k,
                filter: input.filter,
            })
            .await;
        timed_vector_result(start, response)
    }

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

    #[tool(
        name = "delete_texts",
        description = "Delete texts from a collection by id"
    )]
    async fn delete_texts(
        &self,
        Parameters(input): Parameters<DeleteTextsInput>,
    ) -> CallToolResult {
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
        if let Some(metadata) = &item.metadata
            && !metadata.is_object()
        {
            return Err("metadata must be a JSON object".to_string());
        }
    }
    Ok(())
}

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

fn dimension_from_response(response: &VectorToolResponse) -> Result<usize, String> {
    response
        .data
        .get("dimension")
        .and_then(|value| value.as_u64())
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| "collection response did not include a valid dimension".to_string())
}

fn validate_embedding_dimension(
    embedding: &[f64],
    expected_dimension: usize,
) -> Result<(), String> {
    if expected_dimension == 0 {
        return Err("embedding dimension must be positive".to_string());
    }
    if embedding.len() != expected_dimension {
        return Err(format!(
            "embedding dimension mismatch: expected {}, got {}",
            expected_dimension,
            embedding.len()
        ));
    }
    if embedding.iter().any(|value| !value.is_finite()) {
        return Err("embedding contains a non-finite value".to_string());
    }
    Ok(())
}

fn vector_result(response: VectorToolResponse) -> CallToolResult {
    let text = serialize_tool_response(&response, |error| {
        serde_json::json!({
            "success": false,
            "error": {
                "message": format!("failed to serialize response: {error}")
            },
            "elapsed_ms": 0
        })
        .to_string()
    });

    CallToolResult::success(vec![Content::text(text)])
}

fn serialize_tool_response<T>(
    response: &T,
    fallback: impl FnOnce(serde_json::Error) -> String,
) -> String
where
    T: serde::Serialize,
{
    serde_json::to_string(response).unwrap_or_else(fallback)
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for SqliteMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }
}

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

    let app = Router::new()
        .nest_service("/mcp", service)
        .layer(from_fn_with_state(AuthState::new(auth_token), require_auth));

    Ok(app)
}

pub struct TestServer {
    addr: SocketAddr,
    shutdown: CancellationToken,
}

impl TestServer {
    pub fn url(&self) -> String {
        format!("http://{}/mcp", self.addr)
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

pub async fn spawn_test_server(
    executor: SqliteExecutor,
    auth_token: Option<String>,
    embeddings: Option<EmbeddingClient>,
) -> Result<TestServer, AppError> {
    let shutdown = CancellationToken::new();
    let app = router_with_cancellation(executor, auth_token, embeddings, shutdown.child_token())?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let server_shutdown = shutdown.clone();

    tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                server_shutdown.cancelled_owned().await;
            })
            .await;
    });

    Ok(TestServer { addr, shutdown })
}
