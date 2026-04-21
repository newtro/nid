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
    /// keyring (see `nid trust add`).
    Import { path: std::path::PathBuf },
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
        ProfilesCmd::Import { path } => import(path).await,
        other => {
            println!("subcommand {other:?} not yet implemented");
            Ok(())
        }
    }
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

async fn import(path: std::path::PathBuf) -> Result<()> {
    let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    let paths = crate::cmd::paths::resolve()?;
    paths.ensure()?;
    let db = Db::open(&paths.db_path)?;
    let trust = TrustRepo::new(&db);
    let trusted_ids = trust.active_ids()?;

    let unpacked = nidprofile::unpack_and_verify(&bytes, &trusted_ids)
        .context("profile import failed signature/trust check")?;

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
