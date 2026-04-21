//! `nid profiles …`

use anyhow::{Context, Result};
use clap::Subcommand;
use nid_core::signing;
use nid_dsl::nidprofile;
use nid_storage::{
    blob::{BlobKind, BlobStore},
    profile_repo::{NewProfile, ProfileRepo, PROV_IMPORTED},
    trust_repo::TrustRepo,
    Db,
};

#[derive(Debug, Subcommand)]
pub enum ProfilesCmd {
    /// List profiles (bundled + persisted).
    List,
    /// Inspect a single profile by fingerprint.
    Inspect { fingerprint: String },
    /// Pin a profile version.
    Pin { fingerprint: String },
    /// Revoke (force re-synthesis path) a profile.
    Revoke { fingerprint: String },
    /// Export a profile as a signed `.nidprofile` tarball.
    Export {
        fingerprint: String,
        output: std::path::PathBuf,
        /// Path to an ed25519 signing key (32 raw bytes or hex). If not set,
        /// nid will generate a fresh local key and print its key-id.
        #[arg(long)]
        key: Option<std::path::PathBuf>,
    },
    /// Import a signed `.nidprofile` tarball. Signer must be in the trust
    /// keyring (see `nid trust add`). `--allow-unsigned` opts out after
    /// an interactive confirmation (or NID_UNTRUSTED_OK=1).
    Import {
        path: std::path::PathBuf,
        #[arg(long)]
        allow_unsigned: bool,
    },
    /// Re-sign an exported tarball with a different key.
    Sign {
        tarball: std::path::PathBuf,
        #[arg(long)]
        key: std::path::PathBuf,
    },
    /// Roll back to the most recent superseded version.
    Rollback { fingerprint: String },
    /// Permanently remove a profile.
    Purge { fingerprint: String },
}

pub async fn run(sub: ProfilesCmd) -> Result<()> {
    match sub {
        ProfilesCmd::List => list().await,
        ProfilesCmd::Inspect { fingerprint } => inspect(fingerprint).await,
        ProfilesCmd::Export {
            fingerprint,
            output,
            key,
        } => export(fingerprint, output, key).await,
        ProfilesCmd::Import {
            path,
            allow_unsigned,
        } => import(path, allow_unsigned).await,
        ProfilesCmd::Rollback { fingerprint } => rollback(fingerprint).await,
        ProfilesCmd::Purge { fingerprint } => purge(fingerprint).await,
        ProfilesCmd::Revoke { fingerprint } => revoke(fingerprint).await,
        other => {
            println!("subcommand {other:?} not yet implemented");
            Ok(())
        }
    }
}

async fn rollback(fingerprint: String) -> Result<()> {
    let paths = crate::cmd::paths::resolve()?;
    let db = Db::open(&paths.db_path)?;
    let repo = ProfileRepo::new(&db);
    match repo.rollback(&fingerprint)? {
        Some(id) => println!("rolled back `{fingerprint}` to profile id {id}"),
        None => println!("no superseded profile to roll back for `{fingerprint}`"),
    }
    Ok(())
}

async fn purge(fingerprint: String) -> Result<()> {
    let paths = crate::cmd::paths::resolve()?;
    let db = Db::open(&paths.db_path)?;
    let repo = ProfileRepo::new(&db);
    let store = BlobStore::new(&db, &paths.blobs_dir);
    let rows = repo.list()?;
    let mut count = 0usize;
    for r in rows.iter().filter(|r| r.fingerprint == fingerprint) {
        if let Some(sha) = repo.purge(r.id)? {
            let _ = store.release(&sha);
        }
        count += 1;
    }
    println!("purged {count} row(s) for `{fingerprint}`");
    Ok(())
}

async fn revoke(fingerprint: String) -> Result<()> {
    let paths = crate::cmd::paths::resolve()?;
    let db = Db::open(&paths.db_path)?;
    let repo = ProfileRepo::new(&db);
    match repo.active_for(&fingerprint)? {
        Some(row) => {
            repo.set_status(row.id, nid_storage::profile_repo::STATUS_QUARANTINED)?;
            println!(
                "quarantined active profile for `{fingerprint}` (id {})",
                row.id
            );
        }
        None => println!("no active profile for `{fingerprint}`"),
    }
    Ok(())
}

async fn list() -> Result<()> {
    let paths = crate::cmd::paths::resolve()?;
    paths.ensure()?;
    let db = Db::open(&paths.db_path)?;
    let persisted = ProfileRepo::new(&db).list()?;

    let bundled = nid_profiles::load_all();
    println!("Bundled profiles ({}):", bundled.len());
    for (_name, p) in &bundled {
        println!("  {} v{}", p.meta.fingerprint, p.meta.version);
    }
    if !persisted.is_empty() {
        println!("\nPersisted profiles ({}):", persisted.len());
        for p in &persisted {
            println!(
                "  {} v{} [{}] (id={})",
                p.fingerprint, p.version, p.status, p.id
            );
        }
    }
    Ok(())
}

