//! Build `.parcel` packages from package build manifests.

use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Cursor,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::{
    cli::BuildArgs,
    parcel_manifest::ParcelManifest,
    utils::{
        arch::{Architecture, get_architecture},
        hash::verify_file_checksum,
    },
};

#[derive(Debug, Deserialize)]
struct BuildManifest {
    name: String,
    version: String,
    release: usize,
    description: String,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default)]
    architecture: Vec<Architecture>,
    #[serde(default)]
    delta: bool,
    #[serde(default)]
    source: Vec<String>,
    #[serde(default, rename = "source-x86-64", alias = "source-x86_64")]
    source_x86_64: Vec<String>,
    #[serde(default, rename = "source-aarch64")]
    source_aarch64: Vec<String>,
    #[serde(default)]
    build_script: Option<String>,
    #[serde(default)]
    install_script: Option<String>,
    #[serde(default)]
    files: BTreeMap<String, Vec<String>>,
}

impl BuildManifest {
    fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        serde_yaml::from_str(&content).map_err(|e| anyhow::anyhow!(e))
    }
}

#[derive(Debug)]
struct SourceSpec {
    location: String,
    checksum: Option<String>,
}

/// Build `.parcel` archive for package manifest.
pub fn build_package(args: &BuildArgs) -> Result<()> {
    let manifest_path = resolve_manifest(Path::new(&args.manifest))?;
    println!(
        "Preparing to build package from {}",
        manifest_path.display()
    );
    let build_manifest = BuildManifest::load(&manifest_path)?;

    println!("Package name: {}", build_manifest.name);
    println!("Package version: {}", build_manifest.version);

    let current_arch = get_architecture();
    println!("Target architecture: {}", current_arch);

    if !build_manifest.architecture.contains(&current_arch) {
        return Err(anyhow::anyhow!(
            "Unsupported architecture for this package: {}. Supported architectures: {}",
            current_arch,
            build_manifest
                .architecture
                .iter()
                .map(|a| a.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    let build_time = chrono::Utc::now();
    println!("Build time: {}", build_time);

    let build_dir_path = Path::new(&args.build_dir).join(&build_manifest.name);
    if args.clear && build_dir_path.exists() {
        println!("Clearing build directory: {}", build_dir_path.display());
        fs::remove_dir_all(&build_dir_path)?;
    }
    let build_dir = setup_build_dir(&build_dir_path)?;

    println!("Source directory: {}", build_dir.sources.display());
    println!("Build root: {}", build_dir.build_root.display());
    println!("Output directory: {}", build_dir.output.display());

    resolve_sources(
        &current_arch,
        &build_manifest,
        &build_dir,
        manifest_path.parent().unwrap(),
    )?;
    run_build_script(&build_manifest, &build_dir)?;
    run_install_script(&build_manifest, &build_dir)?;

    // Generate archive manifest
    let archive_manifest = ParcelManifest {
        name: build_manifest.name.clone(),
        version: build_manifest.version,
        release: build_manifest.release,
        architecture: current_arch,
        description: build_manifest.description.clone(),
        homepage: build_manifest.homepage.clone(),
        files: build_manifest.files.clone(),
    };

    create_parcel_package(&archive_manifest, &build_dir)?;

    // TODO: delta
    if build_manifest.delta {
        println!("TODO: delta");
    }

    Ok(())
}

fn resolve_manifest(path: &Path) -> Result<PathBuf> {
    if path.is_dir() {
        let filename = format!(
            "{}.yml",
            path.file_name().and_then(|n| n.to_str()).unwrap_or("")
        );

        let final_path = path.join(filename);
        if !final_path.exists() {
            bail!("File not found: {}", final_path.display());
        }

        return Ok(final_path);
    }

    if !path.exists() {
        bail!("File not found: {}", path.display());
    }

    Ok(path.to_path_buf())
}

struct BuildDir {
    sources: PathBuf,
    build_root: PathBuf,
    output: PathBuf,
}

fn setup_build_dir(build_dir: &Path) -> Result<BuildDir> {
    if build_dir.exists() {
        bail!(
            "Build directory already exists: {}. Run with --clear to delete it.",
            build_dir.display()
        );
    }

    fs::create_dir_all(build_dir)?;

    // Setup sources, build root and output directories
    let sources = build_dir.join("sources");
    let build_root = build_dir.join("build");
    let output = build_dir.join("output");

    fs::create_dir_all(&sources)?;
    fs::create_dir_all(&build_root)?;
    fs::create_dir_all(&output)?;

    Ok(BuildDir {
        sources,
        build_root,
        output,
    })
}

fn resolve_sources(
    arch: &Architecture,
    manifest: &BuildManifest,
    build_dir: &BuildDir,
    manifest_dir: &Path,
) -> Result<()> {
    println!("==> Resolving sources");

    for source in &manifest.source {
        println!("  - {}", source);

        let spec = parse_source_spec(source);
        resolve_source(&spec, build_dir, manifest_dir)?;
    }

    // Resolve architecture-specific sources
    if *arch == Architecture::X86_64 {
        println!("==> Resolving {arch} sources");
        for source in &manifest.source_x86_64 {
            println!("  - {}", source);

            let spec = parse_source_spec(source);
            resolve_source(&spec, build_dir, manifest_dir)?;
        }
    }
    if *arch == Architecture::AARCH64 {
        println!("==> Resolving {arch} sources");
        for source in &manifest.source_aarch64 {
            println!("  - {}", source);

            let spec = parse_source_spec(source);
            resolve_source(&spec, build_dir, manifest_dir)?;
        }
    }

    Ok(())
}

fn resolve_source(spec: &SourceSpec, build_dir: &BuildDir, manifest_dir: &Path) -> Result<()> {
    if spec.location.starts_with("https://") {
        bail!("TODO: add support for downloading sources");
    }

    // verify checksum
    if let Some(checksum) = &spec.checksum {
        verify_file_checksum(&spec.location, checksum)?;
    }

    // copy source to build directory
    let dest = build_dir.sources.join(&spec.location);
    fs::create_dir_all(dest.parent().unwrap())?;
    fs::copy(manifest_dir.join(&spec.location), &dest)?;

    Ok(())
}

fn run_build_script(manifest: &BuildManifest, build_dir: &BuildDir) -> Result<()> {
    if let Some(script) = &manifest.build_script {
        println!("==> Running build script");
        run_bash_script(script, build_dir)?;
    }

    Ok(())
}

fn run_install_script(manifest: &BuildManifest, build_dir: &BuildDir) -> Result<()> {
    if let Some(script) = &manifest.install_script {
        println!("==> Running install script");
        run_bash_script(script, build_dir)?;
    }

    // Validate that OUTPUT_DIR contains at least one file
    let has_files = walkdir::WalkDir::new(&build_dir.output)
        .into_iter()
        .filter_map(|e| e.ok())
        .any(|e| e.file_type().is_file());

    if !has_files {
        bail!("install_script must create at least one file in OUTPUT_DIR");
    }

    Ok(())
}

fn run_bash_script(script: &str, build_dir: &BuildDir) -> Result<()> {
    let status = std::process::Command::new("bash")
        .args(["--noprofile", "--norc", "-x", "-c", script])
        .current_dir(&build_dir.build_root)
        .env("SOURCE_DIR", fs::canonicalize(&build_dir.sources)?)
        .env("OUTPUT_DIR", fs::canonicalize(&build_dir.output)?)
        .status()?;

    if !status.success() {
        bail!("Script exited with status: {}", status);
    }

    Ok(())
}

fn parse_source_spec(spec: &str) -> SourceSpec {
    if let Some((location, checksum)) = spec.rsplit_once(':') {
        return SourceSpec {
            location: location.to_string(),
            checksum: Some(checksum.to_string()),
        };
    }

    SourceSpec {
        location: spec.to_string(),
        checksum: None,
    }
}

fn create_parcel_package(manifest: &ParcelManifest, build_dir: &BuildDir) -> Result<()> {
    let parcel_filename = format!(
        "{name}-{version}-{release}-{architecture}.parcel",
        name = manifest.name,
        version = manifest.version,
        release = manifest.release,
        architecture = manifest.architecture,
    );
    let parcel_path = build_dir.output.join(&parcel_filename);

    // Package the output directory into data.tar.zst
    let payload_tar = create_payload_tar(&build_dir.output)?;
    let compressed_payload = compress_payload(payload_tar)?;

    // Create the final parcel package
    let file = File::create(&parcel_path)?;
    let mut outer = tar::Builder::new(file);
    append_bytes_tar(
        &mut outer,
        "manifest.yml",
        serde_yaml::to_string(manifest)?.as_bytes(),
    )?;
    append_bytes_tar(&mut outer, "data.tar.zst", &compressed_payload)?;
    outer.finish().context("finish parcel package archive")?;

    println!("Created parcel package: {}", parcel_path.display());

    // TODO: compute hash

    Ok(())
}

fn create_payload_tar(payload_dir: &Path) -> Result<Vec<u8>> {
    let mut tar = tar::Builder::new(Vec::new());
    tar.append_dir_all(".", payload_dir)
        .with_context(|| format!("create payload tar from {}", payload_dir.display()))?;
    tar.into_inner().context("finish payload tar")
}

fn compress_payload(payload_tar: Vec<u8>) -> Result<Vec<u8>> {
    zstd::encode_all(Cursor::new(payload_tar), 0).context("compress payload tar with zstd")
}

fn append_bytes_tar<W: std::io::Write>(
    builder: &mut tar::Builder<W>,
    path: &str,
    bytes: &[u8],
) -> Result<()> {
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    builder
        .append_data(&mut header, path, bytes)
        .with_context(|| format!("append {path} to package archive"))?;
    Ok(())
}
