use std::{
    io::{BufReader, Read},
    path::Path,
};

use anyhow::{Context, Result, bail};
use blake2::{Blake2b512, Digest};

/// Verify the file checksum against the expected hex string, streaming the file.
pub fn verify_file_checksum(file: &str, expected_hex: &str) -> Result<()> {
    let actual = hash_file_blake2b(Path::new(file))?;
    if actual.eq_ignore_ascii_case(expected_hex) {
        Ok(())
    } else {
        bail!("checksum mismatch: expected {expected_hex}, got {actual}")
    }
}

/// Compute the Blake2b-512 hash of a file by streaming it in 64KB chunks.
pub fn hash_file_blake2b(path: &Path) -> Result<String> {
    let file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let mut hasher = Blake2b512::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf).context("read file for hashing")?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}
