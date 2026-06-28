use std::{fs, path::Path};

use anyhow::{Context, Result, bail};

use crate::{
    cli::{InstallArgs, RemoveArgs, UpgradeArgs},
    delta::apply_delta_archive,
    installer::{activate_staged_install, install_archive, remove_installed_package},
    layout::{Layout, path_file_name},
    receipt::{InstallSource, installed_receipts, load_receipt},
    repo::{fetch_to_path, load_cached_repos},
    resolver::{
        ResolvedPackageArtifact, UpgradePlan, UpgradeStep, resolve_install_target,
        resolve_upgrade_plan,
    },
};

pub fn install_package(args: &InstallArgs) -> Result<()> {
    let layout = Layout::detect()?;
    layout.ensure_all()?;

    let package_path = Path::new(&args.package);
    if package_path.exists() {
        let receipt = install_archive(
            &layout,
            package_path,
            InstallSource::LocalFile {
                path: package_path.to_path_buf(),
            },
            None,
        )?;
        println!("Installed {} {}", receipt.name, receipt.version);
        return Ok(());
    }

    let artifact = resolve_install_target(&args.package, args.version.as_deref())?
        .context("package not found in cached repositories")?;
    let receipt = install_resolved_artifact(&layout, &artifact)?;
    println!("Installed {} {}", receipt.name, receipt.version);
    Ok(())
}

pub fn remove_package(args: &RemoveArgs) -> Result<()> {
    let layout = Layout::detect()?;
    remove_installed_package(&layout, &args.package)?;
    println!("Removed {}", args.package);
    Ok(())
}

pub fn upgrade_packages(args: &UpgradeArgs) -> Result<()> {
    let layout = Layout::detect()?;
    layout.ensure_all()?;
    let targets = if let Some(name) = &args.package {
        vec![
            load_receipt(&layout, name)?
                .with_context(|| format!("package {name} is not installed"))?,
        ]
    } else {
        installed_receipts(&layout)?
    };

    for receipt in targets {
        let Some(plan) = resolve_upgrade_plan(&receipt.name, &receipt.version)? else {
            println!("{} is already up to date", receipt.name);
            continue;
        };
        execute_upgrade_plan(&layout, &receipt, plan)?;
    }

    Ok(())
}

fn install_resolved_artifact(
    layout: &Layout,
    artifact: &ResolvedPackageArtifact,
) -> Result<crate::receipt::Receipt> {
    let file_name = path_file_name(Path::new(&artifact.url));
    let download_path = layout.download_path(&file_name);
    fetch_to_path(&artifact.url, &download_path)?;
    install_archive(
        layout,
        &download_path,
        InstallSource::Repository {
            repo: artifact.repo_name.clone(),
            url: artifact.url.clone(),
        },
        Some(&artifact.checksum),
    )
}

