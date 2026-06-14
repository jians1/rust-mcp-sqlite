use axum::{Json, Router, response::IntoResponse, routing::post};
use http::StatusCode;
use serde_json::{Value, json};
use sqlite_mcp_rs::{
    config::{DEFAULT_EMBEDDING_BATCH_SIZE, EmbeddingRuntimeConfig},
    embedding::{EMBEDDINGS_PATH, EmbeddingClient},
};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

const TEST_EMBEDDINGS_ROUTE: &str = "/v1/embeddings";

#[tokio::test]
async fn embedding_client_posts_openai_compatible_request() {
    assert_eq!(format!("/v1{EMBEDDINGS_PATH}"), TEST_EMBEDDINGS_ROUTE);

    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let server = spawn_embedding_server(
        requests.clone(),
        json!({
            "data": [
                {"index": 0, "embedding": [0.1, 0.2]},
                {"index": 1, "embedding": [0.3, 0.4]}
            ]
        }),
    )
    .await;

    let client = EmbeddingClient::new(EmbeddingRuntimeConfig {
        base_url: server.base_url(),
        api_key: Some("secret".to_string()),
        model: Some("text-embedding-3-small".to_string()),
        dimensions: Some(2),
        timeout_ms: 5_000,
        batch_size: DEFAULT_EMBEDDING_BATCH_SIZE,
    })
    .unwrap();

    let embeddings = client
        .embed(&["alpha".to_string(), "beta".to_string()])
        .await
        .unwrap();

    assert_eq!(embeddings, vec![vec![0.1, 0.2], vec![0.3, 0.4]]);
    let captured = requests.lock().unwrap();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0]["model"], "text-embedding-3-small");
    assert_eq!(captured[0]["input"], json!(["alpha", "beta"]));
    assert_eq!(captured[0]["dimensions"], 2);
}

#[tokio::test]
async fn embedding_client_rejects_malformed_json() {
    let shutdown = CancellationToken::new();
    let app = Router::new().route(
        TEST_EMBEDDINGS_ROUTE,
        post(|| async { (StatusCode::OK, "not-json") }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_shutdown = shutdown.clone();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                server_shutdown.cancelled_owned().await;
            })
            .await;
    });

    let client = EmbeddingClient::new(EmbeddingRuntimeConfig {
        base_url: format!("http://{addr}/v1"),
        api_key: None,
        model: Some("model".to_string()),
        dimensions: None,
        timeout_ms: 5_000,
        batch_size: DEFAULT_EMBEDDING_BATCH_SIZE,
    })
    .unwrap();

    let err = client.embed(&["alpha".to_string()]).await.unwrap_err();
    assert!(err.contains("malformed"), "{err}");
    shutdown.cancel();
}

#[tokio::test]
async fn embedding_client_retries_without_dimensions_after_bad_request() {
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
                    if body.get("dimensions").is_some() {
                        (
                            StatusCode::BAD_REQUEST,
                            Json(json!({
                                "error": {
                                    "message": "parameter validation failed"
                                }
                            })),
                        )
                            .into_response()
                    } else {
                        (
                            StatusCode::OK,
                            Json(json!({
                                "data": [
                                    {"index": 0, "embedding": [0.1, 0.2]}
                                ]
                            })),
                        )
                            .into_response()
                    }
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_shutdown = shutdown.clone();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                server_shutdown.cancelled_owned().await;
            })
            .await;
    });

    let client = EmbeddingClient::new(EmbeddingRuntimeConfig {
        base_url: format!("http://{addr}/v1"),
        api_key: None,
        model: Some("model".to_string()),
        dimensions: Some(2),
        timeout_ms: 5_000,
        batch_size: DEFAULT_EMBEDDING_BATCH_SIZE,
    })
    .unwrap();

    let embeddings = client.embed(&["alpha".to_string()]).await.unwrap();

    assert_eq!(embeddings, vec![vec![0.1, 0.2]]);
    let captured = requests.lock().unwrap();
    assert_eq!(captured.len(), 2);
    assert_eq!(captured[0]["dimensions"], 2);
    assert!(captured[1].get("dimensions").is_none());
    shutdown.cancel();
}

