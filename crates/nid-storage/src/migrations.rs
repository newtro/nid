//! Forward-only SQL migrations.
//!
//! Migration 001 = full Appendix A DDL. Additional migrations append to the
//! `MIGRATIONS` slice; `Db::open` applies any that haven't run against
//! `meta.schema_version`.

pub struct Migration {
    pub version: u32,
    pub name: &'static str,
    pub sql: &'static str,
}

pub const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "initial_v1_schema",
    sql: include_str!("sql/001_initial.sql"),
}];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_monotonic_and_unique() {
        let mut last = 0u32;
        for m in MIGRATIONS {
            assert!(m.version > last, "migrations must strictly increase");
            last = m.version;
        }
    }

    #[test]
    fn initial_migration_contains_all_appendix_a_tables() {
        let sql = MIGRATIONS[0].sql;
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
            let needle = format!("CREATE TABLE {}", t);
            assert!(
                sql.contains(&needle) || sql.contains(&format!("CREATE TABLE IF NOT EXISTS {}", t)),
                "missing table {}",
                t
            );
        }
    }
}
