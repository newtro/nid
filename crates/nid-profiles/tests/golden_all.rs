//! Unified golden-fixture harness: for every directory under
//! `/tests/fixtures/<slug>/` containing `raw.txt` + `expected.txt` + a
//! `profile_fingerprint.txt` pointer, apply the matching bundled profile
//! and assert byte-equal output.
//!
//! This lets us ship many bundled profiles without a proliferating
//! one-test-file-per-profile pattern.

use nid_dsl::interpreter;
use nid_profiles::load_all;
use std::fs;
use std::path::{Path, PathBuf};

fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("tests")
        .join("fixtures")
}

fn read_trim(p: &Path) -> String {
    fs::read_to_string(p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

#[test]
fn every_fixture_dir_matches_its_profile_golden() {
    let root = fixtures_root();
    if !root.is_dir() {
        panic!("fixtures dir missing: {}", root.display());
    }
    let bundled = load_all();

    let mut cases_run = 0usize;
    for entry in fs::read_dir(&root).unwrap() {
        let entry = entry.unwrap();
        if !entry.file_type().unwrap().is_dir() {
            continue;
        }
        let dir = entry.path();
        let raw_p = dir.join("raw.txt");
        let exp_p = dir.join("expected.txt");
        let fp_p = dir.join("profile_fingerprint.txt");
        if !raw_p.exists() || !exp_p.exists() || !fp_p.exists() {
            continue;
        }

        let fp = read_trim(&fp_p).trim().to_string();
        let raw = read_trim(&raw_p);
        let expected = read_trim(&exp_p);
        let (_, profile) = bundled
            .iter()
            .find(|(_, p)| p.meta.fingerprint == fp)
            .unwrap_or_else(|| panic!("no bundled profile for fingerprint `{fp}`"));

        let out = interpreter::apply_rules(&raw, &profile.rules).to_string();
        assert_eq!(
            out.trim_end_matches('\n'),
            expected.trim_end_matches('\n'),
            "mismatch for fixture {}\n=== got ===\n{out}\n=== expected ===\n{expected}",
            dir.display()
        );

        // Also assert structural-subset: every non-placeholder output line must
        // appear in raw (plan §8.1 Tier 2).
        let raw_lines: std::collections::HashSet<&str> = raw.lines().collect();
        for l in out.lines() {
            if l.is_empty() {
                continue;
            }
            if l.starts_with("[... ") || l.starts_with("[nid:") {
                continue;
            }
            assert!(
                raw_lines.contains(l),
                "non-subset output line `{l}` for fixture {}",
                dir.display()
            );
        }

        cases_run += 1;
    }
    assert!(cases_run > 0, "no fixture cases ran");
}
