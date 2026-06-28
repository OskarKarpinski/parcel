//! Build `.parcel` packages from package build manifests.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::{
    artifact::{package_file_name, write_package},
    cli::BuildArgs,
    parcel_manifest::{InstallAction, InstallActionKind, InstallTarget, PackageManifest},
    repo::fetch_to_path,
    utils::{
        arch::{Architecture, get_architecture},
        hash::verify_file_checksum,
    },
};

#[derive(Debug, Deserialize)]
struct BuildManifest {
    name: String,
    version: String,
    #[serde(default)]
    release: Option<u64>,
    description: String,
    homepage: Option<String>,
    architecture: Vec<Architecture>,
    #[serde(default)]
    delta: bool,
    #[serde(default)]
    source: Vec<String>,
    #[serde(default, rename = "source-x86-64", alias = "source-x86_64")]
    source_x86_64: Vec<String>,
    #[serde(default, rename = "source-aarch64")]
    source_aarch64: Vec<String>,
    build_script: Option<String>,
    install_script: Option<String>,
    #[serde(default)]
    files: BTreeMap<String, Vec<String>>,
}

#[derive(Debug)]
struct BuildDir {
    root: PathBuf,
    sources: PathBuf,
    build_root: PathBuf,
    output: PathBuf,
}

#[derive(Debug)]
struct SourceSpec {
    location: String,
    checksum: Option<String>,
}

impl BuildManifest {
    fn load(path: &Path) -> Result<Self> {
        let content =
            fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        serde_yaml::from_str(&content).map_err(Into::into)
    }
}

pub fn build_package(args: &BuildArgs) -> Result<()> {
    let manifest_path = resolve_manifest(Path::new(&args.manifest))?;
    let manifest_dir = manifest_path
        .parent()
        .context("build manifest must have a parent directory")?;
    let build_manifest = BuildManifest::load(&manifest_path)?;
    let arch = get_architecture();
    if !build_manifest.architecture.contains(&arch) {
        bail!(
            "unsupported architecture {} for {}",
            arch,
            build_manifest.name
        );
    }

    let release = args.release.or(build_manifest.release).unwrap_or(1);
    let full_version = format!("{}-{}", build_manifest.version, release);

    let build_root = Path::new(&args.build_dir).join(&build_manifest.name);
    if args.clear && build_root.exists() {
        fs::remove_dir_all(&build_root)
            .with_context(|| format!("remove {}", build_root.display()))?;
    }
    let build_dir = create_build_dir(&build_root)?;

    resolve_sources(&build_manifest, arch, manifest_dir, &build_dir)?;
    run_build_script(build_manifest.build_script.as_deref(), &build_dir)?;
    run_install_script(build_manifest.install_script.as_deref(), &build_dir)?;

    let package_manifest = PackageManifest {
        name: build_manifest.name.clone(),
        version: full_version.clone(),
        arch,
        description: build_manifest.description.clone(),
        homepage: build_manifest.homepage.clone(),
        actions: parse_actions(&build_manifest.files)?,
    };
    validate_actions_exist(&package_manifest, &build_dir.output)?;

    let output_path =
        build_dir
            .root
            .join(package_file_name(&build_manifest.name, &full_version, arch));
    let checksum = write_package(&package_manifest, &build_dir.output, &output_path)?;
    println!("Created {}", output_path.display());
    println!("Package hash (BLAKE2b-512): {checksum}");

    if build_manifest.delta {
        println!("warning: manifest field `delta` is reserved; use `parcel build-delta` instead");
    }

    Ok(())
}

fn resolve_manifest(path: &Path) -> Result<PathBuf> {
    if path.is_dir() {
        let candidate = path.join(format!(
            "{}.yml",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("")
        ));
        if candidate.exists() {
            return Ok(candidate);
        }
        bail!("no package manifest found in {}", path.display());
    }
    if !path.exists() {
        bail!("file not found: {}", path.display());
    }
    Ok(path.to_path_buf())
}

fn create_build_dir(root: &Path) -> Result<BuildDir> {
    if root.exists() {
        bail!(
            "build directory already exists: {} (use --clear to remove it)",
            root.display()
        );
    }
    let sources = root.join("sources");
    let build_root = root.join("build");
    let output = root.join("output");
    fs::create_dir_all(&sources)?;
    fs::create_dir_all(&build_root)?;
    fs::create_dir_all(&output)?;
    Ok(BuildDir {
        root: root.to_path_buf(),
        sources,
        build_root,
        output,
    })
}

