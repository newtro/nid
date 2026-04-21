//! One test per built-in redaction pattern (plan §11.3).

use nid_core::redact::redact;

#[test]
fn catches_aws_access_key() {
    let out = redact("AKIAIOSFODNN7EXAMPLE in output");
    assert!(out.contains("[REDACTED:aws_access_key]"), "{out}");
}

#[test]
fn catches_github_pat_classic() {
    let out = redact("ghp_1234567890abcdefghij1234567890abcdefghij tail");
    assert!(out.contains("[REDACTED:github_pat_classic]"), "{out}");
}

#[test]
fn catches_github_pat_fine_grained() {
    let out = redact("github_pat_0123456789abcdefghij0123456789abcdefghij0123456 tail");
    assert!(out.contains("[REDACTED:github_pat_fine]"), "{out}");
}

#[test]
fn catches_gitlab_token() {
    let out = redact("glpat-abcdefghij0123456789");
    assert!(out.contains("[REDACTED:gitlab_token]"), "{out}");
}

#[test]
fn catches_stripe_live_key() {
    let out = redact("charge: sk_live_abcdefghij0123456789 processed");
    assert!(out.contains("[REDACTED:stripe_live]"), "{out}");
}

#[test]
fn catches_stripe_test_key() {
    let out = redact("test with sk_test_abcdefghij0123456789");
    assert!(out.contains("[REDACTED:stripe_test]"), "{out}");
}

#[test]
fn catches_jwt_triplet() {
    let out =
        redact("bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ4In0.SflKxwRJSMeKKF2QT4fwpMeJf_abc123");
    assert!(out.contains("[REDACTED:jwt]"), "{out}");
}

#[test]
fn catches_ssh_private_key_block() {
    let out = redact(
        "-----BEGIN OPENSSH PRIVATE KEY-----\nlinesoftext\n-----END OPENSSH PRIVATE KEY-----",
    );
    assert!(out.contains("[REDACTED:ssh_private_key_block]"), "{out}");
}

#[test]
fn catches_bearer_header() {
    let out = redact("Authorization: Bearer abcdef1234567890abcdef1234567890\n");
    assert!(out.contains("[REDACTED:bearer_header]"), "{out}");
}

#[test]
fn catches_high_entropy_token() {
    // High-entropy generic secret (aspirational catch-all).
    let out = redact("secret: Xk9mPqRtUvWyZa23bCdE8fGh4iJ6kLmN5oPqR9sTu8vW9x very random");
    assert!(out.contains("[REDACTED:high_entropy]"), "{out}");
}

#[test]
fn leaves_normal_build_output_alone() {
    let normal = "    Compiling hello v0.1.0\n   Finished `dev` profile in 3.2s\n";
    assert_eq!(redact(normal), normal);
}
