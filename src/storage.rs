//! Persistence helpers for Parcel's zstd-compressed database and JSON config.

use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{Cursor, Write};
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};

use crate::models::{PackageDatabase, RemoteConfig};
use crate::paths::Paths;

pub fn load_database(paths: &Paths) -> Result<PackageDatabase> {
    if !paths.database.exists() {
        return Ok(PackageDatabase::default());
    }
    let bytes = fs::read(&paths.database)
        .with_context(|| format!("read database {}", paths.database.display()))?;
    decode_zstd_json(&bytes).with_context(|| format!("parse database {}", paths.database.display()))
}

pub fn save_database(paths: &Paths, db: &PackageDatabase) -> Result<()> {
    let bytes = encode_zstd_json(db).context("encode package database")?;
    atomic_write(&paths.database, &bytes)
        .with_context(|| format!("write database {}", paths.database.display()))
}

pub fn load_remotes(paths: &Paths) -> Result<RemoteConfig> {
    if !paths.remotes.exists() {
        return Ok(RemoteConfig::default());
    }
    let contents = fs::read_to_string(&paths.remotes)
        .with_context(|| format!("read {}", paths.remotes.display()))?;
    serde_json::from_str(&contents).with_context(|| format!("parse {}", paths.remotes.display()))
}

pub fn save_remotes(paths: &Paths, config: &RemoteConfig) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(config).context("serialize remote config")?;
    atomic_write(&paths.remotes, &bytes)
        .with_context(|| format!("write {}", paths.remotes.display()))
}

pub fn encode_zstd_json<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let json = serde_json::to_vec_pretty(value).context("serialize JSON")?;
    zstd::encode_all(Cursor::new(json), 0).context("compress JSON with zstd")
}

pub fn decode_zstd_json<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T> {
    let json = zstd::decode_all(Cursor::new(bytes)).context("decompress zstd JSON")?;
    serde_json::from_slice(&json).context("parse JSON")
}

/// Write through a temporary file in the target directory, then rename.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }

    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("path has no parent: {}", path.display()))?;
    let file_name = path
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| anyhow!("path has no valid file name: {}", path.display()))?;

    let temp_name = format!(".{file_name}.tmp-{}", std::process::id());
    let temp_path = parent.join(temp_name);
    {
        let mut file =
            File::create(&temp_path).with_context(|| format!("create {}", temp_path.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("write {}", temp_path.display()))?;
        file.sync_all()
            .with_context(|| format!("sync {}", temp_path.display()))?;
    }
    fs::rename(&temp_path, path)
        .with_context(|| format!("rename {} to {}", temp_path.display(), path.display()))?;
    Ok(())
}
