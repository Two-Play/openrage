//! Extraction of the RPF9 TFIT material from the user's Scarlett executable.
//!
//! Only SHA-256 fingerprints and addresses for the analysed 1.0.2.1 build are
//! stored here. No key, IV, cipher table, executable byte, or archive byte is
//! distributed by OpenRAGE.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::rpf9::{CryptoMaterial, FIXED_TABLES_SIZE, KEY_BLOB_MAGIC, KEY_BLOB_SIZE, Rpf9Error};

pub const KEY_ID: u64 = 0x1000_0001_0200_0000;
pub const REGISTRY_VA: u64 = 0x1466_E5900;
pub const REGISTRY_COUNT: usize = 24;
pub const IV_VA: u64 = 0x1462_FD470;
pub const FIXED_TABLES_VA: u64 = 0x1460_36F30;

const KEY_SHA256: [u8; 32] = hex32("d7a6d8ddcba95fc4e5ce32ead7720435d80d8c4743f01cd50458defd38dd8db5");
const IV_SHA256: [u8; 32] = hex32("70ad3893679946abe55fa553f19856c1f81401bee02380885084a418c64ff2e3");
const TABLES_SHA256: [u8; 32] = hex32("39dff6172873bac1b93b0aa26d2e31146404f3f40c2261492c9c45b313f1e38e");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyDescriptor {
    pub key_id: u64,
    pub data_va: u64,
    pub size: u64,
}

pub struct ExtractedMaterial {
    /// Verified TFIT-CBC profiles used by the archive metadata.
    pub metadata_keys: std::collections::HashMap<u32, CryptoMaterial>,
    pub payload_keys: std::collections::HashMap<u32, CryptoMaterial>,
    pub descriptor: KeyDescriptor,
    pub image_base: u64,
    pub material: CryptoMaterial,
}

