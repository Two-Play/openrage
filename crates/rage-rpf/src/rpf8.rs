//! RPF8 archives (Red Dead Redemption 2).
//!
//! Layout (all little-endian):
//! - 16-byte header: magic `RPF8`, entry count, names length, decryption tag
//!   (0xFF = table in plaintext), platform character (`'y'` = PC).
//! - 256-byte signature.
//! - `entry count` × 24-byte entries, sorted by name hash. There are no
//!   directories: every entry is a file, identified by the hash of its path.
//! - File data, then (for some archives) a table of the paths, NUL-separated,
//!   in the last `names length` bytes.
//!
//! Most of the game's archives encrypt the entry table and many files with a
//! cipher whose keys only exist in the running game; reading those needs keys
//! the user provides. Without keys, the header is still readable and
//! plaintext archives open fully.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rage_core::hash::joaat;

use crate::RpfError;

const MAGIC: u32 = 0x5250_4638;
const HEADER_LEN: u64 = 16;
const SIGNATURE_LEN: u64 = 256;
const ENTRY_LEN: usize = 24;
/// Decryption tag of an archive whose entry table is not encrypted.
pub const PLAINTEXT: u16 = 0xFF;

/// The 16-byte header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub entry_count: u32,
    pub names_length: u32,
    /// Which key encrypts the entry table ([`PLAINTEXT`] = none).
    pub decryption_tag: u16,
    /// Platform character: `'y'` for PC.
    pub platform: char,
}

impl Header {
    pub fn parse(b: &[u8]) -> Option<Header> {
        if b.len() < HEADER_LEN as usize || u32::from_le_bytes(b[0..4].try_into().ok()?) != MAGIC {
            return None;
        }
        Some(Header {
            entry_count: u32::from_le_bytes(b[4..8].try_into().ok()?),
            names_length: u32::from_le_bytes(b[8..12].try_into().ok()?),
            decryption_tag: u16::from_le_bytes(b[12..14].try_into().ok()?),
            platform: char::from(b[14]),
        })
    }

    pub fn is_encrypted(&self) -> bool {
        self.decryption_tag != PLAINTEXT
    }
}

/// How an entry's data is compressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    None,
    /// Raw deflate, no zlib header.
    Deflate,
    Oodle,
    Unknown(u8),
}

/// One entry of the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Hash of the path (without extension when the extension is in `ext_id`).
    pub hash: u32,
    /// Index into [`EXTENSIONS`], or 0xFF when the hash covers the full path.
    pub ext_id: u8,
    /// Key id of the file data (0xFF = not encrypted).
    pub key_id: u8,
    /// Which parts of the data are encrypted.
    pub encryption_config: u8,
    pub is_resource: bool,
    /// The last 256 bytes of the data are a signature.
    pub signed: bool,
    /// Byte offset of the data in the archive.
    pub offset: u64,
    /// Size of the data on disk.
    pub size_on_disk: u64,
    pub compression: Compression,
    /// Binary files: uncompressed size. Resources: virtual and physical flags.
    pub size_or_flags: u64,
    /// The path, when the archive's name table or a name list knows it.
    pub path: Option<String>,
}

/// Extensions by id; `#` stands for the platform character.
const EXTENSIONS: [(u8, &str); 38] = [
    (0, "rpf"),
    (1, "#mf"),
    (2, "#dr"),
    (3, "#ft"),
    (4, "#dd"),
    (5, "#td"),
    (6, "#bn"),
    (7, "#bd"),
    (8, "#pd"),
    (9, "#bs"),
    (10, "#sd"),
    (11, "#mt"),
    (12, "#sc"),
    (13, "#cs"),
    (64, "mrf"),
    (65, "cut"),
    (66, "gfx"),
    (67, "#cd"),
    (68, "#ld"),
    (69, "#pmd"),
    (70, "#pm"),
    (71, "#ed"),
    (72, "#pt"),
    (73, "#map"),
    (74, "#typ"),
    (75, "#ch"),
    (76, "#ldb"),
    (77, "#jd"),
    (78, "#ad"),
    (79, "#nv"),
    (80, "#hn"),
    (81, "#pl"),
    (82, "#nd"),
    (83, "#vr"),
    (84, "#wr"),
    (85, "#nh"),
    (86, "#fd"),
    (87, "#as"),
];

