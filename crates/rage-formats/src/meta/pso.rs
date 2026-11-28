//! PSO files (`PSIN` …): binary metadata used by `.ymt`, `.ymf`, `.cut`, `.pso`.
//!
//! Big-endian sections, each `ident, length` (length includes the 8-byte header):
//!
//! * `PSIN`: the data; block offsets are relative to the section start.
//! * `PMAP`: root block id (1-based), block count, then 16-byte blocks
//!   (structure name, offset, unused, length).
//! * `PSCH`: schema: count, (name, offset) index, then structure infos
//!   (`type<<24 | count`, length, unused, 12-byte entries: name, type, subtype,
//!   offset u16, reference key) and enum infos (`type<<24 | count`, (name, value);
//!   RDR2 has 8 zero bytes between name and value).
//! * `STRF`, `STRS`, `STRE`, `PSIG`, `CHKS`: strings and checksums (not needed
//!   to read; kept as they are when the file is written back, see [`Pso::from_xml`]).
//!
//! Pointers are 8 bytes; the first big-endian u32 holds the 1-based block
//! (bits 0–11) and the offset (bits 12–31).
//! Arrays are 16 bytes: pointer, count (u16), capacity (u16), unused.

use rage_core::bytes::BeRead;
use std::collections::HashMap;

use crate::names;
use crate::xml::{self, Writer};

mod write;

pub const MAGIC: &[u8; 4] = b"PSIN";
const ARRAYINFO: u32 = 0x100;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PsoError {
    #[error("invalid PSO file: {0}")]
    Invalid(String),
}

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
    /// Every section of the file as it was (ident and whole section), in order.
    sections: Vec<([u8; 4], Vec<u8>)>,
    /// Structures that describe the key/item pairs of maps (GTA V), with
    /// their names; one per map type, all usually named `0x100`.
    pairs: Vec<(u32, StructInfo)>,
}

pub fn is_pso(data: &[u8]) -> bool {
    data.starts_with(MAGIC)
}

fn be<T>(d: &[u8], at: usize, n: usize, v: Option<T>) -> Result<T, String> {
    v.ok_or_else(|| format!("read of {n} bytes at {at:#x} beyond data of {:#x}", d.len()))
}

fn u16_at(d: &[u8], at: usize) -> Result<u16, String> {
    be(d, at, 2, d.u16_be_at(at))
}

fn u32_at(d: &[u8], at: usize) -> Result<u32, String> {
    be(d, at, 4, d.u32_be_at(at))
}

fn u64_at(d: &[u8], at: usize) -> Result<u64, String> {
    be(d, at, 8, d.u64_be_at(at))
}

fn f32_at(d: &[u8], at: usize) -> Result<f32, String> {
    be(d, at, 4, d.f32_be_at(at))
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
    read_pso(file, offset_shift).map_err(PsoError::Invalid)
}

/// The sections of a file: the three that are read, and every one as it was.
struct Sections<'a> {
    data: &'a [u8],
    map: &'a [u8],
    schema: &'a [u8],
    all: Vec<([u8; 4], Vec<u8>)>,
}

fn section_length(file: &[u8], pos: usize, ident: &[u8; 4]) -> Result<usize, String> {
    let len = u32_at(file, pos + 4)? as usize;
    if len < 8 || pos + len > file.len() {
        return Err(format!("section {} has bad length {len}", String::from_utf8_lossy(ident)));
    }
    Ok(len)
}

fn split_sections(file: &[u8]) -> Result<Sections<'_>, String> {
    let mut data = None;
    let mut map = None;
    let mut schema = None;
    let mut all = Vec::new();
    let mut pos = 0;
    while pos + 8 <= file.len() {
        let ident: [u8; 4] = file[pos..pos + 4].try_into().unwrap_or_default();
        let len = section_length(file, pos, &ident)?;
        let section = &file[pos..pos + len];
        all.push((ident, section.to_vec()));
        match &ident {
            b"PSIN" => data = Some(section),
            b"PMAP" => map = Some(section),
            b"PSCH" => schema = Some(section),
            _ => {}
        }
        pos += len;
    }
    let data = data.ok_or("no PSIN section")?;
    let map = map.ok_or("no PMAP section")?;
    let schema = schema.ok_or("no PSCH section")?;
    Ok(Sections { data, map, schema, all })
}

