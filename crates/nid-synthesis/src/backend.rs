//! Pluggable synthesis backends.
//!
//! v1 ships three: Anthropic API, Ollama, claude CLI. Each implements
//! `Backend::refine(prompt) -> Option<String>`. If all are unreachable, the
//! orchestrator falls back to structural-diff only.
//!
//! The actual network/subprocess calls are wired in Phase 6; Phase 1 ships
//! the trait + a no-op implementation so the rest of the pipeline compiles.

use std::future::Future;
use std::pin::Pin;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    Anthropic,
    Ollama,
    ClaudeCli,
    StructuralDiffOnly,
}

pub trait Backend: Send + Sync {
    fn kind(&self) -> BackendKind;

    /// Refine a DSL given a prompt. Returns Some(new DSL as TOML string) or None.
    /// Async to accommodate network/subprocess backends without forcing Tokio
    /// for the structural-diff path.
    fn refine<'a>(
        &'a self,
        prompt: &'a str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<Option<String>>> + Send + 'a>>;
}

/// Default fallback backend: returns None (forces structural-diff-only output).
pub struct NoopBackend;

impl Backend for NoopBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::StructuralDiffOnly
    }
    fn refine<'a>(
        &'a self,
        _prompt: &'a str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<Option<String>>> + Send + 'a>> {
        Box::pin(async { Ok::<Option<String>, anyhow::Error>(None) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn noop_returns_none() {
        let b = NoopBackend;
        let r = b.refine("prompt").await.unwrap();
        assert!(r.is_none());
    }
}
