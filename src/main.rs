use clap::Parser;
use sqlite_mcp_rs::{
    config::Cli,
    error::AppError,
    sqlite::{ExecutorConfig, SqliteExecutor},
};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), AppError> {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let executor = SqliteExecutor::open(ExecutorConfig {
        db_path: cli.db.clone(),
        mode: cli.mode,
        max_rows: cli.max_rows,
        timeout_ms: cli.timeout_ms,
    })?;

    tracing::info!("SQLite FTS5: enabled");
    tracing::info!(
        db = %cli.db.display(),
        host = %cli.host,
        port = cli.port,
        mode = ?cli.mode,
        auth_enabled = cli.auth_token.is_some(),
        max_rows = cli.max_rows,
        timeout_ms = cli.timeout_ms,
        "starting sqlite-mcp-rs"
    );

    let app = sqlite_mcp_rs::mcp::router(executor, cli.auth_token)?;
    let addr = std::net::SocketAddr::new(cli.host, cli.port);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