/// The root block's number and the block table of the `PMAP` section.
fn read_blocks(map: &[u8]) -> Result<(usize, Vec<Block>), String> {
    let root_block = u32_at(map, 8)? as usize;
    let mut count = u16_at(map, 12)? as i16;
    let mut at = 16;
    if count <= 0 {
        count = u16_at(map, 16)? as i16;
        at = 24;
    }
    let mut blocks = Vec::with_capacity(count.max(0) as usize);
    for i in 0..count.max(0) as usize {
        let p = at + i * 16;
        blocks.push(Block {
            name: u32_at(map, p)?,
            offset: u32_at(map, p + 4)? as usize,
            length: u32_at(map, p + 12)? as usize,
        });
    }
    Ok((root_block, blocks))
}

fn read_schema_entry(schema: &[u8], p: usize) -> Result<Entry, String> {
    let beyond = || "schema entry beyond section".to_owned();
    Ok(Entry {
        name: u32_at(schema, p)?,
        kind: *schema.get(p + 4).ok_or_else(beyond)?,
        sub: *schema.get(p + 5).ok_or_else(beyond)?,
        offset: u16_at(schema, p + 6)? as usize,
        key: u32_at(schema, p + 8)?,
    })
}

/// A structure of the schema at `off`, whose first word is `x`.
fn read_struct_info(schema: &[u8], off: usize, x: u32) -> Result<StructInfo, String> {
    let length = u32_at(schema, off + 4)? as usize;
    let mut entries = Vec::new();
    for e in 0..(x & 0xFFFF) as usize {
        entries.push(read_schema_entry(schema, off + 12 + e * 12)?);
    }
    Ok(StructInfo { length, entries })
}

/// An enum of the schema at `off`, whose first word is `x`, as (name, value) pairs.
fn read_enum_values(schema: &[u8], off: usize, x: u32, offset_shift: u32) -> Result<Vec<(u32, i32)>, String> {
    // GTA V: (name, value) pairs; RDR2: name, 8 zero bytes, value.
    let (size, value_at) = if offset_shift == 20 { (16, 12) } else { (8, 4) };
    let mut values = Vec::new();
    for e in 0..(x & 0xFF_FFFF) as usize {
        let p = off + 4 + e * size;
        values.push((u32_at(schema, p)?, u32_at(schema, p + value_at)? as i32));
    }
    Ok(values)
}

/// The structures and enums of the `PSCH` section, and the structures that describe map pairs.
struct Schema {
    structs: HashMap<u32, StructInfo>,
    enums: HashMap<u32, Vec<(u32, i32)>>,
    pairs: Vec<(u32, StructInfo)>,
}

impl Schema {
    /// Adds the element at `off`: a structure (also to `pairs` when it has
    /// the Key and Item fields `pair_names`) or an enum.
    fn add(
        &mut self,
        schema: &[u8],
        (name, off): (u32, usize),
        offset_shift: u32,
        pair_names: (u32, u32),
    ) -> Result<(), String> {
        let x = u32_at(schema, off)?;
        match x >> 24 {
            0 => {
                let info = read_struct_info(schema, off, x)?;
                if info.find(pair_names.0).is_some() && info.find(pair_names.1).is_some() {
                    self.pairs.push((name, info.clone()));
                }
                self.structs.entry(name).or_insert(info);
            }
            1 => {
                self.enums.entry(name).or_insert(read_enum_values(schema, off, x, offset_shift)?);
            }
            t => return Err(format!("unknown schema element type {t}")),
        }
        Ok(())
    }
}

fn read_schema(schema: &[u8], offset_shift: u32) -> Result<Schema, String> {
    let mut out = Schema { structs: HashMap::new(), enums: HashMap::new(), pairs: Vec::new() };
    let pair_names = (names::parse("Key"), names::parse("Item"));
    let n = u32_at(schema, 8)? as usize;
    for i in 0..n.min(0x10000) {
        let name = u32_at(schema, 12 + i * 8)?;
        let off = u32_at(schema, 16 + i * 8)? as usize;
        out.add(schema, (name, off), offset_shift, pair_names)?;
    }
    Ok(out)
}