/// The extension for an id on a platform (`None` for 0xFF and unknown ids).
pub fn extension(ext_id: u8, platform: char) -> Option<String> {
    let (_, ext) = EXTENSIONS.iter().find(|(id, _)| *id == ext_id)?;
    Some(ext.replace('#', &platform.to_string()))
}

/// The id of an extension (`None` when it is stored as part of the hashed path).
pub fn extension_id(ext: &str, platform: char) -> Option<u8> {
    EXTENSIONS.iter().find(|(_, e)| e.replace('#', &platform.to_string()) == ext).map(|(id, _)| *id)
}

/// The hash an entry with this path carries.
pub fn path_hash(path: &str, platform: char) -> (u32, u8) {
    let path = path.to_ascii_lowercase().replace('\\', "/");
    if let Some((stem, ext)) = path.rsplit_once('.')
        && !stem.ends_with('/')
        && let Some(id) = extension_id(ext, platform)
    {
        return (joaat(stem), id);
    }
    (joaat(&path), 0xFF)
}

impl Entry {
    pub fn parse(b: &[u8]) -> Entry {
        let q = |i: usize| u64::from_le_bytes(b[i * 8..i * 8 + 8].try_into().expect("8 bytes"));
        let (q0, q1, q2) = (q(0), q(1), q(2));
        Entry {
            hash: q0 as u32,
            encryption_config: (q0 >> 32) as u8,
            key_id: (q0 >> 40) as u8,
            ext_id: (q0 >> 48) as u8,
            is_resource: q0 >> 56 & 1 == 1,
            signed: q0 >> 57 & 1 == 1,
            size_on_disk: (q1 & 0x0FFF_FFFF) << 4,
            offset: (q1 >> 28 & 0x7FFF_FFFF) << 4,
            compression: match (q1 >> 59) as u8 {
                0 => Compression::None,
                1 => Compression::Deflate,
                2 => Compression::Oodle,
                n => Compression::Unknown(n),
            },
            size_or_flags: q2,
            path: None,
        }
    }

    pub fn is_encrypted(&self) -> bool {
        self.key_id != 0xFF
    }

    /// Size of the file once read: uncompressed size, or both resource page sizes.
    pub fn size(&self) -> u64 {
        if self.is_resource {
            (self.size_or_flags & 0xFFFF_FFF0) + (self.size_or_flags >> 32 & 0xFFFF_FFF0)
        } else {
            self.size_or_flags
        }
    }

    /// Resource version, from the low nibbles of the two flags.
    pub fn resource_version(&self) -> Option<u32> {
        self.is_resource.then_some(((self.size_or_flags as u32 & 0xF) << 4) | ((self.size_or_flags >> 32) as u32 & 0xF))
    }

    /// The entry's name: its path when known, else `hash/XXXXXXXX.ext`.
    pub fn display_name(&self, platform: char) -> String {
        if let Some(p) = &self.path {
            return p.clone();
        }
        match extension(self.ext_id, platform) {
            Some(ext) => format!("hash/{:08X}.{ext}", self.hash),
            None => format!("hash/{:08X}", self.hash),
        }
    }
}

/// The decryption keys of RDR2 PC archives, taken from the user's own game.
pub struct Rdr2Keys {
    pub context: rage_tfit2::Context,
    pub iv: [u8; 16],
    /// By key id (0–162, 0xC0, 0xC5, 0xC6).
    pub keys: HashMap<u8, rage_tfit2::Key>,
}

impl std::fmt::Debug for Rdr2Keys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Rdr2Keys({} keys)", self.keys.len())
    }
}

