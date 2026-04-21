//! `nid show <session-id> [--raw-unredacted]` — retrieve a prior session's
//! raw output (plan §4.1, §11.1, §11.3).
//!
//! Raw blobs are stored AES-GCM-sealed and UNREDACTED (plan §11.1). On read:
//!
//! - default path: decrypt, then apply `redact::redact` before emitting.
//! - `--raw-unredacted`: decrypt, emit plaintext. Requires interactive
//!   "yes" confirmation on tty (or `NID_UNREDACTED_OK=1`). Appends an
//!   access entry to `<data>/show_access.log`.
//!
//! If the raw blob is not an AES-GCM payload (legacy redacted-only raws
//! from earlier nid versions), we fall back to treating it as plaintext
//! and re-redacting.

use anyhow::{Context, Result};
use clap::Args;
use nid_core::{redact, sealed};
use nid_storage::{blob::BlobStore, session_repo::SessionRepo, Db};
use std::io::{BufRead, Write};

#[derive(Debug, Args)]
pub struct ShowArgs {
    pub session_id: String,
    /// Emit the UNREDACTED raw. Requires interactive 'yes' confirmation
    /// (or NID_UNREDACTED_OK=1) and appends an access-log entry.
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

    let plaintext = match try_unseal(&bytes, &paths.local_key) {
        Some(s) => s,
        None => String::from_utf8_lossy(&bytes).into_owned(),
    };

    if args.raw_unredacted {
        if !confirm_unredacted_access(&args.session_id)? {
            anyhow::bail!("unredacted access refused");
        }
        append_access_log(&paths.data_dir, &args.session_id).context("writing access log")?;
        print!("{plaintext}");
    } else {
        print!("{}", redact::redact(&plaintext));
    }
    Ok(())
}

fn try_unseal(bytes: &[u8], key_path: &std::path::Path) -> Option<String> {
    if bytes.is_empty() || bytes[0] != 1 {
        return None;
    }
    let key = sealed::load_or_create_key(key_path).ok()?;
    let plaintext = sealed::open(bytes, &key).ok()?;
    Some(String::from_utf8_lossy(&plaintext).into_owned())
}

fn confirm_unredacted_access(session_id: &str) -> Result<bool> {
    if std::env::var("NID_UNREDACTED_OK").ok().as_deref() == Some("1") {
        return Ok(true);
    }
    eprint!(
        "WARNING: --raw-unredacted will emit unredacted raw output for session {session_id}.\n\
         This may expose secrets that were captured at session time.\n\
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
