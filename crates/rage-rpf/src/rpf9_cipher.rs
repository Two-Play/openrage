//! The Scarlett build's table-driven 16-byte block transform.
//!
//! Reconstructed from the instructions at VA 0x145642c50 of the executable
//! identified below. Tables and key schedules are read from the user's file;
//! none are bundled. This module supports only that exact executable build.

// Not yet wired into the archive reader; integration follows.
#![allow(dead_code, clippy::items_after_test_module)]

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

const EXE_SHA256: [u8; 32] = [
    0xe9, 0x50, 0x20, 0xb2, 0xfa, 0x4a, 0x72, 0x1d, 0xb3, 0xdf, 0x59, 0xfd, 0xec, 0xf7, 0x56, 0x21, 0x73, 0x0a, 0xc5,
    0x59, 0xd8, 0x83, 0x52, 0x52, 0xf6, 0xc3, 0xb6, 0x58, 0xd5, 0xa0, 0x89, 0x93,
];
const INITIAL_VA: u64 = 0x146036f30;
const MIDDLE_VA: u64 = 0x146037f30;
const KEY_TABLE_VA: u64 = 0x1466e5900;
const KEY_RECORDS: usize = 24;
const KEY_BLOB_LEN: usize = 0x10f4;

struct Key {
    words: [u32; 60],
    end: Vec<u8>,
}

/// Cipher material extracted from a supported local executable.
pub(crate) struct Cipher {
    initial: Vec<u8>,
    middle: Vec<u32>,
    keys: BTreeMap<u32, Key>,
    header_iv: [u8; 16],
    ctr_initial: Vec<u8>,
    ctr_middle: Vec<u32>,
    ctr_keys: BTreeMap<u32, Key>,
}

impl Cipher {
    /// Returns `None` for an unknown build or inconsistent PE/key layout.
    pub(crate) fn from_exe(exe: &[u8]) -> Option<Self> {
        if Sha256::digest(exe).as_slice() != EXE_SHA256 {
            return None;
        }
        let initial = pe_bytes(exe, INITIAL_VA, 16 * 256)?.to_vec();
        let header_iv = pe_bytes(exe, 0x1462fd470, 16)?.try_into().ok()?;
        let ctr_initial = pe_bytes(exe, 0x146040f30, 16 * 256)?.to_vec();
        let ctr_middle = pe_bytes(exe, 0x146041f30, 16 * 256 * 4)?
            .as_chunks::<4>()
            .0
            .iter()
            .map(|word| u32::from_le_bytes(*word))
            .collect();
        let middle = pe_bytes(exe, MIDDLE_VA, 16 * 256 * 4)?
            .as_chunks::<4>()
            .0
            .iter()
            .map(|word| u32::from_le_bytes(*word))
            .collect();
        let mut keys = BTreeMap::new();
        let mut ctr_keys = BTreeMap::new();
        for record in pe_bytes(exe, KEY_TABLE_VA, KEY_RECORDS * 24)?.as_chunks::<24>().0 {
            let tag = u64::from_le_bytes(record[..8].try_into().ok()?);
            // The native key lookup uses class 2 for decrypt state 1.
            if tag as u32 != 0x02000000 {
                continue;
            }
            let address = u64::from_le_bytes(record[8..16].try_into().ok()?);
            let len = u64::from_le_bytes(record[16..24].try_into().ok()?);
            if len != KEY_BLOB_LEN as u64 {
                return None;
            }
            let blob = pe_bytes(exe, address, KEY_BLOB_LEN)?;
            let magic = u32::from_le_bytes(blob[..4].try_into().ok()?);
            if !matches!(magic, 0x4e3b9b8e | 0x2718ce5d) {
                // Other algorithms share this key registry. Their serialized
                // blobs have different magics and must not be treated as this
                // transform's context.
                continue;
            }
            let words = std::array::from_fn(|i| {
                u32::from_le_bytes(blob[4 + i * 4..8 + i * 4].try_into().expect("bounded word"))
            });
            let target = if magic == 0x4e3b9b8e { &mut keys } else { &mut ctr_keys };
            target.insert((tag >> 32) as u32, Key { words, end: blob[0xf4..].to_vec() });
        }
        Some(Self { initial, middle, keys, header_iv, ctr_initial, ctr_middle, ctr_keys })
    }

