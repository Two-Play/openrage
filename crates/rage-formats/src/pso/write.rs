//! XML → PSO: builds the data of a PSO file from the XML [`Pso::to_xml`]
//! writes, using the structure and enum infos of the file it came from.
//!
//! The XML does not describe the layout, so the original file is the
//! template: its schema (`PSCH`) and the other sections are written back
//! unchanged, and only the data (`PSIN`) and its block map (`PMAP`) are
//! rebuilt. Like the game's own files, every block holds one type (a
//! structure, or characters, hashes, pointers, …); blocks keep the order the
//! original had, and inside a block the items follow in the order they are
//! met walking the XML depth first. A file read back unchanged therefore
//! comes out byte for byte the same in most cases.
//!
//! The checksum section (`CHKS`) covers the data by an unknown method; it is
//! kept when the data comes out unchanged and left out otherwise, as
//! CodeWalker does for the PSO files it writes.

use std::collections::HashMap;

use roxmltree::{Document, Node};

use super::{ARRAYINFO, Entry, Pso, PsoError, StructInfo, ty};
use crate::names;

type R<T> = Result<T, String>;

/// Blocks of characters (strings), named after the data type, as the game's files do.
const CHARS: u32 = ty::SBYTE as u32;
/// Blocks of 8-byte structure pointers.
const POINTERS: u32 = ty::STRUCTURE as u32;
/// Blocks of 32-bit hashes and integers.
const UINTS: u32 = ty::UINT as u32;

/// Blocks of counted string pointers (16 bytes each); the name is what the game's files use.
const COUNTED_STRINGS: u32 = 0xC8;

/// Pointer offsets have 20 bits in GTA V's layout.
const MAX_OFFSET: usize = 1 << 20;

struct Buf {
    name: u32,
    data: Vec<u8>,
}

/// A pointer to patch once the blocks have their final numbers.
struct Fix {
    block: usize,
    at: usize,
    target: usize,
    offset: usize,
}

struct Builder<'a> {
    pso: &'a Pso,
    /// Structure names as the XML writes them.
    struct_names: HashMap<String, u32>,
    /// Enum value names per enum.
    enum_values: HashMap<u32, HashMap<String, i32>>,
    blocks: Vec<Buf>,
    by_name: HashMap<u32, usize>,
    fixes: Vec<Fix>,
}

fn element_children<'a, 'i>(node: Node<'a, 'i>) -> impl Iterator<Item = Node<'a, 'i>> {
    node.children().filter(Node::is_element)
}

fn text(node: Node<'_, '_>) -> String {
    node.children().filter(Node::is_text).filter_map(|t| t.text()).collect()
}

fn attr<'a>(node: Node<'a, '_>, name: &str) -> R<&'a str> {
    node.attribute(name).ok_or_else(|| format!("<{}> has no {name}", node.tag_name().name()))
}

fn value<'a>(node: Node<'a, '_>) -> R<&'a str> {
    attr(node, "value")
}

/// An integer as the XML writes them: decimal (maybe negative) or `0x` hex.
fn int(s: &str) -> R<i128> {
    let t = s.trim();
    let parsed = match t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        Some(hex) => i128::from_str_radix(hex, 16),
        None => t.parse::<i128>(),
    };
    parsed.map_err(|_| format!("\"{s}\" is not a number"))
}

fn float(s: &str) -> R<f32> {
    s.trim().parse::<f32>().map_err(|_| format!("\"{s}\" is not a number"))
}

/// The numbers of a raw array (`1 2 3`, or vectors `1, 2, 3` one to a line).
fn tokens(node: Node<'_, '_>) -> Vec<String> {
    text(node).split(|c: char| c.is_whitespace() || c == ',').filter(|t| !t.is_empty()).map(str::to_owned).collect()
}

fn vector(node: Node<'_, '_>, n: usize) -> R<Vec<f32>> {
    ["x", "y", "z", "w"][..n].iter().map(|a| float(attr(node, a)?)).collect()
}

/// Largest alignment an item of `size` bytes needs (at most 16).
fn align_of(size: usize) -> usize {
    if size == 0 { 1 } else { (1 << size.trailing_zeros()).min(16) }
}

impl<'a> Builder<'a> {
    fn new(pso: &'a Pso) -> Self {
        Builder {
            pso,
            struct_names: pso.structs.keys().map(|h| (names::field(*h), *h)).collect(),
            enum_values: HashMap::new(),
            blocks: Vec::new(),
            by_name: HashMap::new(),
            fixes: Vec::new(),
        }
    }

    fn is_rdr2(&self) -> bool {
        self.pso.offset_shift == 20
    }

