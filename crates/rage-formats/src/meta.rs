//! RSC "meta" resources (resource version 2): `.ymap`, `.ytyp` and some `.ymt`.
//!
//! The file describes its own layout. The root structure (`rage::Meta`, after the
//! 16-byte resource header) holds:
//!
//! | offset | field |
//! |---|---|
//! | `0x1C` | root block (1-based) |
//! | `0x20` / `0x28` / `0x30` | structure infos / enum infos / data blocks |
//! | `0x38` | name |
//! | `0x48` / `0x4A` / `0x4C` | counts (i16) of the three lists |
//!
//! * Structure info (32 bytes): name `0x00`, entries `0x10`, size `0x18`, entry count `0x1E`.
//! * Entry (16 bytes): name, data offset, data type, unused, reference index, reference key.
//! * Enum info (24 bytes): name, key, entries `0x08`, count `0x10`; entries are (name, value).
//! * Data block (16 bytes): structure name, length, pointer.
//!
//! Pointers inside block data are packed: bits 0–11 the 1-based block, bits 12–31
//! the offset in it. Arrays are 16 bytes: pointer, count (u16), capacity (u16).

use rage_core::bytes::{LeRead, f16_to_f32};
use std::collections::HashMap;

use rage_resource::{Resource, ResourceError};

use crate::names;
use crate::xml::{self, Writer};

/// Resource version of meta files.
pub const VERSION: u32 = 2;
/// Resource version of Red Dead Redemption 2's meta files (`.ymap`, `.ytyp`):
/// the same format with the header fields from `0x1C` moved 8 bytes on (root
/// block `0x20` (4 bytes on), lists `0x28`/`0x30`/`0x38` and name `0x40` (8 on)).
pub const VERSION_RDR2: u32 = 3;

