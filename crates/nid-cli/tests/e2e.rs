//! End-to-end binary tests. Spawns the compiled `nid` binary under
//! `NID_CONFIG_DIR` / `NID_DATA_DIR` overrides pointing at a temp directory.

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

#[test]
fn version_prints_semver() {
    let out = nid().arg("version").output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.starts_with("nid 0."),
        "expected nid 0.X, got: {stdout}"
    );
}

#[test]
fn doctor_runs_against_ephemeral_tmp() {
    let (_tmp, cfg, data) = tmp_env();
    let out = nid()
        .env("NID_CONFIG_DIR", &cfg)
        .env("NID_DATA_DIR", &data)
        .arg("doctor")
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("sqlite:"));
}

#[test]
fn profiles_list_shows_bundled() {
    let (_tmp, cfg, data) = tmp_env();
    let out = nid()
        .env("NID_CONFIG_DIR", &cfg)
        .env("NID_DATA_DIR", &data)
        .args(["profiles", "list"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("git status")
            && stdout.contains("cargo build")
            && stdout.contains("pytest"),
        "stdout: {stdout}"
    );
}

#[test]
fn onboard_check_produces_detection_output() {
    let (_tmp, cfg, data) = tmp_env();
    // Stub home to tmp so agent configs aren't discovered.
    let out = nid()
        .env("NID_CONFIG_DIR", &cfg)
        .env("NID_DATA_DIR", &data)
        .env("HOME", cfg.parent().unwrap())
        .env("USERPROFILE", cfg.parent().unwrap())
        .args(["onboard", "--check"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("onboard --check"));
    // Every supported agent should be listed.
    for a in ["Claude Code", "Cursor", "Codex CLI", "Aider"] {
        assert!(stdout.contains(a), "missing agent {a} in:\n{stdout}");
    }
}

#[test]
fn hook_handler_rewrites_claude_code_payload() {
    let (_tmp, cfg, data) = tmp_env();
    let out = nid()
        .env("NID_CONFIG_DIR", &cfg)
        .env("NID_DATA_DIR", &data)
        .args(["__hook", "claude_code"])
        .write_stdin(r#"{"tool_name":"Bash","tool_input":{"command":"pytest -v"}}"#)
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(
        v.get("updatedInput").unwrap(),
        &serde_json::json!({"command": "nid pytest -v"})
    );
}

#[test]
fn hook_handler_idempotent_on_already_nid() {
    let (_tmp, cfg, data) = tmp_env();
    let out = nid()
        .env("NID_CONFIG_DIR", &cfg)
        .env("NID_DATA_DIR", &data)
        .args(["__hook", "claude_code"])
        .write_stdin(r#"{"tool_name":"Bash","tool_input":{"command":"nid pytest -v"}}"#)
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    // Passthrough: no updatedInput.
    assert!(
        v.get("updatedInput").is_none() || v.get("updatedInput") == Some(&serde_json::Value::Null)
    );
}

#[test]
fn shadow_enable_creates_state_file() {
    let (_tmp, cfg, data) = tmp_env();
    nid()
        .env("NID_CONFIG_DIR", &cfg)
        .env("NID_DATA_DIR", &data)
        .args(["shadow", "enable"])
        .assert()
        .success();
    let state = cfg.join("shadow.state");
    assert!(state.exists());
    assert_eq!(fs::read_to_string(&state).unwrap(), "enable");

    // Commit removes it.
    nid()
        .env("NID_CONFIG_DIR", &cfg)
        .env("NID_DATA_DIR", &data)
        .args(["shadow", "commit"])
        .assert()
        .success();
    assert!(!state.exists());
}

#[test]
fn profiles_export_then_import_roundtrip() {
    let (tmp, cfg, data) = tmp_env();
    let tarball = tmp.path().join("git_status.nidprofile");

    // Export (auto-generates a key).
    let out = nid()
        .env("NID_CONFIG_DIR", &cfg)
        .env("NID_DATA_DIR", &data)
        .args([
            "profiles",
            "export",
            "git status",
            tarball.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "export failed: {:?}", out);
    let export_stdout = String::from_utf8_lossy(&out.stdout).to_string();
    // Key id line: "signer key-id = <16 hex>"
    let key_id = export_stdout
        .split_whitespace()
        .last()
        .expect("stdout had no key id")
        .to_string();
    assert_eq!(key_id.len(), 16, "key_id: {key_id}");
    assert!(tarball.exists());

    // Import without adding to trust should refuse.
    let out = nid()
        .env("NID_CONFIG_DIR", &cfg)
        .env("NID_DATA_DIR", &data)
        .args(["profiles", "import", tarball.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!out.status.success(), "unsigned-trust import must refuse");

    // Extract the signer pubkey from the tarball manually and add to trust.
    let mut bytes = Vec::new();
    let mut ar = tar::Archive::new(std::io::Cursor::new(fs::read(&tarball).unwrap()));
    for entry in ar.entries().unwrap() {
        let mut e = entry.unwrap();
        if e.path().unwrap().to_string_lossy() == "signer.pub" {
            std::io::Read::read_to_end(&mut e, &mut bytes).unwrap();
            break;
        }
    }
    let key_path = tmp.path().join("key.pub");
    fs::write(&key_path, &bytes).unwrap();

    nid()
        .env("NID_CONFIG_DIR", &cfg)
        .env("NID_DATA_DIR", &data)
        .args([
            "trust",
            "add",
            key_path.to_str().unwrap(),
            "--label",
            "test",
        ])
        .assert()
        .success();

    // Now import should succeed.
    let out = nid()
        .env("NID_CONFIG_DIR", &cfg)
        .env("NID_DATA_DIR", &data)
        .args(["profiles", "import", tarball.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "trusted import should succeed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("imported"));
}

#[test]
fn synthesize_refuses_without_samples() {
    let (_tmp, cfg, data) = tmp_env();
    let out = nid()
        .env("NID_CONFIG_DIR", &cfg)
        .env("NID_DATA_DIR", &data)
        .args(["synthesize", "some", "unknown", "cmd"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("no samples"));
}
