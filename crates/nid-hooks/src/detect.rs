//! Agent and LLM-backend detection (plan §10.1 Phase 1).

use crate::agents::AgentKind;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectedAgent {
    pub kind: AgentKind,
    pub config_path: PathBuf,
    pub config_exists: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DetectedBackends {
    pub anthropic_api_key: bool,
    pub ollama_reachable: bool,
    pub claude_cli: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DetectionResult {
    pub agents: Vec<DetectedAgent>,
    pub backends: DetectedBackends,
}

/// Probe for agents under `home` (tests pass a fixture dir).
pub fn detect_agents(home: &Path) -> DetectionResult {
    let mut out = DetectionResult::default();
    for k in AgentKind::all() {
        let p = k.default_config_path(home);
        out.agents.push(DetectedAgent {
            kind: *k,
            config_exists: p.exists(),
            config_path: p,
        });
    }
    out.backends.anthropic_api_key = std::env::var_os("ANTHROPIC_API_KEY").is_some();
    out.backends.claude_cli = which_on_path("claude").is_some();
    // Ollama check is cheap (TCP probe); keep it quick and non-blocking by
    // just checking whether the binary is present. A full health check is
    // deferred to `nid doctor`.
    out.backends.ollama_reachable = which_on_path("ollama").is_some();
    out
}

fn which_on_path(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for p in std::env::split_paths(&path) {
        for ext in ["", ".exe", ".cmd", ".bat"] {
            let candidate = p.join(format!("{bin}{ext}"));
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn detect_returns_all_agents_with_config_flags() {
        let tmp = TempDir::new().unwrap();
        let r = detect_agents(tmp.path());
        assert_eq!(r.agents.len(), AgentKind::all().len());
        for a in &r.agents {
            assert!(!a.config_exists);
        }
    }

    #[test]
    fn detect_flags_existing_config() {
        let tmp = TempDir::new().unwrap();
        let claude_dir = tmp.path().join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();
        std::fs::write(claude_dir.join("settings.json"), "{}").unwrap();
        let r = detect_agents(tmp.path());
        let cc = r
            .agents
            .iter()
            .find(|a| a.kind == AgentKind::ClaudeCode)
            .unwrap();
        assert!(cc.config_exists);
    }
}
