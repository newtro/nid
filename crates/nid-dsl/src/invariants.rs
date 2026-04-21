//! Invariant checks — Tier 1 fidelity (plan §8.1).
//!
//! Runs against raw + compressed after compression. Each check is cheap regex
//! or JSON-path work; zero allocations beyond the compiled regex cache.

use crate::ast::{Invariant, InvariantCheck};
use nid_core::compressor::InvariantResult;
use regex::Regex;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum InvariantCheckError {
    #[error("bad regex in invariant `{name}`: {source}")]
    Regex { name: String, source: regex::Error },
}

/// Run all invariants; each one returns an InvariantResult describing pass/fail
/// and an optional detail string.
pub fn check_invariants(
    invariants: &[Invariant],
    raw: &str,
    compressed: &str,
) -> Result<Vec<InvariantResult>, InvariantCheckError> {
    let mut out = Vec::with_capacity(invariants.len());
    for inv in invariants {
        out.push(run_one(inv, raw, compressed)?);
    }
    Ok(out)
}

fn run_one(inv: &Invariant, raw: &str, compressed: &str) -> Result<InvariantResult, InvariantCheckError> {
    let (passed, detail) = match &inv.check {
        InvariantCheck::LastLineMatches { pattern } => {
            let re = mk(&inv.name, pattern)?;
            let last = compressed.lines().last().unwrap_or("");
            (re.is_match(last), None)
        }
        InvariantCheck::FirstLineMatches { pattern } => {
            let re = mk(&inv.name, pattern)?;
            let first = compressed.lines().next().unwrap_or("");
            (re.is_match(first), None)
        }
        InvariantCheck::AllMatchingPreserved { pattern } => {
            let re = mk(&inv.name, pattern)?;
            let raw_matches: Vec<&str> = raw.lines().filter(|l| re.is_match(l)).collect();
            let cmp_matches: Vec<&str> =
                compressed.lines().filter(|l| re.is_match(l)).collect();
            let missing: Vec<&&str> = raw_matches
                .iter()
                .filter(|l| !cmp_matches.iter().any(|c| c == *l))
                .collect();
            let passed = missing.is_empty();
            let detail = if !passed {
                Some(format!("{} matching lines missing from compressed", missing.len()))
            } else {
                None
            };
            (passed, detail)
        }
        InvariantCheck::CountMatchesAtLeast { pattern, count } => {
            let re = mk(&inv.name, pattern)?;
            let got = compressed.lines().filter(|l| re.is_match(l)).count();
            let passed = got >= *count;
            (
                passed,
                (!passed).then(|| format!("expected >= {count}, got {got}")),
            )
        }
        InvariantCheck::JsonPathExists { path } => {
            let ok = matches!(serde_json::from_str::<serde_json::Value>(compressed), Ok(_))
                && json_exists(compressed, path);
            (ok, None)
        }
        InvariantCheck::ExitLinePreserved => {
            // Look for either an explicit `exit: N` line or a "Process exited"
            // in raw; if present, it must also be in compressed.
            let pat = Regex::new(r"(?i)^(exit[: ]|process exited|command failed|error code)").unwrap();
            let raw_has = raw.lines().any(|l| pat.is_match(l));
            if !raw_has {
                (true, None)
            } else {
                let cmp_has = compressed.lines().any(|l| pat.is_match(l));
                (cmp_has, (!cmp_has).then(|| "exit line missing".to_string()))
            }
        }
    };

    Ok(InvariantResult {
        name: inv.name.clone(),
        passed,
        detail,
    })
}

fn mk(name: &str, pat: &str) -> Result<Regex, InvariantCheckError> {
    Regex::new(pat).map_err(|source| InvariantCheckError::Regex {
        name: name.to_string(),
        source,
    })
}

fn json_exists(doc: &str, path: &str) -> bool {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(doc) else {
        return false;
    };
    let mut cur = &v;
    let rest = path.strip_prefix('$').unwrap_or(path);
    for seg in rest.split('.').filter(|s| !s.is_empty()) {
        match cur.get(seg) {
            Some(n) => cur = n,
            None => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_line_match_passes() {
        let inv = Invariant {
            name: "t".into(),
            check: InvariantCheck::LastLineMatches {
                pattern: "^done$".into(),
            },
        };
        let r = check_invariants(&[inv], "ignored raw", "a\nb\ndone").unwrap();
        assert!(r[0].passed);
    }

    #[test]
    fn first_line_match_fails_clean() {
        let inv = Invariant {
            name: "t".into(),
            check: InvariantCheck::FirstLineMatches {
                pattern: "^start$".into(),
            },
        };
        let r = check_invariants(&[inv], "", "different\nstart").unwrap();
        assert!(!r[0].passed);
    }

    #[test]
    fn all_matching_preserved_detects_dropped_error_line() {
        let raw = "info: ok\nerror: boom\ninfo: done\n";
        let cmp = "info: ok\ninfo: done\n";
        let inv = Invariant {
            name: "ErrorLinesVerbatim".into(),
            check: InvariantCheck::AllMatchingPreserved {
                pattern: "(?i)^error".into(),
            },
        };
        let r = check_invariants(&[inv], raw, cmp).unwrap();
        assert!(!r[0].passed);
    }

    #[test]
    fn count_matches_at_least_passes() {
        let inv = Invariant {
            name: "t".into(),
            check: InvariantCheck::CountMatchesAtLeast {
                pattern: "^x".into(),
                count: 2,
            },
        };
        let r = check_invariants(&[inv], "", "x1\nx2\ny").unwrap();
        assert!(r[0].passed);
    }
}
