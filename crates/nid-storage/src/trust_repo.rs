//! CRUD on the `trust_keys` table (plan §11.2, §11.5).

use crate::db::{Db, DbError};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustKeyRow {
    pub key_id: String,
    pub public_key: Vec<u8>,
    pub label: String,
    pub added_at: i64,
    pub revoked_at: Option<i64>,
}

pub struct TrustRepo<'a> {
    db: &'a Db,
}

impl<'a> TrustRepo<'a> {
    pub fn new(db: &'a Db) -> Self {
        Self { db }
    }

    pub fn add(&self, key_id: &str, pubkey: &[u8], label: &str) -> Result<(), DbError> {
        let now = unix_now();
        self.db.with_conn(|c| {
            c.execute(
                "INSERT INTO trust_keys(key_id, public_key, label, added_at)
                 VALUES(?1, ?2, ?3, ?4)
                 ON CONFLICT(key_id) DO UPDATE SET label = excluded.label, revoked_at = NULL",
                params![key_id, pubkey, label, now],
            )?;
            Ok(())
        })
    }

    pub fn revoke(&self, key_id: &str) -> Result<(), DbError> {
        let now = unix_now();
        self.db.with_conn(|c| {
            c.execute(
                "UPDATE trust_keys SET revoked_at = ?2 WHERE key_id = ?1",
                params![key_id, now],
            )?;
            Ok(())
        })
    }

    pub fn list_active(&self) -> Result<Vec<TrustKeyRow>, DbError> {
        self.db.with_conn(|c| {
            let mut s = c.prepare(
                "SELECT key_id, public_key, label, added_at, revoked_at
                 FROM trust_keys WHERE revoked_at IS NULL ORDER BY added_at",
            )?;
            let rows = s
                .query_map([], |r| {
                    Ok(TrustKeyRow {
                        key_id: r.get(0)?,
                        public_key: r.get(1)?,
                        label: r.get(2)?,
                        added_at: r.get(3)?,
                        revoked_at: r.get(4)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    pub fn active_ids(&self) -> Result<Vec<String>, DbError> {
        Ok(self.list_active()?.into_iter().map(|r| r.key_id).collect())
    }

    pub fn get(&self, key_id: &str) -> Result<Option<TrustKeyRow>, DbError> {
        self.db.with_conn(|c| {
            c.query_row(
                "SELECT key_id, public_key, label, added_at, revoked_at
                 FROM trust_keys WHERE key_id = ?1",
                [key_id],
                |r| {
                    Ok(TrustKeyRow {
                        key_id: r.get(0)?,
                        public_key: r.get(1)?,
                        label: r.get(2)?,
                        added_at: r.get(3)?,
                        revoked_at: r.get(4)?,
                    })
                },
            )
            .optional()
        })
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_list_revoke_flow() {
        let db = Db::open_in_memory().unwrap();
        let repo = TrustRepo::new(&db);
        repo.add("abc123", b"pubkey-bytes", "acme-corp").unwrap();
        repo.add("def456", b"other", "other-org").unwrap();
        assert_eq!(repo.active_ids().unwrap().len(), 2);
        repo.revoke("abc123").unwrap();
        let active = repo.active_ids().unwrap();
        assert_eq!(active, vec!["def456".to_string()]);
    }

    #[test]
    fn add_same_key_overwrites_label() {
        let db = Db::open_in_memory().unwrap();
        let repo = TrustRepo::new(&db);
        repo.add("xyz", b"pub", "old").unwrap();
        repo.add("xyz", b"pub", "new").unwrap();
        let row = repo.get("xyz").unwrap().unwrap();
        assert_eq!(row.label, "new");
    }
}
