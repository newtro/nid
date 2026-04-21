//! Verify every fixture pair demonstrates at least *some* compression, except
//! for format-preserving profiles where identity is expected.
//!
//! This is the guardrail against the "test theatre" failure mode — where
//! raw.txt and expected.txt drift toward being byte-identical and the golden
//! test stops measuring anything.

use nid_dsl::interpreter;
use nid_profiles::load_all;
use std::fs;
use std::path::PathBuf;

/// Fingerprints that are legitimately format-preservation-only. `jq .`
/// re-emits well-formed JSON; we don't expect compression on a valid input.
const IDENTITY_ALLOWED: &[&str] = &["jq ."];

fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("tests")
        .join("fixtures")
}

#[test]
fn every_non_identity_fixture_demonstrates_compression() {
    let root = fixtures_root();
    let bundled = load_all();

    let mut identity_count = 0usize;
    let mut compressed_count = 0usize;

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
        let fp = fs::read_to_string(&fp_p).unwrap().trim().to_string();
        let raw = fs::read_to_string(&raw_p).unwrap();
        let expected = fs::read_to_string(&exp_p).unwrap();

        // Use .as_ref() comparison on the byte-trimmed content.
        let raw_trim = raw.trim_end_matches('\n');
        let exp_trim = expected.trim_end_matches('\n');
        let is_identity = raw_trim == exp_trim;

        if is_identity {
            assert!(
                IDENTITY_ALLOWED.contains(&fp.as_str()),
                "fixture {} is raw==expected but fp `{fp}` is not in the identity-allowed list",
                dir.display()
            );
            identity_count += 1;
        } else {
            // Sanity: expected must actually be a structural subset of raw.
            let raw_lines: std::collections::HashSet<&str> = raw.lines().collect();
            let (_, profile) = bundled
                .iter()
                .find(|(_, p)| p.meta.fingerprint == fp)
                .expect("profile must exist");
            let out = interpreter::apply_rules(&raw, &profile.rules).to_string();
            for l in out.lines() {
                if l.is_empty() || l.starts_with("[... ") {
                    continue;
                }
                assert!(
                    raw_lines.contains(l),
                    "{}: invented line `{l}` not in raw",
                    dir.display()
                );
            }
            // Compression: bytes-out must be strictly less than bytes-in.
            assert!(
                expected.len() < raw.len(),
                "{} ({}): expected ({} bytes) not smaller than raw ({} bytes)",
                dir.display(),
                fp,
                expected.len(),
                raw.len()
            );
            compressed_count += 1;
        }
    }

    // Sanity: at least 15 of our 20 profiles should be demonstrably compressing.
    assert!(
        compressed_count >= 15,
        "only {compressed_count} fixtures compress; rest are identity ({identity_count})"
    );
}
