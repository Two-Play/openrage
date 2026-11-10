//! Minimal RPF7 writer producing OpenIV-style archives.
//!
//! Archives are written with an unencrypted ("OPEN") table of contents, the
//! format OpenIV uses for files in the `mods` folder. Entries can be added from
//! new content (files, RSC7 resources) or copied verbatim from another archive
//! ([`RawEntry`]), which keeps encrypted entries valid without re-encrypting.
//!
//! Layout: header, entries (directory children contiguous, sorted by
//! lowercase name), names, then data in 512-byte blocks.

use std::collections::BTreeMap;
use std::io::Write;

use flate2::Compression;
use flate2::write::DeflateEncoder;

use crate::RpfError;

const BLOCK: usize = 512;
const OPEN: u32 = 0x4E45_504F;
const MAGIC_RPF7: u32 = 0x5250_4637;
const RSC7_MAGIC: u32 = 0x3743_5352;
const MAX_U24: u32 = 0xFF_FFFF;

/// Entry data exactly as stored inside an archive.
#[derive(Debug, Clone)]
pub enum RawEntry {
    Binary {
        /// Bytes as stored (possibly compressed and/or encrypted).
        data: Vec<u8>,
        /// True if `data` is deflate-compressed.
        compressed: bool,
        /// Uncompressed size.
        size: u32,
        encrypted: bool,
    },
    Resource {
        /// Full stored bytes including the 16-byte in-archive header.
        data: Vec<u8>,
        system_flags: u32,
        graphics_flags: u32,
    },
}

#[derive(Default)]
pub struct ArchiveWriter {
    /// Keyed by lowercase path so lookups and ordering are case-insensitive.
    entries: BTreeMap<String, (String, RawEntry)>,
}

fn err(reason: impl Into<String>) -> RpfError {
    RpfError::CorruptToc { archive: "<new archive>".into(), reason: reason.into() }
}

impl ArchiveWriter {
    pub fn new() -> Self {
        Self::default()
    }

    fn key(path: &str) -> String {
        path.replace('\\', "/").trim_matches('/').to_ascii_lowercase()
    }

    /// Adds or replaces an entry copied verbatim from another archive.
    pub fn add_raw(&mut self, path: &str, entry: RawEntry) {
        let clean = path.replace('\\', "/").trim_matches('/').to_owned();
        self.entries.insert(Self::key(path), (clean, entry));
    }

    /// Adds or replaces a plain file. Nested `.rpf` archives are stored
    /// uncompressed (the game requires that); everything else is deflated
    /// unless that would not fit the 24-bit size field.
    pub fn add_file(&mut self, path: &str, data: Vec<u8>) -> Result<(), RpfError> {
        let size = u32::try_from(data.len()).map_err(|_| err("file larger than 4 GiB"))?;
        let entry = if Self::key(path).ends_with(".rpf") {
            RawEntry::Binary { data, compressed: false, size, encrypted: false }
        } else {
            let mut enc = DeflateEncoder::new(Vec::new(), Compression::default());
            enc.write_all(&data)?;
            let packed = enc.finish()?;
            if packed.len() < data.len() && packed.len() <= MAX_U24 as usize {
                RawEntry::Binary { data: packed, compressed: true, size, encrypted: false }
            } else {
                RawEntry::Binary { data, compressed: false, size, encrypted: false }
            }
        };
        self.add_raw(path, entry);
        Ok(())
    }

    /// Adds or replaces a resource given as a standalone RSC7 file (as exported).
    pub fn add_resource_file(&mut self, path: &str, rsc7: Vec<u8>) -> Result<(), RpfError> {
        if Self::key(path).ends_with(".ysc") {
            // Scripts are stored NG-encrypted; re-encryption is not supported yet.
            return Err(err(format!("{path}: adding decrypted scripts is not supported")));
        }
        if rsc7.len() < 16 || u32::from_le_bytes(rsc7[0..4].try_into().unwrap()) != RSC7_MAGIC {
            return Err(err(format!("{path} is not an RSC7 resource")));
        }
        let system_flags = u32::from_le_bytes(rsc7[8..12].try_into().unwrap());
        let graphics_flags = u32::from_le_bytes(rsc7[12..16].try_into().unwrap());
        self.add_raw(path, RawEntry::Resource { data: rsc7, system_flags, graphics_flags });
        Ok(())
    }

