//! Per-agent hook writers (plan §4.3).
//!
//! For each supported agent we know:
//! - where the agent's config/hook file lives on disk,
//! - what JSON shape the agent's hook expects,
//! - how to accept a mock PreTool payload and return `updatedInput`.
//!
//! Hook scripts write JSON to stdout; the actual rewrite logic is a single
//! call into `crate::rewrite::rewrite_command` regardless of agent.

use crate::rewrite::{rewrite_command, RewriteDecision, RewriteOptions};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Every agent nid can install hooks into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    ClaudeCode,
    Cursor,
    CodexCli,
    GeminiCli,
    CopilotCli,
    Windsurf,
    OpenCode,
    /// Config-file wrapper; no per-invocation hook API.
    Aider,
}

impl AgentKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            AgentKind::ClaudeCode => "claude_code",
            AgentKind::Cursor => "cursor",
            AgentKind::CodexCli => "codex_cli",
            AgentKind::GeminiCli => "gemini_cli",
            AgentKind::CopilotCli => "copilot_cli",
            AgentKind::Windsurf => "windsurf",
            AgentKind::OpenCode => "opencode",
            AgentKind::Aider => "aider",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            AgentKind::ClaudeCode => "Claude Code",
            AgentKind::Cursor => "Cursor",
            AgentKind::CodexCli => "Codex CLI",
            AgentKind::GeminiCli => "Gemini CLI",
            AgentKind::CopilotCli => "Copilot CLI",
            AgentKind::Windsurf => "Windsurf",
            AgentKind::OpenCode => "OpenCode",
            AgentKind::Aider => "Aider",
        }
    }

    pub fn all() -> &'static [AgentKind] {
        &[
            AgentKind::ClaudeCode,
            AgentKind::Cursor,
            AgentKind::CodexCli,
            AgentKind::GeminiCli,
            AgentKind::CopilotCli,
            AgentKind::Windsurf,
            AgentKind::OpenCode,
            AgentKind::Aider,
        ]
    }

    /// Whether this agent has a per-invocation hook API (as opposed to
    /// config-file wrapping).
    pub fn has_hook_api(&self) -> bool {
        !matches!(self, AgentKind::Aider)
    }

    /// Default config path for this agent, relative to the user's home.
    pub fn default_config_path(&self, home: &Path) -> PathBuf {
        match self {
            AgentKind::ClaudeCode => home.join(".claude").join("settings.json"),
            AgentKind::Cursor => home.join(".cursor").join("hooks.json"),
            AgentKind::CodexCli => home.join(".codex").join("hooks.json"),
            AgentKind::GeminiCli => home.join(".gemini").join("hooks.json"),
            AgentKind::CopilotCli => home.join(".github-copilot").join("hooks.json"),
            AgentKind::Windsurf => home.join(".codeium").join("windsurf").join("hooks.json"),
            AgentKind::OpenCode => home.join(".config").join("opencode").join("hooks.json"),
            AgentKind::Aider => home.join(".aider.conf.yml"),
        }
    }
}

/// A single agent record as understood by nid.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub kind: AgentKind,
    pub config_path: PathBuf,
}

/// Registry of installed agents (matches `agent_registry` table in storage).
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct AgentRegistry {
    pub agents: Vec<Agent>,
}

/// Common mock PreTool payload shape we use internally. Each agent's real
/// payload is mapped to/from this in `handle_payload`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreToolPayload {
    pub tool_name: String,
    pub command: String,
    #[serde(default)]
    pub shadow: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookResponse {
    /// Shape mirrors Claude Code's `updatedInput` form — other agents translate
    /// in their wrappers, but the core field we always populate is the
    /// command string.
    #[serde(rename = "updatedInput", skip_serializing_if = "Option::is_none")]
    pub updated_input: Option<serde_json::Value>,
    /// `additionalContext` is used by agents that support structured
    /// attestation blocks.
    #[serde(rename = "additionalContext", skip_serializing_if = "Option::is_none")]
    pub additional_context: Option<serde_json::Value>,
    /// Free-form debug field; never consumed by agents but shows up in tests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debug: Option<String>,
}

/// Handle a PreTool payload the same way for every agent.
pub fn handle_payload(payload: &PreToolPayload, extra_passthrough: &[String]) -> HookResponse {
    if payload.tool_name.to_lowercase() != "bash" {
        // Non-Bash tool — leave untouched.
        return HookResponse {
            updated_input: None,
            additional_context: None,
            debug: Some("non-bash-tool".into()),
        };
    }
    let opts = RewriteOptions {
        shadow: payload.shadow,
        extra_passthrough: extra_passthrough.to_vec(),
    };
    let dec = rewrite_command(&payload.command, &opts);
    match dec {
        RewriteDecision::Rewritten { updated, .. } => HookResponse {
            updated_input: Some(serde_json::json!({ "command": updated })),
            additional_context: None,
            debug: Some("rewritten".into()),
        },
        RewriteDecision::Passthrough { reason, original } => HookResponse {
            // Passthrough means: do not alter the input. For `NID_RAW=1`,
            // emit the unwrapped form explicitly so downstream gets raw.
            updated_input: if reason == "nid_raw_escape" {
                Some(serde_json::json!({ "command": original }))
            } else {
                None
            },
            additional_context: None,
            debug: Some(format!("passthrough:{reason}")),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_agent_has_a_config_path() {
        let home = PathBuf::from("/home/tester");
        for a in AgentKind::all() {
            let p = a.default_config_path(&home);
            assert!(p.starts_with(&home), "{:?} not under home", a);
        }
    }

    #[test]
    fn aider_is_config_file_only() {
        assert!(!AgentKind::Aider.has_hook_api());
        for a in AgentKind::all() {
            if *a != AgentKind::Aider {
                assert!(a.has_hook_api(), "{:?} should have hook API", a);
            }
        }
    }

    #[test]
    fn bash_rewrite_via_payload() {
        let r = handle_payload(
            &PreToolPayload {
                tool_name: "Bash".into(),
                command: "pytest -v".into(),
                shadow: false,
            },
            &[],
        );
        assert_eq!(
            r.updated_input.unwrap(),
            serde_json::json!({"command": "nid pytest -v"})
        );
    }

    #[test]
    fn non_bash_tool_is_untouched() {
        let r = handle_payload(
            &PreToolPayload {
                tool_name: "Read".into(),
                command: "src/foo.rs".into(),
                shadow: false,
            },
            &[],
        );
        assert!(r.updated_input.is_none());
    }

    #[test]
    fn idempotent_payload_is_passthrough() {
        let r = handle_payload(
            &PreToolPayload {
                tool_name: "Bash".into(),
                command: "nid pytest -v".into(),
                shadow: false,
            },
            &[],
        );
        assert!(r.updated_input.is_none());
        assert_eq!(r.debug.as_deref(), Some("passthrough:already_nid"));
    }

    #[test]
    fn shadow_mode_prefixes_correctly() {
        let r = handle_payload(
            &PreToolPayload {
                tool_name: "Bash".into(),
                command: "cargo build".into(),
                shadow: true,
            },
            &[],
        );
        let cmd = r
            .updated_input
            .unwrap()
            .get("command")
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(cmd, "nid --shadow cargo build");
    }
}