/// Whether decrypted table bytes look like a real entry table: hashes in
/// ascending order, data inside the file, known compression, valid key ids.
/// Used to recognise the right key without any reference data.
pub fn plausible_table(table: &[u8], archive_size: u64) -> bool {
    let entries: Vec<Entry> = table.as_chunks::<ENTRY_LEN>().0.iter().map(|c| Entry::parse(c)).collect();
    !entries.is_empty()
        && entries.windows(2).all(|w| w[0].hash <= w[1].hash)
        && entries.iter().all(|e| {
            e.offset + e.size_on_disk <= archive_size
                && !matches!(e.compression, Compression::Unknown(_))
                && (e.key_id <= 162 || matches!(e.key_id, 0xC0 | 0xC5 | 0xC6 | 0xFF))
                && (e.ext_id == 0xFF || EXTENSIONS.iter().any(|(id, _)| *id == e.ext_id))
        })
}

/// Keys and names used when archives are opened without passing them, e.g.
/// through the virtual file system. Set once at start-up by the application.
/// The registered keys and names.
type Defaults = (Option<Arc<Rdr2Keys>>, Option<Arc<crate::rpf8_names::NameIndex>>);

fn defaults() -> &'static std::sync::RwLock<Defaults> {
    static DEFAULTS: std::sync::OnceLock<std::sync::RwLock<Defaults>> = std::sync::OnceLock::new();
    DEFAULTS.get_or_init(Default::default)
}

/// Registers the keys and names [`Rpf8::open_default`] uses.
pub fn set_defaults(keys: Option<Arc<Rdr2Keys>>, names: Option<Arc<crate::rpf8_names::NameIndex>>) {
    if let Ok(mut d) = defaults().write() {
        *d = (keys, names);
    }
}

/// Whether keys were registered with [`set_defaults`].
pub fn has_default_keys() -> bool {
    defaults().read().is_ok_and(|d| d.0.is_some())
}

/// An opened RPF8 archive.
#[derive(Debug)]
pub struct Rpf8 {
    path: PathBuf,
    /// Where the archive starts in the file (non-zero for nested archives).
    base: u64,
    /// The archive's name for messages: the file, plus the entries it is nested in.
    display: String,
    header: Header,
    size: u64,
    /// `None` while the entry table is encrypted and no keys are available.
    entries: Option<Vec<Entry>>,
    keys: Option<Arc<Rdr2Keys>>,
    /// Names found in the game's files, handed on to nested archives.
    names: Option<Arc<crate::rpf8_names::NameIndex>>,
}

impl Rpf8 {
    /// Whether these bytes start an RPF8 archive.
    pub fn is_rpf8(start: &[u8]) -> bool {
        Header::parse(start).is_some()
    }

    /// Opens an archive. Encrypted entry tables are left unread.
    pub fn open(path: &Path) -> Result<Rpf8, RpfError> {
        Self::open_with_keys(path, None)
    }

    /// Opens an archive with the keys and names registered by [`set_defaults`].
    pub fn open_default(path: &Path) -> Result<Rpf8, RpfError> {
        let (keys, names) = defaults().read().map(|d| d.clone()).unwrap_or_default();
        let mut rpf = Self::open_with_keys(path, keys)?;
        if let Some(n) = names {
            rpf.set_names(n);
        }
        Ok(rpf)
    }

    /// Reads a resource entry as a [`rage_resource::Resource`]: virtual
    /// pages, then physical pages, sized by the entry's flags.
    pub fn read_resource(&self, entry: &Entry) -> Result<rage_resource::Resource, RpfError> {
        let name = format!("{}/{}", self.display, entry.display_name(self.header.platform));
        let Some(version) = entry.resource_version() else {
            return Err(RpfError::NotFound(format!("{name} is not a resource")));
        };
        let mut pages = self.read(entry)?;
        let virtual_len = (entry.size_or_flags & 0xFFFF_FFF0) as usize;
        if pages.len() < virtual_len {
            return Err(RpfError::BadResource { path: name, reason: "shorter than its virtual pages".into() });
        }
        let graphics = pages.split_off(virtual_len);
        Ok(rage_resource::Resource { version, system: pages, graphics })
    }

