use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};

use crate::{
    artifact::{copy_tree, extract_package, normalize_archive_path},
    layout::Layout,
    parcel_manifest::{InstallAction, InstallActionKind, InstallTarget, PackageManifest},
    receipt::{ExposedPath, ExposureKind, InstallSource, Receipt, load_receipt},
    utils::hash::hash_file_blake2b,
};

pub fn install_archive(
    layout: &Layout,
    archive_path: &Path,
    source: InstallSource,
    expected_checksum: Option<&str>,
) -> Result<Receipt> {
    if let Some(expected) = expected_checksum {
        crate::utils::hash::verify_file_checksum(
            archive_path
                .to_str()
                .context("archive path must be valid UTF-8 for checksum verification")?,
            expected,
        )?;
    }

    let staged_dir = layout.temp_path("staged-install", "");
    fs::create_dir_all(&staged_dir)?;
    let manifest = extract_package(archive_path, &staged_dir)?;
    validate_actions(&manifest, &staged_dir)?;
    let checksum = hash_file_blake2b(archive_path)?;
    activate_staged_install(layout, &manifest, &staged_dir, source, Some(checksum))
}

pub fn activate_staged_install(
    layout: &Layout,
    manifest: &PackageManifest,
    staged_dir: &Path,
    source: InstallSource,
    package_checksum: Option<String>,
) -> Result<Receipt> {
    layout.ensure_all()?;
    let final_dir = layout.version_install_dir(&manifest.name, &manifest.version);
    if final_dir.exists() {
        fs::remove_dir_all(&final_dir)
            .with_context(|| format!("remove {}", final_dir.display()))?;
    }
    if let Some(parent) = final_dir.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    if fs::rename(staged_dir, &final_dir).is_err() {
        copy_tree(staged_dir, &final_dir)?;
        fs::remove_dir_all(staged_dir).ok();
    }

    let previous_receipt = load_receipt(layout, &manifest.name)?;
    if let Some(previous) = &previous_receipt {
        ensure_exposures_owned(layout, previous, manifest, &final_dir)?;
    } else {
        ensure_exposures_free(layout, manifest, &final_dir)?;
    }

    if let Some(previous) = &previous_receipt {
        remove_exposed_paths(previous)?;
    }

    if let Err(err) = switch_opt_link(layout, &manifest.name, &final_dir)
        .and_then(|_| expose_manifest(layout, manifest, &final_dir))
        .and_then(|exposed| {
            let receipt = Receipt {
                name: manifest.name.clone(),
                version: manifest.version.clone(),
                arch: manifest.arch.to_string(),
                source: source.clone(),
                package_checksum: package_checksum.clone(),
                installed_at: chrono::Utc::now(),
                install_dir: final_dir.clone(),
                opt_link: layout.opt_link_path(&manifest.name),
                exposed_paths: exposed,
            };
            receipt.save(layout)?;
            Ok(receipt)
        })
    {
        if let Some(previous) = previous_receipt {
            restore_previous_install(layout, &previous)?;
        }
        return Err(err);
    }

    load_receipt(layout, &manifest.name)?.context("receipt missing after installation")
}

pub fn remove_installed_package(layout: &Layout, name: &str) -> Result<()> {
    layout.ensure_all()?;
    let Some(receipt) = load_receipt(layout, name)? else {
        bail!("package {name} is not installed");
    };

    remove_exposed_paths(&receipt)?;
    let opt_link = layout.opt_link_path(name);
    if opt_link.exists() || fs::symlink_metadata(&opt_link).is_ok() {
        fs::remove_file(&opt_link).with_context(|| format!("remove {}", opt_link.display()))?;
    }
    let receipt_path = layout.receipt_path(name);
    if receipt_path.exists() {
        fs::remove_file(&receipt_path)
            .with_context(|| format!("remove {}", receipt_path.display()))?;
    }
    let package_dir = layout.cellar_package_dir(name);
    if package_dir.exists() {
        fs::remove_dir_all(&package_dir)
            .with_context(|| format!("remove {}", package_dir.display()))?;
    }
    Ok(())
}

