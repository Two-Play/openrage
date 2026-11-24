//! GTA VI metadata containers (`PSIN` with an `LSCH` schema).
//!
//! The successor of the PSO files of GTA V and RDR2 ([`crate::pso`]): the
//! same magic, another layout. Little-endian, except the offset of the first
//! section and the section sizes.
//!
//! - `PSIN`, then the big-endian offset of the first section.
//! - From 8 on, the block table: rows of 16 bytes (type, size, eight `AA`),
//!   ended by `70` bytes up to the next 16-byte boundary. A block's type is
//!   the name hash of a struct, a member type below `0x100`, or `0x100 + n`.
//!   The blocks follow in this order, each padded to 16 bytes, numbered
//!   from 1.
//! - Sections up to the end of the file, each a tag and its big-endian size
//!   (tag and size included): `LSCH` (structs of 32 bytes, then all their
//!   members of 16), `LENM` (enums of 24 bytes, then all their values of 16),
//!   `LRSC` (`0DRP`, and at 16 the number of the root block), and others
//!   that are not needed to read the data.
//!
//! A pointer is 64 bits: the block number in the low 20 (0: none), the byte
//! offset in that block above. Names are hashes (case-sensitive joaat); the
//! names themselves are not in the files, so the XML shows `hash_…`.
//!
//! The layout was worked out by the Leonida project (`docs/psin.md`,
//! `tools/psin_data.py`, GPL-3.0), which this reader follows.

use std::collections::{HashMap, HashSet};

use rage_core::bytes::LeRead;

use crate::xml::{self, Writer};

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PsinError {
    #[error("invalid PSIN file: {0}")]
    Invalid(String),
}

fn invalid(msg: impl Into<String>) -> PsinError {
    PsinError::Invalid(msg.into())
}

/// The member that describes the elements of an array or map.
const ELEMENT: u32 = 0x100;
/// Pointers nest at most this deep.
const MAX_DEPTH: usize = 64;

#[derive(Debug, Clone, Copy)]
struct Member {
    name: u32,
    offset: usize,
    kind: u8,
    /// For an array: which member of the struct describes its elements.
    index: usize,
    /// A struct or enum name, a length, or a map's key and value members.
    reference: u32,
}

#[derive(Debug, Clone)]
struct Struct {
    size: usize,
    align: usize,
    members: Vec<Member>,
}

/// A parsed container.
#[derive(Debug)]
pub struct Psin<'a> {
    data: &'a [u8],
    structs: HashMap<u32, Struct>,
    /// Per enum: value and the hash of its name.
    enums: HashMap<u32, Vec<(i64, u32)>>,
    /// Per block: type, offset in the file, size.
    blocks: Vec<(u32, usize, usize)>,
    root: usize,
}

/// Whether this is a container of the GTA VI kind: `PSIN`, and `LSCH` where
/// the header says the sections start.
pub fn is_psin(data: &[u8]) -> bool {
    data.starts_with(b"PSIN") && be32(data, 4).and_then(|first| data.get(first..first + 4)) == Some(b"LSCH")
}

fn be32(d: &[u8], at: usize) -> Option<usize> {
    Some(u32::from_be_bytes(d.get(at..at + 4)?.try_into().ok()?) as usize)
}

/// How many headers a schema section has: the one count whose headers and
/// the elements they announce fill it exactly.
fn header_count(d: &[u8], header: usize, element: usize, count_at: usize, wide: bool) -> Option<usize> {
    if d.is_empty() {
        return Some(0);
    }
    let mut elements = 0usize;
    for k in 1..=d.len() / header {
        let at = header * (k - 1) + count_at;
        elements += if wide { d.u32_at(at)? as usize } else { usize::from(d.u16_at(at)?) };
        if header * k + element * elements == d.len() {
            return Some(k);
        }
    }
    None
}

