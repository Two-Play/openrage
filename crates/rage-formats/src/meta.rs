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

use std::collections::HashMap;

use rage_resource::{Resource, ResourceError};

use crate::names;
use crate::xml::{self, Writer};

/// Resource version of meta files.
pub const VERSION: u32 = 2;

const ARRAYINFO: u32 = 0x100;

#[derive(Debug, thiserror::Error)]
pub enum MetaError {
    #[error(transparent)]
    Resource(#[from] ResourceError),
    #[error("invalid meta file: {0}")]
    Invalid(String),
}

/// Data types of structure entries.
mod ty {
    pub const BOOLEAN: u8 = 0x01;
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
}

/// Whether a resource with this version is a meta file.
pub fn is_meta(version: u32) -> bool {
    version == VERSION
}

pub fn parse(res: &Resource) -> Result<Meta, MetaError> {
    let root = Resource::ROOT;
    let i16_at = |off: u64| -> Result<usize, MetaError> { Ok((res.u16(root + off)? as i16).max(0) as usize) };
    let (struct_count, enum_count, block_count) = (i16_at(0x48)?, i16_at(0x4A)?, i16_at(0x4C)?);
    if block_count == 0 || block_count > 0x10000 || struct_count > 0x10000 || enum_count > 0x10000 {
        return Err(MetaError::Invalid(format!("{block_count} blocks, {struct_count} structures")));
    }
    let root_block = res.u32(root + 0x1C)? as usize;
    let name_ptr = res.u64(root + 0x38)?;
    let name = if name_ptr != 0 { res.string(name_ptr).unwrap_or_default() } else { String::new() };

    let mut structs = HashMap::new();
    let infos = res.u64(root + 0x20)?;
    for i in 0..struct_count as u64 {
        let at = infos + i * 32;
        let entries_ptr = res.u64(at + 0x10)?;
        let size = res.u32(at + 0x18)? as usize;
        let count = (res.u16(at + 0x1E)? as i16).max(0) as u64;
        let mut entries = Vec::with_capacity(count as usize);
        for e in 0..count {
            let p = entries_ptr + e * 16;
            entries.push(Entry {
                name: res.u32(p)?,
                offset: res.u32(p + 4)? as i32 as usize,
                kind: res.u8(p + 8)?,
                key: res.u32(p + 12)?,
            });
        }
        structs.insert(res.u32(at)?, StructInfo { size, entries });
    }

    let mut enums = HashMap::new();
    let infos = res.u64(root + 0x28)?;
    for i in 0..enum_count as u64 {
        let at = infos + i * 24;
        let entries_ptr = res.u64(at + 8)?;
        let count = res.u32(at + 0x10)? as u64;
        let mut entries = Vec::with_capacity(count as usize);
        for e in 0..count.min(0x10000) {
            let p = entries_ptr + e * 8;
            entries.push((res.u32(p)?, res.u32(p + 4)? as i32));
        }
        enums.insert(res.u32(at)?, entries);
    }

    let mut blocks = Vec::with_capacity(block_count);
    let infos = res.u64(root + 0x30)?;
    for i in 0..block_count as u64 {
        let at = infos + i * 16;
        let len = res.u32(at + 4)? as usize;
        let ptr = res.u64(at + 8)?;
        let data = if len == 0 { Vec::new() } else { res.bytes(ptr, len)?.to_vec() };
        blocks.push(Block { name: res.u32(at)?, data });
    }
    Ok(Meta { name, root_block, structs, enums, blocks })
}

/// Parses a meta file and writes it as XML (see [`Meta::to_xml`]).
pub fn to_xml(res: &Resource) -> Result<(String, Vec<String>), MetaError> {
    Ok(parse(res)?.to_xml())
}

fn le<const N: usize>(data: &[u8], at: usize) -> Result<[u8; N], String> {
    data.get(at..at + N)
        .and_then(|s| s.try_into().ok())
        .ok_or_else(|| format!("read of {N} bytes at {at:#x} beyond block of {:#x}", data.len()))
}

fn u16_at(d: &[u8], at: usize) -> Result<u16, String> {
    le::<2>(d, at).map(u16::from_le_bytes)
}

fn u32_at(d: &[u8], at: usize) -> Result<u32, String> {
    le::<4>(d, at).map(u32::from_le_bytes)
}

fn u64_at(d: &[u8], at: usize) -> Result<u64, String> {
    le::<8>(d, at).map(u64::from_le_bytes)
}

fn f32_at(d: &[u8], at: usize) -> Result<f32, String> {
    le::<4>(d, at).map(f32::from_le_bytes)
}

fn vector(d: &[u8], at: usize, n: usize) -> Result<Vec<f32>, String> {
    (0..n).map(|i| f32_at(d, at + i * 4)).collect()
}

/// Splits a packed pointer into (1-based block, offset).
fn unpack(ptr: u64) -> (usize, usize) {
    ((ptr & 0xFFF) as usize, ((ptr >> 12) & 0xFFFFF) as usize)
}

/// `<tag x="…" y="…" …/>` contents for a vector.
fn vector_attrs(name: &str, v: &[f32]) -> String {
    let mut s = name.to_owned();
    for (axis, value) in ["x", "y", "z", "w"].iter().zip(v) {
        s.push_str(&format!(" {axis}=\"{}\"", xml::float(*value)));
    }
    s
}

#[derive(Clone, Copy, PartialEq)]
enum Tag {
    None,
    Structure,
    Item,
    ItemAndType,
}

impl Meta {
    fn block(&self, id: usize) -> Option<&Block> {
        id.checked_sub(1).and_then(|i| self.blocks.get(i))
    }

