//! High-level package lifecycle operations.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use chrono::{SecondsFormat, Utc};
use tempfile::{NamedTempFile, TempDir};

use crate::actions::{apply_actions, cleanup_action_targets, remove_action_target};
use crate::archive::{extract_payload, list_payload_files, read_parcel_archive};
use crate::models::InstalledPackage;
use crate::paths::Paths;
use crate::repositories::{download_candidate, find_latest_candidate};
use crate::storage::{load_database, save_database};
use crate::version::{is_newer, validate_package_arch};

pub fn install_command(paths: &Paths, package: &str) -> Result<()> {
    let package_path = Path::new(package);
    if package_path.exists() {
        return install_local_package(paths, package_path, "local");
    }

    if package.contains('/') || package.ends_with(".parcel") {
        bail!("package file does not exist: {package}");
    }

    install_remote_package(paths, package)
}

/// Install a local package archive and register it in the package database.
pub fn install_local_package(paths: &Paths, package_path: &Path, source_repo: &str) -> Result<()> {
    let mut db = load_database(paths)?;
    let temp = TempDir::new().context("create temporary package workspace")?;
    let archive = read_parcel_archive(package_path, temp.path())?;

    validate_package_arch(&archive.manifest.arch)?;
    if db.packages.contains_key(&archive.manifest.name) {
        bail!(
            "package '{}' is already installed; remove it before installing another version",
            archive.manifest.name
        );
    }

    let install_path = paths
        .apps_dir
        .join(&archive.manifest.name)
        .join(&archive.manifest.version);
    if install_path.exists() {
        bail!("install path already exists: {}", install_path.display());
    }

    fs::create_dir_all(&install_path)
        .with_context(|| format!("create install directory {}", install_path.display()))?;

    let mut applied_actions = Vec::new();
    let install_result = (|| {
        extract_payload(&archive.data_path, archive.compression, &install_path)?;
        let files = list_payload_files(&install_path)?;
        applied_actions = apply_actions(paths, &install_path, &archive.manifest.actions)?;

        db.packages.insert(
            archive.manifest.name.clone(),
            InstalledPackage {
                name: archive.manifest.name.clone(),
                version: archive.manifest.version.clone(),
                arch: archive.manifest.arch.clone(),
                install_path: install_path.clone(),
                source_repo: source_repo.to_string(),
                installed_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
                actions: applied_actions.clone(),
                files,
            },
        );
        save_database(paths, &db)
    })();

    if let Err(err) = install_result {
        cleanup_action_targets(&applied_actions);
        let _ = fs::remove_dir_all(&install_path);
        return Err(err);
    }

    println!(
        "installed {} {}",
        archive.manifest.name, archive.manifest.version
    );
    Ok(())
}

pub fn install_remote_package(paths: &Paths, name: &str) -> Result<()> {
    let candidate = find_latest_candidate(paths, name)?
        .ok_or_else(|| anyhow!("package not found in cached indexes: {name}"))?;
    let bytes = download_candidate(&candidate)?;
    let mut temp = NamedTempFile::new().context("create temporary downloaded package file")?;
    temp.write_all(&bytes)
        .context("write downloaded package to temporary file")?;
    install_local_package(paths, temp.path(), &candidate.remote_name)
}

pub fn remove_package(paths: &Paths, name: &str) -> Result<()> {
    let mut db = load_database(paths)?;
    let package = db
        .packages
        .remove(name)
        .ok_or_else(|| anyhow!("package is not installed: {name}"))?;

    for action in &package.actions {
        remove_action_target(&package, action)?;
    }

    if package.install_path.exists() {
        fs::remove_dir_all(&package.install_path)
            .with_context(|| format!("remove {}", package.install_path.display()))?;
    }

    save_database(paths, &db)?;
    println!("removed {name}");
    Ok(())
}

pub fn list_installed(paths: &Paths) -> Result<()> {
    let db = load_database(paths)?;
    if db.packages.is_empty() {
        println!("no packages installed");
        return Ok(());
    }

    println!("{:<24} {:<18} {:<10} REPOSITORY", "NAME", "VERSION", "ARCH");
    for package in db.packages.values() {
        println!(
            "{:<24} {:<18} {:<10} {}",
            package.name, package.version, package.arch, package.source_repo
        );
    }
    Ok(())
}

pub fn show_package_info(paths: &Paths, name: &str) -> Result<()> {
    let db = load_database(paths)?;
    let installed = db.packages.get(name);
    let candidate = find_latest_candidate(paths, name)?;

    if installed.is_none() && candidate.is_none() {
        bail!("package not found: {name}");
    }

    println!("Name        : {name}");
    if let Some(package) = installed {
        println!("Installed   : {}", package.version);
        println!("Architecture: {}", package.arch);
        println!("Repository  : {}", package.source_repo);
        println!("Install path: {}", package.install_path.display());
        println!("Installed at: {}", package.installed_at);
        println!("Files       : {}", package.files.len());
    } else {
        println!("Installed   : no");
    }

    if let Some(candidate) = candidate {
        println!("Available   : {}", candidate.version);
        println!("Remote      : {}", candidate.remote_name);
        println!("Description : {}", candidate.description);
        if let Some(homepage) = candidate.homepage {
            println!("Homepage    : {homepage}");
        }
    }

    Ok(())
}