fn execute_upgrade_plan(
    layout: &Layout,
    receipt: &crate::receipt::Receipt,
    plan: UpgradePlan,
) -> Result<()> {
    match plan.steps.as_slice() {
        [UpgradeStep::Full(artifact)] => {
            install_resolved_artifact(layout, artifact)?;
            println!("Upgraded {} to {}", receipt.name, artifact.version);
            Ok(())
        }
        steps => {
            let working_root = layout.temp_path("upgrade", "");
            fs::create_dir_all(&working_root)?;
            let mut current_dir = receipt.install_dir.clone();
            let mut temp_dirs = Vec::new();
            let mut final_manifest = None;
            let mut final_checksum = receipt.package_checksum.clone();
            let mut final_repo = None;

            for (index, step) in steps.iter().enumerate() {
                let UpgradeStep::Delta(delta_artifact) = step else {
                    bail!("mixed delta/full upgrade plans are not supported");
                };
                let download_path =
                    layout.download_path(&path_file_name(Path::new(&delta_artifact.url)));
                fetch_to_path(&delta_artifact.url, &download_path)?;
                crate::utils::hash::verify_file_checksum(
                    download_path
                        .to_str()
                        .context("delta download path must be valid UTF-8")?,
                    &delta_artifact.checksum,
                )?;
                let next_dir = working_root.join(format!("{index}-{}", delta_artifact.to_version));
                let (delta_manifest, manifest, _) =
                    apply_delta_archive(&download_path, &current_dir, &next_dir)?;
                if delta_manifest.from_version != delta_artifact.from_version {
                    bail!(
                        "delta archive metadata mismatch: expected {}, found {}",
                        delta_artifact.from_version,
                        delta_manifest.from_version
                    );
                }
                if delta_manifest.from_version != receipt.version && index == 0 {
                    bail!(
                        "delta upgrade expected base version {}, found {}",
                        receipt.version,
                        delta_manifest.from_version
                    );
                }
                if let Some(expected) = &receipt.package_checksum {
                    if index == 0 && delta_manifest.base_checksum != *expected {
                        bail!("delta base checksum does not match installed package");
                    }
                }
                current_dir = next_dir.clone();
                temp_dirs.push(next_dir);
                final_manifest = Some(manifest);
                final_checksum = Some(delta_manifest.target_checksum);
                final_repo = Some((delta_artifact.repo_name.clone(), delta_artifact.url.clone()));
            }

            let manifest = final_manifest.context("delta upgrade did not produce a manifest")?;
            let (repo_name, url) = final_repo.context("delta upgrade missing repository origin")?;
            activate_staged_install(
                layout,
                &manifest,
                &current_dir,
                InstallSource::Repository {
                    repo: repo_name,
                    url,
                },
                final_checksum,
            )?;
            for dir in temp_dirs {
                if dir.exists() {
                    let _ = fs::remove_dir_all(dir);
                }
            }
            println!("Upgraded {} to {}", receipt.name, plan.target_version);
            Ok(())
        }
    }
}