fn read_pso(file: &[u8], offset_shift: u32) -> Result<Pso, String> {
    let sections = split_sections(file)?;
    let (root_block, blocks) = read_blocks(sections.map)?;
    let Schema { structs, enums, pairs } = read_schema(sections.schema, offset_shift)?;
    Ok(Pso {
        data: sections.data.to_vec(),
        root_block,
        blocks,
        structs,
        enums,
        offset_shift,
        sections: sections.all,
        pairs,
    })
}

/// Parses a PSO file and writes it as XML (see [`Pso::to_xml`]).
pub fn to_xml(file: &[u8]) -> Result<(String, Vec<String>), PsoError> {
    Ok(parse(file)?.to_xml())
}

/// Where an entry is read: the output level, its structure's block and offset in it.
#[derive(Clone, Copy)]
struct Site {
    level: usize,
    block_id: usize,
    block: Block,
    offset: usize,
    depth: usize,
}

impl Site {
    /// The absolute offset in the data of an entry of the structure.
    fn at(&self, entry: &Entry) -> usize {
        self.block.offset + self.offset + entry.offset
    }
}

/// The key and item entries of a map's pairs, and the size of a pair.
#[derive(Clone, Copy)]
struct PairLayout {
    key: Entry,
    item: Entry,
    len: usize,
}

/// What the pairs of one map share: the output level and depth, the block they are in, and their layout.
struct MapPairs<'a> {
    level: usize,
    id: usize,
    block: Block,
    depth: usize,
    info: &'a StructInfo,
    layout: PairLayout,
}

type ItemText = fn(&[u8]) -> String;

/// Item size and text of an array of plain numbers.
fn scalar_format(kind: u8) -> Option<(usize, ItemText)> {
    match kind {
        ty::LONG => Some((8, |b| (u64::from_be_bytes(b[..8].try_into().unwrap_or_default()) as i64).to_string())),
        ty::ULONG => Some((8, |b| u64::from_be_bytes(b[..8].try_into().unwrap_or_default()).to_string())),
        ty::SBYTE => Some((1, |b| (b[0] as i8).to_string())),
        ty::UBYTE | ty::BOOL => Some((1, |b| b[0].to_string())),
        ty::FLOAT => Some((4, |b| be_floats(b, 1))),
        ty::USHORT => Some((2, |b| u16::from_be_bytes([b[0], b[1]]).to_string())),
        ty::UINT | ty::SINT => Some((4, |b| u32::from_be_bytes(b[..4].try_into().unwrap_or_default()).to_string())),
        _ => None,
    }
}

/// The item type entry of an array field.
fn array_item_entry(entry: &Entry, info: &StructInfo) -> Option<Entry> {
    let mut index = (entry.key & 0xFFFF) as usize;
    if index >= info.entries.len() {
        index = (entry.key & 0xFFF) as usize;
    }
    info.entries.get(index).copied()
}

/// String, hash and enum arrays always point elsewhere.
fn is_external_array(entry: &Entry, item: &Entry) -> bool {
    matches!(entry.sub, 0 | 5) || (entry.sub == 4 && item.sub == 3) || matches!(item.kind, ty::STRING | ty::ENUM)
}

/// Where the items of an array are: block, offset in it, absolute offset and count.
#[derive(Clone, Copy)]
struct ArrayLoc {
    block: usize,
    off: usize,
    abs: usize,
    count: usize,
}

fn byte_at(d: &[u8], at: usize) -> Result<u8, String> {
    d.get(at).copied().ok_or_else(|| format!("offset {at:#x} beyond data"))
}

fn be_u32s(items: Vec<&[u8]>) -> Vec<u32> {
    items.iter().map(|b| u32::from_be_bytes(b[..4].try_into().unwrap_or_default())).collect()
}

fn be_floats(b: &[u8], n: usize) -> String {
    (0..n)
        .map(|i| xml::float(f32::from_be_bytes(b[i * 4..i * 4 + 4].try_into().unwrap_or_default())))
        .collect::<Vec<_>>()
        .join(", ")
}

/// How many floats a vector entry has.
fn vector_len(kind: u8) -> Option<usize> {
    match kind {
        ty::FLOAT2 => Some(2),
        ty::FLOAT3 | ty::FLOAT3A | ty::FLOAT4A => Some(3),
        ty::FLOAT4 | ty::VEC4V => Some(4),
        _ => None,
    }
}

