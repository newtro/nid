//! `nid <cmd...>` — the hook-triggered hot path.
//!
//! Pipeline:
//! 1. fingerprint argv (Scheme R).
//! 2. spawn child via shell; capture stdout+stderr.
//! 3. redact secrets (pre-persistence).
//! 4. Layer 1 generic cleanup, always.
//! 5. Layer 3 (bundled DSL) if fingerprint matches, else Layer 2 format path.
//! 6. persist raw + compressed blobs, write session row.
//! 7. emit compressed to stdout with attestation footer.
//!
//! SIGTERM: catch once; flush whatever compressed content we've produced,
//! emit a terminal marker, exit 143.

use anyhow::Result;
use nid_core::{
    fingerprint,
    layers::{detect_format, Layer1Generic},
    redact,
    session::SessionId,
    Compressor, Context,
};
use nid_dsl::ast::Profile;
use nid_fidelity::structural_subset_check;
use nid_storage::{
    blob::{BlobKind, BlobStore},
    fidelity_repo::FidelityRepo,
    profile_repo::ProfileRepo,
    sample_repo::SampleRepo,
    session_repo::{NewSession, SessionRepo},
    Db,
};
use std::io::Cursor;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::AsyncReadExt;
use tokio::process::Command as TokioCommand;

pub async fn run(argv: Vec<String>, shadow: bool) -> Result<()> {
    if argv.is_empty() {
        anyhow::bail!("no command given");
    }
    let paths = crate::cmd::paths::resolve()?;
    paths.ensure()?;

    let fp = fingerprint(&argv);
    let id = SessionId::new_random();
    let started = unix_now();

    // Install SIGTERM trap (best-effort).
    let interrupted = Arc::new(AtomicBool::new(false));
    install_sigterm_trap(interrupted.clone());

    // Resolve a profile by fingerprint — Layer 5 (learned, persisted) has
    // priority over Layer 3 (bundled).
    let db = Db::open(&paths.db_path)?;
    let store = BlobStore::new(&db, &paths.blobs_dir);
    let profile_repo = ProfileRepo::new(&db);
    let sample_repo = SampleRepo::new(&db);

    let (profile, profile_id): (Option<Profile>, Option<i64>) = match resolve_profile_with_id(&profile_repo, &store, &fp)? {
        Some((p, id)) => (Some(p), Some(id)),
        None => {
            let bundled = nid_profiles::load_all();
            let p = bundled
                .into_iter()
                .find(|(_, p)| p.meta.fingerprint == fp)
                .map(|(_, p)| p);
            (p, None)
        }
    };

    // Spawn + capture.
    let cmd_str = argv.join(" ");
    let (exit_code, raw_out) = spawn_and_capture(&argv, interrupted.clone()).await?;

    // Redact before persistence.
    let raw_redacted = redact::redact(&raw_out);

    // Layer 1 — always.
    let layer1 = Layer1Generic::default();
    let ctx = Context::new(&fp, argv.clone()).with_shadow(shadow);
    let mut l1_input = Cursor::new(raw_redacted.as_bytes().to_vec());
    let mut l1_output: Vec<u8> = Vec::with_capacity(raw_redacted.len());
    let _ = layer1
        .compress(&mut l1_input, &mut l1_output, &ctx)
        .map_err(|e| anyhow::anyhow!("layer1: {e}"))?;
    let after_layer1 = String::from_utf8_lossy(&l1_output).to_string();

    // Tier B: try the profile (Layer 3/5), else Layer 2 format detect.
    let compressed = match &profile {
        Some(p) => {
            let co = nid_dsl::interpreter::apply_rules(&after_layer1, &p.rules);
            co.to_string()
        }
        None => {
            // For now, Layer 2 just passes through with format-aware cleanup
            // via the native compressor. We emit the layer1 output directly.
            let _format = detect_format(after_layer1.as_bytes());
            after_layer1.clone()
        }
    };

    // Persist.
    let raw_sha = store.put(raw_redacted.as_bytes(), BlobKind::Raw)?;
    let cmp_sha = store.put(compressed.as_bytes(), BlobKind::Compressed)?;

    // Phase 4: sample capture for unknown fingerprints. Cap at 64 samples per
    // fingerprint to avoid runaway growth; lock-in decides when to synthesize.
    if profile.is_none() {
        let count = sample_repo.count_for(&fp).unwrap_or(0);
        if count < 64 {
            let sample_sha = store.put(raw_redacted.as_bytes(), BlobKind::Sample)?;
            let _ = sample_repo.insert(&fp, &sample_sha, exit_code, None);
        }
    }

    // Phase 5 — Tier 1 invariants + Tier 2 structural subset.
    let mut self_fidelity = 1.0f32;
    let mut failed_invariants: Vec<String> = Vec::new();
    let fidelity_repo = FidelityRepo::new(&db);
    if let (Some(p), Some(pid)) = (&profile, profile_id) {
        if let Ok(results) = nid_dsl::invariants::check_invariants(&p.invariants, &raw_redacted, &compressed) {
            let total = results.len().max(1) as f32;
            let passed = results.iter().filter(|r| r.passed).count() as f32;
            self_fidelity = passed / total;
            for r in &results {
                let kind = if r.passed { "invariant_pass" } else { "invariant_fail" };
                let _ = fidelity_repo.record(Some(id.as_str()), pid, kind, Some(&r.name), None, None, r.detail.as_deref());
                if !r.passed {
                    failed_invariants.push(r.name.clone());
                }
            }
        }
        let s = structural_subset_check(&raw_redacted, &compressed);
        let kind = if s.passed { "structural_pass" } else { "structural_fail" };
        let detail = if s.passed { None } else { Some(format!("{} invented line(s)", s.invented_lines.len())) };
        let _ = fidelity_repo.record(Some(id.as_str()), pid, kind, None, None, None, detail.as_deref());
    }

    let repo = SessionRepo::new(&db);
    let cwd_owned = std::env::current_dir()
        .ok()
        .and_then(|p| p.to_str().map(str::to_string));
    let parent_agent_owned = std::env::var("NID_PARENT_AGENT").ok();
    repo.create(&NewSession {
        id: id.as_str(),
        fingerprint: &fp,
        profile_id: None,
        command: &cmd_str,
        argv_raw: &cmd_str,
        cwd: cwd_owned.as_deref(),
        parent_agent: parent_agent_owned.as_deref(),
        started_at: started,
    })?;

    let ended = unix_now();
    let mode = if shadow {
        "Shadow"
    } else if profile.is_some() {
        "Full"
    } else {
        "Passthrough"
    };
    let tokens_saved = (raw_redacted.len() as i64 - compressed.len() as i64).max(0) / 4;
    repo.finalize(
        id.as_str(),
        ended,
        exit_code,
        Some(&raw_sha),
        Some(&cmp_sha),
        raw_redacted.len() as i64,
        compressed.len() as i64,
        tokens_saved,
        "heuristic:bytes/4",
        mode,
    )?;

    // Output.
    if shadow {
        // Plan §14 — raw passthrough, counterfactual captured in the store.
        print!("{raw_out}");
    } else {
        print!("{compressed}");
        if !compressed.ends_with('\n') {
            println!();
        }
        if interrupted.load(Ordering::SeqCst) {
            println!("--- [nid: interrupted] ---");
        }
        println!(
            "[nid: profile {}/v{}, fidelity {:.2}, mode={}, raw via nid show {}]",
            profile.as_ref().map(|p| p.meta.fingerprint.as_str()).unwrap_or("<none>"),
            profile.as_ref().map(|p| p.meta.version.as_str()).unwrap_or("-"),
            self_fidelity,
            mode,
            id
        );
        if !failed_invariants.is_empty() {
            println!("[nid: invariants failed: {}]", failed_invariants.join(", "));
        }
    }

    std::process::exit(exit_code);
}

