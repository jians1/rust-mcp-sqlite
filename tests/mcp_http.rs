use reqwest::Client;
use serde_json::{Value, json};
use sqlite_mcp_rs::config::RunMode;
use sqlite_mcp_rs::mcp::spawn_test_server;
use sqlite_mcp_rs::sqlite::{ExecutorConfig, SqliteExecutor};

#[tokio::test]
async fn mcp_lists_execute_sql_and_vector_tools() {
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
    let mut tool_names = tools_array
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    tool_names.sort();
    assert_eq!(
        tool_names,
        vec![
            "create_vector_collection",
            "delete_vectors",
            "drop_vector_collection",
            "execute_sql",
            "search_vectors",
            "upsert_vectors",
        ]
    );

    let call: Value = call_tool(
        &client,
        &server.url(),
        3,
        "execute_sql",
        json!({"sql": "SELECT 42 AS answer"}),
    )
    .await;
    assert_eq!(call["success"], true);
    assert_eq!(call["results"][0]["rows"][0]["answer"], 42);

    let create = call_tool(
        &client,
        &server.url(),
        4,
        "create_vector_collection",
        json!({"collection": "docs", "dimension": 2}),
    )
    .await;
    assert_eq!(create["success"], true);
    assert_eq!(create["collection"], "docs");
    assert_eq!(create["created"], true);

    let upsert = call_tool(
        &client,
        &server.url(),
        5,
        "upsert_vectors",
        json!({
            "collection": "docs",
            "items": [
                {
                    "id": "doc-a",
                    "vector": [1.0, 0.0],
                    "text": "alpha",
                    "metadata": {"tenant": "a"}
                }
            ]
        }),
    )
    .await;
    assert_eq!(upsert["success"], true);
    assert_eq!(upsert["upserted_count"], 1);

    let search = call_tool(
        &client,
        &server.url(),
        6,
        "search_vectors",
        json!({
            "collection": "docs",
            "vector": [1.0, 0.0],
            "top_k": 1,
            "filter": {"tenant": "a"}
        }),
    )
    .await;
    assert_eq!(search["success"], true);
    assert_eq!(search["results"][0]["id"], "doc-a");
    assert!(search["results"][0].get("vector").is_none());

    let deleted = call_tool(
        &client,
        &server.url(),
        7,
        "delete_vectors",
        json!({"collection": "docs", "ids": ["doc-a", "missing"]}),
    )
    .await;
    assert_eq!(deleted["success"], true);
    assert_eq!(deleted["requested_count"], 2);
    assert_eq!(deleted["deleted_count"], 1);

    let dropped = call_tool(
        &client,
        &server.url(),
        8,
        "drop_vector_collection",
        json!({"collection": "docs"}),
    )
    .await;
    assert_eq!(dropped["success"], true);
    assert_eq!(dropped["existed"], true);
}

async fn call_tool(client: &Client, url: &str, id: u64, name: &str, arguments: Value) -> Value {
    let call: Value = client
        .post(url)
        .header("accept", "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {
                "name": name,
                "arguments": arguments
            }
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let text = call["result"]["content"][0]["text"].as_str().unwrap();
    serde_json::from_str(text).unwrap()
}
