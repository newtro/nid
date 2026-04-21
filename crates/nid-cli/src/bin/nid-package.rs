//! `nid-package` — pack a signed release tarball.
//!
//! Produces the tar layout that `nid update --from <tarball>` consumes:
//! `nid` (binary) + `manifest.json` + `signature.bin` + `signer.pub`.
//!
//! Usage:
//!   nid-package \
//!     --binary <path-to-nid-binary> \
//!     --version 0.1.0 \
//!     --target x86_64-unknown-linux-musl \
//!     --output nid-x86_64-unknown-linux-musl.nidrel \
//!     --signing-key-hex $NID_RELEASE_SIGNING_KEY

use clap::Parser;
use ed25519_dalek::Signer;
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug, Parser)]
struct Args {
    #[arg(long)]
    binary: PathBuf,
    #[arg(long)]
    version: String,
    #[arg(long)]
    target: String,
    #[arg(long)]
    output: PathBuf,
    /// Hex-encoded 32-byte ed25519 signing seed. Generated via
    /// `cargo run --bin nid-keygen` once; stored as a repo secret.
    #[arg(long)]
    signing_key_hex: String,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let bin = std::fs::read(&args.binary)?;
    let mut h = Sha256::new();
    h.update(&bin);
    let bin_sha = hex::encode(h.finalize());

    let seed: [u8; 32] = hex::decode(args.signing_key_hex.trim())?
        .try_into()
        .map_err(|_| anyhow::anyhow!("signing key must be exactly 32 bytes hex-encoded"))?;
    let sk = ed25519_dalek::SigningKey::from_bytes(&seed);
    let pk = sk.verifying_key();
    let mut hk = Sha256::new();
    hk.update(pk.as_bytes());
    let key_id_hex = hex::encode(&hk.finalize()[..8]);

    let manifest = serde_json::json!({
        "version": args.version,
        "signer_key_id": key_id_hex,
        "target": args.target,
        "binary_sha256": bin_sha,
        "signed_at": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
        "rotation_chain": { "links": [] }
    });
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
    let sig = sk.sign(&manifest_bytes);

    let file = std::fs::File::create(&args.output)?;
    let mut b = tar::Builder::new(file);
    for (name, bytes) in [
        ("nid", bin.as_slice()),
        ("manifest.json", manifest_bytes.as_slice()),
        ("signature.bin", &sig.to_bytes()[..]),
        ("signer.pub", pk.as_bytes().as_slice()),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_path(name)?;
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        b.append(&header, bytes)?;
    }
    b.finish()?;
    let mut out = std::io::stdout().lock();
    writeln!(
        out,
        "packed {} v{} for {} (signer {key_id_hex})",
        args.output.display(),
        args.version,
        args.target
    )?;
    Ok(())
}
