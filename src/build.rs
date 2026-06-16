//! Build `.parcel` packages from package build manifests.
//!
//! A build manifest describes inputs, build/install scripts, compression, and
//! desktop integration actions. The builder resolves sources into an isolated
//! source directory, runs scripts in a temporary build directory, assembles the
//! payload in `$OUTPUT_DIR`, then writes the package archive expected by the
//! installer.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use blake2::{Blake2b512, Digest};
use serde::Deserialize;
use tar::{Builder, Header};
use tempfile::TempDir;

use crate::archive::{list_payload_files, validate_relative_path};
use crate::cli::BuildArgs;
use crate::models::{Action, ActionType, Compression, Manifest};
use crate::version::current_arch;

/// Build-manifest schema from `Parcel.md`.
#[derive(Debug, Deserialize)]
struct BuildManifest {
    name: String,
    version: String,
    description: String,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default)]
    architecture: Vec<String>,
    #[serde(default)]
    delta: bool,
    #[serde(default)]
    compression: Option<CompressionName>,
    #[serde(default)]
    source: Vec<String>,
    #[serde(default)]
    build_script: Option<String>,
    #[serde(default)]
    install_script: Option<String>,
    #[serde(default)]
    files: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum CompressionName {
    Zstd,
    Xz,
}

impl CompressionName {
    fn package_compression(self) -> Compression {
        match self {
            Self::Zstd => Compression::Zstd,
            Self::Xz => Compression::Xz,
        }
    }

    fn data_file_name(self) -> &'static str {
        match self {
            Self::Zstd => "data.tar.zst",
            Self::Xz => "data.tar.xz",
        }
    }
}

#[derive(Debug)]
struct SourceSpec {
    location: String,
    checksum: Option<String>,
}

/// Build one `.parcel` archive for a package manifest.
pub fn build_package(args: &BuildArgs) -> Result<()> {
    let manifest_path = resolve_manifest_path(Path::new(&args.manifest))?;
    let manifest_dir = manifest_path
        .parent()
        .ok_or_else(|| anyhow!("manifest path has no parent: {}", manifest_path.display()))?;
    let build_manifest = load_build_manifest(&manifest_path)?;
    let arch = args.arch.clone().unwrap_or_else(current_arch);

    validate_build_request(&build_manifest, &arch)?;

    let compression = build_manifest.compression.unwrap_or(CompressionName::Zstd);
    let release_version = format!("{}-{}", build_manifest.version, args.release);
    let package_name = format!(
        "{}-{}-{}-{}.parcel",
        build_manifest.name, build_manifest.version, args.release, arch
    );
    let output_dir = args
        .output_dir
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest_dir.to_path_buf());
    let output_path = output_dir.join(package_name);

    print_build_header(
        &build_manifest,
        &manifest_path,
        &arch,
        args.release,
        compression,
        &output_path,
    );

    let workspace = create_build_workspace(args.build_dir.as_deref())?;
    let source_dir = workspace.path().join("sources");
    let build_dir = workspace.path().join("build");
    let package_output_dir = workspace.path().join("output");
    print_step("Preparing build workspace");
    print_detail("workspace", workspace.path().display());
    print_detail("sources", source_dir.display());
    print_detail("build", build_dir.display());
    print_detail("package output", package_output_dir.display());
    fs::create_dir_all(&source_dir).context("create source directory")?;
    fs::create_dir_all(&build_dir).context("create build directory")?;
    fs::create_dir_all(&package_output_dir).context("create package output directory")?;

    resolve_sources(&build_manifest.source, manifest_dir, &source_dir)?;
    run_script(
        build_manifest.build_script.as_deref(),
        &build_dir,
        &source_dir,
        &package_output_dir,
        "build_script",
    )?;
    run_script(
        build_manifest.install_script.as_deref(),
        &build_dir,
        &source_dir,
        &package_output_dir,
        "install_script",
    )?;

    let actions = build_actions(&build_manifest.files, &package_output_dir)?;
    print_step("Collecting package payload");
    let files = list_payload_files(&package_output_dir)?;
    print_detail("payload files", files.len());
    if files.is_empty() {
        bail!("install_script produced no package files in $OUTPUT_DIR");
    }

    let package_manifest = Manifest {
        name: build_manifest.name.clone(),
        version: release_version,
        arch,
        description: build_manifest.description.clone(),
        homepage: build_manifest.homepage.unwrap_or_default(),
        actions,
    };

    print_step("Writing package archive");
    print_detail("compression", compression.data_file_name());
    print_detail("output", output_path.display());
    fs::create_dir_all(&output_dir)
        .with_context(|| format!("create output directory {}", output_dir.display()))?;
    write_package_archive(
        &output_path,
        &package_output_dir,
        &package_manifest,
        compression,
    )?;

    print_step("Build complete");
    print_detail("built", output_path.display());
    if build_manifest.delta {
        print_detail("note", "delta package generation is not implemented yet");
    }
    Ok(())
}

