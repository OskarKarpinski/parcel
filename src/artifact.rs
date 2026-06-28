use std::{
    fs::{self, File},
    io::{Cursor, Read, Write},
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use blake2::Digest;
use tar::{Archive, Builder, EntryType, Header};

use crate::{
    layout::Layout,
    parcel_manifest::PackageManifest,
    utils::{
        arch::{Architecture, get_architecture},
        hash::hash_file_blake2b,
    },
};

#[derive(Debug)]
pub struct PackageFile {
    pub manifest: PackageManifest,
    pub payload_path: PathBuf,
}

pub fn read_package_manifest(package_path: &Path) -> Result<PackageManifest> {
    let package = stage_package(package_path)?;
    let _ = fs::remove_file(package.payload_path);
    Ok(package.manifest)
}

pub fn stage_package(package_path: &Path) -> Result<PackageFile> {
    let layout = Layout::detect()?;
    let staged_payload = layout.temp_path("payload", ".tar.zst");

    let file =
        File::open(package_path).with_context(|| format!("open {}", package_path.display()))?;
    let mut archive = Archive::new(file);
    let mut manifest = None;
    let mut payload_written = false;

    for entry in archive.entries().context("iterate package entries")? {
        let mut entry = entry?;
        let path = entry.path()?.to_path_buf();
        match path.to_string_lossy().as_ref() {
            "manifest.yml" => {
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes)?;
                manifest = Some(serde_yaml::from_slice::<PackageManifest>(&bytes)?);
            }
            "data.tar.zst" => {
                let mut out = File::create(&staged_payload)
                    .with_context(|| format!("create {}", staged_payload.display()))?;
                std::io::copy(&mut entry, &mut out)?;
                payload_written = true;
            }
            _ => {}
        }
    }

    let manifest = manifest.context("manifest.yml missing from package archive")?;
    if !payload_written {
        bail!("data.tar.zst missing from package archive");
    }

    Ok(PackageFile {
        manifest,
        payload_path: staged_payload,
    })
}

pub fn extract_package(package_path: &Path, destination: &Path) -> Result<PackageManifest> {
    let package = stage_package(package_path)?;
    let manifest = package.manifest.clone();
    validate_manifest_arch(&manifest)?;
    extract_payload_tar_zst(&package.payload_path, destination)?;
    let _ = fs::remove_file(package.payload_path);
    Ok(manifest)
}

pub fn validate_manifest_arch(manifest: &PackageManifest) -> Result<()> {
    let host = get_architecture();
    if manifest.arch != host {
        bail!(
            "package {} {} targets {}, but current host is {}",
            manifest.name,
            manifest.version,
            manifest.arch,
            host
        );
    }
    Ok(())
}

pub fn extract_payload_tar_zst(payload_path: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination).with_context(|| format!("create {}", destination.display()))?;

    let file =
        File::open(payload_path).with_context(|| format!("open {}", payload_path.display()))?;
    let decoder = zstd::Decoder::new(file).context("create zstd decoder")?;
    let mut archive = Archive::new(decoder);

    for entry in archive.entries().context("iterate payload entries")? {
        let mut entry = entry?;
        let relative_path = normalize_archive_path(&entry.path()?)?;
        if relative_path.as_os_str().is_empty() {
            continue;
        }

        if matches!(
            entry.header().entry_type(),
            EntryType::Link | EntryType::GNULongLink
        ) {
            bail!("hard links are not supported in package payloads");
        }

        if !entry.unpack_in(destination)? {
            bail!(
                "refusing to unpack payload entry {} outside {}",
                relative_path.display(),
                destination.display()
            );
        }
    }

    Ok(())
}

pub fn write_package(
    manifest: &PackageManifest,
    payload_dir: &Path,
    output_path: &Path,
) -> Result<String> {
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }

    let layout = Layout::detect()?;
    let compressed_payload_path = layout.temp_path("data", ".tar.zst");
    compress_directory_to_tar_zst(payload_dir, &compressed_payload_path)?;

    let file =
        File::create(output_path).with_context(|| format!("create {}", output_path.display()))?;
    let mut outer = Builder::new(file);
    append_bytes_tar(
        &mut outer,
        "manifest.yml",
        serde_yaml::to_string(manifest)?.as_bytes(),
    )?;
    append_file_tar(&mut outer, "data.tar.zst", &compressed_payload_path)?;
    outer.finish().context("finish package archive")?;

    let _ = fs::remove_file(&compressed_payload_path);
    hash_file_blake2b(output_path)
}

