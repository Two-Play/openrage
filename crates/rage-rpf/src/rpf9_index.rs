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
/// "RPF9", the first field of a decoded header.
const MAGIC: u32 = 0x5250_4639;
/// The fixed part of the header, before the directory.
const HEADER_SIZE: usize = 64;
/// A header larger than this is taken as damaged.
const MAX_HEADER_BYTES: usize = 64 << 20;
/// More entries than this are taken as damaged.
const MAX_ENTRIES: usize = 1_000_000;
/// One table descriptor: type hash, record count, offset, stride.
const DESCRIPTOR_SIZE: usize = 24;
/// One directory record.
const DIRECTORY_RECORD_SIZE: usize = 8;
/// The bytes this parser reads of each record kind.
const RESOURCE_RECORD_MIN: usize = 18;
const CHUNKED_RECORD_MIN: usize = 24;
const RAW_RECORD_MIN: usize = 16;
const HASH32_RECORD_MIN: usize = 4;
const HASH64_RECORD_MIN: usize = 8;
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

/// The header's table dimensions, validated against the header's own size.
struct Dims {
    extent: usize,
    split: usize,
    directories: usize,
    blob_size: usize,
    table_count: usize,
    secondary: usize,
    primary: usize,
    count: usize,
    width: usize,
    wide_align: bool,
}

/// Where the header's arrays start.
struct Areas {
    descriptors: usize,
    indices: usize,
    secondary_indices: usize,
    data: usize,
    secondary_data: usize,
}

/// What a payload record says about a member's data.
struct Payload {
    offset: u64,
    stored: u64,
    size: u64,
    type_id: u8,
    layout: u8,
    storage: Storage,
}

fn check_dims(width: usize, secondary: usize, table_count: usize, count: usize) -> Result<(), String> {
    if !matches!(width, 1 | 2 | 4) || secondary > table_count || count > MAX_ENTRIES {
        return Err("unsupported native table dimensions".into());
    }
    if table_count == 0 && count != 0 {
        return Err("native entries have no tables".into());
    }
    Ok(())
}

fn read_dims(header: &[u8], archive_size: u64) -> Result<Dims, String> {
    if header.len() < HEADER_SIZE || u32_at(header, 0) != MAGIC {
        return Err("not a native RPF9 header".into());
    }
    let extent = u32_at(header, 4) as usize;
    let split = u32_at(header, 8) as usize;
    if extent != header.len() || extent as u64 > archive_size || header.len() > MAX_HEADER_BYTES {
        return Err("native header extent is invalid or too large".into());
    }
    let directories = usize::from(u16_at(header, 0x1a));
    let blob_size = u32_at(header, 0x1c) as usize;
    let table_count = usize::from(u16_at(header, 0x20));
    let secondary = usize::from(u16_at(header, 0x22));
    let count = u32_at(header, 0x24) as usize;
    let width = usize::from(header[0x2c]);
    let wide_align = header[0x2d] & 1 != 0;
    check_dims(width, secondary, table_count, count)?;
    let primary = table_count - secondary;
    Ok(Dims { extent, split, directories, blob_size, table_count, secondary, primary, count, width, wide_align })
}

/// The end of `bytes` bytes placed after `prev`, rounded up.
fn area_after(prev: usize, bytes: usize, alignment: usize) -> Result<usize, String> {
    aligned(sum(prev, bytes)?, alignment)
}

fn matrix_bytes(rows: usize, count: usize, width: usize) -> Result<usize, String> {
    product(product(rows, count)?, width)
}

/// Directory, blob, descriptors and the primary index matrix; returns the
/// descriptors' and the matrix' starts.
fn leading_areas(d: &Dims) -> Result<(usize, usize), String> {
    let (directory_align, wide) = if d.wide_align { (16, 16) } else { (4, 8) };
    let directory = aligned(HEADER_SIZE, directory_align)?;
    let blob = area_after(directory, product(d.directories, DIRECTORY_RECORD_SIZE)?, wide)?;
    let descriptors = area_after(blob, d.blob_size, wide)?;
    let indices = area_after(descriptors, product(d.table_count, DESCRIPTOR_SIZE)?, d.width)?;
    Ok((descriptors, indices))
}