fn print_build_header(
    manifest: &BuildManifest,
    manifest_path: &Path,
    arch: &str,
    release: u64,
    compression: CompressionName,
    output_path: &Path,
) {
    print_step("Starting Parcel build");
    print_detail("manifest", manifest_path.display());
    print_detail("package", &manifest.name);
    print_detail("version", &manifest.version);
    print_detail("release", release);
    print_detail("architecture", arch);
    print_detail("compression", compression.data_file_name());
    print_detail("sources", manifest.source.len());
    print_detail("file groups", manifest.files.len());
    print_detail("output", output_path.display());
}

fn print_step(message: &str) {
    println!("==> {message}");
}

fn print_detail(label: &str, value: impl std::fmt::Display) {
    println!("    {label}: {value}");
}

fn create_build_workspace(build_dir: Option<&str>) -> Result<TempDir> {
    match build_dir {
        Some(path) => {
            let path = Path::new(path);
            fs::create_dir_all(path)
                .with_context(|| format!("create build directory {}", path.display()))?;
            tempfile::Builder::new()
                .prefix("parcel-build-")
                .tempdir_in(path)
                .with_context(|| format!("create temporary build workspace in {}", path.display()))
        }
        None => TempDir::new().context("create build workspace"),
    }
}

fn resolve_manifest_path(path: &Path) -> Result<PathBuf> {
    if path.is_dir() {
        let mut candidates: Vec<_> = fs::read_dir(path)
            .with_context(|| format!("read {}", path.display()))?
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let entry_path = entry.path();
                let ext = entry_path.extension()?.to_str()?;
                matches!(ext, "yml" | "yaml").then_some(entry_path)
            })
            .collect();
        candidates.sort();
        return match candidates.len() {
            0 => bail!("no .yml or .yaml manifest found in {}", path.display()),
            1 => Ok(candidates.remove(0)),
            _ => bail!(
                "multiple YAML manifests found in {}; pass a manifest path explicitly",
                path.display()
            ),
        };
    }

    if path.exists() {
        Ok(path.to_path_buf())
    } else {
        bail!("build manifest does not exist: {}", path.display())
    }
}

fn load_build_manifest(path: &Path) -> Result<BuildManifest> {
    let contents =
        fs::read_to_string(path).with_context(|| format!("read manifest {}", path.display()))?;
    serde_yaml::from_str(&contents).with_context(|| format!("parse manifest {}", path.display()))
}

fn validate_build_request(manifest: &BuildManifest, arch: &str) -> Result<()> {
    if manifest.name.trim().is_empty() {
        bail!("manifest name cannot be empty");
    }
    if manifest.version.trim().is_empty() {
        bail!("manifest version cannot be empty");
    }
    if manifest.architecture.is_empty() {
        bail!("manifest architecture list cannot be empty");
    }
    if !manifest
        .architecture
        .iter()
        .any(|candidate| candidate == arch)
    {
        bail!(
            "architecture {arch} is not declared by manifest; available: {}",
            manifest.architecture.join(", ")
        );
    }
    Ok(())
}

