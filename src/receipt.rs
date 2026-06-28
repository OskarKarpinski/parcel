use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::layout::Layout;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Receipt {
    pub name: String,
    pub version: String,
    pub arch: String,
    pub source: InstallSource,
    pub package_checksum: Option<String>,
    pub installed_at: chrono::DateTime<chrono::Utc>,
    pub install_dir: PathBuf,
    pub opt_link: PathBuf,
    #[serde(default)]
    pub exposed_paths: Vec<ExposedPath>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExposedPath {
    pub target: PathBuf,
    pub kind: ExposureKind,
    pub source: PathBuf,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ExposureKind {
    Link,
    Copy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InstallSource {
    LocalFile { path: PathBuf },
    Repository { repo: String, url: String },
}

impl Receipt {
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
        serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))
    }

    pub fn save(&self, layout: &Layout) -> Result<()> {
        let path = layout.receipt_path(&self.name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        let bytes = serde_json::to_vec_pretty(self)?;
        fs::write(&path, bytes).with_context(|| format!("write {}", path.display()))
    }
}

pub fn load_receipt(layout: &Layout, name: &str) -> Result<Option<Receipt>> {
    let path = layout.receipt_path(name);
    if !path.exists() {
        return Ok(None);
    }
    Receipt::load(&path).map(Some)
}

pub fn installed_receipts(layout: &Layout) -> Result<Vec<Receipt>> {
    let dir = layout.receipts_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut receipts = Vec::new();
    for entry in fs::read_dir(&dir).with_context(|| format!("read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        receipts.push(Receipt::load(&path)?);
    }
    receipts.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(receipts)
}

pub fn list_installed_packages() -> Result<()> {
    let layout = Layout::detect()?;
    layout.ensure_all()?;

    for receipt in installed_receipts(&layout)? {
        println!("{} {}", receipt.name, receipt.version);
    }

    Ok(())
}
