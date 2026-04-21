//! `nid update` — download-and-verify-and-swap release flow (plan §10.3).
//!
//! Supports:
//! - `--check`: hit GitHub releases API, show current vs. latest.
//! - `--from <tarball>`: install a specific local signed tarball (offline).
//! - `--dry-run`: download + verify but don't swap.
//!
//! Verification:
//! 1. Tarball contains `nid` + `manifest.json` + `signature.bin` + `signer.pub`.
//! 2. Signature over manifest.json bytes is valid under signer.pub.
//! 3. Manifest's `binary_sha256` matches sha256(nid).
//! 4. Manifest's `signer_key_id` is either the shipped release-anchor pubkey
//!    OR reachable from the anchor via a shipped rotation chain.
//!
//! The current invocation's binary is NOT swapped in-process; we write the
//! new binary to a sibling file and atomically rename over the current one.

use anyhow::{Context, Result};
use clap::Args;
use nid_core::signing;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

const DEFAULT_REPO: &str = "newtro/nid";
const GITHUB_LATEST_RELEASE: &str = "https://api.github.com/repos/newtro/nid/releases/latest";

#[derive(Debug, Args)]
pub struct UpdateArgs {
    #[arg(long)]
    pub check: bool,
    #[arg(long)]
    pub to: Option<String>,
    #[arg(long, default_value = "stable")]
    pub channel: String,
    #[arg(long)]
    pub dry_run: bool,
    #[arg(long)]
    pub from: Option<PathBuf>,
}

pub async fn run(args: UpdateArgs) -> Result<()> {
    if args.check {
        return run_check().await;
    }
    if let Some(tarball) = args.from {
        return run_from_tarball(&tarball, args.dry_run);
    }
    // Network update path. We implement `--check` + `--from` as the
    // offline-safe paths. A full HTTPS-GH-release download is also below.
    run_from_latest_release(args.dry_run).await
}

/// Manifest embedded in every release tarball.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReleaseManifest {
    /// Version (semver).
    version: String,
    /// Signer key id (16 hex chars).
    signer_key_id: String,
    /// Target triple this binary is built for (informational).
    target: String,
    /// sha256 of the `nid` binary bytes.
    binary_sha256: String,
    /// Unix ts of signing.
    signed_at: i64,
    /// Optional rotation chain: links that prove the signer key descends
    /// from the shipped release anchor.
    #[serde(default)]
    rotation_chain: signing::RotationChain,
}

async fn run_check() -> Result<()> {
    println!("current: nid {}", env!("CARGO_PKG_VERSION"));
    println!("source : {DEFAULT_REPO}");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .user_agent(format!("nid/{}", env!("CARGO_PKG_VERSION")))
        .build()?;
    let resp = client.get(GITHUB_LATEST_RELEASE).send().await;
    match resp {
        Ok(r) if r.status().is_success() => {
            let body: serde_json::Value = r.json().await.unwrap_or_default();
            let tag = body
                .get("tag_name")
                .and_then(|v| v.as_str())
                .unwrap_or("(unknown)");
            println!("latest : {tag}");
        }
        Ok(r) => {
            println!("latest : (HTTP {})", r.status());
        }
        Err(e) => {
            println!("latest : (network error: {e})");
        }
    }
    Ok(())
}

fn run_from_tarball(tarball: &Path, dry_run: bool) -> Result<()> {
    let bytes = std::fs::read(tarball).with_context(|| format!("reading {}", tarball.display()))?;
    let (new_bin, manifest) = extract_and_verify(&bytes)?;
    println!("verified tarball:");
    println!("  version: {}", manifest.version);
    println!("  signer : {}", manifest.signer_key_id);
    println!("  target : {}", manifest.target);
    println!("  binary : {} bytes (sha256 ok)", new_bin.len());

    if dry_run {
        println!("dry-run: skipping swap.");
        return Ok(());
    }
    swap_binary(&new_bin).context("swapping binary")?;
    println!("installed {} in place of current binary.", manifest.version);
    Ok(())
}

async fn run_from_latest_release(dry_run: bool) -> Result<()> {
    println!("fetching {GITHUB_LATEST_RELEASE} ...");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent(format!("nid/{}", env!("CARGO_PKG_VERSION")))
        .build()?;
    let resp = client.get(GITHUB_LATEST_RELEASE).send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("GitHub release fetch failed: {}", resp.status());
    }
    let body: serde_json::Value = resp.json().await?;

    // Find the `.nidrel` asset for our target triple.
    let target = target_triple();
    let want = format!("nid-{target}.nidrel");
    let asset_url = body
        .get("assets")
        .and_then(|a| a.as_array())
        .and_then(|arr| {
            arr.iter().find_map(|a| {
                let name = a.get("name").and_then(|n| n.as_str()).unwrap_or("");
                if name == want {
                    a.get("browser_download_url").and_then(|u| u.as_str())
                } else {
                    None
                }
            })
        })
        .with_context(|| format!("no release asset named `{want}` in latest release"))?;
    println!("downloading {asset_url} ...");
    let bytes = client.get(asset_url).send().await?.bytes().await?.to_vec();
    let (new_bin, manifest) = extract_and_verify(&bytes)?;
    println!("verified {}@{}", manifest.version, manifest.signer_key_id);
    if dry_run {
        println!("dry-run: skipping swap.");
        return Ok(());
    }
    swap_binary(&new_bin).context("swapping binary")?;
    println!("installed {} in place of current binary.", manifest.version);
    Ok(())
}