#[derive(Debug, thiserror::Error)]
pub enum ExeError {
    #[error("{0}: {1}")]
    File(String, #[source] std::io::Error),
    #[error("not a supported PE32+ executable: {0}")]
    Pe(&'static str),
    #[error("RPF9 key descriptor {KEY_ID:#018x} was found {0} times, expected once")]
    DescriptorCount(usize),
    #[error("RPF9 key descriptor has size {0:#x}, expected {KEY_BLOB_SIZE:#x}")]
    DescriptorSize(u64),
    #[error("RPF9 material for the analysed Scarlett 1.0.2.1 build failed its {0} fingerprint")]
    Fingerprint(&'static str),
    #[error("invalid extracted RPF9 material: {0}")]
    Material(#[from] Rpf9Error),
}

/// Reads only PE headers, the registry and the selected cipher blobs.
/// No whole-executable allocation, even when the supplied file is very large.
pub fn extract_scarlett_1_0_2_1(path: &Path) -> Result<ExtractedMaterial, ExeError> {
    let mut file = std::fs::File::open(path).map_err(|e| ExeError::File(path.display().to_string(), e))?;
    let mut read = |offset: u64, size: usize| -> Result<Vec<u8>, ExeError> {
        if size > 1 << 20 {
            return Err(ExeError::Pe("PE read exceeds 1 MiB limit"));
        }
        file.seek(SeekFrom::Start(offset)).map_err(|e| ExeError::File(path.display().to_string(), e))?;
        let mut data = vec![0; size];
        file.read_exact(&mut data).map_err(|e| ExeError::File(path.display().to_string(), e))?;
        Ok(data)
    };
    let dos = read(0, 64)?;
    if dos.get(..2) != Some(b"MZ") {
        return Err(ExeError::Pe("missing MZ signature"));
    }
    let pe = u32_at(&dos, 0x3c).ok_or(ExeError::Pe("missing PE offset"))? as usize;
    if pe > 1 << 20 {
        return Err(ExeError::Pe("PE header offset exceeds reader limit"));
    }
    let coff = read(pe as u64, 24)?;
    let sections = u16_at(&coff, 6).ok_or(ExeError::Pe("truncated COFF header"))? as usize;
    let optional = u16_at(&coff, 20).ok_or(ExeError::Pe("truncated COFF header"))? as usize;
    let headers = read(0, pe + 24 + optional + sections * 40)?;
    let image = PeImage::parse(&headers)?;
    extract_with_reader(image.image_base, |va, size| read(image.file_offset(va, size)? as u64, size))
}

pub fn extract_scarlett_1_0_2_1_from_bytes(exe: &[u8]) -> Result<ExtractedMaterial, ExeError> {
    let image = PeImage::parse(exe)?;
    extract_with_reader(image.image_base, |va, size| Ok(image.read_va(va, size)?.to_vec()))
}

fn extract_with_reader(
    image_base: u64,
    mut read_va: impl FnMut(u64, usize) -> Result<Vec<u8>, ExeError>,
) -> Result<ExtractedMaterial, ExeError> {
    let registry = read_va(REGISTRY_VA, REGISTRY_COUNT * 24)?;
    let descriptors: Vec<_> = registry
        .as_chunks::<24>()
        .0
        .iter()
        .map(|b| KeyDescriptor {
            key_id: u64_at(b, 0).expect("descriptor chunk"),
            data_va: u64_at(b, 8).expect("descriptor chunk"),
            size: u64_at(b, 16).expect("descriptor chunk"),
        })
        .filter(|d| d.key_id == KEY_ID)
        .collect();
    if descriptors.len() != 1 {
        return Err(ExeError::DescriptorCount(descriptors.len()));
    }
    let descriptor = descriptors[0];
    if descriptor.size != KEY_BLOB_SIZE as u64 {
        return Err(ExeError::DescriptorSize(descriptor.size));
    }
    let key_blob = read_va(descriptor.data_va, KEY_BLOB_SIZE)?.to_vec();
    let iv: [u8; 16] = read_va(IV_VA, 16)?.try_into().expect("fixed size");
    let fixed_tables = read_va(FIXED_TABLES_VA, FIXED_TABLES_SIZE)?.to_vec();
    check_fingerprint("key blob", &key_blob, KEY_SHA256)?;
    check_fingerprint("IV", &iv, IV_SHA256)?;
    check_fingerprint("fixed tables", &fixed_tables, TABLES_SHA256)?;
    if u32_at(&key_blob, 0) != Some(KEY_BLOB_MAGIC) {
        return Err(ExeError::Fingerprint("key-blob magic"));
    }
    let mut metadata_keys = std::collections::HashMap::new();
    for &(selector, fingerprint) in METADATA_KEYS {
        let id = (u64::from(selector) << 32) | 0x0200_0000;
        let matches: Vec<_> = registry.as_chunks::<24>().0.iter().filter(|b| u64_at(*b, 0) == Some(id)).collect();
        if matches.len() != 1 {
            return Err(ExeError::Pe("missing or duplicate metadata key descriptor"));
        }
        let record = matches[0];
        if u64_at(record, 16) != Some(KEY_BLOB_SIZE as u64) {
            return Err(ExeError::Pe("unexpected metadata key size"));
        }
        let bytes = read_va(u64_at(record, 8).expect("descriptor"), KEY_BLOB_SIZE)?;
        check_fingerprint("metadata key", &bytes, hex32(fingerprint))?;
        metadata_keys.insert(selector, CryptoMaterial::from_parts(bytes.to_vec(), fixed_tables.clone(), iv)?);
    }
    let ctr_tables = read_va(0x1460_40f30, FIXED_TABLES_SIZE)?;
    check_fingerprint(
        "CTR tables",
        &ctr_tables,
        hex32("2ea06f878701cc3d9364f9820da31c1030faaa82805aa258910a69dfac870eb1"),
    )?;
    let mut payload_keys = std::collections::HashMap::new();
    for &(selector, fingerprint) in PAYLOAD_KEYS {
        let id = (u64::from(selector) << 32) | 0x0200_0000;
        let matches: Vec<_> = registry.as_chunks::<24>().0.iter().filter(|b| u64_at(*b, 0) == Some(id)).collect();
        if matches.len() != 1 || u64_at(matches[0], 16) != Some(KEY_BLOB_SIZE as u64) {
            return Err(ExeError::Pe("missing, duplicate or invalid payload key descriptor"));
        }
        let bytes = read_va(u64_at(matches[0], 8).expect("descriptor"), KEY_BLOB_SIZE)?;
        check_fingerprint("payload key", &bytes, hex32(fingerprint))?;
        payload_keys.insert(
            selector,
            CryptoMaterial::from_parts_with_magic(bytes, ctr_tables.clone(), iv, crate::rpf9::CTR_KEY_BLOB_MAGIC)?,
        );
    }
    let material = CryptoMaterial::from_parts(key_blob, fixed_tables, iv)?;
    Ok(ExtractedMaterial { metadata_keys, payload_keys, descriptor, image_base, material })
}

fn check_fingerprint(label: &'static str, bytes: &[u8], expected: [u8; 32]) -> Result<(), ExeError> {
    if Sha256::digest(bytes).as_slice() == expected { Ok(()) } else { Err(ExeError::Fingerprint(label)) }
}

struct Section {
    virtual_address: u32,
    virtual_size: u32,
    raw_offset: u32,
    raw_size: u32,
}

struct PeImage<'a> {
    bytes: &'a [u8],
    image_base: u64,
    sections: Vec<Section>,
}

impl<'a> PeImage<'a> {
    fn parse(bytes: &'a [u8]) -> Result<Self, ExeError> {
        if bytes.get(..2) != Some(b"MZ") {
            return Err(ExeError::Pe("missing MZ signature"));
        }
        let pe = usize::try_from(u32_at(bytes, 0x3C).ok_or(ExeError::Pe("missing PE offset"))?)
            .map_err(|_| ExeError::Pe("PE offset does not fit usize"))?;
        if bytes.get(pe..pe.checked_add(4).ok_or(ExeError::Pe("PE offset overflow"))?) != Some(b"PE\0\0") {
            return Err(ExeError::Pe("missing PE signature"));
        }
        let coff = pe.checked_add(4).ok_or(ExeError::Pe("COFF offset overflow"))?;
        let section_count = usize::from(u16_at(bytes, coff + 2).ok_or(ExeError::Pe("truncated COFF header"))?);
        let optional_size = usize::from(u16_at(bytes, coff + 16).ok_or(ExeError::Pe("truncated COFF header"))?);
        let optional = coff.checked_add(20).ok_or(ExeError::Pe("optional-header offset overflow"))?;
        if u16_at(bytes, optional) != Some(0x20B) {
            return Err(ExeError::Pe("expected a PE32+ optional header"));
        }
        let image_base = u64_at(bytes, optional + 24).ok_or(ExeError::Pe("truncated optional header"))?;
        let section_table = optional.checked_add(optional_size).ok_or(ExeError::Pe("section-table offset overflow"))?;
        let mut sections = Vec::with_capacity(section_count);
        for i in 0..section_count {
            let at = section_table
                .checked_add(i.checked_mul(40).ok_or(ExeError::Pe("section-table overflow"))?)
                .ok_or(ExeError::Pe("section-table overflow"))?;
            sections.push(Section {
                virtual_size: u32_at(bytes, at + 8).ok_or(ExeError::Pe("truncated section table"))?,
                virtual_address: u32_at(bytes, at + 12).ok_or(ExeError::Pe("truncated section table"))?,
                raw_size: u32_at(bytes, at + 16).ok_or(ExeError::Pe("truncated section table"))?,
                raw_offset: u32_at(bytes, at + 20).ok_or(ExeError::Pe("truncated section table"))?,
            });
        }
        Ok(Self { bytes, image_base, sections })
    }

    fn read_va(&self, va: u64, size: usize) -> Result<&'a [u8], ExeError> {
        let offset = self.file_offset(va, size)?;
        self.bytes
            .get(offset..offset.checked_add(size).ok_or(ExeError::Pe("read size overflow"))?)
            .ok_or(ExeError::Pe("VA read lies outside the executable"))
    }

    fn file_offset(&self, va: u64, size: usize) -> Result<usize, ExeError> {
        let rva = va.checked_sub(self.image_base).ok_or(ExeError::Pe("VA precedes image base"))?;
        let section = self.sections.iter().find(|s| {
            let start = u64::from(s.virtual_address);
            let extent = u64::from(s.virtual_size.max(s.raw_size));
            rva >= start && rva < start + extent
        });
        let section = section.ok_or(ExeError::Pe("VA is outside mapped sections"))?;
        let delta = rva - u64::from(section.virtual_address);
        if delta.checked_add(size as u64).is_none_or(|end| end > u64::from(section.raw_size)) {
            return Err(ExeError::Pe("VA is not backed by file data"));
        }
        let offset = u64::from(section.raw_offset)
            .checked_add(delta)
            .and_then(|v| usize::try_from(v).ok())
            .ok_or(ExeError::Pe("file offset overflow"))?;
        Ok(offset)
    }
}

const fn hex32(s: &str) -> [u8; 32] {
    let bytes = s.as_bytes();
    let mut out = [0u8; 32];
    let mut i = 0;
    while i < 32 {
        out[i] = (nibble(bytes[i * 2]) << 4) | nibble(bytes[i * 2 + 1]);
        i += 1;
    }
    out
}

const fn nibble(b: u8) -> u8 {
    match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => b - b'a' + 10,
        b'A'..=b'F' => b - b'A' + 10,
        _ => panic!("invalid hex"),
    }
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at.checked_add(2)?)?.try_into().ok()?))
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

    #[test]
    fn hashes_are_well_formed() {
        assert_ne!(KEY_SHA256, [0; 32]);
        assert_ne!(IV_SHA256, [0; 32]);
        assert_ne!(TABLES_SHA256, [0; 32]);
    }

    #[test]
    fn rejects_non_pe_data() {
        assert!(matches!(extract_scarlett_1_0_2_1_from_bytes(b"not a PE"), Err(ExeError::Pe("missing MZ signature"))));
    }
}

