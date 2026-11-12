//! PSO files (`PSIN` …): binary metadata used by `.ymt`, `.ymf`, `.cut`, `.pso`.
//!
//! Big-endian sections, each `ident, length` (length includes the 8-byte header):
//!
//! * `PSIN`: the data; block offsets are relative to the section start.
//! * `PMAP`: root block id (1-based), block count, then 16-byte blocks
//!   (structure name, offset, unused, length).
//! * `PSCH`: schema: count, (name, offset) index, then structure infos
//!   (`type<<24 | count`, length, unused, 12-byte entries: name, type, subtype,
//!   offset u16, reference key) and enum infos (`type<<24 | count`, (name, value)).
//! * `STRF`, `STRS`, `STRE`, `PSIG`, `CHKS`: strings and checksums (not needed).
//!
//! Pointers are 8 bytes; the first big-endian u32 holds the 1-based block
//! (bits 0–11) and the offset (bits 12–31).
//! Arrays are 16 bytes: pointer, count (u16), capacity (u16), unused.

use std::collections::HashMap;

use crate::names;
use crate::xml::{self, Writer};

pub const MAGIC: &[u8; 4] = b"PSIN";
const ARRAYINFO: u32 = 0x100;

#[derive(Debug, thiserror::Error)]
#[error("invalid PSO file: {0}")]
pub struct PsoError(String);

mod ty {
    pub const BOOL: u8 = 0x00;
    pub const SBYTE: u8 = 0x01;
    pub const UBYTE: u8 = 0x02;
    pub const SSHORT: u8 = 0x03;
    pub const USHORT: u8 = 0x04;
    pub const SINT: u8 = 0x05;
    pub const UINT: u8 = 0x06;
    pub const FLOAT: u8 = 0x07;
    pub const FLOAT2: u8 = 0x08;
    pub const FLOAT3: u8 = 0x09;
    pub const FLOAT4: u8 = 0x0a;
    pub const STRING: u8 = 0x0b;
    pub const STRUCTURE: u8 = 0x0c;
    pub const ARRAY: u8 = 0x0d;
    pub const ENUM: u8 = 0x0e;
    pub const FLAGS: u8 = 0x0f;
    pub const MAP: u8 = 0x10;
    /// RDR2: four floats in 16 bytes.
    pub const VEC4V: u8 = 0x13;
    /// RDR2: a 4×4 float matrix.
    pub const MATRIX44: u8 = 0x17;
    /// RDR2: a 64-bit unsigned integer.
    pub const ULONG: u8 = 0x1f;
    pub const FLOAT3A: u8 = 0x14;
    pub const FLOAT4A: u8 = 0x15;
    pub const HFLOAT: u8 = 0x1e;
    pub const LONG: u8 = 0x20;
    /// RDR2: 16 bytes of unknown meaning, written as hex.
    pub const OPAQUE16: u8 = 0x24;
}

#[derive(Debug, Clone, Copy, Default)]
struct Entry {
    name: u32,
    kind: u8,
    sub: u8,
    offset: usize,
    key: u32,
}

#[derive(Debug, Clone)]
struct StructInfo {
    length: usize,
    entries: Vec<Entry>,
}

impl StructInfo {
    fn find(&self, name: u32) -> Option<&Entry> {
        self.entries.iter().find(|e| e.name == name)
    }
}

#[derive(Debug, Clone, Copy)]
struct Block {
    name: u32,
    offset: usize,
    length: usize,
}

#[derive(Debug, Clone)]
pub struct Pso {
    data: Vec<u8>,
    pub root_block: usize,
    blocks: Vec<Block>,
    structs: HashMap<u32, StructInfo>,
    enums: HashMap<u32, Vec<(u32, i32)>>,
    /// Bit where the offset of a pointer starts: 12 (GTA V) or 20 (RDR2).
    offset_shift: u32,
}

pub fn is_pso(data: &[u8]) -> bool {
    data.starts_with(MAGIC)
}

fn be<const N: usize>(d: &[u8], at: usize) -> Result<[u8; N], String> {
    d.get(at..at + N)
        .and_then(|s| s.try_into().ok())
        .ok_or_else(|| format!("read of {N} bytes at {at:#x} beyond data of {:#x}", d.len()))
}

fn u16_at(d: &[u8], at: usize) -> Result<u16, String> {
    be::<2>(d, at).map(u16::from_be_bytes)
}