fn resolve_sources(entries: &[String], manifest_dir: &Path, source_dir: &Path) -> Result<()> {
    print_step("Resolving sources");
    if entries.is_empty() {
        print_detail("sources", "none declared");
        return Ok(());
    }

    for entry in entries {
        let spec = parse_source_spec(entry);
        if spec.location.starts_with("http://") || spec.location.starts_with("https://") {
            print_detail("download", &spec.location);
        } else {
            print_detail(
                "read",
                source_location_display(&spec.location, manifest_dir).display(),
            );
        }

        let bytes = read_source_bytes(&spec.location, manifest_dir)?;
        if let Some(expected) = spec.checksum {
            print_detail("verify blake2b", &spec.location);
            verify_blake2b(&bytes, &expected)
                .with_context(|| format!("verify checksum for source {}", spec.location))?;
        }

        let file_name = source_file_name(&spec.location)?;
        let target = source_dir.join(file_name);
        print_detail(
            "write source",
            format!("{} ({} bytes)", target.display(), bytes.len()),
        );
        fs::write(&target, bytes).with_context(|| format!("write source {}", target.display()))?;
    }
    Ok(())
}

fn parse_source_spec(entry: &str) -> SourceSpec {
    if let Some((location, checksum)) = entry.rsplit_once(':')
        && is_hex_checksum(checksum)
    {
        return SourceSpec {
            location: location.to_string(),
            checksum: Some(checksum.to_string()),
        };
    }

    SourceSpec {
        location: entry.to_string(),
        checksum: None,
    }
}

fn is_hex_checksum(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn read_source_bytes(location: &str, manifest_dir: &Path) -> Result<Vec<u8>> {
    if location.starts_with("http://") || location.starts_with("https://") {
        let response = ureq::get(location).call().map_err(Box::new)?;
        let mut reader = response.into_reader();
        let mut bytes = Vec::new();
        reader
            .read_to_end(&mut bytes)
            .with_context(|| format!("read response body from {location}"))?;
        return Ok(bytes);
    }

    let path = if let Some(stripped) = location.strip_prefix("file://") {
        PathBuf::from(stripped)
    } else {
        let path = Path::new(location);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            manifest_dir.join(path)
        }
    };

    fs::read(&path).with_context(|| format!("read source {}", path.display()))
}

fn source_location_display(location: &str, manifest_dir: &Path) -> PathBuf {
    if let Some(stripped) = location.strip_prefix("file://") {
        PathBuf::from(stripped)
    } else {
        let path = Path::new(location);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            manifest_dir.join(path)
        }
    }
}

fn source_file_name(location: &str) -> Result<&str> {
    let trimmed = location.trim_end_matches('/');
    trimmed
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| anyhow!("source has no file name: {location}"))
}

fn run_script(
    script: Option<&str>,
    build_dir: &Path,
    source_dir: &Path,
    output_dir: &Path,
    label: &str,
) -> Result<()> {
    let Some(script) = script else {
        print_step(&format!("Skipping {label}"));
        print_detail("reason", "not declared");
        return Ok(());
    };

    print_step(&format!("Running {label}"));
    print_detail("cwd", build_dir.display());
    print_detail("env SOURCE_DIR", source_dir.display());
    print_detail("env OUTPUT_DIR", output_dir.display());
    print_detail("command", "bash --noprofile --norc -x -c <script>");
    print_script(script);
    std::io::stdout()
        .flush()
        .context("flush build output before running script")?;

    let status = Command::new("bash")
        .arg("--noprofile")
        .arg("--norc")
        .arg("-x")
        .arg("-c")
        .arg(script)
        .current_dir(build_dir)
        .env("SOURCE_DIR", source_dir)
        .env("OUTPUT_DIR", output_dir)
        .env_remove("BASH_ENV")
        .status()
        .with_context(|| format!("run {label} in {}", build_dir.display()))?;

    if !status.success() {
        bail!(
            "{label} failed with status {status} in {}",
            build_dir.display()
        );
    }
    print_detail("status", status);
    Ok(())
}

