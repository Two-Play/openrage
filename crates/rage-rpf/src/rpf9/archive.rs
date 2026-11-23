//! Physical metadata views from 0x14087AFF0 / 0x14087B0F0, crypto spans
//! from 0x140A348A0 / 0x1406BD090 / 0x1408EC360 in the Scarlett reference.
use super::*;
use crate::rpf9_exe::ExtractedMaterial;
use std::io::Write;
#[path = "read.rs"]
mod decoded;

const MAX_METADATA: usize = 64 << 20;
const EXTENT_COLUMN: u64 = 0x33f7_0dfa_f2c5_0989;
const HASH_COLUMN: u64 = 0xb6f5_bdb2_c66d_a2e9;
const NAME_HASH_COLUMN: u64 = 0x73bc_b72d_ce74_f11f;
const NAME_HASH32_COLUMN: u64 = 0xb707_6b62_45d3_5fb5;
const RESOURCE_COLUMN: u64 = 0xba18_0afb_232d_1b4b;
const BLOCK_COLUMN: u64 = 0xe0c1_2d7a_5d48_84c5;
const CRYPTO_COLUMN: u64 = 0x2860_30a3_58a7_89fb;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Extent {
    pub offset: u64,
    /// Stored allocation in bytes, including any trailing padding.
    pub stored_size: u64,
    pub unpacked_size: u64,
    /// Uninterpreted upper 24 bits of the packed offset (includes codec).
    pub flags: u32,
    pub stream_codec: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CryptoDescriptor {
    pub selector: u32,
    pub algorithm: u16,
    pub iv: [u8; 16],
    pub range_fields: [u64; 5],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveEntry {
    /// Logical entry index; this is not necessarily its physical record index.
    pub index: u32,
    /// Opaque stored identifier, distinct from the path lookup hash.
    pub hash: u64,
    /// Normalized path lookup hash; extensions are not always included.
    pub name_hash: Option<u64>,
    /// Separate Jenkins lookup key used by some entry types.
    pub name_hash32: Option<u32>,
    /// Some entries use a different storage descriptor; never invent an extent.
    pub extent: Option<Extent>,
    pub has_crypto_descriptor: bool,
    pub crypto: Option<CryptoDescriptor>,
    pub resource_flags: Option<[u32; 2]>,
    pub blocks: Option<std::sync::Arc<[Block]>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub compressed_size: u64,
    pub stored_size: u64,
    pub unpacked_size: u64,
    pub codec: u8,
}
impl Block {
    /// Native packed descriptor decoding at 0x14550DB40.
    pub fn parse(q: u64, format: u8) -> Result<Self, Rpf9Error> {
        let (stored, padding, unpacked, codec) = match format {
            2 => ((q >> 28) & 0x0fff_fff0, (q >> 58) & 15, (q << 4) & 0x0fff_fff0, q >> 62),
            3 => {
                let actual = 0x10000 - ((q >> 16) & 0xffff);
                let pad = (q >> 16) & 15;
                (actual + pad, pad, 0x10000 - (q & 0xffff), (q >> 60) & 3)
            }
            4 => ((q >> 24) & 0x0fff_fff0, (q >> 52) & 15, q & 0x0fff_ffff, (q >> 56) & 3),
            _ => return Err(Rpf9Error::Unsupported(format!("packed block format {format}"))),
        };
        let compressed_size = stored.checked_sub(padding).ok_or_else(|| invalid("block padding exceeds size"))?;
        if compressed_size == 0 || unpacked == 0 {
            return Err(invalid("empty compressed block"));
        }
        Ok(Self { compressed_size, stored_size: stored, unpacked_size: unpacked, codec: codec as u8 })
    }
}

#[derive(Debug)]
pub struct Metadata {
    pub entries: Vec<ArchiveEntry>,
    pub key_selector: u32,
}

pub struct Archive {
    file: File,
    path: std::path::PathBuf,
    pub base_offset: u64,
    depth: u8,
    pub header: Header,
    pub metadata: Metadata,
}

fn invalid(message: &str) -> Rpf9Error {
    Rpf9Error::InvalidMetadata(message.to_owned())
}

fn aligned(n: usize, alignment: usize) -> Result<usize, Rpf9Error> {
    n.checked_add(alignment - 1).map(|n| n & !(alignment - 1)).ok_or_else(|| invalid("alignment overflow"))
}

fn slice(bytes: &[u8], at: usize, len: usize) -> Result<&[u8], Rpf9Error> {
    bytes
        .get(at..at.checked_add(len).ok_or_else(|| invalid("range overflow"))?)
        .ok_or_else(|| invalid("metadata range out of bounds"))
}

#[derive(Debug)]
struct Column {
    hash: u64,
    count: usize,
    offset: usize,
    stride: usize,
    external: bool,
}

impl Metadata {
    /// Parses already-decrypted physical views. Kept separate so malformed
    /// metadata can be tested without distributing game keys or ciphertext.
    pub fn from_decrypted(header: Header, file_size: u64, a: &[u8], b: &[u8]) -> Result<Self, Rpf9Error> {
        let errors = header.validation_errors(file_size);
        if !errors.is_empty() {
            return Err(Rpf9Error::InvalidHeader(errors.join("; ")));
        }
        if header.byte_2d != 0x0f {
            return Err(Rpf9Error::Unsupported("metadata flags (expected 0x0f)".into()));
        }
        let count = header.entry_count as usize;
        let columns = usize::from(header.field_20_low);
        if count > 1_000_000 || !(1..=64).contains(&columns) || a.len() > MAX_METADATA || b.len() > MAX_METADATA {
            return Err(invalid("metadata exceeds reader limits"));
        }
        let external = usize::from(header.region_b_mode);
        if external == 0 || external > columns {
            return Err(invalid("external column count"));
        }
        let internal = columns - external;
        // The inner cipher also covers H[0x30] bytes of property values.
        let column_start = header.alignment as usize;
        let index_start = column_start + columns * 24;
        let row_size = internal * usize::from(header.index_width);
        let data_start = aligned(index_start + row_size * count, 8)?;
        slice(a, 0, data_start)?;
        let b_data = aligned(count * external * usize::from(header.index_width), 8)?;
        slice(b, 0, b_data)?;
        let mut parsed = Vec::with_capacity(columns);
        let mut column_hashes = std::collections::HashSet::new();
        for i in 0..columns {
            let record = slice(a, column_start + i * 24, 24)?;
            let stride_flags = u32_at(record, 16).unwrap();
            let column = Column {
                hash: u64_at(record, 0).unwrap(),
                count: u32_at(record, 8).unwrap() as usize,
                offset: u32_at(record, 12).unwrap() as usize,
                stride: (stride_flags & 0xffff) as usize,
                external: stride_flags >> 16 == 1,
            };
            if stride_flags >> 16 > 1 || column.external != (i >= internal) || u32_at(record, 20) != Some(0) {
                return Err(Rpf9Error::Unsupported("column storage layout".into()));
            }
            if !column_hashes.insert(column.hash) {
                return Err(invalid("duplicate column hash"));
            }
            if column.stride == 0 || column.count > 1_000_000 {
                return Err(invalid("invalid column size"));
            }
            let (bytes, start) = if column.external { (b, b_data) } else { (a, data_start) };
            let length = column.count.checked_mul(column.stride).ok_or_else(|| invalid("column size overflow"))?;
            let at = start.checked_add(column.offset).ok_or_else(|| invalid("column offset overflow"))?;
            slice(bytes, at, length)?;
            parsed.push(column);
        }
        let find = |hash| -> Result<usize, Rpf9Error> {
            let matches: Vec<_> = parsed.iter().enumerate().filter(|(_, c)| c.hash == hash).map(|(i, _)| i).collect();
            if matches.len() != 1 {
                return Err(invalid("missing or duplicate required column"));
            }
            Ok(matches[0])
        };
        let extent_column = parsed.iter().position(|c| c.hash == EXTENT_COLUMN);
        let hash_column = find(HASH_COLUMN)?;
        if extent_column.is_some_and(|i| parsed[i].stride != 24) || parsed[hash_column].stride != 8 {
            return Err(invalid("unexpected extent/hash record size"));
        }
        let mut entries = Vec::with_capacity(count);
        let mut block_cache = std::collections::HashMap::new();
        let mut total_blocks = 0usize;
        for entry in 0..count {
            let mut extent = None;
            let mut hash = None;
            let mut name_hash = None;
            let mut name_hash32 = None;
            let mut has_crypto_descriptor = false;
            let mut crypto = None;
            let mut resource_flags = None;
            let mut resource_extent = None;
            let mut blocks: Option<std::sync::Arc<[Block]>> = None;
            for (i, col) in parsed.iter().enumerate() {
                let (indices, at) = if col.external {
                    (b, (entry * external + i - internal) * usize::from(header.index_width))
                } else {
                    (a, index_start + entry * row_size + i * usize::from(header.index_width))
                };
                let (index, missing) = match header.index_width {
                    1 => (usize::from(slice(indices, at, 1)?[0]), 0xff),
                    2 => (usize::from(u16::from_le_bytes(slice(indices, at, 2)?.try_into().unwrap())), 0xffff),
                    4 => (u32_at(slice(indices, at, 4)?, 0).unwrap() as usize, u32::MAX as usize),
                    _ => return Err(invalid("unsupported index width")),
                };
                if index == missing {
                    continue;
                }
                if index >= col.count {
                    return Err(invalid("column index out of bounds"));
                }
                let (bytes, start) = if col.external { (b, b_data) } else { (a, data_start) };
                let record = slice(bytes, start + col.offset + index * col.stride, col.stride)?;
                if col.hash == NAME_HASH_COLUMN {
                    if record.len() != 8 {
                        return Err(invalid("name hash record size"));
                    }
                    name_hash = u64_at(record, 0);
                }
                if col.hash == NAME_HASH32_COLUMN {
                    if record.len() != 4 {
                        return Err(invalid("32-bit name hash record size"));
                    }
                    name_hash32 = u32_at(record, 0);
                }
                if Some(i) == extent_column {
                    let packed = u64_at(record, 0).unwrap();
                    let offset = (packed & 0xff_ffff_ffff) * 16;
                    let stored_size = (u64_at(record, 8).unwrap() & 0xff_ffff_ffff) * 16;
                    let end = offset.checked_add(stored_size).ok_or_else(|| invalid("payload end overflow"))?;
                    if offset < u64::from(header.metadata_end) || end > file_size - u64::from(header.trailer_size) {
                        return Err(invalid("payload overlaps metadata/trailer or exceeds archive"));
                    }
                    extent = Some(Extent {
                        offset,
                        stored_size,
                        unpacked_size: u64_at(record, 16).unwrap() & 0xfff_ffff_ffff,
                        flags: (packed >> 40) as u32,
                        stream_codec: record[6],
                    });
                }
                if col.hash == RESOURCE_COLUMN {
                    if record.len() != 20 {
                        return Err(invalid("resource descriptor size"));
                    }
                    let f0 = u32_at(record, 0).unwrap();
                    let f1 = u32_at(record, 4).unwrap();
                    resource_flags = Some([f0, f1]);
                    resource_extent = Some(Extent {
                        offset: u64::from(u32_at(record, 12).unwrap()) * 16,
                        stored_size: u64::from(u32_at(record, 8).unwrap().wrapping_shl(4)),
                        unpacked_size: u64::from(f0 & 0x7fff_fff0) + u64::from(f1 & 0xffff_fff0),
                        flags: u32_at(record, 16).unwrap(),
                        stream_codec: record[17],
                    });
                }
                if col.hash == BLOCK_COLUMN {
                    if record.len() != 8 {
                        return Err(invalid("block-table descriptor layout"));
                    }
                    let record_at = start + col.offset + index * col.stride;
                    if let Some(cached) = block_cache.get(&record_at) {
                        blocks = Some(std::sync::Arc::clone(cached));
                    } else {
                        let table_at = record_at
                            .checked_add(u32_at(record, 0).unwrap() as usize)
                            .ok_or_else(|| invalid("block table overflow"))?;
                        let block_count = usize::from(u16::from_le_bytes(record[4..6].try_into().unwrap()));
                        total_blocks += block_count;
                        if total_blocks > 1_000_000 {
                            return Err(invalid("too many block descriptors"));
                        }
                        let raw = slice(bytes, table_at, block_count * 8)?;
                        let decoded: Vec<_> = raw
                            .as_chunks::<8>()
                            .0
                            .iter()
                            .map(|q| Block::parse(u64_at(q, 0).unwrap(), record[7]))
                            .collect::<Result<_, _>>()?;
                        let decoded: std::sync::Arc<[Block]> = decoded.into();
                        block_cache.insert(record_at, std::sync::Arc::clone(&decoded));
                        blocks = Some(decoded);
                    }
                }
                if i == hash_column {
                    hash = u64_at(record, 0);
                }
                if col.hash == CRYPTO_COLUMN {
                    if record.len() != 64 {
                        return Err(invalid("crypto descriptor size"));
                    }
                    has_crypto_descriptor = true;
                    crypto = Some(CryptoDescriptor {
                        selector: u32_at(record, 0).unwrap(),
                        algorithm: u16::from_le_bytes(record[4..6].try_into().unwrap()),
                        iv: record[6..22].try_into().unwrap(),
                        range_fields: std::array::from_fn(|i| u64_at(record, 24 + i * 8).unwrap()),
                    });
                }
            }
            if resource_extent.is_some() {
                extent = resource_extent;
            }
            if let Some(e) = &extent {
                if e.offset < u64::from(header.metadata_end)
                    || e.offset
                        .checked_add(e.stored_size)
                        .is_none_or(|end| end > file_size - u64::from(header.trailer_size))
                {
                    return Err(invalid("resource extent outside payload area"));
                }
                if let Some(blocks) = &blocks {
                    let stored: u64 = blocks.iter().map(|b| b.stored_size).sum();
                    let unpacked: u64 = blocks.iter().map(|b| b.unpacked_size).sum();
                    let prefix = if resource_flags.is_some() { 16 } else { 0 };
                    if stored.checked_add(prefix) != Some(e.stored_size) || unpacked != e.unpacked_size {
                        return Err(invalid("block totals disagree with extent"));
                    }
                }
            }
            entries.push(ArchiveEntry {
                index: entry as u32,
                hash: hash.ok_or_else(|| invalid("entry has no hash"))?,
                name_hash,
                name_hash32,
                extent,
                has_crypto_descriptor,
                crypto,
                resource_flags,
                blocks,
            });
        }
        Ok(Self { entries, key_selector: 0 })
    }
}

impl Archive {
    pub fn open(path: &Path, keys: &ExtractedMaterial) -> Result<Self, Rpf9Error> {
        Self::open_range(path, 0, std::fs::metadata(path)?.len(), keys)
    }

    /// Opens an uncompressed archive stored inside a bounded physical range.
    pub fn open_range(path: &Path, base_offset: u64, size: u64, keys: &ExtractedMaterial) -> Result<Self, Rpf9Error> {
        let mut file = File::open(path)?;
        let physical_size = file.metadata()?.len();
        if size < HEADER_SIZE as u64 || base_offset.checked_add(size).is_none_or(|end| end > physical_size) {
            return Err(invalid("archive range outside physical file"));
        }
        file.seek(SeekFrom::Start(base_offset))?;
        let mut bytes = [0u8; HEADER_SIZE];
        file.read_exact(&mut bytes)?;
        let header = Header::parse(&bytes)?;
        let errors = header.validation_errors(size);
        if !errors.is_empty() {
            return Err(Rpf9Error::InvalidHeader(errors.join("; ")));
        }
        if header.crypto_profile != 9 || header.byte_2d != 0x0f {
            return Err(Rpf9Error::Unsupported("crypto profile or metadata mode".into()));
        }
        let end = header.metadata_end as usize;
        if end > MAX_METADATA {
            return Err(invalid("metadata exceeds reader limit"));
        }
        let mut head = vec![0u8; end];
        head[..HEADER_SIZE].copy_from_slice(&bytes);
        file.read_exact(&mut head[HEADER_SIZE..])?;
        let first_size = header.first_metadata_span_size();
        let property_start = HEADER_SIZE + first_size;
        let property_end = property_start + header.region_constant as usize - header.alignment as usize;
        let split = header.region_b_offset as usize;
        if property_end > split {
            return Err(invalid("property area exceeds region A"));
        }
        // Algorithm 0x10 chains both property spans; it does NOT reset the IV
        // between the descriptor directory and its values (0x1406BD160).
        let properties =
            keys.material.clone().decryptor().decrypt_cbc(slice(&head, HEADER_SIZE, property_end - HEADER_SIZE)?)?;
        let mut descriptor = None;
        for record in slice(&properties, 0, (header.format_constant >> 16) as usize * 8)?.as_chunks::<8>().0 {
            if u32_at(record, 0) == Some(1) {
                if descriptor.is_some() {
                    return Err(invalid("duplicate TOC crypto property"));
                }
                let at = first_size + u32_at(record, 4).unwrap() as usize;
                descriptor = Some(slice(&properties, at, 24)?);
            }
        }
        let descriptor = descriptor.ok_or_else(|| invalid("missing TOC crypto property"))?;
        if u16::from_le_bytes(descriptor[4..6].try_into().unwrap()) != 0x10 {
            return Err(Rpf9Error::Unsupported("TOC cipher is not TFIT-CBC".into()));
        }
        let selector = u32_at(descriptor, 0).unwrap();
        let cipher = keys
            .metadata_keys
            .get(&selector)
            .ok_or_else(|| Rpf9Error::Unsupported(format!("metadata key {selector:#x}")))?
            .clone()
            .decryptor();
        let iv = descriptor[6..22].try_into().unwrap();
        // Each of these calls creates a fresh provider in the reference.
        let a = cipher.decrypt_cbc_with_iv(slice(&head, property_end, split - property_end)?, iv)?;
        let b = cipher.decrypt_cbc_with_iv(slice(&head, split, end - split)?, iv)?;
        let mut metadata = Metadata::from_decrypted(header, size, &a, &b)?;
        metadata.key_selector = selector;
        Ok(Self { file, path: path.to_owned(), base_offset, depth: 0, header, metadata })
    }

    /// Opens a directly stored nested archive without extracting it or allocating
    /// its payload. Compressed/encrypted containers must be decoded separately.
    pub fn open_child(&self, index: u32, keys: &ExtractedMaterial) -> Result<Self, Rpf9Error> {
        if self.depth >= 16 {
            return Err(invalid("archive nesting limit exceeded"));
        }
        let entry = self.metadata.entries.get(index as usize).ok_or_else(|| invalid("entry index out of bounds"))?;
        let extent = entry.extent.as_ref().ok_or_else(|| Rpf9Error::Unsupported("nested extent type".into()))?;
        if entry.crypto.is_some()
            || entry.blocks.is_some()
            || entry.resource_flags.is_some()
            || extent.stream_codec != 0
            || extent.unpacked_size > extent.stored_size
            || extent.stored_size - extent.unpacked_size >= 16
        {
            return Err(Rpf9Error::Unsupported("nested archive is not stored directly".into()));
        }
        let mut child = Self::open_range(&self.path, self.base_offset + extent.offset, extent.unpacked_size, keys)?;
        child.depth = self.depth + 1;
        Ok(child)
    }

    /// Copies the stored allocation verbatim. No names are used as paths,
    /// and no decompression/decryption is implied. The caller owns the writer.
    pub fn copy_raw(&mut self, index: u32, output: &mut impl Write) -> Result<u64, Rpf9Error> {
        let entry = self.metadata.entries.get(index as usize).ok_or_else(|| invalid("entry index out of bounds"))?;
        let extent = entry
            .extent
            .as_ref()
            .ok_or_else(|| Rpf9Error::Unsupported("entry has no supported physical extent".into()))?;
        self.file.seek(SeekFrom::Start(self.base_offset + extent.offset))?;
        let copied = std::io::copy(&mut Read::by_ref(&mut self.file).take(extent.stored_size), output)?;
        if copied != extent.stored_size {
            return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof).into());
        }
        Ok(copied)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (Header, Vec<u8>, Vec<u8>) {
        let header = Header {
            metadata_end: 0x260,
            region_b_offset: 0x240,
            trailer_size: 0x40,
            archive_id: 0,
            format_constant: FORMAT_CONSTANT,
            region_constant: REGION_CONSTANT,
            field_20_low: 2,
            region_b_mode: 1,
            entry_count: 2,
            profile_tag: 0,
            index_width: 1,
            byte_2d: 0x0f,
            crypto_profile: 9,
            byte_2f: 0,
            alignment: 16,
            reserved: [0; 12],
        };
        let mut a = vec![0; 0xa0];
        for (i, (hash, stride)) in [(EXTENT_COLUMN, 24u32), (HASH_COLUMN, 0x10008)].into_iter().enumerate() {
            let at = 16 + i * 24;
            a[at..at + 8].copy_from_slice(&hash.to_le_bytes());
            a[at + 8..at + 12].copy_from_slice(&2u32.to_le_bytes());
            a[at + 16..at + 20].copy_from_slice(&stride.to_le_bytes());
        }
        // Logical order reverses physical order, both in A and B.
        a[64..66].copy_from_slice(&[1, 0]);
        for (i, offset) in [0x30u64, 0x40].into_iter().enumerate() {
            let at = 72 + i * 24;
            a[at..at + 8].copy_from_slice(&offset.to_le_bytes());
            a[at + 8..at + 16].copy_from_slice(&2u64.to_le_bytes());
            a[at + 16..at + 24].copy_from_slice(&32u64.to_le_bytes());
        }
        let mut b = vec![0; 32];
        b[..2].copy_from_slice(&[1, 0]);
        b[8..16].copy_from_slice(&0x11u64.to_le_bytes());
        b[16..24].copy_from_slice(&0x22u64.to_le_bytes());
        (header, a, b)
    }

    #[test]
    fn logical_indices_select_physical_records() {
        let (h, a, b) = fixture();
        let m = Metadata::from_decrypted(h, 0x500, &a, &b).unwrap();
        assert_eq!(m.entries[0].hash, 0x22);
        assert_eq!(m.entries[0].extent.as_ref().unwrap().offset, 0x400);
        assert_eq!(m.entries[1].hash, 0x11);
        assert_eq!(m.entries[1].extent.as_ref().unwrap().offset, 0x300);
    }

    #[test]
    fn multiple_external_columns_use_their_own_row_stride() {
        let (mut h, old_a, _) = fixture();
        h.field_20_low = 3;
        h.region_b_mode = 2;
        h.alignment = 32;
        let mut a = vec![0; 192];
        a[32..56].copy_from_slice(&old_a[16..40]);
        a[56..64].copy_from_slice(&0x1234u64.to_le_bytes());
        a[64..68].copy_from_slice(&2u32.to_le_bytes());
        a[72..76].copy_from_slice(&0x10004u32.to_le_bytes());
        a[80..104].copy_from_slice(&old_a[40..64]);
        a[92..96].copy_from_slice(&8u32.to_le_bytes());
        a[104..106].copy_from_slice(&[1, 0]);
        a[112..160].copy_from_slice(&old_a[72..120]);
        let mut b = vec![0; 32];
        b[..4].copy_from_slice(&[0, 1, 1, 0]);
        b[16..24].copy_from_slice(&0x11u64.to_le_bytes());
        b[24..32].copy_from_slice(&0x22u64.to_le_bytes());
        let metadata = Metadata::from_decrypted(h, 0x500, &a, &b).unwrap();
        assert_eq!(metadata.entries[0].hash, 0x22);
        assert_eq!(metadata.entries[1].hash, 0x11);
        assert_eq!(metadata.entries[0].extent.as_ref().unwrap().offset, 0x400);
    }

    #[test]
    fn missing_extent_is_not_invented() {
        let (h, mut a, b) = fixture();
        a[64] = 0xff;
        let m = Metadata::from_decrypted(h, 0x500, &a, &b).unwrap();
        assert!(m.entries[0].extent.is_none());
    }

    #[test]
    fn rejects_corrupt_indices_sizes_and_ranges() {
        let (h, a, b) = fixture();
        let mut bad = a.clone();
        bad[64] = 2;
        assert!(Metadata::from_decrypted(h, 0x500, &bad, &b).is_err());
        let mut bad = a.clone();
        bad[24..28].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(Metadata::from_decrypted(h, 0x500, &bad, &b).is_err());
        let mut bad = a.clone();
        bad[80..88].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(Metadata::from_decrypted(h, 0x500, &bad, &b).is_err());
        assert!(Metadata::from_decrypted(h, 0x400, &a, &b).is_err());
        for end in 0..120 {
            assert!(Metadata::from_decrypted(h, 0x500, &a[..end], &b).is_err());
        }
        for end in 0..24 {
            assert!(Metadata::from_decrypted(h, 0x500, &a, &b[..end]).is_err());
        }
    }
}
