//! Validated native RPF9 tables after header transforms have been removed.
//!
//! Layout reconstructed from the Scarlett loader and table getters documented
//! in `docs/rpf9-scarlett.md`. Cipher material is supplied separately by the
//! local executable; this parser contains no game data.

use std::collections::HashSet;

pub const RESOURCE: u64 = 0xba18_0afb_232d_1b4b;
pub const CHUNKED: u64 = 0x33f7_0dfa_f2c5_0989;
pub const RAW: u64 = 0x4eaa_16dd_8299_95bc;
pub const HASH32: u64 = 0xb707_6b62_45d3_5fb5;
pub const HASH64: u64 = 0x73bc_b72d_ce74_f11f;
pub const CHUNKS: u64 = 0xe0c1_2d7a_5d48_84c5;
pub const ENCRYPTION: u64 = 0x2860_30a3_58a7_89fb;
const MASK40: u64 = (1 << 40) - 1;
const MASK44: u64 = (1 << 44) - 1;
const MASK48: u64 = (1 << 48) - 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Storage {
    Raw,
    Chunked,
    Resource { system_flags: u32, graphics_flags: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeEntry {
    pub slot: usize,
    pub offset: u64,
    pub stored: u64,
    pub size: u64,
    pub type_id: u8,
    pub layout: u8,
    pub name_hash32: Option<u32>,
    pub name_hash64: Option<u64>,
    pub storage: Storage,
    records: Vec<Option<usize>>,
}

/// One chunk of a member's data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chunk {
    /// From the start of the member's data, after a resource's prefix.
    pub offset: u64,
    /// Stored bytes, without padding.
    pub stored: u64,
    /// Decoded bytes.
    pub size: u64,
    /// A zlib stream; otherwise the bytes as they are.
    pub zlib: bool,
}

#[derive(Debug, Clone)]
pub struct Table {
    pub type_hash: u64,
    pub count: usize,
    pub stride: usize,
    /// Absolute offset inside the decoded header, not inside archive payload.
    pub start: usize,
}

#[derive(Debug)]
pub struct Catalog {
    pub entries: Vec<NativeEntry>,
    pub tables: Vec<Table>,
    pub header: Vec<u8>,
    pub split: usize,
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(b[at..at + 2].try_into().expect("validated field"))
}
fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().expect("validated field"))
}
fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().expect("validated field"))
}
fn aligned(value: usize, alignment: usize) -> Result<usize, String> {
    if !alignment.is_power_of_two() {
        return Err("invalid native table alignment".into());
    }
    value.checked_add(alignment - 1).map(|n| n & !(alignment - 1)).ok_or_else(|| "native table overflow".into())
}
fn sum(a: usize, b: usize) -> Result<usize, String> {
    a.checked_add(b).ok_or_else(|| "native table overflow".into())
}
fn product(a: usize, b: usize) -> Result<usize, String> {
    a.checked_mul(b).ok_or_else(|| "native table overflow".into())
}

