//! Anthropic Messages API backend.
//!
//! Implements `Backend` via `reqwest`. Reads `ANTHROPIC_API_KEY` from env.
//! Model is configurable via `ANTHROPIC_MODEL` (default `claude-haiku-4-5-20251001`).
//!
//! The API is called with `max_tokens=2048` to cap spend per call; the prompt
//! is already size-bounded by the orchestrator.

use crate::backend::{Backend, BackendKind};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;

const DEFAULT_MODEL: &str = "claude-haiku-4-5-20251001";
const API_URL: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2023-06-01";

#[derive(Debug, Clone)]
pub struct AnthropicBackend {
    api_key: String,
    model: String,
    client: Client,
}

impl AnthropicBackend {
    /// Construct from env; returns None if `ANTHROPIC_API_KEY` is unset.
    pub fn from_env() -> Option<Self> {
        let api_key = std::env::var("ANTHROPIC_API_KEY").ok()?;
        if api_key.trim().is_empty() {
            return None;
        }
        let model = std::env::var("ANTHROPIC_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .ok()?;
        Some(Self {
            api_key,
            model,
            client,
        })
    }

    pub fn new(api_key: String, model: String) -> Self {
        Self {
            api_key,
            model,
            client: Client::new(),
        }
    }
}

#[derive(Serialize)]
struct AnthropicRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    messages: Vec<Message<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<&'a str>,
}

#[derive(Serialize)]
struct Message<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize)]
struct AnthropicResponse {
    content: Vec<ContentBlock>,
}

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: String,
}

impl Backend for AnthropicBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Anthropic
    }

    fn refine<'a>(
        &'a self,
        prompt: &'a str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<Option<String>>> + Send + 'a>> {
        Box::pin(async move {
            let body = AnthropicRequest {
                model: &self.model,
                max_tokens: 2048,
                messages: vec![Message {
                    role: "user",
                    content: prompt,
                }],
                system: Some(
                    "You are a compression DSL generator. Emit ONLY a TOML \
                     document — no prose, no backticks, no commentary. Your \
                     output will be parsed by toml::from_str directly.",
                ),
            };
            let resp = self
                .client
                .post(API_URL)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", API_VERSION)
                .header("content-type", "application/json")
                .json(&body)
                .send()
                .await?;
            if !resp.status().is_success() {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                anyhow::bail!("anthropic returned {status}: {text}");
            }
            let parsed: AnthropicResponse = resp.json().await?;
            let txt: String = parsed
                .content
                .into_iter()
                .filter(|c| c.kind == "text")
                .map(|c| c.text)
                .collect::<Vec<_>>()
                .join("\n");
            if txt.trim().is_empty() {
                Ok(None)
            } else {
                Ok(Some(strip_fences(&txt)))
            }
        })
    }
}

/// Models sometimes wrap TOML in ```toml fences. Strip them conservatively.
fn strip_fences(s: &str) -> String {
    let t = s.trim();
    if let Some(rest) = t.strip_prefix("```toml") {
        let rest = rest.trim_start_matches('\n');
        if let Some(body) = rest.strip_suffix("```") {
            return body.trim().to_string();
        }
    }
    if let Some(rest) = t.strip_prefix("```") {
        let rest = rest.trim_start_matches('\n');
        if let Some(body) = rest.strip_suffix("```") {
            return body.trim().to_string();
        }
    }
    t.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_fences_removes_toml_fence() {
        let s = "```toml\n[meta]\nfoo = 1\n```";
        assert_eq!(strip_fences(s), "[meta]\nfoo = 1");
    }

    #[test]
    fn strip_fences_removes_plain_fence() {
        let s = "```\n[meta]\nfoo = 1\n```";
        assert_eq!(strip_fences(s), "[meta]\nfoo = 1");
    }

    #[test]
    fn strip_fences_passthrough_when_no_fence() {
        let s = "[meta]\nfoo = 1\n";
        assert_eq!(strip_fences(s), "[meta]\nfoo = 1");
    }

    #[test]
    fn from_env_returns_none_without_key() {
        // Save + clear the env var.
        let prev = std::env::var("ANTHROPIC_API_KEY").ok();
        std::env::remove_var("ANTHROPIC_API_KEY");
        let b = AnthropicBackend::from_env();
        assert!(b.is_none());
        if let Some(v) = prev {
            std::env::set_var("ANTHROPIC_API_KEY", v);
        }
    }
}