fn u32_at(d: &[u8], at: usize) -> Result<u32, String> {
    be::<4>(d, at).map(u32::from_be_bytes)
}

fn u64_at(d: &[u8], at: usize) -> Result<u64, String> {
    be::<8>(d, at).map(u64::from_be_bytes)
}

fn f32_at(d: &[u8], at: usize) -> Result<f32, String> {
    be::<4>(d, at).map(f32::from_be_bytes)
}

/// A pointer: the first big-endian word, plus the next two bytes as a
/// little-endian u16 in bits 32..48 (RDR2 keeps the offset's high bits there;
/// GTA V leaves them zero).
fn ptr_at(d: &[u8], at: usize) -> Result<u64, String> {
    let high = match (d.get(at + 4), d.get(at + 5)) {
        (Some(lo), Some(hi)) => u64::from(*lo) | u64::from(*hi) << 8,
        _ => 0,
    };
    u32_at(d, at).map(|w| u64::from(w) | high << 32)
}

/// Block id and offset of a pointer. GTA V: offset in bits 12..32. RDR2:
/// bits 20..32 are the low 12 offset bits, bits 32..48 the rest (offsets
/// past 1 MB in a block need more than the first byte of them).
fn unpack_with(ptr: u64, offset_shift: u32) -> (usize, usize) {
    let offset = if offset_shift == 20 {
        ((ptr >> 20) & 0xFFF) | ((ptr >> 32) & 0xFFFF) << 12
    } else {
        (ptr >> offset_shift) & 0xFFFFF
    };
    ((ptr & 0xFFF) as usize, offset as usize)
}

fn vector_attrs(name: &str, v: &[f32]) -> String {
    let mut s = name.to_owned();
    for (axis, value) in ["x", "y", "z", "w"].iter().zip(v) {
        s.push_str(&format!(" {axis}=\"{}\"", xml::float(*value)));
    }
    s
}

pub fn parse(file: &[u8]) -> Result<Pso, PsoError> {
    parse_with(file, 12)
}

/// A PSO file from Red Dead Redemption 2, whose pointers keep the offset from bit 20.
pub fn parse_rdr2(file: &[u8]) -> Result<Pso, PsoError> {
    parse_with(file, 20)
}

fn parse_with(file: &[u8], offset_shift: u32) -> Result<Pso, PsoError> {
    let err = |m: String| PsoError(m);
    let mut data = None;
    let mut map = None;
    let mut schema = None;
    let mut pos = 0;
    while pos + 8 <= file.len() {
        let ident: [u8; 4] = file[pos..pos + 4].try_into().unwrap_or_default();
        let len = u32_at(file, pos + 4).map_err(err)? as usize;
        if len < 8 || pos + len > file.len() {
            return Err(err(format!("section {} has bad length {len}", String::from_utf8_lossy(&ident))));
        }
        let section = &file[pos..pos + len];
        match &ident {
            b"PSIN" => data = Some(section),
            b"PMAP" => map = Some(section),
            b"PSCH" => schema = Some(section),
            _ => {}
        }
        pos += len;
    }
    let data = data.ok_or_else(|| err("no PSIN section".into()))?;
    let map = map.ok_or_else(|| err("no PMAP section".into()))?;
    let schema = schema.ok_or_else(|| err("no PSCH section".into()))?;

    let root_block = u32_at(map, 8).map_err(err)? as usize;
    let mut count = u16_at(map, 12).map_err(err)? as i16;
    let mut at = 16;
    if count <= 0 {
        count = u16_at(map, 16).map_err(err)? as i16;
        at = 24;
    }
    let mut blocks = Vec::with_capacity(count.max(0) as usize);
    for i in 0..count.max(0) as usize {
        let p = at + i * 16;
        blocks.push(Block {
            name: u32_at(map, p).map_err(err)?,
            offset: u32_at(map, p + 4).map_err(err)? as usize,
            length: u32_at(map, p + 12).map_err(err)? as usize,
        });
    }

    let mut structs = HashMap::new();
    let mut enums = HashMap::new();
    let n = u32_at(schema, 8).map_err(err)? as usize;
    for i in 0..n.min(0x10000) {
        let name = u32_at(schema, 12 + i * 8).map_err(err)?;
        let off = u32_at(schema, 16 + i * 8).map_err(err)? as usize;
        let x = u32_at(schema, off).map_err(err)?;
        match x >> 24 {
            0 => {
                let length = u32_at(schema, off + 4).map_err(err)? as usize;
                let mut entries = Vec::new();
                for e in 0..(x & 0xFFFF) as usize {
                    let p = off + 12 + e * 12;
                    entries.push(Entry {
                        name: u32_at(schema, p).map_err(err)?,
                        kind: *schema.get(p + 4).ok_or_else(|| err("schema entry beyond section".into()))?,
                        sub: *schema.get(p + 5).ok_or_else(|| err("schema entry beyond section".into()))?,
                        offset: u16_at(schema, p + 6).map_err(err)? as usize,
                        key: u32_at(schema, p + 8).map_err(err)?,
                    });
                }
                structs.entry(name).or_insert(StructInfo { length, entries });
            }
            1 => {
                let mut values = Vec::new();
                for e in 0..(x & 0xFF_FFFF) as usize {
                    let p = off + 4 + e * 8;
                    values.push((u32_at(schema, p).map_err(err)?, u32_at(schema, p + 4).map_err(err)? as i32));
                }
                enums.entry(name).or_insert(values);
            }
            t => return Err(err(format!("unknown schema element type {t}"))),
        }
    }
    Ok(Pso { data: data.to_vec(), root_block, blocks, structs, enums, offset_shift })
}