const ARRAYINFO: u32 = 0x100;
/// Zero-ended strings are cut off here.
const MAX_STRING: usize = 1 << 20;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MetaError {
    #[error(transparent)]
    Resource(#[from] ResourceError),
    #[error("invalid meta file: {0}")]
    Invalid(String),
}

/// Data types of structure entries.
mod ty {
    pub const BOOLEAN: u8 = 0x01;
    /// 64-bit unsigned integer (RDR2, e.g. entity ids).
    pub const ULONG: u8 = 0x08;
    /// RDR2: another 64-bit integer (guids).
    pub const ULONG2: u8 = 0x17;
    /// RDR2: half-precision float.
    pub const HALF: u8 = 0x20;
    /// RDR2: a 3D vector in 16 bytes.
    pub const VEC3_PADDED: u8 = 0x31;
    /// RDR2: 4×4 float matrix.
    pub const MATRIX: u8 = 0x36;
    /// RDR2: a 32-byte value of unknown layout (shown as bytes).
    pub const OPAQUE32: u8 = 0x53;
    /// RDR2: a map; the entry's key holds the key and value type indices.
    pub const MAP: u8 = 0x71;
    /// GTA VI: 2D vector, 3D vector without padding, 3×4 matrix.
    pub const VEC2: u8 = 0x30;
    pub const VEC3: u8 = 0x32;
    pub const MATRIX34: u8 = 0x35;
    /// GTA VI: another 32-bit name hash, a 64-bit one, a 16-bit enum.
    pub const HASH2: u8 = 0x4D;
    pub const HASH64: u8 = 0x4F;
    pub const SHORT_ENUM: u8 = 0x61;
    /// GTA VI: 16-byte values of unknown layout (shown as bytes).
    pub const OPAQUE16: u8 = 0x5C;
    pub const OPAQUE16B: u8 = 0x67;
    /// GTA VI: pointer to a zero-ended string.
    pub const STRING_POINTER: u8 = 0x42;
    /// GTA VI: a hash table; its key is like a map's, the halves swapped.
    pub const HASH_TABLE: u8 = 0x75;
    pub const STRUCTURE: u8 = 0x05;
    pub const STRUCTURE_POINTER: u8 = 0x07;
    pub const SBYTE: u8 = 0x10;
    pub const UBYTE: u8 = 0x11;
    pub const SSHORT: u8 = 0x12;
    pub const USHORT: u8 = 0x13;
    pub const SINT: u8 = 0x14;
    pub const UINT: u8 = 0x15;
    pub const FLOAT: u8 = 0x21;
    pub const FLOAT_XYZ: u8 = 0x33;
    pub const FLOAT_XYZW: u8 = 0x34;
    pub const ARRAY_OF_CHARS: u8 = 0x40;
    pub const CHAR_POINTER: u8 = 0x44;
    pub const HASH: u8 = 0x4A;
    pub const ARRAY_OF_BYTES: u8 = 0x50;
    pub const ARRAY: u8 = 0x52;
    pub const DATA_BLOCK_POINTER: u8 = 0x59;
    pub const BYTE_ENUM: u8 = 0x60;
    pub const INT_ENUM: u8 = 0x62;
    pub const INT_FLAGS1: u8 = 0x63;
    pub const SHORT_FLAGS: u8 = 0x64;
    pub const INT_FLAGS2: u8 = 0x65;
}

#[derive(Debug, Clone, Copy, Default)]
struct Entry {
    name: u32,
    offset: usize,
    kind: u8,
    key: u32,
}

#[derive(Debug, Clone)]
struct StructInfo {
    size: usize,
    entries: Vec<Entry>,
}

#[derive(Debug, Clone)]
struct Block {
    name: u32,
    data: Vec<u8>,
}

/// A parsed meta file.
#[derive(Debug, Clone)]
pub struct Meta {
    /// The name stored in the file (often empty).
    pub name: String,
    pub root_block: usize,
    structs: HashMap<u32, StructInfo>,
    enums: HashMap<u32, Vec<(u32, i32)>>,
    blocks: Vec<Block>,
    /// Bit where the offset of a packed pointer starts (12, or 20 in RDR2).
    offset_shift: u32,
}

/// The name of a block that holds the items of a map.
const MAP_BLOCK: u32 = 0x101;

/// Whether a resource with this version is a meta file.
pub fn is_meta(version: u32) -> bool {
    version == VERSION || version == VERSION_RDR2
}

/// Whether this is the system part of a GTA VI meta file. They are laid out
/// like Red Dead Redemption 2's ([`VERSION_RDR2`]).
pub fn is_gta6(system: &[u8]) -> bool {
    system.get(0x10..0x18) == Some(b"0DRP\x7a\0\0\0")
}

/// Size of a structure info, an entry, an enum info and a data block info.
const STRUCT_INFO_SIZE: u64 = 32;
const ENTRY_SIZE: u64 = 16;
const ENUM_INFO_SIZE: u64 = 24;
const BLOCK_INFO_SIZE: u64 = 16;
/// Most structures, enums and blocks a file may list; most values an enum may list.
const MAX_LIST: usize = 0x10000;

/// The counts of the structure, enum and block lists.
fn list_counts(res: &Resource) -> Result<(usize, usize, usize), MetaError> {
    let root = Resource::ROOT;
    let i16_at = |off: u64| -> Result<usize, MetaError> { Ok((res.u16(root + off)? as i16).max(0) as usize) };
    let (struct_count, enum_count, block_count) = (i16_at(0x48)?, i16_at(0x4A)?, i16_at(0x4C)?);
    if block_count == 0 || block_count > MAX_LIST || struct_count > MAX_LIST || enum_count > MAX_LIST {
        return Err(MetaError::Invalid(format!("{block_count} blocks, {struct_count} structures")));
    }
    Ok((struct_count, enum_count, block_count))
}

/// The name of the file, empty when it has none.
fn meta_name(res: &Resource, shift: u64) -> Result<String, MetaError> {
    let name_ptr = res.u64(Resource::ROOT + 0x38 + shift)?;
    // GTA VI has a list in place of the name.
    Ok(if name_ptr != 0 && !is_gta6(&res.system) { res.string(name_ptr).unwrap_or_default() } else { String::new() })
}

fn read_struct(res: &Resource, at: u64) -> Result<(u32, StructInfo), MetaError> {
    let entries_ptr = res.u64(at + 0x10)?;
    let size = res.u32(at + 0x18)? as usize;
    let count = (res.u16(at + 0x1E)? as i16).max(0) as u64;
    let mut entries = Vec::with_capacity(count as usize);
    for e in 0..count {
        let p = entries_ptr + e * ENTRY_SIZE;
        entries.push(Entry {
            name: res.u32(p)?,
            offset: res.u32(p + 4)? as i32 as usize,
            kind: res.u8(p + 8)?,
            key: res.u32(p + 12)?,
        });
    }
    Ok((res.u32(at)?, StructInfo { size, entries }))
}

fn read_structs(res: &Resource, infos: u64, count: usize) -> Result<HashMap<u32, StructInfo>, MetaError> {
    let mut structs = HashMap::new();
    for i in 0..count as u64 {
        let (name, info) = read_struct(res, infos + i * STRUCT_INFO_SIZE)?;
        structs.insert(name, info);
    }
    Ok(structs)
}

/// One enum value as (name, value).
fn read_enum_value(res: &Resource, entries_ptr: u64, e: u64) -> Result<(u32, i32), MetaError> {
    // GTA V: (name, value), 8 bytes; RDR2: (value, name), 16 bytes.
    if res.version == VERSION_RDR2 {
        let p = entries_ptr + e * 16;
        Ok((res.u32(p + 8)?, res.u32(p)? as i32))
    } else {
        let p = entries_ptr + e * 8;
        Ok((res.u32(p)?, res.u32(p + 4)? as i32))
    }
}

fn read_enum(res: &Resource, at: u64) -> Result<(u32, Vec<(u32, i32)>), MetaError> {
    let entries_ptr = res.u64(at + 8)?;
    let count = res.u32(at + 0x10)? as u64;
    let mut entries = Vec::with_capacity(count.min(MAX_LIST as u64) as usize);
    for e in 0..count.min(MAX_LIST as u64) {
        entries.push(read_enum_value(res, entries_ptr, e)?);
    }
    Ok((res.u32(at)?, entries))
}

fn read_enums(res: &Resource, infos: u64, count: usize) -> Result<HashMap<u32, Vec<(u32, i32)>>, MetaError> {
    let mut enums = HashMap::new();
    for i in 0..count as u64 {
        let (name, values) = read_enum(res, infos + i * ENUM_INFO_SIZE)?;
        enums.insert(name, values);
    }
    Ok(enums)
}

fn read_blocks(res: &Resource, infos: u64, count: usize) -> Result<Vec<Block>, MetaError> {
    let mut blocks = Vec::with_capacity(count);
    for i in 0..count as u64 {
        let at = infos + i * BLOCK_INFO_SIZE;
        let len = res.u32(at + 4)? as usize;
        let ptr = res.u64(at + 8)?;
        let data = if len == 0 { Vec::new() } else { res.bytes(ptr, len)?.to_vec() };
        blocks.push(Block { name: res.u32(at)?, data });
    }
    Ok(blocks)
}

/// The structures, enums and blocks the lists at `0x20`, `0x28` and `0x30` hold.
type Lists = (HashMap<u32, StructInfo>, HashMap<u32, Vec<(u32, i32)>>, Vec<Block>);

fn read_lists(res: &Resource, shift: u64, (structs, enums, blocks): (usize, usize, usize)) -> Result<Lists, MetaError> {
    let root = Resource::ROOT;
    let structs = read_structs(res, res.u64(root + 0x20 + shift)?, structs)?;
    let enums = read_enums(res, res.u64(root + 0x28 + shift)?, enums)?;
    let blocks = read_blocks(res, res.u64(root + 0x30 + shift)?, blocks)?;
    Ok((structs, enums, blocks))
}

pub fn parse(res: &Resource) -> Result<Meta, MetaError> {
    let counts = list_counts(res)?;
    let rdr2 = res.version == VERSION_RDR2;
    let shift = if rdr2 { 8 } else { 0 };
    let root_block = res.u32(Resource::ROOT + if rdr2 { 0x20 } else { 0x1C })? as usize;
    let name = meta_name(res, shift)?;
    let (structs, enums, blocks) = read_lists(res, shift, counts)?;
    let offset_shift = if rdr2 { 20 } else { 12 };
    Ok(Meta { name, root_block, structs, enums, blocks, offset_shift })
}

/// Parses a meta file and writes it as XML (see [`Meta::to_xml`]).
pub fn to_xml(res: &Resource) -> Result<(String, Vec<String>), MetaError> {
    Ok(parse(res)?.to_xml())
}

fn le<T>(data: &[u8], at: usize, n: usize, v: Option<T>) -> Result<T, String> {
    v.ok_or_else(|| format!("read of {n} bytes at {at:#x} beyond block of {:#x}", data.len()))
}

fn u16_at(d: &[u8], at: usize) -> Result<u16, String> {
    le(d, at, 2, d.u16_at(at))
}

fn u32_at(d: &[u8], at: usize) -> Result<u32, String> {
    le(d, at, 4, d.u32_at(at))
}

fn u64_at(d: &[u8], at: usize) -> Result<u64, String> {
    le(d, at, 8, d.u64_at(at))
}

fn f32_at(d: &[u8], at: usize) -> Result<f32, String> {
    le(d, at, 4, d.f32_at(at))
}

fn vector(d: &[u8], at: usize, n: usize) -> Result<Vec<f32>, String> {
    (0..n).map(|i| f32_at(d, at + i * 4)).collect()
}

/// Splits a packed pointer into (1-based block, offset): bits 0–11 and
/// 12–31 in GTA V; RDR2 starts the offset at bit 20.
fn unpack_with(ptr: u64, offset_shift: u32) -> (usize, usize) {
    ((ptr & 0xFFF) as usize, ((ptr >> offset_shift) & 0xFFFFF) as usize)
}

/// `<tag x="…" y="…" …/>` contents for a vector.
fn vector_attrs(name: &str, v: &[f32]) -> String {
    let mut s = name.to_owned();
    for (axis, value) in ["x", "y", "z", "w"].iter().zip(v) {
        s.push_str(&format!(" {axis}=\"{}\"", xml::float(*value)));
    }
    s
}

/// A 64-bit name hash; nothing for 0.
fn hash64_text(hash: u64) -> String {
    if hash == 0 { String::new() } else { format!("hash_{hash:016X}") }
}

/// Where an entry is read: the output level, the block it lives in, and the position.
#[derive(Clone, Copy)]
struct Site<'a> {
    level: usize,
    block_id: usize,
    data: &'a [u8],
    at: usize,
    depth: usize,
}

