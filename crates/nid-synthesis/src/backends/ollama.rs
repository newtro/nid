//! Ollama local-HTTP backend (default `http://127.0.0.1:11434`).

use crate::backend::{Backend, BackendKind};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;

const DEFAULT_URL: &str = "http://127.0.0.1:11434";
const DEFAULT_MODEL: &str = "llama3.1:8b";

#[derive(Debug, Clone)]
pub struct OllamaBackend {
    base_url: String,
    model: String,
    client: Client,
}

impl OllamaBackend {
    pub fn from_env() -> Option<Self> {
        let base_url = std::env::var("OLLAMA_HOST").unwrap_or_else(|_| DEFAULT_URL.to_string());
        let model = std::env::var("OLLAMA_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .ok()?;
        Some(Self {
            base_url,
            model,
            client,
        })
    }
}

#[derive(Serialize)]
struct OllamaRequest<'a> {
    model: &'a str,
    prompt: &'a str,
    stream: bool,
}

#[derive(Deserialize)]
struct OllamaResponse {
    response: String,
}

impl Backend for OllamaBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Ollama
    }

    fn refine<'a>(
        &'a self,
        prompt: &'a str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<Option<String>>> + Send + 'a>> {
        Box::pin(async move {
            let url = format!("{}/api/generate", self.base_url);
            let body = OllamaRequest {
                model: &self.model,
                prompt,
                stream: false,
            };
            let resp = self.client.post(&url).json(&body).send().await?;
            if !resp.status().is_success() {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                anyhow::bail!("ollama returned {status}: {text}");
            }
            let parsed: OllamaResponse = resp.json().await?;
            if parsed.response.trim().is_empty() {
                Ok(None)
            } else {
                Ok(Some(crate::backends::anthropic_strip_fences(
                    &parsed.response,
                )))
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_env_uses_defaults() {
        std::env::remove_var("OLLAMA_HOST");
        std::env::remove_var("OLLAMA_MODEL");
        let b = OllamaBackend::from_env().unwrap();
        assert_eq!(b.base_url, DEFAULT_URL);
        assert_eq!(b.model, DEFAULT_MODEL);
    }
}