// Checksums only; key bytes are read from the user's executable.
const METADATA_KEYS: &[(u32, &str)] = &[
    (0x10000002, "5a9d5a8e690281de42f5e222872b8b525f672aec264a852cb5aed6c346041c9b"),
    (0x10000015, "f347c5f0a64ab86831be42b635d2f5c6d4ab31d7f5d955be0715a8f24bc01ce4"),
    (0x10000016, "f1697f026bacd7671225b28b42d5fb637d6c2fdabefc806074da7bfc5e26be7c"),
    (0x10000017, "d2af4da01309575ad57f599f218f8164f7907fbae741017c229c8f4fea5af254"),
    (0x10000018, "4682b1c9f0b9ca7d1b78df6bd4cd266ac1f11ed3662b9669a34741d9320218a6"),
    (0x10000019, "e7f7c3f59902a69d053ac6e3df94ae69154c7bd5a7c56d727a68c7ce22237110"),
    (0x1000001a, "17f31e59cade68631c2f48a106368bb47ec68d1514a0896e93da10f2b3a7f842"),
];

const PAYLOAD_KEYS: &[(u32, &str)] = &[
    (0x10000029, "357060c1503827e82248d31e16a9edfb50461bbe0db4f9483de144d6e309c650"),
    (0x1000002a, "f489e861275845fdfd787b9b6a32c2eadf25f68c080bc5354497bfb16e31ccac"),
    (0x1000002b, "e4aee77720f07142dec9ffbd80b66a2fafb0b16e32beb98a7f750ffbdff5b61b"),
    (0x1000002c, "692b9097d9a5b9de1d2d8f65356fd0b21ca93c4d2451a0fb150e2deca9b77a9a"),
    (0x1000002d, "61a089e08b2e3abb3311e50e3f3d2dd1f5fe736b98627e6f8cb8867d4d1a5e1c"),
    (0x1000002e, "3ab565dcf75f3110cd8e2c90e04fd80c370aff8cddb2f188882e7385618b4e4e"),
    (0x1000002f, "c7cd901198177db8ea16152f2ee295361581c7ef1eb09fffc5ff333da6c5d81c"),
    (0x10000030, "c9698b83f76b72c76115151b84e8c1e06867c40c9597024bb9237109add9dc9e"),
];
