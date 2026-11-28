//! Builders for synthetic PSO files used by the reader and writer tests.

use rage_core::hash::{jenkins_bytes, joaat};

use super::ARRAYINFO;
use crate::names;

/// A name the XML shows as the text given: registered, hashed like the game does.
pub(super) fn nm(name: &str) -> u32 {
    names::register([name]);
    joaat(name)
}

/// A name hashed case-sensitively (`Key` and `Item` of map pairs).
pub(super) fn exact(name: &str) -> u32 {
    jenkins_bytes(name.as_bytes())
}

#[derive(Clone, Copy)]
pub(super) struct Field {
    pub name: u32,
    pub kind: u8,
    pub sub: u8,
    pub offset: usize,
    pub key: u32,
}

#[derive(Clone)]
pub(super) struct Def {
    pub name: u32,
    pub entries: Vec<Field>,
    end: usize,
}

impl Def {
    pub fn new(name: &str) -> Self {
        Self { name: nm(name), entries: Vec::new(), end: 0 }
    }

    /// Adds a field of `size` bytes after the last one; returns its offset in the structure.
    pub fn add(&mut self, name: &str, kind: u8, sub: u8, size: usize, key: u32) -> usize {
        self.add_named(nm(name), kind, sub, size, key)
    }

    pub fn add_named(&mut self, name: u32, kind: u8, sub: u8, size: usize, key: u32) -> usize {
        let align = if size >= 4 { 4 } else { size.max(1) };
        let offset = self.end.next_multiple_of(align);
        self.end = offset + size;
        self.entries.push(Field { name, kind, sub, offset, key });
        offset
    }

    /// Adds a field that has no data of its own, only describes one (an array item, a flags enum, a map key).
    /// Returns its index among the entries.
    pub fn info(&mut self, kind: u8, sub: u8, key: u32) -> u32 {
        self.entries.push(Field { name: ARRAYINFO, kind, sub, offset: 0, key });
        self.entries.len() as u32 - 1
    }

    pub fn length(&self) -> usize {
        self.end.next_multiple_of(4)
    }
}

pub(super) struct Spec {
    pub structs: Vec<Def>,
    pub enums: Vec<(u32, Vec<(u32, i32)>)>,
    /// Block name and data, in file order.
    pub blocks: Vec<(u32, Vec<u8>)>,
    /// 1-based id of the root block.
    pub root: u32,
    pub rdr2: bool,
    /// Sections after `PSCH`.
    pub extra: Vec<([u8; 4], Vec<u8>)>,
}

impl Spec {
    pub fn new(rdr2: bool) -> Self {
        Self { structs: Vec::new(), enums: Vec::new(), blocks: Vec::new(), root: 1, rdr2, extra: Vec::new() }
    }

    fn psin(&self) -> (Vec<u8>, Vec<u8>) {
        let mut psin = b"PSIN\0\0\0\0pppppppp".to_vec();
        let mut map = Vec::new();
        for (name, data) in &self.blocks {
            psin.resize(psin.len().next_multiple_of(16), b'p');
            map.extend(name.to_be_bytes());
            map.extend((psin.len() as u32).to_be_bytes());
            map.extend(0u32.to_be_bytes());
            map.extend((data.len() as u32).to_be_bytes());
            psin.extend(data);
        }
        let len = psin.len() as u32;
        psin[4..8].copy_from_slice(&len.to_be_bytes());
        (psin, map)
    }

    fn pmap(&self, map: &[u8]) -> Vec<u8> {
        let header = if self.rdr2 { 24 } else { 16 };
        let mut pmap = b"PMAP".to_vec();
        pmap.extend(((header + map.len()) as u32).to_be_bytes());
        pmap.extend(self.root.to_be_bytes());
        let count = (self.blocks.len() as u16).to_be_bytes();
        if self.rdr2 {
            pmap.extend([0; 4]);
            pmap.extend(count);
            pmap.extend(b"pppppp");
        } else {
            pmap.extend(count);
            pmap.extend(b"pp");
        }
        pmap.extend(map);
        pmap
    }

    fn psch(&self) -> Vec<u8> {
        let mut elements: Vec<(u32, Vec<u8>)> = Vec::new();
        for d in &self.structs {
            let mut e = (d.entries.len() as u32).to_be_bytes().to_vec();
            e.extend((d.length() as u32).to_be_bytes());
            e.extend(0u32.to_be_bytes());
            for f in &d.entries {
                e.extend(f.name.to_be_bytes());
                e.extend([f.kind, f.sub]);
                e.extend((f.offset as u16).to_be_bytes());
                e.extend(f.key.to_be_bytes());
            }
            elements.push((d.name, e));
        }
        for (name, values) in &self.enums {
            let mut e = (1u32 << 24 | values.len() as u32).to_be_bytes().to_vec();
            for (n, v) in values {
                e.extend(n.to_be_bytes());
                if self.rdr2 {
                    e.extend([0; 8]);
                }
                e.extend(v.to_be_bytes());
            }
            elements.push((*name, e));
        }
        let index_len = 12 + elements.len() * 8;
        let total = index_len + elements.iter().map(|(_, e)| e.len()).sum::<usize>();
        let mut psch = b"PSCH".to_vec();
        psch.extend((total as u32).to_be_bytes());
        psch.extend((elements.len() as u32).to_be_bytes());
        let mut at = index_len;
        for (name, e) in &elements {
            psch.extend(name.to_be_bytes());
            psch.extend((at as u32).to_be_bytes());
            at += e.len();
        }
        for (_, e) in &elements {
            psch.extend(e);
        }
        psch
    }

    pub fn build(&self) -> Vec<u8> {
        let (psin, map) = self.psin();
        let mut out = [psin, self.pmap(&map), self.psch()].concat();
        for (ident, payload) in &self.extra {
            out.extend(ident);
            out.extend((payload.len() as u32 + 8).to_be_bytes());
            out.extend(payload);
        }
        out
    }
}

/// An 8-byte pointer to `offset` in block `block` (1-based).
pub(super) fn pointer(block: u32, offset: u32, rdr2: bool) -> [u8; 8] {
    let mut out = [0u8; 8];
    if rdr2 {
        out[..4].copy_from_slice(&(block | (offset & 0xFFF) << 20).to_be_bytes());
        out[4..6].copy_from_slice(&((offset >> 12) as u16).to_le_bytes());
    } else {
        out[..4].copy_from_slice(&(block | offset << 12).to_be_bytes());
    }
    out
}

pub(super) fn put(d: &mut Vec<u8>, at: usize, bytes: &[u8]) {
    if d.len() < at + bytes.len() {
        d.resize(at + bytes.len(), 0);
    }
    d[at..at + bytes.len()].copy_from_slice(bytes);
}