    /// Removes an entry (and everything below it, if it is a directory path).
    pub fn remove(&mut self, path: &str) -> usize {
        let key = Self::key(path);
        let prefix = format!("{key}/");
        let before = self.entries.len();
        self.entries.retain(|k, _| *k != key && !k.starts_with(&prefix));
        before - self.entries.len()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Serializes the archive.
    pub fn build(&self) -> Result<Vec<u8>, RpfError> {
        // Build the directory tree: node 0 is the root.
        struct Node {
            name: String,
            children: BTreeMap<String, usize>,
            file: Option<usize>, // index into `files`
        }
        let mut nodes = vec![Node { name: String::new(), children: BTreeMap::new(), file: None }];
        let files: Vec<&(String, RawEntry)> = self.entries.values().collect();
        for (fi, (path, _)) in files.iter().enumerate() {
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
                if i + 1 == parts.len() {
                    if !nodes[next].children.is_empty() || nodes[next].file.is_some() {
                        return Err(err(format!("{path} is both a file and a directory")));
                    }
                    nodes[next].file = Some(fi);
                } else if nodes[next].file.is_some() {
                    return Err(err(format!("{path} lies below a file")));
                }
                cur = next;
            }
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

        // Names (root's empty name at offset 0).
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

        let toc_len = 16 + order.len() * 16 + names.len();
        let data_start = toc_len.div_ceil(BLOCK) * BLOCK;
        let mut data = Vec::new();
        let mut toc = Vec::with_capacity(order.len() * 16);
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
                    let block = ((data_start + data.len()) / BLOCK) as u32;
                    if block > 0x7F_FFFF {
                        return Err(err("archive exceeds 4 GiB"));
                    }
                    e[0..2].copy_from_slice(&(name_off[n] as u16).to_le_bytes());
                    match &files[fi].1 {
                        RawEntry::Binary { data: bytes, compressed, size, encrypted } => {
                            let stored = if *compressed { bytes.len() as u32 } else { 0 };
                            if stored > MAX_U24 {
                                return Err(err(format!("{}: compressed size too large", files[fi].0)));
                            }
                            e[2..5].copy_from_slice(&stored.to_le_bytes()[..3]);
                            e[5..8].copy_from_slice(&block.to_le_bytes()[..3]);
                            e[8..12].copy_from_slice(&size.to_le_bytes());
                            e[12..16].copy_from_slice(&u32::from(*encrypted).to_le_bytes());
                            data.extend_from_slice(bytes);
                        }
                        RawEntry::Resource { data: bytes, system_flags, graphics_flags } => {
                            if bytes.len() < 16 {
                                return Err(err(format!("{}: resource too short", files[fi].0)));
                            }
                            let size = bytes.len() as u32;
                            let mut header = bytes[..16].to_vec();
                            if size >= MAX_U24 {
                                // Large sizes are scattered across the in-archive header.
                                header[2] = (size >> 24) as u8;
                                header[5] = (size >> 16) as u8;
                                header[7] = size as u8;
                                header[14] = (size >> 8) as u8;
                            }
                            e[2..5].copy_from_slice(&size.min(MAX_U24).to_le_bytes()[..3]);
                            e[5..8].copy_from_slice(&(block | 0x80_0000).to_le_bytes()[..3]);
                            e[8..12].copy_from_slice(&system_flags.to_le_bytes());
                            e[12..16].copy_from_slice(&graphics_flags.to_le_bytes());
                            data.extend_from_slice(&header);
                            data.extend_from_slice(&bytes[16..]);
                        }
                    }
                    while data.len() % BLOCK != 0 {
                        data.push(0);
                    }
                }
            }
            toc.extend_from_slice(&e);
        }

        let mut out = Vec::with_capacity(data_start + data.len());
        out.extend_from_slice(&MAGIC_RPF7.to_le_bytes());
        out.extend_from_slice(&(order.len() as u32).to_le_bytes());
        out.extend_from_slice(&(names.len() as u32).to_le_bytes());
        out.extend_from_slice(&OPEN.to_le_bytes());
        out.extend_from_slice(&toc);
        out.extend_from_slice(&names);
        out.resize(data_start, 0);
        out.extend_from_slice(&data);
        Ok(out)
    }
}
