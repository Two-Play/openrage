//! Building an RPF7 table of contents (header, entries, names).
//!
//! Shared by the archive writer and the in-place editor. Directory children
//! are contiguous and sorted by lowercase name; directories are numbered
//! breadth-first; the root has the empty name at offset 0. The table is
//! written unencrypted ("OPEN"), as OpenIV does.

use std::collections::BTreeMap;

use crate::RpfError;

pub(crate) const BLOCK: u64 = 512;
pub(crate) const MAX_U24: u32 = 0xFF_FFFF;
const OPEN: u32 = 0x4E45_504F;
const MAGIC_RPF7: u32 = 0x5250_4637;

/// What the table of contents needs to know about a file entry.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Meta {
    Binary { stored_len: u32, compressed: bool, size: u32, encrypted: bool },
    Resource { len: u32, system_flags: u32, graphics_flags: u32 },
}

impl Meta {
    /// Bytes the entry occupies in the archive.
    pub(crate) fn len(&self) -> u32 {
        match *self {
            Meta::Binary { stored_len, .. } => stored_len,
            Meta::Resource { len, .. } => len,
        }
    }
}

pub(crate) struct TocFile<'a> {
    pub path: &'a str,
    pub meta: Meta,
    /// Data position in 512-byte blocks.
    pub block: u32,
}

pub(crate) fn err(reason: impl Into<String>) -> RpfError {
    RpfError::CorruptToc { archive: "<new archive>".into(), reason: reason.into() }
}

/// Returns the table bytes (unpadded) and the indices of `files` in table order.
/// `dirs` lists directories that must exist even if empty.
pub(crate) fn build(files: &[TocFile<'_>], dirs: &[&str]) -> Result<(Vec<u8>, Vec<usize>), RpfError> {
    struct Node {
        name: String,
        children: BTreeMap<String, usize>,
        file: Option<usize>,
    }
    let mut nodes = vec![Node { name: String::new(), children: BTreeMap::new(), file: None }];
    let insert = |nodes: &mut Vec<Node>, path: &str, file: Option<usize>| -> Result<(), RpfError> {
        let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
        let mut cur = 0;
        for (i, part) in parts.iter().enumerate() {
            let key = part.to_ascii_lowercase();
            let next = match nodes[cur].children.get(&key) {
                Some(&n) => n,
                None => {
                    nodes.push(Node { name: (*part).to_owned(), children: BTreeMap::new(), file: None });
                    let n = nodes.len() - 1;
                    nodes[cur].children.insert(key, n);
                    n
                }
            };
            let last = i + 1 == parts.len();
            if last && file.is_some() {
                if !nodes[next].children.is_empty() || nodes[next].file.is_some() {
                    return Err(err(format!("{path} is both a file and a directory")));
                }
                nodes[next].file = file;
            } else if nodes[next].file.is_some() {
                return Err(err(format!("{path} lies below a file")));
            }
            cur = next;
        }
        Ok(())
    };
    for d in dirs {
        insert(&mut nodes, d, None)?;
    }
    for (fi, f) in files.iter().enumerate() {
        insert(&mut nodes, f.path, Some(fi))?;
    }

    // Breadth-first order: every directory's children are contiguous.
    let mut order = vec![0usize];
    let mut first_child = vec![0u32; nodes.len()];
    let mut i = 0;
    while i < order.len() {
        let n = order[i];
        if nodes[n].file.is_none() {
            first_child[n] = order.len() as u32;
            order.extend(nodes[n].children.values().copied());
        }
        i += 1;
    }

    let mut names = Vec::new();
    let mut name_off = vec![0u32; nodes.len()];
    for &n in &order {
        name_off[n] = names.len() as u32;
        names.extend_from_slice(nodes[n].name.as_bytes());
        names.push(0);
    }
    while names.len() % 16 != 0 {
        names.push(0);
    }
    if names.len() > 0xFFFF {
        return Err(err("name table exceeds 64 KiB"));
    }

    let mut out = Vec::with_capacity(16 + order.len() * 16 + names.len());
    out.extend_from_slice(&MAGIC_RPF7.to_le_bytes());
    out.extend_from_slice(&(order.len() as u32).to_le_bytes());
    out.extend_from_slice(&(names.len() as u32).to_le_bytes());
    out.extend_from_slice(&OPEN.to_le_bytes());
    let mut file_order = Vec::new();
    for &n in &order {
        let mut e = [0u8; 16];
        match nodes[n].file {
            None => {
                e[0..4].copy_from_slice(&name_off[n].to_le_bytes());
                e[4..8].copy_from_slice(&0x7FFF_FF00u32.to_le_bytes());
                e[8..12].copy_from_slice(&first_child[n].to_le_bytes());
                e[12..16].copy_from_slice(&(nodes[n].children.len() as u32).to_le_bytes());
            }
            Some(fi) => {
                let f = &files[fi];
                if f.block > 0x7F_FFFF {
                    return Err(err("archive exceeds 4 GiB"));
                }
                e[0..2].copy_from_slice(&(name_off[n] as u16).to_le_bytes());
                match f.meta {
                    Meta::Binary { stored_len, compressed, size, encrypted } => {
                        let stored = if compressed { stored_len } else { 0 };
                        if stored > MAX_U24 {
                            return Err(err(format!("{}: compressed size too large", f.path)));
                        }
                        e[2..5].copy_from_slice(&stored.to_le_bytes()[..3]);
                        e[5..8].copy_from_slice(&f.block.to_le_bytes()[..3]);
                        e[8..12].copy_from_slice(&size.to_le_bytes());
                        e[12..16].copy_from_slice(&u32::from(encrypted).to_le_bytes());
                    }
                    Meta::Resource { len, system_flags, graphics_flags } => {
                        if len < 16 {
                            return Err(err(format!("{}: resource too short", f.path)));
                        }
                        e[2..5].copy_from_slice(&len.min(MAX_U24).to_le_bytes()[..3]);
                        e[5..8].copy_from_slice(&(f.block | 0x80_0000).to_le_bytes()[..3]);
                        e[8..12].copy_from_slice(&system_flags.to_le_bytes());
                        e[12..16].copy_from_slice(&graphics_flags.to_le_bytes());
                    }
                }
                file_order.push(fi);
            }
        }
        out.extend_from_slice(&e);
    }
    out.extend_from_slice(&names);
    Ok((out, file_order))
}

/// Patches the in-archive header of a resource ≥ 16 MiB, whose size does not
/// fit the 24-bit field and is scattered across header bytes instead.
pub(crate) fn patch_large_resource_header(header: &mut [u8], len: u32) {
    if len >= MAX_U24 {
        header[2] = (len >> 24) as u8;
        header[5] = (len >> 16) as u8;
        header[7] = len as u8;
        header[14] = (len >> 8) as u8;
    }
}
