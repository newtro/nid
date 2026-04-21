//! `nid <cmd...>` — the hook-triggered hot path.
//!
//! Phase 1 scaffold: fingerprint argv, resolve an (optional) bundled profile,
//! spawn the child, capture output, apply Layer 1 + (if found) Layer 3 DSL,
//! persist raw, write a session record. The full streaming pipeline and
//! SIGTERM handling land in Phase 2. This scaffold already does real work:
//! real child-spawn, real capture, real redaction, real DSL application when
//! a matching bundled profile exists.

use anyhow::Result;
use nid_core::{fingerprint, redact, session::SessionId};
use nid_storage::{
    blob::{BlobKind, BlobStore},
    session_repo::{NewSession, SessionRepo},
    Db,
};
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

    // Resolve the bundled profile by fingerprint, if any.
    let bundled = nid_profiles::load_all();
    let profile = bundled.into_iter().find(|(_, p)| p.meta.fingerprint == fp).map(|(_, p)| p);

    // Spawn the command.
    let cmd_str = argv.join(" ");
    let (exit_code, raw_out) = spawn_and_capture(&argv).await?;

    // Redact before persistence.
    let raw_redacted = redact::redact(&raw_out);

    // Apply DSL if we have one. Layer 1 strip_ansi / dedup come in via the
    // profile's own rules or via the generic Layer 1 in Phase 2.
    let compressed = match &profile {
        Some(p) => nid_dsl::interpreter::apply_rules(&raw_redacted, &p.rules).to_string(),
        None => raw_redacted.clone(),
    };

    // Persist raw blob + session row.
    let db = Db::open(&paths.db_path)?;
    let store = BlobStore::new(&db, &paths.blobs_dir);
    let raw_sha = store.put(raw_redacted.as_bytes(), BlobKind::Raw)?;
    let cmp_sha = store.put(compressed.as_bytes(), BlobKind::Compressed)?;

    let repo = SessionRepo::new(&db);
    repo.create(&NewSession {
        id: id.as_str(),
        fingerprint: &fp,
        profile_id: None,
        command: &cmd_str,
        argv_raw: &cmd_str,
        cwd: std::env::current_dir().ok().and_then(|p| p.to_str().map(str::to_string)).as_deref(),
        parent_agent: std::env::var("NID_PARENT_AGENT").ok().as_deref(),
        started_at: started,
    })?;

    let ended = unix_now();
    let mode = if shadow { "Shadow" } else if profile.is_some() { "Full" } else { "Passthrough" };
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

    // In shadow mode, emit raw unchanged (plan §14).
    if shadow {
        print!("{raw_out}");
    } else {
        print!("{compressed}");
        println!(
            "[nid: profile {}/v{}, mode={}, raw via nid show {}]",
            profile.as_ref().map(|p| p.meta.fingerprint.as_str()).unwrap_or("<none>"),
            profile.as_ref().map(|p| p.meta.version.as_str()).unwrap_or("-"),
            mode,
            id
        );
    }

    std::process::exit(exit_code);
}

async fn spawn_and_capture(argv: &[String]) -> Result<(i32, String)> {
    // Join argv and spawn via the system shell so pipelines work. Plan §4.4.2
    // — whole-pipeline wrap.
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
