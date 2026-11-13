//! The complete GTA V key set plus its local on-disk cache.

use std::fs;
use std::path::{Path, PathBuf};

use sha1::{Digest, Sha1};

use crate::key_hashes;
use crate::magic::{MagicContents, NG_KEYS_LEN, NG_TABLES_LEN};
use crate::ng::{NG_KEY_COUNT, NgKey, NgTables, decrypt_ng};
use crate::{aes_ecb, scan::ExeScan};

const CACHE_MAGIC: &[u8; 8] = b"ORKEYS01";

#[derive(Debug, thiserror::Error)]
pub enum KeyCacheError {
    #[error("I/O error on key cache {path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("key cache {0} is corrupt or from an incompatible OpenRAGE version")]
    Corrupt(PathBuf),
    #[error("no per-user config directory available on this system")]
    NoConfigDir,
}

/// Everything needed to read GTA V PC archives.
#[derive(Clone)]
pub struct GtaVKeys {
    pub aes_key: [u8; 32],
    pub hash_lut: [u8; 256],
    pub ng_keys: Vec<NgKey>,
    pub ng_tables: Box<NgTables>,
    /// Key for encrypted audio containers (`.awc`); from the key blob.
    pub awc_key: Option<[u8; 16]>,
}

impl GtaVKeys {
    /// Builds a key set from raw byte blobs (keys: 101×272, tables: 17×16×256×4).
    pub fn from_parts(aes_key: [u8; 32], hash_lut: [u8; 256], ng_keys: &[u8], ng_tables: &[u8]) -> Option<Self> {
        if ng_keys.len() != NG_KEYS_LEN || ng_tables.len() != NG_TABLES_LEN {
            return None;
        }
        let words = |b: &[u8]| -> Vec<u32> { b.as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c)).collect() };
        let ng_keys = words(ng_keys).as_chunks::<68>().0.to_vec();
        let tw = words(ng_tables);
        let mut tables: Box<NgTables> = vec![[[0u32; 256]; 16]; 17].into_boxed_slice().try_into().ok()?;
        for (i, w) in tw.into_iter().enumerate() {
            tables[i / 4096][(i / 256) % 16][i % 256] = w;
        }
        Some(Self { aes_key, hash_lut, ng_keys, ng_tables: tables, awc_key: None })
    }

    /// Builds a key set entirely from an executable scan (works for GTA V Legacy).
    pub fn from_scan(scan: &ExeScan) -> Option<Self> {
        let aes = scan.aes_key?;
        let lut = scan.hash_lut?;
        if !scan.has_complete_ng() {
            return None;
        }
        let keys: Vec<u8> = scan.ng_keys.iter().flat_map(|k| k.clone().unwrap()).collect();
        let tables: Vec<u8> = scan.ng_tables.iter().flat_map(|t| t.clone().unwrap()).collect();
        Self::from_parts(aes, lut, &keys, &tables)
    }

    /// Builds a key set from a decoded CodeWalker key blob.
    pub fn from_magic(aes_key: [u8; 32], magic: &MagicContents) -> Option<Self> {
        let mut keys = Self::from_parts(aes_key, magic.hash_lut, &magic.ng_keys, &magic.ng_tables)?;
        keys.awc_key = Some(magic.awc_key);
        Some(keys)
    }

    /// Decrypts an encrypted part of an audio container in place (needs the AWC key).
    pub fn decrypt_awc(&self, data: &mut [u8]) -> bool {
        match &self.awc_key {
            Some(key) => {
                crate::xxtea::decrypt(data, key);
                true
            }
            None => false,
        }
    }

    /// RAGE's LUT-based file name hash, used to pick the NG key for a file.
    pub fn name_hash(&self, name: &str) -> u32 {
        let mut result: u32 = 0;
        for b in name.bytes() {
            let temp = 1025u32.wrapping_mul((self.hash_lut[b as usize] as u32).wrapping_add(result));
            result = (temp >> 6) ^ temp;
        }
        let r9 = result.wrapping_mul(9);
        32769u32.wrapping_mul((r9 >> 11) ^ r9)
    }

    /// The NG key for a file of the given name and length.
    pub fn ng_key_for(&self, name: &str, length: u32) -> &NgKey {
        let idx = self.name_hash(name).wrapping_add(length).wrapping_add(61) % NG_KEY_COUNT as u32;
        &self.ng_keys[idx as usize]
    }

    pub fn decrypt_ng(&self, data: &mut [u8], name: &str, length: u32) {
        decrypt_ng(data, self.ng_key_for(name, length), &self.ng_tables);
    }

    pub fn decrypt_aes(&self, data: &mut [u8]) {
        aes_ecb::decrypt_aes(data, &self.aes_key);
    }

    /// Default cache location: `<config dir>/openrage/gtav.keys`.
    pub fn default_cache_path() -> Result<PathBuf, KeyCacheError> {
        Ok(dirs::config_dir().ok_or(KeyCacheError::NoConfigDir)?.join("openrage").join("gtav.keys"))
    }

    pub fn save(&self, path: &Path) -> Result<(), KeyCacheError> {
        let io = |source| KeyCacheError::Io { path: path.to_owned(), source };
        let mut out = Vec::with_capacity(8 + 32 + 256 + NG_KEYS_LEN + NG_TABLES_LEN);
        out.extend_from_slice(CACHE_MAGIC);
        out.extend_from_slice(&self.aes_key);
        out.extend_from_slice(&self.hash_lut);
        for k in &self.ng_keys {
            k.iter().for_each(|w| out.extend_from_slice(&w.to_le_bytes()));
        }
        for w in self.ng_tables.iter().flatten().flatten() {
            out.extend_from_slice(&w.to_le_bytes());
        }
        if let Some(k) = &self.awc_key {
            out.extend_from_slice(k);
        }
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(io)?;
        }
        fs::write(path, out).map_err(io)?;
        restrict_permissions(path);
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self, KeyCacheError> {
        let data = fs::read(path).map_err(|source| KeyCacheError::Io { path: path.to_owned(), source })?;
        let corrupt = || KeyCacheError::Corrupt(path.to_owned());
        // An optional AWC key (16 bytes) follows the NG tables.
        let base = 8 + 32 + 256 + NG_KEYS_LEN + NG_TABLES_LEN;
        if (data.len() != base && data.len() != base + 16) || &data[..8] != CACHE_MAGIC {
            return Err(corrupt());
        }
        let awc_key: Option<[u8; 16]> = data.get(base..base + 16).map(|k| k.try_into().unwrap());
        let aes: [u8; 32] = data[8..40].try_into().unwrap();
        if <[u8; 20]>::from(Sha1::digest(aes)) != key_hashes::AES_KEY {
            return Err(corrupt());
        }
        let lut: [u8; 256] = data[40..296].try_into().unwrap();
        let ng_keys = &data[296..296 + NG_KEYS_LEN];
        let ng_tables = &data[296 + NG_KEYS_LEN..base];
        let mut keys = Self::from_parts(aes, lut, ng_keys, ng_tables).ok_or_else(corrupt)?;
        keys.awc_key = awc_key;
        Ok(keys)
    }
}

/// Key material is the user's own, but keep it out of other users' reach.
pub(crate) fn restrict_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}
