//! Minimal RPF7 writer producing OpenIV-style archives.
//!
//! Archives are written with an unencrypted ("OPEN") table of contents, the
//! format OpenIV uses for files in the `mods` folder. Entries can be added from
//! new content (files, RSC7 resources) or copied verbatim from another archive
//! ([`RawEntry`], [`ArchiveWriter::add_copy`]), which keeps encrypted entries
//! valid without re-encrypting.
//!
//! Layout: header, entries (directory children contiguous, sorted by
//! lowercase name), names, then data in 512-byte blocks.

use std::collections::BTreeMap;
use std::io::Write;
use std::sync::Arc;

use flate2::Compression;
use flate2::write::DeflateEncoder;

use crate::toc::{self, BLOCK, MAX_U24, Meta, TocFile, err};
use crate::{Encryption, EntryId, EntryKind, Rpf7, RpfError};

const RSC7_MAGIC: u32 = 0x3743_5352;

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

impl RawEntry {
    pub(crate) fn meta(&self) -> Meta {
        match self {
            RawEntry::Binary { data, compressed, size, encrypted } => Meta::Binary {
                stored_len: data.len() as u32,
                compressed: *compressed,
                size: *size,
                encrypted: *encrypted,
            },
            RawEntry::Resource { data, system_flags, graphics_flags } => {
                Meta::Resource { len: data.len() as u32, system_flags: *system_flags, graphics_flags: *graphics_flags }
            }
        }
    }

    pub(crate) fn data(&self) -> &[u8] {
        match self {
            RawEntry::Binary { data, .. } | RawEntry::Resource { data, .. } => data,
        }
    }
}

fn normalize(path: &str) -> String {
    path.replace('\\', "/").trim_matches('/').to_owned()
}

/// A plain file as it should be stored. Nested `.rpf` archives are stored
/// uncompressed (the game requires that); everything else is deflated
/// unless that would not fit the 24-bit size field.
pub fn file_entry(path: &str, data: Vec<u8>) -> Result<RawEntry, RpfError> {
    let size = u32::try_from(data.len()).map_err(|_| err("file larger than 4 GiB"))?;
    if path.to_ascii_lowercase().ends_with(".rpf") {
        return Ok(RawEntry::Binary { data, compressed: false, size, encrypted: false });
    }
    let mut enc = DeflateEncoder::new(Vec::new(), Compression::default());
    enc.write_all(&data)?;
    let packed = enc.finish()?;
    Ok(if packed.len() < data.len() && packed.len() <= MAX_U24 as usize {
        RawEntry::Binary { data: packed, compressed: true, size, encrypted: false }
    } else {
        RawEntry::Binary { data, compressed: false, size, encrypted: false }
    })
}

/// A resource given as a standalone RSC7 file (as exported).
pub fn resource_entry(path: &str, rsc7: Vec<u8>) -> Result<RawEntry, RpfError> {
    if path.to_ascii_lowercase().ends_with(".ysc") {
        // Scripts are stored NG-encrypted; re-encryption is not supported yet.
        return Err(err(format!("{path}: adding decrypted scripts is not supported")));
    }
    if rsc7.len() < 16 || u32::from_le_bytes(rsc7[0..4].try_into().unwrap()) != RSC7_MAGIC {
        return Err(err(format!("{path} is not an RSC7 resource")));
    }
    let system_flags = u32::from_le_bytes(rsc7[8..12].try_into().unwrap());
    let graphics_flags = u32::from_le_bytes(rsc7[12..16].try_into().unwrap());
    Ok(RawEntry::Resource { data: rsc7, system_flags, graphics_flags })
}

/// Turns file contents into an entry: RSC7 files become resources.
pub fn entry_for(path: &str, data: Vec<u8>) -> Result<RawEntry, RpfError> {
    if data.starts_with(b"RSC7") { resource_entry(path, data) } else { file_entry(path, data) }
}

/// Metadata of an entry in an existing archive.
pub(crate) fn existing_meta(rpf: &Rpf7, id: EntryId, path: &str) -> Result<Meta, RpfError> {
    match rpf.entry(id).kind {
        EntryKind::Binary { stored_size, size, encrypted, .. } => {
            if encrypted && rpf.encryption() == Encryption::Aes {
                return Err(err(format!("{path}: AES-encrypted entries cannot be copied")));
            }
            let stored_len = if stored_size == 0 { size } else { stored_size };
            Ok(Meta::Binary { stored_len, compressed: stored_size > 0, size, encrypted })
        }
        EntryKind::Resource { size, system_flags, graphics_flags, .. } => {
            Ok(Meta::Resource { len: size, system_flags, graphics_flags })
        }
        EntryKind::Directory { .. } => Err(err(format!("{path}: is a directory"))),
    }
}

