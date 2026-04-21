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
    /// Construct from env *only if* the Ollama daemon responds to a cheap
    /// TCP probe on its `/api/tags` endpoint. Returns None when unreachable.
    /// (Fixes H6 from the adversarial review — previously this was None-never,
    /// which shadowed the claude-CLI fallback in autodetect order.)
    pub fn from_env() -> Option<Self> {
        let base_url = std::env::var("OLLAMA_HOST").unwrap_or_else(|_| DEFAULT_URL.to_string());
        let model = std::env::var("OLLAMA_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());

        // Reachability probe — short timeout via a raw TCP connect to keep
        // the hot path fast and avoid pulling blocking-reqwest.
        if !tcp_probe(&base_url) {
            return None;
        }

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

    /// Testing constructor that skips the reachability probe.
    #[doc(hidden)]
    pub fn new_unchecked(base_url: String, model: String) -> Self {
        Self {
            base_url,
            model,
            client: Client::new(),
        }
    }
}

fn tcp_probe(base_url: &str) -> bool {
    // Extract host:port. Support `http://host:port` only for v0.1.
    let trimmed = base_url
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    let host_port = trimmed.split('/').next().unwrap_or("");
    let sock: Option<std::net::SocketAddr> = host_port.parse().ok().or_else(|| {
        // Try DNS resolution with a short timeout.
        std::net::ToSocketAddrs::to_socket_addrs(&host_port)
            .ok()
            .and_then(|mut it| it.next())
    });
    let Some(addr) = sock else {
        return false;
    };
    std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(250)).is_ok()
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
    fn from_env_returns_none_when_ollama_unreachable() {
        // Point at a port nothing is listening on.
        std::env::set_var("OLLAMA_HOST", "http://127.0.0.1:1");
        let b = OllamaBackend::from_env();
        std::env::remove_var("OLLAMA_HOST");
        assert!(b.is_none(), "unreachable daemon must return None");
    }

    #[test]
    fn new_unchecked_works_without_probe() {
        let b = OllamaBackend::new_unchecked(DEFAULT_URL.into(), DEFAULT_MODEL.into());
        assert_eq!(b.base_url, DEFAULT_URL);
    }
}
