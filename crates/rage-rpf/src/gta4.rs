//! GTA IV archives: IMG3 image files and RPF2/RPF3 packages.
//!
//! * **IMG3** (`.img`, most game data): a 20-byte header, a table of 16-byte
//!   entries followed by their names, data in 2048-byte blocks. The table and
//!   the first 16 header bytes are usually encrypted.
//! * **RPF2** (`.rpf`): a table at 0x800 of 16-byte entries (directories and
//!   files) followed by a name table. The table and, when flagged, the file
//!   data are encrypted.
//! * **RPF3** (audio `.rpf`): like RPF2, but entries carry name hashes
//!   instead of names.
//!
//! Encryption is AES-256-ECB applied 16 times with the key from the user's
//! own `GTAIV.exe` ([`Gta4Key`]). Resources (`.wtd`, `.wdr`, `.wft`, …) are
//! stored as RSC5 files and are returned exactly as stored, which is also the
//! form OpenIV exports.

use std::fs::File;
use std::io::Read;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rage_core::io::read_exact_at;
use rage_crypto::Gta4Key;

use crate::RpfError;
use crate::word;

const IMG3_MAGIC: u32 = 0xA94E_2A52;
const IMG_BLOCK: u64 = 2048;
const RPF_TOC_OFFSET: u64 = 0x800;
/// Hard limits against corrupt tables (the largest game archive has ~23 000 entries).
const MAX_ENTRIES: usize = 1 << 20;
const MAX_TOC: usize = 64 << 20;

/// The archive formats of GTA IV.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gta4Format {
    Img3,
    Rpf2,
    Rpf3,
}

impl Gta4Format {
    pub fn name(self) -> &'static str {
        match self {
            Gta4Format::Img3 => "IMG3",
            Gta4Format::Rpf2 => "RPF2",
            Gta4Format::Rpf3 => "RPF3",
        }
    }
}

/// Index of an entry; [`Gta4Archive::ROOT`] is the top folder.
pub type Gta4EntryId = usize;

#[derive(Debug, Clone)]
pub enum Gta4EntryKind {
    Dir {
        children: Range<usize>,
    },
    /// A plain file: `stored` bytes at `offset`, `size` once inflated.
    File {
        offset: u64,
        stored: u64,
        size: u64,
        compressed: bool,
    },
    /// An RSC5 resource of `stored` bytes at `offset` (header included).
    Resource {
        offset: u64,
        stored: u64,
        resource_type: u32,
        flags: u32,
    },
}

#[derive(Debug, Clone)]
pub struct Gta4Entry {
    pub name: String,
    pub kind: Gta4EntryKind,
}

impl Gta4Entry {
    pub fn is_dir(&self) -> bool {
        matches!(self.kind, Gta4EntryKind::Dir { .. })
    }

    /// The RSC5 resource type (8 textures, 110 models, 112 fragments, 32 bounds, …).
    pub fn resource_type(&self) -> Option<u32> {
        match self.kind {
            Gta4EntryKind::Resource { resource_type, .. } => Some(resource_type),
            _ => None,
        }
    }

    /// Size once extracted (resources: as stored).
    pub fn size(&self) -> u64 {
        match self.kind {
            Gta4EntryKind::Dir { .. } => 0,
            Gta4EntryKind::File { size, .. } => size,
            Gta4EntryKind::Resource { stored, .. } => stored,
        }
    }

    /// Memory sizes (system, graphics) of a resource.
    pub fn resource_sizes(&self) -> Option<(usize, usize)> {
        match self.kind {
            Gta4EntryKind::Resource { flags, .. } => Some(rage_resource::rsc5_sizes(flags)),
            _ => None,
        }
    }
}

/// An opened GTA IV archive.
#[derive(Debug)]
pub struct Gta4Archive {
    path: PathBuf,
    display: String,
    format: Gta4Format,
    toc_encrypted: bool,
    data_encrypted: bool,
    entries: Vec<Gta4Entry>,
    /// RPF3: the name hash of every entry (names are known only for some).
    hashes: Vec<u32>,
    key: Option<Arc<Gta4Key>>,
    file: File,
}

/// Names for the hashes RPF3 (audio) packages store instead of names.
pub type AudioNames = std::collections::HashMap<u32, String>;

fn audio_names_slot() -> &'static std::sync::RwLock<Option<Arc<AudioNames>>> {
    static NAMES: std::sync::OnceLock<std::sync::RwLock<Option<Arc<AudioNames>>>> = std::sync::OnceLock::new();
    NAMES.get_or_init(Default::default)
}

