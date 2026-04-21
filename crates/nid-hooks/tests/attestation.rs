//! Verify plan §8.5 — every hook response carries an attestation block
//! in `additionalContext`.

use nid_hooks::{agents::handle_payload, PreToolPayload};

fn bash(cmd: &str) -> PreToolPayload {
    PreToolPayload {
        tool_name: "Bash".into(),
        command: cmd.into(),
        shadow: false,
    }
}

#[test]
fn rewritten_response_has_attestation() {
    let r = handle_payload(&bash("cargo build"), &[]);
    assert!(r.additional_context.is_some());
    let v = r.additional_context.unwrap();
    assert!(v.get("nid").is_some(), "nid key missing: {v}");
    let version = v["nid"]["version"].as_str().unwrap_or("");
    assert!(
        version.starts_with("0."),
        "version should be semver: {version}"
    );
    assert_eq!(v["nid"]["shadow"], false);
}

#[test]
fn shadow_response_records_shadow_true() {
    let mut p = bash("cargo build");
    p.shadow = true;
    let r = handle_payload(&p, &[]);
    let v = r.additional_context.unwrap();
    assert_eq!(v["nid"]["shadow"], true);
}

#[test]
fn passthrough_still_has_attestation() {
    let r = handle_payload(&bash("cd /tmp"), &[]);
    assert!(
        r.additional_context.is_some(),
        "attestation must be present even for passthrough"
    );
}