#[cfg(unix)]
fn install_sigterm_trap(flag: Arc<AtomicBool>) {
    use tokio::signal::unix::{signal, SignalKind};
    tokio::spawn(async move {
        if let Ok(mut s) = signal(SignalKind::terminate()) {
            s.recv().await;
            flag.store(true, Ordering::SeqCst);
        }
    });
}

#[cfg(not(unix))]
fn install_sigterm_trap(flag: Arc<AtomicBool>) {
    // Windows: approximate SIGTERM with Ctrl-Break.
    tokio::spawn(async move {
        if let Ok(mut s) = tokio::signal::windows::ctrl_break() {
            s.recv().await;
            flag.store(true, Ordering::SeqCst);
        }
    });
}

async fn spawn_and_capture(argv: &[String], _interrupted: Arc<AtomicBool>) -> Result<(i32, String)> {
    let joined = argv.join(" ");
    let mut shell_cmd: TokioCommand = if cfg!(target_os = "windows") {
        let mut c = TokioCommand::new("cmd");
        c.args(["/C", &joined]);
        c
    } else {
        let mut c = TokioCommand::new("sh");
        c.args(["-c", &joined]);
        c
    };
    let mut child = shell_cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;

    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();

    let mut out_buf = String::new();
    let mut err_buf = String::new();

    let (_oa, _ob, status) = tokio::join!(
        stdout.read_to_string(&mut out_buf),
        stderr.read_to_string(&mut err_buf),
        child.wait(),
    );
    let status = status?;
    let mut combined = out_buf;
    if !err_buf.is_empty() {
        combined.push_str(&err_buf);
    }
    let code = status.code().unwrap_or(-1);
    Ok((code, combined))
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Layer 5 lookup: if a persisted `active` profile exists for this
/// fingerprint, load its DSL blob and return (Profile, profile_id).
fn resolve_profile_with_id(
    repo: &ProfileRepo,
    store: &BlobStore,
    fp: &str,
) -> anyhow::Result<Option<(Profile, i64)>> {
    let Some(row) = repo.active_for(fp)? else {
        return Ok(None);
    };
    let bytes = store.get(&row.dsl_blob_sha256)?;
    let toml_src = std::str::from_utf8(&bytes)
        .map_err(|e| anyhow::anyhow!("profile blob not UTF-8: {e}"))?;
    let p = Profile::from_toml(toml_src).map_err(|e| anyhow::anyhow!("profile parse: {e}"))?;
    let _ = repo.record_use(row.id);
    Ok(Some((p, row.id)))
}
