//! RPF7 — the archive format of GTA V (Legacy and Enhanced).
//!
//! Layout: 16-byte header, then a table of contents (`entry_count` × 16-byte
//! entries followed by a block of NUL-terminated names), usually encrypted.
//! File data lives in 512-byte blocks addressed relative to the archive start.
//!
//! Entry kinds are distinguished by the second little-endian word:
//! `0x7FFFFF00` marks a directory, a set top bit marks a resource
//! (RSC7 container), everything else is a plain binary file.

use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use flate2::read::DeflateDecoder;
use rage_core::io::{read_exact_at, u24_le, u32_le};
use rage_crypto::GtaVKeys;
use rage_resource::{Resource, page_size_from_flags};

use crate::RpfError;

const MAGIC_RPF7: u32 = 0x5250_4637;
const DIR_MARKER: u32 = 0x7FFF_FF00;
const BLOCK: u64 = 512;
const RESOURCE_HEADER: usize = 16;
const RSC7_MAGIC: u32 = 0x3743_5352; // "RSC7"
const MAX_ENTRIES: u32 = 1 << 20;

pub type EntryId = usize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encryption {
    None,
    /// Unencrypted TOC written by modding tools ("OPEN").
    Open,
    Aes,
    Ng,
    /// Unknown marker; treated as NG like the game does.
    Unknown(u32),
}

impl Encryption {
    fn from_raw(v: u32) -> Self {
        match v {
            0 => Self::None,
            0x4E45_504F => Self::Open,
            0x0FFF_FFF9 => Self::Aes,
            0x0FEF_FFFF => Self::Ng,
            other => Self::Unknown(other),
        }
    }
}

#[derive(Debug, Clone)]
pub enum EntryKind {
    Directory {
        first: u32,
        count: u32,
    },
    Binary {
        /// Byte offset relative to the archive start.
        offset: u64,
        /// Compressed size; 0 means stored uncompressed.
        stored_size: u32,
        /// Uncompressed size.
        size: u32,
        encrypted: bool,
    },
    Resource {
        offset: u64,
        /// Size of the full RSC7 container (header + compressed pages).
        size: u32,
        system_flags: u32,
        graphics_flags: u32,
    },
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub parent: Option<EntryId>,
    pub kind: EntryKind,
}

impl Entry {
    pub fn is_dir(&self) -> bool {
        matches!(self.kind, EntryKind::Directory { .. })
    }

    /// True for a binary entry that is itself an RPF archive.
    pub fn is_archive(&self) -> bool {
        matches!(self.kind, EntryKind::Binary { .. }) && self.name.to_ascii_lowercase().ends_with(".rpf")
    }

    /// Size of the extracted file (uncompressed for binaries, container size for resources).
    pub fn size(&self) -> u64 {
        match self.kind {
            EntryKind::Directory { .. } => 0,
            EntryKind::Binary { size, .. } => size as u64,
            EntryKind::Resource { size, .. } => size as u64,
        }
    }

    /// Resource format version, packed into the top nibbles of both page flag words.
    pub fn resource_version(&self) -> Option<u32> {
        match self.kind {
            EntryKind::Resource { system_flags, graphics_flags, .. } => {
                Some(((system_flags >> 28) & 0xF) << 4 | ((graphics_flags >> 28) & 0xF))
            }
            _ => None,
        }
    }

    /// Uncompressed (system, graphics) page sizes of a resource, decoded from its flags.
    pub fn resource_page_sizes(&self) -> Option<(u64, u64)> {
        match self.kind {
            EntryKind::Resource { system_flags, graphics_flags, .. } => {
                Some((page_size_from_flags(system_flags) as u64, page_size_from_flags(graphics_flags) as u64))
            }
            _ => None,
        }
    }

    pub fn kind_label(&self) -> &'static str {
        match self.kind {
            EntryKind::Directory { .. } => "dir",
            EntryKind::Binary { .. } if self.is_archive() => "rpf",
            EntryKind::Binary { .. } => "file",
            EntryKind::Resource { .. } => "resource",
        }
    }
}

/// An opened RPF7 archive (top-level or nested).
pub struct Rpf7 {
    file: Arc<File>,
    keys: Option<Arc<GtaVKeys>>,
    name: String,
    display_path: String,
    base: u64,
    size: u64,
    encryption: Encryption,
    entries: Vec<Entry>,
}

