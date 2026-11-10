//! Reads CodeWalker's obfuscated key blob (`magic.dat`).
//!
//! The blob is never shipped with OpenRAGE. Users of GTA V Enhanced, whose
//! executable no longer contains the NG keys in plain form, can point OpenRAGE
//! at a copy they obtained themselves. It can only be decoded with the AES key
//! from the user's own game executable.
//!
//! Layout after de-obfuscation: AES-256-ECB encrypted raw-deflate stream which
//! inflates to `ng_keys (101×272) | ng_tables (17×16×256 u32) | hash_lut (256) | awc_key (16)`.

use std::io::Read;

use flate2::read::DeflateDecoder;
use rage_core::hash::jenkins_bytes;

use crate::aes_ecb::decrypt_aes;
use crate::dotnet_random::DotNetRandom;

pub const NG_KEYS_LEN: usize = 101 * 272;
pub const NG_TABLES_LEN: usize = 17 * 16 * 256 * 4;
pub const LUT_LEN: usize = 256;
pub const AWC_LEN: usize = 16;

#[derive(Debug, thiserror::Error)]
pub enum MagicError {
    #[error("key blob could not be decoded with this AES key (wrong file or wrong game version)")]
    Inflate(#[source] std::io::Error),
    #[error("key blob decoded to {0} bytes, expected at least {1}")]
    TooShort(usize, usize),
}

/// Decoded contents of a CodeWalker key blob.
pub struct MagicContents {
    pub ng_keys: Vec<u8>,
    pub ng_tables: Vec<u8>,
    pub hash_lut: [u8; 256],
    pub awc_key: [u8; 16],
}

pub fn decode_codewalker_magic(blob: &[u8], aes_key: &[u8; 32]) -> Result<MagicContents, MagicError> {
    let mut rng = DotNetRandom::new(jenkins_bytes(aes_key) as i32);
    let mut streams = vec![vec![0u8; blob.len()]; 4];
    for s in &mut streams {
        rng.next_bytes(s);
    }
    let mut data: Vec<u8> = blob
        .iter()
        .enumerate()
        .map(|(i, &m)| {
            m.wrapping_sub(streams[0][i])
                .wrapping_sub(streams[1][i])
                .wrapping_sub(streams[2][i])
                .wrapping_sub(streams[3][i])
        })
        .collect();
    decrypt_aes(&mut data, aes_key);

    let mut out = Vec::new();
    DeflateDecoder::new(&data[..]).read_to_end(&mut out).map_err(MagicError::Inflate)?;

    let need = NG_KEYS_LEN + NG_TABLES_LEN + LUT_LEN + AWC_LEN;
    if out.len() < need {
        return Err(MagicError::TooShort(out.len(), need));
    }
    let (ng_keys, rest) = out.split_at(NG_KEYS_LEN);
    let (ng_tables, rest) = rest.split_at(NG_TABLES_LEN);
    let (lut, rest) = rest.split_at(LUT_LEN);
    Ok(MagicContents {
        ng_keys: ng_keys.to_vec(),
        ng_tables: ng_tables.to_vec(),
        hash_lut: lut.try_into().expect("lut length"),
        awc_key: rest[..AWC_LEN].try_into().expect("awc length"),
    })
}