pub fn compress_directory_to_tar_zst(source_dir: &Path, output_path: &Path) -> Result<()> {
    let file =
        File::create(output_path).with_context(|| format!("create {}", output_path.display()))?;
    let mut encoder = zstd::Encoder::new(file, 0).context("create zstd encoder")?;
    {
        let mut tar = Builder::new(&mut encoder);
        tar.follow_symlinks(false);
        tar.append_dir_all(".", source_dir)
            .with_context(|| format!("archive {}", source_dir.display()))?;
        tar.finish().context("finish payload tar")?;
    }
    encoder.finish()?;
    Ok(())
}

pub fn append_bytes_tar<W: Write>(
    builder: &mut Builder<W>,
    path: &str,
    bytes: &[u8],
) -> Result<()> {
    let mut header = Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    builder
        .append_data(&mut header, path, Cursor::new(bytes))
        .with_context(|| format!("append {path} to tar"))?;
    Ok(())
}

pub fn append_file_tar<W: Write>(
    builder: &mut Builder<W>,
    path: &str,
    file_path: &Path,
) -> Result<()> {
    let mut file =
        File::open(file_path).with_context(|| format!("open {}", file_path.display()))?;
    let metadata = file.metadata()?;
    let mut header = Header::new_gnu();
    header.set_size(metadata.len());
    header.set_mode(0o644);
    header.set_cksum();
    builder
        .append_data(&mut header, path, &mut file)
        .with_context(|| format!("append {path} to tar"))?;
    Ok(())
}

pub fn normalize_archive_path(path: &Path) -> Result<PathBuf> {
    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
            Component::RootDir | Component::Prefix(_) => {
                bail!("archive path must be relative: {}", path.display())
            }
            Component::ParentDir => bail!("archive path may not contain '..': {}", path.display()),
        }
    }

    Ok(normalized)
}

pub fn copy_tree(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst).with_context(|| format!("create {}", dst.display()))?;
    for entry in walkdir::WalkDir::new(src).follow_links(false) {
        let entry = entry?;
        let path = entry.path();
        if path == src {
            continue;
        }
        let relative = path
            .strip_prefix(src)
            .with_context(|| format!("strip prefix {}", src.display()))?;
        let target = dst.join(relative);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target).with_context(|| format!("create {}", target.display()))?;
        } else if entry.file_type().is_symlink() {
            let link_target =
                fs::read_link(path).with_context(|| format!("readlink {}", path.display()))?;
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("create {}", parent.display()))?;
            }
            #[cfg(unix)]
            std::os::unix::fs::symlink(&link_target, &target)
                .with_context(|| format!("symlink {}", target.display()))?;
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("create {}", parent.display()))?;
            }
            fs::copy(path, &target)
                .with_context(|| format!("copy {} to {}", path.display(), target.display()))?;
        }
    }
    Ok(())
}

pub fn directory_checksum(path: &Path) -> Result<String> {
    let mut hasher = blake2::Blake2b512::new();
    let mut entries = Vec::new();
    for entry in walkdir::WalkDir::new(path).follow_links(false) {
        let entry = entry?;
        let current = entry.path();
        if current == path {
            continue;
        }
        entries.push(current.to_path_buf());
    }
    entries.sort();

    for entry_path in entries {
        let relative = entry_path
            .strip_prefix(path)
            .with_context(|| format!("strip prefix {}", path.display()))?;
        hasher.update(relative.to_string_lossy().as_bytes());
        let metadata = fs::symlink_metadata(&entry_path)?;
        if metadata.is_dir() {
            hasher.update(b"dir");
        } else if metadata.file_type().is_symlink() {
            hasher.update(b"symlink");
            hasher.update(fs::read_link(&entry_path)?.to_string_lossy().as_bytes());
        } else {
            hasher.update(b"file");
            let mut file = File::open(&entry_path)?;
            let mut buf = [0u8; 64 * 1024];
            loop {
                let read = file.read(&mut buf)?;
                if read == 0 {
                    break;
                }
                hasher.update(&buf[..read]);
            }
        }
    }

    Ok(hex::encode(hasher.finalize()))
}

pub fn package_file_name(name: &str, version: &str, arch: Architecture) -> String {
    format!("{name}-{version}-{arch}.parcel")
}
