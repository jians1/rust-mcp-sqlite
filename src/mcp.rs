use std::net::SocketAddr;

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
    error::AppError,
    sqlite::SqliteExecutor,
    vector::{
        CreateVectorCollectionInput, DeleteVectorsInput, DropVectorCollectionInput,
        SearchVectorsInput, UpsertVectorsInput, VectorToolResponse,
    },
};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ExecuteSqlToolInput {
    sql: String,
}

#[derive(Clone)]
pub struct SqliteMcpServer {
    executor: SqliteExecutor,
    tool_router: ToolRouter<Self>,
}

impl SqliteMcpServer {
    pub fn new(executor: SqliteExecutor) -> Self {
        Self {
            executor,
            tool_router: Self::tool_router(),
        }
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
        name = "create_vector_collection",
        description = "Create a sqlite-vec vector collection"
    )]
    async fn create_vector_collection(
        &self,
        Parameters(input): Parameters<CreateVectorCollectionInput>,
    ) -> CallToolResult {
        vector_result(self.executor.create_vector_collection(input).await)
    }

    #[tool(
        name = "upsert_vectors",
        description = "Insert or replace vectors in a collection"
    )]
    async fn upsert_vectors(
        &self,
        Parameters(input): Parameters<UpsertVectorsInput>,
    ) -> CallToolResult {
        vector_result(self.executor.upsert_vectors(input).await)
    }

    #[tool(
        name = "search_vectors",
        description = "Search a vector collection with cosine distance"
    )]
    async fn search_vectors(
        &self,
        Parameters(input): Parameters<SearchVectorsInput>,
    ) -> CallToolResult {
        vector_result(self.executor.search_vectors(input).await)
    }

    #[tool(
        name = "delete_vectors",
        description = "Delete vectors from a collection by id"
    )]
    async fn delete_vectors(
        &self,
        Parameters(input): Parameters<DeleteVectorsInput>,
    ) -> CallToolResult {
        vector_result(self.executor.delete_vectors(input).await)
    }

    #[tool(
        name = "drop_vector_collection",
        description = "Drop a vector collection and remove its registry row"
    )]
    async fn drop_vector_collection(
        &self,
        Parameters(input): Parameters<DropVectorCollectionInput>,
    ) -> CallToolResult {
        vector_result(self.executor.drop_vector_collection(input).await)
    }
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

pub fn router(executor: SqliteExecutor, auth_token: Option<String>) -> Result<Router, AppError> {
    router_with_cancellation(executor, auth_token, CancellationToken::new())
}

fn router_with_cancellation(
    executor: SqliteExecutor,
    auth_token: Option<String>,
    cancellation_token: CancellationToken,
) -> Result<Router, AppError> {
    let service: StreamableHttpService<SqliteMcpServer, LocalSessionManager> =
        StreamableHttpService::new(
            move || Ok(SqliteMcpServer::new(executor.clone())),
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
) -> Result<TestServer, AppError> {
    let shutdown = CancellationToken::new();
    let app = router_with_cancellation(executor, auth_token, shutdown.child_token())?;
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