impl Catalog {
    /// Parses already-decoded header bytes and checks all tables, entry
    /// indices and payload ranges before exposing any native entries.
    pub fn parse(header: Vec<u8>, archive_size: u64) -> Result<Self, String> {
        if header.len() < 64 || u32_at(&header, 0) != 0x5250_4639 {
            return Err("not a native RPF9 header".into());
        }
        let extent = u32_at(&header, 4) as usize;
        let split = u32_at(&header, 8) as usize;
        if extent != header.len() || extent as u64 > archive_size || header.len() > 64 << 20 {
            return Err("native header extent is invalid or too large".into());
        }
        let directories = usize::from(u16_at(&header, 0x1a));
        let blob_size = u32_at(&header, 0x1c) as usize;
        let table_count = usize::from(u16_at(&header, 0x20));
        let secondary = usize::from(u16_at(&header, 0x22));
        let count = u32_at(&header, 0x24) as usize;
        let width = usize::from(header[0x2c]);
        let wide_align = header[0x2d] & 1 != 0;
        if !matches!(width, 1 | 2 | 4) || secondary > table_count || count > 1_000_000 {
            return Err("unsupported native table dimensions".into());
        }
        if table_count == 0 && count != 0 {
            return Err("native entries have no tables".into());
        }
        let primary = table_count - secondary;
        let directory = aligned(64, if wide_align { 16 } else { 4 })?;
        let blob = aligned(sum(directory, product(directories, 8)?)?, if wide_align { 16 } else { 8 })?;
        let descriptors = aligned(sum(blob, blob_size)?, if wide_align { 16 } else { 8 })?;
        let indices = aligned(sum(descriptors, product(table_count, 24)?)?, width)?;
        let data = aligned(sum(indices, product(product(primary, count)?, width)?)?, 8)?;
        let secondary_indices = aligned(split, width)?;
        let secondary_data = aligned(sum(secondary_indices, product(product(secondary, count)?, width)?)?, 8)?;
        if data > split || secondary_data > extent {
            return Err("native table arrays lie outside the header".into());
        }
        let mut tables = Vec::with_capacity(table_count);
        let mut types = HashSet::new();
        for column in 0..table_count {
            let at = descriptors + column * 24;
            let type_hash = u64_at(&header, at);
            let records = u32_at(&header, at + 8) as usize;
            let offset = u32_at(&header, at + 12) as usize;
            let stride = usize::from(u16_at(&header, at + 16));
            let (base, limit) = if column < primary { (data, split) } else { (secondary_data, extent) };
            let start = sum(base, offset)?;
            if start > limit || sum(start, product(records, stride)?)? > limit || records > 0 && stride == 0 {
                return Err(format!("native table {column} lies outside its header section"));
            }
            if !types.insert(type_hash) {
                return Err("duplicate native table type".into());
            }
            tables.push(Table { type_hash, count: records, stride, start });
        }
        let mut entries = Vec::with_capacity(count);
        for slot in 0..count {
            let mut selected = Vec::with_capacity(table_count);
            for (column, table) in tables.iter().enumerate() {
                let (matrix, columns, local) = if column < primary {
                    (indices, primary, column)
                } else {
                    (secondary_indices, secondary, column - primary)
                };
                let at = matrix + (slot * columns + local) * width;
                let index = match width {
                    1 => u32::from(header[at]),
                    2 => u32::from(u16_at(&header, at)),
                    _ => u32_at(&header, at),
                };
                let missing = match width {
                    1 => 0xff,
                    2 => 0xffff,
                    _ => u32::MAX,
                };
                if index == missing {
                    selected.push(None);
                } else if (index as usize) < table.count {
                    selected.push(Some(table.start + index as usize * table.stride));
                } else {
                    return Err(format!("slot {slot} has an invalid native table index"));
                }
            }
            let record = |kind: u64, min: usize| -> Result<Option<&[u8]>, String> {
                let Some(column) = tables.iter().position(|t| t.type_hash == kind) else { return Ok(None) };
                let Some(at) = selected[column] else { return Ok(None) };
                let stride = tables[column].stride;
                if stride < min {
                    return Err(format!("native record {kind:016x} is too short"));
                }
                Ok(Some(&header[at..at + stride]))
            };
            let resource = record(RESOURCE, 18)?;
            let chunked = record(CHUNKED, 24)?;
            let raw = record(RAW, 16)?;
            if [resource.is_some(), chunked.is_some(), raw.is_some()].into_iter().filter(|v| *v).count() != 1 {
                return Err(format!("slot {slot} has missing or conflicting payload records"));
            }
            let (offset, stored, size, type_id, layout, storage) = if let Some(r) = resource {
                let system_flags = u32_at(r, 0);
                let graphics_flags = u32_at(r, 4);
                (
                    u64::from(u32_at(r, 12)) << 4,
                    u64::from(u32_at(r, 8)) << 4,
                    u64::from(system_flags & 0x7fff_fff0) + u64::from(graphics_flags & 0xffff_fff0),
                    r[16],
                    r[17],
                    Storage::Resource { system_flags, graphics_flags },
                )
            } else if let Some(r) = chunked {
                (
                    (u64_at(r, 0) & MASK40) << 4,
                    (u64_at(r, 8) & MASK40) << 4,
                    u64_at(r, 16) & MASK44,
                    r[5],
                    r[6],
                    Storage::Chunked,
                )
            } else {
                let r = raw.expect("one checked payload record");
                (
                    u64::from(u32_at(r, 4)) << 4,
                    u64::from(u32_at(r, 0)) << 4,
                    u64_at(r, 8) & MASK48,
                    r[14],
                    r[15],
                    Storage::Raw,
                )
            };
            if offset.checked_add(stored).is_none_or(|end| end > archive_size)
                || stored > 0 && offset < extent as u64
                || stored == 0 && size != 0
            {
                return Err(format!("slot {slot} payload lies outside the archive"));
            }
            let name_hash32 = record(HASH32, 4)?.map(|r| u32_at(r, 0));
            let name_hash64 = record(HASH64, 8)?.map(|r| u64_at(r, 0));
            entries.push(NativeEntry {
                slot,
                offset,
                stored,
                size,
                type_id,
                layout,
                name_hash32,
                name_hash64,
                storage,
                records: selected,
            });
        }
        Ok(Self { entries, tables, header, split })
    }

