//! `nid synthesize <cmd>` — kick off synthesis for a fingerprint.

use anyhow::Result;
use clap::Args;
use nid_dsl::{ast::Profile, validator};
use nid_storage::{
    blob::{BlobKind, BlobStore},
    profile_repo::{NewProfile, ProfileRepo, PROV_SYNTHESIZED},
    sample_repo::SampleRepo,
    Db,
};
use nid_synthesis::{autodetect, lockin, orchestrator::synthesize_from_samples};

#[derive(Debug, Args)]
pub struct SynthesizeArgs {
    pub command: Vec<String>,
    #[arg(long)]
    pub force: bool,
}

pub async fn run(args: SynthesizeArgs) -> Result<()> {
    if args.command.is_empty() {
        anyhow::bail!("synthesize requires a command");
    }
    let paths = crate::cmd::paths::resolve()?;
    paths.ensure()?;

    let fp = nid_core::fingerprint(&args.command);
    let db = Db::open(&paths.db_path)?;
    let store = BlobStore::new(&db, &paths.blobs_dir);
    let samples_repo = SampleRepo::new(&db);
    let profile_repo = ProfileRepo::new(&db);

    let sample_rows = samples_repo.for_fingerprint(&fp)?;
    let samples: Vec<String> = sample_rows
        .iter()
        .map(|r| {
            let b = store.get(&r.sample_blob_sha256).unwrap_or_default();
            String::from_utf8_lossy(&b).into_owned()
        })
        .collect();

    if samples.is_empty() {
        println!("no samples for fingerprint `{fp}`");
        return Ok(());
    }

    let verdict = lockin::should_lock_in(&samples, 5, true);
    if !args.force && !verdict.should_lock {
        println!(
            "only {} sample(s) — need 5 (or 3 identical). pass --force to override.",
            samples.len()
        );
        return Ok(());
    }

    println!(
        "synthesizing from {} sample(s) for `{fp}`...",
        samples.len()
    );

    // Auto-detect a configured LLM backend; fall back to structural-diff-only.
    let backend = autodetect();
    println!("  backend: {:?}", backend.kind());
    let out = synthesize_from_samples(&fp, &samples, |prompt| async move {
        backend.refine(&prompt).await
    })
    .await?;
    validator::validate_profile(&out.profile)?;

    // Self-test: compressing each sample with this profile must pass every
    // listed invariant.
    for (i, s) in samples.iter().enumerate() {
        let compressed = nid_dsl::interpreter::apply_rules(s, &out.profile.rules).to_string();
        let results =
            nid_dsl::invariants::check_invariants(&out.profile.invariants, s, &compressed)?;
        for r in &results {
            if !r.passed {
                anyhow::bail!(
                    "invariant `{}` failed on sample {}: {:?}",
                    r.name,
                    i,
                    r.detail
                );
            }
        }
    }

    // Persist + promote.
    let toml_bytes = out.profile.to_toml()?.into_bytes();
    let dsl_sha = store.put(&toml_bytes, BlobKind::Dsl)?;
    let id = profile_repo.insert_pending(&NewProfile {
        fingerprint: fp.clone(),
        version: out.profile.meta.version.clone(),
        provenance: PROV_SYNTHESIZED.into(),
        synthesis_source: Some("structural_diff".into()),
        dsl_blob_sha256: dsl_sha,
        parent_fp: None,
        split_on_flag: None,
        signer_key_id: None,
    })?;
    profile_repo.promote(id)?;

    println!(
        "synthesized and promoted profile `{fp}` v{} (id={})",
        out.profile.meta.version, id
    );
    Ok(())
}

#[allow(dead_code)]
fn _profile_unused_ref(_: &Profile) {}
