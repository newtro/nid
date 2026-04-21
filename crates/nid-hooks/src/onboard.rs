//! Onboard flow (plan §10.1).
//!
//! This module is responsible for writing hook configs into each agent's
//! settings files and producing an `onboard.backup.json` so `--uninstall`
//! can restore byte-perfect.
//!
//! We write hook fragments as JSON patches; the actual per-agent JSON keys
//! vary but nid's *outer* API is the same: given a set of agents, produce
//! a plan, apply it, verify it.

use crate::agents::{Agent, AgentKind};
use crate::detect::DetectionResult;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Default)]
pub struct OnboardOptions {
    pub non_interactive: bool,
    pub check_only: bool,
    pub agents: Option<Vec<AgentKind>>,
    pub disable_synthesis: bool,
    pub budget_usd: Option<f64>,
    pub preserve_raw: Option<bool>,
}

/// Planned change against a single agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedChange {
    pub agent: AgentKind,
    pub config_path: PathBuf,
    pub action: PlannedAction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum PlannedAction {
    CreateHookConfig { snippet: String },
    MergeHookConfig { snippet: String },
    WrapAiderConf { key: String, value: String },
    AlreadyInstalled,
    Skip { reason: String },
}

/// Computed onboard plan. Printed to the user before apply.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnboardPlan {
    pub changes: Vec<PlannedChange>,
    pub backup_path: PathBuf,
}

/// JSON body written to `onboard.backup.json` on apply — allows byte-perfect
/// uninstall (plan §10.2).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OnboardBackup {
    pub schema: u32,
    pub installed_at: i64,
    pub originals: BTreeMap<String, Option<String>>,
}

/// Build a plan from a detection result + options.
pub fn plan(detected: &DetectionResult, opts: &OnboardOptions, backup_path: PathBuf) -> OnboardPlan {
    let mut out = Vec::new();
    let filter = opts.agents.as_ref();
    for a in &detected.agents {
        if let Some(filter) = filter {
            if !filter.contains(&a.kind) {
                continue;
            }
        }
        let change = match (a.kind, a.config_exists) {
            (AgentKind::Aider, _) => PlannedChange {
                agent: a.kind,
                config_path: a.config_path.clone(),
                action: PlannedAction::WrapAiderConf {
                    key: "shell-command-prefix".into(),
                    value: "nid".into(),
                },
            },
            (_, false) => PlannedChange {
                agent: a.kind,
                config_path: a.config_path.clone(),
                action: PlannedAction::CreateHookConfig {
                    snippet: default_snippet_for(a.kind),
                },
            },
            (_, true) => PlannedChange {
                agent: a.kind,
                config_path: a.config_path.clone(),
                action: PlannedAction::MergeHookConfig {
                    snippet: default_snippet_for(a.kind),
                },
            },
        };
        out.push(change);
    }
    OnboardPlan {
        changes: out,
        backup_path,
    }
}

fn default_snippet_for(_k: AgentKind) -> String {
    // Placeholder snippet; the per-agent deep integration is in Phase 2. This
    // structure is stable enough to let us test planning now and fill in the
    // agent-specific JSON shape in Phase 2.
    serde_json::json!({
        "hooks": {
            "PreToolUse": [
                {
                    "match": { "tool_name": "Bash" },
                    "run": "nid-hook"
                }
            ]
        }
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::{DetectedAgent, DetectedBackends};

    fn stub_detection() -> DetectionResult {
        DetectionResult {
            agents: AgentKind::all()
                .iter()
                .map(|k| DetectedAgent {
                    kind: *k,
                    config_path: std::path::PathBuf::from("/tmp").join(format!("{}.json", k.as_str())),
                    config_exists: false,
                })
                .collect(),
            backends: DetectedBackends::default(),
        }
    }

    #[test]
    fn plan_includes_all_agents_when_none_filter() {
        let det = stub_detection();
        let opts = OnboardOptions::default();
        let p = plan(&det, &opts, std::path::PathBuf::from("/tmp/b.json"));
        assert_eq!(p.changes.len(), AgentKind::all().len());
    }

    #[test]
    fn plan_respects_agent_filter() {
        let det = stub_detection();
        let mut opts = OnboardOptions::default();
        opts.agents = Some(vec![AgentKind::ClaudeCode]);
        let p = plan(&det, &opts, std::path::PathBuf::from("/tmp/b.json"));
        assert_eq!(p.changes.len(), 1);
        assert_eq!(p.changes[0].agent, AgentKind::ClaudeCode);
    }

    #[test]
    fn aider_plan_wraps_conf() {
        let det = stub_detection();
        let mut opts = OnboardOptions::default();
        opts.agents = Some(vec![AgentKind::Aider]);
        let p = plan(&det, &opts, std::path::PathBuf::from("/tmp/b.json"));
        assert!(matches!(
            p.changes[0].action,
            PlannedAction::WrapAiderConf { .. }
        ));
    }
}

/// Keep `Agent` re-exported so downstream has a symmetric API. Placing this
/// here keeps the no-op re-export from leaking into `lib.rs`.
#[allow(dead_code)]
fn _re_export_agent(a: &Agent) -> AgentKind {
    a.kind
}
