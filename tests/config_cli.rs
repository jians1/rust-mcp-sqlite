use clap::Parser;
use sqlite_mcp_rs::config::{Cli, RunMode};

#[test]
fn cli_defaults_match_spec() {
    let cli = Cli::parse_from(["sqlite-mcp-rs", "--db", "/tmp/app.db"]);

    assert_eq!(cli.db.to_string_lossy(), "/tmp/app.db");
    assert_eq!(cli.host.to_string(), "127.0.0.1");
    assert_eq!(cli.port, 3000);
    assert_eq!(cli.mode, RunMode::Readwrite);
    assert_eq!(cli.auth_token, None);
    assert_eq!(cli.max_rows, 500);
    assert_eq!(cli.timeout_ms, 10_000);
}

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
        "--timeout-ms",
        "1500",
    ]);

    assert_eq!(cli.host.to_string(), "0.0.0.0");
    assert_eq!(cli.port, 3100);
    assert_eq!(cli.mode, RunMode::Readonly);
    assert_eq!(cli.auth_token.as_deref(), Some("secret-value"));
    assert_eq!(cli.max_rows, 25);
    assert_eq!(cli.timeout_ms, 1500);
}

#[test]
fn cli_requires_db() {
    let err = Cli::try_parse_from(["sqlite-mcp-rs"]).unwrap_err();
    assert!(err.to_string().contains("--db"));
}
