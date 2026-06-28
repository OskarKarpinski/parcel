use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};

#[derive(Debug, Clone)]
pub struct Layout {
    pub data_home: PathBuf,
    pub state_home: PathBuf,
    pub cache_home: PathBuf,
    pub config_home: PathBuf,
}

impl Layout {
    pub fn detect() -> Result<Self> {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .context("HOME must be set to determine Parcel directories")?;

        let data_home = env_path("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local/share"));
        let state_home = env_path("XDG_STATE_HOME").unwrap_or_else(|| home.join(".local/state"));
        let cache_home = env_path("XDG_CACHE_HOME").unwrap_or_else(|| home.join(".cache"));
        let config_home = env_path("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config"));

        Ok(Self {
            data_home,
            state_home,
            cache_home,
            config_home,
        })
    }

    pub fn ensure_all(&self) -> Result<()> {
        for dir in [
            self.parcel_data_dir(),
            self.cellar_dir(),
            self.opt_dir(),
            self.receipts_dir(),
            self.indexes_dir(),
            self.downloads_dir(),
            self.repos_dir(),
        ] {
            fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        }
        Ok(())
    }

    pub fn parcel_data_dir(&self) -> PathBuf {
        self.data_home.join("parcel")
    }

    pub fn cellar_dir(&self) -> PathBuf {
        self.parcel_data_dir().join("cellar")
    }

    pub fn opt_dir(&self) -> PathBuf {
        self.parcel_data_dir().join("opt")
    }

    pub fn receipts_dir(&self) -> PathBuf {
        self.state_home.join("parcel").join("receipts")
    }

    pub fn indexes_dir(&self) -> PathBuf {
        self.cache_home.join("parcel").join("indexes")
    }

    pub fn downloads_dir(&self) -> PathBuf {
        self.cache_home.join("parcel").join("downloads")
    }

    pub fn repos_dir(&self) -> PathBuf {
        self.config_home.join("parcel").join("repos.d")
    }

    pub fn repo_config_path(&self, name: &str) -> PathBuf {
        self.repos_dir().join(format!("{name}.toml"))
    }

    pub fn index_cache_path(&self, name: &str) -> PathBuf {
        self.indexes_dir().join(format!("{name}.parcel-index.db"))
    }

    pub fn download_path(&self, file_name: &str) -> PathBuf {
        self.downloads_dir().join(file_name)
    }

    pub fn receipt_path(&self, name: &str) -> PathBuf {
        self.receipts_dir().join(format!("{name}.json"))
    }

    pub fn cellar_package_dir(&self, name: &str) -> PathBuf {
        self.cellar_dir().join(name)
    }

    pub fn version_install_dir(&self, name: &str, version: &str) -> PathBuf {
        self.cellar_package_dir(name).join(version)
    }

    pub fn opt_link_path(&self, name: &str) -> PathBuf {
        self.opt_dir().join(name)
    }

    pub fn target_dir(&self, target: &str) -> PathBuf {
        match target {
            "bin" => self
                .data_home
                .parent()
                .map(|parent| parent.join("bin"))
                .unwrap_or_else(|| self.data_home.join("bin")),
            "applications" => self.data_home.join("applications"),
            "icons" => self.data_home.join("icons"),
            "man" => self.data_home.join("man"),
            _ => self.parcel_data_dir().join(target),
        }
    }

    pub fn temp_path(&self, prefix: &str, suffix: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        env::temp_dir().join(format!("parcel-{prefix}-{nanos}{suffix}"))
    }
}

fn env_path(key: &str) -> Option<PathBuf> {
    env::var_os(key).map(PathBuf::from)
}

pub fn path_file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("artifact")
        .to_string()
}
