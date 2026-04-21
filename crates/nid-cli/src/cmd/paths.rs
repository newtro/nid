//! Resolve paths for a CLI run, respecting `NID_CONFIG_DIR` / `NID_DATA_DIR`
//! overrides (tests set these).

use anyhow::Result;
use nid_storage::paths::NidPaths;

pub fn resolve() -> Result<NidPaths> {
    if let (Some(c), Some(d)) = (
        std::env::var_os("NID_CONFIG_DIR"),
        std::env::var_os("NID_DATA_DIR"),
    ) {
        return Ok(NidPaths::from_roots(
            std::path::Path::new(&c),
            std::path::Path::new(&d),
        ));
    }
    Ok(NidPaths::default_for_platform()?)
}