    /// Reserves `size` zero bytes in the block for `name`; returns (block, offset).
    fn alloc(&mut self, name: u32, size: usize, align: usize) -> (usize, usize) {
        let bi = *self.by_name.entry(name).or_insert_with(|| {
            self.blocks.push(Buf { name, data: Vec::new() });
            self.blocks.len() - 1
        });
        let data = &mut self.blocks[bi].data;
        let off = data.len().next_multiple_of(align.max(1));
        data.resize(off + size, 0);
        (bi, off)
    }

    fn put(&mut self, bi: usize, at: usize, bytes: &[u8]) -> R<()> {
        let data = &mut self.blocks[bi].data;
        let dst = data.get_mut(at..at + bytes.len()).ok_or_else(|| format!("write at {at:#x} beyond its block"))?;
        dst.copy_from_slice(bytes);
        Ok(())
    }

    fn pointer(&mut self, block: usize, at: usize, target: usize, offset: usize) {
        self.fixes.push(Fix { block, at, target, offset });
    }

    fn struct_info(&self, hash: u32) -> R<&'a StructInfo> {
        self.pso.structs.get(&hash).ok_or_else(|| format!("structure info {} not found", names::field(hash)))
    }

    fn struct_named(&self, name: &str) -> R<u32> {
        self.struct_names.get(name).copied().ok_or_else(|| format!("unknown structure type \"{name}\""))
    }

    fn enum_value(&mut self, key: u32, name: &str) -> R<i32> {
        let pso = self.pso;
        let values = self.enum_values.entry(key).or_insert_with(|| {
            pso.enums.get(&key).map(|v| v.iter().map(|(n, x)| (names::field(*n), *x)).collect()).unwrap_or_default()
        });
        match values.get(name.trim()) {
            Some(v) => Ok(*v),
            None if name.trim().is_empty() => Ok(0),
            None => int(name).map(|v| v as i32).map_err(|_| format!("\"{name}\" is not a value of the enum")),
        }
    }

    /// A string's characters in the character block; returns (block, offset).
    fn chars(&mut self, s: &str) -> (usize, usize) {
        let (bi, off) = self.alloc(CHARS, s.len() + 1, 1);
        self.blocks[bi].data[off..off + s.len()].copy_from_slice(s.as_bytes());
        (bi, off)
    }

    /// A structure of type `hash` at `off` in block `bi`, its fields from `node`'s children.
    fn structure(&mut self, bi: usize, off: usize, hash: u32, node: Node<'_, '_>, depth: usize) -> R<()> {
        if depth > 64 {
            return Err("structures nested too deeply".into());
        }
        let info = self.struct_info(hash)?;
        let mut fields: Vec<(String, Node<'_, '_>)> =
            element_children(node).map(|c| (c.tag_name().name().to_owned(), c)).collect();
        if let Some((_, e)) = fields.iter().find(|(n, _)| n == "error") {
            return Err(format!("the XML holds an error from reading the original: {}", text(*e)));
        }
        for entry in &info.entries {
            if entry.name == ARRAYINFO {
                continue;
            }
            let name = names::field(entry.name);
            let Some(i) = fields.iter().position(|(n, _)| *n == name) else { continue };
            let (_, child) = fields.remove(i);
            self.entry(bi, off, entry, info, child, depth).map_err(|e| format!("{name}: {e}"))?;
        }
        match fields.first() {
            Some((n, _)) => Err(format!("{} has no field {n}", names::field(hash))),
            None => Ok(()),
        }
    }

    /// One field of a structure at `base` in block `bi`.
    fn entry(
        &mut self,
        bi: usize,
        base: usize,
        e: &Entry,
        info: &StructInfo,
        node: Node<'_, '_>,
        depth: usize,
    ) -> R<()> {
        let at = base + e.offset;
        match e.kind {
            ty::ARRAY => self.array(bi, base, e, info, node, depth)?,
            ty::BOOL => {
                let v = match value(node)? {
                    "true" | "True" | "1" => 1u8,
                    "false" | "False" | "0" => 0,
                    other => return Err(format!("\"{other}\" is not true or false")),
                };
                self.put(bi, at, &[v])?;
            }
            ty::SBYTE | ty::UBYTE => self.put(bi, at, &[int(value(node)?)? as u8])?,
            ty::SSHORT | ty::USHORT | ty::HFLOAT => self.put(bi, at, &(int(value(node)?)? as u16).to_be_bytes())?,
            ty::SINT | ty::UINT => self.put(bi, at, &(int(value(node)?)? as u32).to_be_bytes())?,
            ty::LONG | ty::ULONG => self.put(bi, at, &(int(value(node)?)? as u64).to_be_bytes())?,
            ty::ENUM => {
                let sub = if self.is_rdr2() { e.sub.saturating_sub(1) } else { e.sub };
                let v = self.enum_value(e.key, &text(node))?;
                match sub {
                    0 => self.put(bi, at, &(v as u32).to_be_bytes())?,
                    1 => self.put(bi, at, &(v as u16).to_be_bytes())?,
                    2 => self.put(bi, at, &[v as u8])?,
                    s => return Err(format!("unexpected enum subtype {s}")),
                }
            }
            ty::FLAGS => self.flags(bi, at, e, info, node)?,
            ty::FLOAT => self.put(bi, at, &float(value(node)?)?.to_be_bytes())?,
            ty::FLOAT2 => self.floats(bi, at, &vector(node, 2)?)?,
            ty::FLOAT3 | ty::FLOAT3A | ty::FLOAT4A => self.floats(bi, at, &vector(node, 3)?)?,
            ty::FLOAT4 | ty::VEC4V => self.floats(bi, at, &vector(node, 4)?)?,
            ty::MATRIX44 => {
                let v: Vec<f32> = tokens(node).iter().map(|t| float(t)).collect::<R<_>>()?;
                if v.len() != 16 {
                    return Err(format!("a matrix needs 16 numbers, not {}", v.len()));
                }
                self.floats(bi, at, &v)?;
            }
            ty::STRING => self.string(bi, at, e, &text(node))?,
            ty::STRUCTURE => match e.sub {
                0 => self.structure(bi, at, e.key, node, depth + 1)?,
                3 | 4 => {
                    if let Some(t) = node.attribute("type") {
                        let actual = self.struct_named(t)?;
                        let len = self.struct_info(actual)?.length;
                        let (pb, poff) = self.alloc(actual, len, align_of(len));
                        self.pointer(bi, at, pb, poff);
                        self.structure(pb, poff, actual, node, depth + 1)?;
                    }
                }
                s => return Err(format!("unexpected structure subtype {s}")),
            },
            ty::OPAQUE16 => {
                let hex = value(node)?;
                let bytes: Vec<u8> = (0..16)
                    .map(|i| hex.get(i * 2..i * 2 + 2).and_then(|b| u8::from_str_radix(b, 16).ok()))
                    .collect::<Option<_>>()
                    .ok_or_else(|| format!("\"{hex}\" is not 16 bytes of hex"))?;
                self.put(bi, at, &bytes)?;
            }
            ty::MAP => self.map(bi, at, e, info, node, depth)?,
            other => return Err(format!("unexpected data type {other:#04x}")),
        }
        Ok(())
    }

    /// A map, mirroring [`Pso::map_node`]: a header (`0x01000000`, pointer
    /// to the pairs, count, capacity) and the key/item pairs in a block.
    fn map(&mut self, bi: usize, at: usize, e: &Entry, info: &StructInfo, node: Node<'_, '_>, depth: usize) -> R<()> {
        if e.sub != 1 {
            return Err(format!("unexpected map subtype {}", e.sub));
        }
        let key_info = *info.entries.get((e.key >> 16) as usize & 0xFFF).ok_or("map Key entry missing")?;
        let item_info = *info.entries.get(e.key as usize & 0xFFF).ok_or("map Item entry missing")?;
        self.put(bi, at, &[1])?;
        let items: Vec<_> = element_children(node).collect();
        let count =
            u16::try_from(items.len()).map_err(|_| format!("{} pairs are more than a map holds", items.len()))?;
        self.put(bi, at + 16, &count.to_be_bytes())?;
        self.put(bi, at + 18, &count.to_be_bytes())?;
        if items.is_empty() {
            return Ok(());
        }
        // GTA V: the pairs are a structure with Key and Item fields, named
        // after the block that holds them. RDR2: key at 0 (padded to 8), item at 8.
        let same = |a: &Entry, b: &Entry| a.kind == b.kind && a.sub == b.sub && a.key == b.key;
        let (key_name, item_name) = (names::parse("Key"), names::parse("Item"));
        let pair = self.pso.pairs.iter().find(|(_, st)| {
            st.find(key_name).is_some_and(|k| same(k, &key_info))
                && st.find(item_name).is_some_and(|i| same(i, &item_info))
        });
        let (block_name, key_entry, item_entry, pair_len) = match pair {
            Some((name, st)) => {
                (*name, *st.find(key_name).expect("found"), *st.find(item_name).expect("found"), st.length)
            }
            None if self.is_rdr2() => {
                let item_size = match (item_info.kind, item_info.sub) {
                    (ty::STRUCTURE, 0) => self.struct_info(item_info.key)?.length,
                    (ty::STRING, 7 | 8) | (ty::UINT | ty::SINT | ty::ENUM | ty::FLOAT, _) => 4,
                    _ => 8,
                };
                (
                    ARRAYINFO,
                    Entry { offset: 0, ..key_info },
                    Entry { offset: 8, ..item_info },
                    (8 + item_size).next_multiple_of(8),
                )
            }
            None => return Err("no structure describes the pairs of this map".into()),
        };
        let (pb, poff) = self.alloc(block_name, items.len() * pair_len, align_of(pair_len));
        self.pointer(bi, at + 8, pb, poff);
        for (i, child) in items.into_iter().enumerate() {
            let base = poff + i * pair_len;
            let key = attr(child, "key")?;
            let key_at = base + key_entry.offset;
            match key_entry.kind {
                ty::STRING => self.string(pb, key_at, &key_entry, key)?,
                ty::ENUM | ty::UINT | ty::SINT => self.put(pb, key_at, &names::parse_value(key).to_be_bytes())?,
                _ => self.put(pb, key_at, &(int(key)? as u32).to_be_bytes())?,
            }
            match item_entry.kind {
                ty::STRING => self.string(pb, base + item_entry.offset, &item_entry, &text(child))?,
                ty::STRUCTURE if item_entry.key != 0 && item_entry.sub != 3 => {
                    self.structure(pb, base + item_entry.offset, item_entry.key, child, depth + 1)?;
                }
                ty::STRUCTURE => {
                    let actual = self.struct_named(attr(child, "type")?)?;
                    let len = self.struct_info(actual)?.length;
                    let (sb, soff) = self.alloc(actual, len, align_of(len));
                    self.pointer(pb, base + item_entry.offset, sb, soff);
                    self.structure(sb, soff, actual, child, depth + 1)?;
                }
                _ => {
                    let v = element_children(child)
                        .find(|c| c.tag_name().name() == "Value")
                        .ok_or("map item without Value")?;
                    self.entry(pb, base, &item_entry, info, v, depth + 1)?;
                }
            }
        }
        Ok(())
    }

    fn floats(&mut self, bi: usize, at: usize, v: &[f32]) -> R<()> {
        let bytes: Vec<u8> = v.iter().flat_map(|f| f.to_be_bytes()).collect();
        self.put(bi, at, &bytes)
    }

    fn flags(&mut self, bi: usize, at: usize, e: &Entry, info: &StructInfo, node: Node<'_, '_>) -> R<()> {
        let index = (e.key & 0xFFF) as usize;
        let flags = (index != 0xFFF)
            .then(|| info.entries.get(index))
            .flatten()
            .filter(|x| x.name == ARRAYINFO)
            .and_then(|x| self.pso.enums.get(&x.key));
        let n_words = match e.sub {
            0..=2 => 1,
            4 => ((e.key >> 16).clamp(1, 4096) as usize).div_ceil(32),
            s => return Err(format!("unexpected flags subtype {s}")),
        };
        let mut words = vec![0u32; n_words];
        let content = node.attribute("value").map(str::to_owned).unwrap_or_else(|| text(node));
        match flags {
            Some(values) => {
                // The reader joins the names of the set bits in the enum's
                // order with single spaces (unnamed values give empty names):
                // matched in that order, whatever it wrote comes back exactly.
                // Other text (edited by hand) is taken name by name.
                let bit_of = |bit: i32| usize::try_from(bit).ok().filter(|b| b / 32 < n_words);
                let mut tokens = content.split(' ').peekable();
                for (n, bit) in values {
                    if let Some(b) = bit_of(*bit)
                        && tokens.peek().is_some_and(|t| *t == names::field(*n))
                    {
                        words[b / 32] |= 1 << (b % 32);
                        tokens.next();
                    }
                }
                if tokens.next().is_some() || content.is_empty() {
                    words.iter_mut().for_each(|w| *w = 0);
                    for name in content.split_whitespace() {
                        let b = values
                            .iter()
                            .find(|(n, _)| names::field(*n) == name)
                            .and_then(|(_, bit)| bit_of(*bit))
                            .ok_or_else(|| format!("\"{name}\" is not a flag of this field"))?;
                        words[b / 32] |= 1 << (b % 32);
                    }
                }
            }
            None if content.trim().is_empty() => {}
            None if n_words == 1 => words[0] = int(&content)? as u32,
            None => {
                for (w, hex) in words.iter_mut().zip(content.split_whitespace()) {
                    *w = u32::from_str_radix(hex, 16).map_err(|_| format!("\"{hex}\" is not hex"))?;
                }
            }
        }
        match e.sub {
            0 => self.put(bi, at, &words[0].to_be_bytes()),
            1 => self.put(bi, at, &(words[0] as u16).to_be_bytes()),
            2 => self.put(bi, at, &[words[0] as u8]),
            _ => {
                let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_be_bytes()).collect();
                self.put(bi, at, &bytes)
            }
        }
    }

    fn string(&mut self, bi: usize, at: usize, e: &Entry, s: &str) -> R<()> {
        match e.sub {
            0 => {
                let n = (e.key >> 16) as usize;
                if s.len() > n {
                    return Err(format!("\"{s}\" is longer than the {n} bytes the field holds"));
                }
                self.put(bi, at, s.as_bytes())
            }
            1 => {
                if !s.is_empty() {
                    let (cb, coff) = self.chars(s);
                    self.pointer(bi, at, cb, coff);
                }
                Ok(())
            }
            2 => {
                {
                    // Empty strings point at a terminator, as in the game's files.
                    let (cb, coff) = self.chars(s);
                    self.pointer(bi, at, cb, coff);
                }
                Ok(())
            }
            3 => self.counted_string(bi, at, s),
            7..=15 => self.put(bi, at, &names::parse_value(s.trim()).to_be_bytes()),
            s => Err(format!("unexpected string subtype {s}")),
        }
    }

    /// A string pointer followed by its length and size with the terminator (16 bytes).
    fn counted_string(&mut self, bi: usize, at: usize, s: &str) -> R<()> {
        if s.is_empty() {
            return Ok(());
        }
        let n = u16::try_from(s.len()).map_err(|_| "string too long".to_owned())?;
        let (cb, coff) = self.chars(s);
        self.pointer(bi, at, cb, coff);
        // GTA V mostly writes the size first, RDR2 the length.
        let (first, second) = if self.is_rdr2() { (n, n + 1) } else { (n + 1, n) };
        self.put(bi, at + 8, &first.to_be_bytes())?;
        self.put(bi, at + 10, &second.to_be_bytes())
    }

    /// An array field, mirroring how [`Pso::array_node`] reads it.
    fn array(
        &mut self,
        bi: usize,
        base: usize,
        e: &Entry,
        info: &StructInfo,
        node: Node<'_, '_>,
        depth: usize,
    ) -> R<()> {
        let at = base + e.offset;
        let mut index = (e.key & 0xFFFF) as usize;
        if index >= info.entries.len() {
            index = (e.key & 0xFFF) as usize;
        }
        let item = *info.entries.get(index).ok_or("ARRAYINFO not found")?;
        let external =
            matches!(e.sub, 0 | 5) || (e.sub == 4 && item.sub == 3) || matches!(item.kind, ty::STRING | ty::ENUM);
        if !external && !matches!(e.sub, 1 | 2 | 4 | 129) {
            return Err(format!("unsupported array subtype {}", e.sub));
        }
        if item.kind == ty::ARRAY {
            let n = (e.key >> 16) as usize;
            let items: Vec<_> = element_children(node).collect();
            if items.len() > n {
                return Err(format!("{} arrays where the structure holds {n}", items.len()));
            }
            for (i, child) in items.into_iter().enumerate() {
                let inner = Entry { offset: 0, ..item };
                self.array(bi, at + i * 16, &inner, info, child, depth + 1)?;
            }
            return Ok(());
        }
        let count = match item.kind {
            ty::STRUCTURE | ty::STRING | ty::ENUM => element_children(node).count(),
            ty::FLOAT2 => tokens(node).len().div_ceil(2),
            ty::FLOAT3 | ty::FLOAT3A => tokens(node).len().div_ceil(3),
            ty::FLOAT4 | ty::FLOAT4A | ty::VEC4V => tokens(node).len().div_ceil(4),
            _ => tokens(node).len(),
        };
        if count == 0 {
            // Empty: no storage, and the item type may not even be in the schema.
            if external {
                self.put(bi, at + 8, &[0; 4])?;
            }
            return Ok(());
        }
        // Item size and the block name of external storage.
        let (size, block_name) = match (item.kind, item.sub) {
            (ty::STRUCTURE, 3 | 4) => (8, POINTERS),
            (ty::STRUCTURE, _) => (self.struct_info(item.key)?.length, item.key),
            (ty::STRING, 7..=15) => (4, UINTS),
            (ty::STRING, 2) => (8, POINTERS),
            (ty::STRING, 3) => (16, COUNTED_STRINGS),
            (ty::STRING, s) => return Err(format!("unexpected string array subtype {s}")),
            (ty::FLOAT2, _) => (8, ty::FLOAT2 as u32),
            (ty::FLOAT3 | ty::FLOAT3A | ty::FLOAT4 | ty::FLOAT4A | ty::VEC4V, _) => (16, item.kind as u32),
            (ty::LONG | ty::ULONG, _) => (8, item.kind as u32),
            (ty::SBYTE | ty::UBYTE | ty::BOOL, _) => (1, item.kind as u32),
            (ty::USHORT, _) => (2, item.kind as u32),
            (ty::FLOAT | ty::UINT | ty::SINT, _) => (4, item.kind as u32),
            (ty::ENUM, _) => (4, UINTS),
            (other, _) => return Err(format!("unsupported array item type {other:#04x}")),
        };
        // Where the items go: (block, offset).
        let (ab, aoff) = if external {
            let n = u16::try_from(count).map_err(|_| format!("{count} items are more than an array holds"))?;
            self.put(bi, at + 8, &n.to_be_bytes())?;
            self.put(bi, at + 10, &n.to_be_bytes())?;
            let (ab, aoff) = self.alloc(block_name, count * size, align_of(size));
            self.pointer(bi, at, ab, aoff);
            (ab, aoff)
        } else {
            let n = (e.key >> 16) as usize;
            if count > n {
                return Err(format!("{count} items where the structure holds {n}"));
            }
            (bi, at)
        };
        match item.kind {
            ty::STRUCTURE if matches!(item.sub, 3 | 4) => {
                for (i, child) in element_children(node).enumerate() {
                    if let Some(t) = child.attribute("type") {
                        let actual = self.struct_named(t)?;
                        let len = self.struct_info(actual)?.length;
                        let (pb, poff) = self.alloc(actual, len, align_of(len));
                        self.pointer(ab, aoff + i * 8, pb, poff);
                        self.structure(pb, poff, actual, child, depth + 1)?;
                    }
                }
            }
            ty::STRUCTURE => {
                for (i, child) in element_children(node).enumerate() {
                    self.structure(ab, aoff + i * size, item.key, child, depth + 1)?;
                }
            }
            ty::STRING => {
                for (i, child) in element_children(node).enumerate() {
                    let s = text(child);
                    let p = aoff + i * size;
                    if item.sub >= 7 {
                        self.put(ab, p, &names::parse_value(s.trim()).to_be_bytes())?;
                    } else if item.sub == 3 {
                        self.counted_string(ab, p, &s)?;
                    } else if !s.is_empty() {
                        let (cb, coff) = self.chars(&s);
                        self.pointer(ab, p, cb, coff);
                    }
                }
            }
            ty::ENUM => {
                for (i, child) in element_children(node).enumerate() {
                    let v = self.enum_value(item.key, &text(child))?;
                    self.put(ab, aoff + i * 4, &(v as u32).to_be_bytes())?;
                }
            }
            ty::FLOAT2 | ty::FLOAT3 | ty::FLOAT3A | ty::FLOAT4 | ty::FLOAT4A | ty::VEC4V | ty::FLOAT => {
                let per = match item.kind {
                    ty::FLOAT => 1,
                    ty::FLOAT2 => 2,
                    ty::FLOAT3 | ty::FLOAT3A => 3,
                    _ => 4,
                };
                for (i, t) in tokens(node).iter().enumerate() {
                    let p = aoff + (i / per) * size + (i % per) * 4;
                    self.put(ab, p, &float(t)?.to_be_bytes())?;
                }
            }
            _ => {
                for (i, t) in tokens(node).iter().enumerate() {
                    let v = int(t)?;
                    let p = aoff + i * size;
                    match size {
                        1 => self.put(ab, p, &[v as u8])?,
                        2 => self.put(ab, p, &(v as u16).to_be_bytes())?,
                        4 => self.put(ab, p, &(v as u32).to_be_bytes())?,
                        _ => self.put(ab, p, &(v as u64).to_be_bytes())?,
                    }
                }
            }
        }
        Ok(())
    }

    fn encode_pointer(&self, id: usize, offset: usize) -> R<[u8; 8]> {
        if id > 0xFFF {
            return Err(format!("{id} blocks are more than pointers can address"));
        }
        let mut out = [0u8; 8];
        if self.is_rdr2() {
            if offset >= 1 << 28 {
                return Err(format!("offset {offset:#x} is beyond what pointers can address"));
            }
            let word = id as u32 | ((offset as u32 & 0xFFF) << 20);
            out[..4].copy_from_slice(&word.to_be_bytes());
            out[4..6].copy_from_slice(&((offset >> 12) as u16).to_le_bytes());
        } else {
            if offset >= MAX_OFFSET {
                return Err(format!("a block of over {} MB cannot be addressed", MAX_OFFSET >> 20));
            }
            out[..4].copy_from_slice(&(id as u32 | (offset as u32) << 12).to_be_bytes());
        }
        Ok(out)
    }

    /// Puts the blocks in order, resolves pointers and assembles the file.
    fn finish(mut self, root: usize) -> R<Vec<u8>> {
        let pso = self.pso;
        // Blocks keep the original's order; new types follow.
        let rank = |name: u32| pso.blocks.iter().position(|b| b.name == name).unwrap_or(usize::MAX);
        let mut order: Vec<usize> = (0..self.blocks.len()).collect();
        order.sort_by_key(|&i| (rank(self.blocks[i].name), i));
        let mut id = vec![0usize; self.blocks.len()];
        for (n, &i) in order.iter().enumerate() {
            id[i] = n + 1;
        }
        for f in std::mem::take(&mut self.fixes) {
            let p = self.encode_pointer(id[f.target], f.offset)?;
            self.put(f.block, f.at, &p)?;
        }
        if order.len() > 0x7FFF {
            return Err(format!("{} blocks are more than a block map holds", order.len()));
        }
        let mut psin = b"PSIN\0\0\0\0pppppppp".to_vec();
        let mut map = Vec::with_capacity(16 * order.len());
        for &i in &order {
            let b = &self.blocks[i];
            psin.resize(psin.len().next_multiple_of(16), b'p');
            map.extend_from_slice(&b.name.to_be_bytes());
            map.extend_from_slice(&(psin.len() as u32).to_be_bytes());
            map.extend_from_slice(&0u32.to_be_bytes());
            map.extend_from_slice(&(b.data.len() as u32).to_be_bytes());
            psin.extend_from_slice(&b.data);
        }
        let len = u32::try_from(psin.len()).map_err(|_| "the data is over 4 GB".to_owned())?;
        psin[4..8].copy_from_slice(&len.to_be_bytes());
        let mut pmap = b"PMAP".to_vec();
        let header = if self.is_rdr2() { 24 } else { 16 };
        pmap.extend_from_slice(&((header + map.len()) as u32).to_be_bytes());
        pmap.extend_from_slice(&(id[root] as u32).to_be_bytes());
        let count = (order.len() as u16).to_be_bytes();
        if self.is_rdr2() {
            // RDR2: no count at 12, the count at 16 and the blocks from 24.
            pmap.extend_from_slice(&[0; 4]);
            pmap.extend_from_slice(&count);
            pmap.extend_from_slice(b"pppppp");
        } else {
            pmap.extend_from_slice(&count);
            pmap.extend_from_slice(b"pp");
        }
        pmap.extend_from_slice(&map);

        let same = pso.sections.iter().all(|(ident, s)| match ident {
            b"PSIN" => *s == psin,
            b"PMAP" => *s == pmap,
            _ => true,
        });
        let mut out = Vec::with_capacity(psin.len() + pmap.len() + 4096);
        for (ident, s) in &pso.sections {
            match ident {
                b"PSIN" => out.extend_from_slice(&psin),
                b"PMAP" => out.extend_from_slice(&pmap),
                b"CHKS" if !same => {}
                _ => out.extend_from_slice(s),
            }
        }
        Ok(out)
    }
}

