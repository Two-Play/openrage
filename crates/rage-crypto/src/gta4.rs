//! GTA IV archive encryption: one AES-256 key, applied 16 times in ECB mode.
//!
//! The key is taken from the user's own `GTAIV.exe` by its SHA-1 fingerprint
//! and cached in the user's config directory, like the GTA V keys.

use std::fs;
use std::path::Path;

use aes::Aes256;
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit, generic_array::GenericArray};
use rayon::prelude::*;
use sha1::{Digest, Sha1};

use crate::KeyCacheError;

/// SHA-1 of the GTA IV archive key (a fingerprint only, not the key).
const KEY_SHA1: [u8; 20] = [
    0xDE, 0xA3, 0x75, 0xEF, 0x1E, 0x6E, 0xF2, 0x22, 0x3A, 0x12, 0x21, 0xC2, 0xC5, 0x75, 0xC4, 0x7B, 0xF1, 0x7E, 0xFA,
    0x5E,
];

/// How often the cipher is applied to every block.
const ROUNDS: usize = 16;

const CACHE_MAGIC: &[u8; 8] = b"ORGTA4K1";
const CHUNK: usize = 1 << 20;
/// The key is stored 4-byte aligned in the executable.
const ALIGN: usize = 4;

/// The GTA IV archive key.
#[derive(Clone)]
pub struct Gta4Key([u8; 32]);

impl std::fmt::Debug for Gta4Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Gta4Key(..)")
    }
}

impl Gta4Key {
    /// Accepts `key` only if it matches the known fingerprint.
    pub fn new(key: [u8; 32]) -> Option<Self> {
        (<[u8; 20]>::from(Sha1::digest(key)) == KEY_SHA1).then_some(Self(key))
    }

    /// Searches an executable (`GTAIV.exe`) for the key.
    pub fn find_in_executable(exe: &[u8]) -> Option<Self> {
        let found = (0..exe.len().div_ceil(CHUNK)).into_par_iter().find_map_any(|chunk| {
            let start = chunk * CHUNK;
            let end = (start + CHUNK).min(exe.len());
            (start..end).step_by(ALIGN).find_map(|pos| {
                let window = exe.get(pos..pos + 32)?;
                (<[u8; 20]>::from(Sha1::digest(window)) == KEY_SHA1).then(|| window.try_into().expect("32 bytes"))
            })
        });
        found.map(Self)
    }

    /// Decrypts `data` in place; a trailing partial block stays as it is.
    pub fn decrypt(&self, data: &mut [u8]) {
        let cipher = Aes256::new(GenericArray::from_slice(&self.0));
        for block in data.as_chunks_mut::<16>().0 {
            let b = GenericArray::from_mut_slice(block);
            for _ in 0..ROUNDS {
                cipher.decrypt_block(b);
            }
        }
    }

    /// Encrypts `data` in place; a trailing partial block stays as it is.
    pub fn encrypt(&self, data: &mut [u8]) {
        let cipher = Aes256::new(GenericArray::from_slice(&self.0));
        for block in data.as_chunks_mut::<16>().0 {
            let b = GenericArray::from_mut_slice(block);
            for _ in 0..ROUNDS {
                cipher.encrypt_block(b);
            }
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), KeyCacheError> {
        let io = |source| KeyCacheError::Io { path: path.to_owned(), source };
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(io)?;
        }
        let mut out = CACHE_MAGIC.to_vec();
        out.extend_from_slice(&self.0);
        fs::write(path, out).map_err(io)?;
        crate::keys::restrict_permissions(path);
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self, KeyCacheError> {
        let data = fs::read(path).map_err(|source| KeyCacheError::Io { path: path.to_owned(), source })?;
        let corrupt = || KeyCacheError::Corrupt(path.to_owned());
        if data.len() != 40 || &data[..8] != CACHE_MAGIC {
            return Err(corrupt());
        }
        Self::new(data[8..].try_into().expect("32 bytes")).ok_or_else(corrupt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrong_keys_are_rejected() {
        assert!(Gta4Key::new([0u8; 32]).is_none());
        assert!(Gta4Key::find_in_executable(&[0u8; 4096]).is_none());
    }

    #[test]
    fn sixteen_rounds_roundtrip() {
        let key = Gta4Key([3u8; 32]);
        let original: Vec<u8> = (0..40u8).collect();
        let mut data = original.clone();
        key.encrypt(&mut data);
        assert_ne!(data[..32], original[..32]);
        assert_eq!(data[32..], original[32..]);
        key.decrypt(&mut data);
        assert_eq!(data, original);
    }
}
