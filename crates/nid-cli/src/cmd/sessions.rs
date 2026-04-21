use anyhow::Result;
use clap::Args;
use nid_storage::{session_repo::SessionRepo, Db};

#[derive(Debug, Args)]
pub struct SessionsArgs {
    #[arg(long, default_value_t = 20)]
    pub limit: i64,
}

pub async fn run(args: SessionsArgs) -> Result<()> {
    let paths = crate::cmd::paths::resolve()?;
    let db = Db::open(&paths.db_path)?;
    let repo = SessionRepo::new(&db);
    let rows = repo.list_recent(args.limit)?;
    for r in rows {
        println!(
            "{} {} exit={:?} raw={}B cmp={}B {:?}",
            r.id,
            r.fingerprint,
            r.exit_code,
            r.raw_bytes.unwrap_or(0),
            r.compressed_bytes.unwrap_or(0),
            r.mode,
        );
    }
    Ok(())
}
