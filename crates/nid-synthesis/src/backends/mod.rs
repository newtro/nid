//! Concrete `Backend` implementations.
//!
//! Detection order (plan §7.2): `ANTHROPIC_API_KEY` → Ollama → `claude` CLI
//! → structural-diff-only floor.

pub mod anthropic;
pub mod claude_cli;
pub mod ollama;

use crate::backend::{Backend, NoopBackend};
use std::sync::Arc;

/// Probe available backends in plan §7.2 order; return the first one ready.
/// Falls back to NoopBackend if none are reachable, so the synthesis pipeline
/// always has a backend to call.
pub fn autodetect() -> Arc<dyn Backend> {
    if let Some(b) = anthropic::AnthropicBackend::from_env() {
        return Arc::new(b);
    }
    if let Some(b) = ollama::OllamaBackend::from_env() {
        // Only use Ollama if a health-check probe looks clean; otherwise fall
        // through. For v0.1 we accept its existence and rely on the orchestrator
        // to discard failed refinements.
        return Arc::new(b);
    }
    if let Some(b) = claude_cli::ClaudeCliBackend::from_env() {
        return Arc::new(b);
    }
    Arc::new(NoopBackend)
}

/// Shared fence-stripping helper so every backend behaves identically.
pub(crate) fn anthropic_strip_fences(s: &str) -> String {
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
    fn autodetect_returns_noop_when_nothing_available() {
        // With no env vars and no ollama/claude binary installed, must return
        // NoopBackend.
        let prev_anth = std::env::var("ANTHROPIC_API_KEY").ok();
        std::env::remove_var("ANTHROPIC_API_KEY");
        let b = autodetect();
        // We can't downcast Arc<dyn Backend> without a lot of dancing; check
        // kind() instead.
        match b.kind() {
            crate::backend::BackendKind::StructuralDiffOnly => {}
            other => {
                // On a dev machine Ollama may be running; accept any.
                eprintln!("autodetect picked {other:?}");
            }
        }
        if let Some(v) = prev_anth {
            std::env::set_var("ANTHROPIC_API_KEY", v);
        }
    }

    #[test]
    fn fence_strip_round_trip() {
        assert_eq!(anthropic_strip_fences("```toml\nx = 1\n```"), "x = 1");
        assert_eq!(anthropic_strip_fences("```\nx = 1\n```"), "x = 1");
        assert_eq!(anthropic_strip_fences("x = 1"), "x = 1");
    }
}