/// The entry being read, the array info before it and the structure it belongs to.
#[derive(Clone, Copy)]
struct EntryCtx<'a> {
    entry: &'a Entry,
    array_info: &'a Entry,
    owner: &'a StructInfo,
}

/// Where an array is: its packed pointer, its item count, and the output level and depth.
#[derive(Clone, Copy)]
struct ArrayRef {
    ptr: u64,
    count: usize,
    level: usize,
    depth: usize,
}

type ItemText = fn(&[u8]) -> String;

/// How the items of a number array are read: their size, the numbers per line
/// (`None`: one `Item` element per line) and how one is written.
struct ArrayItems {
    size: usize,
    columns: Option<usize>,
    text: ItemText,
}

/// Four little-endian floats in 16 bytes.
fn float4_text(b: &[u8]) -> String {
    b.as_chunks::<4>().0.iter().map(|c| xml::float(f32::from_le_bytes(*c))).collect::<Vec<_>>().join(", ")
}

fn integer_items(kind: u8) -> Option<ArrayItems> {
    let (size, text): (usize, ItemText) = match kind {
        ty::UINT => (4, |b| u32::from_le_bytes(b.try_into().unwrap_or_default()).to_string()),
        ty::USHORT => (2, |b| u16::from_le_bytes(b.try_into().unwrap_or_default()).to_string()),
        ty::UBYTE => (1, |b| b[0].to_string()),
        ty::ULONG | ty::ULONG2 => (8, |b| u64::from_le_bytes(b.try_into().unwrap_or_default()).to_string()),
        ty::SINT => (4, |b| i32::from_le_bytes(b.try_into().unwrap_or_default()).to_string()),
        ty::SSHORT => (2, |b| i16::from_le_bytes(b.try_into().unwrap_or_default()).to_string()),
        _ => return None,
    };
    Some(ArrayItems { size, columns: Some(10), text })
}