impl Rpf7 {
    /// Opens a top-level archive file.
    pub fn open(path: &Path, keys: Option<Arc<GtaVKeys>>) -> Result<Self, RpfError> {
        let file = File::open(path)?;
        let size = file.metadata()?.len();
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        Self::open_at(Arc::new(file), 0, size, name, path.display().to_string(), keys)
    }

    fn open_at(
        file: Arc<File>,
        base: u64,
        size: u64,
        name: String,
        display_path: String,
        keys: Option<Arc<GtaVKeys>>,
    ) -> Result<Self, RpfError> {
        let corrupt = |reason: &str| RpfError::CorruptToc { archive: display_path.clone(), reason: reason.into() };

        let mut header = [0u8; 16];
        read_exact_at(&file, &mut header, base)?;
        let magic = u32_le(&header, 0);
        if magic != MAGIC_RPF7 {
            return Err(RpfError::BadMagic(display_path, magic));
        }
        let entry_count = u32_le(&header, 4);
        let names_len = u32_le(&header, 8);
        let encryption = Encryption::from_raw(u32_le(&header, 12));

        if entry_count == 0 || entry_count > MAX_ENTRIES {
            return Err(corrupt("implausible entry count"));
        }
        let toc_entries = entry_count as usize * 16;
        if 16 + toc_entries as u64 + names_len as u64 > size {
            return Err(corrupt("table of contents exceeds archive size"));
        }

        let mut entries_raw = vec![0u8; toc_entries];
        let mut names = vec![0u8; names_len as usize];
        read_exact_at(&file, &mut entries_raw, base + 16)?;
        read_exact_at(&file, &mut names, base + 16 + toc_entries as u64)?;

        match encryption {
            Encryption::None | Encryption::Open => {}
            Encryption::Aes => {
                let k = keys.as_deref().ok_or_else(|| RpfError::KeysRequired(display_path.clone()))?;
                k.decrypt_aes(&mut entries_raw);
                k.decrypt_aes(&mut names);
            }
            Encryption::Ng | Encryption::Unknown(_) => {
                let k = keys.as_deref().ok_or_else(|| RpfError::KeysRequired(display_path.clone()))?;
                k.decrypt_ng(&mut entries_raw, &name, size as u32);
                k.decrypt_ng(&mut names, &name, size as u32);
            }
        }

        let read_name = |off: usize| -> String {
            let tail = names.get(off..).unwrap_or(&[]);
            let end = tail.iter().position(|&b| b == 0).unwrap_or(tail.len());
            String::from_utf8_lossy(&tail[..end]).into_owned()
        };

        let mut entries = Vec::with_capacity(entry_count as usize);
        for raw in entries_raw.as_chunks::<16>().0 {
            let second = u32_le(raw, 4);
            let entry = if second == DIR_MARKER {
                Entry {
                    name: read_name(u32_le(raw, 0) as usize),
                    parent: None,
                    kind: EntryKind::Directory { first: u32_le(raw, 8), count: u32_le(raw, 12) },
                }
            } else if second & 0x8000_0000 == 0 {
                let encrypted = match u32_le(raw, 12) {
                    0 => false,
                    1 => true,
                    _ => return Err(corrupt("invalid binary entry encryption flag")),
                };
                Entry {
                    name: read_name(u16::from_le_bytes([raw[0], raw[1]]) as usize),
                    parent: None,
                    kind: EntryKind::Binary {
                        offset: u24_le(raw, 5) as u64 * BLOCK,
                        stored_size: u24_le(raw, 2),
                        size: u32_le(raw, 8),
                        encrypted,
                    },
                }
            } else {
                let offset = (u24_le(raw, 5) & 0x7F_FFFF) as u64 * BLOCK;
                let mut res_size = u24_le(raw, 2);
                if res_size == 0xFF_FFFF {
                    // Sizes ≥ 16 MiB are scattered across the RSC7 header instead.
                    let mut h = [0u8; 16];
                    read_exact_at(&file, &mut h, base + offset)?;
                    res_size = (h[7] as u32) | (h[14] as u32) << 8 | (h[5] as u32) << 16 | (h[2] as u32) << 24;
                }
                Entry {
                    name: read_name(u16::from_le_bytes([raw[0], raw[1]]) as usize),
                    parent: None,
                    kind: EntryKind::Resource {
                        offset,
                        size: res_size,
                        system_flags: u32_le(raw, 8),
                        graphics_flags: u32_le(raw, 12),
                    },
                }
            };
            entries.push(entry);
        }

        if !entries[0].is_dir() {
            return Err(corrupt("root entry is not a directory"));
        }
        // Link parents starting at the root; each entry may be claimed only once,
        // which also rules out cycles in a malformed table of contents.
        let mut pending = vec![0usize];
        while let Some(dir) = pending.pop() {
            let EntryKind::Directory { first, count } = entries[dir].kind else { continue };
            let (first, count) = (first as usize, count as usize);
            if first == 0 || first.checked_add(count).is_none_or(|end| end > entries.len()) {
                return Err(corrupt("directory range out of bounds"));
            }
            for (child, entry) in entries[first..first + count].iter_mut().enumerate() {
                if entry.parent.is_some() {
                    return Err(corrupt("cyclic or shared directory entries"));
                }
                entry.parent = Some(dir);
                if entry.is_dir() {
                    pending.push(first + child);
                }
            }
        }

        Ok(Self { file, keys, name, display_path, base, size, encryption, entries })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Human-readable path of this archive, including any parent archives.
    pub fn display_path(&self) -> &str {
        &self.display_path
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn encryption(&self) -> Encryption {
        self.encryption
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn entry(&self, id: EntryId) -> &Entry {
        &self.entries[id]
    }

    pub const ROOT: EntryId = 0;

    /// Children of a directory (empty for files).
    pub fn children(&self, dir: EntryId) -> impl Iterator<Item = EntryId> + '_ {
        let len = self.entries.len();
        match self.entries[dir].kind {
            EntryKind::Directory { first, count } => {
                (first as usize).min(len)..(first as usize + count as usize).min(len)
            }
            _ => 0..0,
        }
    }

    /// Path of an entry inside this archive, `/`-separated, without leading slash.
    pub fn path_of(&self, id: EntryId) -> String {
        let mut parts = Vec::new();
        let mut cur = Some(id);
        while let Some(i) = cur {
            if i != Self::ROOT {
                parts.push(self.entries[i].name.as_str());
            }
            cur = self.entries[i].parent;
        }
        parts.reverse();
        parts.join("/")
    }

    /// Case-insensitive child lookup by name.
    pub fn find_child(&self, dir: EntryId, name: &str) -> Option<EntryId> {
        self.children(dir).find(|&c| self.entries[c].name.eq_ignore_ascii_case(name))
    }

    fn read_raw(&self, id: EntryId, offset: u64, len: u64) -> Result<Vec<u8>, RpfError> {
        if offset.checked_add(len).is_none_or(|end| end > self.size) {
            return Err(RpfError::OutOfBounds { archive: self.display_path.clone(), entry: self.path_of(id) });
        }
        let mut buf = vec![0u8; len as usize];
        read_exact_at(&self.file, &mut buf, self.base + offset)?;
        Ok(buf)
    }

    fn keys(&self) -> Result<&GtaVKeys, RpfError> {
        self.keys.as_deref().ok_or_else(|| RpfError::KeysRequired(self.display_path.clone()))
    }

    fn decrypt(&self, data: &mut [u8], name: &str, length: u32) -> Result<(), RpfError> {
        let keys = self.keys()?;
        match self.encryption {
            Encryption::Aes => keys.decrypt_aes(data),
            // Archives with an open TOC can still contain NG-encrypted files.
            _ => keys.decrypt_ng(data, name, length),
        }
        Ok(())
    }

    /// Reads an entry the way it would be exported to disk:
    /// binaries decrypted and decompressed, resources as a standalone RSC7 file
    /// (header + compressed pages), the same layout OpenIV and CodeWalker export.
    pub fn read(&self, id: EntryId) -> Result<Vec<u8>, RpfError> {
        let entry = &self.entries[id];
        match entry.kind {
            EntryKind::Directory { .. } => Err(RpfError::NotFound(self.path_of(id))),
            EntryKind::Binary { offset, stored_size, size, encrypted } => {
                let on_disk = if stored_size == 0 { size } else { stored_size };
                let mut data = self.read_raw(id, offset, on_disk as u64)?;
                if encrypted {
                    self.decrypt(&mut data, &entry.name, size)?;
                }
                if stored_size == 0 {
                    return Ok(data);
                }
                let mut out = Vec::with_capacity(size as usize);
                DeflateDecoder::new(&data[..]).read_to_end(&mut out).map_err(|source| RpfError::Inflate {
                    archive: self.display_path.clone(),
                    entry: self.path_of(id),
                    source,
                })?;
                Ok(out)
            }
            EntryKind::Resource { offset, size, system_flags, graphics_flags } => {
                let mut data = self.read_raw(id, offset, size as u64)?;
                if data.len() < RESOURCE_HEADER {
                    return Err(RpfError::OutOfBounds { archive: self.display_path.clone(), entry: self.path_of(id) });
                }
                // Scripts are the only resources stored encrypted.
                if entry.name.to_ascii_lowercase().ends_with(".ysc") {
                    self.decrypt(&mut data[RESOURCE_HEADER..], &entry.name, size)?;
                }
                // Inside the archive the first 16 bytes are archive bookkeeping, not a
                // standalone header. Exported files get a proper RSC7 header instead.
                let version = entry.resource_version().unwrap_or_default();
                data[0..4].copy_from_slice(&RSC7_MAGIC.to_le_bytes());
                data[4..8].copy_from_slice(&version.to_le_bytes());
                data[8..12].copy_from_slice(&system_flags.to_le_bytes());
                data[12..16].copy_from_slice(&graphics_flags.to_le_bytes());
                Ok(data)
            }
        }
    }

    /// Returns an entry's stored bytes unchanged (still compressed/encrypted),
    /// for copying it into another archive.
    pub fn read_raw_entry(&self, id: EntryId) -> Result<crate::writer::RawEntry, RpfError> {
        use crate::writer::RawEntry;
        match self.entries[id].kind {
            EntryKind::Directory { .. } => Err(RpfError::NotFound(self.path_of(id))),
            EntryKind::Binary { offset, stored_size, size, encrypted } => {
                let on_disk = if stored_size == 0 { size } else { stored_size };
                let data = self.read_raw(id, offset, on_disk as u64)?;
                if encrypted && self.encryption == Encryption::Aes {
                    // Raw AES data cannot be moved into an OPEN archive (it would be read as NG).
                    return Err(RpfError::NotAnArchive(format!("{} (AES-encrypted entry)", self.path_of(id))));
                }
                Ok(RawEntry::Binary { data, compressed: stored_size > 0, size, encrypted })
            }
            EntryKind::Resource { offset, size, system_flags, graphics_flags } => {
                Ok(RawEntry::Resource { data: self.read_raw(id, offset, size as u64)?, system_flags, graphics_flags })
            }
        }
    }

    /// Reads a resource and returns its inflated page data (system + graphics pages).
    pub fn read_resource_pages(&self, id: EntryId) -> Result<Vec<u8>, RpfError> {
        let container = self.read(id)?;
        let body = container.get(RESOURCE_HEADER..).unwrap_or(&[]);
        let mut out = Vec::new();
        DeflateDecoder::new(body).read_to_end(&mut out).map_err(|source| RpfError::Inflate {
            archive: self.display_path.clone(),
            entry: self.path_of(id),
            source,
        })?;
        Ok(out)
    }

    /// Reads a resource entry as a [`Resource`] (inflated and split into memory segments).
    pub fn read_resource(&self, id: EntryId) -> Result<Resource, RpfError> {
        let entry = &self.entries[id];
        let EntryKind::Resource { system_flags, graphics_flags, .. } = entry.kind else {
            return Err(RpfError::NotFound(format!("{} is not a resource", self.path_of(id))));
        };
        let pages = self.read_resource_pages(id)?;
        let version = entry.resource_version().unwrap_or_default();
        Resource::from_pages(version, system_flags, graphics_flags, pages).map_err(|e| RpfError::BadResource {
            path: format!("{}/{}", self.display_path, self.path_of(id)),
            reason: e.to_string(),
        })
    }

    /// Opens a nested `.rpf` entry as an archive (no copy; reads go to the same file).
    pub fn open_child(&self, id: EntryId) -> Result<Rpf7, RpfError> {
        let entry = &self.entries[id];
        let EntryKind::Binary { offset, stored_size: 0, size, encrypted: false } = entry.kind else {
            return Err(RpfError::NotAnArchive(self.path_of(id)));
        };
        if !entry.is_archive() || offset + size as u64 > self.size {
            return Err(RpfError::NotAnArchive(self.path_of(id)));
        }
        Rpf7::open_at(
            Arc::clone(&self.file),
            self.base + offset,
            size as u64,
            entry.name.clone(),
            format!("{}/{}", self.display_path, self.path_of(id)),
            self.keys.clone(),
        )
    }
}
