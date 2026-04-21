//! `nid <cmd...>` — the hook-triggered hot path.
//!
//! Pipeline:
//! 1. fingerprint argv (Scheme R).
//! 2. spawn child via shell; capture stdout+stderr.
//! 3. redact secrets (pre-persistence).
//! 4. Layer 1 generic cleanup, always.
//! 5. Layer 3/5 DSL if fingerprint matches, else Layer 2 format path.
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
    // Load user config (falls back to defaults if missing/malformed).
    let cfg = nid_storage::config::load(&paths.config_dir);
    // Honour the shadow state file: `nid shadow enable` persists intent across
    // invocations independent of the --shadow flag.
    let shadow = shadow || crate::cmd::shadow::is_shadow_enabled(&paths.config_dir);
    // Opportunistic blob-orphan sweep (bounded; once per calendar day).
    let _ = crate::cmd::gc::opportunistic(&paths);

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

    let (profile, profile_id): (Option<Profile>, Option<i64>) =
        match resolve_profile_with_id(&profile_repo, &store, &fp)? {
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

    // Detect per-invocation bypass signals before we spawn.
    let bypass_signals = detect_bypass_signals(&argv, &fp, &paths);

    // Spawn + capture.
    let cmd_str = argv.join(" ");
    let (exit_code, raw_out) = spawn_and_capture(&argv, interrupted.clone()).await?;

    // Redact before persistence. Config `allow_commands` opts a command out;
    // `extra_patterns` adds to the built-ins.
    let bin = argv
        .first()
        .and_then(|p| std::path::Path::new(p).file_stem())
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let redaction_on = !cfg
        .security
        .redaction
        .allow_commands
        .iter()
        .any(|c| c == bin);
    let raw_redacted = if redaction_on {
        let mut out = redact::redact(&raw_out);
        for pat in &cfg.security.redaction.extra_patterns {
            if let Ok(re) = regex::Regex::new(pat) {
                out = re.replace_all(&out, "[REDACTED:user]").into_owned();
            }
        }
        out
    } else {
        raw_out.clone()
    };

    let preserve_raw =
        cfg.session.preserve_raw && !cfg.session.deny_raw_commands.iter().any(|c| c == bin);

    // Layer 1 — always.
    let layer1 = Layer1Generic::default();
    let ctx = Context::new(&fp, argv.clone()).with_shadow(shadow);
    let mut l1_input = Cursor::new(raw_redacted.as_bytes().to_vec());
    let mut l1_output: Vec<u8> = Vec::with_capacity(raw_redacted.len());
    let _ = layer1
        .compress(&mut l1_input, &mut l1_output, &ctx)
        .map_err(|e| anyhow::anyhow!("layer1: {e}"))?;
    let after_layer1 = String::from_utf8_lossy(&l1_output).to_string();

    // Tier B: profile (Layer 3/5), else Layer 2 format-aware cleanup.
    let compressed = match &profile {
        Some(p) => nid_dsl::interpreter::apply_rules(&after_layer1, &p.rules).to_string(),
        None => {
            let format = detect_format(after_layer1.as_bytes());
            let l2 = nid_core::layers::Layer2Format { format };
            let mut l2_input = Cursor::new(after_layer1.as_bytes().to_vec());
            let mut l2_output: Vec<u8> = Vec::with_capacity(after_layer1.len());
            match l2.compress(&mut l2_input, &mut l2_output, &ctx) {
                Ok(_) => String::from_utf8_lossy(&l2_output).to_string(),
                Err(_) => after_layer1.clone(),
            }
        }
    };

    // Persist.
    let raw_sha = if preserve_raw {
        Some(store.put(raw_redacted.as_bytes(), BlobKind::Raw)?)
    } else {
        None
    };
    let cmp_sha = store.put(compressed.as_bytes(), BlobKind::Compressed)?;

    // Sample capture + auto-synthesis on lock-in.
    let mut just_captured = false;
    if profile.is_none() {
        let count = sample_repo.count_for(&fp).unwrap_or(0);
        if count < 64 {
            let sample_sha = store.put(raw_redacted.as_bytes(), BlobKind::Sample)?;
            let _ = sample_repo.insert(&fp, &sample_sha, exit_code, None);
            just_captured = true;
        }
    }
    if just_captured && profile.is_none() {
        let _ = try_auto_synthesize(
            &fp,
            &sample_repo,
            &store,
            &profile_repo,
            cfg.synthesis.samples_to_lock,
            cfg.synthesis.fast_path_if_zero_variance,
        )
        .await;
    }

    // Phase 5 — Tier 1 invariants + Tier 2 structural subset + bypass signals.
    let mut self_fidelity = 1.0f32;
    let mut failed_invariants: Vec<String> = Vec::new();
    let fidelity_repo = FidelityRepo::new(&db);
    if let (Some(p), Some(pid)) = (&profile, profile_id) {
        if let Ok(results) =
            nid_dsl::invariants::check_invariants(&p.invariants, &raw_redacted, &compressed)
        {
            let total = results.len().max(1) as f32;
            let passed = results.iter().filter(|r| r.passed).count() as f32;
            self_fidelity = passed / total;
            for r in &results {
                let kind = if r.passed {
                    "invariant_pass"
                } else {
                    "invariant_fail"
                };
                let _ = fidelity_repo.record(
                    Some(id.as_str()),
                    pid,
                    kind,
                    Some(&r.name),
                    None,
                    None,
                    r.detail.as_deref(),
                );
                if !r.passed {
                    failed_invariants.push(r.name.clone());
                }
            }
        }
        let s = structural_subset_check(&raw_redacted, &compressed);
        let kind = if s.passed {
            "structural_pass"
        } else {
            "structural_fail"
        };
        let detail = if s.passed {
            None
        } else {
            Some(format!("{} invented line(s)", s.invented_lines.len()))
        };
        let _ = fidelity_repo.record(
            Some(id.as_str()),
            pid,
            kind,
            None,
            None,
            None,
            detail.as_deref(),
        );

        for sig in &bypass_signals {
            let weight = sig.weight();
            let name = format!("{sig:?}");
            let _ = fidelity_repo.record(
                Some(id.as_str()),
                pid,
                "bypass_signal",
                Some(&name),
                Some(weight as f64),
                Some(weight as f64),
                None,
            );
        }
    }

    let repo = SessionRepo::new(&db);
    let cwd_owned = std::env::current_dir()
        .ok()
        .and_then(|p| p.to_str().map(str::to_string));
    let parent_agent_owned = std::env::var("NID_PARENT_AGENT").ok();
    repo.create(&NewSession {
        id: id.as_str(),
        fingerprint: &fp,
        profile_id,
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
        raw_sha.as_deref(),
        Some(&cmp_sha),
        raw_redacted.len() as i64,
        compressed.len() as i64,
        tokens_saved,
        "heuristic:bytes/4",
        mode,
    )?;

    // Bump the gain-daily rollup (plan §12.1).
    let _ = repo.bump_gain_daily(
        raw_redacted.len() as i64,
        compressed.len() as i64,
        tokens_saved,
    );

    // Opportunistic exit-skew check (plan §8.3). Cheap (single SQL query);
    // runs only when we have a profile.
    if let Some(pid) = profile_id {
        let _ = maybe_record_exit_skew(&db, &fp, pid, &fidelity_repo);
    }

    // Opportunistic retention purge (plan §12.3) — bounded.
    let _ = opportunistic_retention_purge(&db, &store, &paths, cfg.session.retention_days);

    // Output.
    if shadow {
        // Plan §14 — emit the REDACTED raw, not the pre-redaction output.
        print!("{raw_redacted}");
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
            profile
                .as_ref()
                .map(|p| p.meta.fingerprint.as_str())
                .unwrap_or("<none>"),
            profile
                .as_ref()
                .map(|p| p.meta.version.as_str())
                .unwrap_or("-"),
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

async fn spawn_and_capture(
    argv: &[String],
    _interrupted: Arc<AtomicBool>,
) -> Result<(i32, String)> {
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

/// Cheap per-invocation bypass-signal detection (plan §8.2).
fn detect_bypass_signals(
    argv: &[String],
    fp: &str,
    paths: &nid_storage::NidPaths,
) -> Vec<nid_fidelity::BypassSignal> {
    use nid_fidelity::BypassSignal;
    let mut sigs = Vec::new();

    if std::env::var("NID_RAW").ok().as_deref() == Some("1") {
        sigs.push(BypassSignal::NidRawEnv);
    }

    let bin = argv
        .first()
        .and_then(|p| std::path::Path::new(p).file_stem())
        .and_then(|s| s.to_str())
        .unwrap_or("");
    if matches!(bin, "cat" | "head" | "tail" | "less" | "more")
        && argv
            .iter()
            .any(|a| a.contains("nid") || a.contains("sess_"))
    {
        sigs.push(BypassSignal::RawReFetch);
    }
    if matches!(bin, "grep" | "rg") {
        sigs.push(BypassSignal::GrepAfterRead);
    }

    if let Ok(db) = Db::open(&paths.db_path) {
        let sessions = SessionRepo::new(&db);
        if let Ok(recents) = sessions.list_recent(3) {
            let now = unix_now();
            if recents
                .iter()
                .any(|s| s.fingerprint == fp && (now - s.started_at) <= 30)
            {
                sigs.push(BypassSignal::NearDuplicateReInvocation);
            }
        }
    }

    sigs
}

/// Plan §8.3 exit-code skew detection. Counts sessions in each bucket and
/// computes the ratio only if both buckets have ≥ min_samples runs
/// (warmup). Records an `exit_code_skew` event when skew_factor > 2.0.
fn maybe_record_exit_skew(
    db: &Db,
    fingerprint: &str,
    profile_id: i64,
    fidelity_repo: &FidelityRepo,
) -> anyhow::Result<()> {
    let sessions = SessionRepo::new(db);
    let (succ_runs, succ_raw, succ_cmp, fail_runs, fail_raw, fail_cmp) =
        sessions.exit_bucket_aggregates(fingerprint)?;

    let report = nid_fidelity::exit_code_skew(
        succ_runs, succ_raw, succ_cmp, fail_runs, fail_raw, fail_cmp, 50,
    );
    if report.needs_restratified_resynthesis {
        let _ = fidelity_repo.record(
            None,
            profile_id,
            "exit_code_skew",
            None,
            Some(report.skew_factor as f64),
            None,
            Some(&format!(
                "success_ratio={:.3} failure_ratio={:.3}",
                report.success_ratio, report.failure_ratio
            )),
        );
    }
    Ok(())
}

/// Cheap per-invocation retention sweep (plan §12.3). We purge at most a
/// handful of old sessions per call to stay within the 100ms budget.
fn opportunistic_retention_purge(
    db: &Db,
    store: &BlobStore,
    paths: &nid_storage::NidPaths,
    retention_days: u32,
) -> anyhow::Result<()> {
    // Only run once per day.
    let marker = paths.data_dir.join(".last_retention_day");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let today = now / 86400;
    let last = std::fs::read_to_string(&marker)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0);
    if last >= today {
        return Ok(());
    }
    let _ = std::fs::write(&marker, today.to_string());

    let cutoff = now as i64 - (retention_days as i64 * 86400);
    let sessions = SessionRepo::new(db);
    let released = sessions.purge_older_than(cutoff)?;
    for (raw, cmp) in released.iter().take(256) {
        if !raw.is_empty() {
            let _ = store.release(raw);
        }
        if !cmp.is_empty() {
            let _ = store.release(cmp);
        }
    }
    Ok(())
}

/// Auto-synthesis on lock-in (plan §7.3).
async fn try_auto_synthesize(
    fp: &str,
    samples_repo: &SampleRepo<'_>,
    store: &BlobStore<'_>,
    profile_repo: &ProfileRepo<'_>,
    lock_in_n: usize,
    fast_path_zv: bool,
) -> anyhow::Result<()> {
    let rows = samples_repo.for_fingerprint(fp)?;
    let mut samples: Vec<String> = Vec::with_capacity(rows.len());
    for r in &rows {
        if let Ok(b) = store.get(&r.sample_blob_sha256) {
            samples.push(String::from_utf8_lossy(&b).into_owned());
        }
    }

    let verdict = nid_synthesis::lockin::should_lock_in(&samples, lock_in_n, fast_path_zv);
    if !verdict.should_lock {
        return Ok(());
    }
    if profile_repo.active_for(fp)?.is_some() {
        return Ok(());
    }

    let backend = nid_synthesis::autodetect();
    let out =
        nid_synthesis::orchestrator::synthesize_from_samples(fp, &samples, |prompt| async move {
            backend.refine(&prompt).await
        })
        .await?;

    nid_dsl::validator::validate_profile(&out.profile)?;
    for s in &samples {
        let compressed = nid_dsl::interpreter::apply_rules(s, &out.profile.rules).to_string();
        let results =
            nid_dsl::invariants::check_invariants(&out.profile.invariants, s, &compressed)?;
        for r in &results {
            if !r.passed {
                tracing::warn!(
                    fp = fp,
                    invariant = r.name,
                    "auto-synthesis invariant failed on sample; keeping fingerprint unlearned"
                );
                return Ok(());
            }
        }
    }

    let toml_bytes = out.profile.to_toml()?.into_bytes();
    let dsl_sha = store.put(&toml_bytes, BlobKind::Dsl)?;
    let id = profile_repo.insert_pending(&nid_storage::profile_repo::NewProfile {
        fingerprint: fp.to_string(),
        version: out.profile.meta.version.clone(),
        provenance: nid_storage::profile_repo::PROV_SYNTHESIZED.into(),
        synthesis_source: Some("structural_diff".into()),
        dsl_blob_sha256: dsl_sha,
        parent_fp: None,
        split_on_flag: None,
        signer_key_id: None,
    })?;
    profile_repo.promote(id)?;
    tracing::info!(fp = fp, id = id, "auto-synthesized profile locked in");
    Ok(())
}

/// Layer 5 lookup.
fn resolve_profile_with_id(
    repo: &ProfileRepo,
    store: &BlobStore,
    fp: &str,
) -> anyhow::Result<Option<(Profile, i64)>> {
    let Some(row) = repo.active_for(fp)? else {
        return Ok(None);
    };
    let bytes = store.get(&row.dsl_blob_sha256)?;
    let toml_src =
        std::str::from_utf8(&bytes).map_err(|e| anyhow::anyhow!("profile blob not UTF-8: {e}"))?;
    let p = Profile::from_toml(toml_src).map_err(|e| anyhow::anyhow!("profile parse: {e}"))?;
    let _ = repo.record_use(row.id);
    Ok(Some((p, row.id)))
}
