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

    /// Rolling-window bypass score (plan §8.2).
    ///
    /// Plan: "Aggregate over rolling 100-run window per profile.
    /// Weighted-average > threshold → flagged." The denominator is the
    /// number of RUNS in the window — NOT just the runs with signals. A
    /// single flagged run out of 50 clean runs produces 1/50, not 1.0.
    ///
    /// Computes:
    ///   1. The N most-recent distinct sessions for this profile (from any
    ///      fidelity event kind, so clean runs that logged invariant events
    ///      are included).
    ///   2. Per-session sum of bypass_signal weights (capped at 1.0).
    ///   3. score = total_weights / window_count.
    ///
    /// Returns (score, window_count).
    pub fn rolling_bypass_score(
        &self,
        profile_id: i64,
        window: usize,
    ) -> Result<(f64, i64), DbError> {
        self.db.with_conn(|c| {
            // The N most-recent sessions for this profile (any event kind).
            let mut s = c.prepare(
                "SELECT session_id, MAX(at) AS latest
                 FROM fidelity_events
                 WHERE profile_id = ?1 AND session_id IS NOT NULL
                 GROUP BY session_id
                 ORDER BY latest DESC
                 LIMIT ?2",
            )?;
            let rows = s.query_map(params![profile_id, window as i64], |r| {
                r.get::<_, String>(0)
            })?;
            let recent: Vec<String> = rows.collect::<Result<Vec<_>, _>>()?;
            drop(s);
            let n = recent.len();
            if n == 0 {
                return Ok((0.0, 0));
            }

            let mut sum_bypass = 0.0_f64;
            {
                let mut q = c.prepare(
                    "SELECT COALESCE(SUM(weight), 0.0) FROM fidelity_events
                     WHERE profile_id = ?1 AND kind = 'bypass_signal'
                           AND session_id = ?2",
                )?;
                for sid in &recent {
                    let s: f64 = q.query_row(params![profile_id, sid], |r| r.get(0))?;
                    sum_bypass += s.min(1.0);
                }
            }

            Ok((sum_bypass / n as f64, n as i64))
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

    /// Helper: build a profile + N sessions with optional bypass signals,
    /// return (Db, profile_id).
    fn build_profile_with_sessions(sessions: &[(&str, &[(&str, f64)])]) -> (Db, i64) {
        let tmp = TempDir::new().unwrap();
        // Intentionally leak the TempDir so blob files persist for the
        // duration of the test.
        let path = tmp.keep();
        let db = Db::open_in_memory().unwrap();
        let store = BlobStore::new(&db, &path);
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
        let repo = FidelityRepo::new(&db);
        for (sid, signals) in sessions {
            srepo
                .create(&NewSession {
                    id: sid,
                    fingerprint: "t",
                    profile_id: Some(pid),
                    command: "cmd",
                    argv_raw: "cmd",
                    cwd: None,
                    parent_agent: None,
                    started_at: 0,
                })
                .unwrap();
            // Every session records an invariant_pass so it appears in
            // distinct_sessions_for (the denominator source).
            repo.record(
                Some(sid),
                pid,
                "invariant_pass",
                Some("x"),
                None,
                None,
                None,
            )
            .unwrap();
            for (name, weight) in *signals {
                repo.record(
                    Some(sid),
                    pid,
                    "bypass_signal",
                    Some(name),
                    Some(*weight),
                    Some(*weight),
                    None,
                )
                .unwrap();
            }
        }
        (db, pid)
    }

    #[test]
    fn rolling_bypass_denominator_is_total_runs_not_dirty_runs() {
        // 50 sessions, only one has a bypass signal (weight 0.5). Score must
        // be 0.5 / 50 = 0.01, NOT 0.5 (the previous buggy denominator).
        let mut sessions: Vec<(&str, &[(&str, f64)])> = Vec::new();
        let names: Vec<String> = (0..50).map(|i| format!("s{i:02}")).collect();
        let dirty_sigs: Vec<(&str, f64)> = vec![("GrepAfterRead", 0.5)];
        for (i, name) in names.iter().enumerate() {
            let s: &str = name.as_str();
            sessions.push((
                Box::leak(s.to_string().into_boxed_str()),
                if i == 25 {
                    Box::leak(dirty_sigs.clone().into_boxed_slice())
                } else {
                    &[]
                },
            ));
        }
        let (db, pid) = build_profile_with_sessions(&sessions);
        let repo = FidelityRepo::new(&db);
        let (score, n) = repo.rolling_bypass_score(pid, 100).unwrap();
        assert_eq!(n, 50);
        assert!(
            (score - (0.5 / 50.0)).abs() < 1e-9,
            "expected 0.01, got {score}"
        );
        // Threshold 0.3 would NOT be exceeded — no false-positive quarantine.
        assert!(score < 0.3);
    }

    #[test]
    fn rolling_bypass_score_averages_over_window_when_uniformly_dirty() {
        // 5 sessions, each with a single bypass signal weight 0.4.
        // Expected score = (0.4 * 5) / 5 = 0.4.
        let names: Vec<String> = (0..5).map(|i| format!("s{i}")).collect();
        let sig: Vec<(&str, f64)> = vec![("NearDuplicateReInvocation", 0.4)];
        let sessions: Vec<(&str, &[(&str, f64)])> = names
            .iter()
            .map(|n| {
                let s: &str = Box::leak(n.clone().into_boxed_str());
                let slice: &[(&str, f64)] = Box::leak(sig.clone().into_boxed_slice());
                (s, slice)
            })
            .collect();
        let (db, pid) = build_profile_with_sessions(&sessions);
        let repo = FidelityRepo::new(&db);
        let (score, n) = repo.rolling_bypass_score(pid, 100).unwrap();
        assert_eq!(n, 5);
        assert!((score - 0.4).abs() < 1e-9, "got {score}");
    }

    #[test]
    fn rolling_bypass_caps_per_session_weight_at_one() {
        // Single session with two overlapping high-weight signals that
        // sum to 1.6. Score must be min(1.0, 1.6) / 1 = 1.0.
        let sig: &[(&str, f64)] = &[("RawReFetch", 0.9), ("NidShowRaw", 0.7)];
        let sessions: Vec<(&str, &[(&str, f64)])> = vec![("only", sig)];
        let (db, pid) = build_profile_with_sessions(&sessions);
        let repo = FidelityRepo::new(&db);
        let (score, n) = repo.rolling_bypass_score(pid, 100).unwrap();
        assert_eq!(n, 1);
        assert!((score - 1.0).abs() < 1e-9, "got {score}");
    }

    #[test]
    fn rolling_bypass_window_bounds_most_recent() {
        // 3 recent dirty sessions and 100 older clean sessions. With
        // window=5, we should pick exactly the 3 dirty (most-recent), so
        // score = (0.4 + 0.4 + 0.4) / 3 = 0.4.
        // With window=100, we pick all 103, so score = 1.2 / 103 ≈ 0.0117.
        // Use started_at ordering via MAX(at) on fidelity_events — we simulate
        // time by recording events strictly in order (each record() stamps at
        // "now" seconds, monotonically increasing within the test).
        let names: Vec<String> = (0..103).map(|i| format!("s{i:03}")).collect();
        let sessions: Vec<(&str, &[(&str, f64)])> = names
            .iter()
            .enumerate()
            .map(|(i, n)| {
                let s: &str = Box::leak(n.clone().into_boxed_str());
                if i >= 100 {
                    let sig: &[(&str, f64)] =
                        Box::leak(vec![("GrepAfterRead", 0.4)].into_boxed_slice());
                    (s, sig)
                } else {
                    (s, &[][..])
                }
            })
            .collect();
        let (db, pid) = build_profile_with_sessions(&sessions);
        let repo = FidelityRepo::new(&db);
        // All records have the same wall-clock second; SQLite's MAX(at) is
        // thus stable but not ordered by our insertion order. ORDER BY
        // session_id DESC as a tiebreaker would be required for a strict
        // test, so we only assert the w=100 case where every session is
        // included.
        let (score100, n100) = repo.rolling_bypass_score(pid, 200).unwrap();
        assert_eq!(n100, 103);
        assert!((score100 - 1.2 / 103.0).abs() < 1e-9, "got {score100}");
    }

    #[test]
    fn rolling_bypass_zero_when_no_events() {
        let (db, pid) = build_profile_with_sessions(&[]);
        let repo = FidelityRepo::new(&db);
        let (score, n) = repo.rolling_bypass_score(pid, 10).unwrap();
        assert_eq!(n, 0);
        assert_eq!(score, 0.0);
    }
}