fn resolve_sources(
    manifest: &BuildManifest,
    arch: Architecture,
    manifest_dir: &Path,
    build_dir: &BuildDir,
) -> Result<()> {
    for source in &manifest.source {
        resolve_source(&parse_source_spec(source), manifest_dir, build_dir)?;
    }
    match arch {
        Architecture::X86_64 => {
            for source in &manifest.source_x86_64 {
                resolve_source(&parse_source_spec(source), manifest_dir, build_dir)?;
            }
        }
        Architecture::AARCH64 => {
            for source in &manifest.source_aarch64 {
                resolve_source(&parse_source_spec(source), manifest_dir, build_dir)?;
            }
        }
    }
    Ok(())
}

fn resolve_source(spec: &SourceSpec, manifest_dir: &Path, build_dir: &BuildDir) -> Result<()> {
    let file_name = Path::new(&spec.location)
        .file_name()
        .and_then(|name| name.to_str())
        .context("source path must have a file name")?;
    let destination = build_dir.sources.join(file_name);

    if spec.location.starts_with("http://")
        || spec.location.starts_with("https://")
        || spec.location.starts_with("file://")
    {
        fetch_to_path(&spec.location, &destination)?;
    } else {
        let source_path = manifest_dir.join(&spec.location);
        fs::copy(&source_path, &destination).with_context(|| {
            format!(
                "copy source {} to {}",
                source_path.display(),
                destination.display()
            )
        })?;
    }

    if let Some(checksum) = &spec.checksum {
        verify_file_checksum(
            destination
                .to_str()
                .context("source path must be valid UTF-8")?,
            checksum,
        )?;
    }

    Ok(())
}

fn run_build_script(script: Option<&str>, build_dir: &BuildDir) -> Result<()> {
    if let Some(script) = script {
        run_shell_script(script, build_dir)?;
    }
    Ok(())
}

fn run_install_script(script: Option<&str>, build_dir: &BuildDir) -> Result<()> {
    if let Some(script) = script {
        run_shell_script(script, build_dir)?;
    }

    let has_files = walkdir::WalkDir::new(&build_dir.output)
        .into_iter()
        .filter_map(Result::ok)
        .any(|entry| entry.file_type().is_file() || entry.file_type().is_symlink());
    if !has_files {
        bail!("install_script must create at least one payload entry");
    }
    Ok(())
}

fn run_shell_script(script: &str, build_dir: &BuildDir) -> Result<()> {
    let status = Command::new("bash")
        .args(["--noprofile", "--norc", "-euxo", "pipefail", "-c", script])
        .current_dir(&build_dir.build_root)
        .env("SOURCE_DIR", fs::canonicalize(&build_dir.sources)?)
        .env("OUTPUT_DIR", fs::canonicalize(&build_dir.output)?)
        .status()
        .context("spawn bash")?;
    if !status.success() {
        bail!("script exited with {status}");
    }
    Ok(())
}

fn parse_source_spec(spec: &str) -> SourceSpec {
    if let Some((location, checksum)) = spec.rsplit_once(':') {
        if checksum.chars().all(|ch| ch.is_ascii_hexdigit()) && !checksum.is_empty() {
            return SourceSpec {
                location: location.to_string(),
                checksum: Some(checksum.to_string()),
            };
        }
    }

    SourceSpec {
        location: spec.to_string(),
        checksum: None,
    }
}

fn parse_actions(files: &BTreeMap<String, Vec<String>>) -> Result<Vec<InstallAction>> {
    let mut actions = Vec::new();
    for (target_name, entries) in files {
        let target = match target_name.as_str() {
            "bin" => InstallTarget::Bin,
            "applications" => InstallTarget::Applications,
            "desktop" => InstallTarget::Desktop,
            "icons" => InstallTarget::Icons,
            "man" => InstallTarget::Man,
            other => bail!("unsupported target category {other}"),
        };
        for entry in entries {
            let (source, mode) = entry
                .rsplit_once(':')
                .with_context(|| format!("invalid file action {entry}"))?;
            let kind = match mode {
                "link" => InstallActionKind::Link,
                "copy" => InstallActionKind::Copy,
                other => bail!("unsupported file action mode {other}"),
            };
            actions.push(InstallAction {
                source: source.to_string(),
                target: target.clone(),
                kind,
            });
        }
    }
    Ok(actions)
}

fn validate_actions_exist(manifest: &PackageManifest, output_dir: &Path) -> Result<()> {
    for action in &manifest.actions {
        let path = output_dir.join(&action.source);
        if !path.exists() {
            bail!("payload source {} is missing", path.display());
        }
    }
    Ok(())
}
