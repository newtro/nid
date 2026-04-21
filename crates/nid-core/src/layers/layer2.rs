//! Layer 2 — format auto-detect (plan §6.1).
//!
//! Inspects a preview of the raw output and decides whether it's:
//! - JSON (single document)
//! - NDJSON (line-delimited JSON)
//! - Unified diff
//! - Stack trace
//! - Tabular
//! - Log
//! - Plain
//!
//! Layer 2 dispatch is "run a format-aware cleanup" without a profile; the
//! full per-format strategies are in nid-dsl bundled profiles. What Layer 2
//! provides here is the classifier + a small set of format-specific drops.

use crate::compressor::{
    Applicability, Compressor, CompressionResult, CompressorMode, FormatKind,
};
use crate::context::Context;
use crate::session::SessionRef;
use regex::Regex;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::OnceLock;

/// Peek at a preview of raw output and classify it.
pub fn detect_format(preview: &[u8]) -> FormatKind {
    let s = std::str::from_utf8(preview).unwrap_or("");
    let trimmed = s.trim_start();

    // Single JSON document.
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        // Try parsing just the preview; if it parses as a value, call it JSON.
        if serde_json::from_str::<serde_json::Value>(trimmed).is_ok() {
            return FormatKind::Json;
        }
    }

    // NDJSON: each line is its own JSON object.
    let ndjson_candidate = s
        .lines()
        .take(5)
        .all(|l| l.trim().is_empty() || serde_json::from_str::<serde_json::Value>(l).is_ok());
    if ndjson_candidate && s.lines().filter(|l| !l.trim().is_empty()).count() >= 2 {
        return FormatKind::Ndjson;
    }

    // Unified diff.
    if s.lines().any(|l| l.starts_with("@@ ")) && s.lines().any(|l| l.starts_with("diff --git") || l.starts_with("--- ") || l.starts_with("+++ ")) {
        return FormatKind::Diff;
    }

    // Stack trace (Python/Rust/Java flavors).
    if stack_trace_re().is_match(s) {
        return FormatKind::StackTrace;
    }

    // Tabular: multiple lines share whitespace-delimited column counts.
    if is_tabular(s) {
        return FormatKind::Tabular;
    }

    // Log — has leading timestamps or common level tags.
    if log_re().is_match(s) {
        return FormatKind::Log;
    }

    FormatKind::Plain
}

fn stack_trace_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?m)(^\s*Traceback \(most recent call last\):|^\s*at [\w.]+\(|\bthread 'main' panicked at)",
        )
        .unwrap()
    })
}

fn log_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"(?m)^(\d{4}-\d{2}-\d{2}[ T]\d{2}:\d{2}:\d{2}|\[\d{2}:\d{2}:\d{2}\]|\w{3} \d{1,2} \d{2}:\d{2}:\d{2}|(?i)^(debug|info|warn|error|fatal|trace)[: ])").unwrap()
    })
}

fn is_tabular(s: &str) -> bool {
    let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() < 3 {
        return false;
    }
    let col_counts: Vec<usize> = lines
        .iter()
        .take(5)
        .map(|l| l.split_whitespace().count())
        .collect();
    if col_counts.is_empty() {
        return false;
    }
    let first = col_counts[0];
    if first < 2 {
        return false;
    }
    col_counts.iter().all(|c| *c == first)
}

/// A lightweight Layer 2 that, given a detected format, drops noise and keeps
/// signal. Used when no Layer 3/5 profile matches.
pub struct Layer2Format {
    pub format: FormatKind,
}

impl Compressor for Layer2Format {
    fn name(&self) -> &str {
        "layer2-format"
    }

    fn probe(&self, preview: &[u8], _ctx: &Context) -> Applicability {
        if detect_format(preview) == self.format {
            Applicability::Applicable
        } else {
            Applicability::Inapplicable
        }
    }

    fn compress(
        &self,
        input: &mut dyn Read,
        output: &mut dyn Write,
        _ctx: &Context,
    ) -> anyhow::Result<CompressionResult> {
        // For v1 this is mostly a pass-through per format: we keep stack
        // traces verbatim (they're already compact signal), drop DEBUG lines
        // from log output, and pass through diff / tabular / json as-is.
        // Full per-format strategies live in bundled profiles.
        let reader = BufReader::new(input);
        let mut bytes_in = 0usize;
        let mut bytes_out = 0usize;
        for line in reader.lines() {
            let line = line?;
            bytes_in += line.len() + 1;
            if matches!(self.format, FormatKind::Log) {
                // Drop DEBUG/TRACE lines.
                if let Some(re) = debug_re() {
                    if re.is_match(&line) {
                        continue;
                    }
                }
            }
            writeln!(output, "{line}")?;
            bytes_out += line.len() + 1;
        }
        Ok(CompressionResult {
            mode: CompressorMode::Full,
            kept_ranges: vec![],
            dropped_blocks: vec![],
            invariants: vec![],
            format_claim: Some(self.format),
            self_fidelity: 1.0,
            raw_pointer: SessionRef::new("".into()),
            bytes_written: bytes_out,
            bytes_read: bytes_in,
        })
    }
}

fn debug_re() -> Option<&'static Regex> {
    static R: OnceLock<Regex> = OnceLock::new();
    let re = R.get_or_init(|| Regex::new(r"(?i)\b(debug|trace)\b").unwrap());
    Some(re)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_single_json() {
        let s = b"{\"foo\": 1}";
        assert_eq!(detect_format(s), FormatKind::Json);
    }

    #[test]
    fn detects_ndjson() {
        let s = b"{\"a\":1}\n{\"a\":2}\n{\"a\":3}\n";
        assert_eq!(detect_format(s), FormatKind::Ndjson);
    }

    #[test]
    fn detects_diff() {
        let s = b"diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1,1 +1,1 @@\n-old\n+new\n";
        assert_eq!(detect_format(s), FormatKind::Diff);
    }

    #[test]
    fn detects_stack_trace() {
        let s = b"Traceback (most recent call last):\n  File \"foo.py\", line 1, in <module>\n";
        assert_eq!(detect_format(s), FormatKind::StackTrace);
    }

    #[test]
    fn detects_log() {
        let s = b"2025-01-01 12:00:00 INFO started\n2025-01-01 12:00:01 INFO ready\n";
        assert_eq!(detect_format(s), FormatKind::Log);
    }

    #[test]
    fn detects_plain_text() {
        let s = b"hello\nworld\n";
        assert_eq!(detect_format(s), FormatKind::Plain);
    }
}
