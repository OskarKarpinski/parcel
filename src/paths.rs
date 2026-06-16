//! Filesystem layout for user-space Parcel state.

use std::path::PathBuf;

use anyhow::{Result, anyhow};

/// Filesystem locations required by Parcel.
#[derive(Debug, Clone)]
pub struct Paths {
    pub home: PathBuf,
    pub apps_dir: PathBuf,
    pub database: PathBuf,
    pub remotes: PathBuf,
    pub indexes_dir: PathBuf,
}

impl Paths {
    /// Discover Parcel paths from `$HOME`.
    pub fn discover() -> Result<Self> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| anyhow!("HOME is not set"))?;
        Ok(Self::from_home(home))
    }

    /// Build paths for a specific home directory. Tests use this to stay fully
    /// isolated from the developer's real package database.
    pub fn from_home(home: PathBuf) -> Self {
        let parcel_dir = home.join(".local/share/parcel");
        Self {
            apps_dir: parcel_dir.join("apps"),
            database: parcel_dir.join("parcel.db"),
            remotes: parcel_dir.join("remotes.json"),
            indexes_dir: parcel_dir.join("indexes"),
            home,
        }
    }

    pub fn local_bin(&self) -> PathBuf {
        self.home.join(".local/bin")
    }

    pub fn applications(&self) -> PathBuf {
        self.home.join(".local/share/applications")
    }

    pub fn icons(&self) -> PathBuf {
        self.home.join(".local/share/icons")
    }

    pub fn man(&self) -> PathBuf {
        self.home.join(".local/share/man")
    }
}
