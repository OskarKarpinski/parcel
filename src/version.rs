//! Version and architecture helpers.

use std::cmp::Ordering;

use anyhow::{Result, bail};
use semver::Version;

/// Parsed version-release pair used for latest-version selection.
#[derive(Debug, Clone, Eq, PartialEq)]
struct ParcelVersion {
    semver: Version,
    release: u64,
}

pub fn latest_version<'a>(versions: impl Iterator<Item = &'a String>) -> Option<String> {
    versions
        .max_by(|left, right| compare_version_strings(left, right))
        .cloned()
}

pub fn compare_version_strings(left: &str, right: &str) -> Ordering {
    match (parse_parcel_version(left), parse_parcel_version(right)) {
        (Some(left), Some(right)) => left.cmp(&right),
        _ => left.cmp(right),
    }
}

pub fn is_newer(candidate: &str, installed: &str) -> bool {
    compare_version_strings(candidate, installed).is_gt()
}

fn parse_parcel_version(version: &str) -> Option<ParcelVersion> {
    let (semver_part, release_part) = version.rsplit_once('-')?;
    Some(ParcelVersion {
        semver: Version::parse(semver_part).ok()?,
        release: release_part.parse().ok()?,
    })
}

impl Ord for ParcelVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        self.semver
            .cmp(&other.semver)
            .then_with(|| self.release.cmp(&other.release))
    }
}

impl PartialOrd for ParcelVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

pub fn validate_package_arch(package_arch: &str) -> Result<()> {
    let current = current_arch();
    if package_arch == current {
        Ok(())
    } else {
        bail!("package architecture is {package_arch}, current architecture is {current}")
    }
}

pub fn current_arch() -> String {
    match std::env::consts::ARCH {
        "x86_64" => "x86_64".to_string(),
        "aarch64" => "aarch64".to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latest_version_uses_semver_and_release() {
        let versions = [
            "1.0.0-alpha01-1".to_string(),
            "1.0.0-2".to_string(),
            "1.0.1-1".to_string(),
        ];

        assert_eq!(latest_version(versions.iter()).as_deref(), Some("1.0.1-1"));
    }

    #[test]
    fn detects_upgrade_versions() {
        assert!(is_newer("1.0.1-1", "1.0.0-2"));
        assert!(is_newer("1.0.0-2", "1.0.0-1"));
        assert!(!is_newer("1.0.0-1", "1.0.0-1"));
    }
}
