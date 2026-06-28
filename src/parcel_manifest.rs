use serde::{Deserialize, Serialize};

use crate::utils::arch::Architecture;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackageManifest {
    pub name: String,
    pub version: String,
    pub arch: Architecture,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(default)]
    pub actions: Vec<InstallAction>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstallAction {
    pub source: String,
    pub target: InstallTarget,
    #[serde(rename = "type")]
    pub kind: InstallActionKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum InstallActionKind {
    Link,
    Copy,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum InstallTarget {
    Bin,
    Applications,
    Desktop,
    Icons,
    Man,
}

impl InstallTarget {
    pub fn canonical(self) -> Self {
        match self {
            Self::Desktop => Self::Applications,
            other => other,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self.canonical() {
            Self::Bin => "bin",
            Self::Applications => "applications",
            Self::Desktop => "applications",
            Self::Icons => "icons",
            Self::Man => "man",
        }
    }
}
