//! Grammar validator (plan §11.4 Forbidden list).
//!
//! Runs at three points: synthesis output, profile import, update migration.
//! Validator is the single chokepoint that enforces:
//! - Regexes parseable by Rust `regex` (rejects backreferences natively).
//! - No unbounded recursion in state machines.
//! - Schema version matches what this interpreter supports.
//!
//! The DSL grammar itself has no IO/exec primitives, so those are enforced by
//! the AST — there is literally no way to express them. The validator confirms
//! what the grammar already guarantees.

use crate::ast::{Invariant, InvariantCheck, Profile, Rule, RuleKind};
use regex::Regex;
use std::collections::HashSet;
use thiserror::Error;

pub const SUPPORTED_SCHEMA: &str = "1.0";

#[derive(Debug, Error)]
pub enum ValidationError {
    #[error("unsupported schema version: {0} (supported: {SUPPORTED_SCHEMA})")]
    Schema(String),
    #[error("invalid regex in rule {index} ({kind}): {source}")]
    Regex {
        index: usize,
        kind: &'static str,
        source: regex::Error,
    },
    #[error("invalid regex in invariant {name}: {source}")]
    InvariantRegex { name: String, source: regex::Error },
    #[error("regex contains forbidden construct: {0}")]
    ForbiddenConstruct(String),
    #[error("state_machine has empty states list")]
    EmptyStateMachine,
    #[error("state_machine state name `{0}` appears more than once")]
    DuplicateState(String),
    #[error("head/tail n must be > 0")]
    HeadTailZero,
    #[error("truncate_to bytes must be > 0")]
    TruncateZero,
    #[error("collapse_repeated min must be >= 2")]
    CollapseMinTooSmall,
    #[error("invariant name `{0}` appears more than once")]
    DuplicateInvariant(String),
    #[error("empty fingerprint")]
    EmptyFingerprint,
    #[error("empty version")]
    EmptyVersion,
}

/// Validate a Profile against the DSL rules. Returns `Ok(())` if it's
/// well-formed and safe to execute.
pub fn validate_profile(p: &Profile) -> Result<(), ValidationError> {
    if p.meta.schema != SUPPORTED_SCHEMA {
        return Err(ValidationError::Schema(p.meta.schema.clone()));
    }
    if p.meta.fingerprint.trim().is_empty() {
        return Err(ValidationError::EmptyFingerprint);
    }
    if p.meta.version.trim().is_empty() {
        return Err(ValidationError::EmptyVersion);
    }

    for (i, rule) in p.rules.iter().enumerate() {
        validate_rule(i, rule)?;
    }

    let mut names = HashSet::new();
    for inv in &p.invariants {
        if !names.insert(inv.name.clone()) {
            return Err(ValidationError::DuplicateInvariant(inv.name.clone()));
        }
        validate_invariant(inv)?;
    }

    Ok(())
}