fn other_items(kind: u8) -> Option<ArrayItems> {
    let (size, columns, text): (usize, Option<usize>, ItemText) = match kind {
        ty::FLOAT => (4, Some(10), |b| xml::float(f32::from_le_bytes(b.try_into().unwrap_or_default()))),
        ty::FLOAT_XYZ | ty::FLOAT_XYZW => (16, None, float4_text),
        ty::HASH64 => (8, None, |b| hash64_text(u64::from_le_bytes(b.try_into().unwrap_or_default()))),
        ty::HASH | ty::HASH2 => (4, None, |b| names::value(u32::from_le_bytes(b.try_into().unwrap_or_default()))),
        _ => return None,
    };
    Some(ArrayItems { size, columns, text })
}

fn array_items(kind: u8) -> Option<ArrayItems> {
    integer_items(kind).or_else(|| other_items(kind))
}

/// What the pairs of one map share.
struct MapPairs<'a> {
    level: usize,
    id: usize,
    block: &'a Block,
    stride: usize,
    key_kind: u8,
    value_entry: Entry,
    value_info: Entry,
    owner: &'a StructInfo,
    depth: usize,
}

fn matrix_entry(w: &mut Writer, site: &Site, kind: u8, ename: &str) -> Result<(), String> {
    let Site { level, data, at, .. } = *site;
    if kind == ty::MATRIX {
        let m = vector(data, at, 16)?;
        let rows: Vec<String> =
            m.chunks(4).map(|r| r.iter().map(|v| xml::float(*v)).collect::<Vec<_>>().join(" ")).collect();
        w.raw_array(level, ename, &rows, 1);
    } else {
        let v: Vec<String> = vector(data, at, 12)?.into_iter().map(xml::float).collect();
        w.raw_array(level, ename, &v, 4);
    }
    Ok(())
}

fn opaque_entry(w: &mut Writer, site: &Site, kind: u8, ename: &str) -> Result<(), String> {
    let Site { level, data, at, .. } = *site;
    let size = if kind == ty::OPAQUE32 { 32 } else { 16 };
    let b: Vec<String> =
        (0..size).map(|i| byte_at(data, at + i).map(|v| format!("{v:02X}"))).collect::<Result<_, _>>()?;
    w.raw_array(level, ename, &b, 16);
    Ok(())
}

fn chars_entry(w: &mut Writer, site: &Site, n: usize, ename: &str) {
    let Site { level, data, at, .. } = *site;
    let bytes = data.get(at..(at + n).min(data.len())).unwrap_or_default();
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    w.text(level, ename, &String::from_utf8_lossy(&bytes[..end]));
}

fn byte_at(data: &[u8], at: usize) -> Result<u8, String> {
    data.get(at).copied().ok_or_else(|| format!("offset {at:#x} beyond block"))
}

/// The items of an `ARRAY_OF_BYTES` entry as text, by the type of its items.
fn byte_array_text(data: &[u8], at: usize, n: usize, item_kind: u8) -> Result<Vec<String>, String> {
    let items = |size: usize, f: &dyn Fn(usize) -> Result<String, String>| (0..n).map(|i| f(at + i * size)).collect();
    match item_kind {
        ty::SBYTE => items(1, &|p| byte_at(data, p).map(|b| (b as i8).to_string())),
        ty::UBYTE => items(1, &|p| byte_at(data, p).map(|b| b.to_string())),
        ty::SSHORT => items(2, &|p| u16_at(data, p).map(|v| (v as i16).to_string())),
        ty::USHORT => items(2, &|p| u16_at(data, p).map(|v| v.to_string())),
        ty::SINT => items(4, &|p| u32_at(data, p).map(|v| (v as i32).to_string())),
        ty::UINT => items(4, &|p| u32_at(data, p).map(|v| v.to_string())),
        ty::FLOAT => items(4, &|p| f32_at(data, p).map(xml::float)),
        _ => {
            Ok(vec![(0..n).map(|i| byte_at(data, at + i).map(|b| format!("{b:02X}"))).collect::<Result<String, _>>()?])
        }
    }
}

/// How many floats a vector entry has.
fn vector_len(kind: u8) -> Option<usize> {
    match kind {
        ty::FLOAT_XYZ | ty::VEC3_PADDED | ty::VEC3 => Some(3),
        ty::FLOAT_XYZW => Some(4),
        ty::VEC2 => Some(2),
        _ => None,
    }
}

/// The text of a plain number (or boolean) entry; `None` for other kinds.
fn number_text(kind: u8, ename: &str, data: &[u8], at: usize) -> Result<Option<String>, String> {
    let text = match kind {
        ty::BOOLEAN | ty::BYTE_ENUM | ty::UBYTE | ty::SBYTE => byte_text(kind, data, at)?,
        ty::HALF | ty::SHORT_ENUM | ty::USHORT | ty::SSHORT => short_text(kind, data, at)?,
        ty::FLOAT | ty::SINT | ty::UINT => word_text(kind, ename, data, at)?,
        ty::ULONG | ty::ULONG2 => u64_at(data, at)?.to_string(),
        _ => return Ok(None),
    };
    Ok(Some(text))
}

fn byte_text(kind: u8, data: &[u8], at: usize) -> Result<String, String> {
    let b = byte_at(data, at)?;
    Ok(match kind {
        ty::BOOLEAN => (if b != 0 { "true" } else { "false" }).to_owned(),
        ty::SBYTE => (b as i8).to_string(),
        _ => b.to_string(),
    })
}

