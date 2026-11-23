//! Experimental, read-only support for the RPF9 variant observed on Scarlett.
//!
//! Reads the two-stage encrypted metadata and lists entries by their stored
//! hash. Decoded export supports selective TFIT-CTR and verified block codecs.
//! Raw export preserves encryption, compression and padding. Candidate path
//! lists can resolve name hashes; trailer decoding and writing are unsupported.

#[path = "rpf9/archive.rs"]
mod archive;
#[path = "rpf9/ctr.rs"]
mod ctr;
#[path = "rpf9/names.rs"]
mod names;
pub use archive::{Archive, ArchiveEntry, Block, CryptoDescriptor, Extent, Metadata};
pub use ctr::TfitCtr;
pub use names::{NameIndex, name_hash};

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

pub const MAGIC: u32 = 0x5250_4639;
pub const HEADER_SIZE: usize = 0x40;
pub const FORMAT_CONSTANT: u32 = 0x0004_7A00;
pub const REGION_CONSTANT: u32 = 0x0000_0150;
pub const ALIGNMENT: u32 = 0x10;
pub const KEY_BLOB_MAGIC: u32 = 0x4E3B_9B8E;
pub const CTR_KEY_BLOB_MAGIC: u32 = 0x2718_CE5D;
pub const KEY_BLOB_SIZE: usize = 0x10F4;
pub const FIXED_TABLES_SIZE: usize = 0x5000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub metadata_end: u32,
    pub region_b_offset: u32,
    pub trailer_size: u32,
    pub archive_id: u64,
    pub format_constant: u32,
    pub region_constant: u32,
    pub field_20_low: u16,
    pub region_b_mode: u16,
    pub entry_count: u32,
    pub profile_tag: u32,
    pub index_width: u8,
    pub byte_2d: u8,
    pub crypto_profile: u8,
    pub byte_2f: u8,
    /// Historical field name: encrypted property suffix length at +0x30.
    pub alignment: u32,
    pub reserved: [u8; 12],
}

impl Header {
    pub fn parse(bytes: &[u8]) -> Result<Self, Rpf9Error> {
        if bytes.len() < HEADER_SIZE {
            return Err(Rpf9Error::TruncatedHeader(bytes.len()));
        }
        let magic = u32_at(bytes, 0).expect("header length checked");
        if magic != MAGIC {
            return Err(Rpf9Error::BadMagic(magic));
        }
        let packed_20 = u32_at(bytes, 0x20).expect("header length checked");
        let packed_2c = u32_at(bytes, 0x2C).expect("header length checked");
        Ok(Self {
            metadata_end: u32_at(bytes, 0x04).expect("header length checked"),
            region_b_offset: u32_at(bytes, 0x08).expect("header length checked"),
            trailer_size: u32_at(bytes, 0x0C).expect("header length checked"),
            archive_id: u64_at(bytes, 0x10).expect("header length checked"),
            format_constant: u32_at(bytes, 0x18).expect("header length checked"),
            region_constant: u32_at(bytes, 0x1C).expect("header length checked"),
            field_20_low: packed_20 as u16,
            region_b_mode: (packed_20 >> 16) as u16,
            entry_count: u32_at(bytes, 0x24).expect("header length checked"),
            profile_tag: u32_at(bytes, 0x28).expect("header length checked"),
            index_width: packed_2c as u8,
            byte_2d: (packed_2c >> 8) as u8,
            crypto_profile: (packed_2c >> 16) as u8,
            byte_2f: (packed_2c >> 24) as u8,
            alignment: u32_at(bytes, 0x30).expect("header length checked"),
            reserved: bytes[0x34..0x40].try_into().expect("header length checked"),
        })
    }

    pub fn region_a_size(self) -> Option<u32> {
        self.region_b_offset.checked_sub(HEADER_SIZE as u32)
    }

    pub fn region_b_size(self) -> Option<u32> {
        self.metadata_end.checked_sub(self.region_b_offset)
    }

    pub fn predicted_region_b_size(self) -> Option<u32> {
        let raw = self.entry_count.checked_mul(8 + u32::from(self.index_width))?;
        raw.checked_add(ALIGNMENT - 1).map(|n| n & !(ALIGNMENT - 1))
    }

    /// The first encrypted span is `align16(u16(header + 0x1a) * 8)`.
    pub fn first_metadata_span_size(self) -> usize {
        let records = (self.format_constant >> 16) as u16;
        align_up(u32::from(records) * 8, ALIGNMENT) as usize
    }

