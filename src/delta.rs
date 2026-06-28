use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tar::{Archive, Builder};

use crate::{
    artifact::{
        append_bytes_tar, append_file_tar, copy_tree, directory_checksum, extract_package,
        extract_payload_tar_zst, normalize_archive_path, read_package_manifest,
    },
    cli::BuildDeltaArgs,
    layout::Layout,
    parcel_manifest::PackageManifest,
    utils::{arch::Architecture, hash::hash_file_blake2b},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeltaManifest {
    pub name: String,
    pub from_version: String,
    pub to_version: String,
    pub arch: Architecture,
    pub base_checksum: String,
    pub target_checksum: String,
    #[serde(default)]
    pub removed_paths: Vec<String>,
}

#[derive(Debug)]
pub struct DeltaFile {
    pub delta: DeltaManifest,
    pub manifest: PackageManifest,
    pub overlay_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum EntryFingerprint {
    Directory,
    File(String),
    Symlink(String),
}

pub fn build_delta_command(args: &BuildDeltaArgs) -> Result<()> {
    let from = Path::new(&args.from);
    let to = Path::new(&args.to);
    let to_manifest = read_package_manifest(to)?;

    let output_dir = args
        .output_dir
        .as_ref()
        .map(PathBuf::from)
        .or_else(|| to.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));
    fs::create_dir_all(&output_dir).with_context(|| format!("create {}", output_dir.display()))?;

    let output_path = output_dir.join(format!(
        "{}-{}-{}-{}.delta.parcel",
        to_manifest.name,
        read_package_manifest(from)?.version,
        to_manifest.version,
        to_manifest.arch
    ));
    build_delta_archive(from, to, &output_path)?;
    println!("{}", output_path.display());
    Ok(())
}

pub fn build_delta_archive(from_path: &Path, to_path: &Path, output_path: &Path) -> Result<()> {
    let layout = Layout::detect()?;
    let from_dir = layout.temp_path("delta-from", "");
    let to_dir = layout.temp_path("delta-to", "");
    let overlay_dir = layout.temp_path("delta-overlay", "");
    fs::create_dir_all(&from_dir)?;
    fs::create_dir_all(&to_dir)?;
    fs::create_dir_all(&overlay_dir)?;

    let from_manifest = extract_package(from_path, &from_dir)?;
    let to_manifest = extract_package(to_path, &to_dir)?;
    if from_manifest.name != to_manifest.name {
        bail!("delta requires matching package names");
    }
    if from_manifest.arch != to_manifest.arch {
        bail!("delta requires matching architectures");
    }

    let from_entries = collect_entries(&from_dir)?;
    let to_entries = collect_entries(&to_dir)?;

    let mut removed_paths = Vec::new();
    for path in from_entries.keys() {
        if !to_entries.contains_key(path) {
            removed_paths.push(path.to_string_lossy().to_string());
        }
    }

    for (path, fingerprint) in &to_entries {
        let changed = from_entries.get(path) != Some(fingerprint);
        if changed {
            copy_entry(&to_dir.join(path), &overlay_dir.join(path), fingerprint)?;
        }
    }

    let delta_manifest = DeltaManifest {
        name: to_manifest.name.clone(),
        from_version: from_manifest.version.clone(),
        to_version: to_manifest.version.clone(),
        arch: to_manifest.arch,
        base_checksum: hash_file_blake2b(from_path)?,
        target_checksum: hash_file_blake2b(to_path)?,
        removed_paths,
    };

    write_delta_archive(&delta_manifest, &to_manifest, &overlay_dir, output_path)?;

    let _ = fs::remove_dir_all(from_dir);
    let _ = fs::remove_dir_all(to_dir);
    let _ = fs::remove_dir_all(overlay_dir);
    Ok(())
}

pub fn write_delta_archive(
    delta_manifest: &DeltaManifest,
    target_manifest: &PackageManifest,
    overlay_dir: &Path,
    output_path: &Path,
) -> Result<()> {
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let layout = Layout::detect()?;
    let overlay_tar_path = layout.temp_path("delta-data", ".tar.zst");
    crate::artifact::compress_directory_to_tar_zst(overlay_dir, &overlay_tar_path)?;

    let file =
        File::create(output_path).with_context(|| format!("create {}", output_path.display()))?;
    let mut outer = Builder::new(file);
    append_bytes_tar(
        &mut outer,
        "delta.yml",
        serde_yaml::to_string(delta_manifest)?.as_bytes(),
    )?;
    append_bytes_tar(
        &mut outer,
        "manifest.yml",
        serde_yaml::to_string(target_manifest)?.as_bytes(),
    )?;
    append_file_tar(&mut outer, "delta-data.tar.zst", &overlay_tar_path)?;
    outer.finish()?;
    let _ = fs::remove_file(overlay_tar_path);
    Ok(())
}

pub fn read_delta_metadata(delta_path: &Path) -> Result<(DeltaManifest, PackageManifest)> {
    let delta = stage_delta(delta_path)?;
    let _ = fs::remove_file(delta.overlay_path);
    Ok((delta.delta, delta.manifest))
}

pub fn stage_delta(delta_path: &Path) -> Result<DeltaFile> {
    let layout = Layout::detect()?;
    let staged_overlay = layout.temp_path("overlay", ".tar.zst");
    let file = File::open(delta_path).with_context(|| format!("open {}", delta_path.display()))?;
    let mut archive = Archive::new(file);

    let mut delta_manifest = None;
    let mut package_manifest = None;
    let mut overlay_written = false;

    for entry in archive.entries().context("iterate delta entries")? {
        let mut entry = entry?;
        let path = entry.path()?.to_path_buf();
        match path.to_string_lossy().as_ref() {
            "delta.yml" => {
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes)?;
                delta_manifest = Some(serde_yaml::from_slice::<DeltaManifest>(&bytes)?);
            }
            "manifest.yml" => {
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes)?;
                package_manifest = Some(serde_yaml::from_slice::<PackageManifest>(&bytes)?);
            }
            "delta-data.tar.zst" => {
                let mut out = File::create(&staged_overlay)
                    .with_context(|| format!("create {}", staged_overlay.display()))?;
                std::io::copy(&mut entry, &mut out)?;
                overlay_written = true;
            }
            _ => {}
        }
    }

    let delta = delta_manifest.context("delta.yml missing from delta archive")?;
    let manifest = package_manifest.context("manifest.yml missing from delta archive")?;
    if !overlay_written {
        bail!("delta-data.tar.zst missing from delta archive");
    }

    Ok(DeltaFile {
        delta,
        manifest,
        overlay_path: staged_overlay,
    })
}