pub fn upgrade_packages(paths: &Paths, requested: Option<&str>, yes: bool) -> Result<()> {
    let db = load_database(paths)?;
    if db.packages.is_empty() {
        println!("no packages installed");
        return Ok(());
    }

    let mut plans = Vec::new();
    for package in db.packages.values() {
        if requested.is_some_and(|name| name != package.name) {
            continue;
        }
        let Some(candidate) = find_latest_candidate(paths, &package.name)? else {
            continue;
        };
        if is_newer(&candidate.version, &package.version) {
            plans.push(UpgradePlan {
                installed: package.clone(),
                candidate,
            });
        }
    }

    if let Some(name) = requested
        && !db.packages.contains_key(name)
    {
        bail!("package is not installed: {name}");
    }

    if plans.is_empty() {
        println!("nothing to upgrade");
        return Ok(());
    }

    println!(
        "{:<24} {:<18} {:<18} REPOSITORY",
        "NAME", "INSTALLED", "AVAILABLE"
    );
    for plan in &plans {
        println!(
            "{:<24} {:<18} {:<18} {}",
            plan.installed.name,
            plan.installed.version,
            plan.candidate.version,
            plan.candidate.remote_name
        );
    }

    if !yes && !confirm("Proceed with upgrade? [y/N] ")? {
        println!("aborted");
        return Ok(());
    }

    let mut prepared = Vec::new();
    for plan in plans {
        println!(
            "downloading {} {}",
            plan.candidate.name, plan.candidate.version
        );
        let bytes = download_candidate(&plan.candidate)?;
        let mut temp = NamedTempFile::new().context("create temporary package file")?;
        temp.write_all(&bytes)
            .context("write downloaded package to temporary file")?;
        prepared.push(PreparedUpgrade {
            installed: plan.installed,
            remote_name: plan.candidate.remote_name,
            package_file: temp,
        });
    }

    for prepared in prepared {
        remove_package(paths, &prepared.installed.name)?;
        install_local_package(paths, prepared.package_file.path(), &prepared.remote_name)?;
    }

    Ok(())
}

struct UpgradePlan {
    installed: InstalledPackage,
    candidate: crate::repositories::PackageCandidate,
}

struct PreparedUpgrade {
    installed: InstalledPackage,
    remote_name: String,
    package_file: NamedTempFile,
}

fn confirm(prompt: &str) -> Result<bool> {
    print!("{prompt}");
    io::stdout().flush().context("flush prompt")?;

    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .context("read response")?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes" | "YES" | "Yes"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::version::current_arch;
    use std::io::Cursor;

    #[test]
    fn installs_and_removes_local_package_archive() -> Result<()> {
        let temp = TempDir::new()?;
        let home = temp.path().join("home");
        let paths = Paths::from_home(home.clone());

        let archive_path = temp.path().join("example-1.0.0-1-x86_64.parcel");
        write_test_package(&archive_path, "1.0.0-1")?;

        install_local_package(&paths, &archive_path, "local")?;

        let bin_link = home.join(".local/bin/example");
        let desktop_file = home.join(".local/share/applications/example.desktop");
        assert!(fs::symlink_metadata(&bin_link)?.file_type().is_symlink());
        assert_eq!(
            fs::read_link(&bin_link)?,
            paths.apps_dir.join("example/1.0.0-1/bin/example")
        );
        assert_eq!(
            fs::read_to_string(&desktop_file)?,
            "[Desktop Entry]\nName=Example\nExec=example\n"
        );

        let db = load_database(&paths)?;
        assert!(db.packages.contains_key("example"));

        remove_package(&paths, "example")?;

        assert!(fs::symlink_metadata(&bin_link).is_err());
        assert!(fs::symlink_metadata(&desktop_file).is_err());
        assert!(!paths.apps_dir.join("example/1.0.0-1").exists());
        Ok(())
    }

    fn write_test_package(path: &Path, version: &str) -> Result<()> {
        let mut payload = tar::Builder::new(Vec::new());
        append_tar_bytes(&mut payload, "bin/example", b"#!/bin/sh\necho example\n")?;
        append_tar_bytes(
            &mut payload,
            "share/applications/example.desktop",
            b"[Desktop Entry]\nName=Example\nExec=example\n",
        )?;
        let payload_tar = payload.into_inner()?;
        let compressed_payload = zstd::encode_all(Cursor::new(payload_tar), 0)?;

        let manifest = format!(
            "name: example\nversion: {version}\narch: {}\ndescription: Example package\nhomepage: https://example.com\nactions:\n  - source: bin/example\n    target: bin\n    type: link\n  - source: share/applications/example.desktop\n    target: applications\n    type: copy\n",
            current_arch()
        );

        let mut outer = tar::Builder::new(Vec::new());
        append_tar_bytes(&mut outer, "manifest.yml", manifest.as_bytes())?;
        append_tar_bytes(&mut outer, "data.tar.zst", &compressed_payload)?;
        fs::write(path, outer.into_inner()?)?;
        Ok(())
    }

    fn append_tar_bytes<W: Write>(
        builder: &mut tar::Builder<W>,
        path: &str,
        bytes: &[u8],
    ) -> Result<()> {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, path, bytes)?;
        Ok(())
    }
}
