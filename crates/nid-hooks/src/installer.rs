//! Per-agent installer — writes hook JSON into agent config files and
//! creates `onboard.backup.json` for byte-perfect uninstall.
//!
//! Each agent's config is in JSON; we merge our PreTool-Bash entry in,
//! stashing the original contents in the backup.

use crate::agents::AgentKind;
use crate::onboard::{OnboardBackup, OnboardPlan, PlannedAction};
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum InstallerError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json parse error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("toml error: {0}")]
    Toml(String),
}

/// Apply an `OnboardPlan` against the filesystem. Writes backup file to
/// `backup_path` and per-agent hook JSON into each config.
pub fn apply(plan: &OnboardPlan, nid_binary: &str) -> Result<OnboardBackup, InstallerError> {
    let mut backup = OnboardBackup {
        schema: 1,
        installed_at: unix_now(),
        originals: Default::default(),
    };
    for change in &plan.changes {
        let path = &change.config_path;
        let original = if path.exists() {
            Some(fs::read_to_string(path)?)
        } else {
            None
        };
        backup
            .originals
            .insert(change.agent.as_str().to_string(), original.clone());

        match &change.action {
            PlannedAction::CreateHookConfig { .. } | PlannedAction::MergeHookConfig { .. } => {
                let new_body = merge_hook_into(change.agent, original.as_deref(), nid_binary)?;
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)?;
                }
                atomic_write(path, &new_body)?;
            }
            PlannedAction::WrapAiderConf { key, value } => {
                let new_body = merge_aider_yaml(original.as_deref(), key, value);
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)?;
                }
                atomic_write(path, &new_body)?;
            }
            PlannedAction::AlreadyInstalled | PlannedAction::Skip { .. } => {}
        }
    }

    // Write the backup file.
    fs::create_dir_all(plan.backup_path.parent().unwrap_or(Path::new(".")))?;
    atomic_write(&plan.backup_path, &serde_json::to_string_pretty(&backup)?)?;
    Ok(backup)
}

/// Restore byte-perfect from a backup file (uninstall).
pub fn uninstall(backup_path: &Path) -> Result<(), InstallerError> {
    let body = fs::read_to_string(backup_path)?;
    let backup: OnboardBackup = serde_json::from_str(&body)?;
    for (agent_str, original) in &backup.originals {
        let Some(kind) = AgentKind::all().iter().find(|a| a.as_str() == agent_str) else {
            continue;
        };
        let home = directories::BaseDirs::new()
            .map(|b| b.home_dir().to_path_buf())
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let path = kind.default_config_path(&home);
        match original {
            Some(body) => {
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)?;
                }
                atomic_write(&path, body)?;
            }
            None => {
                if path.exists() {
                    fs::remove_file(&path)?;
                }
            }
        }
    }
    Ok(())
}

fn merge_hook_into(
    agent: AgentKind,
    existing: Option<&str>,
    nid_binary: &str,
) -> Result<String, InstallerError> {
    let mut doc: Value = match existing {
        Some(s) if !s.trim().is_empty() => serde_json::from_str(s)?,
        _ => json!({}),
    };
    match agent {
        AgentKind::ClaudeCode => inject_claude_code(&mut doc, nid_binary),
        AgentKind::Cursor => inject_simple(&mut doc, "beforeShellExecution", nid_binary),
        AgentKind::CodexCli => inject_pretool(&mut doc, nid_binary),
        AgentKind::GeminiCli => inject_simple(&mut doc, "BeforeTool", nid_binary),
        AgentKind::CopilotCli => inject_pretool(&mut doc, nid_binary),
        AgentKind::Windsurf => inject_simple(&mut doc, "preCascade", nid_binary),
        AgentKind::OpenCode => inject_simple(&mut doc, "before_tool_call", nid_binary),
        AgentKind::Aider => { /* handled separately */ }
    }
    Ok(serde_json::to_string_pretty(&doc)?)
}

/// Claude Code uses `PreToolUse` with a `matchers` array. Emits `updatedInput`
/// without `permissionDecision` to survive bypassPermissions (plan §4.3).
fn inject_claude_code(doc: &mut Value, nid_binary: &str) {
    let hooks = doc.as_object_mut().unwrap();
    let entry = json!({
        "matcher": "Bash",
        "hooks": [{
            "type": "command",
            "command": format!("{nid_binary} __hook claude_code")
        }]
    });
    let root = hooks
        .entry("hooks".to_string())
        .or_insert_with(|| json!({}));
    let pretool = root
        .as_object_mut()
        .unwrap()
        .entry("PreToolUse".to_string())
        .or_insert_with(|| json!([]));
    if let Some(arr) = pretool.as_array_mut() {
        // Don't duplicate.
        let already = arr.iter().any(|v| {
            v.get("hooks")
                .and_then(|h| h.as_array())
                .map(|a| {
                    a.iter().any(|x| {
                        x.get("command")
                            .and_then(|c| c.as_str())
                            .map(|s| s.contains(nid_binary))
                            .unwrap_or(false)
                    })
                })
                .unwrap_or(false)
        });
        if !already {
            arr.push(entry);
        }
    }
}