fn validate_rule(index: usize, rule: &Rule) -> Result<(), ValidationError> {
    match &rule.kind {
        RuleKind::KeepLines { match_ } | RuleKind::DropLines { match_ } => {
            check_regex(index, "keep/drop_lines", match_)
        }
        RuleKind::CollapseRepeated { pattern, min, .. } => {
            if *min < 2 {
                return Err(ValidationError::CollapseMinTooSmall);
            }
            check_regex(index, "collapse_repeated", pattern)
        }
        RuleKind::CollapseBetween { begin, end, .. } => {
            check_regex(index, "collapse_between.begin", begin)?;
            check_regex(index, "collapse_between.end", end)
        }
        RuleKind::Head { n } | RuleKind::Tail { n } => {
            if *n == 0 {
                Err(ValidationError::HeadTailZero)
            } else {
                Ok(())
            }
        }
        RuleKind::HeadAfter { n, after_match } => {
            if *n == 0 {
                return Err(ValidationError::HeadTailZero);
            }
            check_regex(index, "head_after", after_match)
        }
        RuleKind::TailBefore { n, before_match } => {
            if *n == 0 {
                return Err(ValidationError::HeadTailZero);
            }
            check_regex(index, "tail_before", before_match)
        }
        RuleKind::Dedup | RuleKind::StripAnsi => Ok(()),
        RuleKind::JsonPathKeep { paths } | RuleKind::JsonPathDrop { paths } => {
            for p in paths {
                check_json_path(p)?;
            }
            Ok(())
        }
        RuleKind::NdjsonFilter { field, keep_values } => {
            if field.trim().is_empty() {
                return Err(ValidationError::ForbiddenConstruct(
                    "ndjson_filter.field empty".into(),
                ));
            }
            if keep_values.is_empty() {
                return Err(ValidationError::ForbiddenConstruct(
                    "ndjson_filter.keep_values empty".into(),
                ));
            }
            Ok(())
        }
        RuleKind::StateMachine { states } => {
            if states.is_empty() {
                return Err(ValidationError::EmptyStateMachine);
            }
            let mut seen = HashSet::new();
            for s in states {
                if !seen.insert(s.name.clone()) {
                    return Err(ValidationError::DuplicateState(s.name.clone()));
                }
                check_regex(index, "state_machine.enter", &s.enter)?;
                for k in &s.keep {
                    check_regex(index, "state_machine.keep", k)?;
                }
                for d in &s.drop {
                    check_regex(index, "state_machine.drop", d)?;
                }
            }
            Ok(())
        }
        RuleKind::TruncateTo { bytes } => {
            if *bytes == 0 {
                Err(ValidationError::TruncateZero)
            } else {
                Ok(())
            }
        }
    }
}

fn validate_invariant(inv: &Invariant) -> Result<(), ValidationError> {
    match &inv.check {
        InvariantCheck::LastLineMatches { pattern }
        | InvariantCheck::FirstLineMatches { pattern }
        | InvariantCheck::AllMatchingPreserved { pattern }
        | InvariantCheck::CountMatchesAtLeast { pattern, .. } => {
            Regex::new(pattern).map_err(|source| ValidationError::InvariantRegex {
                name: inv.name.clone(),
                source,
            })?;
            Ok(())
        }
        InvariantCheck::JsonPathExists { path } => check_json_path(path),
        InvariantCheck::ExitLinePreserved => Ok(()),
    }
}

/// The Rust `regex` crate already rejects backreferences and look-around, so
/// `Regex::new` serves as our forbidden-construct filter. We still run a
/// lightweight pre-check to give a clearer error message on obvious abuses.
fn check_regex(index: usize, kind: &'static str, src: &str) -> Result<(), ValidationError> {
    // Obvious abuses with clearer errors.
    // - Backreferences like \1..\9 (Rust regex already refuses them, but a
    //   clearer message helps users debugging imported profiles).
    if contains_backref(src) {
        return Err(ValidationError::ForbiddenConstruct(format!(
            "regex backreferences are forbidden (rule #{index}, {kind})"
        )));
    }
    Regex::new(src).map_err(|source| ValidationError::Regex {
        index,
        kind,
        source,
    })?;
    Ok(())
}

fn contains_backref(s: &str) -> bool {
    // Scan for `\1`..`\9` outside a character class. Very rough — `Regex::new`
    // is the real arbiter; this is just for friendlier errors.
    let bytes = s.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'\\' {
            let c = bytes[i + 1];
            if c.is_ascii_digit() && c != b'0' {
                return true;
            }
            i += 2;
            continue;
        }
        i += 1;
    }
    false
}

