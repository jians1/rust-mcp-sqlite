use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

use crate::config::EmbeddingRuntimeConfig;

pub const EMBEDDINGS_PATH: &str = "/embeddings";

const EMBEDDING_MAX_ATTEMPTS: usize = 3;
const EMBEDDING_RETRY_BASE_DELAY_MS: u64 = 100;
const EMBEDDING_RETRY_MAX_DELAY_MS: u64 = 1_000;

#[derive(Clone)]
pub struct EmbeddingClient {
    config: EmbeddingRuntimeConfig,
    client: reqwest::Client,
    endpoint: String,
    send_dimensions: Arc<AtomicBool>,
}

#[derive(Debug, Serialize)]
struct EmbeddingRequest<'a> {
    model: &'a str,
    input: &'a [String],
    #[serde(skip_serializing_if = "Option::is_none")]
    dimensions: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingData {
    embedding: Vec<f64>,
}

impl EmbeddingClient {
    pub fn new(config: EmbeddingRuntimeConfig) -> Result<Self, String> {
        match config.model.as_deref() {
            Some(model) if !model.is_empty() => {}
            _ => return Err("embedding is not configured; set --embedding-model".to_string()),
        }

        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(config.timeout_ms))
            .build()
            .map_err(|error| error.to_string())?;
        let endpoint = format!(
            "{}{}",
            config.base_url.trim_end_matches('/'),
            EMBEDDINGS_PATH
        );

        Ok(Self {
            config,
            client,
            endpoint,
            send_dimensions: Arc::new(AtomicBool::new(true)),
        })
    }

    pub fn from_runtime_config(config: &EmbeddingRuntimeConfig) -> Option<Result<Self, String>> {
        config.model.as_ref().map(|_| Self::new(config.clone()))
    }

    pub fn batch_size(&self) -> usize {
        self.config.batch_size
    }

    pub async fn embed(&self, input: &[String]) -> Result<Vec<Vec<f64>>, String> {
        if input.is_empty() {
            return Err("embedding input must not be empty".to_string());
        }

        let model = self
            .config
            .model
            .as_deref()
            .ok_or_else(|| "embedding is not configured; set --embedding-model".to_string())?;
        let request = EmbeddingRequest {
            model,
            input,
            dimensions: self
                .send_dimensions
                .load(Ordering::Relaxed)
                .then_some(self.config.dimensions)
                .flatten(),
        };
        let (status, text) = self.post_embedding_request_with_retry(&request).await?;
        if status == StatusCode::BAD_REQUEST && request.dimensions.is_some() {
            let retry_request = EmbeddingRequest {
                model,
                input,
                dimensions: None,
            };
            let (retry_status, retry_text) = self
                .post_embedding_request_with_retry(&retry_request)
                .await?;
            if !retry_status.is_success() {
                return Err(format_embedding_status_error(retry_status, &retry_text));
            }

            self.send_dimensions.store(false, Ordering::Relaxed);
            return parse_embedding_response(&retry_text, input.len());
        }

        if !status.is_success() {
            return Err(format_embedding_status_error(status, &text));
        }

        parse_embedding_response(&text, input.len())
    }

    async fn post_embedding_request_with_retry(
        &self,
        request: &EmbeddingRequest<'_>,
    ) -> Result<(StatusCode, String), String> {
        let mut attempt = 1;
        loop {
            let started = Instant::now();
            let response = self.post_embedding_request_once(request).await;
            let elapsed_ms = started.elapsed().as_millis() as u64;

            match response {
                Ok((status, text)) => {
                    tracing::info!(
                        model = %request.model,
                        input_count = request.input.len(),
                        attempt,
                        status = %status,
                        elapsed_ms,
                        retryable = should_retry_status(status),
                        "embedding HTTP request completed"
                    );
                    if should_retry_status(status) && attempt < EMBEDDING_MAX_ATTEMPTS {
                        tokio::time::sleep(retry_delay(attempt)).await;
                        attempt += 1;
                        continue;
                    }
                    return Ok((status, text));
                }
                Err(message) => {
                    tracing::info!(
                        model = %request.model,
                        input_count = request.input.len(),
                        attempt,
                        elapsed_ms,
                        error = %message,
                        "embedding HTTP request failed"
                    );
                    return Err(message);
                }
            }
        }
    }

    async fn post_embedding_request_once(
        &self,
        request: &EmbeddingRequest<'_>,
    ) -> Result<(StatusCode, String), String> {
        let mut builder = self.client.post(&self.endpoint).json(request);
        if let Some(api_key) = &self.config.api_key {
            builder = builder.bearer_auth(api_key);
        }

        let response = builder.send().await.map_err(|error| {
            if error.is_timeout() {
                "embedding HTTP request timed out".to_string()
            } else {
                format!("embedding HTTP request failed: {error}")
            }
        })?;

        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|error| format!("embedding HTTP response read failed: {error}"))?;

        Ok((status, text))
    }
}

fn should_retry_status(status: StatusCode) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

fn retry_delay(attempt: usize) -> Duration {
    let shift = attempt.saturating_sub(1).min(20) as u32;
    let multiplier = 1_u64 << shift;
    let delay_ms = EMBEDDING_RETRY_BASE_DELAY_MS
        .saturating_mul(multiplier)
        .min(EMBEDDING_RETRY_MAX_DELAY_MS);
    Duration::from_millis(delay_ms)
}

fn parse_embedding_response(text: &str, expected_count: usize) -> Result<Vec<Vec<f64>>, String> {
    let parsed: EmbeddingResponse = serde_json::from_str(text)
        .map_err(|error| format!("embedding response JSON is malformed: {error}"))?;
    if parsed.data.len() != expected_count {
        return Err(format!(
            "embedding response count mismatch: expected {}, got {}",
            expected_count,
            parsed.data.len()
        ));
    }

    Ok(parsed.data.into_iter().map(|item| item.embedding).collect())
}

fn format_embedding_status_error(status: StatusCode, body: &str) -> String {
    let excerpt: String = body.chars().take(300).collect();
    if excerpt.is_empty() {
        format!("embedding HTTP response was not successful: {status}")
    } else {
        format!("embedding HTTP response was not successful: {status}: {excerpt}")
    }
}
