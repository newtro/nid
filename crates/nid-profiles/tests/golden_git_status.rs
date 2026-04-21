//! Golden-output test for the bundled `git status` profile.

use nid_dsl::{interpreter, validator};
use nid_profiles::load_all;

const RAW: &str = include_str!("../../../tests/fixtures/git_status/raw.txt");
const EXPECTED: &str = include_str!("../../../tests/fixtures/git_status/expected.txt");

#[test]
fn git_status_profile_matches_golden() {
    let all = load_all();
    let (_, profile) = all
        .iter()
        .find(|(_, p)| p.meta.fingerprint == "git status")
        .expect("git status profile should be bundled");
    validator::validate_profile(profile).expect("profile must validate");

    let out = interpreter::apply_rules(RAW, &profile.rules);
    let got = out.to_string();
    // The interpreter emits a trailing newline after every line — match by
    // trimming trailing whitespace only.
    assert_eq!(
        got.trim_end_matches('\n'),
        EXPECTED.trim_end_matches('\n'),
        "bytes differ:\n=== got ===\n{got}\n=== expected ===\n{EXPECTED}"
    );
}

#[test]
fn git_status_profile_is_structural_subset() {
    // Every non-placeholder output line must appear in raw.
    let all = load_all();
    let (_, profile) = all
        .iter()
        .find(|(_, p)| p.meta.fingerprint == "git status")
        .unwrap();
    let out = interpreter::apply_rules(RAW, &profile.rules);
    let got = out.to_string();
    let raw_lines: std::collections::HashSet<&str> = RAW.lines().collect();
    for l in got.lines() {
        if l.is_empty() {
            continue;
        }
        assert!(
            raw_lines.contains(l),
            "output line `{l}` not found in raw — compression must be a structural subset"
        );
    }
}

#[test]
fn git_status_profile_invariants_pass() {
    let all = load_all();
    let (_, profile) = all
        .iter()
        .find(|(_, p)| p.meta.fingerprint == "git status")
        .unwrap();
    let out = interpreter::apply_rules(RAW, &profile.rules).to_string();
    let results = nid_dsl::invariants::check_invariants(&profile.invariants, RAW, &out).unwrap();
    for r in &results {
        assert!(
            r.passed,
            "invariant `{}` failed: {:?}",
            r.name, r.detail
        );
    }
}