fn read_areas(d: &Dims) -> Result<Areas, String> {
    let (descriptors, indices) = leading_areas(d)?;
    let data = area_after(indices, matrix_bytes(d.primary, d.count, d.width)?, 8)?;
    let secondary_indices = aligned(d.split, d.width)?;
    let secondary_data = area_after(secondary_indices, matrix_bytes(d.secondary, d.count, d.width)?, 8)?;
    if data > d.split || secondary_data > d.extent {
        return Err("native table arrays lie outside the header".into());
    }
    Ok(Areas { descriptors, indices, secondary_indices, data, secondary_data })
}

/// The table described at `at`, whose data starts `base` plus its offset and
/// must end within `limit`.
fn read_table(header: &[u8], at: usize, column: usize, base: usize, limit: usize) -> Result<Table, String> {
    let type_hash = u64_at(header, at);
    let records = u32_at(header, at + 8) as usize;
    let offset = u32_at(header, at + 12) as usize;
    let stride = usize::from(u16_at(header, at + 16));
    let start = sum(base, offset)?;
    if start > limit || sum(start, product(records, stride)?)? > limit || records > 0 && stride == 0 {
        return Err(format!("native table {column} lies outside its header section"));
    }
    Ok(Table { type_hash, count: records, stride, start })
}

fn read_tables(header: &[u8], d: &Dims, a: &Areas) -> Result<Vec<Table>, String> {
    let mut tables = Vec::with_capacity(d.table_count);
    let mut types = HashSet::new();
    for column in 0..d.table_count {
        let (base, limit) = if column < d.primary { (a.data, d.split) } else { (a.secondary_data, d.extent) };
        let table = read_table(header, a.descriptors + column * DESCRIPTOR_SIZE, column, base, limit)?;
        if !types.insert(table.type_hash) {
            return Err("duplicate native table type".into());
        }
        tables.push(table);
    }
    Ok(tables)
}

/// An index cell of the given width, and the value that stands for "none".
fn index_cell(header: &[u8], at: usize, width: usize) -> (u32, u32) {
    match width {
        1 => (u32::from(header[at]), 0xff),
        2 => (u32::from(u16_at(header, at)), 0xffff),
        _ => (u32_at(header, at), u32::MAX),
    }
}

/// The offset of each table's record for one slot.
fn select_records(
    header: &[u8],
    tables: &[Table],
    d: &Dims,
    a: &Areas,
    slot: usize,
) -> Result<Vec<Option<usize>>, String> {
    let mut selected = Vec::with_capacity(d.table_count);
    for (column, table) in tables.iter().enumerate() {
        let (matrix, columns, local) = if column < d.primary {
            (a.indices, d.primary, column)
        } else {
            (a.secondary_indices, d.secondary, column - d.primary)
        };
        let at = matrix + (slot * columns + local) * d.width;
        let (index, missing) = index_cell(header, at, d.width);
        if index == missing {
            selected.push(None);
        } else if (index as usize) < table.count {
            selected.push(Some(table.start + index as usize * table.stride));
        } else {
            return Err(format!("slot {slot} has an invalid native table index"));
        }
    }
    Ok(selected)
}

/// A slot's record of the table of type `kind`, if it has one.
fn slot_record<'h>(
    header: &'h [u8],
    tables: &[Table],
    selected: &[Option<usize>],
    kind: u64,
    min: usize,
) -> Result<Option<&'h [u8]>, String> {
    let Some(column) = tables.iter().position(|t| t.type_hash == kind) else { return Ok(None) };
    let Some(at) = selected[column] else { return Ok(None) };
    let stride = tables[column].stride;
    if stride < min {
        return Err(format!("native record {kind:016x} is too short"));
    }
    Ok(Some(&header[at..at + stride]))
}

fn resource_payload(r: &[u8]) -> Payload {
    let system_flags = u32_at(r, 0);
    let graphics_flags = u32_at(r, 4);
    Payload {
        offset: u64::from(u32_at(r, 12)) << 4,
        stored: u64::from(u32_at(r, 8)) << 4,
        size: u64::from(system_flags & 0x7fff_fff0) + u64::from(graphics_flags & 0xffff_fff0),
        type_id: r[16],
        layout: r[17],
        storage: Storage::Resource { system_flags, graphics_flags },
    }
}