/// Where an entry's bytes come from.
enum Item {
    Raw(RawEntry),
    /// Copied verbatim from another archive while writing (no full copy in memory).
    Copy(Arc<Rpf7>, EntryId),
}

impl Item {
    fn meta(&self, path: &str) -> Result<Meta, RpfError> {
        match self {
            Item::Raw(r) => Ok(r.meta()),
            Item::Copy(rpf, id) => existing_meta(rpf, *id, path),
        }
    }

    fn bytes(&self) -> Result<std::borrow::Cow<'_, [u8]>, RpfError> {
        Ok(match self {
            Item::Raw(r) => r.data().into(),
            Item::Copy(rpf, id) => match rpf.read_raw_entry(*id)? {
                RawEntry::Binary { data, .. } | RawEntry::Resource { data, .. } => data.into(),
            },
        })
    }
}

#[derive(Default)]
pub struct ArchiveWriter {
    /// Keyed by lowercase path so lookups and ordering are case-insensitive.
    entries: BTreeMap<String, (String, Item)>,
}

impl ArchiveWriter {
    pub fn new() -> Self {
        Self::default()
    }

    fn insert(&mut self, path: &str, item: Item) {
        let clean = normalize(path);
        self.entries.insert(clean.to_ascii_lowercase(), (clean, item));
    }

    /// Adds or replaces an entry from raw stored bytes.
    pub fn add_raw(&mut self, path: &str, entry: RawEntry) {
        self.insert(path, Item::Raw(entry));
    }

    /// Adds or replaces an entry that is copied from another archive while writing.
    pub fn add_copy(&mut self, path: &str, rpf: Arc<Rpf7>, id: EntryId) {
        self.insert(path, Item::Copy(rpf, id));
    }

    /// Adds or replaces a plain file (see [`file_entry`]).
    pub fn add_file(&mut self, path: &str, data: Vec<u8>) -> Result<(), RpfError> {
        let entry = file_entry(path, data)?;
        self.add_raw(path, entry);
        Ok(())
    }

    /// Adds or replaces a resource given as a standalone RSC7 file (as exported).
    pub fn add_resource_file(&mut self, path: &str, rsc7: Vec<u8>) -> Result<(), RpfError> {
        let entry = resource_entry(path, rsc7)?;
        self.add_raw(path, entry);
        Ok(())
    }

    /// Removes an entry (and everything below it, if it is a directory path).
    pub fn remove(&mut self, path: &str) -> usize {
        let key = normalize(path).to_ascii_lowercase();
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

    /// Serializes the archive into memory.
    pub fn build(&self) -> Result<Vec<u8>, RpfError> {
        let mut out = Vec::new();
        self.write_to(&mut out)?;
        Ok(out)
    }

    /// Streams the archive to `out`; copied entries are read one at a time.
    /// Returns the number of bytes written.
    pub fn write_to<W: Write>(&self, out: &mut W) -> Result<u64, RpfError> {
        let items: Vec<&(String, Item)> = self.entries.values().collect();
        let metas: Vec<Meta> = items.iter().map(|(p, it)| it.meta(p)).collect::<Result<_, _>>()?;
        let mut files: Vec<TocFile<'_>> =
            items.iter().zip(&metas).map(|((p, _), m)| TocFile { path: p, meta: *m, block: 0 }).collect();
        // First pass: table size and data order; second pass: real offsets.
        let (probe, order) = toc::build(&files, &[])?;
        let mut cursor = (probe.len() as u64).div_ceil(BLOCK) * BLOCK;
        for &fi in &order {
            files[fi].block = (cursor / BLOCK) as u32;
            cursor += (files[fi].meta.len() as u64).div_ceil(BLOCK) * BLOCK;
        }
        let (mut header, _) = toc::build(&files, &[])?;
        header.resize((header.len() as u64).div_ceil(BLOCK) as usize * BLOCK as usize, 0);

        let mut written = 0u64;
        let mut put = |out: &mut W, bytes: &[u8]| -> Result<(), RpfError> {
            out.write_all(bytes)?;
            written += bytes.len() as u64;
            Ok(())
        };
        put(out, &header)?;
        let zeros = [0u8; BLOCK as usize];
        for fi in order {
            let bytes = items[fi].1.bytes()?;
            if let Meta::Resource { len, .. } = metas[fi] {
                let mut h = bytes[..16].to_vec();
                toc::patch_large_resource_header(&mut h, len);
                put(out, &h)?;
                put(out, &bytes[16..])?;
            } else {
                put(out, &bytes)?;
            }
            let pad = (BLOCK as usize - bytes.len() % BLOCK as usize) % BLOCK as usize;
            put(out, &zeros[..pad])?;
        }
        Ok(written)
    }
}