/// Item size in bytes and floats per item of a vector array.
fn vector_array_dims(kind: u8) -> Option<(usize, usize)> {
    match kind {
        ty::FLOAT2 => Some((8, 2)),
        ty::FLOAT3 | ty::FLOAT3A => Some((16, 3)),
        ty::FLOAT4 | ty::FLOAT4A | ty::VEC4V => Some((16, 4)),
        _ => None,
    }
}

/// The text of a plain number (or boolean) entry; `None` for other kinds.
fn number_text(entry: &Entry, d: &[u8], at: usize) -> Result<Option<String>, String> {
    let text = match entry.kind {
        ty::BOOL | ty::SBYTE | ty::UBYTE => byte_text(entry.kind, d, at)?,
        ty::SSHORT | ty::HFLOAT | ty::USHORT => short_text(entry.kind, d, at)?,
        ty::FLOAT | ty::SINT | ty::UINT => word_text(entry, d, at)?,
        ty::LONG | ty::ULONG => u64_at(d, at)?.to_string(),
        _ => return Ok(None),
    };
    Ok(Some(text))
}

fn byte_text(kind: u8, d: &[u8], at: usize) -> Result<String, String> {
    let b = byte_at(d, at)?;
    Ok(match kind {
        ty::BOOL => (if b != 0 { "true" } else { "false" }).to_owned(),
        ty::SBYTE => (b as i8).to_string(),
        _ => b.to_string(),
    })
}

fn short_text(kind: u8, d: &[u8], at: usize) -> Result<String, String> {
    let v = u16_at(d, at)?;
    Ok(if kind == ty::USHORT { v.to_string() } else { (v as i16).to_string() })
}

fn word_text(entry: &Entry, d: &[u8], at: usize) -> Result<String, String> {
    if entry.kind == ty::FLOAT {
        return Ok(xml::float(f32_at(d, at)?));
    }
    match (entry.kind, entry.sub) {
        (ty::SINT, _) | (_, 0) => Ok((u32_at(d, at)? as i32).to_string()),
        (_, 1) => Ok(format!("0x{:08X}", u32_at(d, at)?)),
        (_, s) => Err(format!("unexpected integer subtype {s}")),
    }
}

/// The bits of a flags entry as 32-bit words; RDR2's subtype 4 is a bit set
/// whose size (in bits) is the high half of the entry's key.
fn flag_words(d: &[u8], at: usize, entry: &Entry) -> Result<Vec<u32>, String> {
    Ok(match entry.sub {
        0 => vec![u32_at(d, at)?],
        1 => vec![u16_at(d, at)? as u32],
        2 => vec![byte_at(d, at)? as u32],
        4 => {
            let bits = (entry.key >> 16).clamp(1, 4096) as usize;
            (0..bits.div_ceil(32)).map(|i| u32_at(d, at + i * 4)).collect::<Result<_, _>>()?
        }
        s => return Err(format!("unexpected flags subtype {s}")),
    })
}