fn chunked_payload(r: &[u8]) -> Payload {
    Payload {
        offset: (u64_at(r, 0) & MASK40) << 4,
        stored: (u64_at(r, 8) & MASK40) << 4,
        size: u64_at(r, 16) & MASK44,
        type_id: r[5],
        layout: r[6],
        storage: Storage::Chunked,
    }
}

fn raw_payload(r: &[u8]) -> Payload {
    Payload {
        offset: u64::from(u32_at(r, 4)) << 4,
        stored: u64::from(u32_at(r, 0)) << 4,
        size: u64_at(r, 8) & MASK48,
        type_id: r[14],
        layout: r[15],
        storage: Storage::Raw,
    }
}

/// The payload of the one record a slot has among the three kinds.
fn slot_payload(
    slot: usize,
    resource: Option<&[u8]>,
    chunked: Option<&[u8]>,
    raw: Option<&[u8]>,
) -> Result<Payload, String> {
    match (resource, chunked, raw) {
        (Some(r), None, None) => Ok(resource_payload(r)),
        (None, Some(r), None) => Ok(chunked_payload(r)),
        (None, None, Some(r)) => Ok(raw_payload(r)),
        _ => Err(format!("slot {slot} has missing or conflicting payload records")),
    }
}

fn check_payload(
    slot: usize,
    (offset, stored, size): (u64, u64, u64),
    extent: usize,
    archive_size: u64,
) -> Result<(), String> {
    if offset.checked_add(stored).is_none_or(|end| end > archive_size)
        || stored > 0 && offset < extent as u64
        || stored == 0 && size != 0
    {
        return Err(format!("slot {slot} payload lies outside the archive"));
    }
    Ok(())
}

fn parse_entry(
    header: &[u8],
    tables: &[Table],
    d: &Dims,
    a: &Areas,
    archive_size: u64,
    slot: usize,
) -> Result<NativeEntry, String> {
    let selected = select_records(header, tables, d, a, slot)?;
    let record = |kind: u64, min: usize| slot_record(header, tables, &selected, kind, min);
    let resource = record(RESOURCE, RESOURCE_RECORD_MIN)?;
    let chunked = record(CHUNKED, CHUNKED_RECORD_MIN)?;
    let raw = record(RAW, RAW_RECORD_MIN)?;
    let Payload { offset, stored, size, type_id, layout, storage } = slot_payload(slot, resource, chunked, raw)?;
    check_payload(slot, (offset, stored, size), d.extent, archive_size)?;
    let name_hash32 = record(HASH32, HASH32_RECORD_MIN)?.map(|r| u32_at(r, 0));
    let name_hash64 = record(HASH64, HASH64_RECORD_MIN)?.map(|r| u64_at(r, 0));
    Ok(NativeEntry {
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
    })
}

/// One list item of a chunk record in the given format; `next` is where the
/// previous chunk's padded end left off, for the formats without offsets.
fn decode_chunk(format: u8, value: u64, next: u64) -> Result<Chunk, String> {
    let (low, high) = (value as u32, (value >> 32) as u32);
    let wrapped = |v: u32| 0x10000 - u64::from(v & 0xffff);
    let (offset, stored, size, codec) = match format {
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
    Ok(Chunk { offset, stored, size, zlib: codec == 3 })
}

impl Catalog {
    /// Parses already-decoded header bytes and checks all tables, entry
    /// indices and payload ranges before exposing any native entries.
    pub fn parse(header: Vec<u8>, archive_size: u64) -> Result<Self, String> {
        let dims = read_dims(&header, archive_size)?;
        let areas = read_areas(&dims)?;
        let tables = read_tables(&header, &dims, &areas)?;
        let mut entries = Vec::with_capacity(dims.count);
        for slot in 0..dims.count {
            entries.push(parse_entry(&header, &tables, &dims, &areas, archive_size, slot)?);
        }
        let split = dims.split;
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
            let chunk = decode_chunk(self.header[at + 7], u64::from_le_bytes(*item), next)?;
            next = (chunk.offset + chunk.stored).next_multiple_of(16);
            chunks.push(chunk);
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