    pub(crate) fn header_iv(&self) -> [u8; 16] {
        self.header_iv
    }

    pub(crate) fn has_key(&self, id: u32) -> bool {
        self.keys.contains_key(&id)
    }

    /// The pure block transform, without a chaining mode or padding.
    pub(crate) fn decrypt_block(&self, id: u32, input: &[u8; 16]) -> Option<[u8; 16]> {
        let key = self.keys.get(&id)?;
        let mut state = [0u32; 4];
        for (column, word) in state.iter_mut().enumerate() {
            for byte in 0..4 {
                let i = column * 4 + byte;
                *word = (*word << 8) ^ u32::from(self.initial[i * 256 + usize::from(input[i])]);
            }
            *word ^= key.words[column];
        }
        let t = |table: usize, word: u32, shift: u32| self.middle[table * 256 + ((word >> shift) & 255) as usize];
        // The native loop processes pairs of rounds, then exits after the
        // first half of its seventh iteration (13 rounds in total).
        for round in 1..=13 {
            let [a, b, c, d] = state;
            state = [
                t(0, a, 24) ^ t(7, b, 0) ^ t(10, c, 8) ^ t(13, d, 16) ^ key.words[round * 4],
                t(4, b, 24) ^ t(11, c, 0) ^ t(1, a, 16) ^ t(14, d, 8) ^ key.words[round * 4 + 1],
                t(8, c, 24) ^ t(15, d, 0) ^ t(2, a, 8) ^ t(5, b, 16) ^ key.words[round * 4 + 2],
                t(12, d, 24) ^ t(3, a, 0) ^ t(6, b, 8) ^ t(9, c, 16) ^ key.words[round * 4 + 3],
            ];
        }
        let [a, b, c, d] = state;
        let selected = [
            a >> 24,
            d >> 16,
            c >> 8,
            b,
            b >> 24,
            a >> 16,
            d >> 8,
            c,
            c >> 24,
            b >> 16,
            a >> 8,
            d,
            d >> 24,
            c >> 16,
            b >> 8,
            a,
        ];
        Some(std::array::from_fn(|i| key.end[i * 256 + (selected[i] & 255) as usize]))
    }

    /// Decrypts complete CBC blocks and updates the caller-owned chaining
    /// state, permitting successive metadata spans to share a chain.
    pub(crate) fn decrypt_cbc(&self, id: u32, bytes: &mut [u8], iv: &mut [u8; 16]) -> Option<()> {
        if !bytes.len().is_multiple_of(16) || !self.has_key(id) {
            return None;
        }
        for block in bytes.as_chunks_mut::<16>().0 {
            let stored = *block;
            let plain = self.decrypt_block(id, &stored)?;
            for i in 0..16 {
                block[i] = plain[i] ^ iv[i];
            }
            *iv = stored;
        }
        Some(())
    }

    /// The counter-mode keystream primitive at VA `0x1456437c0`. This is a
    /// separate transform from the CBC block primitive, with its own tables.
    pub(crate) fn counter_block(&self, id: u32, input: &[u8; 16]) -> Option<[u8; 16]> {
        let key = self.ctr_keys.get(&id)?;
        let mut state = [0u32; 4];
        for (column, word) in state.iter_mut().enumerate() {
            for byte in 0..4 {
                let i = column * 4 + byte;
                *word = (*word << 8) ^ u32::from(self.ctr_initial[i * 256 + usize::from(input[i])]);
            }
            *word ^= key.words[column];
        }
        let t = |table: usize, word: u32, shift: u32| self.ctr_middle[table * 256 + ((word >> shift) & 255) as usize];
        for round in 1..=13 {
            let [a, b, c, d] = state;
            state = [
                t(0, a, 24) ^ t(5, b, 16) ^ t(10, c, 8) ^ t(15, d, 0) ^ key.words[round * 4],
                t(4, b, 24) ^ t(9, c, 16) ^ t(14, d, 8) ^ t(3, a, 0) ^ key.words[round * 4 + 1],
                t(8, c, 24) ^ t(13, d, 16) ^ t(2, a, 8) ^ t(7, b, 0) ^ key.words[round * 4 + 2],
                t(12, d, 24) ^ t(1, a, 16) ^ t(6, b, 8) ^ t(11, c, 0) ^ key.words[round * 4 + 3],
            ];
        }
        let [a, b, c, d] = state;
        let selected = [
            a >> 24,
            b >> 16,
            c >> 8,
            d,
            b >> 24,
            c >> 16,
            d >> 8,
            a,
            c >> 24,
            d >> 16,
            a >> 8,
            b,
            d >> 24,
            a >> 16,
            b >> 8,
            c,
        ];
        Some(std::array::from_fn(|i| key.end[i * 256 + (selected[i] & 255) as usize]))
    }