pub fn apply_delta_archive(
    delta_path: &Path,
    base_dir: &Path,
    output_dir: &Path,
) -> Result<(DeltaManifest, PackageManifest, String)> {
    let staged = stage_delta(delta_path)?;
    copy_tree(base_dir, output_dir)?;

    for removed in &staged.delta.removed_paths {
        let target = output_dir.join(normalize_archive_path(Path::new(removed))?);
        if target.exists() || fs::symlink_metadata(&target).is_ok() {
            if fs::symlink_metadata(&target)?.is_dir() {
                fs::remove_dir_all(&target)
                    .with_context(|| format!("remove {}", target.display()))?;
            } else {
                fs::remove_file(&target).with_context(|| format!("remove {}", target.display()))?;
            }
        }
    }

    let overlay_dir = Layout::detect()?.temp_path("delta-apply", "");
    fs::create_dir_all(&overlay_dir)?;
    extract_payload_tar_zst(&staged.overlay_path, &overlay_dir)?;

    let overlay_entries = collect_entries(&overlay_dir)?;
    for (path, fingerprint) in overlay_entries {
        copy_entry(
            &overlay_dir.join(&path),
            &output_dir.join(&path),
            &fingerprint,
        )?;
    }

    let checksum = directory_checksum(output_dir)?;
    let _ = fs::remove_file(staged.overlay_path);
    let _ = fs::remove_dir_all(overlay_dir);
    Ok((staged.delta, staged.manifest, checksum))
}