#[allow(dead_code)]
fn _ensure_indexes_exist() -> Result<()> {
    if load_cached_repos()?.is_empty() {
        bail!("no cached repositories available; run `parcel update` first");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        sync::{Mutex, OnceLock},
    };

    use anyhow::{Context, Result};

    use crate::{
        artifact::write_package,
        cli::{
            InfoArgs, InstallArgs, RemoveArgs, RepoAddArgs, RepoIndexArgs, SearchArgs, UpgradeArgs,
        },
        delta::build_delta_archive,
        layout::Layout,
        parcel_manifest::{InstallAction, InstallActionKind, InstallTarget, PackageManifest},
        receipt::{InstallSource, list_installed_packages},
        repo::{add_repo, build_repo_index, search_packages, show_package_info, update_indexes},
        utils::arch::Architecture,
    };

    use super::{
        install_archive, install_package, remove_installed_package, remove_package,
        upgrade_packages,
    };

    fn unique_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "parcel-test-{name}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn with_test_env<T>(root: &PathBuf, action: impl FnOnce() -> Result<T>) -> Result<T> {
        let _guard = env_lock().lock().unwrap();
        let home = root.join("home");
        let data = root.join("xdg-data");
        let state = root.join("xdg-state");
        let cache = root.join("xdg-cache");
        let config = root.join("xdg-config");
        fs::create_dir_all(&home)?;

        let previous_home = std::env::var_os("HOME");
        let previous_data = std::env::var_os("XDG_DATA_HOME");
        let previous_state = std::env::var_os("XDG_STATE_HOME");
        let previous_cache = std::env::var_os("XDG_CACHE_HOME");
        let previous_config = std::env::var_os("XDG_CONFIG_HOME");

        unsafe {
            std::env::set_var("HOME", &home);
            std::env::set_var("XDG_DATA_HOME", &data);
            std::env::set_var("XDG_STATE_HOME", &state);
            std::env::set_var("XDG_CACHE_HOME", &cache);
            std::env::set_var("XDG_CONFIG_HOME", &config);
        }

        let result = action();
        restore_env("HOME", previous_home);
        restore_env("XDG_DATA_HOME", previous_data);
        restore_env("XDG_STATE_HOME", previous_state);
        restore_env("XDG_CACHE_HOME", previous_cache);
        restore_env("XDG_CONFIG_HOME", previous_config);
        result
    }

    fn restore_env(key: &str, value: Option<std::ffi::OsString>) {
        unsafe {
            if let Some(value) = value {
                std::env::set_var(key, value);
            } else {
                std::env::remove_var(key);
            }
        }
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

    #[test]
    fn repo_install_search_info_upgrade_and_remove() -> Result<()> {
        let root = unique_dir("repo-flow");
        with_test_env(&root, || {
            let repo_dir = root.join("repo");
            fs::create_dir_all(&repo_dir)?;

            let payload_v1 = root.join("payload-v1");
            let payload_v2 = root.join("payload-v2");
            let payload_v3 = root.join("payload-v3");
            for payload in [&payload_v1, &payload_v2, &payload_v3] {
                fs::create_dir_all(payload.join("bin"))?;
            }
            fs::write(payload_v1.join("bin/example"), b"v1")?;
            fs::write(payload_v2.join("bin/example"), b"v2")?;
            fs::write(payload_v3.join("bin/example"), b"v3")?;
            fs::write(payload_v3.join("bin/helper"), b"helper")?;

            let manifest_v1 = PackageManifest {
                name: "example".into(),
                version: "1.0.0-1".into(),
                arch: Architecture::X86_64,
                description: "Example package".into(),
                homepage: Some("https://example.com".into()),
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
            let manifest_v3 = PackageManifest {
                version: "1.2.0-1".into(),
                actions: vec![
                    InstallAction {
                        source: "bin/example".into(),
                        target: InstallTarget::Bin,
                        kind: InstallActionKind::Link,
                    },
                    InstallAction {
                        source: "bin/helper".into(),
                        target: InstallTarget::Bin,
                        kind: InstallActionKind::Link,
                    },
                ],
                ..manifest_v1.clone()
            };

            let parcel_v1 = repo_dir.join("example-1.0.0-1-x86_64.parcel");
            let parcel_v2 = repo_dir.join("example-1.1.0-1-x86_64.parcel");
            let parcel_v3 = repo_dir.join("example-1.2.0-1-x86_64.parcel");
            write_package(&manifest_v1, &payload_v1, &parcel_v1)?;
            write_package(&manifest_v2, &payload_v2, &parcel_v2)?;
            write_package(&manifest_v3, &payload_v3, &parcel_v3)?;

            build_delta_archive(
                &parcel_v1,
                &parcel_v2,
                &repo_dir.join("example-1.0.0-1-1.1.0-1-x86_64.delta.parcel"),
            )?;
            build_delta_archive(
                &parcel_v2,
                &parcel_v3,
                &repo_dir.join("example-1.1.0-1-1.2.0-1-x86_64.delta.parcel"),
            )?;

            build_repo_index(&RepoIndexArgs {
                artifacts_dir: repo_dir.display().to_string(),
                base_url: repo_dir.display().to_string(),
                output: None,
            })?;
            add_repo(&RepoAddArgs {
                name: "local".into(),
                url: repo_dir.display().to_string(),
            })?;
            update_indexes()?;
            search_packages(&SearchArgs {
                query: "example".into(),
            })?;
            show_package_info(&InfoArgs {
                package: "example".into(),
            })?;

            install_package(&InstallArgs {
                package: "example".into(),
                version: Some("1.0.0-1".into()),
            })?;
            list_installed_packages()?;
            upgrade_packages(&UpgradeArgs {
                package: Some("example".into()),
            })?;

            let layout = Layout::detect()?;
            let receipt = crate::receipt::load_receipt(&layout, "example")?
                .context("receipt missing after upgrade")?;
            assert_eq!(receipt.version, "1.2.0-1");
            assert!(layout.target_dir("bin").join("example").exists());
            assert!(layout.target_dir("bin").join("helper").exists());

            remove_package(&RemoveArgs {
                package: "example".into(),
            })?;
            assert!(crate::receipt::load_receipt(&layout, "example")?.is_none());
            Ok(())
        })
    }
}
