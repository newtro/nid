//! nid-storage: SQLite schema + migrations + content-addressed blob store + GC.
//!
//! See plan §12 and Appendix A.

pub mod blob;
pub mod db;
pub mod fidelity_repo;
pub mod migrations;
pub mod paths;
pub mod profile_repo;
pub mod sample_repo;
pub mod session_repo;
pub mod trust_repo;

pub use blob::{BlobKind, BlobStore};
pub use db::{Db, DbError};
pub use paths::NidPaths;