/// Registers names for audio packages (from the game's audio configuration;
/// set at start-up, before packages are opened). Entries without a known name
/// keep their hash as eight hex digits. Process-wide, like [`set_default_key`].
pub fn set_audio_names(names: Option<Arc<AudioNames>>) {
    if let Ok(mut n) = audio_names_slot().write() {
        *n = names;
    }
}

/// Adds names to the registered ones (e.g. of entries the user created).
pub fn add_audio_names(more: &[String]) {
    if let Ok(mut slot) = audio_names_slot().write() {
        let mut names = slot.as_deref().cloned().unwrap_or_default();
        for n in more {
            names.entry(audio_hash(n)).or_insert_with(|| n.to_ascii_lowercase());
        }
        *slot = Some(Arc::new(names));
    }
}

/// The hash an RPF3 entry name stands for: eight hex digits are the hash
/// itself, anything else is hashed (joaat of the lower-case name).
pub fn audio_hash(name: &str) -> u32 {
    if name.len() == 8
        && let Ok(h) = u32::from_str_radix(name, 16)
    {
        return h;
    }
    rage_core::hash::joaat(name)
}

fn default_key_slot() -> &'static std::sync::RwLock<Option<Arc<Gta4Key>>> {
    static KEY: std::sync::OnceLock<std::sync::RwLock<Option<Arc<Gta4Key>>>> = std::sync::OnceLock::new();
    KEY.get_or_init(Default::default)
}

/// Registers the key [`Gta4Archive::open_default`] (and so [`crate::vfs::resolve`])
/// uses. This is process-wide state, meant to be set once at start-up; code
/// that knows its key should call [`Gta4Archive::open`] instead.
pub fn set_default_key(key: Option<Arc<Gta4Key>>) {
    if let Ok(mut k) = default_key_slot().write() {
        *k = key;
    }
}

/// The key registered with [`set_default_key`].
pub fn default_key() -> Option<Arc<Gta4Key>> {
    default_key_slot().read().ok()?.clone()
}

/// Whether a file name is a GTA IV archive candidate.
pub fn is_archive_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".img") || lower.ends_with(".rpf")
}

impl Gta4Archive {
    pub const ROOT: Gta4EntryId = 0;