pub fn parse(data: &[u8]) -> Result<Psin<'_>, PsinError> {
    if !is_psin(data) {
        return Err(invalid("no LSCH section"));
    }
    let first = be32(data, 4).ok_or_else(|| invalid("truncated"))?;
    let mut sections: HashMap<&[u8], &[u8]> = HashMap::new();
    let mut at = first;
    while let Some(tag) = data.get(at..at + 4) {
        let size = be32(data, at + 4).ok_or_else(|| invalid("truncated section"))?;
        let payload = (size >= 8).then(|| data.get(at + 8..at + size)).flatten();
        sections.insert(tag, payload.ok_or_else(|| invalid("the file is cut off inside a section"))?);
        at += size;
    }

    let lsch = sections[&b"LSCH"[..]];
    let count = header_count(lsch, 32, 16, 30, false).ok_or_else(|| invalid("no struct count fills LSCH"))?;
    let mut structs = HashMap::new();
    let mut member = 32 * count;
    for h in lsch[..32 * count].as_chunks::<32>().0 {
        let members = (0..usize::from(u16::from_le_bytes([h[30], h[31]])))
            .map(|i| {
                let m = &lsch[member + 16 * i..member + 16 * i + 16];
                Member {
                    name: m.u32_at(0).unwrap_or(0),
                    offset: m.u32_at(4).unwrap_or(0) as usize,
                    kind: m[8],
                    index: usize::from(m.u16_at(10).unwrap_or(0)),
                    reference: m.u32_at(12).unwrap_or(0),
                }
            })
            .collect::<Vec<_>>();
        member += 16 * members.len();
        let (name, size) = (h.u32_at(0).unwrap_or(0), h.u32_at(24).unwrap_or(0) as usize);
        structs.insert(name, Struct { size, align: 1 << h[9].min(4), members });
    }

    let mut enums = HashMap::new();
    if let Some(lenm) = sections.get(&b"LENM"[..]) {
        let count = header_count(lenm, 24, 16, 16, true).ok_or_else(|| invalid("no enum count fills LENM"))?;
        let mut value = 24 * count;
        for h in lenm[..24 * count].as_chunks::<24>().0 {
            let n = h.u32_at(16).unwrap_or(0) as usize;
            let values = lenm[value..value + 16 * n]
                .as_chunks::<16>()
                .0
                .iter()
                .map(|v| (v.u64_at(0).unwrap_or(0) as i64, v.u32_at(8).unwrap_or(0)))
                .collect();
            value += 16 * n;
            enums.insert(h.u32_at(0).unwrap_or(0), values);
        }
    }

    // The block table, then the blocks.
    let mut rows = Vec::new();
    let mut at = 8;
    loop {
        let kind = data.u32_at(at).filter(|_| at + 8 <= first).ok_or_else(|| invalid("block table without end"))?;
        if kind == 0x7070_7070 {
            break;
        }
        let size = data.u32_at(at + 4).filter(|_| at + 16 <= first).ok_or_else(|| invalid("block table cut off"))?;
        rows.push((kind, size as usize));
        at += 16;
    }
    at = at.next_multiple_of(16);
    let mut blocks = Vec::with_capacity(rows.len());
    for (kind, size) in rows {
        if at + size > first {
            return Err(invalid("a block ends after the first section"));
        }
        blocks.push((kind, at, size));
        at += size.next_multiple_of(16);
    }

    let lrsc = sections.get(&b"LRSC"[..]).filter(|s| s.starts_with(b"0DRP"));
    let root = lrsc.and_then(|s| s.u32_at(0x10)).ok_or_else(|| invalid("no root (LRSC section)"))? as usize;
    match blocks.get(root.wrapping_sub(1)) {
        Some((kind, ..)) if structs.contains_key(kind) => {}
        _ => return Err(invalid("the root is not a struct block")),
    }
    Ok(Psin { data, structs, enums, blocks, root })
}

fn hash_name(hash: u32) -> String {
    format!("hash_{hash:08X}")
}

