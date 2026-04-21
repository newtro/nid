//! nid's user-editable TOML config (plan §11.3).
//!
//! Loaded at startup from `<config_dir>/config.toml`. Missing file → all
//! defaults. Malformed file → warn + use defaults (we never fail startup on
//! user config).

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub security: SecurityConfig,
    #[serde(default)]
    pub session: SessionConfig,
    #[serde(default)]
    pub hook: HookConfig,
    #[serde(default)]
    pub synthesis: SynthesisConfig,
    #[serde(default)]
    pub fidelity: FidelityConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SecurityConfig {
    #[serde(default)]
    pub redaction: RedactionConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RedactionConfig {
    /// Extra user-supplied regex patterns to redact from raw output.
    #[serde(default)]
    pub extra_patterns: Vec<String>,
    /// Commands (matched against argv[0]) whose raw output is redacted
    /// aggressively — beyond the default heuristics.
    #[serde(default)]
    pub deny_commands: Vec<String>,
    /// Commands whose raw output skips redaction (opt-out).
    #[serde(default)]
    pub allow_commands: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    #[serde(default = "default_preserve_raw")]
    pub preserve_raw: bool,
    #[serde(default = "default_retention_days")]
    pub retention_days: u32,
    #[serde(default = "default_max_total_mb")]
    pub max_total_mb: u64,
    /// Commands whose raw output is never persisted.
    #[serde(default)]
    pub deny_raw_commands: Vec<String>,
    /// Commands whose raw is always persisted, even if globally denied.
    #[serde(default)]
    pub allow_raw_commands: Vec<String>,
}

fn default_preserve_raw() -> bool {
    true
}
fn default_retention_days() -> u32 {
    14
}
fn default_max_total_mb() -> u64 {
    2048
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            preserve_raw: default_preserve_raw(),
            retention_days: default_retention_days(),
            max_total_mb: default_max_total_mb(),
            deny_raw_commands: Vec::new(),
            allow_raw_commands: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HookConfig {
    /// Extra regex patterns that match commands to pass through (in addition
    /// to the built-in builtin + tee/cat skip list).
    #[serde(default)]
    pub passthrough_patterns: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SynthesisConfig {
    #[serde(default = "default_samples_to_lock")]
    pub samples_to_lock: usize,
    #[serde(default = "default_fast_path_zv")]
    pub fast_path_if_zero_variance: bool,
    #[serde(default = "default_daily_budget")]
    pub daily_budget_usd: f64,
    #[serde(default = "default_refinement_cooldown")]
    pub per_profile_refinement_cooldown_hours: u32,
}

fn default_samples_to_lock() -> usize {
    5
}
fn default_fast_path_zv() -> bool {
    true
}
fn default_daily_budget() -> f64 {
    0.50
}
fn default_refinement_cooldown() -> u32 {
    24
}

impl Default for SynthesisConfig {
    fn default() -> Self {
        Self {
            samples_to_lock: default_samples_to_lock(),
            fast_path_if_zero_variance: default_fast_path_zv(),
            daily_budget_usd: default_daily_budget(),
            per_profile_refinement_cooldown_hours: default_refinement_cooldown(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FidelityConfig {
    #[serde(default = "default_bypass_threshold")]
    pub bypass_threshold: f32,
    #[serde(default = "default_bypass_warmup")]
    pub bypass_warmup_runs: usize,
    #[serde(default = "default_judge_sample_rate")]
    pub judge_sample_rate: f64,
}

fn default_bypass_threshold() -> f32 {
    0.3
}
fn default_bypass_warmup() -> usize {
    3
}
fn default_judge_sample_rate() -> f64 {
    0.01
}

impl Default for FidelityConfig {
    fn default() -> Self {
        Self {
            bypass_threshold: default_bypass_threshold(),
            bypass_warmup_runs: default_bypass_warmup(),
            judge_sample_rate: default_judge_sample_rate(),
        }
    }
}

/// Load config from `<config_dir>/config.toml`. Missing → defaults.
/// Malformed → log a warning, return defaults. Never panics.
pub fn load(config_dir: &Path) -> Config {
    let path = config_dir.join("config.toml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Config::default();
    };
    match toml::from_str::<Config>(&text) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "config.toml failed to parse; using defaults"
            );
            Config::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn missing_file_returns_defaults() {
        let t = TempDir::new().unwrap();
        let c = load(t.path());
        assert_eq!(c.session.retention_days, 14);
        assert!(c.session.preserve_raw);
        assert_eq!(c.synthesis.samples_to_lock, 5);
    }

    #[test]
    fn partial_config_merges_with_defaults() {
        let t = TempDir::new().unwrap();
        std::fs::write(
            t.path().join("config.toml"),
            r#"
[session]
retention_days = 30

[hook]
passthrough_patterns = ["^mytool\\b"]
"#,
        )
        .unwrap();
        let c = load(t.path());
        assert_eq!(c.session.retention_days, 30);
        // Other session fields stay default.
        assert_eq!(c.session.max_total_mb, 2048);
        assert!(c.session.preserve_raw);
        assert_eq!(c.hook.passthrough_patterns.len(), 1);
        assert_eq!(c.synthesis.samples_to_lock, 5);
    }

    #[test]
    fn malformed_file_falls_back() {
        let t = TempDir::new().unwrap();
        std::fs::write(t.path().join("config.toml"), "this is not valid toml ][").unwrap();
        let c = load(t.path());
        assert_eq!(c.session.retention_days, 14);
    }

    #[test]
    fn extra_redaction_patterns_survive() {
        let t = TempDir::new().unwrap();
        std::fs::write(
            t.path().join("config.toml"),
            r#"
[security.redaction]
extra_patterns = ["SECRET-[A-Z0-9]{8}"]
deny_commands = ["env"]
"#,
        )
        .unwrap();
        let c = load(t.path());
        assert_eq!(
            c.security.redaction.extra_patterns,
            vec!["SECRET-[A-Z0-9]{8}".to_string()]
        );
        assert_eq!(c.security.redaction.deny_commands, vec!["env".to_string()]);
    }
}