fn short_text(kind: u8, data: &[u8], at: usize) -> Result<String, String> {
    let v = u16_at(data, at)?;
    Ok(match kind {
        ty::HALF => xml::float(f16_to_f32(v)),
        ty::SSHORT => (v as i16).to_string(),
        _ => v.to_string(),
    })
}

fn word_text(kind: u8, ename: &str, data: &[u8], at: usize) -> Result<String, String> {
    if kind == ty::FLOAT {
        return Ok(xml::float(f32_at(data, at)?));
    }
    let v = u32_at(data, at)?;
    Ok(match kind {
        ty::SINT => (v as i32).to_string(),
        _ if ename == "color" => format!("0x{v:08X}"),
        _ => v.to_string(),
    })
}

/// The pointer, group count (hash tables only) and pair count of a map.
#[derive(Clone, Copy)]
struct MapHeader {
    ptr: u64,
    groups: usize,
    count: usize,
}

fn map_header(data: &[u8], at: usize, table: bool) -> Result<MapHeader, String> {
    let ptr = u64_at(data, at + 8)?;
    // A hash table has its groups of 16 places and its count in front.
    let groups = if table { u32_at(data, at)? as usize } else { 0 };
    let count = if table { u32_at(data, at + 4)? as usize } else { u16_at(data, at + 16)? as usize };
    Ok(MapHeader { ptr, groups, count })
}

/// The key and value entry indices of a map entry's key; a hash table has them the other way round.
fn map_entry_indices(key: u32, table: bool) -> (usize, usize) {
    let (ki, vi) = ((key >> 16) as usize, (key & 0xFFFF) as usize);
    if table { (vi, ki) } else { (ki, vi) }
}

/// Where a hash table's pairs are: its used places. Each group is 16 control
/// bytes (the top bit set: unused), then 16 places.
fn hash_table_pairs(block: &Block, off: usize, groups: usize, stride: usize) -> Result<Vec<usize>, String> {
    let group = 16 + 16 * stride;
    let end = groups.checked_mul(group).and_then(|len| off.checked_add(len));
    let bytes = end.and_then(|end| block.data.get(off..end)).ok_or("hash table beyond its block")?;
    let used = |g: usize| {
        let at = off + g * group;
        (0..16).filter(move |i| bytes[g * group + i] < 0x80).map(move |i| at + 16 + i * stride)
    };
    Ok((0..groups).flat_map(used).collect())
}

#[derive(Clone, Copy, PartialEq)]
enum Tag {
    None,
    Structure,
    Item,
    ItemAndType,
}

impl Meta {
    fn unpack(&self, ptr: u64) -> (usize, usize) {
        unpack_with(ptr, self.offset_shift)
    }

    fn block(&self, id: usize) -> Option<&Block> {
        id.checked_sub(1).and_then(|i| self.blocks.get(i))
    }

    /// Names of the structures used, the root first; e.g. `CMapData`.
    pub fn root_type(&self) -> Option<String> {
        self.block(self.root_block).map(|b| names::field(b.name))
    }

    /// The texts of a GTA VI language file (`#ldbmt`), by the hash of their
    /// label. They are in a map: groups of 16 control bytes (above `0x7f`: no
    /// item) and as many items of 16 bytes, the label's hash and a pointer to
    /// a struct with the text's pointer at 8.
    pub fn label_texts(&self) -> Vec<(u32, String)> {
        const GROUP: usize = 16 + 16 * 16;
        let text = |item: &[u8]| -> Option<String> {
            let (id, off) = self.unpack(item.u64_at(8)?);
            let (id, off) = self.unpack(self.block(id)?.data.as_slice().u64_at(off + 8)?);
            let start = self.block(id)?.data.get(off..)?;
            let end = start.iter().position(|b| *b == 0)?;
            Some(String::from_utf8_lossy(&start[..end]).into_owned())
        };
        let mut out = Vec::new();
        for block in self.blocks.iter().filter(|b| b.name == MAP_BLOCK) {
            for group in block.data.as_chunks::<GROUP>().0 {
                let (control, items) = group.split_at(16);
                for (item, _) in items.as_chunks::<16>().0.iter().zip(control).filter(|(_, c)| **c < 0x80) {
                    if let (Some(label), Some(text)) = (item.u32_at(0), text(item)) {
                        out.push((label, text));
                    }
                }
            }
        }
        out
    }

    /// Reads `count` items of `size` bytes starting at a packed pointer; long
    /// arrays continue in the following blocks.
    fn array(&self, ptr: u64, count: usize, size: usize) -> Result<Vec<u8>, String> {
        let (mut id, mut off) = self.unpack(ptr);
        let mut out = Vec::with_capacity(count * size);
        for _ in 0..count {
            let mut block = self.block(id).ok_or_else(|| format!("array block {id} missing"))?;
            if off + size > block.data.len() {
                id += 1;
                off = 0;
                block = self.block(id).ok_or_else(|| format!("array block {id} missing"))?;
            }
            out.extend_from_slice(block.data.get(off..off + size).ok_or("array item beyond block")?);
            off += size;
        }
        Ok(out)
    }

    fn enum_string(&self, entry: &Entry, value: i32) -> String {
        let Some(values) = self.enums.get(&entry.key) else { return value.to_string() };
        if matches!(entry.kind, ty::INT_FLAGS1 | ty::INT_FLAGS2) {
            let set: Vec<String> = values
                .iter()
                .filter(|(_, bit)| (0..32).contains(bit) && value & (1 << bit) != 0)
                .map(|(name, _)| names::field(*name))
                .collect();
            return set.join(", ");
        }
        values.iter().find(|(_, v)| *v == value).map(|(n, _)| names::field(*n)).unwrap_or_else(|| value.to_string())
    }

