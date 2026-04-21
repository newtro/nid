//! Invocation context carried through the pipeline.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::SystemTime;

/// Runtime context for a single nid invocation.
#[derive(Debug, Clone)]
pub struct Context {
    /// Fingerprint (Scheme R) of the argv we're running.
    pub fingerprint: String,
    /// Original argv as the agent sent it.
    pub argv: Vec<String>,
    /// Working directory at invocation.
    pub cwd: Option<PathBuf>,
    /// Parent agent identity (claude_code, cursor, codex, ...).
    pub parent_agent: Option<String>,
    /// Wall-clock start of invocation.
    pub started_at: SystemTime,
    /// True when running under `nid --shadow`.
    pub shadow: bool,
    /// Extra data layers may stash.
    pub attrs: HashMap<String, String>,
}

impl Context {
    pub fn new(fingerprint: impl Into<String>, argv: Vec<String>) -> Self {
        Self {
            fingerprint: fingerprint.into(),
            argv,
            cwd: std::env::current_dir().ok(),
            parent_agent: std::env::var("NID_PARENT_AGENT").ok(),
            started_at: SystemTime::now(),
            shadow: false,
            attrs: HashMap::new(),
        }
    }

    pub fn with_shadow(mut self, shadow: bool) -> Self {
        self.shadow = shadow;
        self
    }
}