fn print_script(script: &str) {
    println!("    script:");
    for line in script.lines() {
        println!("      | {line}");
    }
}

fn build_actions(files: &BTreeMap<String, Vec<String>>, output_dir: &Path) -> Result<Vec<Action>> {
    print_step("Validating file actions");
    if files.is_empty() {
        print_detail("actions", "none declared");
    }

    let mut actions = Vec::new();
    for (target, entries) in files {
        print_detail(
            "target group",
            format!("{target} ({} entries)", entries.len()),
        );
        for entry in entries {
            let action = parse_file_action(target, entry)?;
            let source = output_dir.join(&action.source);
            if !source.exists() {
                bail!(
                    "declared file action source does not exist after install_script: {}",
                    action.source
                );
            }
            print_detail(
                "action",
                format!(
                    "{} -> {} ({:?})",
                    action.source, action.target, action.action_type
                ),
            );
            actions.push(action);
        }
    }
    print_detail("actions", actions.len());
    Ok(actions)
}

fn parse_file_action(target: &str, entry: &str) -> Result<Action> {
    let (source, action_type) = entry
        .rsplit_once(':')
        .ok_or_else(|| anyhow!("file action must use path:link or path:copy format: {entry}"))?;
    validate_relative_path(Path::new(source))?;

    let action_type = match action_type {
        "link" => ActionType::Link,
        "copy" => ActionType::Copy,
        other => bail!("unsupported file action type '{other}' in {entry}"),
    };

    Ok(Action {
        source: source.to_string(),
        target: target.to_string(),
        action_type,
    })
}

fn write_package_archive(
    output_path: &Path,
    payload_dir: &Path,
    manifest: &Manifest,
    compression: CompressionName,
) -> Result<()> {
    let payload_tar = create_payload_tar(payload_dir)?;
    let compressed_payload = compress_payload(payload_tar, compression.package_compression())?;
    let manifest_yaml = serde_yaml::to_string(manifest).context("serialize package manifest")?;

    let file = File::create(output_path)
        .with_context(|| format!("create package {}", output_path.display()))?;
    let mut outer = Builder::new(file);
    append_bytes(&mut outer, "manifest.yml", manifest_yaml.as_bytes())?;
    append_bytes(
        &mut outer,
        compression.data_file_name(),
        &compressed_payload,
    )?;
    outer.finish().context("finish package archive")?;
    Ok(())
}

fn create_payload_tar(payload_dir: &Path) -> Result<Vec<u8>> {
    let mut tar = Builder::new(Vec::new());
    tar.append_dir_all(".", payload_dir)
        .with_context(|| format!("create payload tar from {}", payload_dir.display()))?;
    tar.into_inner().context("finish payload tar")
}

fn compress_payload(payload_tar: Vec<u8>, compression: Compression) -> Result<Vec<u8>> {
    match compression {
        Compression::Zstd => {
            zstd::encode_all(Cursor::new(payload_tar), 0).context("compress payload tar with zstd")
        }
        Compression::Xz => {
            let mut encoder = xz2::write::XzEncoder::new(Vec::new(), 6);
            std::io::copy(&mut Cursor::new(payload_tar), &mut encoder)
                .context("compress payload tar with xz")?;
            encoder.finish().context("finish xz stream")
        }
    }
}

fn append_bytes<W: std::io::Write>(
    builder: &mut Builder<W>,
    path: &str,
    bytes: &[u8],
) -> Result<()> {
    let mut header = Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    builder
        .append_data(&mut header, path, bytes)
        .with_context(|| format!("append {path} to package archive"))?;
    Ok(())
}