/// Builds a PSO file from XML in the form [`Pso::to_xml`] writes, with the
/// schema and the other sections of `template` (the file the XML came from).
pub(super) fn from_xml(template: &Pso, xml: &str) -> Result<Vec<u8>, PsoError> {
    let err = PsoError::Invalid;
    let doc = Document::parse(xml).map_err(|e| err(format!("the XML cannot be read: {e}")))?;
    let root = doc.root_element();
    let root_block = template.block(template.root_block).ok_or_else(|| err("the original has no root block".into()))?;
    let root_name = names::field(root_block.name);
    if root.tag_name().name() != root_name {
        return Err(err(format!("the root element is <{}>, the file needs <{root_name}>", root.tag_name().name())));
    }
    let mut b = Builder::new(template);
    let len = b.struct_info(root_block.name).map_err(err)?.length;
    let (bi, off) = b.alloc(root_block.name, len, align_of(len));
    b.structure(bi, off, root_block.name, root, 0).map_err(err)?;
    b.finish(bi).map_err(err)
}

impl Pso {
    /// Builds a PSO file from XML as [`Pso::to_xml`] writes it, keeping this
    /// file's schema: the way to write back an edited copy of this file.
    pub fn from_xml(&self, xml: &str) -> Result<Vec<u8>, PsoError> {
        from_xml(self, xml)
    }
}

