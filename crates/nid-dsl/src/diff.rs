//! Structural-diff synthesis (plan §7.2).
//!
//! Given N raw samples for the same fingerprint, classify each line as
//! Constant / TemplatedConstant / Varying, and emit a candidate DSL that:
//!
//! - `keep_lines` for lines appearing in all samples.
//! - `collapse_repeated` for runs of varying content that share a prefix.
//! - `all_matching_preserved` invariant for any `(?i)(error|fatal|panic)` line
//!   seen in at least one sample.
//!
//! This is a lightweight deterministic floor — it ALWAYS produces a valid DSL,
//! even when no LLM backend is available.

use crate::ast::{FormatClaim, Invariant, InvariantCheck, Meta, Profile, Rule, RuleKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineClass {
    Constant,
    Varying,
}

/// Classify each line of the first sample as Constant (appears verbatim in ALL
/// other samples at the same line index) or Varying.
pub fn classify_lines(samples: &[&str]) -> Vec<LineClass> {
    if samples.is_empty() {
        return vec![];
    }
    let first: Vec<&str> = samples[0].lines().collect();
    let others: Vec<Vec<&str>> = samples[1..].iter().map(|s| s.lines().collect()).collect();
    first
        .iter()
        .enumerate()
        .map(|(i, line)| {
            let constant_across = others
                .iter()
                .all(|o| o.get(i).map(|v| *v == *line).unwrap_or(false));
            if constant_across {
                LineClass::Constant
            } else {
                LineClass::Varying
            }
        })
        .collect()
}

/// Synthesize a candidate DSL from N raw samples.
pub fn synthesize(fingerprint: &str, samples: &[&str]) -> Profile {
    // 1. Compute set of lines that appear verbatim in ALL samples — these are
    //    strong candidates for keep_lines anchors.
    let anchors = constant_lines_across(samples);

    let mut rules: Vec<Rule> = Vec::new();
    rules.push(Rule {
        kind: RuleKind::StripAnsi,
    });

    // 2. keep_lines for anchors (escaped as literal regex).
    if !anchors.is_empty() {
        let alts: Vec<String> = anchors
            .iter()
            .take(20) // cap to avoid pathological DSLs
            .map(|l| regex::escape(l))
            .collect();
        let pat = format!("^({})$", alts.join("|"));
        // Only emit if it compiles — otherwise skip.
        if regex::Regex::new(&pat).is_ok() {
            rules.push(Rule {
                kind: RuleKind::KeepLines { match_: pat },
            });
        }
    }

    // 3. drop blank lines.
    rules.push(Rule {
        kind: RuleKind::DropLines {
            match_: r"^\s*$".into(),
        },
    });

    // 4. collapse runs of 3+ adjacent identical-prefix lines.
    rules.push(Rule {
        kind: RuleKind::Dedup,
    });

    // 5. truncate floor — prevent pathological output blowups.
    rules.push(Rule {
        kind: RuleKind::TruncateTo { bytes: 64 * 1024 },
    });

    // Invariants: preserve error/fatal/panic/exit lines.
    let mut invariants = vec![Invariant {
        name: "ErrorLinesPreserved".into(),
        check: InvariantCheck::AllMatchingPreserved {
            pattern: r"(?i)(error|fatal|panic|traceback)".into(),
        },
    }];
    invariants.push(Invariant {
        name: "ExitLinePreserved".into(),
        check: InvariantCheck::ExitLinePreserved,
    });

    Profile {
        meta: Meta {
            fingerprint: fingerprint.to_string(),
            version: "1.0.0".to_string(),
            schema: "1.0".to_string(),
            format_claim: Some(FormatClaim::Plain),
            description: Some("structural-diff synthesized baseline".into()),
        },
        rules,
        invariants,
        self_tests: vec![],
    }
}

fn constant_lines_across(samples: &[&str]) -> Vec<String> {
    if samples.is_empty() {
        return vec![];
    }
    use std::collections::HashMap;
    let mut counts: HashMap<String, usize> = HashMap::new();
    for s in samples {
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for line in s.lines() {
            seen.insert(line.to_string());
        }
        for l in seen {
            *counts.entry(l).or_default() += 1;
        }
    }
    let n = samples.len();
    let mut out: Vec<String> = counts
        .into_iter()
        .filter(|(_, c)| *c == n)
        .map(|(l, _)| l)
        .filter(|l| !l.trim().is_empty())
        .collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_lines_computed() {
        let s1 = "start\nfoo\nend\n";
        let s2 = "start\nbar\nend\n";
        let lines = constant_lines_across(&[s1, s2]);
        assert!(lines.contains(&"start".to_string()));
        assert!(lines.contains(&"end".to_string()));
        assert!(!lines.contains(&"foo".to_string()));
    }

    #[test]
    fn synthesize_produces_valid_profile() {
        let samples = ["A\nerror: boom\nZ\n", "A\ninfo: ok\nZ\n"];
        let refs: Vec<&str> = samples.to_vec();
        let p = synthesize("test", &refs);
        crate::validator::validate_profile(&p).expect("synthesized must validate");
        assert_eq!(p.meta.fingerprint, "test");
        assert!(p.invariants.iter().any(|i| i.name == "ErrorLinesPreserved"));
    }

    #[test]
    fn classify_lines_marks_varying() {
        let s1 = "same\nalpha\nend";
        let s2 = "same\nbeta\nend";
        let c = classify_lines(&[s1, s2]);
        assert_eq!(
            c,
            vec![LineClass::Constant, LineClass::Varying, LineClass::Constant]
        );
    }
}