#[tokio::test]
async fn embedding_client_caches_dimensions_bad_request_fallback_across_clones() {
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
                    if body.get("dimensions").is_some() {
                        (
                            StatusCode::BAD_REQUEST,
                            Json(json!({
                                "error": {
                                    "message": "parameter validation failed"
                                }
                            })),
                        )
                            .into_response()
                    } else {
                        (
                            StatusCode::OK,
                            Json(json!({
                                "data": [
                                    {"index": 0, "embedding": [0.1, 0.2]}
                                ]
                            })),
                        )
                            .into_response()
                    }
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_shutdown = shutdown.clone();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                server_shutdown.cancelled_owned().await;
            })
            .await;
    });

    let client = EmbeddingClient::new(EmbeddingRuntimeConfig {
        base_url: format!("http://{addr}/v1"),
        api_key: None,
        model: Some("model".to_string()),
        dimensions: Some(2),
        timeout_ms: 5_000,
        batch_size: DEFAULT_EMBEDDING_BATCH_SIZE,
    })
    .unwrap();
    let cloned = client.clone();

    client.embed(&["alpha".to_string()]).await.unwrap();
    cloned.embed(&["beta".to_string()]).await.unwrap();

    let captured = requests.lock().unwrap();
    assert_eq!(captured.len(), 3);
    assert_eq!(captured[0]["dimensions"], 2);
    assert!(captured[1].get("dimensions").is_none());
    assert!(captured[2].get("dimensions").is_none());
    shutdown.cancel();
}

#[tokio::test]
async fn embedding_client_rejects_wrong_response_count() {
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let server = spawn_embedding_server(
        requests,
        json!({
            "data": [
                {"index": 0, "embedding": [0.1, 0.2]}
            ]
        }),
    )
    .await;

    let client = EmbeddingClient::new(EmbeddingRuntimeConfig {
        base_url: server.base_url(),
        api_key: None,
        model: Some("model".to_string()),
        dimensions: None,
        timeout_ms: 5_000,
        batch_size: DEFAULT_EMBEDDING_BATCH_SIZE,
    })
    .unwrap();

    let err = client
        .embed(&["alpha".to_string(), "beta".to_string()])
        .await
        .unwrap_err();
    assert!(err.contains("count mismatch"), "{err}");
}

#[tokio::test]
async fn embedding_client_reports_non_success_status_with_body_excerpt() {
    let shutdown = CancellationToken::new();
    let app = Router::new().route(
        TEST_EMBEDDINGS_ROUTE,
        post(|| async { (StatusCode::UNAUTHORIZED, "invalid api key") }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_shutdown = shutdown.clone();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                server_shutdown.cancelled_owned().await;
            })
            .await;
    });

    let client = EmbeddingClient::new(EmbeddingRuntimeConfig {
        base_url: format!("http://{addr}/v1"),
        api_key: Some("secret".to_string()),
        model: Some("model".to_string()),
        dimensions: None,
        timeout_ms: 5_000,
        batch_size: DEFAULT_EMBEDDING_BATCH_SIZE,
    })
    .unwrap();

    let err = client.embed(&["alpha".to_string()]).await.unwrap_err();
    assert!(err.contains("401 Unauthorized"), "{err}");
    assert!(err.contains("invalid api key"), "{err}");
    assert!(!err.contains("secret"), "{err}");
    shutdown.cancel();
}