    /// The chunks a member is stored in, or `None` if it is stored whole. A
    /// chunk record holds where its list starts, counted from the record
    /// itself, the number of chunks, the size of a list item and its format.
    pub fn chunks(&self, slot: usize) -> Result<Option<Vec<Chunk>>, String> {
        let Some(column) = self.tables.iter().position(|t| t.type_hash == CHUNKS) else { return Ok(None) };
        let at = self.entries.get(slot).and_then(|e| e.records.get(column).copied().flatten());
        let Some(at) = at else { return Ok(None) };
        if self.tables[column].stride != 8 || self.header[at + 6] != 8 {
            return Err("unsupported chunk record".into());
        }
        let list = sum(at, u32_at(&self.header, at) as usize)?;
        let count = usize::from(u16_at(&self.header, at + 4));
        let items = self.header.get(list..sum(list, count * 8)?).ok_or("chunk list lies outside the header")?;
        let mut chunks = Vec::with_capacity(count);
        // Formats 2 and 4 have no offsets: chunks follow each other, padded.
        let mut next = 0u64;
        for item in items.as_chunks::<8>().0 {
            let value = u64::from_le_bytes(*item);
            let (low, high) = (value as u32, (value >> 32) as u32);
            let wrapped = |v: u32| 0x10000 - u64::from(v & 0xffff);
            let (offset, stored, size, codec) = match self.header[at + 7] {
                2 => {
                    let padded = u64::from(high & 0x03ff_ffff) << 4;
                    (next, padded.checked_sub(u64::from(high >> 26 & 15)), u64::from(low) << 4, high >> 30)
                }
                3 => (u64::from(high & 0x0fff_ffff) << 4, Some(wrapped(low >> 16)), wrapped(low), high >> 28),
                4 => {
                    let padded = value >> 24 & 0xff_ffff;
                    (next, padded.checked_sub(u64::from(high >> 20 & 15)), value & 0xff_ffff, high >> 24)
                }
                format => return Err(format!("chunk format {format} is not supported")),
            };
            let stored = stored.ok_or("a chunk is shorter than its padding")?;
            if !matches!(codec, 0 | 3) {
                return Err(format!("chunk codec {codec} is not supported"));
            }
            next = (offset + stored).next_multiple_of(16);
            chunks.push(Chunk { offset, stored, size, zlib: codec == 3 });
        }
        Ok(Some(chunks))
    }

