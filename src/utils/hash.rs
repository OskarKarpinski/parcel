use blake2::{Blake2b512, Digest};
use anyhow::{Result, bail};

/// Compute the Blake2b-512 hash of the given bytes and verify it against the expected hex string.
pub fn verify_blake2b(bytes: &[u8], expected_hex: &str) -> Result<()> {
    let actual = hash_blake2b(bytes);
    if actual.eq_ignore_ascii_case(expected_hex) {
        Ok(())
    } else {
        bail!("checksum mismatch: expected {expected_hex}, got {actual}")
    }
}

/// Verify the file checksum against the expected hex string.
pub fn verify_file_checksum(file: &str, expected_hex: &str) -> Result<()> {
    let bytes = std::fs::read(file)?;
    verify_blake2b(&bytes, expected_hex)
}

/// Compute the Blake2b-512 hash of the given bytes and return it as a hex string.
pub fn hash_blake2b(bytes: &[u8]) -> String {
    hex::encode(Blake2b512::digest(bytes))
}