pub fn expose_manifest(
    layout: &Layout,
    manifest: &PackageManifest,
    install_dir: &Path,
) -> Result<Vec<ExposedPath>> {
    let mut exposed = Vec::new();
    for action in &manifest.actions {
        let source_relative = normalize_archive_path(Path::new(&action.source))?;
        let source_path = install_dir.join(&source_relative);
        let target_path = exposed_destination(layout, action, &source_relative)?;

        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }

        match action.kind {
            InstallActionKind::Link => {
                #[cfg(unix)]
                std::os::unix::fs::symlink(&source_path, &target_path)
                    .with_context(|| format!("symlink {}", target_path.display()))?;
                exposed.push(ExposedPath {
                    target: target_path,
                    kind: ExposureKind::Link,
                    source: source_path,
                });
            }
            InstallActionKind::Copy => {
                fs::copy(&source_path, &target_path).with_context(|| {
                    format!(
                        "copy {} to {}",
                        source_path.display(),
                        target_path.display()
                    )
                })?;
                exposed.push(ExposedPath {
                    target: target_path,
                    kind: ExposureKind::Copy,
                    source: source_path,
                });
            }
        }
    }
    Ok(exposed)
}

pub fn remove_exposed_paths(receipt: &Receipt) -> Result<()> {
    for exposed in &receipt.exposed_paths {
        if !exposed.target.exists() && fs::symlink_metadata(&exposed.target).is_err() {
            continue;
        }

        match exposed.kind {
            ExposureKind::Link => {
                let current = fs::read_link(&exposed.target)
                    .with_context(|| format!("readlink {}", exposed.target.display()))?;
                if current == exposed.source {
                    fs::remove_file(&exposed.target)
                        .with_context(|| format!("remove {}", exposed.target.display()))?;
                }
            }
            ExposureKind::Copy => {
                if exposed.source.exists()
                    && hash_file_blake2b(&exposed.source)? == hash_file_blake2b(&exposed.target)?
                {
                    fs::remove_file(&exposed.target)
                        .with_context(|| format!("remove {}", exposed.target.display()))?;
                }
            }
        }
    }
    Ok(())
}

fn ensure_exposures_free(
    layout: &Layout,
    manifest: &PackageManifest,
    install_dir: &Path,
) -> Result<()> {
    for action in &manifest.actions {
        let source_relative = normalize_archive_path(Path::new(&action.source))?;
        let target_path = exposed_destination(layout, action, &source_relative)?;
        if target_path.exists() || fs::symlink_metadata(&target_path).is_ok() {
            bail!(
                "refusing to overwrite existing destination {} while installing {}",
                target_path.display(),
                install_dir.display()
            );
        }
    }
    Ok(())
}

fn ensure_exposures_owned(
    layout: &Layout,
    previous: &Receipt,
    manifest: &PackageManifest,
    install_dir: &Path,
) -> Result<()> {
    for action in &manifest.actions {
        let source_relative = normalize_archive_path(Path::new(&action.source))?;
        let target_path = exposed_destination(layout, action, &source_relative)?;
        if let Some(existing) = previous
            .exposed_paths
            .iter()
            .find(|item| item.target == target_path)
        {
            match existing.kind {
                ExposureKind::Link => {
                    let current = fs::read_link(&existing.target)?;
                    if current != existing.source {
                        bail!(
                            "destination {} is no longer owned by Parcel",
                            existing.target.display()
                        );
                    }
                }
                ExposureKind::Copy => {
                    if existing.target.exists()
                        && existing.source.exists()
                        && hash_file_blake2b(&existing.target)?
                            != hash_file_blake2b(&existing.source)?
                    {
                        bail!(
                            "destination {} has been modified and cannot be replaced safely",
                            existing.target.display()
                        );
                    }
                }
            }
        } else if target_path.exists() || fs::symlink_metadata(&target_path).is_ok() {
            bail!(
                "refusing to overwrite existing destination {} while installing {}",
                target_path.display(),
                install_dir.display()
            );
        }
    }
    Ok(())
}