    pub fn validation_errors(self, file_size: u64) -> Vec<String> {
        let mut errors = Vec::new();
        if self.format_constant & 0xffff != FORMAT_CONSTANT & 0xffff
            || !(1..=64).contains(&(self.format_constant >> 16))
        {
            errors.push("unsupported property directory signature/count".to_owned());
        }
        // +0x30 is an encrypted property suffix size, not an alignment.
        if self.alignment > self.region_constant
            || !self.alignment.is_multiple_of(16)
            || !self.region_constant.is_multiple_of(16)
        {
            errors.push("invalid encrypted property-area sizes".to_owned());
        }
        if self.reserved.iter().any(|&b| b != 0) {
            errors.push("reserved bytes +0x34..+0x3f are non-zero".to_owned());
        }
        if !(HEADER_SIZE as u32 <= self.region_b_offset
            && self.region_b_offset <= self.metadata_end
            && u64::from(self.metadata_end) <= file_size)
        {
            errors.push("expected 0x40 <= region_b_offset <= metadata_end <= file size".to_owned());
        }
        if !self.metadata_end.is_multiple_of(ALIGNMENT)
            || !self.region_b_offset.is_multiple_of(ALIGNMENT)
            || !self.trailer_size.is_multiple_of(ALIGNMENT)
        {
            errors.push("metadata offsets or trailer size are not 16-byte aligned".to_owned());
        }
        if u64::from(self.trailer_size) > file_size.saturating_sub(u64::from(self.metadata_end)) {
            errors.push("trailer overlaps the metadata region".to_owned());
        }
        if !matches!(self.index_width, 1 | 2 | 4) {
            errors.push(format!("unexpected index width {}", self.index_width));
        }
        let expected_width = if self.entry_count <= 255 {
            1
        } else if self.entry_count <= 65535 {
            2
        } else {
            4
        };
        if self.index_width != expected_width {
            errors
                .push(format!("index width {} disagrees with count-derived width {expected_width}", self.index_width));
        }
        if u64::from(self.region_b_offset) < (HEADER_SIZE + self.first_metadata_span_size()) as u64 {
            errors.push("first metadata span exceeds region A".to_owned());
        }
        if self.region_b_mode == 1 && self.region_b_size() != self.predicted_region_b_size() {
            errors.push(format!(
                "compact region B has size {:?}, predicted {:?}",
                self.region_b_size(),
                self.predicted_region_b_size()
            ));
        }
        errors
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Rpf9Error {
    #[error("RPF9 header is truncated ({0} bytes, expected at least {HEADER_SIZE})")]
    TruncatedHeader(usize),
    #[error("not an RPF9 archive (magic {0:#010x})")]
    BadMagic(u32),
    #[error("invalid RPF9 header: {0}")]
    InvalidHeader(String),
    #[error("invalid RPF9 TFIT material: {0}")]
    InvalidMaterial(String),
    #[error("TFIT-CBC input length {0} is not a multiple of 16")]
    InvalidCiphertextLength(usize),
    #[error("unsupported RPF9 feature: {0}")]
    Unsupported(String),
    #[error("invalid RPF9 metadata: {0}")]
    InvalidMetadata(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Clone)]
pub struct CryptoMaterial {
    key_blob: Vec<u8>,
    fixed_tables: Vec<u8>,
    iv: [u8; 16],
}

impl CryptoMaterial {
    pub(crate) fn from_parts(key_blob: Vec<u8>, fixed_tables: Vec<u8>, iv: [u8; 16]) -> Result<Self, Rpf9Error> {
        Self::from_parts_with_magic(key_blob, fixed_tables, iv, KEY_BLOB_MAGIC)
    }

    pub(crate) fn from_parts_with_magic(
        key_blob: Vec<u8>,
        fixed_tables: Vec<u8>,
        iv: [u8; 16],
        magic: u32,
    ) -> Result<Self, Rpf9Error> {
        if key_blob.len() != KEY_BLOB_SIZE {
            return Err(Rpf9Error::InvalidMaterial(format!(
                "key blob is {:#x} bytes, expected {KEY_BLOB_SIZE:#x}",
                key_blob.len()
            )));
        }
        if u32_at(&key_blob, 0) != Some(magic) {
            return Err(Rpf9Error::InvalidMaterial("bad key-blob magic".to_owned()));
        }
        if fixed_tables.len() != FIXED_TABLES_SIZE {
            return Err(Rpf9Error::InvalidMaterial(format!(
                "fixed tables are {:#x} bytes, expected {FIXED_TABLES_SIZE:#x}",
                fixed_tables.len()
            )));
        }
        Ok(Self { key_blob, fixed_tables, iv })
    }

    pub fn decryptor(self) -> TfitDecryptor {
        TfitDecryptor { material: self }
    }
}

pub struct TfitDecryptor {
    material: CryptoMaterial,
}

impl TfitDecryptor {
    const FINAL_PERMUTATION: [usize; 16] = [0, 13, 10, 7, 4, 1, 14, 11, 8, 5, 2, 15, 12, 9, 6, 3];

    pub fn decrypt_block(&self, block: [u8; 16]) -> [u8; 16] {
        let key = |i: usize| u32_at(&self.material.key_blob, 4 + i * 4).expect("validated key blob");
        let initial = &self.material.fixed_tables[..0x1000];
        let table = |table: usize, byte: u8| {
            u32_at(&self.material.fixed_tables, 0x1000 + table * 0x400 + usize::from(byte) * 4)
                .expect("validated fixed tables")
        };
        let mut words = [0u32; 4];
        for (group, word) in words.iter_mut().enumerate() {
            let mut value = 0u32;
            for index in 0..4 {
                let position = group * 4 + index;
                value = (value << 8) ^ u32::from(initial[position * 256 + usize::from(block[position])]);
            }
            *word = value ^ key(group);
        }

        for round in 1..14 {
            let data: [u8; 16] = std::array::from_fn(|i| words[i / 4].to_be_bytes()[i % 4]);
            let at = round * 4;
            words = [
                table(0, data[0]) ^ table(7, data[7]) ^ table(10, data[10]) ^ table(13, data[13]) ^ key(at),
                table(1, data[1]) ^ table(4, data[4]) ^ table(11, data[11]) ^ table(14, data[14]) ^ key(at + 1),
                table(2, data[2]) ^ table(5, data[5]) ^ table(8, data[8]) ^ table(15, data[15]) ^ key(at + 2),
                table(3, data[3]) ^ table(6, data[6]) ^ table(9, data[9]) ^ table(12, data[12]) ^ key(at + 3),
            ];
        }

        let data: [u8; 16] = std::array::from_fn(|i| words[i / 4].to_be_bytes()[i % 4]);
        let final_tables = &self.material.key_blob[0xF4..];
        std::array::from_fn(|position| {
            let source = Self::FINAL_PERMUTATION[position];
            final_tables[position * 256 + usize::from(data[source])]
        })
    }

    pub fn decrypt_cbc(&self, ciphertext: &[u8]) -> Result<Vec<u8>, Rpf9Error> {
        self.decrypt_cbc_with_iv(ciphertext, self.material.iv)
    }

    pub fn decrypt_cbc_with_iv(&self, ciphertext: &[u8], iv: [u8; 16]) -> Result<Vec<u8>, Rpf9Error> {
        if !ciphertext.len().is_multiple_of(16) {
            return Err(Rpf9Error::InvalidCiphertextLength(ciphertext.len()));
        }
        let mut previous = iv;
        let mut output = Vec::with_capacity(ciphertext.len());
        for chunk in ciphertext.as_chunks::<16>().0 {
            let current = *chunk;
            let decoded = self.decrypt_block(current);
            output.extend(decoded.iter().zip(previous).map(|(a, b)| a ^ b));
            previous = current;
        }
        Ok(output)
    }
}

pub fn read_header(path: &Path) -> Result<(Header, u64), Rpf9Error> {
    let mut file = File::open(path)?;
    let file_size = file.metadata()?.len();
    let mut bytes = Vec::with_capacity(HEADER_SIZE);
    file.by_ref().take(HEADER_SIZE as u64).read_to_end(&mut bytes)?;
    let header = Header::parse(&bytes)?;
    let errors = header.validation_errors(file_size);
    if !errors.is_empty() {
        return Err(Rpf9Error::InvalidHeader(errors.join("; ")));
    }
    Ok((header, file_size))
}

pub fn decrypt_first_metadata_span(path: &Path, cipher: &TfitDecryptor) -> Result<Vec<u8>, Rpf9Error> {
    let (header, _) = read_header(path)?;
    if header.crypto_profile != 9 {
        return Err(Rpf9Error::Unsupported("first-span crypto profile".into()));
    }
    let span_size = header.first_metadata_span_size();
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(HEADER_SIZE as u64))?;
    let mut encrypted = vec![0u8; span_size];
    file.read_exact(&mut encrypted)?;
    cipher.decrypt_cbc(&encrypted)
}

fn align_up(value: u32, alignment: u32) -> u32 {
    value.div_ceil(alignment) * alignment
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

fn u64_at(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(at..at.checked_add(8)?)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> [u8; HEADER_SIZE] {
        let mut b = [0u8; HEADER_SIZE];
        b[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        b[4..8].copy_from_slice(&0x110u32.to_le_bytes());
        b[8..12].copy_from_slice(&0xF0u32.to_le_bytes());
        b[12..16].copy_from_slice(&0x40u32.to_le_bytes());
        b[16..24].copy_from_slice(&0x1122_3344_5566_7788u64.to_le_bytes());
        b[24..28].copy_from_slice(&FORMAT_CONSTANT.to_le_bytes());
        b[28..32].copy_from_slice(&REGION_CONSTANT.to_le_bytes());
        b[32..36].copy_from_slice(&0x0001_1234u32.to_le_bytes());
        b[36..40].copy_from_slice(&3u32.to_le_bytes());
        b[40..44].copy_from_slice(&0xAABB_CCDDu32.to_le_bytes());
        b[44..48].copy_from_slice(&0x0009_0001u32.to_le_bytes());
        b[48..52].copy_from_slice(&ALIGNMENT.to_le_bytes());
        b
    }

    #[test]
    fn final_round_uses_inverse_row_permutation() {
        // Synthetic tables: the final state is 00..0f, with identity final
        // substitutions. This detects the previous forward-row permutation
        // without embedding any executable or archive material.
        let mut key = vec![0; KEY_BLOB_SIZE];
        key[..4].copy_from_slice(&KEY_BLOB_MAGIC.to_le_bytes());
        for group in 0..4 {
            let word = u32::from_be_bytes(std::array::from_fn(|i| (group * 4 + i) as u8));
            key[4 + (52 + group) * 4..8 + (52 + group) * 4].copy_from_slice(&word.to_le_bytes());
        }
        for table in 0..16 {
            for byte in 0..256 {
                key[0xf4 + table * 256 + byte] = byte as u8;
            }
        }
        let cipher = CryptoMaterial::from_parts(key, vec![0; FIXED_TABLES_SIZE], [0; 16]).unwrap().decryptor();
        assert_eq!(cipher.decrypt_block([0; 16]), [0, 13, 10, 7, 4, 1, 14, 11, 8, 5, 2, 15, 12, 9, 6, 3]);
        assert!(cipher.decrypt_cbc(&[0; 15]).is_err());
        let result = cipher.decrypt_cbc(&[0x80; 32]).unwrap();
        for i in 0..16 {
            assert_eq!(result[16 + i], result[i] ^ 0x80);
        }
    }

    #[test]
    fn predicted_size_cannot_overflow_on_alignment() {
        let mut h = Header::parse(&header()).unwrap();
        h.entry_count = u32::MAX / 9;
        assert_eq!(h.predicted_region_b_size(), None);
    }

    #[test]
    fn parses_and_validates_header() {
        let bytes = header();
        let h = Header::parse(&bytes).unwrap();
        assert_eq!(h.archive_id, 0x1122_3344_5566_7788);
        assert_eq!(h.region_b_mode, 1);
        assert_eq!(h.entry_count, 3);
        assert_eq!(h.index_width, 1);
        assert_eq!(h.crypto_profile, 9);
        assert_eq!(h.first_metadata_span_size(), 0x20);
        assert_eq!(h.region_b_size(), Some(0x20));
        assert_eq!(h.predicted_region_b_size(), Some(0x20));
        assert!(h.validation_errors(0x200).is_empty());
    }

    #[test]
    fn rejects_short_and_other_formats() {
        assert!(matches!(Header::parse(&[]), Err(Rpf9Error::TruncatedHeader(0))));
        let mut bytes = header();
        bytes[..4].copy_from_slice(b"NOPE");
        assert!(matches!(Header::parse(&bytes), Err(Rpf9Error::BadMagic(_))));
    }
}

#[path = "rpf9/recovery.rs"]
mod recovery;
#[path = "rpf9/toc_view.rs"]
mod toc_view;
pub use recovery::{
    Entry, HEADER_LEN, Indexer, Kind, Namer, Names, Rpf9, decode_index, encode_index, set_indexer, set_namer,
};
