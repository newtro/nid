//! CRUD on the `samples` table — Phase 4 sample capture for synthesis.

use crate::db::{Db, DbError};
use rusqlite::params;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SampleRow {
    pub id: i64,
    pub fingerprint: String,
    pub sample_blob_sha256: String,
    pub exit_code: i32,
    pub captured_at: i64,
    pub shape_class: Option<String>,
}

pub struct SampleRepo<'a> {
    db: &'a Db,
}

impl<'a> SampleRepo<'a> {
    pub fn new(db: &'a Db) -> Self {
        Self { db }
    }

    pub fn insert(
        &self,
        fingerprint: &str,
        blob_sha256: &str,
        exit_code: i32,
        shape_class: Option<&str>,
    ) -> Result<i64, DbError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        self.db.with_conn(|c| {
            c.execute(
                "INSERT INTO samples(fingerprint, sample_blob_sha256, exit_code, captured_at, shape_class)
                 VALUES(?1, ?2, ?3, ?4, ?5)",
                params![fingerprint, blob_sha256, exit_code, now, shape_class],
            )?;
            Ok(c.last_insert_rowid())
        })
    }

    pub fn for_fingerprint(&self, fingerprint: &str) -> Result<Vec<SampleRow>, DbError> {
        self.db.with_conn(|c| {
            let mut s = c.prepare(
                "SELECT id, fingerprint, sample_blob_sha256, exit_code, captured_at, shape_class
                 FROM samples WHERE fingerprint = ?1 ORDER BY captured_at ASC",
            )?;
            let rows = s
                .query_map([fingerprint], |r| {
                    Ok(SampleRow {
                        id: r.get(0)?,
                        fingerprint: r.get(1)?,
                        sample_blob_sha256: r.get(2)?,
                        exit_code: r.get(3)?,
                        captured_at: r.get(4)?,
                        shape_class: r.get(5)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    pub fn count_for(&self, fingerprint: &str) -> Result<i64, DbError> {
        self.db.with_conn(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM samples WHERE fingerprint = ?1",
                [fingerprint],
                |r| r.get(0),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blob::{BlobKind, BlobStore};
    use tempfile::TempDir;

    #[test]
    fn insert_and_list_samples() {
        let tmp = TempDir::new().unwrap();
        let db = Db::open_in_memory().unwrap();
        let store = BlobStore::new(&db, tmp.path());
        let sha1 = store.put(b"sample one", BlobKind::Sample).unwrap();
        let sha2 = store.put(b"sample two", BlobKind::Sample).unwrap();

        let repo = SampleRepo::new(&db);
        repo.insert("fp", &sha1, 0, None).unwrap();
        repo.insert("fp", &sha2, 1, Some("error")).unwrap();

        let rows = repo.for_fingerprint("fp").unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(repo.count_for("fp").unwrap(), 2);
        assert_eq!(repo.count_for("other").unwrap(), 0);
    }
}