    /// The file as XML, formatted like CodeWalker's export, plus the problems
    /// found while reading (also written as `<error>` elements).
    pub fn to_xml(&self) -> (String, Vec<String>) {
        let mut w = Writer::new();
        self.node(&mut w, 0, self.root_block, 0, Tag::Structure, 0, 0);
        w.finish()
    }

    #[allow(clippy::too_many_arguments)]
    fn node(
        &self,
        w: &mut Writer,
        level: usize,
        block_id: usize,
        offset: usize,
        tag: Tag,
        struct_name: u32,
        depth: usize,
    ) {
        if w.full() {
            return;
        }
        let Some(block) = self.block(block_id) else {
            return w.error(level, format!("block {block_id} not found"));
        };
        if depth > 64 {
            return w.error(level, "structures nested too deeply".into());
        }
        let struct_name = if struct_name == 0 { block.name } else { struct_name };
        let name = names::field(struct_name);
        let Some(info) = self.structs.get(&struct_name) else {
            return w.error(level, format!("structure info {name} not found"));
        };
        let tag_name = match tag {
            Tag::None => None,
            Tag::Structure if !self.name.is_empty() && depth == 0 => {
                Some(format!("{name} name=\"{}\"", xml::escape(&self.name)))
            }
            Tag::Structure => Some(name.clone()),
            Tag::Item => Some("Item".to_owned()),
            Tag::ItemAndType => Some(format!("Item type=\"{name}\"")),
        };
        if let Some(t) = &tag_name {
            w.open(level, t);
        }
        let inner = level + 1;
        let mut array_info = Entry::default();
        for entry in &info.entries {
            if entry.name == ARRAYINFO {
                array_info = *entry;
                continue;
            }
            let ename = names::field(entry.name);
            let site = Site { level: inner, block_id, data: &block.data, at: offset + entry.offset, depth };
            let ctx = EntryCtx { entry, array_info: &array_info, owner: info };
            if let Err(e) = self.entry(w, &site, &ctx, &ename) {
                w.error(inner, format!("{ename}: {e}"));
            }
        }
        if let Some(t) = tag_name {
            w.close(level, if tag == Tag::Structure { t.split(' ').next().unwrap_or(&t) } else { "Item" });
        }
    }

    fn entry(&self, w: &mut Writer, site: &Site, ctx: &EntryCtx, ename: &str) -> Result<(), String> {
        let Site { level, data, at, .. } = *site;
        let EntryCtx { entry, array_info, owner } = *ctx;
        match entry.kind {
            ty::ARRAY => self.array_node(w, site, array_info, ename),
            ty::MAP | ty::HASH_TABLE => self.map_node(w, site, entry, owner, ename),
            ty::ARRAY_OF_BYTES => {
                let text = byte_array_text(data, at, entry.key as usize, array_info.kind)?;
                w.text(level, ename, &text.join(" "));
                Ok(())
            }
            ty::ARRAY_OF_CHARS => {
                chars_entry(w, site, entry.key as usize, ename);
                Ok(())
            }
            ty::CHAR_POINTER => self.char_pointer(w, site, ename),
            ty::STRING_POINTER => self.string_pointer(w, site, ename),
            ty::DATA_BLOCK_POINTER => self.data_block_pointer(w, site, ename),
            ty::STRUCTURE | ty::STRUCTURE_POINTER => self.structure_entry(w, site, entry, ename),
            _ => self.scalar_entry(w, site, entry, ename),
        }
    }

    fn char_pointer(&self, w: &mut Writer, site: &Site, ename: &str) -> Result<(), String> {
        let Site { level, data, at, .. } = *site;
        let ptr = u64_at(data, at)?;
        let len = u16_at(data, at + 8)? as usize;
        let text = if ptr == 0 || len == 0 { String::new() } else { self.string(ptr, len)? };
        w.text(level, ename, &text);
        Ok(())
    }

    fn string_pointer(&self, w: &mut Writer, site: &Site, ename: &str) -> Result<(), String> {
        let ptr = u64_at(site.data, site.at)?;
        let text = if ptr == 0 { String::new() } else { self.string(ptr, MAX_STRING)? };
        w.text(site.level, ename, &text);
        Ok(())
    }

    fn data_block_pointer(&self, w: &mut Writer, site: &Site, ename: &str) -> Result<(), String> {
        let (id, _) = self.unpack(u64_at(site.data, site.at)?);
        let bytes: Vec<String> = match self.block(id) {
            Some(b) => b.data.iter().map(|b| format!("{b:02X}")).collect(),
            None => Vec::new(),
        };
        w.raw_array(site.level, ename, &bytes, 32);
        Ok(())
    }

    /// Nested structures, inline or through a pointer.
    fn structure_entry(&self, w: &mut Writer, site: &Site, entry: &Entry, ename: &str) -> Result<(), String> {
        let Site { level, block_id, data, at, depth } = *site;
        if entry.kind == ty::STRUCTURE {
            w.open(level, ename);
            self.node(w, level, block_id, at, Tag::None, entry.key, depth + 1);
            w.close(level, ename);
            return Ok(());
        }
        let ptr = u64_at(data, at)?;
        let (id, off) = self.unpack(ptr);
        match self.block(id) {
            Some(b) if ptr != 0 => {
                w.open(level, &format!("{ename} type=\"{}\"", names::field(b.name)));
                self.node(w, level, id, off, Tag::None, 0, depth + 1);
                w.close(level, ename);
            }
            _ => w.empty(level, ename),
        }
        Ok(())
    }

