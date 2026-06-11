use clap::Parser;
use sqlite_mcp_rs::config::{Cli, RuntimeConfig};

fn main() {
    let _config = RuntimeConfig::from(Cli::parse());
    eprintln!("sqlite-mcp-rs server startup is not wired yet");
}
