use std::sync::Arc;

use axum::{
    body::Body,
    extract::State,
    http::{HeaderValue, Request, StatusCode, header::AUTHORIZATION},
    middleware::Next,
    response::{IntoResponse, Response},
};

#[derive(Clone)]
pub struct AuthState {
    token: Option<Arc<str>>,
}

impl AuthState {
    pub fn new(token: Option<String>) -> Self {
        Self {
            token: token.map(Arc::<str>::from),
        }
    }
}

pub async fn require_auth(
    State(state): State<AuthState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if let Some(expected) = state.token.as_deref() {
        let header = request.headers().get(AUTHORIZATION);
        if !bearer_matches(header, expected) {
            return StatusCode::UNAUTHORIZED.into_response();
        }
    }

    next.run(request).await
}

fn bearer_matches(header: Option<&HeaderValue>, expected: &str) -> bool {
    let Some(header) = header else {
        return false;
    };
    let Ok(value) = header.to_str() else {
        return false;
    };

    value.strip_prefix("Bearer ") == Some(expected)
}
