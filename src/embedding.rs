use std::time::Duration;

use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

use crate::config::EmbeddingRuntimeConfig;

pub const EMBEDDINGS_PATH: &str = "/embeddings";

#[derive(Clone)]
pub struct EmbeddingClient {
    config: EmbeddingRuntimeConfig,
    client: reqwest::Client,
    endpoint: String,
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
        })
    }

    pub fn from_runtime_config(config: &EmbeddingRuntimeConfig) -> Option<Result<Self, String>> {
        config.model.as_ref().map(|_| Self::new(config.clone()))
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
            dimensions: self.config.dimensions,
        };
        let mut builder = self.client.post(&self.endpoint).json(&request);
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
        if !status.is_success() {
            return Err(format_embedding_status_error(status, &text));
        }

        let parsed: EmbeddingResponse = serde_json::from_str(&text)
            .map_err(|error| format!("embedding response JSON is malformed: {error}"))?;
        if parsed.data.len() != input.len() {
            return Err(format!(
                "embedding response count mismatch: expected {}, got {}",
                input.len(),
                parsed.data.len()
            ));
        }

        Ok(parsed
            .data
            .into_iter()
            .map(|item| item.embedding)
            .collect())
    }
}

fn format_embedding_status_error(status: StatusCode, body: &str) -> String {
    let excerpt: String = body.chars().take(300).collect();
    if excerpt.is_empty() {
        format!("embedding HTTP response was not successful: {status}")
    } else {
        format!("embedding HTTP response was not successful: {status}: {excerpt}")
    }
}
