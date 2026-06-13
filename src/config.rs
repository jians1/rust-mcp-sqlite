use std::{net::IpAddr, path::PathBuf};

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
}

impl From<Cli> for RuntimeConfig {
    fn from(cli: Cli) -> Self {
        Self {
            db: cli.db,
            host: cli.host,
            port: cli.port,
            mode: cli.mode,
            auth_token: cli.auth_token,
            max_rows: cli.max_rows,
            max_top_k: cli.max_top_k,
            timeout_ms: cli.timeout_ms,
        }
    }
}
