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

    // Spawn + capture — bounded by `session.max_total_mb` (plan §11.3).
    let cmd_str = argv.join(" ");
    // Per-invocation cap: if max_total_mb is N, a single capture of > N bytes
    // is nonsensical — cap at N MiB. Disable by setting max_total_mb = 0.
    let capture_cap_bytes = if cfg.session.max_total_mb == 0 {
        None
    } else {
        Some((cfg.session.max_total_mb as usize).saturating_mul(1024 * 1024))
    };
    let (exit_code, raw_out, _capture_truncated) =
        spawn_and_capture(&argv, interrupted.clone(), capture_cap_bytes).await?;

    // Redaction (plan §11.3):
    // - `allow_commands` opts a command out of redaction entirely.
    // - `deny_commands` forces aggressive-redact (extra high-entropy sweep +
    //   always applies even if `allow_commands` was a bad-faith entry).
    // - `extra_patterns` adds to the built-ins.
    let bin = argv
        .first()
        .and_then(|p| std::path::Path::new(p).file_stem())
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let allow_for_bin = cfg
        .security
        .redaction
        .allow_commands
        .iter()
        .any(|c| c == bin);
    let deny_for_bin = cfg
        .security
        .redaction
        .deny_commands
        .iter()
        .any(|c| c == bin);
    // `deny_commands` wins over `allow_commands` — you can't opt a dangerous
    // command out of redaction by mistake.
    let redaction_on = !allow_for_bin || deny_for_bin;
    let raw_redacted = if redaction_on {
        let mut out = redact::redact(&raw_out);
        for pat in &cfg.security.redaction.extra_patterns {
            if let Ok(re) = regex::Regex::new(pat) {
                out = re.replace_all(&out, "[REDACTED:user]").into_owned();
            }
        }
        if deny_for_bin {
            // Aggressive additional sweep: redact any token ≥ 24 chars from
            // base64url alphabet, regardless of Shannon entropy. This is
            // overzealous on purpose — `deny_commands` is opt-in.
            let re = regex::Regex::new(r"\b[A-Za-z0-9_+/=-]{24,}\b").unwrap();
            out = re
                .replace_all(&out, |_: &regex::Captures| "[REDACTED:deny]".to_string())
                .into_owned();
        }
        out
    } else {
        raw_out.clone()
    };

    // `deny_raw_commands` — don't persist raw for this command.
    // `allow_raw_commands` — force persist even if deny list or global
    // `preserve_raw=false` would have blocked it (plan §11.3).
    let deny_raw = cfg.session.deny_raw_commands.iter().any(|c| c == bin);
    let force_raw = cfg.session.allow_raw_commands.iter().any(|c| c == bin);
    let preserve_raw = force_raw || (cfg.session.preserve_raw && !deny_raw);

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
    // DSL is budgeted (plan §11.4) — a pathological profile aborts into
    // Layer-1-only output instead of hanging the wrapped command (C4 fix).
    let mut budget_aborted = false;
    let compressed = match &profile {
        Some(p) => {
            let out = nid_dsl::interpreter::apply_rules_with_budget(
                &after_layer1,
                &p.rules,
                nid_dsl::Budget::default(),
            );
            if out.budget_aborted {
                budget_aborted = true;
                tracing::warn!(
                    fp = %fp,
                    "DSL budget aborted mid-run; degrading to Layer-1 output"
                );
                after_layer1.clone()
            } else {
                out.to_string()
            }
        }
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

    // Persist. Plan §11.1 — raw is stored UNREDACTED, sealed with AES-GCM
    // using a machine-local key. `nid show` decrypts + re-redacts on read
    // by default; only `--raw-unredacted` (with confirmation) emits
    // plaintext. This makes the flag semantically meaningful (H-R4 fix).
    //
    // Samples are persisted REDACTED because they feed the synthesis
    // prompt path, which we don't want to leak secrets into.
    let raw_sha = if preserve_raw {
        let key = nid_core::sealed::load_or_create_key(&paths.local_key)
            .map_err(|e| anyhow::anyhow!("sealed-key init: {e}"))?;
        let sealed_bytes = nid_core::sealed::seal(raw_out.as_bytes(), &key)
            .map_err(|e| anyhow::anyhow!("seal: {e}"))?;
        Some(store.put(&sealed_bytes, BlobKind::Raw)?)
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
            &paths,
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

        // Plan §11.4 — if the DSL budget aborted this run, the profile is
        // pathological. Quarantine it so it stops being dispatched, and
        // record a fidelity_event for audit.
        if budget_aborted {
            let _ = profile_repo.set_status(pid, nid_storage::profile_repo::STATUS_QUARANTINED);
            let _ = fidelity_repo.record(
                Some(id.as_str()),
                pid,
                "dsl_budget_exceeded",
                None,
                None,
                None,
                Some("DSL execution budget exceeded; profile quarantined"),
            );
        }

        // Rolling bypass score → quarantine the profile if it's been
        // gamed past threshold (plan §8.2). Honour the warmup window.
        let observed = fidelity_repo.distinct_sessions_for(pid).unwrap_or(0) as usize;
        if observed > cfg.fidelity.bypass_warmup_runs {
            let (score, _n) = fidelity_repo
                .rolling_bypass_score(pid, 100)
                .unwrap_or((0.0, 0));
            if score as f32 > cfg.fidelity.bypass_threshold {
                let _ = profile_repo.set_status(pid, nid_storage::profile_repo::STATUS_QUARANTINED);
                let _ = fidelity_repo.record(
                    Some(id.as_str()),
                    pid,
                    "bypass_threshold_exceeded",
                    None,
                    Some(score),
                    None,
                    Some(&format!(
                        "rolling score {score:.3} > threshold {}",
                        cfg.fidelity.bypass_threshold
                    )),
                );
                tracing::warn!(
                    fp = %fp,
                    profile_id = pid,
                    score = score,
                    "profile quarantined: rolling bypass score exceeded threshold"
                );
            }
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

/// Spawn a shell command, capture stdout+stderr up to `cap_bytes` (None = no
/// cap). Returns (exit_code, combined_output, truncated_flag).
/// Bounded capture prevents OOM on pathological command output (plan §11.3
/// `max_total_mb`).
async fn spawn_and_capture(
    argv: &[String],
    _interrupted: Arc<AtomicBool>,
    cap_bytes: Option<usize>,
) -> Result<(i32, String, bool)> {
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

    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();

    // Per-stream half-cap so stdout and stderr can't together breach cap_bytes.
    let half = cap_bytes.map(|n| n / 2);
    let out_fut = read_capped(stdout, half);
    let err_fut = read_capped(stderr, half);
    let (out_res, err_res, status_res) = tokio::join!(out_fut, err_fut, child.wait());
    let (out_buf, out_trunc) = out_res?;
    let (err_buf, err_trunc) = err_res?;
    let status = status_res?;

    let mut combined = out_buf;
    if !err_buf.is_empty() {
        combined.push_str(&err_buf);
    }
    let truncated = out_trunc || err_trunc;
    if truncated {
        combined.push_str("\n--- [nid: output truncated at capture cap] ---\n");
    }
    let code = status.code().unwrap_or(-1);
    Ok((code, combined, truncated))
}

async fn read_capped<R: tokio::io::AsyncRead + Unpin>(
    mut r: R,
    cap_bytes: Option<usize>,
) -> std::io::Result<(String, bool)> {
    use tokio::io::AsyncReadExt as _;
    let mut buf = Vec::with_capacity(4096);
    let mut truncated = false;
    let mut chunk = [0u8; 4096];
    loop {
        let n = r.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        if let Some(cap) = cap_bytes {
            if buf.len() + n > cap {
                let take = cap.saturating_sub(buf.len());
                buf.extend_from_slice(&chunk[..take]);
                truncated = true;
                // Drain remaining output without buffering it, otherwise the
                // child can block on a full pipe.
                while r.read(&mut chunk).await? > 0 {}
                break;
            }
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    Ok((String::from_utf8_lossy(&buf).into_owned(), truncated))
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
    // Release all blob refs for purged sessions (no arbitrary cap). The
    // 100ms opportunistic budget is enforced by the once-per-day marker,
    // not by partially releasing blobs — leaving refs dangling between
    // runs was a real storage-leak bug.
    for (raw, cmp) in &released {
        if !raw.is_empty() {
            let _ = store.release(raw);
        }
        if !cmp.is_empty() {
            let _ = store.release(cmp);
        }
    }
    Ok(())
}

/// Per-fingerprint advisory lock backed by an O_EXCL|O_CREAT file.
/// Dropped on Drop (removes the file). Stale locks older than 10 min are
/// ignored to prevent permanent deadlock if a previous run crashed.
struct SynthLock {
    path: std::path::PathBuf,
}

impl SynthLock {
    fn try_acquire(paths: &nid_storage::NidPaths, fp: &str) -> Option<Self> {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(fp.as_bytes());
        let name = hex::encode(&h.finalize()[..8]);
        let dir = paths.data_dir.join("synth_locks");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(format!("{name}.lock"));

        // If the lock is stale (>10 min old), clean it.
        if let Ok(md) = std::fs::metadata(&path) {
            if let Ok(modified) = md.modified() {
                if std::time::SystemTime::now()
                    .duration_since(modified)
                    .map(|d| d.as_secs() > 600)
                    .unwrap_or(false)
                {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }

        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => Some(SynthLock { path }),
            Err(_) => None,
        }
    }
}

impl Drop for SynthLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Auto-synthesis on lock-in (plan §7.3).
async fn try_auto_synthesize(
    fp: &str,
    samples_repo: &SampleRepo<'_>,
    store: &BlobStore<'_>,
    profile_repo: &ProfileRepo<'_>,
    lock_in_n: usize,
    fast_path_zv: bool,
    paths: &nid_storage::NidPaths,
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

    // Cross-process advisory lock so two concurrent nid invocations don't
    // both run synthesis for the same fingerprint (M-R3). Lock file is
    // per-fingerprint (hash of the fp into a filename).
    let _guard = match SynthLock::try_acquire(paths, fp) {
        Some(g) => g,
        None => {
            tracing::info!(fp = fp, "auto-synthesis already in progress; skipping");
            return Ok(());
        }
    };
    // Re-check after acquiring the lock — the winning process may have
    // already promoted a profile.
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
        // Budgeted self-test: a synthesized profile that aborts the budget
        // on any of its own training samples is pathological and must not
        // be promoted.
        let co = nid_dsl::interpreter::apply_rules_with_budget(
            s,
            &out.profile.rules,
            nid_dsl::Budget::default(),
        );
        if co.budget_aborted {
            tracing::warn!(
                fp = fp,
                "auto-synthesis budget aborted on a training sample; rejecting"
            );
            return Ok(());
        }
        let compressed = co.to_string();
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
