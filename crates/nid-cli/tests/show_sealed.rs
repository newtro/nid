//! End-to-end: `nid <cmd>` stores sealed raw; `nid show` returns redacted
//! by default and unredacted under confirmation.
#![cfg(unix)]

use assert_cmd::Command;
use std::fs;
use tempfile::TempDir;

fn nid() -> Command {
    Command::cargo_bin("nid").unwrap()
}

fn tmp_env() -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let t = TempDir::new().unwrap();
    let cfg = t.path().join("config");
    let data = t.path().join("data");
    fs::create_dir_all(&cfg).unwrap();
    fs::create_dir_all(&data).unwrap();
    (t, cfg, data)
}

#[cfg(unix)]
#[test]
fn show_redacts_by_default_and_unredacts_under_confirmation() {
    // Run a synthetic command that emits a fake AWS key, then show the
    // session both ways. Note: `nid <cmd...>` joins its argv into a
    // single string and spawns `sh -c <joined>`. Nesting `sh -c` on top
    // of that double-shells and collapses the command. A plain `echo
    // AKIA...` works fine because the outer sh resolves echo as the
    // first token.
    let (_t, cfg, data) = tmp_env();
    let fake_secret = "AKIAIOSFODNN7EXAMPLE";
    let out = nid()
        .env("NID_CONFIG_DIR", &cfg)
        .env("NID_DATA_DIR", &data)
        .arg("echo")
        .arg(fake_secret)
        .output()
        .unwrap();
    // `nid <cmd>` exits with the wrapped command's exit code.
    // We need the session id — parse it from the attestation footer on stdout.
    let stdout = String::from_utf8_lossy(&out.stdout);
    let sess_id = stdout
        .lines()
        .find_map(|l| {
            l.split_whitespace()
                .find(|tok| tok.starts_with("sess_"))
                .map(|s| s.trim_end_matches(']').to_string())
        })
        .expect("could not find session id in attestation footer");

    // Default: show redacts.
    let redacted = nid()
        .env("NID_CONFIG_DIR", &cfg)
        .env("NID_DATA_DIR", &data)
        .args(["show", &sess_id])
        .output()
        .unwrap();
    assert!(redacted.status.success());
    let redacted_text = String::from_utf8_lossy(&redacted.stdout);
    assert!(
        !redacted_text.contains(fake_secret),
        "default show must redact: {redacted_text}"
    );
    assert!(
        redacted_text.contains("[REDACTED:aws_access_key]"),
        "default show must show redact marker: {redacted_text}"
    );

    // --raw-unredacted with confirmation.
    let unredacted = nid()
        .env("NID_CONFIG_DIR", &cfg)
        .env("NID_DATA_DIR", &data)
        .env("NID_UNREDACTED_OK", "1")
        .args(["show", "--raw-unredacted", &sess_id])
        .output()
        .unwrap();
    assert!(unredacted.status.success());
    let unredacted_text = String::from_utf8_lossy(&unredacted.stdout);
    assert!(
        unredacted_text.contains(fake_secret),
        "--raw-unredacted must produce the original secret: {unredacted_text}"
    );
    // Access log must exist.
    let log = data.join("show_access.log");
    assert!(log.exists(), "access log must be written");
    let log_body = fs::read_to_string(&log).unwrap();
    assert!(log_body.contains(&sess_id));
}