fn collect_entries(root: &Path) -> Result<BTreeMap<PathBuf, EntryFingerprint>> {
    let mut entries = BTreeMap::new();
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        let entry = entry?;
        let path = entry.path();
        if path == root {
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .with_context(|| format!("strip prefix {}", root.display()))?
            .to_path_buf();
        let metadata = fs::symlink_metadata(path)?;
        let fingerprint = if metadata.is_dir() {
            EntryFingerprint::Directory
        } else if metadata.file_type().is_symlink() {
            EntryFingerprint::Symlink(fs::read_link(path)?.to_string_lossy().to_string())
        } else {
            EntryFingerprint::File(crate::utils::hash::hash_file_blake2b(path)?)
        };
        entries.insert(relative, fingerprint);
    }
    Ok(entries)
}

fn copy_entry(source: &Path, target: &Path, fingerprint: &EntryFingerprint) -> Result<()> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }

    if target.exists() || fs::symlink_metadata(target).is_ok() {
        if fs::symlink_metadata(target)?.is_dir() {
            fs::remove_dir_all(target).with_context(|| format!("remove {}", target.display()))?;
        } else {
            fs::remove_file(target).with_context(|| format!("remove {}", target.display()))?;
        }
    }

    match fingerprint {
        EntryFingerprint::Directory => {
            fs::create_dir_all(target).with_context(|| format!("create {}", target.display()))?;
        }
        EntryFingerprint::File(_) => {
            fs::copy(source, target)
                .with_context(|| format!("copy {} to {}", source.display(), target.display()))?;
        }
        EntryFingerprint::Symlink(link_target) => {
            #[cfg(unix)]
            std::os::unix::fs::symlink(link_target, target)
                .with_context(|| format!("symlink {}", target.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use anyhow::Result;

    use crate::{
        artifact::{extract_package, package_file_name, write_package},
        parcel_manifest::{InstallAction, InstallActionKind, InstallTarget, PackageManifest},
        utils::arch::Architecture,
    };

    use super::{apply_delta_archive, build_delta_archive, read_delta_metadata};

    fn unique_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "parcel-test-{name}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn build_and_apply_delta_overlay() -> Result<()> {
        let root = unique_dir("delta");
        let from_payload = root.join("from-payload");
        let to_payload = root.join("to-payload");
        fs::create_dir_all(from_payload.join("bin"))?;
        fs::create_dir_all(to_payload.join("bin"))?;
        fs::write(from_payload.join("bin/example"), b"one")?;
        fs::write(to_payload.join("bin/example"), b"two")?;
        fs::write(to_payload.join("bin/extra"), b"new")?;

        let manifest_v1 = PackageManifest {
            name: "example".into(),
            version: "1.0.0-1".into(),
            arch: Architecture::X86_64,
            description: "Example".into(),
            homepage: None,
            actions: vec![InstallAction {
                source: "bin/example".into(),
                target: InstallTarget::Bin,
                kind: InstallActionKind::Link,
            }],
        };
        let manifest_v2 = PackageManifest {
            version: "1.1.0-1".into(),
            ..manifest_v1.clone()
        };

        let from_parcel = root.join(package_file_name(
            "example",
            "1.0.0-1",
            Architecture::X86_64,
        ));
        let to_parcel = root.join(package_file_name(
            "example",
            "1.1.0-1",
            Architecture::X86_64,
        ));
        write_package(&manifest_v1, &from_payload, &from_parcel)?;
        write_package(&manifest_v2, &to_payload, &to_parcel)?;

        let delta_path = root.join("example-1.0.0-1-1.1.0-1-x86_64.delta.parcel");
        build_delta_archive(&from_parcel, &to_parcel, &delta_path)?;

        let (delta, _) = read_delta_metadata(&delta_path)?;
        assert_eq!(delta.from_version, "1.0.0-1");
        assert_eq!(delta.to_version, "1.1.0-1");

        let base_dir = root.join("base");
        extract_package(&from_parcel, &base_dir)?;
        let out_dir = root.join("out");
        let (_, manifest, _) = apply_delta_archive(&delta_path, &base_dir, &out_dir)?;
        assert_eq!(manifest.version, "1.1.0-1");
        assert_eq!(fs::read(out_dir.join("bin/example"))?, b"two");
        assert_eq!(fs::read(out_dir.join("bin/extra"))?, b"new");
        Ok(())
    }
}