    /// Whether a file starts with the RPF8 magic.
    pub fn file_is_rpf8(path: &Path) -> bool {
        let mut head = [0u8; HEADER_LEN as usize];
        File::open(path).and_then(|mut f| f.read_exact(&mut head)).is_ok() && Header::parse(&head).is_some()
    }

    /// The display name of entry `i`: its path when known, else `hash/XXXXXXXX.ext`.
    pub fn entry_name(&self, i: usize) -> String {
        let Some(e) = self.entries.as_ref().and_then(|e| e.get(i)) else { return String::new() };
        // The audio archives' files have no extension of their own (their
        // hash covers the whole path): they are audio containers.
        if e.path.is_none() && e.ext_id == 0xFF && !e.is_resource && self.is_audio_archive() {
            return format!("hash/{:08X}.awc", e.hash);
        }
        e.display_name(self.header.platform)
    }

    /// Whether this is one of the game's sound archives (`x64/audio/…`).
    fn is_audio_archive(&self) -> bool {
        let path = self.path.to_string_lossy().replace('\\', "/").to_ascii_lowercase();
        path.contains("/x64/audio/")
    }

    /// The entry at a path (case-insensitive).
    pub fn find(&self, path: &str) -> Option<usize> {
        let entries = self.entries.as_ref()?;
        (0..entries.len()).find(|&i| self.entry_name(i).eq_ignore_ascii_case(path))
    }

    /// Whether `dir` is a folder: some entry's path starts with it. RPF8 has no
    /// directories of its own; they come from the entries' paths.
    pub fn is_dir(&self, dir: &str) -> bool {
        let prefix = format!("{}/", dir.to_ascii_lowercase());
        dir.is_empty()
            || (0..self.entries.as_ref().map_or(0, Vec::len))
                .any(|i| self.entry_name(i).to_ascii_lowercase().starts_with(&prefix))
    }

    /// The folders and files directly in `dir` ("" for the top): folder names,
    /// and entry indices of the files.
    pub fn list_dir(&self, dir: &str) -> (Vec<String>, Vec<usize>) {
        let prefix = if dir.is_empty() { String::new() } else { format!("{}/", dir.to_ascii_lowercase()) };
        let mut dirs: Vec<String> = Vec::new();
        let mut files = Vec::new();
        for i in 0..self.entries.as_ref().map_or(0, Vec::len) {
            let name = self.entry_name(i);
            let Some(rest) = name.to_ascii_lowercase().strip_prefix(&prefix).map(str::to_owned) else { continue };
            match rest.split_once('/') {
                Some((first, _)) => {
                    let first = name[prefix.len()..prefix.len() + first.len()].to_owned();
                    if !dirs.iter().any(|d| d.eq_ignore_ascii_case(&first)) {
                        dirs.push(first);
                    }
                }
                None => files.push(i),
            }
        }
        dirs.sort();
        files.sort_by_key(|&i| self.entry_name(i).to_ascii_lowercase());
        (dirs, files)
    }

    /// Opens an archive, decrypting its entry table when the keys have the one it needs.
    pub fn open_with_keys(path: &Path, keys: Option<Arc<Rdr2Keys>>) -> Result<Rpf8, RpfError> {
        let size = std::fs::metadata(path)?.len();
        Self::open_range(path, 0, size, path.display().to_string(), keys)
    }