async fn inspect(fingerprint: String) -> Result<()> {
    let bundled = nid_profiles::load_all();
    if let Some((_, p)) = bundled
        .iter()
        .find(|(_, p)| p.meta.fingerprint == fingerprint)
    {
        println!("{}", p.to_toml()?);
        return Ok(());
    }
    // Try the persisted store.
    let paths = crate::cmd::paths::resolve()?;
    let db = Db::open(&paths.db_path)?;
    let repo = ProfileRepo::new(&db);
    if let Some(row) = repo.active_for(&fingerprint)? {
        let store = BlobStore::new(&db, &paths.blobs_dir);
        let bytes = store.get(&row.dsl_blob_sha256)?;
        println!("{}", String::from_utf8_lossy(&bytes));
        return Ok(());
    }
    println!("no profile matching fingerprint `{fingerprint}`");
    Ok(())
}

async fn export(
    fingerprint: String,
    output: std::path::PathBuf,
    key: Option<std::path::PathBuf>,
) -> Result<()> {
    let bundled = nid_profiles::load_all();
    let profile = bundled
        .into_iter()
        .find(|(_, p)| p.meta.fingerprint == fingerprint)
        .map(|(_, p)| p)
        .with_context(|| format!("no bundled profile for fingerprint `{fingerprint}`"))?;

    let sk = match key {
        Some(path) => {
            let bytes =
                std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
            let bytes = if bytes.len() == 32 {
                bytes
            } else {
                let s = std::str::from_utf8(&bytes).unwrap_or("").trim();
                hex::decode(s).context("key file not raw 32 bytes or hex")?
            };
            let arr: [u8; 32] = bytes
                .as_slice()
                .try_into()
                .context("key must be 32 bytes")?;
            ed25519_dalek::SigningKey::from_bytes(&arr)
        }
        None => signing::generate_keypair(),
    };

    let mut f =
        std::fs::File::create(&output).with_context(|| format!("creating {}", output.display()))?;
    nidprofile::pack(&mut f, &profile, &sk)?;
    println!(
        "exported {} to {}; signer key-id = {}",
        profile.meta.fingerprint,
        output.display(),
        signing::key_id(&sk.verifying_key())
    );
    Ok(())
}

async fn import(path: std::path::PathBuf, allow_unsigned: bool) -> Result<()> {
    let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    let paths = crate::cmd::paths::resolve()?;
    paths.ensure()?;
    let db = Db::open(&paths.db_path)?;
    let trust = TrustRepo::new(&db);
    let trusted_ids = trust.active_ids()?;

    let unpacked = match nidprofile::unpack_and_verify(&bytes, &trusted_ids) {
        Ok(u) => u,
        Err(nidprofile::NidProfileError::UntrustedSigner(key)) if allow_unsigned => {
            if !confirm_allow_unsigned(&key)? {
                anyhow::bail!("untrusted import refused");
            }
            // Build an allow-list of exactly this signer and retry.
            nidprofile::unpack_and_verify(&bytes, &[key])
                .context("profile signature invalid even with --allow-unsigned")?
        }
        Err(e) => return Err(anyhow::anyhow!("profile import failed: {e}")),
    };

    let store = BlobStore::new(&db, &paths.blobs_dir);
    let dsl_sha = store.put(unpacked.profile.to_toml()?.as_bytes(), BlobKind::Dsl)?;
    let repo = ProfileRepo::new(&db);
    let id = repo.insert_pending(&NewProfile {
        fingerprint: unpacked.profile.meta.fingerprint.clone(),
        version: unpacked.profile.meta.version.clone(),
        provenance: PROV_IMPORTED.into(),
        synthesis_source: None,
        dsl_blob_sha256: dsl_sha,
        parent_fp: None,
        split_on_flag: None,
        signer_key_id: Some(unpacked.manifest.signer_key_id.clone()),
    })?;
    repo.promote(id)?;

    println!(
        "imported {} v{} (signer {}, id={})",
        unpacked.profile.meta.fingerprint,
        unpacked.profile.meta.version,
        unpacked.manifest.signer_key_id,
        id,
    );
    Ok(())
}

fn confirm_allow_unsigned(signer_key_id: &str) -> Result<bool> {
    if std::env::var("NID_UNTRUSTED_OK").ok().as_deref() == Some("1") {
        return Ok(true);
    }
    use std::io::Write;
    eprint!(
        "WARNING: profile is signed by key `{signer_key_id}` which is NOT in your trust keyring.\n\
         Importing an untrusted profile can execute arbitrary regex/DSL rules on your command\n\
         output. Type 'yes' to continue: "
    );
    std::io::stderr().flush().ok();
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    Ok(line.trim() == "yes")
}

use std::io::BufRead as _;
