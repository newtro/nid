use anyhow::Result;
use clap::Args;
use nid_storage::{blob::BlobStore, session_repo::SessionRepo, Db};

#[derive(Debug, Args)]
pub struct ShowArgs {
    pub session_id: String,
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
    let text = String::from_utf8_lossy(&bytes);
    print!("{text}");
    Ok(())
}
