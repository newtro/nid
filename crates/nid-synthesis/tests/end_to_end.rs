//! Phase 4 verification: structural-diff synthesis on 5 fixture samples
//! produces a valid DSL that passes self-tests.

use nid_dsl::{interpreter, invariants as inv, validator};
use nid_synthesis::orchestrator::synthesize_from_samples;

#[tokio::test(flavor = "current_thread")]
async fn five_samples_lock_in_and_self_tests_pass() {
    // Five near-identical samples of a synthetic "build" command.
    let samples: Vec<String> = vec![
        "Compiling foo v0.1.0\nFinished in 1.23s\n".into(),
        "Compiling foo v0.1.0\nFinished in 1.45s\n".into(),
        "Compiling foo v0.1.0\nFinished in 0.92s\n".into(),
        "Compiling foo v0.1.0\nFinished in 2.10s\n".into(),
        "Compiling foo v0.1.0\nFinished in 1.05s\n".into(),
    ];

    let out = synthesize_from_samples("synth-test", &samples, |_| async { Ok(None) })
        .await
        .unwrap();
    validator::validate_profile(&out.profile).expect("DSL must validate");
    assert_eq!(out.profile.meta.fingerprint, "synth-test");

    // Self-test against every sample: invariants must pass.
    for s in &samples {
        let compressed = interpreter::apply_rules(s, &out.profile.rules).to_string();
        let results = inv::check_invariants(&out.profile.invariants, s, &compressed).unwrap();
        for r in results {
            assert!(r.passed, "invariant {} failed", r.name);
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn synthesized_output_is_structural_subset() {
    let samples: Vec<String> = vec![
        "ERROR: something bad\ndetail\ndetail\ndetail\ndetail\n".into(),
        "ERROR: something else\ndetail\ndetail\ndetail\ndetail\n".into(),
        "ERROR: another\ndetail\ndetail\ndetail\ndetail\n".into(),
        "ERROR: yet another\ndetail\ndetail\ndetail\ndetail\n".into(),
        "ERROR: fifth\ndetail\ndetail\ndetail\ndetail\n".into(),
    ];
    let out = synthesize_from_samples("err-test", &samples, |_| async { Ok(None) })
        .await
        .unwrap();
    for s in &samples {
        let compressed = interpreter::apply_rules(s, &out.profile.rules).to_string();
        let raw_lines: std::collections::HashSet<&str> = s.lines().collect();
        for l in compressed.lines() {
            if l.is_empty() || l.starts_with("[... ") {
                continue;
            }
            assert!(
                raw_lines.contains(l),
                "synthesized profile invented line `{l}`"
            );
        }
    }
}
