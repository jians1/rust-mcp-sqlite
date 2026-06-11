use axum::{Router, middleware::from_fn_with_state, routing::post};
use http::{Request, StatusCode};
use sqlite_mcp_rs::auth::{AuthState, require_auth};
use tower::ServiceExt;

async fn ok_handler() -> &'static str {
    "ok"
}

#[tokio::test]
async fn auth_disabled_allows_request() {
    let app = Router::new()
        .route("/mcp", post(ok_handler))
        .layer(from_fn_with_state(AuthState::new(None), require_auth));

    let response = app
        .oneshot(
            Request::post("/mcp")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn auth_enabled_requires_matching_bearer_token() {
    let app = Router::new()
        .route("/mcp", post(ok_handler))
        .layer(from_fn_with_state(
            AuthState::new(Some("secret".to_string())),
            require_auth,
        ));

    for header in [None, Some("Basic secret"), Some("Bearer wrong")] {
        let mut builder = Request::post("/mcp");
        if let Some(value) = header {
            builder = builder.header("authorization", value);
        }
        let response = app
            .clone()
            .oneshot(builder.body(axum::body::Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    let response = app
        .oneshot(
            Request::post("/mcp")
                .header("authorization", "Bearer secret")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}
