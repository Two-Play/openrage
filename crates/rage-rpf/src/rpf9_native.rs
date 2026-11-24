//! Executable-backed native RPF9 indexes. The executable is parsed as data.

// Not yet wired into the archive reader; integration follows.
#![allow(dead_code)]

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use crate::rpf9_cipher::Cipher;
use crate::rpf9_index::{Catalog, NativeEntry, Storage};
use crate::rpf9_names;

pub(crate) struct Materials {
    pub cipher: Cipher,
    names: HashMap<(u32, u64, u8), String>,
}

impl std::fmt::Debug for Materials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Materials").field("candidate_names", &self.names.len()).finish_non_exhaustive()
    }
}

/// Reuses runtime tables, never a checked-in or on-disk key cache.
pub(crate) fn materials(path: &Path) -> Option<Arc<Materials>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Arc<Materials>>>> = OnceLock::new();
    let executable =
        path.ancestors().skip(1).take(5).map(|p| p.join("game_scarlett_master.exe")).find(|p| p.is_file())?;
    let mut cache = CACHE.get_or_init(Default::default).lock().ok()?;
    if let Some(materials) = cache.get(&executable) {
        return Some(Arc::clone(materials));
    }
    let bytes = std::fs::read(&executable).ok()?;
    let cipher = Cipher::from_exe(&bytes)?;
    let mut names = HashMap::new();
    for candidate in rpf9_names::candidates(&bytes) {
        let hashes = rpf9_names::path_hashes(&candidate);
        let key = (hashes.hash32, hashes.hash64, hashes.file_type.unwrap_or(255));
        names.entry(key).or_insert(candidate);
    }
    let materials = Arc::new(Materials { cipher, names });
    cache.insert(executable, Arc::clone(&materials));
    Some(materials)
}

#[derive(Debug)]
pub(crate) struct NativeArchive {
    pub catalog: Catalog,
    pub materials: Arc<Materials>,
    pub paths: Vec<String>,
}

fn u16_at(b: &[u8], at: usize) -> Option<usize> {
    Some(u16::from_le_bytes(b.get(at..at.checked_add(2)?)?.try_into().ok()?) as usize)
}
fn u32_at(b: &[u8], at: usize) -> Option<usize> {
    Some(u32::from_le_bytes(b.get(at..at.checked_add(4)?)?.try_into().ok()?) as usize)
}

pub(crate) fn decode_header(cipher: &Cipher, h: &mut [u8]) -> Option<()> {
    if h.len() < 64 || h.get(..4)? != b"9FPR" {
        return None;
    }
    if h[0x2e] == 0 {
        return Some(());
    }
    // The supported Scarlett build uses selector 9: two metadata spans
    // share one CBC chain, leaving the secondary transform prefix intact.
    if h[0x2e] != 9 {
        return None;
    }
    let count = u16_at(h, 0x1a)?;
    let directory_len = count.checked_mul(8)?.checked_add(15)? & !15;
    let blob = 64usize.checked_add(directory_len)?;
    let metadata_end = blob.checked_add(u32_at(h, 0x1c)?)?;
    let tail = u32_at(h, 0x30)?;
    let prefix = metadata_end.checked_sub(tail)?;
    let split = u32_at(h, 8)?;
    if prefix < blob || split < metadata_end || split > h.len() {
        return None;
    }
    let mut iv = cipher.header_iv();
    cipher.decrypt_cbc(0x10000001, h.get_mut(64..blob)?, &mut iv)?;
    cipher.decrypt_cbc(0x10000001, h.get_mut(blob..prefix)?, &mut iv)?;
    let descriptor = (0..count).find_map(|i| {
        let at = 64 + i * 8;
        (u16_at(h, at)? == 1).then(|| blob.checked_add(u32_at(h, at + 4)?)).flatten()
    })?;
    let key = u32_at(h, descriptor)? as u32;
    if u16_at(h, descriptor + 4)? != 16 {
        return None;
    }
    let initial: [u8; 16] = h.get(descriptor + 6..descriptor + 22)?.try_into().ok()?;
    let mut iv = initial;
    cipher.decrypt_cbc(key, h.get_mut(prefix..split)?, &mut iv)?;
    let mut iv = initial;
    cipher.decrypt_cbc(key, h.get_mut(split..)?, &mut iv)?;
    Some(())
}

impl NativeArchive {
    pub fn open(path: &Path, base: u64, extent: u64, materials: Arc<Materials>) -> Result<Self, String> {
        let mut file = File::open(path).map_err(|e| e.to_string())?;
        file.seek(SeekFrom::Start(base)).map_err(|e| e.to_string())?;
        let mut head = [0u8; 64];
        file.read_exact(&mut head).map_err(|e| e.to_string())?;
        let len = u32_at(&head, 4).ok_or("short header")?;
        if !(64..=64 << 20).contains(&len) || len as u64 > extent {
            return Err("native header extent is outside the archive".into());
        }
        let mut bytes = vec![0u8; len];
        bytes[..64].copy_from_slice(&head);
        file.read_exact(&mut bytes[64..]).map_err(|e| e.to_string())?;
        decode_header(&materials.cipher, &mut bytes).ok_or("unsupported or damaged native header transform")?;
        let catalog = Catalog::parse(bytes, extent)?;
        let paths = catalog
            .entries
            .iter()
            .map(|entry| {
                let exact = entry
                    .name_hash32
                    .zip(entry.name_hash64)
                    .and_then(|(a, b)| materials.names.get(&(a, b, entry.type_id)));
                if let Some(path) = exact {
                    return path.clone();
                }
                let ext = rpf9_names::extension_for_type(entry.type_id).unwrap_or("bin");
                format!("entries/{:06}_{:016x}.{ext}", entry.slot, entry.name_hash64.unwrap_or(0))
            })
            .collect();
        Ok(Self { catalog, materials, paths })
    }

    pub fn find(&self, path: &str) -> Option<usize> {
        if let Some(slot) = self.paths.iter().position(|p| p.eq_ignore_ascii_case(path)) {
            return Some(slot);
        }
        let h = rpf9_names::path_hashes(path);
        self.catalog.entries.iter().position(|e| {
            e.type_id == h.file_type.unwrap_or(255)
                && e.name_hash64 == Some(h.hash64)
                && e.name_hash32.is_none_or(|v| v == h.hash32)
        })
    }

    pub fn copy_to(&self, path: &Path, base: u64, entry: &NativeEntry, out: &mut dyn Write) -> Result<u64, String> {
        let mut file = File::open(path).map_err(|e| e.to_string())?;
        file.seek(SeekFrom::Start(base.checked_add(entry.offset).ok_or("offset overflow")?))
            .map_err(|e| e.to_string())?;
        if matches!(entry.storage, Storage::Raw) {
            let n = std::io::copy(&mut file.take(entry.size), out).map_err(|e| e.to_string())?;
            return if n == entry.size { Ok(n) } else { Err("short native member".into()) };
        }
        // The chunk decoder validates each decoded length and checksum.
        self.copy_chunks(file, entry, out)
    }

    fn copy_chunks(&self, _file: File, _entry: &NativeEntry, _out: &mut dyn Write) -> Result<u64, String> {
        Err("native payload layout is not supported yet".into())
    }
}
