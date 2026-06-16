//! Remote repository and cached index operations.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use blake2::{Blake2b512, Digest};

use crate::cli::RemoteCommand;
use crate::models::{IndexPackage, Remote, RepositoryIndex};
use crate::paths::Paths;
use crate::storage::{atomic_write, decode_zstd_json, load_remotes, save_remotes};
use crate::version::{current_arch, latest_version};

/// A concrete remote package version selected from cached indexes.
#[derive(Debug, Clone)]
pub struct PackageCandidate {
    pub remote_name: String,
    pub name: String,
    pub version: String,
    pub checksum: String,
    pub download_url: String,
    pub description: String,
    pub homepage: Option<String>,
}

pub fn remote_command(paths: &Paths, command: RemoteCommand) -> Result<()> {
    match command {
        RemoteCommand::Add { name, url } => add_remote(paths, &name, &url),
        RemoteCommand::Remove { name } => remove_remote(paths, &name),
        RemoteCommand::List => list_remotes(paths),
    }
}

pub fn add_remote(paths: &Paths, name: &str, url: &str) -> Result<()> {
    validate_remote_name(name)?;
    let mut config = load_remotes(paths)?;
    if config.remotes.contains_key(name) {
        bail!("remote already exists: {name}");
    }

    config.remotes.insert(
        name.to_string(),
        Remote {
            url: url.to_string(),
            index_url: normalize_remote_index_url(url),
        },
    );
    save_remotes(paths, &config)?;
    println!("added remote {name}");
    Ok(())
}

pub fn remove_remote(paths: &Paths, name: &str) -> Result<()> {
    let mut config = load_remotes(paths)?;
    if config.remotes.remove(name).is_none() {
        bail!("remote does not exist: {name}");
    }
    save_remotes(paths, &config)?;

    let index_path = remote_index_path(paths, name)?;
    if index_path.exists() {
        fs::remove_file(&index_path)
            .with_context(|| format!("remove cached index {}", index_path.display()))?;
    }

    println!("removed remote {name}");
    Ok(())
}

pub fn list_remotes(paths: &Paths) -> Result<()> {
    let config = load_remotes(paths)?;
    if config.remotes.is_empty() {
        println!("no remotes configured");
        return Ok(());
    }

    println!("{:<20} INDEX", "NAME");
    for (name, remote) in config.remotes {
        println!("{:<20} {}", name, remote.index_url);
    }
    Ok(())
}

pub fn update_indexes(paths: &Paths) -> Result<()> {
    let config = load_remotes(paths)?;
    if config.remotes.is_empty() {
        bail!("no remotes configured");
    }

    fs::create_dir_all(&paths.indexes_dir)
        .with_context(|| format!("create {}", paths.indexes_dir.display()))?;

    for (name, remote) in config.remotes {
        let bytes = fetch_bytes(&remote.index_url)
            .with_context(|| format!("download index for remote '{name}'"))?;
        let _: RepositoryIndex =
            decode_zstd_json(&bytes).with_context(|| format!("parse index for remote '{name}'"))?;
        let index_path = remote_index_path(paths, &name)?;
        atomic_write(&index_path, &bytes)
            .with_context(|| format!("write cached index {}", index_path.display()))?;
        println!("updated {name}");
    }

    Ok(())
}

pub fn search_indexes(paths: &Paths, query: &str) -> Result<()> {
    let query = query.to_lowercase();
    let indexes = load_cached_indexes(paths)?;
    if indexes.is_empty() {
        bail!("no cached indexes found; run `parcel update` first");
    }

    let mut found = false;
    for (remote_name, index) in &indexes {
        for (package_name, package) in &index.packages {
            if package_matches(package_name, package, &query) {
                let latest = latest_version(package.versions.keys())
                    .unwrap_or_else(|| "unknown".to_string());
                println!(
                    "{:<16} {:<24} {:<18} {}",
                    remote_name, package_name, latest, package.description
                );
                found = true;
            }
        }
    }

    if !found {
        println!("no packages found");
    }

    Ok(())
}