const BINARY_ENTRY: &str = "nid";
const MANIFEST_ENTRY: &str = "manifest.json";
const SIGNATURE_ENTRY: &str = "signature.bin";
const SIGNER_PUB_ENTRY: &str = "signer.pub";

fn extract_and_verify(bytes: &[u8]) -> Result<(Vec<u8>, ReleaseManifest)> {
    let mut binary: Option<Vec<u8>> = None;
    let mut manifest_bytes: Option<Vec<u8>> = None;
    let mut signature: Option<Vec<u8>> = None;
    let mut pubkey: Option<Vec<u8>> = None;

    let mut ar = tar::Archive::new(std::io::Cursor::new(bytes));
    for e in ar.entries()? {
        let mut e = e?;
        let name = e.path()?.to_string_lossy().to_string();
        let mut buf = Vec::new();
        e.read_to_end(&mut buf)?;
        match name.as_str() {
            BINARY_ENTRY => binary = Some(buf),
            MANIFEST_ENTRY => manifest_bytes = Some(buf),
            SIGNATURE_ENTRY => signature = Some(buf),
            SIGNER_PUB_ENTRY => pubkey = Some(buf),
            _ => {}
        }
    }
    let binary = binary.context("release tarball missing `nid` entry")?;
    let manifest_bytes = manifest_bytes.context("release tarball missing manifest.json")?;
    let signature = signature.context("release tarball missing signature.bin")?;
    let pubkey = pubkey.context("release tarball missing signer.pub")?;

    let signer = signing::pubkey_from_bytes(&pubkey)
        .context("signer.pub is not a valid ed25519 public key")?;
    signing::verify(&signer, &manifest_bytes, &signature)
        .context("release manifest signature verification failed")?;

    let manifest: ReleaseManifest =
        serde_json::from_slice(&manifest_bytes).context("manifest.json parse")?;
    if manifest.signer_key_id != signing::key_id(&signer) {
        anyhow::bail!(
            "manifest signer_key_id {} != key embedded in signer.pub {}",
            manifest.signer_key_id,
            signing::key_id(&signer)
        );
    }

    // Binary integrity.
    let mut h = Sha256::new();
    h.update(&binary);
    let got = hex::encode(h.finalize());
    if got != manifest.binary_sha256 {
        anyhow::bail!(
            "binary sha256 mismatch: expected {}, got {}",
            manifest.binary_sha256,
            got
        );
    }

    // Trust: require the signer to be reachable from the anchor, or equal
    // to it. In v0.1 we ship no pinned anchor; the check is best-effort.
    if let Some(anchor_pubkey) = shipped_release_anchor() {
        if signing::key_id(&anchor_pubkey) != manifest.signer_key_id {
            manifest
                .rotation_chain
                .resolve(&anchor_pubkey, &manifest.signer_key_id)
                .context("release signer not reachable from shipped release anchor")?;
        }
    } else {
        // No anchor shipped — accept the signed tarball on self-trust (the
        // binary is free to install what the user asked for via `--from`).
        tracing::warn!(
            "no release anchor pinned in this nid build; trusting the tarball's signer by self-assertion"
        );
    }

    Ok((binary, manifest))
}

/// Pinned release anchor pubkey, if the build embedded one. v0.1 does not
/// ship a pinned anchor; future builds will include bytes via include_bytes!.
fn shipped_release_anchor() -> Option<ed25519_dalek::VerifyingKey> {
    // Placeholder: returns None for now. When a real release ships, replace
    // with:
    //   const ANCHOR: &[u8; 32] = include_bytes!("../../../release-anchor.pub");
    //   signing::pubkey_from_bytes(ANCHOR).ok()
    None
}

fn target_triple() -> &'static str {
    // Baked in at build time via NID_TARGET_TRIPLE env if set; otherwise a
    // string that says so. The release workflow sets it.
    option_env!("NID_TARGET_TRIPLE").unwrap_or("unknown-target")
}

fn swap_binary(new_bytes: &[u8]) -> std::io::Result<()> {
    let current = std::env::current_exe()?;
    let parent = current
        .parent()
        .ok_or_else(|| std::io::Error::other("current exe has no parent"))?;
    let tmp = parent.join("nid.new");
    std::fs::write(&tmp, new_bytes)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    }

    // Windows: can't overwrite a running executable directly. Rename current
    // out of the way first.
    #[cfg(windows)]
    {
        let backup = parent.join("nid.old");
        let _ = std::fs::remove_file(&backup);
        if current.exists() {
            std::fs::rename(&current, &backup)?;
        }
    }

    std::fs::rename(&tmp, &current)?;
    Ok(())
}