fn verify_blake2b(bytes: &[u8], expected_hex: &str) -> Result<()> {
    let actual = hex::encode(Blake2b512::digest(bytes));
    if actual.eq_ignore_ascii_case(expected_hex) {
        Ok(())
    } else {
        bail!("checksum mismatch: expected {expected_hex}, got {actual}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn builds_example_manifest_package() -> Result<()> {
        let temp = TempDir::new()?;
        let package_dir = temp.path().join("example");
        fs::create_dir_all(&package_dir)?;
        fs::write(
            package_dir.join("example.desktop"),
            "[Desktop Entry]\nName=Example\nExec=example\n",
        )?;
        fs::write(
            package_dir.join("example.yml"),
            format!(
                "name: example\nversion: 1.0.0\ndescription: Example\narchitecture:\n  - {}\ncompression: zstd\nsource:\n  - ./example.desktop\nbuild_script: |\n  echo '#!/bin/sh' > example.sh\n  echo 'echo example' >> example.sh\n  chmod +x example.sh\ninstall_script: |\n  mkdir -p $OUTPUT_DIR/bin\n  mv ./example.sh $OUTPUT_DIR/bin/example\n  mv $SOURCE_DIR/example.desktop $OUTPUT_DIR/example.desktop\nfiles:\n  bin:\n    - bin/example:link\n  desktop:\n    - example.desktop:copy\n",
                current_arch()
            ),
        )?;

        let dist = temp.path().join("dist");
        build_package(&BuildArgs {
            manifest: package_dir.to_string_lossy().into_owned(),
            release: 1,
            arch: None,
            build_dir: None,
            output_dir: Some(dist.to_string_lossy().into_owned()),
        })?;

        assert!(
            dist.join("example-1.0.0-1-x86_64.parcel").exists()
                || dist
                    .join(format!("example-1.0.0-1-{}.parcel", current_arch()))
                    .exists()
        );
        Ok(())
    }

    #[test]
    fn custom_build_dir_is_used_for_temporary_workspace() -> Result<()> {
        let temp = TempDir::new()?;
        let build_root = temp.path().join("custom-build-root");

        {
            let workspace = create_build_workspace(Some(&build_root.to_string_lossy()))?;
            assert!(workspace.path().starts_with(&build_root));
            assert!(workspace.path().exists());
        }

        assert!(build_root.exists());
        assert_eq!(fs::read_dir(&build_root)?.count(), 0);
        Ok(())
    }

    #[test]
    fn default_output_dir_is_manifest_directory() -> Result<()> {
        let temp = TempDir::new()?;
        let package_dir = temp.path().join("example");
        fs::create_dir_all(&package_dir)?;
        fs::write(
            package_dir.join("example.desktop"),
            "[Desktop Entry]\nName=Example\nExec=example\n",
        )?;
        fs::write(
            package_dir.join("example.yml"),
            format!(
                "name: example\nversion: 1.0.0\ndescription: Example\narchitecture:\n  - {}\ncompression: zstd\nsource:\n  - ./example.desktop\nbuild_script: |\n  echo '#!/bin/sh' > example.sh\n  echo 'echo example' >> example.sh\n  chmod +x example.sh\ninstall_script: |\n  mkdir -p $OUTPUT_DIR/bin\n  mv ./example.sh $OUTPUT_DIR/bin/example\n  mv $SOURCE_DIR/example.desktop $OUTPUT_DIR/example.desktop\nfiles:\n  bin:\n    - bin/example:link\n",
                current_arch()
            ),
        )?;

        build_package(&BuildArgs {
            manifest: package_dir.to_string_lossy().into_owned(),
            release: 1,
            arch: None,
            build_dir: None,
            output_dir: None,
        })?;

        assert!(
            package_dir
                .join(format!("example-1.0.0-1-{}.parcel", current_arch()))
                .exists()
        );
        Ok(())
    }
}
