//! Desktop integration actions for installed packages.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};

use crate::archive::validate_relative_path;
use crate::models::{Action, ActionType, InstalledPackage, ResolvedAction};
use crate::paths::Paths;

/// Apply manifest actions and return their resolved database records.
pub fn apply_actions(
    paths: &Paths,
    install_path: &Path,
    actions: &[Action],
) -> Result<Vec<ResolvedAction>> {
    let mut resolved = Vec::new();

    for action in actions {
        match apply_single_action(paths, install_path, action) {
            Ok(applied) => resolved.push(applied),
            Err(err) => {
                rollback_actions(&resolved);
                return Err(err);
            }
        }
    }

    Ok(resolved)
}

fn apply_single_action(
    paths: &Paths,
    install_path: &Path,
    action: &Action,
) -> Result<ResolvedAction> {
    let source_relative = Path::new(&action.source);
    validate_relative_path(source_relative)?;
    let source = install_path.join(source_relative);
    if !source.exists() {
        bail!(
            "action source does not exist in package payload: {}",
            action.source
        );
    }

    let category_dir = category_directory(paths, &action.target)?;
    let relative = target_relative_path(&action.target, source_relative)?;
    let target = category_dir.join(relative);

    if target.exists() || fs::symlink_metadata(&target).is_ok() {
        bail!("refusing to overwrite existing path: {}", target.display());
    }

    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create action target directory {}", parent.display()))?;
    }

    match action.action_type {
        ActionType::Link => {
            std::os::unix::fs::symlink(&source, &target).with_context(|| {
                format!(
                    "create symlink {} -> {}",
                    target.display(),
                    source.display()
                )
            })?;
        }
        ActionType::Copy => {
            fs::copy(&source, &target)
                .with_context(|| format!("copy {} to {}", source.display(), target.display()))?;
        }
    }

    Ok(ResolvedAction {
        source: action.source.clone(),
        target: action.target.clone(),
        action_type: action.action_type,
        resolved_target: target,
    })
}

pub fn rollback_actions(actions: &[ResolvedAction]) {
    for action in actions.iter().rev() {
        let _ = fs::remove_file(&action.resolved_target);
    }
}

pub fn uninstall_action(package: &InstalledPackage, action: &ResolvedAction) -> Result<()> {
    let target = &action.resolved_target;

    let metadata = match fs::symlink_metadata(target) {
        Ok(m) => m,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err).with_context(|| format!("inspect {}", target.display())),
    };

    if action.action_type == ActionType::Link && !metadata.file_type().is_symlink() {
        eprintln!("warning: not removing non-symlink {}", target.display());
        return Ok(());
    }

    if action.action_type == ActionType::Link {
        let link_target =
            fs::read_link(target).with_context(|| format!("read symlink {}", target.display()))?;
        let expected = package.install_path.join(&action.source);
        if link_target != expected {
            eprintln!(
                "warning: not removing symlink {} because it no longer points to {}",
                target.display(),
                expected.display()
            );
            return Ok(());
        }
    }

    fs::remove_file(target).with_context(|| format!("remove {}", target.display()))
}

fn category_directory(paths: &Paths, category: &str) -> Result<PathBuf> {
    match normalize_category(category).as_str() {
        "bin" => Ok(paths.local_bin()),
        "applications" => Ok(paths.applications()),
        "icons" => Ok(paths.icons()),
        "man" => Ok(paths.man()),
        other => bail!("unsupported action target category: {other}"),
    }
}

fn normalize_category(category: &str) -> String {
    match category {
        "desktop" => "applications".to_string(),
        other => other.to_string(),
    }
}

/// Preserve category-specific subpaths where XDG expects them.
fn target_relative_path(category: &str, source: &Path) -> Result<PathBuf> {
    let normalized = normalize_category(category);
    match normalized.as_str() {
        "icons" => strip_path_prefix(source, Path::new("share/icons")),
        "man" => strip_path_prefix(source, Path::new("share/man")),
        "bin" | "applications" => source
            .file_name()
            .map(PathBuf::from)
            .ok_or_else(|| anyhow!("action source has no file name: {}", source.display())),
        other => bail!("unsupported action target category: {other}"),
    }
}

fn strip_path_prefix(source: &Path, prefix: &Path) -> Result<PathBuf> {
    match source.strip_prefix(prefix) {
        Ok(stripped) if !stripped.as_os_str().is_empty() => Ok(stripped.to_path_buf()),
        _ => source
            .file_name()
            .map(PathBuf::from)
            .ok_or_else(|| anyhow!("action source has no file name: {}", source.display())),
    }
}
