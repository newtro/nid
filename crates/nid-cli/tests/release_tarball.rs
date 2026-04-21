//! Build a signed release tarball in-test, then verify `nid update --from`
//! can read and swap it (--dry-run variant to avoid actually replacing the
//! binary under test).

use assert_cmd::Command;
use ed25519_dalek::Signer;
use sha2::{Digest, Sha256};
use std::io::Write as _;
use tempfile::TempDir;

fn nid() -> Command {
    Command::cargo_bin("nid").unwrap()
}

#[test]
fn update_from_signed_tarball_dry_run() {
    let tmp = TempDir::new().unwrap();

    // Build a fake binary payload + manifest + signature.
    let bin: Vec<u8> = b"#!/bin/sh\necho fake\n".to_vec();
    let mut h = Sha256::new();
    h.update(&bin);
    let bin_sha = hex::encode(h.finalize());

    use rand::RngCore;
    let mut seed = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut seed);
    let sk = ed25519_dalek::SigningKey::from_bytes(&seed);
    let pk = sk.verifying_key();
    let key_id_hex = {
        let mut hk = Sha256::new();
        hk.update(pk.as_bytes());
        let d = hk.finalize();
        hex::encode(&d[..8])
    };

    let manifest = serde_json::json!({
        "version": "0.9.9-test",
        "signer_key_id": key_id_hex,
        "target": "x86_64-pc-windows-msvc",
        "binary_sha256": bin_sha,
        "signed_at": 1_713_000_000,
        "rotation_chain": { "links": [] }
    });
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).unwrap();
    let sig = sk.sign(&manifest_bytes);

    // Assemble tar.
    let tarball_path = tmp.path().join("test.nidrel");
    let file = std::fs::File::create(&tarball_path).unwrap();
    let mut builder = tar::Builder::new(file);
    for (name, bytes) in [
        ("nid", bin.as_slice()),
        ("manifest.json", manifest_bytes.as_slice()),
        ("signature.bin", &sig.to_bytes()[..]),
        ("signer.pub", pk.as_bytes().as_slice()),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_path(name).unwrap();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append(&header, bytes).unwrap();
    }
    builder.finish().unwrap();

    // Run `nid update --from <tarball> --dry-run`. Tests build nid without
    // a pinned anchor, so the explicit unanchored opt-in is required.
    let out = nid()
        .env("NID_RELEASE_ALLOW_UNANCHORED", "1")
        .args([
            "update",
            "--from",
            tarball_path.to_str().unwrap(),
            "--dry-run",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "update dry-run failed: stdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("verified tarball"), "stdout: {stdout}");
    assert!(stdout.contains("0.9.9-test"), "stdout: {stdout}");
    assert!(stdout.contains("dry-run"), "stdout: {stdout}");
}

#[test]
fn update_rejects_tampered_binary() {
    let tmp = TempDir::new().unwrap();
    let bin: Vec<u8> = b"fake binary".to_vec();
    let mut h = Sha256::new();
    h.update(&bin);
    let bin_sha = hex::encode(h.finalize());

    use rand::RngCore;
    let mut seed = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut seed);
    let sk = ed25519_dalek::SigningKey::from_bytes(&seed);
    let pk = sk.verifying_key();
    let mut hk = Sha256::new();
    hk.update(pk.as_bytes());
    let key_id_hex = hex::encode(&hk.finalize()[..8]);

    // Manifest says sha=bin_sha; but we'll ship DIFFERENT bytes in the
    // tarball so the check fires.
    let manifest = serde_json::json!({
        "version": "0.0.1",
        "signer_key_id": key_id_hex,
        "target": "x86_64-pc-windows-msvc",
        "binary_sha256": bin_sha,
        "signed_at": 0,
        "rotation_chain": { "links": [] }
    });
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).unwrap();
    let sig = sk.sign(&manifest_bytes);

    let tarball_path = tmp.path().join("bad.nidrel");
    {
        let file = std::fs::File::create(&tarball_path).unwrap();
        let mut builder = tar::Builder::new(file);
        let tampered = b"tampered binary bytes";
        for (name, bytes) in [
            ("nid", tampered.as_slice()),
            ("manifest.json", manifest_bytes.as_slice()),
            ("signature.bin", &sig.to_bytes()[..]),
            ("signer.pub", pk.as_bytes().as_slice()),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_path(name).unwrap();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append(&header, bytes).unwrap();
        }
        builder.finish().unwrap();
    }

    let out = nid()
        .env("NID_RELEASE_ALLOW_UNANCHORED", "1")
        .args([
            "update",
            "--from",
            tarball_path.to_str().unwrap(),
            "--dry-run",
        ])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "tampered tarball must be rejected; stdout={}, stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("sha256 mismatch") || stderr.contains("signature"),
        "stderr did not report binary integrity failure: {stderr}"
    );
}

// Shut up dead-code warnings.
#[allow(dead_code)]
fn _write_ignored(mut f: std::fs::File, data: &[u8]) -> std::io::Result<()> {
    f.write_all(data)
}

#[test]
fn unanchored_build_refuses_signed_tarball_by_default() {
    let tmp = TempDir::new().unwrap();
    let bin: Vec<u8> = b"fake".to_vec();
    let mut h = Sha256::new();
    h.update(&bin);
    let bin_sha = hex::encode(h.finalize());

    use rand::RngCore;
    let mut seed = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut seed);
    let sk = ed25519_dalek::SigningKey::from_bytes(&seed);
    let pk = sk.verifying_key();
    let mut hk = Sha256::new();
    hk.update(pk.as_bytes());
    let key_id_hex = hex::encode(&hk.finalize()[..8]);

    let manifest = serde_json::json!({
        "version": "0.0.1",
        "signer_key_id": key_id_hex,
        "target": "x86_64-pc-windows-msvc",
        "binary_sha256": bin_sha,
        "signed_at": 0,
        "rotation_chain": { "links": [] }
    });
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).unwrap();
    let sig = sk.sign(&manifest_bytes);

    let tarball = tmp.path().join("unanchored.nidrel");
    {
        let file = std::fs::File::create(&tarball).unwrap();
        let mut b = tar::Builder::new(file);
        for (name, bytes) in [
            ("nid", bin.as_slice()),
            ("manifest.json", manifest_bytes.as_slice()),
            ("signature.bin", &sig.to_bytes()[..]),
            ("signer.pub", pk.as_bytes().as_slice()),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_path(name).unwrap();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            b.append(&header, bytes).unwrap();
        }
        b.finish().unwrap();
    }

    // Without NID_RELEASE_ALLOW_UNANCHORED, installer must refuse.
    let out = nid()
        .args(["update", "--from", tarball.to_str().unwrap(), "--dry-run"])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "unanchored build must refuse signed tarball without opt-in"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no pinned release anchor"),
        "stderr: {stderr}"
    );
}
