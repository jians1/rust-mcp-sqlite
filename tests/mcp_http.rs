use axum::{Json, Router, routing::post};
use reqwest::Client;
use serde_json::{Value, json};
use sqlite_mcp_rs::config::{EmbeddingRuntimeConfig, RunMode};
use sqlite_mcp_rs::embedding::{EMBEDDINGS_PATH, EmbeddingClient};
use sqlite_mcp_rs::mcp::spawn_test_server;
use sqlite_mcp_rs::sqlite::{ExecutorConfig, SqliteExecutor};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

const TEST_EMBEDDINGS_ROUTE: &str = "/v1/embeddings";

#[tokio::test]
async fn mcp_lists_execute_sql_and_vector_tools() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("mcp.db");
    let executor = SqliteExecutor::open(ExecutorConfig {
        db_path,
        mode: RunMode::Readwrite,
        max_rows: 500,
        max_top_k: 100,
        timeout_ms: 10_000,
    })
    .unwrap();

    let embedding_server = spawn_test_embedding_server().await;
    let server = spawn_test_server(executor, None, Some(embedding_server.client()))
        .await
        .unwrap();
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
            "create_text_collection",
            "delete_texts",
            "drop_text_collection",
            "execute_sql",
            "search_text",
            "upsert_texts",
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
        "create_text_collection",
        json!({"collection": "docs"}),
    )
    .await;
    assert_eq!(create["success"], true);
    assert_eq!(create["collection"], "docs");
    assert_eq!(create["dimension"], 2);
    assert_eq!(create["created"], true);

    let upsert = call_tool(
        &client,
        &server.url(),
        5,
        "upsert_texts",
        json!({
            "collection": "docs",
            "items": [
                {
                    "id": "doc-a",
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
        "search_text",
        json!({
            "collection": "docs",
            "query": "alpha query",
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
        "delete_texts",
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
        "drop_text_collection",
        json!({"collection": "docs"}),
    )
    .await;
    assert_eq!(dropped["success"], true);
    assert_eq!(dropped["existed"], true);

    assert_eq!(embedding_server.requests.lock().unwrap().len(), 3);
}

struct TestEmbeddingServer {
    base_url: String,
    requests: Arc<Mutex<Vec<Value>>>,
    shutdown: CancellationToken,
}

impl TestEmbeddingServer {
    fn client(&self) -> EmbeddingClient {
        EmbeddingClient::new(EmbeddingRuntimeConfig {
            base_url: self.base_url.clone(),
            api_key: None,
            model: Some("test-embedding".to_string()),
            dimensions: Some(2),
            timeout_ms: 5_000,
        })
        .unwrap()
    }
}

impl Drop for TestEmbeddingServer {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

async fn spawn_test_embedding_server() -> TestEmbeddingServer {
    assert_eq!(format!("/v1{EMBEDDINGS_PATH}"), TEST_EMBEDDINGS_ROUTE);

    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let shutdown = CancellationToken::new();
    let app = Router::new().route(
        TEST_EMBEDDINGS_ROUTE,
        post({
            let requests = requests.clone();
            move |Json(body): Json<Value>| {
                let requests = requests.clone();
                async move {
                    requests.lock().unwrap().push(body.clone());
                    let input = body["input"].as_array().unwrap();
                    let data = input
                        .iter()
                        .enumerate()
                        .map(|(index, value)| {
                            let text = value.as_str().unwrap();
                            let embedding = if text.contains("beta") {
                                json!([0.0, 1.0])
                            } else {
                                json!([1.0, 0.0])
                            };
                            json!({"index": index, "embedding": embedding})
                        })
                        .collect::<Vec<_>>();
                    Json(json!({"data": data}))
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let server_shutdown = shutdown.clone();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                server_shutdown.cancelled_owned().await;
            })
            .await;
    });

    TestEmbeddingServer {
        base_url: format!("http://{addr}/v1"),
        requests,
        shutdown,
    }
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
