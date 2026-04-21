//! SQLite connection + migration runner.

use crate::migrations::MIGRATIONS;
use rusqlite::{Connection, OptionalExtension};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DbError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("schema downgrade not supported: db at v{db} but binary at v{bin}")]
    Downgrade { db: u32, bin: u32 },
}

/// Thread-safe SQLite handle. Runs migrations on open.
pub struct Db {
    pub(crate) conn: Mutex<Connection>,
    path: PathBuf,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self, DbError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        let db = Db {
            conn: Mutex::new(conn),
            path: path.to_path_buf(),
        };
        db.apply_migrations()?;
        Ok(db)
    }

    /// Open an ephemeral in-memory DB (tests).
    pub fn open_in_memory() -> Result<Self, DbError> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let db = Db {
            conn: Mutex::new(conn),
            path: PathBuf::from(":memory:"),
        };
        db.apply_migrations()?;
        Ok(db)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn apply_migrations(&self) -> Result<(), DbError> {
        let mut c = self.conn.lock().unwrap();
        // Bootstrap meta table outside migrations (chicken/egg).
        c.execute(
            "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
            [],
        )?;

        let current: u32 = c
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        let latest = MIGRATIONS.last().map(|m| m.version).unwrap_or(0);
        if current > latest {
            return Err(DbError::Downgrade {
                db: current,
                bin: latest,
            });
        }

        for m in MIGRATIONS {
            if m.version <= current {
                continue;
            }
            tracing::info!(version = m.version, name = m.name, "applying migration");
            let tx = c.transaction()?;
            tx.execute_batch(m.sql)?;
            tx.execute(
                "INSERT INTO meta(key,value) VALUES('schema_version', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [m.version.to_string()],
            )?;
            tx.commit()?;
        }
        Ok(())
    }

    pub fn schema_version(&self) -> Result<u32, DbError> {
        let c = self.conn.lock().unwrap();
        let s: String = c.query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |r| r.get(0),
        )?;
        Ok(s.parse().unwrap_or(0))
    }

    /// Run a closure with a mutable borrow of the underlying connection.
    /// Kept `pub(crate)` so it's not part of the public API.
    pub(crate) fn with_conn<F, R>(&self, f: F) -> Result<R, DbError>
    where
        F: FnOnce(&mut Connection) -> Result<R, rusqlite::Error>,
    {
        let mut c = self.conn.lock().unwrap();
        Ok(f(&mut c)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_memory_migrations_apply() {
        let db = Db::open_in_memory().unwrap();
        assert!(db.schema_version().unwrap() >= 1);
    }

    #[test]
    fn idempotent_reopen() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        drop(tmp); // file path stays, file is gone — just need a unique path
        {
            let db = Db::open(&path).unwrap();
            assert_eq!(db.schema_version().unwrap(), 1);
        }
        {
            let db = Db::open(&path).unwrap();
            assert_eq!(db.schema_version().unwrap(), 1);
        }
        std::fs::remove_file(&path).ok();
        std::fs::remove_file(path.with_extension("sqlite-shm")).ok();
        std::fs::remove_file(path.with_extension("sqlite-wal")).ok();
    }

    #[test]
    fn all_eleven_tables_present() {
        let db = Db::open_in_memory().unwrap();
        let c = db.conn.lock().unwrap();
        for t in [
            "meta",
            "profiles",
            "blobs",
            "samples",
            "sessions",
            "fidelity_events",
            "synthesis_events",
            "gain_daily",
            "trust_keys",
            "profile_import_events",
            "agent_registry",
        ] {
            let n: i64 = c
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [t],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "missing table {}", t);
        }
    }
}
