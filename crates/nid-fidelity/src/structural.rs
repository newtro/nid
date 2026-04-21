//! Tier 2 structural-subset check (plan §8.1).
//!
//! Every output line (excluding placeholders we recognize) must appear
//! somewhere in the raw input. Detects profiles "inventing content."

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuralResult {
    pub passed: bool,
    pub lines_in_raw: usize,
    pub lines_in_compressed: usize,
    pub invented_lines: Vec<String>,
}

/// Check that compressed lines are a structural subset of raw (after
/// whitespace trim). Placeholder lines starting with `[... ` or `[nid:` are
/// exempt — those are produced by the pipeline itself.
pub fn structural_subset_check(raw: &str, compressed: &str) -> StructuralResult {
    let raw_lines: HashSet<&str> = raw.lines().map(str::trim_end).collect();
    let compressed_lines: Vec<&str> = compressed.lines().map(str::trim_end).collect();
    let mut invented = Vec::new();
    for l in &compressed_lines {
        if is_placeholder(l) {
            continue;
        }
        if l.trim().is_empty() {
            continue;
        }
        if !raw_lines.contains(l) {
            invented.push((*l).to_string());
        }
    }
    StructuralResult {
        passed: invented.is_empty(),
        lines_in_raw: raw_lines.len(),
        lines_in_compressed: compressed_lines.len(),
        invented_lines: invented,
    }
}

fn is_placeholder(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("[... ")
        || t.starts_with("[nid:")
        || t.starts_with("[REDACTED:")
        || t.starts_with("--- [nid:")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pure_subset_passes() {
        let raw = "a\nb\nc\n";
        let cmp = "a\nc\n";
        let r = structural_subset_check(raw, cmp);
        assert!(r.passed);
    }

    #[test]
    fn invented_line_fails() {
        let raw = "a\nb\n";
        let cmp = "a\nTHIS_IS_NEW\n";
        let r = structural_subset_check(raw, cmp);
        assert!(!r.passed);
        assert_eq!(r.invented_lines, vec!["THIS_IS_NEW"]);
    }

    #[test]
    fn placeholder_lines_are_exempt() {
        let raw = "alpha\nbeta\n";
        let cmp = "alpha\n[... 42 elided ...]\n[nid: profile foo/v1.0.0]\nbeta\n";
        let r = structural_subset_check(raw, cmp);
        assert!(r.passed);
    }
}
