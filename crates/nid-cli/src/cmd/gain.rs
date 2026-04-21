use anyhow::Result;
use clap::Args;
use nid_storage::{session_repo::SessionRepo, Db};

#[derive(Debug, Args)]
pub struct GainArgs {
    #[arg(long)]
    pub shadow: bool,
}

pub async fn run(args: GainArgs) -> Result<()> {
    let paths = crate::cmd::paths::resolve()?;
    let db = Db::open(&paths.db_path)?;
    let repo = SessionRepo::new(&db);
    let rows = repo.list_recent(10_000)?;

    let (mut raw, mut cmp, mut runs) = (0i64, 0i64, 0i64);
    for r in &rows {
        if args.shadow && r.mode.as_deref() != Some("Shadow") {
            continue;
        }
        raw += r.raw_bytes.unwrap_or(0);
        cmp += r.compressed_bytes.unwrap_or(0);
        runs += 1;
    }
    let saved_tokens = (raw - cmp).max(0) / 4;
    println!(
        "runs: {runs}  raw: {raw} B  compressed: {cmp} B  saved ~{saved_tokens} tokens"
    );
    Ok(())
}
