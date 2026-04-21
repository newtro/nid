//! CRUD on `fidelity_events` — Phase 5 invariant-check persistence.

use crate::db::{Db, DbError};
use rusqlite::params;

pub struct FidelityRepo<'a> {
    db: &'a Db,
}

impl<'a> FidelityRepo<'a> {
    pub fn new(db: &'a Db) -> Self {
        Self { db }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record(
        &self,
        session_id: Option<&str>,
        profile_id: i64,
        kind: &str,
        signal: Option<&str>,
        score: Option<f64>,
        weight: Option<f64>,
        detail: Option<&str>,
    ) -> Result<i64, DbError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        self.db.with_conn(|c| {
            c.execute(
                "INSERT INTO fidelity_events(session_id, profile_id, kind, signal, score, weight, detail, at)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![session_id, profile_id, kind, signal, score, weight, detail, now],
            )?;
            Ok(c.last_insert_rowid())
        })
    }

    pub fn count_for(&self, profile_id: i64) -> Result<i64, DbError> {
        self.db.with_conn(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM fidelity_events WHERE profile_id = ?1",
                [profile_id],
                |r| r.get(0),
            )
            .map_err(Into::into)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blob::{BlobKind, BlobStore};
    use crate::profile_repo::{NewProfile, ProfileRepo, PROV_BUNDLED};
    use crate::session_repo::{NewSession, SessionRepo};
    use tempfile::TempDir;

    #[test]
    fn record_and_count() {
        let tmp = TempDir::new().unwrap();
        let db = Db::open_in_memory().unwrap();
        let store = BlobStore::new(&db, tmp.path());

        let dsl_sha = store.put(b"dsl-bytes", BlobKind::Dsl).unwrap();
        let prepo = ProfileRepo::new(&db);
        let pid = prepo
            .insert_pending(&NewProfile {
                fingerprint: "t".into(),
                version: "1.0.0".into(),
                provenance: PROV_BUNDLED.into(),
                synthesis_source: None,
                dsl_blob_sha256: dsl_sha,
                parent_fp: None,
                split_on_flag: None,
                signer_key_id: None,
            })
            .unwrap();
        prepo.promote(pid).unwrap();

        let srepo = SessionRepo::new(&db);
        srepo
            .create(&NewSession {
                id: "s1",
                fingerprint: "t",
                profile_id: Some(pid),
                command: "cmd",
                argv_raw: "cmd",
                cwd: None,
                parent_agent: None,
                started_at: 0,
            })
            .unwrap();

        let repo = FidelityRepo::new(&db);
        repo.record(Some("s1"), pid, "invariant_pass", Some("Foo"), None, None, None)
            .unwrap();
        repo.record(
            Some("s1"),
            pid,
            "invariant_fail",
            Some("Bar"),
            None,
            None,
            Some("missing"),
        )
        .unwrap();
        assert_eq!(repo.count_for(pid).unwrap(), 2);
    }
}
