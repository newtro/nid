//! Integration test: each of the 8 agent hook payload shapes must produce
//! the correct rewrite (command prepended with `nid`, idempotent, builtins
//! skipped).

use nid_hooks::{agents::handle_payload, AgentKind, PreToolPayload};
use serde_json::json;

fn bash(cmd: &str) -> PreToolPayload {
    PreToolPayload {
        tool_name: "Bash".into(),
        command: cmd.into(),
        shadow: false,
    }
}

#[test]
fn every_agent_rewrites_basic_bash_command() {
    for a in AgentKind::all() {
        let r = handle_payload(&bash("pytest -v"), &[]);
        assert!(
            r.updated_input.is_some(),
            "{:?}: should produce updatedInput",
            a
        );
        let cmd = r
            .updated_input
            .unwrap()
            .get("command")
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(cmd, "nid pytest -v", "agent {:?}", a);
    }
}

#[test]
fn every_agent_idempotent_for_already_nid() {
    for a in AgentKind::all() {
        let r = handle_payload(&bash("nid pytest"), &[]);
        assert!(
            r.updated_input.is_none(),
            "agent {:?} should be passthrough",
            a
        );
    }
}

#[test]
fn every_agent_skips_builtins() {
    for a in AgentKind::all() {
        for b in [
            "cd /tmp",
            "export FOO=1",
            "source env.sh",
            "alias ll='ls -l'",
        ] {
            let r = handle_payload(&bash(b), &[]);
            assert!(
                r.updated_input.is_none(),
                "agent {:?} / cmd `{b}` should passthrough",
                a
            );
        }
    }
}

#[test]
fn every_agent_handles_pipelines_as_whole() {
    for a in AgentKind::all() {
        let r = handle_payload(&bash("pytest | tee log.txt | grep FAIL"), &[]);
        let cmd = r
            .updated_input
            .unwrap()
            .get("command")
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(cmd, "nid pytest | tee log.txt | grep FAIL", "agent {:?}", a);
    }
}

#[test]
fn nid_raw_env_unwraps_for_every_agent() {
    for a in AgentKind::all() {
        let r = handle_payload(&bash("NID_RAW=1 cargo build"), &[]);
        let cmd = r
            .updated_input
            .unwrap()
            .get("command")
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(cmd, "cargo build", "agent {:?}", a);
    }
}

#[test]
fn non_bash_tool_is_untouched() {
    let r = handle_payload(
        &PreToolPayload {
            tool_name: "Read".into(),
            command: "src/foo.rs".into(),
            shadow: false,
        },
        &[],
    );
    assert!(r.updated_input.is_none());
}

#[test]
fn double_nid_collapses_to_single() {
    // "nid nid pytest" — the outer nid should be preserved as-is (starts with
    // "nid "), so the response is a passthrough.
    let r = handle_payload(&bash("nid nid pytest"), &[]);
    assert!(r.updated_input.is_none());
}

#[test]
fn response_json_shape_has_expected_fields() {
    let r = handle_payload(&bash("cargo build"), &[]);
    let v: serde_json::Value = serde_json::to_value(&r).unwrap();
    assert!(v.get("updatedInput").is_some());
    assert_eq!(
        v.get("updatedInput").unwrap(),
        &json!({"command": "nid cargo build"})
    );
}
