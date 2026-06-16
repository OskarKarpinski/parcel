//! Serializable data models for package metadata, local state, and repositories.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Metadata stored in a `.parcel` archive.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    pub arch: String,
    pub description: String,
    pub homepage: String,
    #[serde(default)]
    pub actions: Vec<Action>,
}

/// A file action requested by package metadata.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Action {
    pub source: String,
    pub target: String,
    #[serde(rename = "type")]
    pub action_type: ActionType,
}

/// Supported desktop integration action kinds.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ActionType {
    Link,
    Copy,
}

/// The on-disk package database stored at `~/.local/share/parcel/parcel.db`.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct PackageDatabase {
    #[serde(default)]
    pub packages: BTreeMap<String, InstalledPackage>,
}

/// A package entry persisted in the local database.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct InstalledPackage {
    pub name: String,
    pub version: String,
    pub arch: String,
    pub install_path: PathBuf,
    pub source_repo: String,
    pub installed_at: String,
    pub actions: Vec<ResolvedAction>,
    pub files: Vec<String>,
}

/// An action with its absolute destination path resolved at install time.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ResolvedAction {
    pub source: String,
    pub target: String,
    #[serde(rename = "type")]
    pub action_type: ActionType,
    pub resolved_target: PathBuf,
}

/// Remote configuration stored separately from the package database.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct RemoteConfig {
    #[serde(default)]
    pub remotes: BTreeMap<String, Remote>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Remote {
    pub url: String,
    pub index_url: String,
}

/// Decoded JSON package index from a remote repository.
#[derive(Debug, Deserialize)]
pub struct RepositoryIndex {
    #[serde(rename = "_dl")]
    pub download_template: String,
    #[serde(rename = "_dl_delta", default)]
    pub _delta_template: Option<String>,
    #[serde(flatten)]
    pub packages: BTreeMap<String, IndexPackage>,
}

#[derive(Debug, Deserialize)]
pub struct IndexPackage {
    pub description: String,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub versions: BTreeMap<String, String>,
    #[serde(default)]
    pub _delta: BTreeMap<String, String>,
}

/// Supported payload compression algorithms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    Zstd,
    Xz,
}