    /// Entries that are a plain value, a vector or an enum.
    fn scalar_entry(&self, w: &mut Writer, site: &Site, entry: &Entry, ename: &str) -> Result<(), String> {
        let Site { level, data, at, .. } = *site;
        if let Some(text) = number_text(entry.kind, ename, data, at)? {
            w.value(level, ename, &text);
            return Ok(());
        }
        if let Some(n) = vector_len(entry.kind) {
            w.empty(level, &vector_attrs(ename, &vector(data, at, n)?));
            return Ok(());
        }
        if let Some(text) = self.enum_text(entry, data, at)? {
            w.string(level, ename, &text);
            return Ok(());
        }
        self.blob_entry(w, site, entry.kind, ename)
    }

    /// Enum and flag values as their names.
    fn enum_text(&self, entry: &Entry, data: &[u8], at: usize) -> Result<Option<String>, String> {
        let value = match entry.kind {
            // RDR2's 0x63 flags are a single byte.
            ty::INT_FLAGS1 if self.offset_shift == 20 => i32::from(byte_at(data, at)?),
            ty::INT_ENUM | ty::INT_FLAGS1 | ty::INT_FLAGS2 => u32_at(data, at)? as i32,
            ty::SHORT_FLAGS => u16_at(data, at)? as i16 as i32,
            _ => return Ok(None),
        };
        Ok(Some(self.enum_string(entry, value)))
    }

    /// Hashes, matrices and opaque byte blocks; anything else is an error.
    fn blob_entry(&self, w: &mut Writer, site: &Site, kind: u8, ename: &str) -> Result<(), String> {
        let Site { level, data, at, .. } = *site;
        match kind {
            ty::HASH | ty::HASH2 => w.string(level, ename, &names::value(u32_at(data, at)?)),
            ty::HASH64 => w.text(level, ename, &hash64_text(u64_at(data, at)?)),
            ty::MATRIX | ty::MATRIX34 => matrix_entry(w, site, kind, ename)?,
            ty::OPAQUE32 | ty::OPAQUE16 | ty::OPAQUE16B => opaque_entry(w, site, kind, ename)?,
            other => return Err(format!("unexpected data type {other:#04x}")),
        }
        Ok(())
    }

    fn string(&self, ptr: u64, len: usize) -> Result<String, String> {
        let (id, off) = self.unpack(ptr);
        let block = self.block(id).ok_or_else(|| format!("string block {id} missing"))?;
        let bytes = block.data.get(off..(off + len).min(block.data.len())).ok_or("string beyond block")?;
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        Ok(String::from_utf8_lossy(&bytes[..end]).into_owned())
    }

    /// An RDR2 map: `1`, pointer at +8, count at +16. Each pair is the key
    /// (4 bytes, padded to 8) and the value; the entry's key names the
    /// owner's type entries for key (high 16 bits) and value (low 16 bits).
    fn map_node(
        &self,
        w: &mut Writer,
        site: &Site,
        entry: &Entry,
        owner: &StructInfo,
        ename: &str,
    ) -> Result<(), String> {
        let Site { level, data, at, depth, .. } = *site;
        let table = entry.kind == ty::HASH_TABLE;
        let header = map_header(data, at, table)?;
        if header.count == 0 {
            w.empty(level, ename);
            return Ok(());
        }
        let (pairs_at, pairs) = self.map_places(entry, owner, header, level + 1, depth + 1)?;
        w.open(level, ename);
        for (i, pair) in pairs.into_iter().enumerate() {
            if !self.map_pair(w, &pairs_at, i, pair)? {
                break;
            }
        }
        w.close(level, ename);
        Ok(())
    }

