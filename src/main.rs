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

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