#[tokio::test]
async fn embedding_client_retries_429_then_succeeds() {
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let server = spawn_embedding_status_sequence_server(
        requests.clone(),
        vec![
            (
                StatusCode::TOO_MANY_REQUESTS,
                json!({"error": {"message": "rate limited"}}),
            ),
            (
                StatusCode::OK,
                json!({"data": [{"index": 0, "embedding": [0.1, 0.2]}]}),
            ),
        ],
    )
    .await;

    let client = EmbeddingClient::new(EmbeddingRuntimeConfig {
        base_url: server.base_url(),
        api_key: None,
        model: Some("model".to_string()),
        dimensions: None,
        timeout_ms: 5_000,
        batch_size: DEFAULT_EMBEDDING_BATCH_SIZE,
    })
    .unwrap();

    let embeddings = client.embed(&["alpha".to_string()]).await.unwrap();

    assert_eq!(embeddings, vec![vec![0.1, 0.2]]);
    assert_eq!(requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn embedding_client_retries_5xx_then_succeeds() {
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let server = spawn_embedding_status_sequence_server(
        requests.clone(),
        vec![
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({"error": {"message": "temporary"}}),
            ),
            (
                StatusCode::BAD_GATEWAY,
                json!({"error": {"message": "gateway"}}),
            ),
            (
                StatusCode::OK,
                json!({"data": [{"index": 0, "embedding": [0.3, 0.4]}]}),
            ),
        ],
    )
    .await;

    let client = EmbeddingClient::new(EmbeddingRuntimeConfig {
        base_url: server.base_url(),
        api_key: None,
        model: Some("model".to_string()),
        dimensions: None,
        timeout_ms: 5_000,
        batch_size: DEFAULT_EMBEDDING_BATCH_SIZE,
    })
    .unwrap();

    let embeddings = client.embed(&["alpha".to_string()]).await.unwrap();

    assert_eq!(embeddings, vec![vec![0.3, 0.4]]);
    assert_eq!(requests.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn embedding_client_does_not_retry_401() {
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let server = spawn_embedding_status_sequence_server(
        requests.clone(),
        vec![(
            StatusCode::UNAUTHORIZED,
            json!({"error": {"message": "invalid api key"}}),
        )],
    )
    .await;

    let client = EmbeddingClient::new(EmbeddingRuntimeConfig {
        base_url: server.base_url(),
        api_key: Some("secret".to_string()),
        model: Some("model".to_string()),
        dimensions: None,
        timeout_ms: 5_000,
        batch_size: DEFAULT_EMBEDDING_BATCH_SIZE,
    })
    .unwrap();

    let err = client.embed(&["alpha".to_string()]).await.unwrap_err();

    assert!(err.contains("401 Unauthorized"), "{err}");
    assert!(err.contains("invalid api key"), "{err}");
    assert!(!err.contains("secret"), "{err}");
    assert_eq!(requests.lock().unwrap().len(), 1);
}

struct TestEmbeddingServer {
    base_url: String,
    shutdown: CancellationToken,
}

impl TestEmbeddingServer {
    fn base_url(&self) -> String {
        self.base_url.clone()
    }
}

impl Drop for TestEmbeddingServer {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

async fn spawn_embedding_server(
    requests: Arc<Mutex<Vec<Value>>>,
    response: Value,
) -> TestEmbeddingServer {
    let shutdown = CancellationToken::new();
    let app = Router::new().route(
        TEST_EMBEDDINGS_ROUTE,
        post({
            let requests = requests.clone();
            move |Json(body): Json<Value>| {
                let requests = requests.clone();
                let response = response.clone();
                async move {
                    requests.lock().unwrap().push(body);
                    Json(response)
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
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
        shutdown,
    }
}

async fn spawn_embedding_status_sequence_server(
    requests: Arc<Mutex<Vec<Value>>>,
    responses: Vec<(StatusCode, Value)>,
) -> TestEmbeddingServer {
    let shutdown = CancellationToken::new();
    let responses = Arc::new(Mutex::new(responses));
    let app = Router::new().route(
        TEST_EMBEDDINGS_ROUTE,
        post({
            let requests = requests.clone();
            let responses = responses.clone();
            move |Json(body): Json<Value>| {
                let requests = requests.clone();
                let responses = responses.clone();
                async move {
                    requests.lock().unwrap().push(body);
                    let mut responses = responses.lock().unwrap();
                    let (status, body) = responses.remove(0);
                    (status, Json(body)).into_response()
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
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
        shutdown,
    }
}