pub fn find_latest_candidate(paths: &Paths, name: &str) -> Result<Option<PackageCandidate>> {
    let arch = current_arch();
    let mut best: Option<PackageCandidate> = None;

    for (remote_name, index) in load_cached_indexes(paths)? {
        let Some(package) = index.packages.get(name) else {
            continue;
        };
        let Some(version) = latest_version(package.versions.keys()) else {
            continue;
        };
        let checksum = package
            .versions
            .get(&version)
            .ok_or_else(|| anyhow!("remote package version disappeared: {name} {version}"))?;
        let candidate = PackageCandidate {
            remote_name,
            name: name.to_string(),
            version: version.clone(),
            checksum: checksum.clone(),
            download_url: expand_download_template(&index.download_template, name, &version, &arch),
            description: package.description.clone(),
            homepage: package.homepage.clone(),
        };

        if best
            .as_ref()
            .map_or(true, |current| crate::version::is_newer(&candidate.version, &current.version))
        {
            best = Some(candidate);
        }
    }

    Ok(best)
}

pub fn download_candidate(candidate: &PackageCandidate) -> Result<Vec<u8>> {
    let bytes = fetch_bytes(&candidate.download_url)
        .with_context(|| format!("download package {} {}", candidate.name, candidate.version))?;
    verify_blake2b(&bytes, &candidate.checksum).with_context(|| {
        format!(
            "verify checksum for {} {}",
            candidate.name, candidate.version
        )
    })?;
    Ok(bytes)
}

pub fn load_cached_indexes(paths: &Paths) -> Result<BTreeMap<String, RepositoryIndex>> {
    let config = load_remotes(paths)?;
    let mut indexes = BTreeMap::new();

    for name in config.remotes.keys() {
        let index_path = remote_index_path(paths, name)?;
        if !index_path.exists() {
            continue;
        }
        let bytes =
            fs::read(&index_path).with_context(|| format!("read {}", index_path.display()))?;
        let index: RepositoryIndex = decode_zstd_json(&bytes)
            .with_context(|| format!("parse cached index {}", index_path.display()))?;
        indexes.insert(name.clone(), index);
    }

    Ok(indexes)
}

pub fn normalize_remote_index_url(url: &str) -> String {
    let trimmed = url.trim_end_matches('/');
    if trimmed.ends_with("parcel-index.db") {
        return trimmed.to_string();
    }
    if trimmed.contains("github.com/") {
        return format!("{trimmed}/releases/download/parcel-index/parcel-index.db");
    }
    trimmed.to_string()
}

fn package_matches(name: &str, package: &IndexPackage, query: &str) -> bool {
    let description = package.description.to_lowercase();
    let homepage = package.homepage.as_deref().unwrap_or("").to_lowercase();
    name.to_lowercase().contains(query) || description.contains(query) || homepage.contains(query)
}

fn expand_download_template(template: &str, name: &str, version: &str, arch: &str) -> String {
    template
        .replace("{name}", name)
        .replace("{version}", version)
        .replace("{arch}", arch)
}

fn validate_remote_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("remote name cannot be empty");
    }
    if !name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        bail!("remote names may only contain ASCII letters, digits, '-' and '_'");
    }
    Ok(())
}

fn remote_index_path(paths: &Paths, name: &str) -> Result<PathBuf> {
    validate_remote_name(name)?;
    Ok(paths.indexes_dir.join(format!("{name}.db")))
}

fn fetch_bytes(url: &str) -> Result<Vec<u8>> {
    if let Some(path) = url.strip_prefix("file://") {
        return fs::read(path).with_context(|| format!("read {url}"));
    }

    let path = Path::new(url);
    if path.exists() {
        return fs::read(path).with_context(|| format!("read {}", path.display()));
    }

    let response = ureq::get(url).call().map_err(Box::new)?;
    let mut reader = response.into_reader();
    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .with_context(|| format!("read response body from {url}"))?;
    Ok(bytes)
}

fn verify_blake2b(bytes: &[u8], expected_hex: &str) -> Result<()> {
    let actual = blake2b_hex(bytes);
    if actual.eq_ignore_ascii_case(expected_hex) {
        Ok(())
    } else {
        bail!("checksum mismatch: expected {expected_hex}, got {actual}")
    }
}

fn blake2b_hex(bytes: &[u8]) -> String {
    let digest = Blake2b512::digest(bytes);
    hex::encode(digest)
}