    /// Opens an archive nested in this one (an entry with the `.rpf` extension,
    /// stored as is: neither compressed nor encrypted as a whole).
    pub fn open_child(&self, entry: &Entry) -> Result<Rpf8, RpfError> {
        let display = format!("{}/{}", self.display, entry.display_name(self.header.platform));
        if entry.ext_id != 0 || entry.compression != Compression::None || entry.is_encrypted() {
            return Err(RpfError::NotAnArchive(display));
        }
        let mut child =
            Self::open_range(&self.path, self.base + entry.offset, entry.size_on_disk, display, self.keys.clone())?;
        if let Some(names) = &self.names {
            child.set_names(Arc::clone(names));
        }
        Ok(child)
    }

    /// Names entries that the archive's own name table left unnamed; nested
    /// archives opened afterwards get the names too.
    pub fn set_names(&mut self, names: Arc<crate::rpf8_names::NameIndex>) {
        let platform = self.header.platform;
        if let Some(entries) = &mut self.entries {
            for e in entries.iter_mut().filter(|e| e.path.is_none()) {
                e.path = names.name_for(e, platform);
            }
        }
        self.names = Some(names);
    }

    fn open_range(
        path: &Path,
        base: u64,
        size: u64,
        name: String,
        keys: Option<Arc<Rdr2Keys>>,
    ) -> Result<Rpf8, RpfError> {
        let mut f = File::open(path)?;
        f.seek(SeekFrom::Start(base))?;
        let mut head = [0u8; HEADER_LEN as usize];
        f.read_exact(&mut head)?;
        let header = Header::parse(&head).ok_or_else(|| {
            RpfError::BadMagic(name.clone(), u32::from_le_bytes(head[0..4].try_into().unwrap_or_default()))
        })?;
        let table_end = HEADER_LEN + SIGNATURE_LEN + u64::from(header.entry_count) * ENTRY_LEN as u64;
        if table_end > size || u64::from(header.names_length) > size {
            return Err(RpfError::CorruptToc { archive: name, reason: "entry table larger than the file".into() });
        }
        let table_key = match &keys {
            Some(k) if header.is_encrypted() => {
                k.keys.get(&(header.decryption_tag as u8)).filter(|_| header.decryption_tag < 0x100)
            }
            _ => None,
        };
        let entries = if header.is_encrypted() && table_key.is_none() {
            None
        } else {
            f.seek(SeekFrom::Start(base + HEADER_LEN + SIGNATURE_LEN))?;
            let mut table = vec![0u8; header.entry_count as usize * ENTRY_LEN];
            f.read_exact(&mut table)?;
            if let (Some(key), Some(k)) = (table_key, &keys) {
                rage_tfit2::Cbc::new(&k.context, key, k.iv).decrypt(&mut table);
                if !plausible_table(&table, size) {
                    return Err(RpfError::CorruptToc {
                        archive: name,
                        reason: "the table does not decrypt with the loaded keys".into(),
                    });
                }
            }
            let mut entries: Vec<Entry> = table.as_chunks::<ENTRY_LEN>().0.iter().map(|c| Entry::parse(c)).collect();
            for e in &entries {
                if e.offset + e.size_on_disk > size {
                    return Err(RpfError::OutOfBounds { archive: name, entry: e.display_name(header.platform) });
                }
            }
            if header.names_length > 0 {
                f.seek(SeekFrom::Start(base + size - u64::from(header.names_length)))?;
                let mut names = vec![0u8; header.names_length as usize];
                f.read_exact(&mut names)?;
                apply_names(&mut entries, &names, header.platform);
            }
            Some(entries)
        };
        Ok(Rpf8 { path: path.to_path_buf(), base, display: name, header, size, entries, keys, names: None })
    }

