//! CRUD and status-flip operations on the `profiles` table.
//!
//! Lazy activation (plan §12.2): inserts go in as `pending`; promotion to
//! `active` flips the old `active` row to `superseded` in the same tx.

use crate::db::{Db, DbError};
use rusqlite::params;
use serde::{Deserialize, Serialize};

pub const STATUS_ACTIVE: &str = "active";
pub const STATUS_PENDING: &str = "pending";
pub const STATUS_SUPERSEDED: &str = "superseded";
pub const STATUS_QUARANTINED: &str = "quarantined";

pub const PROV_SYNTHESIZED: &str = "synthesized";
pub const PROV_HAND_TUNED: &str = "hand_tuned";
pub const PROV_BUNDLED: &str = "bundled";
pub const PROV_IMPORTED: &str = "imported";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileRow {
    pub id: i64,
    pub fingerprint: String,
    pub version: String,
    pub provenance: String,
    pub synthesis_source: Option<String>,
    pub status: String,
    pub dsl_blob_sha256: String,
    pub rubric_blob_sha256: Option<String>,
    pub parent_fp: Option<String>,
    pub split_on_flag: Option<String>,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
    pub sample_count: i64,
    pub fidelity_rolling: Option<f64>,
    pub signer_key_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NewProfile {
    pub fingerprint: String,
    pub version: String,
    pub provenance: String,
    pub synthesis_source: Option<String>,
    pub dsl_blob_sha256: String,
    pub parent_fp: Option<String>,
    pub split_on_flag: Option<String>,
    pub signer_key_id: Option<String>,
}

pub struct ProfileRepo<'a> {
    db: &'a Db,
}

impl<'a> ProfileRepo<'a> {
    pub fn new(db: &'a Db) -> Self {
        Self { db }
    }