fn bit_set(words: &[u32], bit: i32) -> bool {
    usize::try_from(bit).ok().is_some_and(|b| words.get(b / 32).is_some_and(|w| w & (1 << (b % 32)) != 0))
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

    /// The name of an enum value; a value the enum does not list as its number.
    fn enum_name(&self, key: u32, value: i32) -> String {
        self.enums
            .get(&key)
            .and_then(|v| v.iter().find(|(_, x)| *x == value))
            .map_or_else(|| value.to_string(), |(n, _)| names::field(*n))
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
            let site = Site { level: inner, block_id, block, offset, depth };
            if let Err(e) = self.entry(w, &site, entry, info, &ename) {
                w.error(inner, format!("{ename}: {e}"));
            }
        }
        if let Some(t) = tag_name {
            w.close(level, if tag == Tag::Structure { &t } else { "Item" });
        }
    }

    fn entry(&self, w: &mut Writer, site: &Site, entry: &Entry, info: &StructInfo, ename: &str) -> Result<(), String> {
        let at = site.at(entry);
        match entry.kind {
            ty::ARRAY => self.array_node(w, site, entry, info, ename),
            ty::ENUM => self.enum_entry(w, site, entry, ename),
            ty::FLAGS => self.flags_entry(w, site, entry, info, ename),
            ty::MAP => self.map_node(w, site, entry, info, ename),
            ty::STRING => {
                let s = self.string_value(entry, at)?;
                w.string(site.level, ename, &s);
                Ok(())
            }
            ty::STRUCTURE => self.structure_entry(w, site, entry, ename),
            _ => self.plain_entry(w, site, entry, ename),
        }
    }

    fn enum_entry(&self, w: &mut Writer, site: &Site, entry: &Entry, ename: &str) -> Result<(), String> {
        let v = self.enum_value(entry, site.at(entry))?;
        w.string(site.level, ename, &self.enum_name(entry.key, v));
        Ok(())
    }

    /// Numbers, vectors, matrices and opaque bytes.
    fn plain_entry(&self, w: &mut Writer, site: &Site, entry: &Entry, ename: &str) -> Result<(), String> {
        let level = site.level;
        let d = &self.data[..];
        let at = site.at(entry);
        if let Some(text) = number_text(entry, d, at)? {
            w.value(level, ename, &text);
        } else if let Some(n) = vector_len(entry.kind) {
            let v = (0..n).map(|i| f32_at(d, at + i * 4)).collect::<Result<Vec<_>, _>>()?;
            w.empty(level, &vector_attrs(ename, &v));
        } else if entry.kind == ty::MATRIX44 {
            let m: Vec<String> = (0..16).map(|i| f32_at(d, at + i * 4).map(xml::float)).collect::<Result<_, _>>()?;
            let rows: Vec<String> = m.chunks(4).map(|r| r.join(" ")).collect();
            w.raw_array(level, ename, &rows, 1);
        } else if entry.kind == ty::OPAQUE16 {
            let b = d.get(at..at + 16).ok_or("16 bytes beyond the data")?;
            w.value(level, ename, &b.iter().map(|x| format!("{x:02X}")).collect::<String>());
        } else {
            return Err(format!("unexpected data type {:#04x}", entry.kind));
        }
        Ok(())
    }

    /// The value of an enum entry; the subtype is its size.
    fn enum_value(&self, entry: &Entry, at: usize) -> Result<i32, String> {
        let d = &self.data[..];
        // RDR2 counts the subtypes one higher: 1 = 32 bit, 2 = 16 bit, 3 = 8 bit.
        let sub = if self.offset_shift == 20 { entry.sub.saturating_sub(1) } else { entry.sub };
        Ok(match sub {
            0 => u32_at(d, at)? as i32,
            1 => u16_at(d, at)? as i16 as i32,
            2 => byte_at(d, at)? as i32,
            s => return Err(format!("unexpected enum subtype {s}")),
        })
    }

    fn flags_entry(
        &self,
        w: &mut Writer,
        site: &Site,
        entry: &Entry,
        info: &StructInfo,
        ename: &str,
    ) -> Result<(), String> {
        let level = site.level;
        let index = (entry.key & 0xFFF) as usize;
        let flags = (index != 0xFFF)
            .then(|| info.entries.get(index))
            .flatten()
            .filter(|e| e.name == ARRAYINFO)
            .and_then(|e| self.enums.get(&e.key));
        let words = flag_words(&self.data, site.at(entry), entry)?;
        match flags {
            Some(values) => {
                let set: Vec<String> =
                    values.iter().filter(|(_, bit)| bit_set(&words, *bit)).map(|(n, _)| names::field(*n)).collect();
                w.string(level, ename, &set.join(" "));
            }
            None if words.len() == 1 && words[0] != 0 => w.value(level, ename, &words[0].to_string()),
            None if words.iter().any(|&x| x != 0) => {
                let hex: Vec<String> = words.iter().map(|x| format!("{x:08X}")).collect();
                w.value(level, ename, &hex.join(" "));
            }
            None => w.empty(level, ename),
        }
        Ok(())
    }

    /// A structure inline (subtype 0) or through a pointer (3, 4).
    fn structure_entry(&self, w: &mut Writer, site: &Site, entry: &Entry, ename: &str) -> Result<(), String> {
        let Site { level, block_id, offset, depth, .. } = *site;
        match entry.sub {
            0 => {
                w.open(level, ename);
                self.node(w, level, block_id, offset + entry.offset, Tag::None, entry.key, depth + 1);
                w.close(level, ename);
            }
            3 | 4 => {
                let (id, off) = self.unpack(ptr_at(&self.data, site.at(entry))?);
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

    fn array_node(
        &self,
        w: &mut Writer,
        site: &Site,
        entry: &Entry,
        info: &StructInfo,
        ename: &str,
    ) -> Result<(), String> {
        let (item, loc) = self.array_location(site, entry, info, ename)?;
        match item.kind {
            ty::ARRAY => self.nested_array(w, site, entry, info, &item, ename),
            ty::STRUCTURE if matches!(item.sub, 3 | 4) => self.struct_pointer_array(w, site, ename, &loc),
            ty::STRUCTURE => self.struct_array(w, site, ename, &item, &loc),
            ty::STRING => self.string_array(w, site.level, ename, &item, &loc),
            ty::ENUM => self.enum_array(w, site.level, ename, &item, &loc),
            _ => self.number_array(w, site.level, ename, &item, &loc),
        }
    }

    fn enum_array(
        &self,
        w: &mut Writer,
        level: usize,
        ename: &str,
        item: &Entry,
        loc: &ArrayLoc,
    ) -> Result<(), String> {
        let v: Vec<String> = be_u32s(self.items(loc.abs, loc.count, 4)?)
            .into_iter()
            .map(|x| self.enum_name(item.key, x as i32))
            .collect();
        w.item_array(level, ename, &v);
        Ok(())
    }

    /// The array's item type entry and where its items are.
    fn array_location(
        &self,
        site: &Site,
        entry: &Entry,
        info: &StructInfo,
        ename: &str,
    ) -> Result<(Entry, ArrayLoc), String> {
        let item = array_item_entry(entry, info).ok_or_else(|| format!("ARRAYINFO not found for {ename}"))?;
        // Where the items are: (block, offset in block, absolute offset, count).
        let mut loc = ArrayLoc {
            block: site.block_id,
            off: site.offset + entry.offset,
            abs: site.at(entry),
            count: (entry.key >> 16) as usize,
        };
        if is_external_array(entry, &item) {
            self.locate_external(&mut loc, site.at(entry))?;
        } else if !matches!(entry.sub, 1 | 2 | 4 | 129) {
            return Err(format!("unsupported array subtype {}", entry.sub));
        }
        Ok((item, loc))
    }

    /// Follows the pointer of an array whose items are stored elsewhere.
    fn locate_external(&self, loc: &mut ArrayLoc, at: usize) -> Result<(), String> {
        let d = &self.data[..];
        let (id, off) = self.unpack(ptr_at(d, at)?);
        loc.block = id;
        loc.off = off;
        loc.count = u16_at(d, at + 8)? as usize;
        if let Some(b) = self.block(id) {
            loc.abs = b.offset + off;
        }
        Ok(())
    }

    /// An array of arrays: one 16-byte inner array after the other.
    fn nested_array(
        &self,
        w: &mut Writer,
        site: &Site,
        entry: &Entry,
        info: &StructInfo,
        item: &Entry,
        ename: &str,
    ) -> Result<(), String> {
        let level = site.level;
        let n = (entry.key >> 16) as usize;
        if n == 0 {
            w.empty(level, ename);
            return Ok(());
        }
        w.open(level, ename);
        let mut off = site.offset + entry.offset;
        for _ in 0..n {
            let inner = Entry { offset: 0, ..*item };
            let inner_site = Site { level: level + 1, offset: off, depth: site.depth + 1, ..*site };
            self.array_node(w, &inner_site, &inner, info, "Item")?;
            off += 16;
        }
        w.close(level, ename);
        Ok(())
    }

    /// An array of pointers to structures.
    fn struct_pointer_array(&self, w: &mut Writer, site: &Site, ename: &str, loc: &ArrayLoc) -> Result<(), String> {
        let (level, depth) = (site.level, site.depth);
        if loc.count == 0 {
            w.empty(level, ename);
            return Ok(());
        }
        let ptrs = self.items(loc.abs, loc.count, 8)?;
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
        Ok(())
    }

    /// An array of structures, one after the other.
    fn struct_array(
        &self,
        w: &mut Writer,
        site: &Site,
        ename: &str,
        item: &Entry,
        loc: &ArrayLoc,
    ) -> Result<(), String> {
        let (level, depth, count) = (site.level, site.depth, loc.count);
        let tag = format!("{ename} itemType=\"{}\"", names::field(item.key));
        if count == 0 {
            w.empty(level, &tag);
            return Ok(());
        }
        let a = *self.block(loc.block).ok_or_else(|| format!("array block {} not found", loc.block))?;
        let st = self.structs.get(&item.key).ok_or("array item structure not found")?;
        w.open(level, &tag);
        let mut off = loc.off;
        for n in 0..count {
            self.node(w, level + 1, loc.block, off, Tag::Item, item.key, depth + 1);
            off += st.length;
            if n + 1 < count && off >= a.length {
                break;
            }
        }
        w.close(level, ename);
        Ok(())
    }

    /// An array of hashed or pointed-to strings.
    fn string_array(
        &self,
        w: &mut Writer,
        level: usize,
        ename: &str,
        item: &Entry,
        loc: &ArrayLoc,
    ) -> Result<(), String> {
        let (abs, count) = (loc.abs, loc.count);
        match item.sub {
            7..=15 => {
                let v: Vec<String> = be_u32s(self.items(abs, count, 4)?).into_iter().map(names::value).collect();
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
                        self.string_at(id, off, len).unwrap_or_default()
                    })
                    .collect();
                w.item_array(level, ename, &v);
            }
            s => return Err(format!("unexpected string array subtype {s}")),
        }
        Ok(())
    }

    /// Arrays of vectors (one per line) and of plain numbers (ten per line).
    fn number_array(
        &self,
        w: &mut Writer,
        level: usize,
        ename: &str,
        item: &Entry,
        loc: &ArrayLoc,
    ) -> Result<(), String> {
        if let Some((size, n)) = vector_array_dims(item.kind) {
            let v: Vec<String> = self.items(loc.abs, loc.count, size)?.into_iter().map(|b| be_floats(b, n)).collect();
            w.raw_array(level, ename, &v, 1);
            return Ok(());
        }
        let Some(v) = self.scalar_array(item, loc)? else {
            return Err(format!("unsupported array item type {:#04x}", item.kind));
        };
        w.raw_array(level, ename, &v, 10);
        Ok(())
    }

    fn scalar_array(&self, item: &Entry, loc: &ArrayLoc) -> Result<Option<Vec<String>>, String> {
        let Some((size, format)) = scalar_format(item.kind) else { return Ok(None) };
        Ok(Some(self.items(loc.abs, loc.count, size)?.into_iter().map(format).collect()))
    }

    /// The pointer to the pairs of a map and their count.
    fn map_header(&self, at: usize) -> Result<(u64, usize), String> {
        let d = &self.data[..];
        let x1 = u32_at(d, at)?;
        // RDR2 leaves the rest of the first 8 bytes uninitialised: only the first byte counts.
        let pointer_first = x1 == 0x0100_0000 || (self.offset_shift == 20 && x1 >> 24 == 1);
        if !pointer_first {
            Ok((ptr_at(d, at + 16)?, u16_at(d, at + 2)? as usize))
        } else {
            Ok((ptr_at(d, at + 8)?, u16_at(d, at + 16)? as usize))
        }
    }

    /// The size of a map item in RDR2's layout, which sets the stride of the pairs.
    fn map_item_size(&self, item: &Entry) -> usize {
        match (item.kind, item.sub) {
            (ty::STRUCTURE, 0) => self.structs.get(&item.key).map_or(8, |s| s.length),
            (ty::STRING, 7 | 8) | (ty::UINT | ty::SINT | ty::ENUM | ty::FLOAT, _) => 4,
            _ => 8,
        }
    }

    /// GTA V describes the pairs by a structure with Key and Item fields.
    /// RDR2 describes them by two type entries of the owning structure
    /// (the map entry's key: key type index << 16 | item type index), and
    /// stores each pair as the key (4 bytes, padded to 8) and the item.
    fn map_pair_layout(&self, block: &Block, info: &StructInfo, entry: &Entry) -> Result<PairLayout, String> {
        let (key_name, item_name) = (names::parse("Key"), names::parse("Item"));
        let pair_struct =
            self.structs.get(&block.name).and_then(|st| Some((*st.find(key_name)?, *st.find(item_name)?, st.length)));
        if let Some((key, item, len)) = pair_struct {
            return Ok(PairLayout { key, item, len });
        }
        let mut key = *info.entries.get((entry.key >> 16) as usize & 0xFFF).ok_or("map Key entry missing")?;
        let mut item = *info.entries.get(entry.key as usize & 0xFFF).ok_or("map Item entry missing")?;
        item.offset = 8;
        key.offset = 0;
        Ok(PairLayout { key, item, len: (8 + self.map_item_size(&item)).next_multiple_of(8) })
    }

    fn map_node(
        &self,
        w: &mut Writer,
        site: &Site,
        entry: &Entry,
        info: &StructInfo,
        ename: &str,
    ) -> Result<(), String> {
        let (level, depth) = (site.level, site.depth);
        if entry.sub != 1 {
            return Err(format!("unexpected map subtype {}", entry.sub));
        }
        let (ptr, count) = self.map_header(site.at(entry))?;
        let (id, mut off) = self.unpack(ptr);
        if count == 0 {
            w.empty(level, ename);
            return Ok(());
        }
        let block = *self.block(id).ok_or_else(|| format!("map block {id} not found"))?;
        if off >= block.length {
            off >>= 8;
        }
        let layout = self.map_pair_layout(&block, info, entry)?;
        let pairs = MapPairs { level, id, block, depth, info, layout };
        w.open(level, ename);
        for _ in 0..count {
            if off >= block.length {
                w.error(level + 1, format!("map offset out of range (count {count})"));
                break;
            }
            self.map_pair(w, &pairs, off)?;
            off += layout.len;
        }
        w.close(level, ename);
        Ok(())
    }

    /// One key/item pair of a map, at `off` in the map's block.
    fn map_pair(&self, w: &mut Writer, m: &MapPairs, off: usize) -> Result<(), String> {
        let base = m.block.offset + off;
        let key = xml::escape(&self.map_key(&m.layout.key, base + m.layout.key.offset)?);
        let item = &m.layout.item;
        match item.kind {
            ty::STRING => {
                let s = self.string_value(item, base + item.offset)?;
                w.text(m.level + 1, &format!("Item type=\"String\" key=\"{key}\""), &s);
                Ok(())
            }
            ty::STRUCTURE if item.key != 0 && item.sub != 3 => {
                w.open(m.level + 1, &format!("Item type=\"{}\" key=\"{key}\"", names::field(item.key)));
                self.node(w, m.level + 1, m.id, off + item.offset, Tag::None, item.key, m.depth + 1);
                w.close(m.level + 1, "Item");
                Ok(())
            }
            ty::STRUCTURE => self.map_pointer_item(w, m, base + item.offset, &key),
            _ => {
                self.map_value_item(w, m, off, &key);
                Ok(())
            }
        }
    }

    fn map_key(&self, key: &Entry, key_at: usize) -> Result<String, String> {
        Ok(match key.kind {
            ty::STRING => self.string_value(key, key_at)?,
            ty::ENUM | ty::UINT | ty::SINT => names::value(u32_at(&self.data, key_at)?),
            _ => format!("{:#x}", u32_at(&self.data, key_at)?),
        })
    }

    /// A map item written like a structure entry, as the pair's `Value`.
    fn map_value_item(&self, w: &mut Writer, m: &MapPairs, off: usize, key: &str) {
        w.open(m.level + 1, &format!("Item key=\"{key}\""));
        let site = Site { level: m.level + 2, block_id: m.id, block: m.block, offset: off, depth: m.depth + 1 };
        if let Err(e) = self.entry(w, &site, &m.layout.item, m.info, "Value") {
            w.error(m.level + 2, format!("map value: {e}"));
        }
        w.close(m.level + 1, "Item");
    }

    /// A map item that is a pointer to a structure, whose block names its type.
    fn map_pointer_item(&self, w: &mut Writer, m: &MapPairs, at: usize, key: &str) -> Result<(), String> {
        let (pid, poff) = self.unpack(ptr_at(&self.data, at)?);
        let pb = self.block(pid).ok_or("map item block not found")?;
        w.open(m.level + 1, &format!("Item type=\"{}\" key=\"{key}\"", names::field(pb.name)));
        if self.structs.get(&pb.name).is_some_and(|s| !s.entries.is_empty()) {
            self.node(w, m.level + 1, pid, poff, Tag::None, 0, m.depth + 1);
        }
        w.close(m.level + 1, "Item");
        Ok(())
    }
}

#[cfg(test)]
mod read_tests;
#[cfg(test)]
mod test_support;

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
