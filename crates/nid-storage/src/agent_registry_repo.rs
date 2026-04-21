//! CRUD on `agent_registry` (plan §11.1 hook-integrity tracking).

use crate::db::{Db, DbError};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRow {
    pub agent: String,
    pub hook_path: String,
    pub hook_sha256: String,
    pub installed_at: i64,
    pub original_backup: Option<String>,
}

pub struct AgentRegistryRepo<'a> {
    db: &'a Db,
}

impl<'a> AgentRegistryRepo<'a> {
    pub fn new(db: &'a Db) -> Self {
        Self { db }
    }

    pub fn upsert(
        &self,
        agent: &str,
        hook_path: &str,
        hook_sha256: &str,
        original_backup: Option<&str>,
    ) -> Result<(), DbError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        self.db.with_conn(|c| {
            c.execute(
                "INSERT INTO agent_registry(agent, hook_path, hook_sha256, installed_at, original_backup)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(agent) DO UPDATE SET
                   hook_path = excluded.hook_path,
                   hook_sha256 = excluded.hook_sha256,
                   installed_at = excluded.installed_at,
                   original_backup = excluded.original_backup",
                params![agent, hook_path, hook_sha256, now, original_backup],
            )?;
            Ok(())
        })
    }

    pub fn get(&self, agent: &str) -> Result<Option<AgentRow>, DbError> {
        self.db.with_conn(|c| {
            c.query_row(
                "SELECT agent, hook_path, hook_sha256, installed_at, original_backup
                 FROM agent_registry WHERE agent = ?1",
                [agent],
                |r| {
                    Ok(AgentRow {
                        agent: r.get(0)?,
                        hook_path: r.get(1)?,
                        hook_sha256: r.get(2)?,
                        installed_at: r.get(3)?,
                        original_backup: r.get(4)?,
                    })
                },
            )
            .optional()
        })
    }

    pub fn list(&self) -> Result<Vec<AgentRow>, DbError> {
        self.db.with_conn(|c| {
            let mut s = c.prepare(
                "SELECT agent, hook_path, hook_sha256, installed_at, original_backup
                 FROM agent_registry ORDER BY agent",
            )?;
            let rows = s
                .query_map([], |r| {
                    Ok(AgentRow {
                        agent: r.get(0)?,
                        hook_path: r.get(1)?,
                        hook_sha256: r.get(2)?,
                        installed_at: r.get(3)?,
                        original_backup: r.get(4)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_and_list() {
        let db = Db::open_in_memory().unwrap();
        let r = AgentRegistryRepo::new(&db);
        r.upsert("claude_code", "/a/b", "abc123", Some("{}"))
            .unwrap();
        r.upsert("cursor", "/c/d", "def456", None).unwrap();
        // Upsert same agent again: should update sha256.
        r.upsert("claude_code", "/a/b", "new-sha", Some("{}"))
            .unwrap();
        let row = r.get("claude_code").unwrap().unwrap();
        assert_eq!(row.hook_sha256, "new-sha");
        assert_eq!(r.list().unwrap().len(), 2);
    }
}
