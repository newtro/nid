//! `nid gc` — garbage-collect orphaned blobs + purge expired sessions
//! (plan §12.3).

use anyhow::Result;
use clap::Args;
use nid_storage::{blob::BlobStore, session_repo::SessionRepo, Db};

#[derive(Debug, Args)]
pub struct GcArgs {
    /// Sessions older than this many days get their raw/compressed blobs
    /// released and the session row deleted. Default: 14 (plan §11.3).
    #[arg(long, default_value_t = 14)]
    pub retention_days: u32,
    /// Print what would be deleted but don't actually delete.
    #[arg(long)]
    pub dry_run: bool,
}

pub async fn run(args: GcArgs) -> Result<()> {
    let paths = crate::cmd::paths::resolve()?;
    let db = Db::open(&paths.db_path)?;
    let store = BlobStore::new(&db, &paths.blobs_dir);
    let sessions = SessionRepo::new(&db);

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let cutoff = now - (args.retention_days as i64 * 86400);

    let mut sessions_purged = 0u64;
    let mut blobs_released = 0u64;

    if args.dry_run {
        // Count candidate rows only. We don't have a count-older-than helper,
        // so reuse list_recent with a generous limit and filter.
        let all = sessions.list_recent(1_000_000)?;
        let candidates: Vec<_> = all.into_iter().filter(|s| s.started_at < cutoff).collect();
        println!(
            "dry-run: would purge {} session(s) older than {} days",
            candidates.len(),
            args.retention_days
        );
        let freed_estimate: i64 = candidates.iter().filter_map(|s| s.raw_bytes).sum::<i64>()
            + candidates
                .iter()
                .filter_map(|s| s.compressed_bytes)
                .sum::<i64>();
        println!("dry-run: approximate bytes freed: {freed_estimate}");
        return Ok(());
    }

    let released = sessions.purge_older_than(cutoff)?;
    sessions_purged += released.len() as u64;
    for (raw_sha, cmp_sha) in released {
        if !raw_sha.is_empty() {
            let _ = store.release(&raw_sha);
            blobs_released += 1;
        }
        if !cmp_sha.is_empty() {
            let _ = store.release(&cmp_sha);
            blobs_released += 1;
        }
    }

    let freed = store.gc_orphans()?;
    println!(
        "gc: purged {sessions_purged} session(s) older than {} days, released {blobs_released} blob ref(s), reclaimed {freed} orphan byte(s)",
        args.retention_days
    );
    Ok(())
}

/// Opportunistic GC: if the current calendar day is newer than the last run,
/// fire a bounded-time GC pass. Called on every hot-path invocation.
/// Returns quickly (≤ 100ms budget per plan §12.3).
pub fn opportunistic(paths: &nid_storage::NidPaths) -> anyhow::Result<()> {
    let marker = paths.data_dir.join(".last_gc_day");
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

    // Stamp the marker BEFORE we do the work — we don't want to block the hot
    // path if GC is slow; subsequent invocations on the same day will skip.
    if let Some(parent) = marker.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(&marker, today.to_string()).ok();

    // Only run a blob orphan sweep here (fast). Full retention-driven purge is
    // `nid gc`.
    if let Ok(db) = Db::open(&paths.db_path) {
        let store = BlobStore::new(&db, &paths.blobs_dir);
        let _ = store.gc_orphans();
    }
    Ok(())
}
