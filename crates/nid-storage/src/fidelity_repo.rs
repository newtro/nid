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
        })
    }

    /// Count distinct sessions that recorded any fidelity event for this
    /// profile. Used to implement the bypass-tracker warmup window
    /// (plan §8.2 — ignore first 3 runs after profile activation).
    pub fn distinct_sessions_for(&self, profile_id: i64) -> Result<i64, DbError> {
        self.db.with_conn(|c| {
            c.query_row(
                "SELECT COUNT(DISTINCT session_id) FROM fidelity_events
                 WHERE profile_id = ?1 AND session_id IS NOT NULL",
                [profile_id],
                |r| r.get(0),
            )
        })
    }

    /// Rolling-window bypass score (plan §8.2): per-session sum of
    /// `bypass_signal.weight`, averaged over the most-recent `window`
    /// sessions for this profile. Returns (score, window_count).
    pub fn rolling_bypass_score(
        &self,
        profile_id: i64,
        window: usize,
    ) -> Result<(f64, i64), DbError> {
        self.db.with_conn(|c| {
            let mut s = c.prepare(
                "SELECT COALESCE(SUM(weight), 0.0) AS s FROM fidelity_events
                 WHERE profile_id = ?1 AND kind = 'bypass_signal'
                 GROUP BY session_id
                 ORDER BY MAX(at) DESC
                 LIMIT ?2",
            )?;
            let rows: Vec<f64> = s
                .query_map(params![profile_id, window as i64], |r| r.get::<_, f64>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            let n = rows.len() as i64;
            if n == 0 {
                return Ok((0.0, 0));
            }
            let sum: f64 = rows.iter().sum();
            Ok((sum / n as f64, n))
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
        repo.record(
            Some("s1"),
            pid,
            "invariant_pass",
            Some("Foo"),
            None,
            None,
            None,
        )
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
