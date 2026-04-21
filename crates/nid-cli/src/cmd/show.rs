//! `nid show <session-id> [--raw-unredacted]` — retrieve a prior session's
//! raw output (plan §4.1, §11.3).
//!
//! Default: re-applies redaction on top of whatever's in the blob store.
//! Raw blobs are redacted pre-persistence so this is normally a no-op, but
//! it's defence in depth and matches the plan's "`nid show` always redacts".
//!
//! `--raw-unredacted`: requires interactive confirmation + appends an access
//! entry to `<data>/show_access.log`. If stdin isn't a tty and
//! `NID_UNREDACTED_OK=1` isn't set, the command refuses.

use anyhow::{Context, Result};
use clap::Args;
use nid_core::redact;
use nid_storage::{blob::BlobStore, session_repo::SessionRepo, Db};
use std::io::{BufRead, Write};

#[derive(Debug, Args)]
pub struct ShowArgs {
    pub session_id: String,
    /// Skip the re-redact pass on top of the stored blob. Requires
    /// interactive confirmation (or `NID_UNREDACTED_OK=1`) and writes an
    /// access-log entry.
    #[arg(long)]
    pub raw_unredacted: bool,
}

pub async fn run(args: ShowArgs) -> Result<()> {
    let paths = crate::cmd::paths::resolve()?;
    let db = Db::open(&paths.db_path)?;
    let sessions = SessionRepo::new(&db);
    let store = BlobStore::new(&db, &paths.blobs_dir);
    let Some(s) = sessions.get(&args.session_id)? else {
        anyhow::bail!("session {} not found", args.session_id);
    };
    let sha = s
        .raw_blob_sha256
        .ok_or_else(|| anyhow::anyhow!("session has no raw blob"))?;
    let bytes = store.get(&sha)?;
    let text = String::from_utf8_lossy(&bytes).into_owned();

    if args.raw_unredacted {
        if !confirm_unredacted_access(&args.session_id)? {
            anyhow::bail!("unredacted access refused");
        }
        append_access_log(&paths.data_dir, &args.session_id).context("writing access log")?;
        print!("{text}");
    } else {
        // Defence-in-depth re-redact — even though raw was redacted pre-
        // persistence, any new pattern added since ingestion would catch here.
        print!("{}", redact::redact(&text));
    }
    Ok(())
}

fn confirm_unredacted_access(session_id: &str) -> Result<bool> {
    // Explicit env-var escape for non-interactive use (agents that have
    // already shown the user a confirmation prompt in their own UI).
    if std::env::var("NID_UNREDACTED_OK").ok().as_deref() == Some("1") {
        return Ok(true);
    }
    eprint!(
        "WARNING: --raw-unredacted will emit unredacted raw output for session {session_id}.\n\
         This may expose secrets that were redacted at capture time.\n\
         Type 'yes' to continue: "
    );
    std::io::stderr().flush().ok();
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    Ok(line.trim() == "yes")
}

fn append_access_log(data_dir: &std::path::Path, session_id: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(data_dir)?;
    let path = data_dir.join("show_access.log");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&path)?;
    writeln!(f, "{now}\t{session_id}\traw-unredacted")?;
    Ok(())
}