    /// What the pairs of a map share, and where each pair is.
    fn map_places<'a>(
        &'a self,
        entry: &Entry,
        owner: &'a StructInfo,
        header: MapHeader,
        level: usize,
        depth: usize,
    ) -> Result<(MapPairs<'a>, Vec<usize>), String> {
        let MapHeader { ptr, groups, count } = header;
        let table = entry.kind == ty::HASH_TABLE;
        let (ki, vi) = map_entry_indices(entry.key, table);
        let key_entry = *owner.entries.get(ki).ok_or("map key type not found")?;
        let value_entry = *owner.entries.get(vi).ok_or("map value type not found")?;
        // An array value's item type is the type entry before it.
        let value_info = vi.checked_sub(1).and_then(|i| owner.entries.get(i)).copied().unwrap_or_default();
        let stride = (8 + self.map_value_size(&value_entry)).next_multiple_of(8);
        let (id, off) = self.unpack(ptr);
        let block = self.block(id).ok_or_else(|| format!("map block {id} not found"))?;
        let pairs: Vec<usize> = if table {
            hash_table_pairs(block, off, groups, stride)?
        } else {
            (0..count).map(|i| off + i * stride).collect()
        };
        if pairs.len() != count {
            return Err(format!("{} places used, the table names {count}", pairs.len()));
        }
        let key_kind = key_entry.kind;
        Ok((MapPairs { level, id, block, stride, key_kind, value_entry, value_info, owner, depth }, pairs))
    }

    /// The size of a map value, which sets the stride of the pairs.
    fn map_value_size(&self, value_entry: &Entry) -> usize {
        match value_entry.kind {
            ty::ARRAY => 16,
            ty::STRUCTURE => self.structs.get(&value_entry.key).map_or(8, |s| s.size),
            ty::INT_FLAGS1 if self.offset_shift == 20 => 1,
            ty::HASH | ty::HASH2 | ty::UINT | ty::SINT | ty::FLOAT | ty::INT_ENUM | ty::INT_FLAGS1 | ty::INT_FLAGS2 => {
                4
            }
            ty::BOOLEAN | ty::UBYTE | ty::SBYTE | ty::BYTE_ENUM => 1,
            ty::USHORT | ty::SSHORT | ty::SHORT_FLAGS => 2,
            ty::FLOAT_XYZ | ty::FLOAT_XYZW => 16,
            _ => 8,
        }
    }

    /// One key/value pair of a map; `false` when it lies beyond its block, which ends the map.
    fn map_pair(&self, w: &mut Writer, m: &MapPairs, i: usize, pair: usize) -> Result<bool, String> {
        if pair + m.stride > m.block.data.len() {
            w.error(m.level, format!("map pair {i} beyond its block"));
            return Ok(false);
        }
        let key = match m.key_kind {
            ty::HASH | ty::UINT | ty::INT_ENUM => names::value(u32_at(&m.block.data, pair)?),
            _ => u32_at(&m.block.data, pair)?.to_string(),
        };
        w.open(m.level, &format!("Item key=\"{}\"", xml::escape(&key)));
        let site = Site { level: m.level + 1, block_id: m.id, data: &m.block.data, at: pair + 8, depth: m.depth };
        if let Err(e) = self.entry(
            w,
            &site,
            &EntryCtx { entry: &m.value_entry, array_info: &m.value_info, owner: m.owner },
            "Value",
        ) {
            w.error(m.level + 1, format!("map value: {e}"));
        }
        w.close(m.level, "Item");
        Ok(true)
    }

    fn array_node(&self, w: &mut Writer, site: &Site, item: &Entry, ename: &str) -> Result<(), String> {
        let Site { level, data, at, depth, .. } = *site;
        let loc = ArrayRef { ptr: u64_at(data, at)?, count: u16_at(data, at + 8)? as usize, level, depth };
        match item.kind {
            ty::STRUCTURE => self.structure_array(w, &loc, item, ename),
            ty::STRUCTURE_POINTER => self.structure_pointer_array(w, &loc, ename),
            kind => self.number_array(w, &loc, kind, ename),
        }
    }

    fn structure_array(&self, w: &mut Writer, loc: &ArrayRef, item: &Entry, ename: &str) -> Result<(), String> {
        let ArrayRef { ptr, count, level, depth } = *loc;
        let tag = format!("{ename} itemType=\"{}\"", names::field(item.key));
        if count == 0 {
            w.empty(level, &tag);
            return Ok(());
        }
        let info = self.structs.get(&item.key).ok_or("array item structure not found")?;
        let (mut id, mut off) = self.unpack(ptr);
        w.open(level, &tag);
        for n in 0..count {
            self.node(w, level + 1, id, off, Tag::Item, item.key, depth + 1);
            off += info.size;
            if n + 1 < count && self.block(id).is_some_and(|b| off >= b.data.len()) {
                id += 1;
                off = 0;
            }
        }
        w.close(level, ename);
        Ok(())
    }

    fn structure_pointer_array(&self, w: &mut Writer, loc: &ArrayRef, ename: &str) -> Result<(), String> {
        let ArrayRef { ptr, count, level, depth } = *loc;
        if count == 0 {
            w.empty(level, ename);
            return Ok(());
        }
        let ptrs = self.array(ptr, count, 8)?;
        w.open(level, ename);
        for p in ptrs.as_chunks::<8>().0 {
            let p = u64::from_le_bytes(*p);
            let (id, off) = self.unpack(p);
            if p == 0 {
                w.empty(level + 1, "Item");
            } else {
                self.node(w, level + 1, id, off, Tag::ItemAndType, 0, depth + 1);
            }
        }
        w.close(level, ename);
        Ok(())
    }

    /// An array of plain numbers, hashes or vectors.
    fn number_array(&self, w: &mut Writer, loc: &ArrayRef, kind: u8, ename: &str) -> Result<(), String> {
        let Some(items) = array_items(kind) else { return Err(format!("unexpected array item type {kind:#04x}")) };
        let bytes = self.array(loc.ptr, loc.count, items.size)?;
        let v: Vec<String> = bytes.chunks_exact(items.size).map(items.text).collect();
        match items.columns {
            Some(columns) => w.raw_array(loc.level, ename, &v, columns),
            None => w.item_array(loc.level, ename, &v),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gta6_label_texts() {
        let block = |name, data: Vec<u8>| Block { name, data };
        let mut map = vec![0x80u8; 16];
        map[2] = 0x2a;
        map.resize(16 + 16 * 16, 0);
        map[16 + 32..16 + 36].copy_from_slice(&7u32.to_le_bytes());
        // Block 1, 16 bytes in; there the text is in block 3, 4 bytes in.
        map[16 + 40..16 + 48].copy_from_slice(&(1u64 | 16 << 20).to_le_bytes());
        let mut items = vec![0u8; 32];
        items[24..].copy_from_slice(&(3u64 | 4 << 20).to_le_bytes());
        let blocks = vec![block(9, items), block(MAP_BLOCK, map), block(0x10, b"abc\0Motel\0".to_vec())];
        let meta = Meta {
            name: String::new(),
            root_block: 1,
            structs: HashMap::new(),
            enums: HashMap::new(),
            blocks,
            offset_shift: 20,
        };
        assert_eq!(meta.label_texts(), [(7, "Motel".to_owned())]);
    }
}
