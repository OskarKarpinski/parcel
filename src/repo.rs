use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::{
    cli::{InfoArgs, RepoAddArgs, RepoIndexArgs, RepoRemoveArgs, SearchArgs},
    delta::read_delta_metadata,
    layout::{Layout, path_file_name},
    resolver::{PackageSelection, compare_versions, latest_version},
    utils::arch::{Architecture, get_architecture},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoConfig {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RepoIndex {
    #[serde(rename = "_schema")]
    pub schema: String,
    #[serde(rename = "_generated_at")]
    pub generated_at: String,
    #[serde(rename = "_dl")]
    pub download_template: String,
    #[serde(rename = "_dl_delta", default, skip_serializing_if = "Option::is_none")]
    pub delta_download_template: Option<String>,
    #[serde(flatten)]
    pub packages: BTreeMap<String, RepoPackage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RepoPackage {
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(default)]
    pub architectures: BTreeMap<Architecture, RepoArchitecture>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RepoArchitecture {
    #[serde(default)]
    pub versions: BTreeMap<String, ArtifactRecord>,
    #[serde(default)]
    pub deltas: BTreeMap<String, ArtifactRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRecord {
    pub checksum: String,
    pub size: u64,
}

#[derive(Debug, Clone)]
pub struct CachedRepo {
    pub config: RepoConfig,
    pub index: RepoIndex,
}

pub fn add_repo(args: &RepoAddArgs) -> Result<()> {
    let layout = Layout::detect()?;
    layout.ensure_all()?;
    let path = layout.repo_config_path(&args.name);
    if path.exists() {
        bail!("repository {} already exists", args.name);
    }
    let config = RepoConfig {
        name: args.name.clone(),
        url: args.url.clone(),
    };
    save_repo_config(&path, &config)?;
    println!("Added repo {} -> {}", config.name, config.url);
    Ok(())
}

pub fn remove_repo(args: &RepoRemoveArgs) -> Result<()> {
    let layout = Layout::detect()?;
    layout.ensure_all()?;
    let path = layout.repo_config_path(&args.name);
    if path.exists() {
        fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
    }
    let cache = layout.index_cache_path(&args.name);
    if cache.exists() {
        fs::remove_file(&cache).with_context(|| format!("remove {}", cache.display()))?;
    }
    println!("Removed repo {}", args.name);
    Ok(())
}

pub fn update_indexes() -> Result<()> {
    let layout = Layout::detect()?;
    layout.ensure_all()?;
    for config in load_repo_configs(&layout)? {
        let url = normalize_repo_url(&config.url)?;
        let cache_path = layout.index_cache_path(&config.name);
        fetch_to_path(&url, &cache_path)?;
        println!("Updated {} from {}", config.name, url);
    }
    Ok(())
}

pub fn search_packages(args: &SearchArgs) -> Result<()> {
    let query = args.query.to_lowercase();
    for repo in load_cached_repos()? {
        for (name, package) in &repo.index.packages {
            let haystack = format!("{name}\n{}", package.description).to_lowercase();
            if haystack.contains(&query) {
                let latest = package
                    .architectures
                    .get(&get_architecture())
                    .and_then(|arch| latest_version(arch.versions.keys().cloned()))
                    .unwrap_or_else(|| "<unavailable>".into());
                println!("{name}\t{latest}\t{}", repo.config.name);
            }
        }
    }
    Ok(())
}

pub fn show_package_info(args: &InfoArgs) -> Result<()> {
    let selection =
        resolve_package(&args.package)?.context("package not found in cached repositories")?;
    println!("name: {}", selection.name);
    println!("repo: {}", selection.repo.config.name);
    println!("description: {}", selection.package.description);
    if let Some(homepage) = &selection.package.homepage {
        println!("homepage: {homepage}");
    }
    println!("arch: {}", selection.arch);
    let versions = selection
        .arch_data
        .versions
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    println!("versions: {}", versions.join(", "));
    if let Some(latest) = latest_version(selection.arch_data.versions.keys().cloned()) {
        println!("latest: {latest}");
    }
    println!("delta edges: {}", selection.arch_data.deltas.len());
    Ok(())
}

#[cfg(feature = "build")]
pub fn build_repo_index(args: &RepoIndexArgs) -> Result<()> {
    let artifacts_dir = Path::new(&args.artifacts_dir);
    let output_path = args
        .output
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| artifacts_dir.join("parcel-index.db"));
    fs::create_dir_all(artifacts_dir)
        .with_context(|| format!("create {}", artifacts_dir.display()))?;

    let base_url = args.base_url.trim_end_matches('/');
    let mut index = RepoIndex {
        schema: "parcel.v1".into(),
        generated_at: Utc::now().to_rfc3339(),
        download_template: format!("{base_url}/{{name}}-{{version}}-{{arch}}.parcel"),
        delta_download_template: Some(format!(
            "{base_url}/{{name}}-{{from_version}}-{{to_version}}-{{arch}}.delta.parcel"
        )),
        packages: BTreeMap::new(),
    };

    for entry in
        fs::read_dir(artifacts_dir).with_context(|| format!("read {}", artifacts_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("parcel") {
            continue;
        }
        let file_name = path_file_name(&path);
        if file_name.ends_with(".delta.parcel") {
            let (delta, manifest) = read_delta_metadata(&path)?;
            let package = index.packages.entry(manifest.name.clone()).or_default();
            package.description = manifest.description.clone();
            package.homepage = manifest.homepage.clone();
            let arch = package.architectures.entry(manifest.arch).or_default();
            arch.deltas.insert(
                format!("{}->{}", delta.from_version, delta.to_version),
                ArtifactRecord {
                    checksum: crate::utils::hash::hash_file_blake2b(&path)?,
                    size: fs::metadata(&path)?.len(),
                },
            );
            continue;
        }

        let manifest = crate::artifact::read_package_manifest(&path)?;
        let package = index.packages.entry(manifest.name.clone()).or_default();
        package.description = manifest.description.clone();
        package.homepage = manifest.homepage.clone();
        let arch = package.architectures.entry(manifest.arch).or_default();
        arch.versions.insert(
            manifest.version.clone(),
            ArtifactRecord {
                checksum: crate::utils::hash::hash_file_blake2b(&path)?,
                size: fs::metadata(&path)?.len(),
            },
        );
    }

    let json = serde_json::to_vec_pretty(&index)?;
    let file = fs::File::create(&output_path)
        .with_context(|| format!("create {}", output_path.display()))?;
    let mut encoder = zstd::Encoder::new(file, 0)?;
    encoder.write_all(&json)?;
    encoder.finish()?;
    println!("{}", output_path.display());
    Ok(())
}

pub fn load_cached_repos() -> Result<Vec<CachedRepo>> {
    let layout = Layout::detect()?;
    layout.ensure_all()?;
    let mut repos = Vec::new();
    for config in load_repo_configs(&layout)? {
        let path = layout.index_cache_path(&config.name);
        if !path.exists() {
            continue;
        }
        let index = read_repo_index(&path)?;
        repos.push(CachedRepo { config, index });
    }
    Ok(repos)
}

pub fn resolve_package(name: &str) -> Result<Option<PackageSelection>> {
    let arch = get_architecture();
    let mut best: Option<PackageSelection> = None;
    for repo in load_cached_repos()? {
        let Some(package) = repo.index.packages.get(name).cloned() else {
            continue;
        };
        let Some(arch_data) = package.architectures.get(&arch).cloned() else {
            continue;
        };
        let selection = PackageSelection {
            name: name.to_string(),
            repo,
            package,
            arch,
            arch_data,
        };
        if let Some(current) = &best {
            let current_latest =
                latest_version(current.arch_data.versions.keys().cloned()).unwrap_or_default();
            let candidate_latest =
                latest_version(selection.arch_data.versions.keys().cloned()).unwrap_or_default();
            if compare_versions(&candidate_latest, &current_latest).is_gt() {
                best = Some(selection);
            }
        } else {
            best = Some(selection);
        }
    }
    Ok(best)
}

pub fn normalize_repo_url(raw: &str) -> Result<String> {
    if raw.ends_with("parcel-index.db") {
        return Ok(raw.to_string());
    }

    if raw.contains("github.com/") {
        return Ok(format!(
            "{}/releases/download/parcel-index/parcel-index.db",
            raw.trim_end_matches('/')
        ));
    }

    if let Some(path) = raw.strip_prefix("file://") {
        let file_path = Path::new(path);
        if file_path.is_dir() {
            return Ok(format!(
                "file://{}",
                file_path.join("parcel-index.db").display()
            ));
        }
        return Ok(raw.to_string());
    }

    let path = Path::new(raw);
    if path.exists() && path.is_dir() {
        return Ok(path.join("parcel-index.db").display().to_string());
    }

    if raw.ends_with('/') {
        return Ok(format!("{raw}parcel-index.db"));
    }

    Ok(raw.to_string())
}

pub fn render_download_url(
    template: &str,
    name: &str,
    version: &str,
    arch: Architecture,
) -> String {
    template
        .replace("{name}", name)
        .replace("{version}", version)
        .replace("{arch}", &arch.to_string())
}

pub fn render_delta_download_url(
    template: &str,
    name: &str,
    from_version: &str,
    to_version: &str,
    arch: Architecture,
) -> String {
    template
        .replace("{name}", name)
        .replace("{from_version}", from_version)
        .replace("{to_version}", to_version)
        .replace("{arch}", &arch.to_string())
}

pub fn fetch_to_path(url: &str, destination: &Path) -> Result<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }

    if let Some(path) = url.strip_prefix("file://") {
        fs::copy(path, destination)
            .with_context(|| format!("copy {} to {}", path, destination.display()))?;
        return Ok(());
    }

    let local_path = Path::new(url);
    if local_path.exists() {
        fs::copy(local_path, destination).with_context(|| {
            format!("copy {} to {}", local_path.display(), destination.display())
        })?;
        return Ok(());
    }

    let status = Command::new("curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            url,
            "--output",
            destination
                .to_str()
                .context("download destination must be valid UTF-8")?,
        ])
        .status()
        .context("spawn curl")?;
    if !status.success() {
        bail!("curl failed downloading {url} with status {status}");
    }
    Ok(())
}

fn save_repo_config(path: &Path, config: &RepoConfig) -> Result<()> {
    let content = format!("name = \"{}\"\nurl = \"{}\"\n", config.name, config.url);
    fs::write(path, content).with_context(|| format!("write {}", path.display()))
}

fn load_repo_configs(layout: &Layout) -> Result<Vec<RepoConfig>> {
    let dir = layout.repos_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut configs = Vec::new();
    for entry in fs::read_dir(&dir).with_context(|| format!("read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
            continue;
        }
        configs.push(parse_repo_config(&path)?);
    }
    configs.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(configs)
}

fn parse_repo_config(path: &Path) -> Result<RepoConfig> {
    let content = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let mut name = None;
    let mut url = None;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(value) = line.strip_prefix("name = ") {
            name = Some(trim_quoted(value));
        } else if let Some(value) = line.strip_prefix("url = ") {
            url = Some(trim_quoted(value));
        }
    }
    Ok(RepoConfig {
        name: name.context("repo config missing name")?,
        url: url.context("repo config missing url")?,
    })
}

fn trim_quoted(input: &str) -> String {
    input.trim().trim_matches('"').to_string()
}

fn read_repo_index(path: &Path) -> Result<RepoIndex> {
    let file = fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut decoder = zstd::Decoder::new(file)?;
    let mut bytes = Vec::new();
    decoder.read_to_end(&mut bytes)?;
    serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))
}
