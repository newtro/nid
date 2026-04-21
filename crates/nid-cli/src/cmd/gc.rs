use anyhow::Result;
use nid_storage::{blob::BlobStore, Db};

pub async fn run() -> Result<()> {
    let paths = crate::cmd::paths::resolve()?;
    let db = Db::open(&paths.db_path)?;
    let store = BlobStore::new(&db, &paths.blobs_dir);
    let freed = store.gc_orphans()?;
    println!("gc: freed {freed} bytes");
    Ok(())
}