    /// XORs bytes with algorithm 17's stream at `offset` bytes from the
    /// beginning of an encrypted member. The native implementation increments
    /// the full 16-byte counter in big-endian order after using each block.
    /// `iv` is the descriptor's initial counter and is never modified.
    pub(crate) fn crypt_ctr(&self, id: u32, bytes: &mut [u8], iv: &[u8; 16], offset: u64) -> Option<()> {
        if !self.ctr_keys.contains_key(&id) {
            return None;
        }
        let mut counter = u128::from_be_bytes(*iv).wrapping_add(u128::from(offset / 16));
        let mut skip = (offset % 16) as usize;
        let mut at = 0;
        while at < bytes.len() {
            let stream = self.counter_block(id, &counter.to_be_bytes())?;
            let take = (16 - skip).min(bytes.len() - at);
            for i in 0..take {
                bytes[at + i] ^= stream[skip + i];
            }
            at += take;
            skip = 0;
            counter = counter.wrapping_add(1);
        }
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Zero tables produce a constant zero block. This isolates the CBC
    // chaining contract from the reconstructed block cipher and game data.
    fn constant_cipher() -> Cipher {
        let key = Key { words: [0; 60], end: vec![0; 16 * 256] };
        Cipher {
            initial: vec![0; 16 * 256],
            middle: vec![0; 16 * 256],
            keys: BTreeMap::from([(7, key)]),
            header_iv: [0x12; 16],
            ctr_initial: vec![0; 16 * 256],
            ctr_middle: vec![0; 16 * 256],
            ctr_keys: BTreeMap::new(),
        }
    }

    #[test]
    fn cbc_uses_previous_ciphertext_and_carries_the_chain_between_spans() {
        let cipher = constant_cipher();
        let mut first = [0x34; 32];
        first[16..].fill(0x56);
        let mut chain = cipher.header_iv();
        cipher.decrypt_cbc(7, &mut first, &mut chain).unwrap();
        assert_eq!(&first[..16], &[0x12; 16]);
        assert_eq!(&first[16..], &[0x34; 16]);
        assert_eq!(chain, [0x56; 16]);
        let mut second = [0x78; 16];
        cipher.decrypt_cbc(7, &mut second, &mut chain).unwrap();
        assert_eq!(second, [0x56; 16]);
        assert_eq!(chain, [0x78; 16]);
    }

    #[test]
    fn cbc_rejects_unknown_keys_and_partial_blocks_before_mutating() {
        let cipher = constant_cipher();
        let mut chain = [0x12; 16];
        let mut bytes = [0x34; 17];
        assert!(cipher.decrypt_cbc(7, &mut bytes, &mut chain).is_none());
        assert_eq!(bytes, [0x34; 17]);
        assert!(cipher.decrypt_cbc(8, &mut bytes[..16], &mut chain).is_none());
        assert_eq!(bytes, [0x34; 17]);
        assert_eq!(chain, [0x12; 16]);
        assert!(Cipher::from_exe(b"an unknown executable build").is_none());
    }

    fn counter_cipher() -> Cipher {
        let mut cipher = constant_cipher();
        cipher.ctr_initial = (0..16 * 256).map(|i| i as u8).collect();
        cipher.ctr_middle =
            (0..16 * 256).map(|i| ((i % 256) as u32).wrapping_mul(0x9e3779b9).rotate_left((i / 256) as u32)).collect();
        cipher.ctr_keys.insert(
            9,
            Key {
                words: std::array::from_fn(|i| (i as u32).wrapping_mul(0x6a09e667)),
                end: (0..16 * 256).map(|i| i as u8).collect(),
            },
        );
        cipher
    }

    #[test]
    fn ctr_increments_the_counter_big_endian_and_wraps() {
        let cipher = counter_cipher();
        let mut iv = [0u8; 16];
        iv[15] = 255;
        let mut next = [0u8; 16];
        next[14] = 1;
        let mut bytes = [0u8; 32];
        cipher.crypt_ctr(9, &mut bytes, &iv, 0).unwrap();
        assert_eq!(&bytes[..16], &cipher.counter_block(9, &iv).unwrap());
        assert_eq!(&bytes[16..], &cipher.counter_block(9, &next).unwrap());
        let mut wrap = [0u8; 32];
        cipher.crypt_ctr(9, &mut wrap, &[255; 16], 0).unwrap();
        assert_eq!(&wrap[16..], &cipher.counter_block(9, &[0; 16]).unwrap());
    }

    #[test]
    fn ctr_reads_at_unaligned_offsets_match_whole_member_reads() {
        let cipher = counter_cipher();
        let iv = [0x12; 16];
        let plain: Vec<u8> = (0..97).collect();
        let mut whole = plain.clone();
        cipher.crypt_ctr(9, &mut whole, &iv, 0).unwrap();
        assert_ne!(&whole[..16], &whole[16..32]);
        for (start, end) in [(0, 3), (3, 21), (21, 32), (32, 81), (81, 97)] {
            let mut part = plain[start..end].to_vec();
            cipher.crypt_ctr(9, &mut part, &iv, start as u64).unwrap();
            assert_eq!(part, whole[start..end]);
        }
        cipher.crypt_ctr(9, &mut whole, &iv, 0).unwrap();
        assert_eq!(whole, plain);
        assert!(cipher.crypt_ctr(10, &mut whole, &iv, 0).is_none());
        assert_eq!(whole, plain);
    }
}

/// Converts an original-file PE address to a bounded source range. No
/// executable code is loaded or run by the parser.
fn pe_bytes(exe: &[u8], address: u64, len: usize) -> Option<&[u8]> {
    if exe.get(..2)? != b"MZ" {
        return None;
    }
    let pe = u32::from_le_bytes(exe.get(0x3c..0x40)?.try_into().ok()?) as usize;
    if exe.get(pe..pe.checked_add(4)?)? != b"PE\0\0" {
        return None;
    }
    let count = u16::from_le_bytes(exe.get(pe + 6..pe + 8)?.try_into().ok()?) as usize;
    let opt_len = u16::from_le_bytes(exe.get(pe + 20..pe + 22)?.try_into().ok()?) as usize;
    if count > 96 || opt_len < 32 || exe.get(pe + 24..pe + 26)? != [0x0b, 0x02] {
        return None;
    }
    let base = u64::from_le_bytes(exe.get(pe + 48..pe + 56)?.try_into().ok()?);
    let rva = address.checked_sub(base)?;
    let start = pe.checked_add(24)?.checked_add(opt_len)?;
    let sections = exe.get(start..start.checked_add(count.checked_mul(40)?)?)?;
    for section in sections.as_chunks::<40>().0 {
        let virtual_start = u32::from_le_bytes(section[12..16].try_into().ok()?) as u64;
        let stored_len = u32::from_le_bytes(section[16..20].try_into().ok()?) as u64;
        let stored_start = u32::from_le_bytes(section[20..24].try_into().ok()?) as u64;
        let Some(relative) = rva.checked_sub(virtual_start) else { continue };
        if relative.checked_add(len as u64)? > stored_len {
            continue;
        }
        let offset = usize::try_from(stored_start.checked_add(relative)?).ok()?;
        return exe.get(offset..offset.checked_add(len)?);
    }
    None
}