    /// Insert a profile as `pending`. Returns the new row id.
    pub fn insert_pending(&self, p: &NewProfile) -> Result<i64, DbError> {
        let now = unix_now();
        self.db.with_conn(|c| {
            c.execute(
                "INSERT INTO profiles(
                    fingerprint, version, provenance, synthesis_source, status,
                    dsl_blob_sha256, parent_fp, split_on_flag, signer_key_id,
                    created_at, sample_count
                 ) VALUES (?1,?2,?3,?4,'pending',?5,?6,?7,?8,?9,0)",
                params![
                    p.fingerprint,
                    p.version,
                    p.provenance,
                    p.synthesis_source,
                    p.dsl_blob_sha256,
                    p.parent_fp,
                    p.split_on_flag,
                    p.signer_key_id,
                    now,
                ],
            )?;
            Ok(c.last_insert_rowid())
        })
    }

    /// Promote a pending profile to active. Flips the current active profile for
    /// the same fingerprint (if any) to superseded in one transaction.
    pub fn promote(&self, id: i64) -> Result<(), DbError> {
        self.db.with_conn(|c| {
            let tx = c.transaction()?;
            let fp: String = tx.query_row(
                "SELECT fingerprint FROM profiles WHERE id = ?1",
                [id],
                |r| r.get(0),
            )?;
            tx.execute(
                "UPDATE profiles SET status='superseded'
                 WHERE fingerprint=?1 AND status='active' AND id != ?2",
                params![fp, id],
            )?;
            tx.execute("UPDATE profiles SET status='active' WHERE id = ?1", [id])?;
            tx.commit()?;
            Ok(())
        })
    }

    pub fn set_status(&self, id: i64, status: &str) -> Result<(), DbError> {
        self.db.with_conn(|c| {
            c.execute(
                "UPDATE profiles SET status=?1 WHERE id=?2",
                params![status, id],
            )?;
            Ok(())
        })
    }

    pub fn active_for(&self, fingerprint: &str) -> Result<Option<ProfileRow>, DbError> {
        self.db.with_conn(|c| {
            let mut s = c.prepare(
                "SELECT id, fingerprint, version, provenance, synthesis_source, status,
                        dsl_blob_sha256, rubric_blob_sha256, parent_fp, split_on_flag,
                        created_at, last_used_at, sample_count, fidelity_rolling, signer_key_id
                 FROM profiles
                 WHERE fingerprint = ?1 AND status = 'active'
                 ORDER BY created_at DESC
                 LIMIT 1",
            )?;
            let mut rows = s.query([fingerprint])?;
            if let Some(r) = rows.next()? {
                Ok(Some(row_to_profile(r)?))
            } else {
                Ok(None)
            }
        })
    }

    pub fn list(&self) -> Result<Vec<ProfileRow>, DbError> {
        self.db.with_conn(|c| {
            let mut s = c.prepare(
                "SELECT id, fingerprint, version, provenance, synthesis_source, status,
                        dsl_blob_sha256, rubric_blob_sha256, parent_fp, split_on_flag,
                        created_at, last_used_at, sample_count, fidelity_rolling, signer_key_id
                 FROM profiles
                 ORDER BY fingerprint, created_at DESC",
            )?;
            let rows = s
                .query_map([], row_to_profile)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    pub fn get(&self, id: i64) -> Result<Option<ProfileRow>, DbError> {
        self.db.with_conn(|c| {
            let mut s = c.prepare(
                "SELECT id, fingerprint, version, provenance, synthesis_source, status,
                        dsl_blob_sha256, rubric_blob_sha256, parent_fp, split_on_flag,
                        created_at, last_used_at, sample_count, fidelity_rolling, signer_key_id
                 FROM profiles WHERE id = ?1",
            )?;
            let mut rows = s.query([id])?;
            if let Some(r) = rows.next()? {
                Ok(Some(row_to_profile(r)?))
            } else {
                Ok(None)
            }
        })
    }

    pub fn increment_sample_count(&self, fingerprint: &str) -> Result<(), DbError> {
        self.db.with_conn(|c| {
            c.execute(
                "UPDATE profiles SET sample_count = sample_count + 1
                 WHERE fingerprint=?1 AND status='active'",
                [fingerprint],
            )?;
            Ok(())
        })
    }

    /// Roll back: flip the most-recent superseded profile for `fingerprint`
    /// back to `active`, demoting the current active to superseded in the
    /// same transaction.
    pub fn rollback(&self, fingerprint: &str) -> Result<Option<i64>, DbError> {
        self.db.with_conn(|c| {
            let tx = c.transaction()?;
            let target: Option<i64> = tx
                .query_row(
                    "SELECT id FROM profiles
                     WHERE fingerprint = ?1 AND status = 'superseded'
                     ORDER BY created_at DESC
                     LIMIT 1",
                    [fingerprint],
                    |r| r.get::<_, i64>(0),
                )
                .ok();
            let Some(target) = target else {
                return Ok(None);
            };
            tx.execute(
                "UPDATE profiles SET status='superseded'
                 WHERE fingerprint=?1 AND status='active' AND id != ?2",
                params![fingerprint, target],
            )?;
            tx.execute(
                "UPDATE profiles SET status='active' WHERE id = ?1",
                [target],
            )?;
            tx.commit()?;
            Ok(Some(target))
        })
    }

    /// Purge: hard-delete a profile row and return (dsl_blob_sha,
    /// rubric_blob_sha) so the caller can release both. Previously only the
    /// dsl sha was returned — rubric blobs leaked on purge (M-R2).
    pub fn purge(&self, id: i64) -> Result<(Option<String>, Option<String>), DbError> {
        self.db.with_conn(|c| {
            let pair: Option<(String, Option<String>)> = c
                .query_row(
                    "SELECT dsl_blob_sha256, rubric_blob_sha256 FROM profiles WHERE id = ?1",
                    [id],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
                )
                .ok();
            c.execute("DELETE FROM profiles WHERE id = ?1", [id])?;
            Ok(match pair {
                Some((dsl, rubric)) => (Some(dsl), rubric),
                None => (None, None),
            })
        })
    }

    /// List every row matching `fingerprint` in any status. Useful for
    /// `purge <fp>` which needs to release blobs for every historical row.
    pub fn list_by_fingerprint(&self, fingerprint: &str) -> Result<Vec<ProfileRow>, DbError> {
        self.db.with_conn(|c| {
            let mut s = c.prepare(
                "SELECT id, fingerprint, version, provenance, synthesis_source, status,
                        dsl_blob_sha256, rubric_blob_sha256, parent_fp, split_on_flag,
                        created_at, last_used_at, sample_count, fidelity_rolling, signer_key_id
                 FROM profiles WHERE fingerprint = ?1
                 ORDER BY created_at DESC",
            )?;
            let rows = s
                .query_map([fingerprint], row_to_profile)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    pub fn record_use(&self, id: i64) -> Result<(), DbError> {
        let now = unix_now();
        self.db.with_conn(|c| {
            c.execute(
                "UPDATE profiles SET last_used_at = ?2 WHERE id = ?1",
                params![id, now],
            )?;
            Ok(())
        })
    }
}

fn row_to_profile(r: &rusqlite::Row<'_>) -> rusqlite::Result<ProfileRow> {
    Ok(ProfileRow {
        id: r.get(0)?,
        fingerprint: r.get(1)?,
        version: r.get(2)?,
        provenance: r.get(3)?,
        synthesis_source: r.get(4)?,
        status: r.get(5)?,
        dsl_blob_sha256: r.get(6)?,
        rubric_blob_sha256: r.get(7)?,
        parent_fp: r.get(8)?,
        split_on_flag: r.get(9)?,
        created_at: r.get(10)?,
        last_used_at: r.get(11)?,
        sample_count: r.get(12)?,
        fidelity_rolling: r.get(13)?,
        signer_key_id: r.get(14)?,
    })
}

fn unix_now() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn np(fp: &str, ver: &str, dsl: &str) -> NewProfile {
        NewProfile {
            fingerprint: fp.into(),
            version: ver.into(),
            provenance: PROV_BUNDLED.into(),
            synthesis_source: None,
            dsl_blob_sha256: dsl.into(),
            parent_fp: None,
            split_on_flag: None,
            signer_key_id: None,
        }
    }

    #[test]
    fn insert_and_promote() {
        let db = Db::open_in_memory().unwrap();
        let repo = ProfileRepo::new(&db);
        let id = repo
            .insert_pending(&np("git status", "1.0.0", "aaa"))
            .unwrap();
        assert!(repo.active_for("git status").unwrap().is_none());
        repo.promote(id).unwrap();
        let row = repo.active_for("git status").unwrap().unwrap();
        assert_eq!(row.status, STATUS_ACTIVE);
    }

    #[test]
    fn promote_supersedes_old_active() {
        let db = Db::open_in_memory().unwrap();
        let repo = ProfileRepo::new(&db);
        let old = repo.insert_pending(&np("git log", "1.0.0", "aaa")).unwrap();
        repo.promote(old).unwrap();
        let new_id = repo.insert_pending(&np("git log", "1.1.0", "bbb")).unwrap();
        repo.promote(new_id).unwrap();
        let active = repo.active_for("git log").unwrap().unwrap();
        assert_eq!(active.version, "1.1.0");

        let old_row = repo.get(old).unwrap().unwrap();
        assert_eq!(old_row.status, STATUS_SUPERSEDED);
    }

    #[test]
    fn increment_sample_count_updates_only_active() {
        let db = Db::open_in_memory().unwrap();
        let repo = ProfileRepo::new(&db);
        let id = repo.insert_pending(&np("pytest", "1.0.0", "aaa")).unwrap();
        repo.promote(id).unwrap();
        repo.increment_sample_count("pytest").unwrap();
        repo.increment_sample_count("pytest").unwrap();
        let p = repo.get(id).unwrap().unwrap();
        assert_eq!(p.sample_count, 2);
    }
}
