//! CRUD on the `sessions` table and `gain_daily` rollup.

use crate::db::{Db, DbError};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRow {
    pub id: String,
    pub fingerprint: String,
    pub profile_id: Option<i64>,
    pub command: String,
    pub argv_raw: String,
    pub cwd: Option<String>,
    pub parent_agent: Option<String>,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub exit_code: Option<i64>,
    pub raw_blob_sha256: Option<String>,
    pub compressed_blob_sha256: Option<String>,
    pub raw_bytes: Option<i64>,
    pub compressed_bytes: Option<i64>,
    pub tokens_saved_est: Option<i64>,
    pub model_estimator: Option<String>,
    pub mode: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NewSession<'a> {
    pub id: &'a str,
    pub fingerprint: &'a str,
    pub profile_id: Option<i64>,
    pub command: &'a str,
    pub argv_raw: &'a str,
    pub cwd: Option<&'a str>,
    pub parent_agent: Option<&'a str>,
    pub started_at: i64,
}

pub struct SessionRepo<'a> {
    db: &'a Db,
}

impl<'a> SessionRepo<'a> {
    pub fn new(db: &'a Db) -> Self {
        Self { db }
    }

    pub fn create(&self, s: &NewSession) -> Result<(), DbError> {
        self.db.with_conn(|c| {
            c.execute(
                "INSERT INTO sessions(
                    id, fingerprint, profile_id, command, argv_raw, cwd, parent_agent, started_at
                 ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    s.id,
                    s.fingerprint,
                    s.profile_id,
                    s.command,
                    s.argv_raw,
                    s.cwd,
                    s.parent_agent,
                    s.started_at,
                ],
            )?;
            Ok(())
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn finalize(
        &self,
        id: &str,
        ended_at: i64,
        exit_code: i32,
        raw_blob: Option<&str>,
        compressed_blob: Option<&str>,
        raw_bytes: i64,
        compressed_bytes: i64,
        tokens_saved_est: i64,
        model_estimator: &str,
        mode: &str,
    ) -> Result<(), DbError> {
        self.db.with_conn(|c| {
            c.execute(
                "UPDATE sessions SET
                    ended_at=?2, exit_code=?3,
                    raw_blob_sha256=?4, compressed_blob_sha256=?5,
                    raw_bytes=?6, compressed_bytes=?7,
                    tokens_saved_est=?8, model_estimator=?9, mode=?10
                 WHERE id = ?1",
                params![
                    id,
                    ended_at,
                    exit_code,
                    raw_blob,
                    compressed_blob,
                    raw_bytes,
                    compressed_bytes,
                    tokens_saved_est,
                    model_estimator,
                    mode,
                ],
            )?;
            Ok(())
        })
    }

    pub fn get(&self, id: &str) -> Result<Option<SessionRow>, DbError> {
        self.db.with_conn(|c| {
            c.query_row(
                "SELECT id, fingerprint, profile_id, command, argv_raw, cwd, parent_agent,
                        started_at, ended_at, exit_code,
                        raw_blob_sha256, compressed_blob_sha256,
                        raw_bytes, compressed_bytes, tokens_saved_est, model_estimator, mode
                 FROM sessions WHERE id = ?1",
                [id],
                |r| {
                    Ok(SessionRow {
                        id: r.get(0)?,
                        fingerprint: r.get(1)?,
                        profile_id: r.get(2)?,
                        command: r.get(3)?,
                        argv_raw: r.get(4)?,
                        cwd: r.get(5)?,
                        parent_agent: r.get(6)?,
                        started_at: r.get(7)?,
                        ended_at: r.get(8)?,
                        exit_code: r.get(9)?,
                        raw_blob_sha256: r.get(10)?,
                        compressed_blob_sha256: r.get(11)?,
                        raw_bytes: r.get(12)?,
                        compressed_bytes: r.get(13)?,
                        tokens_saved_est: r.get(14)?,
                        model_estimator: r.get(15)?,
                        mode: r.get(16)?,
                    })
                },
            )
            .optional()
        })
    }

    /// Recent sessions, newest-first. `limit` caps the result set.
    pub fn list_recent(&self, limit: i64) -> Result<Vec<SessionRow>, DbError> {
        self.db.with_conn(|c| {
            let mut s = c.prepare(
                "SELECT id, fingerprint, profile_id, command, argv_raw, cwd, parent_agent,
                        started_at, ended_at, exit_code,
                        raw_blob_sha256, compressed_blob_sha256,
                        raw_bytes, compressed_bytes, tokens_saved_est, model_estimator, mode
                 FROM sessions ORDER BY started_at DESC LIMIT ?1",
            )?;
            let rows = s
                .query_map([limit], |r| {
                    Ok(SessionRow {
                        id: r.get(0)?,
                        fingerprint: r.get(1)?,
                        profile_id: r.get(2)?,
                        command: r.get(3)?,
                        argv_raw: r.get(4)?,
                        cwd: r.get(5)?,
                        parent_agent: r.get(6)?,
                        started_at: r.get(7)?,
                        ended_at: r.get(8)?,
                        exit_code: r.get(9)?,
                        raw_blob_sha256: r.get(10)?,
                        compressed_blob_sha256: r.get(11)?,
                        raw_bytes: r.get(12)?,
                        compressed_bytes: r.get(13)?,
                        tokens_saved_est: r.get(14)?,
                        model_estimator: r.get(15)?,
                        mode: r.get(16)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    /// Delete sessions older than `cutoff_unix` and return their raw/compressed
    /// blob sha256s for the caller to release.
    pub fn purge_older_than(&self, cutoff_unix: i64) -> Result<Vec<(String, String)>, DbError> {
        self.db.with_conn(|c| {
            let tx = c.transaction()?;
            let mut to_release: Vec<(String, String)> = Vec::new();
            {
                let mut s = tx.prepare(
                    "SELECT COALESCE(raw_blob_sha256,''), COALESCE(compressed_blob_sha256,'')
                     FROM sessions WHERE started_at < ?1",
                )?;
                let rows = s.query_map([cutoff_unix], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })?;
                for row in rows {
                    to_release.push(row?);
                }
            }
            tx.execute("DELETE FROM sessions WHERE started_at < ?1", [cutoff_unix])?;
            tx.commit()?;
            Ok(to_release)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_and_finalize_session() {
        let db = Db::open_in_memory().unwrap();
        let repo = SessionRepo::new(&db);
        repo.create(&NewSession {
            id: "sess_abcdef0001",
            fingerprint: "git status",
            profile_id: None,
            command: "git status",
            argv_raw: "git status",
            cwd: Some("/tmp"),
            parent_agent: Some("claude_code"),
            started_at: 1000,
        })
        .unwrap();

        repo.finalize(
            "sess_abcdef0001",
            1010,
            0,
            Some("raw-sha"),
            Some("cmp-sha"),
            1000,
            300,
            700,
            "gpt2",
            "Full",
        )
        .unwrap();

        let got = repo.get("sess_abcdef0001").unwrap().unwrap();
        assert_eq!(got.exit_code, Some(0));
        assert_eq!(got.compressed_bytes, Some(300));
        assert_eq!(got.mode.as_deref(), Some("Full"));
    }

    #[test]
    fn purge_older_returns_blob_refs() {
        let db = Db::open_in_memory().unwrap();
        let repo = SessionRepo::new(&db);
        repo.create(&NewSession {
            id: "s1",
            fingerprint: "fp",
            profile_id: None,
            command: "c",
            argv_raw: "c",
            cwd: None,
            parent_agent: None,
            started_at: 100,
        })
        .unwrap();
        repo.finalize(
            "s1",
            110,
            0,
            Some("raw1"),
            Some("cmp1"),
            1,
            1,
            0,
            "e",
            "Full",
        )
        .unwrap();

        let purged = repo.purge_older_than(200).unwrap();
        assert_eq!(purged.len(), 1);
        assert_eq!(purged[0], ("raw1".into(), "cmp1".into()));
        assert!(repo.get("s1").unwrap().is_none());
    }
}
