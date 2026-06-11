use clap::Parser;
use sqlite_mcp_rs::config::{Cli, RunMode};
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
        .stdout(predicates::str::contains("--timeout-ms"));
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