fn restore_previous_install(layout: &Layout, previous: &Receipt) -> Result<()> {
    switch_opt_link(layout, &previous.name, &previous.install_dir)?;
    for exposed in &previous.exposed_paths {
        if let Some(parent) = exposed.target.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        match exposed.kind {
            ExposureKind::Link => {
                #[cfg(unix)]
                std::os::unix::fs::symlink(&exposed.source, &exposed.target)
                    .with_context(|| format!("symlink {}", exposed.target.display()))?;
            }
            ExposureKind::Copy => {
                fs::copy(&exposed.source, &exposed.target).with_context(|| {
                    format!(
                        "copy {} to {}",
                        exposed.source.display(),
                        exposed.target.display()
                    )
                })?;
            }
        }
    }
    previous.save(layout)
}

fn switch_opt_link(layout: &Layout, name: &str, install_dir: &Path) -> Result<()> {
    let opt_path = layout.opt_link_path(name);
    let temp_link = layout.opt_dir().join(format!("{name}.tmp"));
    if temp_link.exists() || fs::symlink_metadata(&temp_link).is_ok() {
        fs::remove_file(&temp_link).ok();
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(install_dir, &temp_link)
        .with_context(|| format!("symlink {}", temp_link.display()))?;
    fs::rename(&temp_link, &opt_path).with_context(|| {
        format!(
            "replace active symlink {} -> {}",
            opt_path.display(),
            install_dir.display()
        )
    })?;
    Ok(())
}

fn validate_actions(manifest: &PackageManifest, install_dir: &Path) -> Result<()> {
    for action in &manifest.actions {
        let relative = normalize_archive_path(Path::new(&action.source))?;
        let source_path = install_dir.join(relative);
        if !source_path.exists() {
            bail!(
                "package action source {} is missing from payload",
                source_path.display()
            );
        }
    }
    Ok(())
}

fn exposed_destination(
    layout: &Layout,
    action: &InstallAction,
    source_relative: &Path,
) -> Result<PathBuf> {
    let target = action.target.clone().canonical();
    let base = layout.target_dir(target.clone().as_str());
    let destination = match target {
        InstallTarget::Bin | InstallTarget::Applications => base.join(
            source_relative
                .file_name()
                .context("action source must have a basename")?,
        ),
        InstallTarget::Icons => {
            if let Ok(stripped) = source_relative.strip_prefix("share/icons") {
                base.join(stripped)
            } else {
                base.join(
                    source_relative
                        .file_name()
                        .context("icon source must have a basename")?,
                )
            }
        }
        InstallTarget::Man => {
            if let Ok(stripped) = source_relative.strip_prefix("share/man") {
                base.join(stripped)
            } else {
                base.join(
                    source_relative
                        .file_name()
                        .context("man source must have a basename")?,
                )
            }
        }
        InstallTarget::Desktop => unreachable!(),
    };
    Ok(destination)
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use anyhow::Result;

    use crate::{
        artifact::write_package,
        layout::Layout,
        parcel_manifest::{InstallAction, InstallActionKind, InstallTarget, PackageManifest},
        receipt::InstallSource,
        utils::arch::Architecture,
    };

    use super::{install_archive, remove_installed_package};

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
    fn installs_and_removes_local_package() -> Result<()> {
        let root = unique_dir("install");
        let layout = Layout {
            data_home: root.join("data"),
            state_home: root.join("state"),
            cache_home: root.join("cache"),
            config_home: root.join("config"),
        };
        layout.ensure_all()?;

        let payload_dir = root.join("payload");
        fs::create_dir_all(payload_dir.join("bin"))?;
        fs::write(payload_dir.join("bin/example"), b"hello")?;

        let manifest = PackageManifest {
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
        let archive = root.join("example-1.0.0-1-x86_64.parcel");
        write_package(&manifest, &payload_dir, &archive)?;

        let receipt = install_archive(
            &layout,
            &archive,
            InstallSource::LocalFile {
                path: archive.clone(),
            },
            None,
        )?;
        assert_eq!(receipt.version, "1.0.0-1");
        assert!(layout.opt_link_path("example").exists());

        remove_installed_package(&layout, "example")?;
        assert!(!layout.receipt_path("example").exists());
        Ok(())
    }
}
