use std::{env, net::IpAddr, path::PathBuf};

use clap::{Parser, ValueEnum};

pub const DEFAULT_EMBEDDING_BATCH_SIZE: usize = 64;

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
    #[arg(
        long,
        default_value_t = DEFAULT_EMBEDDING_BATCH_SIZE,
        value_parser = parse_positive_usize
    )]
    pub embedding_batch_size: usize,
}

fn parse_positive_usize(value: &str) -> Result<usize, String> {
    let parsed = value
        .parse::<usize>()
        .map_err(|error| format!("invalid positive integer: {error}"))?;
    if parsed == 0 {
        return Err("value must be at least 1".to_string());
    }
    Ok(parsed)
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
    pub batch_size: usize,
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
                batch_size: cli.embedding_batch_size,
            },
        }
    }
}
