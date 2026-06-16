//! Reading and extracting `.parcel` package archives.

use std::fs::File;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use walkdir::WalkDir;

use crate::models::{Compression, Manifest};

/// Parsed outer package archive state.
pub struct ParsedParcelArchive {
    pub manifest: Manifest,
    pub data_path: PathBuf,
    pub compression: Compression,
}

/// Read the outer `.parcel` tar archive and copy its payload tar to a temp dir.
pub fn read_parcel_archive(package_path: &Path, temp_dir: &Path) -> Result<ParsedParcelArchive> {
    let file = File::open(package_path)
        .with_context(|| format!("open package archive {}", package_path.display()))?;
    let mut outer = tar::Archive::new(file);

    let mut manifest: Option<Manifest> = None;
    let mut data_path: Option<PathBuf> = None;
    let mut compression: Option<Compression> = None;

    for entry in outer.entries().context("read package archive entries")? {
        let mut entry = entry.context("read package archive entry")?;
        let path = entry.path().context("read package archive entry path")?;

        match path.as_ref() {
            p if p == Path::new("manifest.yml") => {
                let mut contents = String::new();
                entry
                    .read_to_string(&mut contents)
                    .context("read manifest.yml")?;
                manifest = Some(serde_yaml::from_str(&contents).context("parse manifest.yml")?);
            }
            p if p == Path::new("data.tar.zst") => {
                let target = temp_dir.join("data.tar.zst");
                copy_entry_to_file(&mut entry, &target)?;
                data_path = Some(target);
                compression = Some(Compression::Zstd);
            }
            p if p == Path::new("data.tar.xz") => {
                let target = temp_dir.join("data.tar.xz");
                copy_entry_to_file(&mut entry, &target)?;
                data_path = Some(target);
                compression = Some(Compression::Xz);
            }
            _ => {}
        }
    }

    Ok(ParsedParcelArchive {
        manifest: manifest.ok_or_else(|| anyhow!("package is missing manifest.yml"))?,
        data_path: data_path
            .ok_or_else(|| anyhow!("package is missing data.tar.zst or data.tar.xz"))?,
        compression: compression.expect("compression is set with data_path"),
    })
}

fn copy_entry_to_file<R: Read>(entry: &mut tar::Entry<R>, target: &Path) -> Result<()> {
    let mut output =
        File::create(target).with_context(|| format!("create {}", target.display()))?;
    io::copy(entry, &mut output).with_context(|| format!("write {}", target.display()))?;
    Ok(())
}

/// Extract the inner payload tar, rejecting absolute paths and `..` traversal.
pub fn extract_payload(
    data_path: &Path,
    compression: Compression,
    target_dir: &Path,
) -> Result<()> {
    let file = File::open(data_path).with_context(|| format!("open {}", data_path.display()))?;
    match compression {
        Compression::Zstd => {
            let decoder = zstd::Decoder::new(file).context("create zstd decoder")?;
            extract_tar_stream(decoder, target_dir)
        }
        Compression::Xz => {
            let decoder = xz2::read::XzDecoder::new(file);
            extract_tar_stream(decoder, target_dir)
        }
    }
}

fn extract_tar_stream<R: Read>(reader: R, target_dir: &Path) -> Result<()> {
    let mut archive = tar::Archive::new(reader);
    for entry in archive.entries().context("read payload tar entries")? {
        let mut entry = entry.context("read payload tar entry")?;
        let path = entry.path().context("read payload tar path")?.into_owned();
        validate_relative_path(&path)?;
        entry
            .unpack_in(target_dir)
            .with_context(|| format!("extract payload path {}", path.display()))?;
    }
    Ok(())
}

pub fn validate_relative_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() {
        bail!("empty path is not allowed in archives");
    }

    for component in path.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir => bail!("archive path escapes install root: {}", path.display()),
            Component::RootDir | Component::Prefix(_) => {
                bail!("absolute archive path is not allowed: {}", path.display())
            }
        }
    }
    Ok(())
}

pub fn list_payload_files(install_path: &Path) -> Result<Vec<String>> {
    let mut files = Vec::new();
    for entry in WalkDir::new(install_path) {
        let entry = entry.context("walk installed payload")?;
        if entry.file_type().is_file() || entry.file_type().is_symlink() {
            let relative = entry
                .path()
                .strip_prefix(install_path)
                .context("installed file is outside install path")?;
            files.push(path_to_unix(relative)?);
        }
    }
    files.sort();
    Ok(files)
}

fn path_to_unix(path: &Path) -> Result<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push(
                part.to_str()
                    .ok_or_else(|| anyhow!("path is not valid UTF-8: {}", path.display()))?,
            ),
            Component::CurDir => {}
            _ => bail!("path is not relative: {}", path.display()),
        }
    }
    Ok(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_paths_must_not_escape_install_root() {
        assert!(validate_relative_path(Path::new("bin/example")).is_ok());
        assert!(validate_relative_path(Path::new("../example")).is_err());
        assert!(validate_relative_path(Path::new("/tmp/example")).is_err());
    }
}