    /// A selected row's native table record, including optional tables whose
    /// interpretation is handled by the payload decoder.
    pub fn record(&self, slot: usize, table_type: u64) -> Option<&[u8]> {
        let column = self.tables.iter().position(|t| t.type_hash == table_type)?;
        let at = *self.entries.get(slot)?.records.get(column)?.as_ref()?;
        self.header.get(at..at + self.tables[column].stride)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<u8> {
        let mut b = vec![0u8; 224];
        let put32 = |b: &mut [u8], at, v: u32| b[at..at + 4].copy_from_slice(&v.to_le_bytes());
        put32(&mut b, 0, 0x5250_4639);
        put32(&mut b, 4, 224);
        put32(&mut b, 8, 192);
        b[0x20..0x22].copy_from_slice(&3u16.to_le_bytes());
        b[0x22..0x24].copy_from_slice(&1u16.to_le_bytes());
        put32(&mut b, 0x24, 2);
        b[0x2c] = 1;
        b[0x2d] = 1;
        for (column, kind, offset, stride) in [(0, RAW, 0u32, 16u16), (1, HASH32, 32, 4), (2, HASH64, 0, 8)] {
            let at = 64 + column * 24;
            b[at..at + 8].copy_from_slice(&kind.to_le_bytes());
            put32(&mut b, at + 8, 2);
            put32(&mut b, at + 12, offset);
            b[at + 16..at + 18].copy_from_slice(&stride.to_le_bytes());
        }
        b[136..140].copy_from_slice(&[0, 0, 1, 1]);
        b[192..194].copy_from_slice(&[0, 1]);
        for slot in 0..2 {
            let at = 144 + slot * 16;
            put32(&mut b, at, 1);
            put32(&mut b, at + 4, 16 + slot as u32 * 2);
            put32(&mut b, at + 8, 5);
            b[at + 14] = 0xff;
            put32(&mut b, 176 + slot * 4, 0x1234 + slot as u32);
            b[200 + slot * 8..208 + slot * 8].copy_from_slice(&(0xabcd + slot as u64).to_le_bytes());
        }
        b
    }

    #[test]
    fn resolves_split_index_matrices_and_payload_records() {
        let catalog = Catalog::parse(fixture(), 320).unwrap();
        assert_eq!(catalog.entries.len(), 2);
        let e = &catalog.entries[1];
        assert_eq!((e.slot, e.offset, e.stored, e.size), (1, 288, 16, 5));
        assert_eq!((e.name_hash32, e.name_hash64), (Some(0x1235), Some(0xabce)));
        assert_eq!(e.storage, Storage::Raw);
        assert_eq!(catalog.record(1, RAW).unwrap().len(), 16);
        assert!(catalog.record(2, RAW).is_none());
        assert!(catalog.record(0, RESOURCE).is_none());
    }

    #[test]
    fn accepts_absent_optional_records_but_rejects_bad_indices_and_ranges() {
        let mut b = fixture();
        b[139] = 0xff;
        assert_eq!(Catalog::parse(b.clone(), 320).unwrap().entries[1].name_hash32, None);
        b[138] = 2;
        assert!(Catalog::parse(b, 320).is_err());
        let mut b = fixture();
        b[148..152].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(Catalog::parse(b, 320).is_err());
        let mut b = fixture();
        b[64 + 8..64 + 12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(Catalog::parse(b, 320).is_err());
        assert!(Catalog::parse(fixture()[..223].to_vec(), 320).is_err());
        assert!(Catalog::parse(fixture(), 200).is_err());
    }

    #[test]
    fn reads_the_three_chunk_formats() {
        let chunks = |format: u8, items: &[[u8; 8]]| {
            // The second table (hashes, 8-byte records here) becomes chunks;
            // the list follows the header's other data.
            let mut b = fixture();
            b[112..120].copy_from_slice(&CHUNKS.to_le_bytes());
            let record = 200;
            let list = b.len();
            b[record..record + 4].copy_from_slice(&((list - record) as u32).to_le_bytes());
            b[record + 4..record + 8].copy_from_slice(&[items.len() as u8, 0, 8, format]);
            items.iter().for_each(|item| b.extend_from_slice(item));
            let len = b.len() as u32;
            b[4..8].copy_from_slice(&len.to_le_bytes());
            for slot in 0..2 {
                b[148 + slot * 16..152 + slot * 16].copy_from_slice(&32u32.to_le_bytes());
            }
            Catalog::parse(b, 1024).unwrap().chunks(0)
        };
        let chunk = |offset, stored, size, zlib| Chunk { offset, stored, size, zlib };
        // 2: sizes in units of 16, padding in the last one.
        let items = [[0, 0x40, 1, 0, 0xfd, 0x50, 0, 0xd4], [2, 0, 0, 0, 2, 0, 0, 0]];
        let expected = vec![chunk(0, 0x50fd0 - 5, 0x14_0000, true), chunk(0x50fd0, 32, 32, false)];
        assert_eq!(chunks(2, &items), Ok(Some(expected)));
        // 3: sizes counted down from 64 KiB, with the chunk's offset.
        let items = [[0, 0, 0x41, 0xb8, 0, 0, 0, 0x30], [0xf1, 0xff, 0xf1, 0xff, 0x7c, 4, 0, 0]];
        let expected = vec![chunk(0, 0x47bf, 0x10000, true), chunk(0x47c0, 15, 15, false)];
        assert_eq!(chunks(3, &items), Ok(Some(expected)));
        // 4: 24-bit sizes, padding, codec.
        let items = [[0, 0, 0x13, 0x70, 0xaa, 4, 0x70, 3], [0x0f, 0, 0, 0x10, 0, 0, 0x10, 0]];
        let expected = vec![chunk(0, 0x4aa70 - 7, 0x13_0000, true), chunk(0x4aa70, 15, 15, false)];
        assert_eq!(chunks(4, &items), Ok(Some(expected)));
        assert!(chunks(5, &items).is_err());
        assert!(chunks(4, &[[0, 0, 0, 0, 0, 0, 0, 9]]).is_err());
        // Less stored than padding.
        assert!(chunks(4, &[[1, 0, 0, 0, 0, 0, 0x10, 0]]).is_err());
    }

    #[test]
    fn rejects_bad_dimensions_and_missing_storage() {
        let mut b = fixture();
        b[0x2c] = 0;
        assert!(Catalog::parse(b, 320).is_err());
        let mut b = fixture();
        b[0x22..0x24].copy_from_slice(&4u16.to_le_bytes());
        assert!(Catalog::parse(b, 320).is_err());
        let mut b = fixture();
        b[136] = 0xff;
        assert!(Catalog::parse(b, 320).is_err());
        let mut b = fixture();
        b[64..72].copy_from_slice(&HASH32.to_le_bytes());
        assert!(Catalog::parse(b, 320).is_err());
    }
}
