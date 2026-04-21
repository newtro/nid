//! `nid trust {add,revoke,list}` — signer trust keyring (plan §11.2).

use anyhow::{Context, Result};
use clap::Subcommand;
use nid_core::signing;
use nid_storage::{trust_repo::TrustRepo, Db};

#[derive(Debug, Subcommand)]
pub enum TrustCmd {
    /// Add a public key file (32 raw bytes OR hex-encoded) to the trust
    /// keyring.
    Add {
        key: std::path::PathBuf,
        #[arg(long)]
        label: Option<String>,
    },
    /// Revoke a key by its 16-char hex key-id.
    Revoke { key_id: String },
    /// List all trusted keys.
    List,
}

pub async fn run(sub: TrustCmd) -> Result<()> {
    let paths = crate::cmd::paths::resolve()?;
    paths.ensure()?;
    let db = Db::open(&paths.db_path)?;
    let repo = TrustRepo::new(&db);

    match sub {
        TrustCmd::List => {
            let keys = repo.list_active()?;
            if keys.is_empty() {
                println!("(no trusted keys)");
            } else {
                for k in keys {
                    println!("  {:<16}  {}  (added {})", k.key_id, k.label, k.added_at);
                }
            }
            Ok(())
        }
        TrustCmd::Add { key, label } => {
            let bytes = load_key_file(&key)?;
            let pk = signing::pubkey_from_bytes(&bytes)
                .with_context(|| format!("not a valid ed25519 public key: {}", key.display()))?;
            let id = signing::key_id(&pk);
            let label = label.unwrap_or_else(|| id.clone());
            repo.add(&id, pk.as_bytes(), &label)?;
            println!("trusted {id} ({label})");
            Ok(())
        }
        TrustCmd::Revoke { key_id } => {
            repo.revoke(&key_id)?;
            println!("revoked {key_id}");
            Ok(())
        }
    }
}

/// Accept either raw 32-byte bytes or a hex string.
fn load_key_file(p: &std::path::Path) -> Result<Vec<u8>> {
    let bytes = std::fs::read(p).with_context(|| format!("reading {}", p.display()))?;
    if bytes.len() == 32 {
        return Ok(bytes);
    }
    // Strip whitespace and try hex.
    let s = std::str::from_utf8(&bytes).unwrap_or("").trim();
    let decoded = hex::decode(s).with_context(|| {
        format!(
            "key file {} is not 32 raw bytes or hex-encoded 32-byte key",
            p.display()
        )
    })?;
    if decoded.len() != 32 {
        anyhow::bail!("decoded key is {} bytes, expected 32", decoded.len());
    }
    Ok(decoded)
}
