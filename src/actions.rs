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
        match apply_action(paths, install_path, action) {
            Ok(applied) => resolved.push(applied),
            Err(err) => {
                cleanup_action_targets(&resolved);
                return Err(err);
            }
        }
    }

    Ok(resolved)
}

fn apply_action(paths: &Paths, install_path: &Path, action: &Action) -> Result<ResolvedAction> {
    let source_relative = Path::new(&action.source);
    validate_relative_path(source_relative)?;
    let source = install_path.join(source_relative);
    if !source.exists() {
        bail!(
            "action source does not exist in package payload: {}",
            action.source
        );
    }

    let target_root = target_root(paths, &action.target)?;
    let target_relative = action_target_relative(&action.target, source_relative)?;
    let target = target_root.join(target_relative);

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

pub fn cleanup_action_targets(actions: &[ResolvedAction]) {
    for action in actions.iter().rev() {
        let _ = fs::remove_file(&action.resolved_target);
    }
}

pub fn remove_action_target(package: &InstalledPackage, action: &ResolvedAction) -> Result<()> {
    let target = &action.resolved_target;
    match fs::symlink_metadata(target) {
        Ok(metadata) => {
            if action.action_type == ActionType::Link {
                if !metadata.file_type().is_symlink() {
                    eprintln!("warning: not removing non-symlink {}", target.display());
                    return Ok(());
                }
                let link_target = fs::read_link(target)
                    .with_context(|| format!("read symlink {}", target.display()))?;
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
            fs::remove_file(target).with_context(|| format!("remove {}", target.display()))?;
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err).with_context(|| format!("inspect {}", target.display())),
    }
    Ok(())
}

fn target_root(paths: &Paths, target: &str) -> Result<PathBuf> {
    match normalize_target(target).as_str() {
        "bin" => Ok(paths.local_bin()),
        "applications" => Ok(paths.applications()),
        "icons" => Ok(paths.icons()),
        "man" => Ok(paths.man()),
        other => bail!("unsupported action target category: {other}"),
    }
}

fn normalize_target(target: &str) -> String {
    match target {
        "desktop" => "applications".to_string(),
        other => other.to_string(),
    }
}

/// Preserve category-specific subpaths where XDG expects them.
fn action_target_relative(target: &str, source: &Path) -> Result<PathBuf> {
    let normalized = normalize_target(target);
    match normalized.as_str() {
        "icons" => strip_known_prefix(source, Path::new("share/icons")),
        "man" => strip_known_prefix(source, Path::new("share/man")),
        "bin" | "applications" => source
            .file_name()
            .map(PathBuf::from)
            .ok_or_else(|| anyhow!("action source has no file name: {}", source.display())),
        other => bail!("unsupported action target category: {other}"),
    }
}

fn strip_known_prefix(source: &Path, prefix: &Path) -> Result<PathBuf> {
    match source.strip_prefix(prefix) {
        Ok(stripped) if !stripped.as_os_str().is_empty() => Ok(stripped.to_path_buf()),
        _ => source
            .file_name()
            .map(PathBuf::from)
            .ok_or_else(|| anyhow!("action source has no file name: {}", source.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_target_for_icons_preserves_hicolor_subtree() {
        let target = action_target_relative(
            "icons",
            Path::new("share/icons/hicolor/48x48/apps/example.png"),
        )
        .unwrap();

        assert_eq!(target, Path::new("hicolor/48x48/apps/example.png"));
    }

    #[test]
    fn action_target_accepts_desktop_alias() {
        let target = action_target_relative("desktop", Path::new("example.desktop")).unwrap();

        assert_eq!(target, Path::new("example.desktop"));
    }
}
