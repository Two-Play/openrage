//! Candidate dictionary matching, analogous to the RPF8 name resolver.
//! Native hash: 0x144F66DE0; ASCII normalization table: 0x1465B23B0.
//! A match identifies the hashed path, not necessarily its file extension.
use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::{self, Read};
use std::path::Path;

const C1: u64 = 0x87c3_7b91_1142_53d5;
const C2: u64 = 0x4cf5_ad43_2745_937f;

fn fmix(mut h: u64) -> u64 {
    h = (h ^ (h >> 33)).wrapping_mul(0xff51_afd7_ed55_8ccd);
    h = (h ^ (h >> 33)).wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    h ^ (h >> 33)
}

/// Native normalized MurmurHash3-derived low 64 bits, seed zero.
/// Stops at NUL, folds ASCII case and backslashes. The native finalizer uses
/// the low byte of the length; this differs from standard Murmur for >=256.
pub fn name_hash(path: &str) -> u64 {
    let data: Vec<u8> =
        path.bytes().take_while(|b| *b != 0).map(|b| if b == b'\\' { b'/' } else { b.to_ascii_lowercase() }).collect();
    let (mut h1, mut h2) = (0u64, 0u64);
    let (chunks, tail) = data.as_chunks::<16>();
    for chunk in chunks {
        let k1 = u64::from_le_bytes(chunk[..8].try_into().unwrap());
        let k2 = u64::from_le_bytes(chunk[8..].try_into().unwrap());
        h1 ^= k1.wrapping_mul(C1).rotate_left(31).wrapping_mul(C2);
        h1 = h1.rotate_left(27).wrapping_add(h2).wrapping_mul(5).wrapping_add(0x52dc_e729);
        h2 ^= k2.wrapping_mul(C2).rotate_left(33).wrapping_mul(C1);
        h2 = h2.rotate_left(31).wrapping_add(h1).wrapping_mul(5).wrapping_add(0x3849_5ab5);
    }
    let mut last = [0u8; 16];
    last[..tail.len()].copy_from_slice(tail);
    if tail.len() > 8 {
        h2 ^= u64::from_le_bytes(last[8..].try_into().unwrap()).wrapping_mul(C2).rotate_left(33).wrapping_mul(C1);
    }
    if !tail.is_empty() {
        h1 ^= u64::from_le_bytes(last[..8].try_into().unwrap()).wrapping_mul(C1).rotate_left(31).wrapping_mul(C2);
    }
    h1 ^= (data.len() & 255) as u64;
    h2 ^= (data.len() & 255) as u64;
    h1 = h1.wrapping_add(h2);
    h2 = h2.wrapping_add(h1);
    fmix(h1).wrapping_add(fmix(h2))
}

#[derive(Debug, Default)]
pub struct NameIndex {
    // Preserve distinct matches: never silently choose between collisions.
    names: HashMap<u64, BTreeSet<String>>,
}

impl NameIndex {
    /// Import an explicit UTF-8 candidate list (one path per line), <=1MiB.
    /// Only matching paths are retained. This never scans game payloads.
    pub fn load_candidates(path: &Path, wanted: &HashSet<u64>) -> io::Result<Self> {
        let mut bytes = Vec::new();
        std::fs::File::open(path)?.take(1_048_577).read_to_end(&mut bytes)?;
        if bytes.len() > 1_048_576 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "name candidate list exceeds 1MiB"));
        }
        let text = std::str::from_utf8(&bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let mut index = Self::default();
        for line in text.lines() {
            index.add_candidate(line.trim(), wanted);
        }
        Ok(index)
    }

    pub fn add_candidate(&mut self, path: &str, wanted: &HashSet<u64>) {
        if path.is_empty() || path.len() > 1024 || !path.is_ascii() || path.bytes().any(|b| b.is_ascii_control()) {
            return;
        }
        let path = path.to_ascii_lowercase().replace('\\', "/");
        let after = path.split_once(":/").map_or(path.as_str(), |(_, p)| p);
        let stripped = after.strip_prefix("x64/").unwrap_or(after);
        let base = stripped.rsplit('/').next().unwrap_or(stripped);
        let stem = |s: &str| s.rsplit_once('.').map_or(s.to_owned(), |(s, _)| s.to_owned());
        for candidate in
            [path.clone(), after.to_owned(), stripped.to_owned(), base.to_owned(), stem(stripped), stem(base)]
        {
            if candidate.is_empty() {
                continue;
            }
            let hash = name_hash(&candidate);
            if wanted.contains(&hash) {
                self.names.entry(hash).or_default().insert(candidate);
            }
        }
    }

    /// All matching normalized paths; no inferred extension is appended.
    pub fn candidates(&self, hash: u64) -> impl Iterator<Item = &str> {
        self.names.get(&hash).into_iter().flatten().map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_vectors_and_normalization() {
        assert_eq!(name_hash(""), 0);
        assert_eq!(name_hash("a"), 0x8555_5565_f659_7889);
        assert_eq!(name_hash("1234567890123456"), 0x4fbe_5dc5_c0e3_2cf8);
        // Independently reproduced native function; matches common.rpf row 5.
        assert_eq!(name_hash("DATA\\graphics/graphics_definitions.meta"), 0x5c33_030f_6402_b9dd);
        assert_eq!(name_hash("a\0ignored"), name_hash("a"));
        assert_eq!(name_hash(&"a".repeat(256)), 0x42bf_fdb3_5fe4_59a8);
    }

    #[test]
    fn dictionary_matches_mount_and_extension_forms_only_when_wanted() {
        let path = "data/camera/cam_core";
        let hash = name_hash(path);
        let mut index = NameIndex::default();
        index.add_candidate("COMMON:/DATA/camera/cam_core.ymt", &HashSet::from([hash]));
        assert_eq!(index.candidates(hash).collect::<Vec<_>>(), [path]);
        assert_eq!(index.candidates(name_hash("cam_core")).count(), 0);
    }
}