/// Very small JSONPath subset: `$`, dotted keys, bracketed indices.
fn check_json_path(p: &str) -> Result<(), ValidationError> {
    if !p.starts_with('$') {
        return Err(ValidationError::ForbiddenConstruct(format!(
            "json path must start with `$`: {p}"
        )));
    }
    // Reject wildcards and filter expressions — we only support simple
    // `$.a.b[0].c` walks.
    for bad in ["*", "?(", "..", "@"] {
        if p.contains(bad) {
            return Err(ValidationError::ForbiddenConstruct(format!(
                "json path uses unsupported construct `{bad}`: {p}"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{FormatClaim, Invariant, Meta, Profile, Rule, RuleKind, StateDef};

    fn base(rules: Vec<Rule>, invariants: Vec<Invariant>) -> Profile {
        Profile {
            meta: Meta {
                fingerprint: "test".into(),
                version: "1.0.0".into(),
                schema: SUPPORTED_SCHEMA.into(),
                format_claim: Some(FormatClaim::Plain),
                description: None,
            },
            rules,
            invariants,
            self_tests: vec![],
        }
    }

    #[test]
    fn valid_minimal_profile() {
        let p = base(
            vec![Rule {
                kind: RuleKind::Dedup,
            }],
            vec![],
        );
        validate_profile(&p).unwrap();
    }

    #[test]
    fn rejects_bad_schema() {
        let mut p = base(vec![], vec![]);
        p.meta.schema = "2.0".into();
        assert!(matches!(
            validate_profile(&p),
            Err(ValidationError::Schema(_))
        ));
    }

    #[test]
    fn rejects_backreferences() {
        let p = base(
            vec![Rule {
                kind: RuleKind::KeepLines {
                    match_: r"(\w+)\1".into(),
                },
            }],
            vec![],
        );
        let err = validate_profile(&p).unwrap_err();
        assert!(matches!(err, ValidationError::ForbiddenConstruct(_)));
    }

    #[test]
    fn rejects_bad_regex() {
        let p = base(
            vec![Rule {
                kind: RuleKind::KeepLines {
                    match_: "(unclosed".into(),
                },
            }],
            vec![],
        );
        assert!(matches!(
            validate_profile(&p),
            Err(ValidationError::Regex { .. })
        ));
    }

    #[test]
    fn rejects_zero_head() {
        let p = base(
            vec![Rule {
                kind: RuleKind::Head { n: 0 },
            }],
            vec![],
        );
        assert!(matches!(
            validate_profile(&p),
            Err(ValidationError::HeadTailZero)
        ));
    }

    #[test]
    fn rejects_bad_collapse_min() {
        let p = base(
            vec![Rule {
                kind: RuleKind::CollapseRepeated {
                    pattern: "x".into(),
                    placeholder: "[...]".into(),
                    min: 1,
                },
            }],
            vec![],
        );
        assert!(matches!(
            validate_profile(&p),
            Err(ValidationError::CollapseMinTooSmall)
        ));
    }

    #[test]
    fn rejects_duplicate_state() {
        let p = base(
            vec![Rule {
                kind: RuleKind::StateMachine {
                    states: vec![
                        StateDef {
                            name: "a".into(),
                            enter: "^A".into(),
                            keep: vec![],
                            drop: vec![],
                        },
                        StateDef {
                            name: "a".into(),
                            enter: "^B".into(),
                            keep: vec![],
                            drop: vec![],
                        },
                    ],
                },
            }],
            vec![],
        );
        assert!(matches!(
            validate_profile(&p),
            Err(ValidationError::DuplicateState(_))
        ));
    }

    #[test]
    fn rejects_empty_fingerprint() {
        let mut p = base(vec![], vec![]);
        p.meta.fingerprint = "".into();
        assert!(matches!(
            validate_profile(&p),
            Err(ValidationError::EmptyFingerprint)
        ));
    }

    #[test]
    fn rejects_json_path_wildcard() {
        let p = base(
            vec![Rule {
                kind: RuleKind::JsonPathKeep {
                    paths: vec!["$.foo[*].bar".into()],
                },
            }],
            vec![],
        );
        assert!(matches!(
            validate_profile(&p),
            Err(ValidationError::ForbiddenConstruct(_))
        ));
    }

    #[test]
    fn rejects_duplicate_invariant_names() {
        let p = base(
            vec![],
            vec![
                Invariant {
                    name: "Dup".into(),
                    check: InvariantCheck::ExitLinePreserved,
                },
                Invariant {
                    name: "Dup".into(),
                    check: InvariantCheck::ExitLinePreserved,
                },
            ],
        );
        assert!(matches!(
            validate_profile(&p),
            Err(ValidationError::DuplicateInvariant(_))
        ));
    }
}