/// Parses a PSO file and writes it as XML (see [`Pso::to_xml`]).
pub fn to_xml(file: &[u8]) -> Result<(String, Vec<String>), PsoError> {
    Ok(parse(file)?.to_xml())
}

#[derive(Clone, Copy, PartialEq)]
enum Tag {
    None,
    Structure,
    Item,
    ItemAndType,
}

impl Pso {
    /// Splits a pointer into (block, offset). RDR2's files use both layouts
    /// (offset from bit 20 in most, from bit 12 in some arrays): the RDR2
    /// layout is taken when it lands inside a block, else the GTA V one.
    fn unpack(&self, ptr: u64) -> (usize, usize) {
        let (block, offset) = unpack_with(ptr, self.offset_shift);
        if self.offset_shift == 12 || self.block(block).is_some_and(|b| offset < b.length.max(1)) {
            return (block, offset);
        }
        let (b12, o12) = unpack_with(ptr, 12);
        if self.block(b12).is_some_and(|b| o12 < b.length.max(1)) { (b12, o12) } else { (block, offset) }
    }

    fn block(&self, id: usize) -> Option<&Block> {
        id.checked_sub(1).and_then(|i| self.blocks.get(i))
    }

    /// The root structure's name, e.g. `CPedModelInfo__InitDataList`.
    pub fn root_type(&self) -> Option<String> {
        self.block(self.root_block).map(|b| names::field(b.name))
    }

    /// The file as XML, formatted like CodeWalker's export, plus problems found.
    pub fn to_xml(&self) -> (String, Vec<String>) {
        self.to_xml_with_limit(crate::xml::MAX_OUTPUT)
    }

    /// Like [`Pso::to_xml`] with another output limit.
    pub fn to_xml_with_limit(&self, limit: usize) -> (String, Vec<String>) {
        let mut w = Writer::with_limit(limit);
        self.node(&mut w, 0, self.root_block, 0, Tag::Structure, 0, 0);
        w.finish()
    }

