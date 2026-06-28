use std::cmp::Ordering;

use anyhow::{Context, Result};

use crate::{
    repo::{
        ArtifactRecord, CachedRepo, RepoArchitecture, RepoPackage, render_delta_download_url,
        render_download_url, resolve_package,
    },
    utils::arch::Architecture,
};

#[derive(Debug, Clone)]
pub struct PackageSelection {
    pub name: String,
    pub repo: CachedRepo,
    pub package: RepoPackage,
    pub arch: Architecture,
    pub arch_data: RepoArchitecture,
}

#[derive(Debug, Clone)]
pub struct ResolvedPackageArtifact {
    pub version: String,
    pub checksum: String,
    pub size: u64,
    pub url: String,
    pub repo_name: String,
}

#[derive(Debug, Clone)]
pub struct ResolvedDeltaArtifact {
    pub from_version: String,
    pub to_version: String,
    pub checksum: String,
    pub size: u64,
    pub url: String,
    pub repo_name: String,
}

#[derive(Debug, Clone)]
pub enum UpgradeStep {
    Full(ResolvedPackageArtifact),
    Delta(ResolvedDeltaArtifact),
}

#[derive(Debug, Clone)]
pub struct UpgradePlan {
    pub target_version: String,
    pub steps: Vec<UpgradeStep>,
    pub total_size: u64,
}

pub fn resolve_install_target(
    name: &str,
    requested_version: Option<&str>,
) -> Result<Option<ResolvedPackageArtifact>> {
    let selection = resolve_package(name)?;
    let Some(selection) = selection else {
        return Ok(None);
    };
    let version = if let Some(requested) = requested_version {
        requested.to_string()
    } else {
        latest_version(selection.arch_data.versions.keys().cloned())
            .context("package has no versions for current architecture")?
    };
    let record = selection
        .arch_data
        .versions
        .get(&version)
        .with_context(|| format!("version {version} not found for {name}"))?;

    Ok(Some(resolve_full_artifact(&selection, &version, record)))
}

pub fn resolve_upgrade_plan(name: &str, current_version: &str) -> Result<Option<UpgradePlan>> {
    let selection = resolve_package(name)?;
    let Some(selection) = selection else {
        return Ok(None);
    };

    let target_version = latest_version(selection.arch_data.versions.keys().cloned())
        .context("package has no available versions")?;
    if compare_versions(&target_version, current_version).is_le() {
        return Ok(None);
    }

    let full_record = selection
        .arch_data
        .versions
        .get(&target_version)
        .with_context(|| format!("target version {target_version} missing"))?;
    let full_artifact = resolve_full_artifact(&selection, &target_version, full_record);
    let mut best = UpgradePlan {
        target_version: target_version.clone(),
        steps: vec![UpgradeStep::Full(full_artifact.clone())],
        total_size: full_artifact.size,
    };

    if selection.repo.index.delta_download_template.is_some() {
        let mut path = Vec::new();
        let mut visited = vec![current_version.to_string()];
        dfs_delta_paths(
            &selection,
            current_version,
            &target_version,
            0,
            &mut path,
            &mut visited,
            &mut best,
        )?;
    }

    Ok(Some(best))
}

pub fn latest_version<I>(versions: I) -> Option<String>
where
    I: IntoIterator<Item = String>,
{
    versions
        .into_iter()
        .max_by(|left, right| compare_versions(left, right))
}

pub fn compare_versions(left: &str, right: &str) -> Ordering {
    let left = parse_version(left);
    let right = parse_version(right);

    for (lhs, rhs) in left.main.iter().zip(right.main.iter()) {
        match lhs.cmp(rhs) {
            Ordering::Equal => {}
            other => return other,
        }
    }
    match left.main.len().cmp(&right.main.len()) {
        Ordering::Equal => left
            .release
            .cmp(&right.release)
            .then_with(|| left.raw.cmp(&right.raw)),
        other => other,
    }
}

#[derive(Debug)]
struct ParsedVersion<'a> {
    raw: &'a str,
    main: Vec<u64>,
    release: u64,
}

fn parse_version(version: &str) -> ParsedVersion<'_> {
    let (main_part, release_part) = version.rsplit_once('-').unwrap_or((version, "0"));
    ParsedVersion {
        raw: version,
        main: main_part
            .split('.')
            .map(|part| part.parse::<u64>().unwrap_or(0))
            .collect(),
        release: release_part.parse::<u64>().unwrap_or(0),
    }
}

fn resolve_full_artifact(
    selection: &PackageSelection,
    version: &str,
    record: &ArtifactRecord,
) -> ResolvedPackageArtifact {
    ResolvedPackageArtifact {
        version: version.to_string(),
        checksum: record.checksum.clone(),
        size: record.size,
        url: render_download_url(
            &selection.repo.index.download_template,
            &selection.name,
            version,
            selection.arch,
        ),
        repo_name: selection.repo.config.name.clone(),
    }
}

fn dfs_delta_paths(
    selection: &PackageSelection,
    current_version: &str,
    target_version: &str,
    depth: usize,
    path: &mut Vec<ResolvedDeltaArtifact>,
    visited: &mut Vec<String>,
    best: &mut UpgradePlan,
) -> Result<()> {
    if depth >= 3 {
        return Ok(());
    }

    for (edge_key, record) in &selection.arch_data.deltas {
        let Some((from_version, to_version)) = edge_key.split_once("->") else {
            continue;
        };
        if from_version != current_version {
            continue;
        }
        if visited.iter().any(|version| version == to_version) {
            continue;
        }
        if compare_versions(to_version, current_version).is_le() {
            continue;
        }

        let template = selection
            .repo
            .index
            .delta_download_template
            .as_ref()
            .context("delta template missing")?;
        let delta = ResolvedDeltaArtifact {
            from_version: from_version.to_string(),
            to_version: to_version.to_string(),
            checksum: record.checksum.clone(),
            size: record.size,
            url: render_delta_download_url(
                template,
                &selection.name,
                from_version,
                to_version,
                selection.arch,
            ),
            repo_name: selection.repo.config.name.clone(),
        };

        path.push(delta.clone());
        visited.push(to_version.to_string());

        if to_version == target_version {
            let total_size = path.iter().map(|item| item.size).sum();
            if total_size < best.total_size {
                best.steps = path.iter().cloned().map(UpgradeStep::Delta).collect();
                best.total_size = total_size;
            }
        } else {
            dfs_delta_paths(
                selection,
                to_version,
                target_version,
                depth + 1,
                path,
                visited,
                best,
            )?;
        }

        path.pop();
        visited.pop();
    }

    Ok(())
}