fn inject_simple(doc: &mut Value, hook_key: &str, nid_binary: &str) {
    let obj = doc.as_object_mut().unwrap();
    let hooks = obj.entry("hooks".to_string()).or_insert_with(|| json!({}));
    let this = hooks
        .as_object_mut()
        .unwrap()
        .entry(hook_key.to_string())
        .or_insert_with(|| json!([]));
    let entry = json!({
        "match": { "tool": "bash" },
        "run": format!("{nid_binary} __hook {}", hook_key)
    });
    if let Some(arr) = this.as_array_mut() {
        let already = arr.iter().any(|v| {
            v.get("run")
                .and_then(|c| c.as_str())
                .map(|s| s.contains(nid_binary))
                .unwrap_or(false)
        });
        if !already {
            arr.push(entry);
        }
    }
}

fn inject_pretool(doc: &mut Value, nid_binary: &str) {
    inject_simple(doc, "PreToolUse", nid_binary)
}

/// YAML munging for `.aider.conf.yml`. We don't pull in a YAML crate just for
/// this; write a minimal key=value line if the key isn't already present.
fn merge_aider_yaml(existing: Option<&str>, key: &str, value: &str) -> String {
    let mut existing = existing.unwrap_or("").to_string();
    let needle = format!("\n{key}:");
    if existing.contains(&needle) || existing.starts_with(&format!("{key}:")) {
        return existing;
    }
    if !existing.ends_with('\n') && !existing.is_empty() {
        existing.push('\n');
    }
    existing.push_str(&format!("{key}: \"{value}\"\n"));
    existing
}

fn atomic_write(path: &Path, body: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("nidtmp");
    {
        let mut f = fs::File::create(&tmp)?;
        std::io::Write::write_all(&mut f, body.as_bytes())?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::{DetectedAgent, DetectedBackends, DetectionResult};
    use crate::onboard::{plan, OnboardOptions};
    use tempfile::TempDir;

    fn stub_detected(home: &Path) -> DetectionResult {
        DetectionResult {
            agents: AgentKind::all()
                .iter()
                .map(|k| DetectedAgent {
                    kind: *k,
                    config_path: k.default_config_path(home),
                    config_exists: false,
                })
                .collect(),
            backends: DetectedBackends::default(),
        }
    }

    #[test]
    fn apply_writes_every_agent_config_and_backup() {
        let tmp = TempDir::new().unwrap();
        let det = stub_detected(tmp.path());
        let opts = OnboardOptions {
            agents: Some(vec![
                AgentKind::ClaudeCode,
                AgentKind::Cursor,
                AgentKind::Aider,
            ]),
            ..Default::default()
        };
        let backup_path = tmp.path().join("backup.json");
        let p = plan(&det, &opts, backup_path.clone());
        let _ = apply(&p, "/opt/nid/bin/nid").unwrap();

        assert!(backup_path.exists());
        for c in &p.changes {
            assert!(c.config_path.exists(), "{:?} not written", c.config_path);
        }
        let cc = fs::read_to_string(AgentKind::ClaudeCode.default_config_path(tmp.path())).unwrap();
        assert!(cc.contains("PreToolUse"), "claude code hook missing: {cc}");
        let aider = fs::read_to_string(AgentKind::Aider.default_config_path(tmp.path())).unwrap();
        assert!(aider.contains("shell-command-prefix"));
    }

    #[test]
    fn apply_is_idempotent_on_second_run() {
        let tmp = TempDir::new().unwrap();
        let det = stub_detected(tmp.path());
        let opts = OnboardOptions {
            agents: Some(vec![AgentKind::ClaudeCode]),
            ..Default::default()
        };
        let p = plan(&det, &opts, tmp.path().join("backup.json"));
        apply(&p, "/opt/nid/bin/nid").unwrap();
        let after_first =
            fs::read_to_string(AgentKind::ClaudeCode.default_config_path(tmp.path())).unwrap();

        let det2 = stub_detected(tmp.path());
        let p2 = plan(&det2, &opts, tmp.path().join("backup.json"));
        apply(&p2, "/opt/nid/bin/nid").unwrap();
        let after_second =
            fs::read_to_string(AgentKind::ClaudeCode.default_config_path(tmp.path())).unwrap();

        // Second apply must not duplicate the hook entry.
        let count = after_second.matches("/opt/nid/bin/nid").count();
        assert_eq!(count, 1, "hook entry must not duplicate:\n{after_second}");
        assert_eq!(after_first.matches("/opt/nid/bin/nid").count(), 1);
    }

    #[test]
    fn merge_preserves_other_user_keys() {
        let existing = json!({ "someUserSetting": "keep-me" }).to_string();
        let out = merge_hook_into(AgentKind::ClaudeCode, Some(&existing), "/bin/nid").unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["someUserSetting"], "keep-me");
        assert!(v["hooks"]["PreToolUse"].is_array());
    }
}