    fn enum_name(&self, key: u32, value: i32) -> String {
        self.enums
            .get(&key)
            .and_then(|v| v.iter().find(|(_, x)| *x == value))
            .map(|(n, _)| names::field(*n))
            .unwrap_or_default()
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
        let Some(block) = self.block(block_id).copied() else {
            return w.error(level, format!("block {block_id} not found"));
        };
        if depth > 64 {
            return w.error(level, "structures nested too deeply".into());
        }
        // Some offsets are stored shifted by 8 bits.
        let offset = if offset >= block.length { offset >> 8 } else { offset };
        let struct_name = if struct_name == 0 { block.name } else { struct_name };
        let name = names::field(struct_name);
        let Some(info) = self.structs.get(&struct_name) else {
            return w.error(level, format!("structure info {name} not found"));
        };
        let tag_name = match tag {
            Tag::None => None,
            Tag::Structure => Some(name.clone()),
            Tag::Item => Some("Item".to_owned()),
            Tag::ItemAndType => Some(format!("Item type=\"{name}\"")),
        };
        if let Some(t) = &tag_name {
            w.open(level, t);
        }
        let inner = level + 1;
        for entry in &info.entries {
            if entry.name == ARRAYINFO {
                continue;
            }
            let ename = names::field(entry.name);
            if let Err(e) = self.entry(w, inner, block_id, block, offset, entry, info, &ename, depth) {
                w.error(inner, format!("{ename}: {e}"));
            }
        }
        if let Some(t) = tag_name {
            w.close(level, if tag == Tag::Structure { &t } else { "Item" });
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn entry(
        &self,
        w: &mut Writer,
        level: usize,
        block_id: usize,
        block: Block,
        offset: usize,
        entry: &Entry,
        info: &StructInfo,
        ename: &str,
        depth: usize,
    ) -> Result<(), String> {
        let d = &self.data[..];
        let at = block.offset + offset + entry.offset;
        let byte = |at: usize| d.get(at).copied().ok_or_else(|| format!("offset {at:#x} beyond data"));
        match entry.kind {
            ty::ARRAY => self.array_node(w, level, block_id, offset, entry, info, ename, depth)?,
            ty::BOOL => w.value(level, ename, if byte(at)? != 0 { "true" } else { "false" }),
            ty::SBYTE => w.value(level, ename, &(byte(at)? as i8).to_string()),
            ty::UBYTE => w.value(level, ename, &byte(at)?.to_string()),
            ty::ENUM => {
                // RDR2 counts the subtypes one higher: 1 = 32 bit, 2 = 16 bit, 3 = 8 bit.
                let sub = if self.offset_shift == 20 { entry.sub.saturating_sub(1) } else { entry.sub };
                let v = match sub {
                    0 => u32_at(d, at)? as i32,
                    1 => u16_at(d, at)? as i16 as i32,
                    2 => byte(at)? as i32,
                    s => return Err(format!("unexpected enum subtype {s}")),
                };
                w.string(level, ename, &self.enum_name(entry.key, v));
            }
            ty::FLAGS => {
                let index = (entry.key & 0xFFF) as usize;
                let flags = (index != 0xFFF)
                    .then(|| info.entries.get(index))
                    .flatten()
                    .filter(|e| e.name == ARRAYINFO)
                    .and_then(|e| self.enums.get(&e.key));
                // Bits as 32-bit words; RDR2's subtype 4 is a bit set whose
                // size (in bits) is the high half of the entry's key.
                let words: Vec<u32> = match entry.sub {
                    0 => vec![u32_at(d, at)?],
                    1 => vec![u16_at(d, at)? as u32],
                    2 => vec![byte(at)? as u32],
                    4 => {
                        let bits = (entry.key >> 16).clamp(1, 4096) as usize;
                        (0..bits.div_ceil(32)).map(|i| u32_at(d, at + i * 4)).collect::<Result<_, _>>()?
                    }
                    s => return Err(format!("unexpected flags subtype {s}")),
                };
                let bit_set = |bit: i32| {
                    usize::try_from(bit)
                        .ok()
                        .is_some_and(|b| words.get(b / 32).is_some_and(|w| w & (1 << (b % 32)) != 0))
                };
                match flags {
                    Some(values) => {
                        let set: Vec<String> =
                            values.iter().filter(|(_, bit)| bit_set(*bit)).map(|(n, _)| names::field(*n)).collect();
                        w.string(level, ename, &set.join(" "));
                    }
                    None if words.len() == 1 && words[0] != 0 => w.value(level, ename, &words[0].to_string()),
                    None if words.iter().any(|&x| x != 0) => {
                        let hex: Vec<String> = words.iter().map(|x| format!("{x:08X}")).collect();
                        w.value(level, ename, &hex.join(" "));
                    }
                    None => w.empty(level, ename),
                }
            }
            ty::FLOAT => w.value(level, ename, &xml::float(f32_at(d, at)?)),
            ty::FLOAT2 => w.empty(level, &vector_attrs(ename, &[f32_at(d, at)?, f32_at(d, at + 4)?])),
            ty::FLOAT3 | ty::FLOAT3A | ty::FLOAT4A => {
                w.empty(level, &vector_attrs(ename, &[f32_at(d, at)?, f32_at(d, at + 4)?, f32_at(d, at + 8)?]))
            }
            ty::FLOAT4 | ty::VEC4V => w.empty(
                level,
                &vector_attrs(ename, &[f32_at(d, at)?, f32_at(d, at + 4)?, f32_at(d, at + 8)?, f32_at(d, at + 12)?]),
            ),
            ty::SINT => w.value(level, ename, &(u32_at(d, at)? as i32).to_string()),
            ty::UINT => match entry.sub {
                0 => w.value(level, ename, &(u32_at(d, at)? as i32).to_string()),
                1 => w.value(level, ename, &format!("0x{:08X}", u32_at(d, at)?)),
                s => return Err(format!("unexpected integer subtype {s}")),
            },
            ty::LONG => w.value(level, ename, &u64_at(d, at)?.to_string()),
            ty::MAP => self.map_node(w, level, at, entry, info, ename, depth)?,
            ty::MATRIX44 => {
                let m: Vec<String> =
                    (0..16).map(|i| f32_at(d, at + i * 4).map(xml::float)).collect::<Result<_, _>>()?;
                let rows: Vec<String> = m.chunks(4).map(|r| r.join(" ")).collect();
                w.raw_array(level, ename, &rows, 1);
            }
            ty::ULONG => w.value(level, ename, &u64_at(d, at)?.to_string()),
            ty::SSHORT => w.value(level, ename, &(u16_at(d, at)? as i16).to_string()),
            ty::USHORT => w.value(level, ename, &u16_at(d, at)?.to_string()),
            ty::HFLOAT => w.value(level, ename, &(u16_at(d, at)? as i16).to_string()),
            ty::STRING => {
                let s = self.string_value(entry, at)?;
                w.string(level, ename, &s);
            }
            ty::STRUCTURE => match entry.sub {
                0 => {
                    w.open(level, ename);
                    self.node(w, level, block_id, offset + entry.offset, Tag::None, entry.key, depth + 1);
                    w.close(level, ename);
                }
                3 | 4 => {
                    let (id, off) = self.unpack(ptr_at(d, at)?);
                    match self.block(id) {
                        Some(b) => {
                            // The pointed-to block's type is the actual (derived) type.
                            let actual = if self.structs.contains_key(&b.name) { b.name } else { entry.key };
                            w.open(level, &format!("{ename} type=\"{}\"", names::field(actual)));
                            self.node(w, level, id, off, Tag::None, actual, depth + 1);
                            w.close(level, ename);
                        }
                        None => w.empty(level, ename),
                    }
                }
                s => return Err(format!("unexpected structure subtype {s}")),
            },
            ty::OPAQUE16 => {
                let b = d.get(at..at + 16).ok_or("16 bytes beyond the data")?;
                w.value(level, ename, &b.iter().map(|x| format!("{x:02X}")).collect::<String>());
            }
            other => return Err(format!("unexpected data type {other:#04x}")),
        }
        Ok(())
    }

    fn string_at(&self, block_id: usize, offset: usize, len: Option<usize>) -> Option<String> {
        let block = self.block(block_id)?;
        let start = block.offset + offset;
        let rest = self.data.get(start..)?;
        let rest = match len {
            Some(n) => rest.get(..n.min(rest.len()))?,
            None => rest,
        };
        let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
        Some(String::from_utf8_lossy(&rest[..end]).into_owned())
    }

    fn string_value(&self, entry: &Entry, at: usize) -> Result<String, String> {
        let d = &self.data[..];
        Ok(match entry.sub {
            0 => {
                let n = (entry.key >> 16) as usize;
                let bytes = d.get(at..(at + n).min(d.len())).unwrap_or_default();
                let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
                String::from_utf8_lossy(&bytes[..end]).into_owned()
            }
            1 | 2 => {
                let (id, off) = self.unpack(ptr_at(d, at)?);
                self.string_at(id, off, None).unwrap_or_default()
            }
            3 => {
                let (id, off) = self.unpack(ptr_at(d, at)?);
                let (c1, c2) = (u16_at(d, at + 8)? as usize, u16_at(d, at + 10)? as usize);
                let len = if c2 != 0 { c1.min(c2) } else { c1 };
                if len == 0 { String::new() } else { self.string_at(id, off, Some(len)).unwrap_or_default() }
            }
            // Hashes (RDR2 adds more hash string kinds, 11 and 13).
            7..=15 => names::value(u32_at(d, at)?),
            s => return Err(format!("unexpected string subtype {s}")),
        })
    }

    /// Reads `count` items of `size` bytes at an absolute data offset.
    fn items(&self, at: usize, count: usize, size: usize) -> Result<Vec<&[u8]>, String> {
        let bytes =
            self.data.get(at..at + count * size).ok_or_else(|| format!("array of {count} at {at:#x} beyond data"))?;
        Ok(bytes.chunks_exact(size.max(1)).collect())
    }

    #[allow(clippy::too_many_arguments)]
    fn array_node(
        &self,
        w: &mut Writer,
        level: usize,
        block_id: usize,
        offset: usize,
        entry: &Entry,
        info: &StructInfo,
        ename: &str,
        depth: usize,
    ) -> Result<(), String> {
        let d = &self.data[..];
        let block = *self.block(block_id).ok_or("block missing")?;
        let at = block.offset + offset + entry.offset;
        let mut index = (entry.key & 0xFFFF) as usize;
        if index >= info.entries.len() {
            index = (entry.key & 0xFFF) as usize;
        }
        let item = *info.entries.get(index).ok_or_else(|| format!("ARRAYINFO not found for {ename}"))?;
        // Where the items are: (block, offset in block, absolute offset, count).
        let mut a_block = block_id;
        let mut a_off = offset + entry.offset;
        let mut abs = at;
        let mut count = (entry.key >> 16) as usize;
        // String, hash and enum arrays always point elsewhere.
        let external = matches!(entry.sub, 0 | 5)
            || (entry.sub == 4 && item.sub == 3)
            || matches!(item.kind, ty::STRING | ty::ENUM);
        if external {
            let (id, off) = self.unpack(ptr_at(d, at)?);
            a_block = id;
            a_off = off;
            count = u16_at(d, at + 8)? as usize;
            if let Some(b) = self.block(id) {
                abs = b.offset + off;
            }
        } else if !matches!(entry.sub, 1 | 2 | 4 | 129) {
            return Err(format!("unsupported array subtype {}", entry.sub));
        }
        let u32s = |items: Vec<&[u8]>| -> Vec<u32> {
            items.iter().map(|b| u32::from_be_bytes(b[..4].try_into().unwrap_or_default())).collect()
        };
        let floats = |b: &[u8], n: usize| -> String {
            (0..n)
                .map(|i| xml::float(f32::from_be_bytes(b[i * 4..i * 4 + 4].try_into().unwrap_or_default())))
                .collect::<Vec<_>>()
                .join(", ")
        };
        match item.kind {
            ty::ARRAY => {
                let n = (entry.key >> 16) as usize;
                if n == 0 {
                    w.empty(level, ename);
                } else {
                    w.open(level, ename);
                    let mut off = offset + entry.offset;
                    for _ in 0..n {
                        let inner = Entry { offset: 0, ..item };
                        self.array_node(w, level + 1, block_id, off, &inner, info, "Item", depth + 1)?;
                        off += 16;
                    }
                    w.close(level, ename);
                }
            }
            ty::STRUCTURE if matches!(item.sub, 3 | 4) => {
                if count == 0 {
                    w.empty(level, ename);
                    return Ok(());
                }
                let ptrs = self.items(abs, count, 8)?;
                w.open(level, ename);
                for p in ptrs {
                    let p = ptr_at(p, 0).unwrap_or(0);
                    if p == 0 {
                        w.empty(level + 1, "Item");
                    } else {
                        let (id, off) = self.unpack(p);
                        self.node(w, level + 1, id, off, Tag::ItemAndType, 0, depth + 1);
                    }
                }
                w.close(level, ename);
            }
            ty::STRUCTURE => {
                let tag = format!("{ename} itemType=\"{}\"", names::field(item.key));
                if count == 0 {
                    w.empty(level, &tag);
                    return Ok(());
                }
                let a = *self.block(a_block).ok_or_else(|| format!("array block {a_block} not found"))?;
                let st = self.structs.get(&item.key).ok_or("array item structure not found")?;
                w.open(level, &tag);
                let mut off = a_off;
                for n in 0..count {
                    self.node(w, level + 1, a_block, off, Tag::Item, item.key, depth + 1);
                    off += st.length;
                    if n + 1 < count && off >= a.length {
                        break;
                    }
                }
                w.close(level, ename);
            }
            ty::STRING => match item.sub {
                7..=15 => {
                    let v: Vec<String> = u32s(self.items(abs, count, 4)?).into_iter().map(names::value).collect();
                    w.item_array(level, ename, &v);
                }
                2 | 3 => {
                    // Data block pointers are 8 bytes, char pointers 16 (with a length).
                    let size = if item.sub == 2 { 8 } else { 16 };
                    let v: Vec<String> = self
                        .items(abs, count, size)?
                        .into_iter()
                        .map(|b| {
                            let (id, off) = self.unpack(ptr_at(b, 0).unwrap_or(0));
                            let len = (item.sub == 3).then(|| u16::from_be_bytes([b[8], b[9]]) as usize);
                            xml::escape(&self.string_at(id, off, len).unwrap_or_default())
                        })
                        .collect();
                    w.item_array(level, ename, &v);
                }
                s => return Err(format!("unexpected string array subtype {s}")),
            },
            ty::FLOAT2 => {
                let v: Vec<String> = self.items(abs, count, 8)?.into_iter().map(|b| floats(b, 2)).collect();
                w.raw_array(level, ename, &v, 1);
            }
            ty::FLOAT3 | ty::FLOAT3A => {
                let v: Vec<String> = self.items(abs, count, 16)?.into_iter().map(|b| floats(b, 3)).collect();
                w.raw_array(level, ename, &v, 1);
            }
            ty::FLOAT4 | ty::FLOAT4A | ty::VEC4V => {
                let v: Vec<String> = self.items(abs, count, 16)?.into_iter().map(|b| floats(b, 4)).collect();
                w.raw_array(level, ename, &v, 1);
            }
            ty::LONG | ty::ULONG => {
                let v: Vec<String> = self
                    .items(abs, count, 8)?
                    .into_iter()
                    .map(|b| {
                        let x = u64::from_be_bytes(b[..8].try_into().unwrap_or_default());
                        if item.kind == ty::LONG { (x as i64).to_string() } else { x.to_string() }
                    })
                    .collect();
                w.raw_array(level, ename, &v, 10);
            }
            ty::SBYTE => {
                let v: Vec<String> = self.items(abs, count, 1)?.into_iter().map(|b| (b[0] as i8).to_string()).collect();
                w.raw_array(level, ename, &v, 10);
            }
            ty::UBYTE | ty::BOOL => {
                let v: Vec<String> = self.items(abs, count, 1)?.into_iter().map(|b| b[0].to_string()).collect();
                w.raw_array(level, ename, &v, 10);
            }
            ty::FLOAT => {
                let v: Vec<String> = self.items(abs, count, 4)?.into_iter().map(|b| floats(b, 1)).collect();
                w.raw_array(level, ename, &v, 10);
            }
            ty::USHORT => {
                let v: Vec<String> = self
                    .items(abs, count, 2)?
                    .into_iter()
                    .map(|b| u16::from_be_bytes([b[0], b[1]]).to_string())
                    .collect();
                w.raw_array(level, ename, &v, 10);
            }
            ty::UINT | ty::SINT => {
                let v: Vec<String> = u32s(self.items(abs, count, 4)?).into_iter().map(|x| x.to_string()).collect();
                w.raw_array(level, ename, &v, 10);
            }
            ty::ENUM => {
                let v: Vec<String> =
                    u32s(self.items(abs, count, 4)?).into_iter().map(|x| self.enum_name(item.key, x as i32)).collect();
                w.item_array(level, ename, &v);
            }
            other => return Err(format!("unsupported array item type {other:#04x}")),
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn map_node(
        &self,
        w: &mut Writer,
        level: usize,
        at: usize,
        entry: &Entry,
        info: &StructInfo,
        ename: &str,
        depth: usize,
    ) -> Result<(), String> {
        if entry.sub != 1 {
            return Err(format!("unexpected map subtype {}", entry.sub));
        }
        let d = &self.data[..];
        let x1 = u32_at(d, at)?;
        // RDR2 leaves the rest of the first 8 bytes uninitialised: only the first byte counts.
        let pointer_first = x1 == 0x0100_0000 || (self.offset_shift == 20 && x1 >> 24 == 1);
        let (ptr, count) = if !pointer_first {
            (ptr_at(d, at + 16)?, u16_at(d, at + 2)? as usize)
        } else {
            (ptr_at(d, at + 8)?, u16_at(d, at + 16)? as usize)
        };
        let (id, mut off) = self.unpack(ptr);
        if count == 0 {
            w.empty(level, ename);
            return Ok(());
        }
        let block = *self.block(id).ok_or_else(|| format!("map block {id} not found"))?;
        if off >= block.length {
            off >>= 8;
        }
        // GTA V describes the pairs by a structure with Key and Item fields.
        // RDR2 describes them by two type entries of the owning structure
        // (the map entry's key: key type index << 16 | item type index), and
        // stores each pair as the key (4 bytes, padded to 8) and the item.
        let pair_struct = self
            .structs
            .get(&block.name)
            .filter(|st| st.find(names::parse("Key")).is_some() && st.find(names::parse("Item")).is_some());
        let (key_entry, item_entry, pair_len) = match pair_struct {
            Some(st) => (
                *st.find(names::parse("Key")).expect("checked"),
                *st.find(names::parse("Item")).expect("checked"),
                st.length,
            ),
            None => {
                let key_info = *info.entries.get((entry.key >> 16) as usize & 0xFFF).ok_or("map Key entry missing")?;
                let mut item_info = *info.entries.get(entry.key as usize & 0xFFF).ok_or("map Item entry missing")?;
                item_info.offset = 8;
                let mut key_info = key_info;
                key_info.offset = 0;
                let item_size = match (item_info.kind, item_info.sub) {
                    (ty::STRUCTURE, 0) => self.structs.get(&item_info.key).map_or(8, |s| s.length),
                    (ty::STRING, 7 | 8) | (ty::UINT | ty::SINT | ty::ENUM | ty::FLOAT, _) => 4,
                    _ => 8,
                };
                (key_info, item_info, (8 + item_size).next_multiple_of(8))
            }
        };
        w.open(level, ename);
        for _ in 0..count {
            if off >= block.length {
                w.error(level + 1, format!("map offset out of range (count {count})"));
                break;
            }
            let base = block.offset + off;
            let key_at = base + key_entry.offset;
            let key = match key_entry.kind {
                ty::STRING => self.string_value(&key_entry, key_at)?,
                ty::ENUM | ty::UINT | ty::SINT => names::value(u32_at(d, key_at)?),
                _ => format!("{:#x}", u32_at(d, key_at)?),
            };
            let key = xml::escape(&key);
            match item_entry.kind {
                ty::STRING => {
                    let s = self.string_value(&item_entry, base + item_entry.offset)?;
                    w.text(level + 1, &format!("Item type=\"String\" key=\"{key}\""), &s);
                }
                ty::STRUCTURE if item_entry.key != 0 && item_entry.sub != 3 => {
                    w.open(level + 1, &format!("Item type=\"{}\" key=\"{key}\"", names::field(item_entry.key)));
                    self.node(w, level + 1, id, off + item_entry.offset, Tag::None, item_entry.key, depth + 1);
                    w.close(level + 1, "Item");
                }
                ty::STRUCTURE => {
                    let (pid, poff) = self.unpack(ptr_at(d, base + item_entry.offset)?);
                    let pb = self.block(pid).ok_or("map item block not found")?;
                    w.open(level + 1, &format!("Item type=\"{}\" key=\"{key}\"", names::field(pb.name)));
                    if self.structs.get(&pb.name).is_some_and(|s| !s.entries.is_empty()) {
                        self.node(w, level + 1, pid, poff, Tag::None, 0, depth + 1);
                    }
                    w.close(level + 1, "Item");
                }
                _ => {
                    w.open(level + 1, &format!("Item key=\"{key}\""));
                    if let Err(e) = self.entry(w, level + 2, id, block, off, &item_entry, info, "Value", depth + 1) {
                        w.error(level + 2, format!("map value: {e}"));
                    }
                    w.close(level + 1, "Item");
                }
            }
            off += pair_len;
        }
        w.close(level, ename);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rdr2_pointer_offsets_past_one_megabyte() {
        // assets_albedo.ymt: block 1, offset 0x123F90 (low 12 bits in the
        // first word, the rest as a little-endian u16 after it).
        let raw = [0xF9, 0x00, 0x00, 0x01, 0x23, 0x01, 0x00, 0x00];
        assert_eq!(unpack_with(ptr_at(&raw, 0).unwrap(), 20), (1, 0x12_3F90));
        let small = [0x13, 0x80, 0x00, 0x01, 0xE4, 0x00, 0x00, 0x00];
        assert_eq!(unpack_with(ptr_at(&small, 0).unwrap(), 20), (1, 0xE_4138));
        // GTA V: offset in bits 12..32 of the first word only.
        let gta = [0x00, 0x12, 0x30, 0x02, 0x00, 0x00, 0x00, 0x00];
        assert_eq!(unpack_with(ptr_at(&gta, 0).unwrap(), 12), (2, 0x123));
    }
}
