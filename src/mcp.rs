use std::net::SocketAddr;

use axum::{Router, middleware::from_fn_with_state};
use rmcp::{
    ServerHandler, schemars, tool, tool_handler, tool_router,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Content, ServerCapabilities, ServerInfo},
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
        let text = serde_json::to_string(&response).unwrap_or_else(|error| {
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
) -> Result<Router, AppError> {
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
