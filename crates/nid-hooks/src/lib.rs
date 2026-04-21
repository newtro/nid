//! nid-hooks: per-agent PreTool hook writers + the hook handler itself.
//!
//! A hook handler is a small JSON-in/JSON-out routine that rewrites
//! `<cmd>` → `nid <cmd>` subject to the rules in plan §4.4:
//!
//! 1. Idempotent (`nid nid foo` collapses to `nid foo`).
//! 2. Whole-pipeline wrap (don't parse pipeline internals).
//! 3. Passthrough list (shell builtins, tee/cat-as-plumbing, user config).
//! 4. `NID_RAW=1` escape hatch.
//! 5. Hook-ordering metadata collision warnings.

pub mod agents;
pub mod detect;
pub mod onboard;
pub mod rewrite;

pub use agents::{Agent, AgentKind, AgentRegistry};
pub use detect::{detect_agents, DetectionResult};
pub use onboard::{OnboardBackup, OnboardOptions, OnboardPlan};
pub use rewrite::{rewrite_command, RewriteDecision};
