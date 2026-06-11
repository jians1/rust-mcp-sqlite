use reqwest::Client;
use serde_json::{Value, json};
use sqlite_mcp_rs::config::RunMode;
use sqlite_mcp_rs::mcp::spawn_test_server;
use sqlite_mcp_rs::sqlite::{ExecutorConfig, SqliteExecutor};

#[tokio::test]
async fn mcp_lists_only_execute_sql_and_calls_it() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("mcp.db");
    let executor = SqliteExecutor::open(ExecutorConfig {
        db_path,
        mode: RunMode::Readwrite,
        max_rows: 500,
        timeout_ms: 10_000,
    })
    .unwrap();

    let server = spawn_test_server(executor, None).await.unwrap();
    let client = Client::new();

    let initialize: Value = client
        .post(server.url())
        .header("accept", "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "0.1.0"}
            }
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(initialize.get("result").is_some(), "{initialize}");

    let tools: Value = client
        .post(server.url())
        .header("accept", "application/json, text/event-stream")
        .json(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let tools_array = tools["result"]["tools"].as_array().unwrap();
    assert_eq!(tools_array.len(), 1);
    assert_eq!(tools_array[0]["name"], "execute_sql");

    let call: Value = client
        .post(server.url())
        .header("accept", "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "execute_sql",
                "arguments": {"sql": "SELECT 42 AS answer"}
            }
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let text = call["result"]["content"][0]["text"].as_str().unwrap();
    let envelope: Value = serde_json::from_str(text).unwrap();
    assert_eq!(envelope["success"], true);
    assert_eq!(envelope["results"][0]["rows"][0]["answer"], 42);
}