#[cfg(test)]
mod tests {
    use rage_core::hash::{jenkins_bytes, joaat};

    use super::super::{parse, parse_rdr2};

    fn h(name: &str) -> u32 {
        jenkins_bytes(name.as_bytes())
    }

    /// A PSO file with a list of two items (a hash and a float each), laid
    /// out like the game's: items in block 1, the root in block 2.
    fn tiny() -> Vec<u8> {
        let (root, item) = (h("CPedModelInfo__InitDataList"), h("CPedModelInfo__InitData"));
        let mut items = Vec::new();
        for (name, radius) in [("adder", 1.5f32), ("banshee", -2.0)] {
            items.extend(joaat(name).to_be_bytes());
            items.extend(radius.to_be_bytes());
        }
        let mut root_data = 1u32.to_be_bytes().to_vec(); // block 1, offset 0
        root_data.extend([0, 0, 0, 0, 0, 2, 0, 2, 0, 0, 0, 0]);
        let mut psin = b"PSIN\0\0\0\0pppppppp".to_vec();
        psin.extend(&items);
        psin.extend(&root_data);
        let len = psin.len() as u32;
        psin[4..8].copy_from_slice(&len.to_be_bytes());
        let mut pmap = b"PMAP".to_vec();
        pmap.extend(48u32.to_be_bytes());
        pmap.extend(2u32.to_be_bytes());
        pmap.extend([0, 2, b'p', b'p']);
        for (name, off, len) in [(item, 16u32, 16u32), (root, 32, 16)] {
            for v in [name, off, 0, len] {
                pmap.extend(v.to_be_bytes());
            }
        }
        let entry = |name: u32, kind: u8, sub: u8, offset: u16, key: u32| {
            let mut e = name.to_be_bytes().to_vec();
            e.extend([kind, sub]);
            e.extend(offset.to_be_bytes());
            e.extend(key.to_be_bytes());
            e
        };
        let structure = |length: u32, entries: Vec<Vec<u8>>| {
            let mut s = (entries.len() as u32).to_be_bytes().to_vec();
            s.extend(length.to_be_bytes());
            s.extend(0u32.to_be_bytes());
            s.extend(entries.concat());
            s
        };
        let root_info = structure(16, vec![entry(0x100, 0x0c, 0, 0, item), entry(h("InitDatas"), 0x0d, 0, 0, 0)]);
        let item_info = structure(8, vec![entry(h("Name"), 0x0b, 7, 0, 0), entry(h("Radius"), 0x07, 0, 4, 0)]);
        let index_len = 12 + 2 * 8;
        let mut psch = b"PSCH".to_vec();
        psch.extend(((index_len + root_info.len() + item_info.len()) as u32).to_be_bytes());
        psch.extend(2u32.to_be_bytes());
        psch.extend(root.to_be_bytes());
        psch.extend((index_len as u32).to_be_bytes());
        psch.extend(item.to_be_bytes());
        psch.extend(((index_len + root_info.len()) as u32).to_be_bytes());
        psch.extend(root_info);
        psch.extend(item_info);
        [psin, pmap, psch].concat()
    }

