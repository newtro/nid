//! Content-addressed blob store: SHA-256 keys, zstd-compressed payloads,
//! ref-counted via the `blobs` table.

use crate::db::{Db, DbError};
use rusqlite::params;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Kind tag recorded on each blob. Matches plan §12.1 (`blobs.kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlobKind {
    Dsl,
    Rubric,
    Sample,
    Compressed,
    Raw,
    Signature,
}

impl BlobKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            BlobKind::Dsl => "dsl",
            BlobKind::Rubric => "rubric",
            BlobKind::Sample => "sample",
            BlobKind::Compressed => "compressed",
            BlobKind::Raw => "raw",
            BlobKind::Signature => "signature",
        }
    }
}

pub struct BlobStore<'a> {
    db: &'a Db,
    root: PathBuf,
}

impl<'a> BlobStore<'a> {
    pub fn new(db: &'a Db, root: &Path) -> Self {
        Self {
            db,
            root: root.to_path_buf(),
        }
    }

    fn blob_path(&self, sha256: &str) -> PathBuf {
        self.root.join(format!("sha256-{sha256}.zst"))
    }

    /// Compute SHA-256 of payload.
    pub fn hash(data: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(data);
        hex::encode(h.finalize())
    }

    /// Insert a blob. If the hash already exists, bump ref_count only.
    /// Returns the sha256 key.
    pub fn put(&self, data: &[u8], kind: BlobKind) -> Result<String, DbError> {
        let sha = Self::hash(data);
        fs::create_dir_all(&self.root)?;

        let existed = self.db.with_conn(|c| {
            let n: i64 = c.query_row(
                "SELECT COUNT(*) FROM blobs WHERE sha256 = ?1",
                [&sha],
                |r| r.get(0),
            )?;
            if n > 0 {
                c.execute(
                    "UPDATE blobs SET ref_count = ref_count + 1 WHERE sha256 = ?1",
                    [&sha],
                )?;
                Ok(true)
            } else {
                Ok(false)
            }
        })?;

        if !existed {
            let path = self.blob_path(&sha);
            let compressed = zstd::bulk::compress(data, 3).map_err(std::io::Error::from)?;
            // Atomic write: tmp → rename.
            let tmp = path.with_extension("tmp");
            {
                let mut f = fs::File::create(&tmp)?;
                f.write_all(&compressed)?;
                f.sync_all()?;
            }
            fs::rename(&tmp, &path)?;

            let now = unix_now();
            self.db.with_conn(|c| {
                c.execute(
                    "INSERT INTO blobs(sha256, kind, size, created_at, ref_count)
                     VALUES(?1, ?2, ?3, ?4, 1)",
                    params![sha, kind.as_str(), data.len() as i64, now],
                )?;
                Ok(())
            })?;
        }
        Ok(sha)
    }

    /// Retrieve and decompress a blob.
    pub fn get(&self, sha256: &str) -> Result<Vec<u8>, DbError> {
        let path = self.blob_path(sha256);
        let bytes = fs::read(&path)?;
        let out = zstd::bulk::decompress(&bytes, 256 * 1024 * 1024)
            .map_err(std::io::Error::from)?;
        // Verify integrity — content-addressed means the hash must match.
        let got = Self::hash(&out);
        if got != sha256 {
            return Err(DbError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("blob hash mismatch: expected {sha256}, got {got}"),
            )));
        }
        Ok(out)
    }

    /// Decrement ref_count; if it drops to zero, remove the blob file.
    pub fn release(&self, sha256: &str) -> Result<(), DbError> {
        let maybe_rm = self.db.with_conn(|c| {
            c.execute(
                "UPDATE blobs SET ref_count = ref_count - 1
                 WHERE sha256 = ?1 AND ref_count > 0",
                [sha256],
            )?;
            let rc: i64 = c
                .query_row(
                    "SELECT ref_count FROM blobs WHERE sha256 = ?1",
                    [sha256],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            if rc <= 0 {
                c.execute("DELETE FROM blobs WHERE sha256 = ?1", [sha256])?;
                Ok(true)
            } else {
                Ok(false)
            }
        })?;
        if maybe_rm {
            let path = self.blob_path(sha256);
            if path.exists() {
                fs::remove_file(&path)?;
            }
        }
        Ok(())
    }

    /// Run garbage-collection for any blob whose ref_count <= 0 that somehow
    /// still has a file on disk. Returns bytes reclaimed.
    pub fn gc_orphans(&self) -> Result<u64, DbError> {
        let orphans: Vec<(String, i64)> = self.db.with_conn(|c| {
            let mut s = c.prepare(
                "SELECT sha256, size FROM blobs WHERE ref_count <= 0",
            )?;
            let rows = s
                .query_map([], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })?;
        let mut freed = 0u64;
        for (sha, size) in orphans {
            let path = self.blob_path(&sha);
            if path.exists() {
                fs::remove_file(&path)?;
                freed += size as u64;
            }
            self.db.with_conn(|c| {
                c.execute("DELETE FROM blobs WHERE sha256 = ?1", [sha])?;
                Ok(())
            })?;
        }
        Ok(freed)
    }

    /// Total bytes of stored blobs (from sqlite `blobs.size`).
    pub fn total_bytes(&self) -> Result<u64, DbError> {
        let v: i64 = self.db.with_conn(|c| {
            c.query_row("SELECT COALESCE(SUM(size), 0) FROM blobs", [], |r| r.get(0))
        })?;
        Ok(v.max(0) as u64)
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn put_then_get_roundtrips() {
        let tmp = TempDir::new().unwrap();
        let db = Db::open_in_memory().unwrap();
        let bs = BlobStore::new(&db, tmp.path());
        let sha = bs.put(b"hello nid", BlobKind::Raw).unwrap();
        assert_eq!(sha, BlobStore::hash(b"hello nid"));
        let got = bs.get(&sha).unwrap();
        assert_eq!(&got, b"hello nid");
    }

    #[test]
    fn put_is_idempotent_and_refcounts() {
        let tmp = TempDir::new().unwrap();
        let db = Db::open_in_memory().unwrap();
        let bs = BlobStore::new(&db, tmp.path());
        let a = bs.put(b"same", BlobKind::Sample).unwrap();
        let b = bs.put(b"same", BlobKind::Sample).unwrap();
        assert_eq!(a, b);
        let rc: i64 = db
            .with_conn(|c| {
                c.query_row("SELECT ref_count FROM blobs WHERE sha256=?1", [&a], |r| {
                    r.get(0)
                })
            })
            .unwrap();
        assert_eq!(rc, 2);
    }

    #[test]
    fn release_removes_when_refcount_zero() {
        let tmp = TempDir::new().unwrap();
        let db = Db::open_in_memory().unwrap();
        let bs = BlobStore::new(&db, tmp.path());
        let sha = bs.put(b"ephemeral", BlobKind::Compressed).unwrap();
        let path = bs.blob_path(&sha);
        assert!(path.exists());
        bs.release(&sha).unwrap();
        assert!(!path.exists());
        let n: i64 = db
            .with_conn(|c| c.query_row("SELECT COUNT(*) FROM blobs", [], |r| r.get(0)))
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn release_decrements_without_removing_when_refcount_positive() {
        let tmp = TempDir::new().unwrap();
        let db = Db::open_in_memory().unwrap();
        let bs = BlobStore::new(&db, tmp.path());
        let _ = bs.put(b"shared", BlobKind::Sample).unwrap();
        let sha = bs.put(b"shared", BlobKind::Sample).unwrap(); // rc=2
        bs.release(&sha).unwrap();
        assert!(bs.blob_path(&sha).exists());
        let rc: i64 = db
            .with_conn(|c| {
                c.query_row("SELECT ref_count FROM blobs WHERE sha256=?1", [&sha], |r| {
                    r.get(0)
                })
            })
            .unwrap();
        assert_eq!(rc, 1);
    }
}
