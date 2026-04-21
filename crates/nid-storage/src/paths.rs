//! Resolve nid's on-disk layout (plan §4.2).

use directories::ProjectDirs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct NidPaths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
    pub blobs_dir: PathBuf,
    pub sessions_dir: PathBuf,
    pub hooks_dir: PathBuf,
    pub onboard_backup: PathBuf,
    pub config_toml: PathBuf,
    pub local_key: PathBuf,
    pub release_key_pub: PathBuf,
}

impl NidPaths {
    /// Default platform paths.
    ///
    /// Linux: `~/.config/nid/`, `~/.local/share/nid/`
    /// macOS: `~/Library/Application Support/nid/` (doubles for config+data; we
    ///        still split conceptually under one root for compatibility).
    pub fn default_for_platform() -> anyhow::Result<Self> {
        let pd = ProjectDirs::from("", "", "nid")
            .ok_or_else(|| anyhow::anyhow!("failed to locate user home"))?;
        let config_dir = pd.config_dir().to_path_buf();
        let data_dir = pd.data_dir().to_path_buf();
        Ok(Self::from_roots(&config_dir, &data_dir))
    }

    pub fn from_roots(config_dir: &Path, data_dir: &Path) -> Self {
        Self {
            config_dir: config_dir.to_path_buf(),
            data_dir: data_dir.to_path_buf(),
            db_path: data_dir.join("nid.sqlite"),
            blobs_dir: data_dir.join("blobs"),
            sessions_dir: data_dir.join("sessions"),
            hooks_dir: config_dir.join("hooks"),
            onboard_backup: config_dir.join("onboard.backup.json"),
            config_toml: config_dir.join("config.toml"),
            local_key: data_dir.join("key"),
            release_key_pub: data_dir.join("release-key.pub"),
        }
    }

    /// Ensure all directories exist with tight perms.
    pub fn ensure(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.config_dir)?;
        std::fs::create_dir_all(&self.data_dir)?;
        std::fs::create_dir_all(&self.blobs_dir)?;
        std::fs::create_dir_all(&self.sessions_dir)?;
        std::fs::create_dir_all(&self.hooks_dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for p in [&self.config_dir, &self.data_dir, &self.blobs_dir, &self.sessions_dir] {
                let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn from_roots_sets_all_paths() {
        let t = TempDir::new().unwrap();
        let c = t.path().join("config");
        let d = t.path().join("data");
        let p = NidPaths::from_roots(&c, &d);
        assert_eq!(p.db_path, d.join("nid.sqlite"));
        assert_eq!(p.blobs_dir, d.join("blobs"));
        assert_eq!(p.hooks_dir, c.join("hooks"));
    }

    #[test]
    fn ensure_creates_dirs() {
        let t = TempDir::new().unwrap();
        let p = NidPaths::from_roots(&t.path().join("c"), &t.path().join("d"));
        p.ensure().unwrap();
        assert!(p.blobs_dir.is_dir());
        assert!(p.hooks_dir.is_dir());
    }
}