    #[test]
    fn writes_back_the_same_bytes() {
        let file = tiny();
        let pso = parse(&file).unwrap();
        let (xml, errors) = pso.to_xml();
        assert!(errors.is_empty(), "{errors:?}");
        assert!(xml.contains("<Name>adder</Name>") || xml.contains("<Name>hash_"), "{xml}");
        assert!(xml.contains("<Radius value=\"-2\" />"), "{xml}");
        assert_eq!(pso.from_xml(&xml).unwrap(), file);
    }

    #[test]
    fn writes_edits() {
        let pso = parse(&tiny()).unwrap();
        let (xml, _) = pso.to_xml();
        let edited = xml.replace("<Radius value=\"1.5\" />", "<Radius value=\"7.25\" />").replace(
            "  </InitDatas>",
            "   <Item>\n    <Name>zentorno</Name>\n    <Radius value=\"3\" />\n   </Item>\n  </InitDatas>",
        );
        assert_ne!(edited, xml);
        let written = pso.from_xml(&edited).unwrap();
        let (again, errors) = parse(&written).unwrap().to_xml();
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(again, edited.replace("zentorno", &crate::names::value(joaat("zentorno"))));
        assert!(again.contains("<Radius value=\"7.25\" />"));
    }

    #[test]
    fn refuses_what_does_not_fit() {
        let pso = parse(&tiny()).unwrap();
        let (xml, _) = pso.to_xml();
        let unknown = xml.replace("<Radius value=\"1.5\" />", "<Radius value=\"1.5\" />\n    <Speed value=\"1\" />");
        assert!(pso.from_xml(&unknown).unwrap_err().to_string().contains("no field Speed"));
        assert!(pso.from_xml(&xml.replace("value=\"1.5\"", "value=\"fast\"")).is_err());
        assert!(pso.from_xml("<Other />").is_err());
        assert!(pso.from_xml("not xml").is_err());
    }

    #[test]
    fn rdr2_layout() {
        // The same file read as RDR2's reads the same; written back it uses RDR2's block map.
        let pso = parse_rdr2(&tiny()).unwrap();
        let (xml, _) = pso.to_xml();
        let written = pso.from_xml(&xml).unwrap();
        let again = parse_rdr2(&written).unwrap();
        assert_eq!(again.to_xml().0, xml);
        let pmap = written.windows(4).position(|w| w == b"PMAP").unwrap();
        assert_eq!(&written[pmap + 12..pmap + 18], &[0, 0, 0, 0, 0, 2]);
    }
}