impl Psin<'_> {
    /// The name (as a hash) of the root struct.
    pub fn root_type(&self) -> String {
        hash_name(self.blocks[self.root - 1].0)
    }

    pub fn to_xml(&self) -> (String, Vec<String>) {
        let mut w = Writer::new();
        let (kind, at, _) = self.blocks[self.root - 1];
        let name = hash_name(kind);
        let mut open = HashSet::new();
        self.members(&mut w, 0, &name, &name, kind, at, &mut open);
        w.finish()
    }

    /// Size of a member's value in its struct, if its type is understood.
    fn size_of(&self, m: &Member) -> Option<usize> {
        Some(match m.kind {
            0x01 | 0x10 | 0x11 | 0x60 | 0x63 => 1,
            0x12 | 0x13 | 0x20 | 0x64 => 2,
            0x14 | 0x15 | 0x21 | 0x4A | 0x4B | 0x4D | 0x62 | 0x65 => 4,
            0x07 | 0x08 | 0x16 | 0x17 | 0x30 | 0x42 | 0x4F | 0x68 => 8,
            0x32 => 12,
            0x0C | 0x0D | 0x31 | 0x33 | 0x34 | 0x35 | 0x44 | 0x52 | 0x5C | 0x73 | 0x75 => 16,
            0x71 => 24,
            0x36 => 64,
            0x05 => self.structs.get(&m.reference)?.size,
            0x40 => m.reference as usize,
            _ => return None,
        })
    }

    fn align_of(&self, m: &Member) -> usize {
        match m.kind {
            0x05 => self.structs.get(&m.reference).map_or(8, |s| s.align),
            0x0C | 0x0D | 0x31 | 0x33 | 0x34 | 0x35 | 0x36 => 16,
            0x32 => 4,
            0x40 => 1,
            _ => self.size_of(m).filter(|n| matches!(n, 1 | 2 | 4)).unwrap_or(8),
        }
    }

    /// Where a pointer leads: the block's type, the offset in the file, and
    /// the end of the block. `None` for no target or one outside its block.
    fn pointer(&self, at: usize) -> Option<(u32, usize, usize)> {
        let v = self.data.u64_at(at)?;
        let (number, offset) = ((v & 0xF_FFFF) as usize, usize::try_from(v >> 20).ok()?);
        let (kind, base, size) = *self.blocks.get(number.checked_sub(1)?)?;
        (offset < size).then_some((kind, base + offset, base + size))
    }

    /// The end of the block that holds `at`.
    fn block_end(&self, at: usize) -> Option<usize> {
        self.blocks.iter().find(|(_, base, size)| (*base..*base + *size).contains(&at)).map(|(_, b, s)| b + s)
    }

    fn string_at(&self, at: usize, end: usize) -> Option<String> {
        let bytes = self.data.get(at..end)?;
        let len = bytes.iter().position(|b| *b == 0)?;
        Some(String::from_utf8_lossy(&bytes[..len]).into_owned())
    }

    fn floats(&self, at: usize, n: usize) -> Option<Vec<String>> {
        (0..n).map(|i| Some(xml::float(f32::from_bits(self.data.u32_at(at + 4 * i)?)))).collect()
    }

    /// Writes a struct: its members between an opening and a closing tag.
    #[allow(clippy::too_many_arguments)]
    fn members(
        &self,
        w: &mut Writer,
        level: usize,
        tag: &str,
        name: &str,
        kind: u32,
        at: usize,
        open: &mut HashSet<(u32, usize)>,
    ) {
        let Some(s) = self.structs.get(&kind) else {
            return w.error(level, format!("{name}: struct {} is not described in this file", hash_name(kind)));
        };
        if self.data.len() < at + s.size {
            return w.error(level, format!("{name}: outside the file"));
        }
        // A struct that points back to itself, or far too deep.
        if open.len() > MAX_DEPTH || !open.insert((kind, at)) {
            return w.error(level, format!("{name}: refers back to itself"));
        }
        if s.members.iter().all(|m| m.name == ELEMENT) {
            w.empty(level, tag);
        } else {
            w.open(level, tag);
            for m in s.members.iter().filter(|m| m.name != ELEMENT) {
                if w.full() {
                    break;
                }
                self.value(w, level + 1, &hash_name(m.name), m, &s.members, at + m.offset, open);
            }
            w.close(level, name);
        }
        open.remove(&(kind, at));
    }

    /// Writes one value: a member of a struct, or an element of an array.
    #[allow(clippy::too_many_arguments)]
    fn value(
        &self,
        w: &mut Writer,
        level: usize,
        name: &str,
        m: &Member,
        members: &[Member],
        at: usize,
        open: &mut HashSet<(u32, usize)>,
    ) {
        let d = self.data;
        let missing = |w: &mut Writer| w.error(level, format!("{name}: outside the file"));
        let number = |w: &mut Writer, v: Option<String>| match v {
            Some(v) => w.value(level, name, &v),
            None => w.error(level, format!("{name}: outside the file")),
        };
        match m.kind {
            0x01 => number(w, d.get(at).map(|b| (*b != 0).to_string())),
            0x10 => number(w, d.get(at).map(|b| (*b as i8).to_string())),
            0x11 => number(w, d.get(at).map(u8::to_string)),
            0x12 => number(w, d.u16_at(at).map(|v| (v as i16).to_string())),
            0x13 => number(w, d.u16_at(at).map(|v| v.to_string())),
            0x14 => number(w, d.u32_at(at).map(|v| (v as i32).to_string())),
            0x15 => number(w, d.u32_at(at).map(|v| v.to_string())),
            0x16 | 0x08 => number(w, d.u64_at(at).map(|v| (v as i64).to_string())),
            0x17 => number(w, d.u64_at(at).map(|v| v.to_string())),
            0x20 => number(w, d.u16_at(at).map(|v| xml::float(half(v)))),
            0x21 => number(w, self.floats(at, 1).map(|f| f[0].clone())),
            0x4A | 0x4B | 0x4D => number(w, d.u32_at(at).map(|v| format!("0x{v:08X}"))),
            0x4F => number(w, d.u64_at(at).map(|v| format!("0x{v:016X}"))),
            0x30 | 0x32 | 0x31 | 0x33 | 0x34 | 0x35 | 0x0C | 0x0D => {
                let n = match m.kind {
                    0x30 => 2,
                    0x32 => 3,
                    _ => 4,
                };
                match self.floats(at, n) {
                    Some(f) => {
                        let parts: Vec<String> =
                            f.iter().zip(["x", "y", "z", "w"]).map(|(v, axis)| format!("{axis}=\"{v}\"")).collect();
                        w.empty(level, &format!("{name} {}", parts.join(" ")));
                    }
                    None => missing(w),
                }
            }
            0x36 => match self.floats(at, 16) {
                Some(f) => w.raw_array(level, name, &f, 4),
                None => missing(w),
            },
            0x60 | 0x63 | 0x64 | 0x62 | 0x65 | 0x68 => self.enumeration(w, level, name, m, at),
            0x05 => self.members(w, level, name, name, m.reference, at, open),
            0x07 => match self.pointer(at) {
                Some((kind, target, _)) => {
                    let tag = format!("{name} type=\"{}\"", hash_name(kind));
                    self.members(w, level, &tag, name, kind, target, open);
                }
                None => w.empty(level, name),
            },
            0x44 if d.get(at + 15).is_some_and(|b| b & 0x80 != 0) => {
                let len = usize::from(d[at + 15] & 0x7F).min(15);
                w.string(level, name, &String::from_utf8_lossy(&d[at..at + len]));
            }
            0x42 | 0x44 => match self.pointer(at) {
                Some((_, target, end)) => match self.string_at(target, end) {
                    Some(text) => w.string(level, name, &text),
                    None => w.error(level, format!("{name}: a string without an end")),
                },
                None => w.empty(level, name),
            },
            0x40 => match d.get(at..at + m.reference as usize) {
                Some(bytes) => {
                    let len = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
                    w.string(level, name, &String::from_utf8_lossy(&bytes[..len]));
                }
                None => missing(w),
            },
            0x50 | 0x53 | 0x54 | 0x5C | 0x52 => self.array(w, level, name, m, members, at, open),
            0x71 | 0x73 | 0x75 => self.map(w, level, name, m, members, at, open),
            other => w.error(level, format!("{name}: type {other:#04x} is not understood")),
        }
    }

    fn enumeration(&self, w: &mut Writer, level: usize, name: &str, m: &Member, at: usize) {
        let d = self.data;
        let raw = match m.kind {
            0x60 | 0x63 => d.get(at).map(|b| i64::from(*b)),
            0x64 => d.u16_at(at).map(i64::from),
            0x62 => d.u32_at(at).map(|v| i64::from(v as i32)),
            0x65 => d.u32_at(at).map(i64::from),
            _ => d.u64_at(at).map(|v| v as i64),
        };
        let Some(raw) = raw else { return w.error(level, format!("{name}: outside the file")) };
        let values = self.enums.get(&m.reference).map(Vec::as_slice).unwrap_or_default();
        // A set of flags: the names of the bits that are set.
        if m.kind == 0x65 {
            let flags: Vec<String> = (0..32)
                .filter(|bit| raw >> bit & 1 != 0)
                .map(|bit| match values.iter().find(|(v, _)| *v == bit) {
                    Some((_, hash)) => hash_name(*hash),
                    None => format!("bit{bit}"),
                })
                .collect();
            return w.value(level, name, &flags.join(" "));
        }
        // A 32-bit enum lists its values as negative numbers or as numbers
        // from 2^31 on.
        let listed = values.iter().find(|(v, _)| *v == raw || (m.kind == 0x62 && *v == i64::from(raw as i32 as u32)));
        match listed {
            Some((_, hash)) => w.value(level, name, &hash_name(*hash)),
            None => w.value(level, name, &raw.to_string()),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn array(
        &self,
        w: &mut Writer,
        level: usize,
        name: &str,
        m: &Member,
        members: &[Member],
        at: usize,
        open: &mut HashSet<(u32, usize)>,
    ) {
        let d = self.data;
        let element = members.get(m.index).filter(|e| e.name == ELEMENT);
        // An empty array may name a struct that this file does not describe.
        let empty = match m.kind {
            0x5C => d.u32_at(at) == Some(0),
            0x52 => d.u16_at(at + 8) == Some(0),
            _ => false,
        };
        if empty {
            return w.empty(level, name);
        }
        let Some((element, size)) = element.and_then(|e| Some((e, self.size_of(e).filter(|s| *s > 0)?))) else {
            let kind = members.get(m.index).map_or(0, |e| e.kind);
            return w.error(level, format!("{name}: an array of type {kind:#04x}, which is not understood"));
        };
        let bad = |w: &mut Writer| w.error(level, format!("{name}: array outside its block"));
        // Where the elements are, how many, and where their block ends.
        let (start, count, end) = match m.kind {
            0x50 | 0x53 => (at, (m.reference & 0xFFFF) as usize, self.block_end(at)),
            0x54 => {
                let (Some(capacity), Some(count)) = (d.u32_at(at), d.u32_at(at + 4)) else { return bad(w) };
                (at + 8, count.min(capacity) as usize, self.block_end(at))
            }
            _ => {
                let (count, pointer) = if m.kind == 0x5C {
                    (d.u32_at(at).map(|c| c as usize), at + 8)
                } else {
                    (d.u16_at(at + 8).map(usize::from), at)
                };
                let Some(count) = count else { return bad(w) };
                if count == 0 {
                    return w.empty(level, name);
                }
                match self.pointer(pointer) {
                    Some((_, target, end)) => (target, count, Some(end)),
                    None => return bad(w),
                }
            }
        };
        if end.is_none_or(|end| start + size * count > end) {
            return bad(w);
        }
        if count == 0 {
            return w.empty(level, name);
        }
        w.open(level, name);
        for i in 0..count {
            if w.full() {
                break;
            }
            self.value(w, level + 1, "Item", element, members, start + i * size, open);
        }
        w.close(level, name);
    }

    #[allow(clippy::too_many_arguments)]
    fn map(
        &self,
        w: &mut Writer,
        level: usize,
        name: &str,
        m: &Member,
        members: &[Member],
        at: usize,
        open: &mut HashSet<(u32, usize)>,
    ) {
        let d = self.data;
        let part = |i: u32| members.get(i as usize).filter(|e| e.name == ELEMENT);
        let (Some(key), Some(value)) = (part(m.reference & 0xFFFF), part(m.reference >> 16)) else {
            return w.error(level, format!("{name}: a map without a description of its entries"));
        };
        let (Some(key_size), Some(value_size)) = (self.size_of(key), self.size_of(value)) else {
            return w.error(level, format!("{name}: a map of types that are not understood"));
        };
        // The value starts where its alignment allows; the pair is rounded
        // up to the larger alignment.
        let value_at = key_size.next_multiple_of(self.align_of(value));
        let pair = (value_at + value_size).next_multiple_of(self.align_of(key).max(self.align_of(value)));
        let bad = |w: &mut Writer| w.error(level, format!("{name}: map outside its block"));
        let mut pairs = Vec::new();
        match m.kind {
            // Pairs sorted by key.
            0x71 => {
                let Some(count) = d.u16_at(at + 16).map(usize::from) else { return bad(w) };
                if count > 0 {
                    let Some((_, start, end)) = self.pointer(at + 8) else { return bad(w) };
                    if start + count * pair > end.next_multiple_of(16) {
                        return bad(w);
                    }
                    pairs.extend((0..count).map(|i| start + i * pair));
                }
            }
            // Groups of 16 control bytes and 16 pairs; bit 7 marks a free one.
            0x75 => {
                let (Some(groups), Some(count)) = (d.u32_at(at), d.u32_at(at + 4)) else { return bad(w) };
                if count > 0 {
                    let group = 16 + 16 * pair;
                    let Some((_, start, end)) = self.pointer(at + 8) else { return bad(w) };
                    if start + groups as usize * group > end.next_multiple_of(16) {
                        return bad(w);
                    }
                    for g in 0..groups as usize {
                        let base = start + g * group;
                        pairs.extend((0..16).filter(|i| d[base + i] & 0x80 == 0).map(|i| base + 16 + i * pair));
                    }
                }
            }
            // Slots; a key of zeros marks a free one.
            _ => {
                let (Some(capacity), Some(count)) = (d.u32_at(at), d.u32_at(at + 4)) else { return bad(w) };
                if count > 0 {
                    let Some((_, start, end)) = self.pointer(at + 8) else { return bad(w) };
                    if start + capacity as usize * pair > end.next_multiple_of(16) {
                        return bad(w);
                    }
                    let used = |slot: &usize| d[*slot..*slot + key_size].iter().any(|b| *b != 0);
                    pairs.extend((0..capacity as usize).map(|i| start + i * pair).filter(used));
                }
            }
        }
        if pairs.iter().any(|p| p + pair > d.len()) {
            return bad(w);
        }
        if pairs.is_empty() {
            return w.empty(level, name);
        }
        w.open(level, name);
        for at in pairs {
            if w.full() {
                break;
            }
            w.open(level + 1, "Item");
            self.value(w, level + 2, "key", key, members, at, open);
            self.value(w, level + 2, "value", value, members, at + value_at, open);
            w.close(level + 1, "Item");
        }
        w.close(level, name);
    }
}

/// A 16-bit float.
fn half(v: u16) -> f32 {
    let (sign, exponent, fraction) = (v >> 15, i32::from(v >> 10 & 0x1F), f32::from(v & 0x3FF));
    let magnitude = match exponent {
        0 => fraction / 1024.0 * 2f32.powi(-14),
        31 if fraction == 0.0 => f32::INFINITY,
        31 => f32::NAN,
        e => (1.0 + fraction / 1024.0) * 2f32.powi(e - 15),
    };
    if sign == 1 { -magnitude } else { magnitude }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section(tag: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = tag.to_vec();
        out.extend_from_slice(&(payload.len() as u32 + 8).to_be_bytes());
        out.extend_from_slice(payload);
        out
    }

    fn member(name: u32, offset: u32, kind: u8, index: u16, reference: u32) -> Vec<u8> {
        let mut m = Vec::new();
        m.extend_from_slice(&name.to_le_bytes());
        m.extend_from_slice(&offset.to_le_bytes());
        m.extend_from_slice(&[kind, 0]);
        m.extend_from_slice(&index.to_le_bytes());
        m.extend_from_slice(&reference.to_le_bytes());
        m
    }

    /// A root struct with a number, a float, an enum, a string pointer and
    /// an array of numbers in a second block.
    fn container() -> Vec<u8> {
        let pointer = |block: u64, offset: u64| (block | offset << 20).to_le_bytes();
        let mut root = Vec::new();
        root.extend_from_slice(&(-5i32).to_le_bytes()); // 0: s32
        root.extend_from_slice(&1.5f32.to_le_bytes()); // 4: f32
        root.extend_from_slice(&2u32.to_le_bytes()); // 8: enum
        root.extend_from_slice(&[0; 4]);
        root.extend_from_slice(&pointer(2, 0)); // 16: string
        root.extend_from_slice(&3u32.to_le_bytes()); // 24: array {count, capacity, pointer}
        root.extend_from_slice(&4u32.to_le_bytes());
        root.extend_from_slice(&pointer(3, 2));
        let blocks: [(u32, Vec<u8>); 3] =
            [(0xAA01, root), (0x10, b"hello\0".to_vec()), (0x13, vec![9, 9, 1, 0, 2, 0, 3, 0])];

        let mut out = b"PSIN\0\0\0\0".to_vec();
        for (kind, data) in &blocks {
            out.extend_from_slice(&kind.to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&[0xAA; 8]);
        }
        out.extend_from_slice(&[0x70; 8]);
        for (_, data) in &blocks {
            out.extend_from_slice(data);
            out.resize(out.len().next_multiple_of(16), 0);
        }
        let first = out.len() as u32;
        out[4..8].copy_from_slice(&first.to_be_bytes());

        let mut lsch = Vec::new();
        lsch.extend_from_slice(&0xAA01u32.to_le_bytes());
        lsch.extend_from_slice(&[0; 5]);
        lsch.push(3); // aligned to 8
        lsch.extend_from_slice(&[0; 6]);
        lsch.extend_from_slice(&[0xAA; 8]);
        lsch.extend_from_slice(&40u32.to_le_bytes());
        lsch.extend_from_slice(&[0, 0, 6, 0]);
        lsch.extend(member(0x11, 0, 0x14, 0, 0));
        lsch.extend(member(0x22, 4, 0x21, 0, 0));
        lsch.extend(member(0x33, 8, 0x62, 0, 0xE001));
        lsch.extend(member(0x44, 16, 0x42, 0, 0));
        lsch.extend(member(0x55, 24, 0x5C, 5, 0));
        lsch.extend(member(ELEMENT, 0, 0x13, 0, 0));
        let mut lenm = Vec::new();
        lenm.extend_from_slice(&0xE001u32.to_le_bytes());
        lenm.extend_from_slice(&[0; 4]);
        lenm.extend_from_slice(&[0xEE; 8]);
        lenm.extend_from_slice(&1u32.to_le_bytes());
        lenm.extend_from_slice(&[0; 4]);
        lenm.extend_from_slice(&2i64.to_le_bytes());
        lenm.extend_from_slice(&0xBEEFu64.to_le_bytes());
        let mut lrsc = b"0DRP".to_vec();
        lrsc.extend_from_slice(&122u32.to_le_bytes());
        lrsc.extend_from_slice(&[0; 8]);
        lrsc.extend_from_slice(&1u32.to_le_bytes());
        out.extend(section(b"LSCH", &lsch));
        out.extend(section(b"LENM", &lenm));
        out.extend(section(b"LGID", &[]));
        out.extend(section(b"LRSC", &lrsc));
        out
    }

    #[test]
    fn reads_a_container_as_xml() {
        let data = container();
        assert!(is_psin(&data));
        let psin = parse(&data).unwrap();
        assert_eq!(psin.root_type(), "hash_0000AA01");
        let (text, errors) = psin.to_xml();
        assert!(errors.is_empty(), "{errors:?}");
        for line in [
            "<hash_0000AA01>",
            " <hash_00000011 value=\"-5\" />",
            " <hash_00000022 value=\"1.5\" />",
            " <hash_00000033 value=\"hash_0000BEEF\" />",
            " <hash_00000044>hello</hash_00000044>",
            "  <Item value=\"1\" />",
            "  <Item value=\"3\" />",
            "</hash_0000AA01>",
        ] {
            assert!(text.contains(line), "{line} is missing in\n{text}");
        }
    }

    #[test]
    fn rejects_what_is_cut_off() {
        let data = container();
        assert!(parse(&data[..data.len() - 4]).is_err());
        assert!(!is_psin(b"PSIN\0\0\0\x10PMAP"));
        let mut no_root = data.clone();
        let at = no_root.len() - 4;
        no_root[at] = 9;
        assert!(parse(&no_root).is_err());
    }

    #[test]
    fn half_floats() {
        assert_eq!((half(0x3C00), half(0xC000), half(0x0000), half(0x3800)), (1.0, -2.0, 0.0, 0.5));
    }
}
