//! `nid shadow {enable, disable, commit}` — trust-ramp rollout (plan §14).
//!
//! The implementation model:
//! - A tiny state file at `<config>/shadow.state` holds one of: `enable` /
//!   `disable` / `commit`.
//! - The hook handler reads it on each invocation and sets the `shadow`
//!   bit on the rewrite accordingly.
//!
//! `enable` → every wrapped command runs as `nid --shadow …` (raw
//! passthrough but counterfactual compressed captured).
//! `commit` → flip OFF shadow so compressed output is returned.
//! `disable` → also off; semantic difference is user-facing messaging.

use anyhow::{Context, Result};
use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub enum ShadowCmd {
    Enable,
    Disable,
    Commit,
    Status,
}

pub async fn run(sub: ShadowCmd) -> Result<()> {
    let paths = crate::cmd::paths::resolve()?;
    paths.ensure()?;
    let state_path = paths.config_dir.join("shadow.state");

    match sub {
        ShadowCmd::Enable => {
            std::fs::write(&state_path, "enable").context("writing shadow.state")?;
            println!("shadow mode: ENABLED");
            println!(
                "all wrapped commands now pass raw output through and capture a counterfactual."
            );
            println!("use `nid gain --shadow` to see projected savings; `nid shadow commit` to switch on compression.");
            Ok(())
        }
        ShadowCmd::Disable => {
            if state_path.exists() {
                std::fs::remove_file(&state_path).ok();
            }
            println!("shadow mode: DISABLED (no counterfactual captured)");
            Ok(())
        }
        ShadowCmd::Commit => {
            if state_path.exists() {
                std::fs::remove_file(&state_path).ok();
            }
            println!("shadow mode: COMMITTED — wrapped commands now return compressed output.");
            Ok(())
        }
        ShadowCmd::Status => {
            let s = std::fs::read_to_string(&state_path).unwrap_or_default();
            let status = match s.trim() {
                "enable" => "ENABLED (capturing counterfactuals)",
                _ => "OFF (commit/passthrough)",
            };
            println!("shadow: {status}");
            Ok(())
        }
    }
}

/// Read the on-disk shadow flag. Called by run.rs on every hot-path invocation
/// to decide whether to force shadow on.
pub fn is_shadow_enabled(config_dir: &std::path::Path) -> bool {
    let path = config_dir.join("shadow.state");
    std::fs::read_to_string(path)
        .map(|s| s.trim() == "enable")
        .unwrap_or(false)
}
