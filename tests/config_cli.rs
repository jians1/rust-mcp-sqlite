use clap::Parser;
use sqlite_mcp_rs::config::{Cli, DEFAULT_EMBEDDING_BATCH_SIZE, RunMode};
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    process::Stdio,
    thread,
    time::{Duration, Instant},
};

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
    assert_eq!(cli.embedding_batch_size, DEFAULT_EMBEDDING_BATCH_SIZE);
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
        "--embedding-batch-size",
        "7",
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
    assert_eq!(
        cli.embedding_model.as_deref(),
        Some("text-embedding-3-small")
    );
    assert_eq!(cli.embedding_dimensions, Some(512));
    assert_eq!(cli.embedding_timeout_ms, 2500);
    assert_eq!(cli.embedding_batch_size, 7);
}

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
    assert_eq!(config.embedding.batch_size, DEFAULT_EMBEDDING_BATCH_SIZE);

    unsafe {
        match original {
            Some(value) => std::env::set_var("OPENAI_API_KEY", value),
            None => std::env::remove_var("OPENAI_API_KEY"),
        }
    }
}

#[test]
fn cli_requires_db() {
    let err = Cli::try_parse_from(["sqlite-mcp-rs"]).unwrap_err();
    assert!(err.to_string().contains("--db"));
}

#[test]
fn binary_help_mentions_expected_flags() {
    let mut cmd = assert_cmd::Command::cargo_bin("sqlite-mcp-rs").unwrap();
    cmd.arg("--help")
        .assert()
        .success()
        .stdout(predicates::str::contains("--db"))
        .stdout(predicates::str::contains("--host"))
        .stdout(predicates::str::contains("--port"))
        .stdout(predicates::str::contains("--mode"))
        .stdout(predicates::str::contains("--auth-token"))
        .stdout(predicates::str::contains("--max-rows"))
        .stdout(predicates::str::contains("--max-top-k"))
        .stdout(predicates::str::contains("--timeout-ms"))
        .stdout(predicates::str::contains("--embedding-base-url"))
        .stdout(predicates::str::contains("--embedding-api-key"))
        .stdout(predicates::str::contains("--embedding-model"))
        .stdout(predicates::str::contains("--embedding-dimensions"))
        .stdout(predicates::str::contains("--embedding-timeout-ms"))
        .stdout(predicates::str::contains("--embedding-batch-size"));
}

#[test]
fn cli_rejects_zero_embedding_batch_size() {
    let err = Cli::try_parse_from([
        "sqlite-mcp-rs",
        "--db",
        "/tmp/app.db",
        "--embedding-batch-size",
        "0",
    ])
    .unwrap_err();

    assert!(err.to_string().contains("0"), "{err}");
}

#[test]
fn binary_starts_http_server() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("server.db");
    let port = unused_local_port();
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let binary = assert_cmd::cargo::cargo_bin("sqlite-mcp-rs");

    let mut child = std::process::Command::new(binary)
        .arg("--db")
        .arg(&db_path)
        .arg("--host")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(port.to_string())
        .arg("--mode")
        .arg("readwrite")
        .arg("--max-rows")
        .arg("500")
        .arg("--timeout-ms")
        .arg("10000")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let started = wait_for_tcp(addr, &mut child, Duration::from_secs(5));
    let _ = child.kill();
    let _ = child.wait();

    started.unwrap();
}

fn unused_local_port() -> u16 {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    listener.local_addr().unwrap().port()
}

fn wait_for_tcp(
    addr: SocketAddr,
    child: &mut std::process::Child,
    timeout: Duration,
) -> Result<(), String> {
    let start = Instant::now();

    loop {
        if TcpStream::connect_timeout(&addr, Duration::from_millis(50)).is_ok() {
            return Ok(());
        }

        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            return Err(format!("server exited before listening: {status}"));
        }

        if start.elapsed() >= timeout {
            return Err(format!(
                "server did not listen on {addr} within {timeout:?}"
            ));
        }

        thread::sleep(Duration::from_millis(50));
    }
}