    pub fn header(&self) -> Header {
        self.header
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The archive's name: the file, plus the entries it is nested in.
    pub fn display_name(&self) -> &str {
        &self.display
    }

    /// The entries, or `None` while the table is encrypted.
    pub fn entries(&self) -> Option<&[Entry]> {
        self.entries.as_deref()
    }

    /// Reads an entry's data: decrypted and decompressed (deflate or Oodle).
    /// Resources come without their 16-byte on-disk prefix: virtual pages,
    /// then physical pages.
    pub fn read(&self, entry: &Entry) -> Result<Vec<u8>, RpfError> {
        let archive = self.display.clone();
        let name = entry.display_name(self.header.platform);
        let key = if entry.is_encrypted() {
            let k = self.keys.as_ref().ok_or_else(|| RpfError::KeysRequired(format!("{archive}/{name}")))?;
            Some((
                k,
                k.keys
                    .get(&entry.key_id)
                    .ok_or_else(|| RpfError::KeysRequired(format!("{archive}/{name} (key {})", entry.key_id)))?,
            ))
        } else {
            None
        };
        let mut f = File::open(&self.path)?;
        f.seek(SeekFrom::Start(self.base + entry.offset))?;
        let mut data = vec![0u8; entry.size_on_disk as usize];
        f.read_exact(&mut data)?;
        // Unverified on real data: Swage places the signature at the end.
        if entry.signed && data.len() >= SIGNATURE_LEN as usize {
            data.truncate(data.len() - SIGNATURE_LEN as usize);
        }
        if entry.is_resource && data.len() >= 16 {
            data.drain(..16);
        }
        if let Some((k, key)) = key {
            let compressed = entry.compression != Compression::None;
            let chunk = rage_tfit2::chunk_size(entry.is_resource, compressed, entry.size());
            let prefix = if entry.is_resource { 16 } else { 0 };
            rage_tfit2::decrypt_file(&k.context, key, k.iv, entry.encryption_config, chunk, prefix, &mut data);
        }
        match entry.compression {
            Compression::None => {
                if !entry.is_resource {
                    data.truncate(entry.size_or_flags as usize);
                }
                Ok(data)
            }
            Compression::Deflate => {
                let mut out = Vec::new();
                flate2::read::DeflateDecoder::new(&data[..])
                    .read_to_end(&mut out)
                    .map_err(|source| RpfError::Inflate { archive, entry: name, source })?;
                Ok(out)
            }
            Compression::Oodle => {
                let expected = entry.size() as usize;
                let mut out = vec![0u8; expected];
                // oozextract panics on some damaged input instead of failing.
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    oozextract::Extractor::new().read_from_slice(&data, &mut out).map_err(|e| e.to_string())
                }))
                .unwrap_or_else(|_| Err("damaged Oodle data".to_owned()));
                let n = result.map_err(|e| RpfError::Inflate {
                    archive: archive.clone(),
                    entry: name.clone(),
                    source: std::io::Error::other(e),
                })?;
                if n != expected {
                    return Err(RpfError::Inflate {
                        archive,
                        entry: name,
                        source: std::io::Error::other(format!("Oodle gave {n} bytes, expected {expected}")),
                    });
                }
                Ok(out)
            }
            Compression::Unknown(n) => Err(RpfError::Inflate {
                archive,
                entry: name,
                source: std::io::Error::other(format!("unknown compression {n}")),
            }),
        }
    }
}

