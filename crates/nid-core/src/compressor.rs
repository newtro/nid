//! The Compressor trait every layer implements.
//!
//! Hard rule (plan §6.3): output is a line-preserving subset or structure-preserving
//! filter of input. Never a rewrite. Only Layer 4 may rewrite, and must mark itself
//! `Mode::Degraded`.

use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::ops::Range;

use crate::context::Context;
use crate::session::SessionRef;

/// Whether a layer can handle the given input preview.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applicability {
    Applicable,
    Inapplicable,
    DegradedOnly,
}

/// What mode a compressor ran in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum CompressorMode {
    /// Profile/layer matched and applied clean.
    Full,
    /// Layer ran but had to degrade (e.g., Layer 4 rewrote content).
    Degraded,
    /// No compression; input passed through verbatim.
    Passthrough,
    /// Shadow mode: raw through to agent, counterfactual compressed captured.
    Shadow,
}

/// Which structured format (if any) the output is parseable as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FormatKind {
    Plain,
    Json,
    Ndjson,
    Diff,
    Log,
    Tabular,
    StackTrace,
}

/// A byte range dropped by a compressor, with a human-visible placeholder.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DroppedBlock {
    pub range: Range<usize>,
    pub placeholder: String,
}

/// Outcome of a single invariant check (plan §8.1 Tier 1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvariantResult {
    pub name: String,
    pub passed: bool,
    pub detail: Option<String>,
}

/// Result of a compression pass.
#[derive(Debug, Clone)]
pub struct CompressionResult {
    pub mode: CompressorMode,
    pub kept_ranges: Vec<Range<usize>>,
    pub dropped_blocks: Vec<DroppedBlock>,
    pub invariants: Vec<InvariantResult>,
    pub format_claim: Option<FormatKind>,
    pub self_fidelity: f32,
    pub raw_pointer: SessionRef,
    /// Bytes of compressed output actually written.
    pub bytes_written: usize,
    /// Bytes of raw input consumed.
    pub bytes_read: usize,
}

impl CompressionResult {
    pub fn passthrough(raw_pointer: SessionRef) -> Self {
        Self {
            mode: CompressorMode::Passthrough,
            kept_ranges: vec![],
            dropped_blocks: vec![],
            invariants: vec![],
            format_claim: None,
            self_fidelity: 1.0,
            raw_pointer,
            bytes_written: 0,
            bytes_read: 0,
        }
    }
}

/// Every compression layer implements this.
///
/// - `probe` inspects a preview chunk and says whether this layer should run.
/// - `compress` streams raw input → compressed output, producing a result.
///
/// Implementations must be cancellation-safe: a SIGTERM while `compress` is
/// running should leave partial compressed output valid on `output`.
pub trait Compressor: Send + Sync {
    fn name(&self) -> &str;

    fn probe(&self, preview: &[u8], ctx: &Context) -> Applicability;

    fn compress(
        &self,
        input: &mut dyn Read,
        output: &mut dyn Write,
        ctx: &Context,
    ) -> anyhow::Result<CompressionResult>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_result_is_full_fidelity() {
        let r = CompressionResult::passthrough(SessionRef::new("sess_test".into()));
        assert_eq!(r.mode, CompressorMode::Passthrough);
        assert_eq!(r.self_fidelity, 1.0);
    }

    #[test]
    fn compressor_mode_roundtrips_json() {
        for m in [
            CompressorMode::Full,
            CompressorMode::Degraded,
            CompressorMode::Passthrough,
            CompressorMode::Shadow,
        ] {
            let s = serde_json::to_string(&m).unwrap();
            let back: CompressorMode = serde_json::from_str(&s).unwrap();
            assert_eq!(m, back);
        }
    }
}
