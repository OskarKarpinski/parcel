use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::utils::arch::Architecture;

#[derive(Debug, Serialize, Deserialize)]
pub struct ParcelManifest {
    pub name: String,
    pub version: String,
    pub release: usize,
    pub description: String,
    #[serde(default)]
    pub homepage: Option<String>,
    pub architecture: Architecture,
    pub files: BTreeMap<String, Vec<String>>,
}
