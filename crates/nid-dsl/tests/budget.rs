//! End-to-end tests for the DSL execution budget (plan §11.4).

use nid_dsl::{
    apply_rules_with_budget,
    ast::{Profile, Rule, RuleKind},
    Budget,
};

fn profile_dedup_only() -> Profile {
    Profile {
        meta: nid_dsl::ast::Meta {
            fingerprint: "budget-test".into(),
            version: "1.0.0".into(),
            schema: "1.0".into(),
            format_claim: None,
            description: None,
        },
        rules: vec![Rule {
            kind: RuleKind::Dedup,
        }],
        invariants: vec![],
        self_tests: vec![],
    }
}

#[test]
fn budget_step_exceeded_sets_aborted_flag() {
    let input: String = (0..10_000)
        .map(|i| format!("line{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let p = profile_dedup_only();
    let out = apply_rules_with_budget(
        &input,
        &p.rules,
        Budget {
            max_steps: 100,
            max_wallclock_ms: u64::MAX,
            max_peak_bytes: u64::MAX,
        },
    );
    assert!(out.budget_aborted, "budget should have aborted");
}

#[test]
fn budget_peak_memory_exceeded_sets_aborted_flag() {
    let input: String = (0..1_000)
        .map(|i| format!("line{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let p = profile_dedup_only();
    let out = apply_rules_with_budget(
        &input,
        &p.rules,
        Budget {
            max_steps: u64::MAX,
            max_wallclock_ms: u64::MAX,
            max_peak_bytes: 100, // tiny
        },
    );
    assert!(out.budget_aborted, "peak-memory budget should have aborted");
}

#[test]
fn default_budget_passes_normal_input() {
    let input: String = (0..100)
        .map(|i| format!("line{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let p = profile_dedup_only();
    let out = apply_rules_with_budget(&input, &p.rules, Budget::default());
    assert!(!out.budget_aborted);
    assert!(out.lines.len() >= 100);
}