    /// Names of the structures used, the root first; e.g. `CMapData`.
    pub fn root_type(&self) -> Option<String> {
        self.block(self.root_block).map(|b| names::field(b.name))
    }

    /// Reads `count` items of `size` bytes starting at a packed pointer; long
    /// arrays continue in the following blocks.
    fn array(&self, ptr: u64, count: usize, size: usize) -> Result<Vec<u8>, String> {
        let (mut id, mut off) = unpack(ptr);
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
            if let Err(e) =
                self.entry(w, inner, block_id, &block.data, offset + entry.offset, entry, &array_info, &ename, depth)
            {
                w.error(inner, format!("{ename}: {e}"));
            }
        }
        if let Some(t) = tag_name {
            w.close(level, if tag == Tag::Structure { t.split(' ').next().unwrap_or(&t) } else { "Item" });
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn entry(
        &self,
        w: &mut Writer,
        level: usize,
        block_id: usize,
        data: &[u8],
        at: usize,
        entry: &Entry,
        array_info: &Entry,
        ename: &str,
        depth: usize,
    ) -> Result<(), String> {
        let byte = |at: usize| data.get(at).copied().ok_or_else(|| format!("offset {at:#x} beyond block"));
        match entry.kind {
            ty::ARRAY => self.array_node(w, level, data, at, array_info, ename, depth)?,
            ty::ARRAY_OF_BYTES => {
                let n = entry.key as usize;
                let text = match array_info.kind {
                    ty::SBYTE => {
                        (0..n).map(|i| byte(at + i).map(|b| (b as i8).to_string())).collect::<Result<Vec<_>, _>>()?
                    }
                    ty::UBYTE => (0..n).map(|i| byte(at + i).map(|b| b.to_string())).collect::<Result<Vec<_>, _>>()?,
                    ty::SSHORT => (0..n)
                        .map(|i| u16_at(data, at + i * 2).map(|v| (v as i16).to_string()))
                        .collect::<Result<Vec<_>, _>>()?,
                    ty::USHORT => (0..n)
                        .map(|i| u16_at(data, at + i * 2).map(|v| v.to_string()))
                        .collect::<Result<Vec<_>, _>>()?,
                    ty::SINT => (0..n)
                        .map(|i| u32_at(data, at + i * 4).map(|v| (v as i32).to_string()))
                        .collect::<Result<Vec<_>, _>>()?,
                    ty::UINT => (0..n)
                        .map(|i| u32_at(data, at + i * 4).map(|v| v.to_string()))
                        .collect::<Result<Vec<_>, _>>()?,
                    ty::FLOAT => {
                        (0..n).map(|i| f32_at(data, at + i * 4).map(xml::float)).collect::<Result<Vec<_>, _>>()?
                    }
                    _ => vec![(0..n).map(|i| byte(at + i).map(|b| format!("{b:02X}"))).collect::<Result<String, _>>()?],
                };
                w.text(level, ename, &text.join(" "));
            }
            ty::ARRAY_OF_CHARS => {
                let n = entry.key as usize;
                let bytes = data.get(at..(at + n).min(data.len())).unwrap_or_default();
                let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
                w.text(level, ename, &String::from_utf8_lossy(&bytes[..end]));
            }
            ty::BOOLEAN => w.value(level, ename, if byte(at)? != 0 { "true" } else { "false" }),
            ty::BYTE_ENUM | ty::UBYTE => w.value(level, ename, &byte(at)?.to_string()),
            ty::SBYTE => w.value(level, ename, &(byte(at)? as i8).to_string()),
            ty::CHAR_POINTER => {
                let ptr = u64_at(data, at)?;
                let len = u16_at(data, at + 8)? as usize;
                let text = if ptr == 0 || len == 0 { String::new() } else { self.string(ptr, len)? };
                w.text(level, ename, &text);
            }
            ty::DATA_BLOCK_POINTER => {
                let (id, _) = unpack(u64_at(data, at)?);
                let bytes: Vec<String> = match self.block(id) {
                    Some(b) => b.data.iter().map(|b| format!("{b:02X}")).collect(),
                    None => Vec::new(),
                };
                w.raw_array(level, ename, &bytes, 32);
            }
            ty::FLOAT => w.value(level, ename, &xml::float(f32_at(data, at)?)),
            ty::FLOAT_XYZ => w.empty(level, &vector_attrs(ename, &vector(data, at, 3)?)),
            ty::FLOAT_XYZW => w.empty(level, &vector_attrs(ename, &vector(data, at, 4)?)),
            ty::HASH => w.string(level, ename, &names::value(u32_at(data, at)?)),
            ty::INT_ENUM | ty::INT_FLAGS1 | ty::INT_FLAGS2 => {
                let v = u32_at(data, at)? as i32;
                w.string(level, ename, &self.enum_string(entry, v));
            }
            ty::SHORT_FLAGS => {
                let v = u16_at(data, at)? as i16 as i32;
                w.string(level, ename, &self.enum_string(entry, v));
            }
            ty::SINT => w.value(level, ename, &(u32_at(data, at)? as i32).to_string()),
            ty::SSHORT => w.value(level, ename, &(u16_at(data, at)? as i16).to_string()),
            ty::USHORT => w.value(level, ename, &u16_at(data, at)?.to_string()),
            ty::UINT => {
                let v = u32_at(data, at)?;
                let text = if ename == "color" { format!("0x{v:08X}") } else { v.to_string() };
                w.value(level, ename, &text);
            }
            ty::STRUCTURE => {
                w.open(level, ename);
                self.node(w, level, block_id, at, Tag::None, entry.key, depth + 1);
                w.close(level, ename);
            }
            ty::STRUCTURE_POINTER => {
                let ptr = u64_at(data, at)?;
                let (id, off) = unpack(ptr);
                match self.block(id) {
                    Some(b) if ptr != 0 => {
                        w.open(level, &format!("{ename} type=\"{}\"", names::field(b.name)));
                        self.node(w, level, id, off, Tag::None, 0, depth + 1);
                        w.close(level, ename);
                    }
                    _ => w.empty(level, ename),
                }
            }
            other => return Err(format!("unexpected data type {other:#04x}")),
        }
        Ok(())
    }

    fn string(&self, ptr: u64, len: usize) -> Result<String, String> {
        let (id, off) = unpack(ptr);
        let block = self.block(id).ok_or_else(|| format!("string block {id} missing"))?;
        let bytes = block.data.get(off..(off + len).min(block.data.len())).ok_or("string beyond block")?;
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        Ok(String::from_utf8_lossy(&bytes[..end]).into_owned())
    }

    #[allow(clippy::too_many_arguments)]
    fn array_node(
        &self,
        w: &mut Writer,
        level: usize,
        data: &[u8],
        at: usize,
        item: &Entry,
        ename: &str,
        depth: usize,
    ) -> Result<(), String> {
        let ptr = u64_at(data, at)?;
        let count = u16_at(data, at + 8)? as usize;
        let numbers = |size: usize, fmt: &dyn Fn(&[u8]) -> String| -> Result<Vec<String>, String> {
            Ok(self.array(ptr, count, size)?.chunks_exact(size).map(fmt).collect())
        };
        match item.kind {
            ty::STRUCTURE => {
                let tag = format!("{ename} itemType=\"{}\"", names::field(item.key));
                if count == 0 {
                    w.empty(level, &tag);
                    return Ok(());
                }
                let info = self.structs.get(&item.key).ok_or("array item structure not found")?;
                let (mut id, mut off) = unpack(ptr);
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
            }
            ty::STRUCTURE_POINTER => {
                if count == 0 {
                    w.empty(level, ename);
                    return Ok(());
                }
                let ptrs = self.array(ptr, count, 8)?;
                w.open(level, ename);
                for p in ptrs.as_chunks::<8>().0 {
                    let p = u64::from_le_bytes(*p);
                    let (id, off) = unpack(p);
                    if p == 0 {
                        w.empty(level + 1, "Item");
                    } else {
                        self.node(w, level + 1, id, off, Tag::ItemAndType, 0, depth + 1);
                    }
                }
                w.close(level, ename);
            }
            ty::UINT => {
                let v = numbers(4, &|b| u32::from_le_bytes(b.try_into().unwrap_or_default()).to_string())?;
                w.raw_array(level, ename, &v, 10);
            }
            ty::USHORT => {
                let v = numbers(2, &|b| u16::from_le_bytes(b.try_into().unwrap_or_default()).to_string())?;
                w.raw_array(level, ename, &v, 10);
            }
            ty::UBYTE => {
                let v = numbers(1, &|b| b[0].to_string())?;
                w.raw_array(level, ename, &v, 10);
            }
            ty::FLOAT => {
                let v = numbers(4, &|b| xml::float(f32::from_le_bytes(b.try_into().unwrap_or_default())))?;
                w.raw_array(level, ename, &v, 10);
            }
            ty::FLOAT_XYZ => {
                let v = numbers(16, &|b| {
                    (0..4)
                        .map(|i| xml::float(f32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap_or_default())))
                        .collect::<Vec<_>>()
                        .join(", ")
                })?;
                w.item_array(level, ename, &v);
            }
            ty::HASH => {
                let v = numbers(4, &|b| names::value(u32::from_le_bytes(b.try_into().unwrap_or_default())))?;
                w.item_array(level, ename, &v);
            }
            other => return Err(format!("unexpected array item type {other:#04x}")),
        }
        Ok(())
    }
}