/// Matches the NUL-separated paths of a name table to the entries.
fn apply_names(entries: &mut [Entry], names: &[u8], platform: char) {
    for name in names.split(|&b| b == 0).filter(|n| !n.is_empty()) {
        let Ok(name) = std::str::from_utf8(name) else { continue };
        let (hash, ext) = path_hash(name, platform);
        if let Some(e) = entries.iter_mut().find(|e| e.hash == hash && (e.ext_id == ext || ext == 0xFF)) {
            e.path = Some(name.replace('\\', "/"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry_bytes(hash: u32, ext: u8, key: u8, offset: u64, disk: u64, comp: u64, size: u64) -> Vec<u8> {
        let q0 = u64::from(hash) | u64::from(key) << 40 | u64::from(ext) << 48;
        let q1 = disk >> 4 | (offset >> 4) << 28 | comp << 59;
        [q0.to_le_bytes(), q1.to_le_bytes(), size.to_le_bytes()].concat()
    }

    fn archive(entries: &[Vec<u8>], data: &[u8], names: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend(MAGIC.to_le_bytes());
        out.extend((entries.len() as u32).to_le_bytes());
        out.extend((names.len() as u32).to_le_bytes());
        out.extend(PLAINTEXT.to_le_bytes());
        out.extend([b'y', 0]);
        out.extend([0u8; 256]);
        for e in entries {
            out.extend(e);
        }
        out.resize(0x200, 0);
        out.extend(data);
        out.extend(names);
        out
    }

    #[test]
    fn reads_a_plaintext_archive_with_names() {
        let text = b"hello rdr";
        let mut deflated = Vec::new();
        {
            use std::io::Write;
            let mut enc = flate2::write::DeflateEncoder::new(&mut deflated, flate2::Compression::default());
            enc.write_all(text).unwrap();
        }
        let mut data = vec![0u8; 0x10];
        data[..text.len()].copy_from_slice(text);
        let second = 0x210;
        data.extend(&deflated);
        data.resize(0x10 + deflated.len().next_multiple_of(16), 0);
        let (h1, x1) = path_hash("common/data/a.txt", 'y');
        let (h2, x2) = path_hash("common/data/b.ymt", 'y');
        assert_eq!(x1, 0xFF);
        assert_eq!(x2, 11);
        let bytes = archive(
            &[
                entry_bytes(h1, x1, 0xFF, 0x200, 0x10, 0, text.len() as u64),
                entry_bytes(h2, x2, 0xFF, second, (deflated.len() as u64).next_multiple_of(16), 1, text.len() as u64),
            ],
            &data,
            b"common/data/a.txt\0common/data/b.ymt\0",
        );
        let dir = std::env::temp_dir().join(format!("openrage-rpf8-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.rpf");
        std::fs::write(&path, bytes).unwrap();
        let rpf = Rpf8::open(&path).unwrap();
        let entries = rpf.entries().unwrap();
        assert_eq!(entries[0].path.as_deref(), Some("common/data/a.txt"));
        assert_eq!(entries[1].path.as_deref(), Some("common/data/b.ymt"));
        assert_eq!(rpf.read(&entries[0]).unwrap(), text);
        assert_eq!(rpf.read(&entries[1]).unwrap(), text);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn recognises_plausible_tables() {
        let good = [entry_bytes(1, 5, 3, 0x200, 0x40, 2, 7), entry_bytes(9, 0xFF, 0xFF, 0x240, 0x10, 0, 3)].concat();
        assert!(plausible_table(&good, 0x1000));
        let unsorted = [entry_bytes(9, 5, 3, 0x200, 0x40, 2, 7), entry_bytes(1, 5, 3, 0x240, 0x10, 0, 3)].concat();
        assert!(!plausible_table(&unsorted, 0x1000));
        assert!(!plausible_table(&good, 0x220), "data past the end");
        let bad_ext = entry_bytes(1, 40, 3, 0x200, 0x40, 2, 7);
        assert!(!plausible_table(&bad_ext, 0x1000));
    }

    #[test]
    fn encrypted_tables_stay_closed() {
        let mut bytes = archive(&[entry_bytes(1, 5, 3, 0x200, 0, 0, 0)], &[], b"");
        bytes[12] = 0x44;
        bytes[13] = 0;
        let h = Header::parse(&bytes).unwrap();
        assert!(h.is_encrypted());
        assert_eq!(h.platform, 'y');
        assert_eq!(extension(5, 'y').as_deref(), Some("ytd"));
        let e = Entry::parse(&entry_bytes(0xABCD, 5, 3, 0x1230, 0x40, 2, 7));
        assert_eq!((e.offset, e.size_on_disk, e.compression, e.key_id), (0x1230, 0x40, Compression::Oodle, 3));
        assert_eq!(e.display_name('y'), "hash/0000ABCD.ytd");
    }
}
