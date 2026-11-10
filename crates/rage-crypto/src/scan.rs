//! Locates crypto material inside a game executable by SHA-1 fingerprint.
//!
//! Every 8-byte-aligned window of the executable is hashed for each material
//! size and compared to the known fingerprints in [`crate::key_hashes`].

use std::collections::HashMap;
use std::sync::Mutex;

use rayon::prelude::*;
use sha1::{Digest, Sha1};

use crate::key_hashes;

const ALIGN: usize = 8;
const CHUNK: usize = 1 << 20;

const LEN_AES: usize = 32;
const LEN_LUT: usize = 256;
const LEN_NG_KEY: usize = 272;
const LEN_NG_TABLE: usize = 1024;

/// What was found in an executable. NG material is absent from GTA V Enhanced.
#[derive(Default)]
pub struct ExeScan {
    pub aes_key: Option<[u8; 32]>,
    pub hash_lut: Option<[u8; 256]>,
    /// 101 slots, one per NG key.
    pub ng_keys: Vec<Option<Vec<u8>>>,
    /// 272 slots (17 rounds × 16 tables).
    pub ng_tables: Vec<Option<Vec<u8>>>,
}

impl ExeScan {
    pub fn ng_keys_found(&self) -> usize {
        self.ng_keys.iter().filter(|k| k.is_some()).count()
    }

    pub fn ng_tables_found(&self) -> usize {
        self.ng_tables.iter().filter(|k| k.is_some()).count()
    }

    pub fn has_complete_ng(&self) -> bool {
        self.ng_keys_found() == self.ng_keys.len() && self.ng_tables_found() == self.ng_tables.len()
    }
}

type HashIndex = HashMap<[u8; 20], Vec<Slot>>;

#[derive(Clone, Copy)]
enum Slot {
    Aes,
    Lut,
    NgKey(usize),
    NgTable(usize),
}

/// Scans `exe` for all known crypto material. Runs in parallel over 1 MiB chunks.
pub fn scan_executable(exe: &[u8]) -> ExeScan {
    let mut by_len: HashMap<usize, HashIndex> = HashMap::new();
    let mut add = |len: usize, hash: [u8; 20], slot: Slot| {
        by_len.entry(len).or_default().entry(hash).or_default().push(slot);
    };
    add(LEN_AES, key_hashes::AES_KEY, Slot::Aes);
    add(LEN_LUT, key_hashes::HASH_LUT, Slot::Lut);
    for (i, h) in key_hashes::NG_KEYS.iter().enumerate() {
        add(LEN_NG_KEY, *h, Slot::NgKey(i));
    }
    for (i, h) in key_hashes::NG_DECRYPT_TABLES.iter().enumerate() {
        add(LEN_NG_TABLE, *h, Slot::NgTable(i));
    }
    let lengths: Vec<(usize, &HashIndex)> = by_len.iter().map(|(l, m)| (*l, m)).collect();

    let result = Mutex::new(ExeScan {
        ng_keys: vec![None; key_hashes::NG_KEYS.len()],
        ng_tables: vec![None; key_hashes::NG_DECRYPT_TABLES.len()],
        ..Default::default()
    });

    (0..exe.len().div_ceil(CHUNK)).into_par_iter().for_each(|chunk| {
        let start = chunk * CHUNK;
        let end = (start + CHUNK).min(exe.len());
        for pos in (start..end).step_by(ALIGN) {
            for &(len, wanted) in &lengths {
                let Some(window) = exe.get(pos..pos + len) else { continue };
                let digest: [u8; 20] = Sha1::digest(window).into();
                if let Some(slots) = wanted.get(&digest) {
                    let mut r = result.lock().expect("scan mutex");
                    for slot in slots {
                        match *slot {
                            Slot::Aes => r.aes_key = window.try_into().ok(),
                            Slot::Lut => r.hash_lut = window.try_into().ok(),
                            Slot::NgKey(i) => r.ng_keys[i] = Some(window.to_vec()),
                            Slot::NgTable(i) => r.ng_tables[i] = Some(window.to_vec()),
                        }
                    }
                }
            }
        }
    });

    result.into_inner().expect("scan mutex")
}