    /// Whether a file is a GTA IV archive. Encrypted IMG3 files have no plain
    /// magic: they are recognised by their `.img` name plus the item size
    /// (16) that stays readable at byte 16.
    pub fn file_is_gta4(path: &Path) -> bool {
        let mut head = [0u8; 20];
        if File::open(path).and_then(|mut f| f.read_exact(&mut head)).is_err() {
            return false;
        }
        if &head[..4] == b"RPF2" || &head[..4] == b"RPF3" || word(&head, 0) == IMG3_MAGIC {
            return true;
        }
        let is_img = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("img"));
        is_img && u16::from_le_bytes([head[16], head[17]]) == 16
    }

    /// Opens an archive with the key registered by [`set_default_key`].
    pub fn open_default(path: &Path) -> Result<Self, RpfError> {
        Self::open(path, default_key())
    }

    /// Opens an archive. Encrypted tables need `key`.
    pub fn open(path: &Path, key: Option<Arc<Gta4Key>>) -> Result<Self, RpfError> {
        let file = File::open(path)?;
        let display = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mut head = [0u8; 24];
        read_exact_at(&file, &mut head, 0)?;
        let mut archive = Gta4Archive {
            path: path.to_owned(),
            display,
            format: Gta4Format::Img3,
            toc_encrypted: false,
            data_encrypted: false,
            entries: Vec::new(),
            hashes: Vec::new(),
            key,
            file,
        };
        match &head[..4] {
            b"RPF2" => archive.read_rpf(&head, Gta4Format::Rpf2)?,
            b"RPF3" => archive.read_rpf(&head, Gta4Format::Rpf3)?,
            _ => archive.read_img(&head)?,
        }
        Ok(archive)
    }

    fn corrupt(&self, reason: impl Into<String>) -> RpfError {
        RpfError::CorruptToc { archive: self.display.clone(), reason: reason.into() }
    }

    fn need_key(&self) -> Result<Arc<Gta4Key>, RpfError> {
        self.key.clone().ok_or_else(|| RpfError::KeysRequired(self.display.clone()))
    }

    fn read_img(&mut self, head: &[u8; 24]) -> Result<(), RpfError> {
        let mut header = [0u8; 20];
        header.copy_from_slice(&head[..20]);
        if word(&header, 0) != IMG3_MAGIC {
            let key = self.need_key()?;
            key.decrypt(&mut header[..16]);
            if word(&header, 0) != IMG3_MAGIC {
                return Err(RpfError::BadMagic(self.display.clone(), word(&head[..], 0)));
            }
            self.toc_encrypted = true;
        }
        if word(&header, 4) != 3 {
            return Err(self.corrupt(format!("unsupported IMG version {}", word(&header, 4))));
        }
        let count = word(&header, 8) as usize;
        let toc_len = word(&header, 12) as usize;
        if count > MAX_ENTRIES || toc_len > MAX_TOC || toc_len < count * 16 {
            return Err(self.corrupt(format!("{count} entries in a {toc_len}-byte table")));
        }
        let mut toc = vec![0u8; toc_len];
        read_exact_at(&self.file, &mut toc, 20)?;
        if self.toc_encrypted {
            self.need_key()?.decrypt(&mut toc);
        }
        let mut names = toc[count * 16..].split(|b| *b == 0);
        let file_len = self.file.metadata()?.len();
        let mut entries = Vec::with_capacity(count + 1);
        entries.push(Gta4Entry { name: String::new(), kind: Gta4EntryKind::Dir { children: 1..count + 1 } });
        for i in 0..count {
            let e = &toc[i * 16..i * 16 + 16];
            let size = word(e, 0);
            let resource_type = word(e, 4);
            let offset = word(e, 8) as u64 * IMG_BLOCK;
            let blocks = u16::from_le_bytes([e[12], e[13]]) as u64;
            let name = names.next().ok_or_else(|| self.corrupt("names are missing"))?;
            let name = String::from_utf8_lossy(name).into_owned();
            let stored = blocks * IMG_BLOCK;
            if offset + stored > file_len {
                return Err(RpfError::OutOfBounds { archive: self.display.clone(), entry: name });
            }
            let kind = if size & 0xC000_0000 == 0xC000_0000 {
                Gta4EntryKind::Resource { offset, stored, resource_type, flags: size }
            } else {
                let size = (size as u64).min(stored);
                Gta4EntryKind::File { offset, stored: size, size, compressed: false }
            };
            entries.push(Gta4Entry { name, kind });
        }
        self.entries = entries;
        Ok(())
    }

    fn read_rpf(&mut self, head: &[u8; 24], format: Gta4Format) -> Result<(), RpfError> {
        self.format = format;
        let toc_len = word(head, 4) as usize;
        let count = word(head, 8) as usize;
        self.toc_encrypted = word(head, 16) != 0;
        self.data_encrypted = format == Gta4Format::Rpf2 && word(head, 20) != 0;
        if count == 0 || count > MAX_ENTRIES || toc_len > MAX_TOC || toc_len < count * 16 {
            return Err(self.corrupt(format!("{count} entries in a {toc_len}-byte table")));
        }
        let mut toc = vec![0u8; toc_len];
        read_exact_at(&self.file, &mut toc, RPF_TOC_OFFSET)?;
        if self.toc_encrypted {
            self.need_key()?.decrypt(&mut toc);
        }
        let names = &toc[count * 16..];
        let name_at = |off: u32| -> String {
            let start = off as usize;
            let Some(rest) = names.get(start..) else { return String::new() };
            let end = rest.iter().position(|b| *b == 0).unwrap_or(rest.len());
            String::from_utf8_lossy(&rest[..end]).into_owned()
        };
        let file_len = self.file.metadata()?.len();
        let audio_names =
            if format == Gta4Format::Rpf3 { audio_names_slot().read().ok().and_then(|n| n.clone()) } else { None };
        let mut entries = Vec::with_capacity(count);
        for i in 0..count {
            let e = &toc[i * 16..i * 16 + 16];
            let (w0, w1, w2, w3) = (word(e, 0), word(e, 4), word(e, 8), word(e, 12));
            let name = if i == 0 {
                String::new()
            } else if format == Gta4Format::Rpf3 {
                self.hashes.resize(i, 0);
                self.hashes.push(w0);
                audio_names.as_ref().and_then(|n| n.get(&w0)).cloned().unwrap_or_else(|| format!("{w0:08x}"))
            } else {
                name_at(w0)
            };
            let kind = if w2 & 0x8000_0000 != 0 {
                let first = (w2 & 0x7FFF_FFFF) as usize;
                // Bit 30 of the count is a flag in some audio archives.
                let n = (w3 & 0x3FFF_FFFF) as usize;
                if first <= i || first + n > count {
                    return Err(self.corrupt(format!("folder {i} lists entries {first}..{}", first + n)));
                }
                Gta4EntryKind::Dir { children: first..first + n }
            } else if w3 & 0xC000_0000 == 0xC000_0000 {
                let offset = (w2 & 0x7FFF_FF00) as u64;
                Gta4EntryKind::Resource { offset, stored: w1 as u64, resource_type: w2 & 0xFF, flags: w3 }
            } else {
                let compressed = w3 & 0x4000_0000 != 0;
                let stored = (w3 & 0xBFFF_FFFF) as u64;
                Gta4EntryKind::File { offset: w2 as u64, stored, size: w1 as u64, compressed }
            };
            if let Gta4EntryKind::File { offset, stored, .. } | Gta4EntryKind::Resource { offset, stored, .. } = kind
                && offset + stored > file_len
            {
                return Err(RpfError::OutOfBounds { archive: self.display.clone(), entry: name });
            }
            entries.push(Gta4Entry { name, kind });
        }
        if !entries[0].is_dir() {
            return Err(self.corrupt("the first entry is not a folder"));
        }
        // Every entry belongs to one folder. Folders sharing entries would make
        // a walk over the tree exponential in the number of folders.
        let mut claimed = vec![false; count];
        for e in &entries {
            if let Gta4EntryKind::Dir { children } = &e.kind {
                for c in children.clone() {
                    if std::mem::replace(&mut claimed[c], true) {
                        return Err(self.corrupt(format!("entry {c} is listed by more than one folder")));
                    }
                }
            }
        }
        self.entries = entries;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn display_name(&self) -> &str {
        &self.display
    }

    pub fn format(&self) -> Gta4Format {
        self.format
    }

    /// Whether the table of contents is encrypted.
    pub fn is_encrypted(&self) -> bool {
        self.toc_encrypted
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.len() <= 1
    }

    pub fn entries(&self) -> &[Gta4Entry] {
        &self.entries
    }

    pub fn entry(&self, id: Gta4EntryId) -> &Gta4Entry {
        &self.entries[id]
    }

    /// The entries of a folder.
    pub fn children(&self, dir: Gta4EntryId) -> Range<usize> {
        match &self.entries[dir].kind {
            Gta4EntryKind::Dir { children } => children.clone(),
            _ => 0..0,
        }
    }

    /// Finds `name` in a folder, ignoring ASCII case.
    pub fn find_child(&self, dir: Gta4EntryId, name: &str) -> Option<Gta4EntryId> {
        if self.format == Gta4Format::Rpf3 {
            // By name or by hash (`061b65da`), whichever the caller has.
            let hash = audio_hash(name);
            return self.children(dir).find(|&i| self.hashes.get(i) == Some(&hash));
        }
        self.children(dir).find(|&i| self.entries[i].name.eq_ignore_ascii_case(name))
    }

    /// The name hash of an RPF3 (audio) entry.
    pub fn audio_hash_of(&self, id: Gta4EntryId) -> Option<u32> {
        (self.format == Gta4Format::Rpf3).then(|| self.hashes.get(id).copied()).flatten()
    }

    /// The path of an entry inside the archive.
    pub fn entry_path(&self, id: Gta4EntryId) -> String {
        let mut parts = vec![self.entries[id].name.as_str()];
        let mut cur = id;
        while cur != Self::ROOT {
            let Some(parent) = (0..cur).rev().find(|&d| self.children(d).contains(&cur)) else { break };
            if parent != Self::ROOT {
                parts.push(self.entries[parent].name.as_str());
            }
            cur = parent;
        }
        parts.reverse();
        parts.join("/")
    }

    /// Reads an entry as it would be extracted: plain files decrypted and
    /// inflated, resources as stored (RSC5 header included).
    pub fn read(&self, id: Gta4EntryId) -> Result<Vec<u8>, RpfError> {
        let entry = &self.entries[id];
        let (offset, stored) = match entry.kind {
            Gta4EntryKind::Dir { .. } => return Err(RpfError::NotFound(format!("{}: is a folder", entry.name))),
            Gta4EntryKind::File { offset, stored, .. } | Gta4EntryKind::Resource { offset, stored, .. } => {
                (offset, stored)
            }
        };
        let mut data = vec![0u8; stored as usize];
        read_exact_at(&self.file, &mut data, offset)?;
        if self.data_encrypted {
            self.need_key()?.decrypt(&mut data);
        }
        match entry.kind {
            Gta4EntryKind::File { compressed: true, size, .. } => {
                let mut out = Vec::with_capacity(size as usize);
                flate2::read::ZlibDecoder::new(&data[..]).take(size).read_to_end(&mut out).map_err(|source| {
                    RpfError::Inflate { archive: self.display.clone(), entry: entry.name.clone(), source }
                })?;
                Ok(out)
            }
            _ => Ok(data),
        }
    }

    /// Reads and inflates a resource entry.
    pub fn read_resource(&self, id: Gta4EntryId) -> Result<rage_resource::Resource, RpfError> {
        let data = self.read(id)?;
        rage_resource::Resource::from_rsc5(&data).map_err(|e| RpfError::BadResource {
            path: format!("{}/{}", self.display, self.entry_path(id)),
            reason: e.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str, data: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("openrage-gta4-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, data).unwrap();
        p
    }

    /// A plain (unencrypted) IMG3 with a text file and a fake resource.
    fn plain_img() -> Vec<u8> {
        let names = b"hello.txt\0thing.wtd\0";
        let toc_len = 2 * 16 + names.len();
        let mut out = Vec::new();
        out.extend_from_slice(&IMG3_MAGIC.to_le_bytes());
        out.extend_from_slice(&3u32.to_le_bytes());
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&(toc_len as u32).to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        // hello.txt: 5 bytes in block 1
        out.extend_from_slice(&5u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        // thing.wtd: resource in block 2
        out.extend_from_slice(&0xC000_0000u32.to_le_bytes());
        out.extend_from_slice(&8u32.to_le_bytes());
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(names);
        out.resize(2048, 0);
        out.extend_from_slice(b"hello");
        out.resize(4096, 0);
        out.extend_from_slice(b"RSC\x05");
        out.resize(6144, 0);
        out
    }

    #[test]
    fn reads_plain_img3() {
        let p = temp_file("plain.img", &plain_img());
        assert!(Gta4Archive::file_is_gta4(&p));
        let img = Gta4Archive::open(&p, None).unwrap();
        assert_eq!(img.format(), Gta4Format::Img3);
        assert!(!img.is_encrypted());
        assert_eq!(img.children(Gta4Archive::ROOT), 1..3);
        let hello = img.find_child(Gta4Archive::ROOT, "HELLO.TXT").unwrap();
        assert_eq!(img.read(hello).unwrap(), b"hello");
        let wtd = img.find_child(Gta4Archive::ROOT, "thing.wtd").unwrap();
        assert_eq!(img.entry(wtd).resource_type(), Some(8));
        assert_eq!(img.entry(wtd).size(), 2048);
        assert_eq!(img.entry_path(wtd), "thing.wtd");
    }

    #[test]
    fn encrypted_img_needs_a_key() {
        let mut data = plain_img();
        data[0] ^= 0x55;
        let p = temp_file("enc.img", &data);
        assert!(Gta4Archive::file_is_gta4(&p));
        assert!(matches!(Gta4Archive::open(&p, None), Err(RpfError::KeysRequired(_))));
    }

    #[test]
    fn reads_plain_rpf2_folders() {
        // root -> data/ -> a.txt
        let names = b"\0data\0a.txt\0";
        let mut toc = Vec::new();
        for w in [0u32, 1, 0x8000_0001, 1, 1, 1, 0x8000_0002, 1, 6, 3, 0x1000, 3] {
            toc.extend_from_slice(&w.to_le_bytes());
        }
        toc.extend_from_slice(names);
        let mut out = Vec::new();
        out.extend_from_slice(b"RPF2");
        for w in [toc.len() as u32, 3, 0, 0, 0] {
            out.extend_from_slice(&w.to_le_bytes());
        }
        out.resize(0x800, 0);
        out.extend_from_slice(&toc);
        out.resize(0x1000, 0);
        out.extend_from_slice(b"abc");
        let p = temp_file("t.rpf", &out);
        let rpf = Gta4Archive::open(&p, None).unwrap();
        let data = rpf.find_child(Gta4Archive::ROOT, "data").unwrap();
        let a = rpf.find_child(data, "a.txt").unwrap();
        assert_eq!(rpf.read(a).unwrap(), b"abc");
        assert_eq!(rpf.entry_path(a), "data/a.txt");
    }
}
