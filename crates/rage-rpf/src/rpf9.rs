//! RPF9 archives (GTA VI).
//!
//! With the game's executable (`game_scarlett_master.exe`, the supported
//! build) in the archive's folder or one above, the archive's own table of
//! contents is read: all its files, their folders and — as far as they can be
//! matched to the hashes in the table — names, and the archives inside it
//! ([`Rpf9::open_child`]). Nothing from the executable is stored.
//!
//! Without it, an archive is read without its table of contents, as below.
//!
//! Layout (little-endian):
//! - 64-byte header: magic `RPF9`, then the offset where the file data
//!   starts, a declared entry count at `0x24` and transform tag at `0x2e`.
//! - The table of contents (names, sizes, offsets) up to the data offset. It
//!   is encrypted with keys that only the game's executable has.
//! - The file data. Most of it is not encrypted: zlib streams laid end to
//!   end, each starting on a 16-byte boundary, Bink 2 videos and audio
//!   containers (`ADAT`).
//!
//! So the archive is read as a *recovery view*: [`Rpf9::scan`] walks the data
//! area and lists every zlib stream, Bink 2 video and audio container it finds. Files have no
//! names, only their offset (`xml/00000000b000.xml`), and are sorted into
//! folders by what their first bytes look like. A file stored in several
//! 64 KiB chunks is put together again; a resource whose pages are separate
//! streams shows up as several entries. Other data (encrypted files) is not
//! listed.
//!
//! Scanning reads the whole archive, so the result is worth keeping:
//! [`encode_index`] and [`decode_index`] store it, and a program can
//! register a loader with [`set_indexer`] that [`Rpf9::entries`] then uses.

use std::collections::{BTreeSet, HashMap};
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use rayon::prelude::*;

use crate::RpfError;
use crate::rpf9_index::{NativeEntry, Storage};
use crate::rpf9_native::{self, NativeArchive};

const MAGIC: u32 = 0x5250_4639;
/// Length of the header in front of the table of contents.
pub const HEADER_LEN: usize = 64;
/// Files larger than this are stored as several zlib streams of this many
/// unpacked bytes, the last one shorter.
const CHUNK: u64 = 0x10000;
/// Streams start on a multiple of this.
const ALIGN: u64 = 16;
/// The data area is scanned in pieces of this size, in parallel.
const SEGMENT: u64 = 256 << 20;
/// How much of the file a scanner keeps in memory.
const WINDOW: usize = 4 << 20;
/// A window with less than this left is read anew, so most streams fit.
const WINDOW_MIN: usize = 64 << 10;
/// Archives of at least this size get a folder per GiB of offset.
const BUCKETED_FROM: u64 = 1 << 30;

/// Four-byte magics that tagged formats of the game start with. Only these
/// become an extension: among a million files, some start with four letters
/// by chance.
const MAGICS: [&[u8; 4]; 7] = [b"PSIN", b"PAMH", b"MHIU", b"RBF0", b"SPS1", b"RSC8", b"0DRP"];

/// The readable part of the header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// Where the file data starts; the table of contents ends here.
    pub data_offset: u32,
    /// Native index slots declared by the clear header. Recovery streams do
    /// not correspond one-to-one to these slots.
    pub entry_count: u32,
    /// Native header transform selector; this is not an archive key ID.
    pub toc_transform: u8,
}

impl Header {
    pub fn parse(b: &[u8]) -> Option<Header> {
        if b.len() < HEADER_LEN || crate::word(b, 0) != MAGIC {
            return None;
        }
        Some(Header { data_offset: crate::word(b, 4), entry_count: crate::word(b, 0x24), toc_transform: b[0x2e] })
    }
}

/// How an entry is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// One or more zlib streams.
    Zlib,
    /// A Bink 2 video, stored as is.
    Bink2,
    /// An audio container (`ADAT`), stored as is.
    Awc,
    /// A file of the archive's own table, at this slot.
    Native { slot: u32 },
}

/// One recovered file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Where it starts in the archive file.
    pub offset: u64,
    /// Bytes it takes in the archive, padding between its streams included.
    pub stored: u64,
    /// Unpacked size.
    pub size: u64,
    /// Number of zlib streams (1 for a video).
    pub streams: u32,
    pub kind: Kind,
    /// The first unpacked bytes (`head_len` of them).
    pub head: [u8; 16],
    pub head_len: u8,
}

impl Entry {
    fn native(e: &NativeEntry) -> Entry {
        let kind = Kind::Native { slot: e.slot as u32 };
        Entry { offset: e.offset, stored: e.stored, size: e.size, streams: 1, kind, head: [0; 16], head_len: 0 }
    }

    /// The first unpacked bytes.
    pub fn head(&self) -> &[u8] {
        &self.head[..usize::from(self.head_len).min(16)]
    }

    /// Whether this looks like the start of a resource (a model, for
    /// instance): they start 16 bytes into a 4 KiB page, with eight zero
    /// bytes and a pointer into their own first part (`0x5000_0000` on).
    pub fn is_resource(&self) -> bool {
        let head = self.head();
        self.kind == Kind::Zlib
            && self.offset % 4096 == 16
            && head.len() == 16
            && head[..8] == [0; 8]
            && (0x5000_0000..0x5200_0000).contains(&u64::from_le_bytes(head[8..].try_into().unwrap_or_default()))
    }

    /// An extension guessed from the first bytes: `bk2`, `awc`, `res`, `xml`, `txt`, the
    /// magic of a known tagged format in lower case (`psin`), else `bin`.
    pub fn extension(&self) -> String {
        match self.kind {
            Kind::Bink2 => return "bk2".into(),
            Kind::Awc => return "awc".into(),
            Kind::Zlib | Kind::Native { .. } => {}
        }
        if self.is_resource() {
            return "res".into();
        }
        let head = self.head();
        let text = head.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(head);
        // Text only when all of the first bytes are: binary files start with
        // `<` and a letter often enough (an offset table, for instance).
        let printable = |b: &u8| b.is_ascii_graphic() || matches!(b, b' ' | b'\t' | b'\r' | b'\n');
        if !text.is_empty() && text.iter().all(printable) {
            let markup = text.starts_with(b"<?xml")
                || (text.first() == Some(&b'<') && text.get(1).is_some_and(u8::is_ascii_alphabetic));
            return if markup { "xml" } else { "txt" }.into();
        }
        if let Some(magic) = head.get(..4)
            && MAGICS.iter().any(|m| m == &magic)
        {
            return String::from_utf8_lossy(magic).to_ascii_lowercase();
        }
        "bin".into()
    }
}

/// A stream as the scanner finds it, before chunks are joined.
#[derive(Debug, Clone)]
struct Stream {
    offset: u64,
    stored: u64,
    size: u64,
    kind: Kind,
    head: [u8; 16],
    head_len: u8,
}

/// Loads the entries of an archive, for example from a cache.
pub type Indexer = dyn Fn(&Rpf9) -> Result<Vec<Entry>, RpfError> + Send + Sync;

fn indexer() -> &'static RwLock<Option<Arc<Indexer>>> {
    static INDEXER: OnceLock<RwLock<Option<Arc<Indexer>>>> = OnceLock::new();
    INDEXER.get_or_init(Default::default)
}

/// Registers what [`Rpf9::entries`] calls to get an archive's entries.
/// Without one it scans the archive every time it is opened.
pub fn set_indexer(load: Arc<Indexer>) {
    if let Ok(mut slot) = indexer().write() {
        *slot = Some(load);
    }
}

/// The folder of the entries whose own names are known.
const NAMED: &str = "models";

/// Names of entries, found in their own data: a model carries its name. The
/// table of contents, which has all names, cannot be read.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Names {
    by_offset: HashMap<u64, String>,
    /// Lower-case name to offset.
    by_name: HashMap<String, u64>,
}

impl Names {
    /// From entries' offsets and names. Characters that do not belong in a
    /// file name are replaced; of entries with the same name, the later ones
    /// (by offset) get `~2`, `~3`, … in front of the extension.
    pub fn new(found: impl IntoIterator<Item = (u64, String)>) -> Names {
        let mut found: Vec<(u64, String)> = found.into_iter().collect();
        found.sort();
        let mut names = Names::default();
        for (offset, name) in found {
            let clean: String =
                name.chars().map(|c| if c.is_ascii_alphanumeric() || "_.#-".contains(c) { c } else { '_' }).collect();
            let clean = clean.trim_matches('.');
            if clean.is_empty() {
                continue;
            }
            let (stem, ext) = clean.split_once('.').map_or((clean, String::new()), |(s, e)| (s, format!(".{e}")));
            let unique = (1..)
                .map(|n| if n == 1 { clean.to_owned() } else { format!("{stem}~{n}{ext}") })
                .find(|candidate| !names.by_name.contains_key(&candidate.to_ascii_lowercase()));
            let Some(unique) = unique else { continue };
            names.by_name.insert(unique.to_ascii_lowercase(), offset);
            names.by_offset.insert(offset, unique);
        }
        names
    }

    pub fn get(&self, offset: u64) -> Option<&str> {
        self.by_offset.get(&offset).map(String::as_str)
    }

    /// The offset of the entry with this name (case-insensitive).
    pub fn offset_of(&self, name: &str) -> Option<u64> {
        self.by_name.get(&name.to_ascii_lowercase()).copied()
    }

    pub fn len(&self) -> usize {
        self.by_offset.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_offset.is_empty()
    }

    /// Offsets and names, sorted by offset.
    pub fn iter(&self) -> impl Iterator<Item = (u64, &str)> {
        let mut all: Vec<(u64, &str)> = self.by_offset.iter().map(|(o, n)| (*o, n.as_str())).collect();
        all.sort();
        all.into_iter()
    }

    /// The folder a name is listed in: what it starts with, up to the first
    /// `_` (`w` for weapons, `prop`, …); `other` without one.
    fn group(name: &str) -> String {
        match name.split_once('_') {
            Some((start, _)) if (1..=12).contains(&start.len()) => start.to_ascii_lowercase(),
            _ => "other".to_owned(),
        }
    }
}

/// Loads the names of an archive's entries, for example from a cache.
pub type Namer = dyn Fn(&Rpf9) -> Option<Names> + Send + Sync;

fn namer() -> &'static RwLock<Option<Arc<Namer>>> {
    static NAMER: OnceLock<RwLock<Option<Arc<Namer>>>> = OnceLock::new();
    NAMER.get_or_init(Default::default)
}

/// Registers what [`Rpf9::names`] calls to get the names of an archive's entries.
pub fn set_namer(load: Arc<Namer>) {
    if let Ok(mut slot) = namer().write() {
        *slot = Some(load);
    }
}

/// An opened RPF9 archive.
#[derive(Debug)]
pub struct Rpf9 {
    path: PathBuf,
    display: String,
    header: Header,
    size: u64,
    /// Where the archive starts in the file: a nested one, inside its parent.
    base: u64,
    /// The archive's own table, read on first use.
    native: OnceLock<Option<NativeArchive>>,
    /// Filled on first use: finding the entries means reading the archive.
    entries: Mutex<Option<Arc<Vec<Entry>>>>,
    /// Filled on first use.
    names: Mutex<Option<Arc<Names>>>,
}

impl Rpf9 {
    /// Whether these bytes start an RPF9 archive.
    pub fn is_rpf9(start: &[u8]) -> bool {
        start.len() >= 4 && crate::word(start, 0) == MAGIC
    }

    /// Whether a file starts with the RPF9 magic.
    pub fn file_is_rpf9(path: &Path) -> bool {
        let mut head = [0u8; 4];
        File::open(path).and_then(|mut f| f.read_exact(&mut head)).is_ok() && Self::is_rpf9(&head)
    }

    /// Opens an archive: reads its header, not its entries.
    pub fn open(path: &Path) -> Result<Rpf9, RpfError> {
        let display = path.display().to_string();
        let mut file = File::open(path)?;
        let size = file.metadata()?.len();
        let mut head = [0u8; HEADER_LEN];
        if file.read_exact(&mut head).is_err() {
            return Err(RpfError::Empty(display, size));
        }
        let Some(header) = Header::parse(&head) else {
            return Err(RpfError::BadMagic(display, crate::word(&head, 0)));
        };
        if u64::from(header.data_offset) < HEADER_LEN as u64 || u64::from(header.data_offset) > size {
            return Err(RpfError::CorruptToc { archive: display, reason: "the data starts outside the file".into() });
        }
        Ok(Rpf9 {
            path: path.to_path_buf(),
            display,
            header,
            size,
            base: 0,
            native: OnceLock::new(),
            entries: Mutex::new(None),
            names: Mutex::new(None),
        })
    }

    /// The archive's own table; `None` without the game's executable (or
    /// with another build's) and for damaged tables.
    fn native(&self) -> Option<&NativeArchive> {
        self.native
            .get_or_init(|| {
                let materials = rpf9_native::materials(&self.path)?;
                NativeArchive::open(&self.path, self.base, self.size, materials).ok()
            })
            .as_ref()
    }

    /// Whether the archive's own table is read, rather than a recovery view
    /// made by scanning. Decides it on first use, without a scan.
    pub fn is_native(&self) -> bool {
        self.native().is_some()
    }

    /// Whether the archive lies inside another one.
    pub fn is_nested(&self) -> bool {
        self.base != 0
    }

    /// The record of an entry in the archive's own table.
    fn native_entry(&self, entry: &Entry) -> Option<(&NativeArchive, &NativeEntry)> {
        let Kind::Native { slot } = entry.kind else { return None };
        let native = self.native()?;
        Some((native, native.catalog.entries.get(slot as usize)?))
    }

    /// Whether an entry is an archive that [`Rpf9::open_child`] opens.
    pub fn is_archive(&self, entry: &Entry) -> bool {
        self.native_entry(entry).is_some_and(|(_, e)| e.type_id == 0)
    }

    /// The sizes of a resource's system and graphics parts, which follow
    /// each other in its data. `None` for other files.
    pub fn resource_parts(&self, entry: &Entry) -> Option<(u64, u64)> {
        match self.native_entry(entry)?.1.storage {
            Storage::Resource { system_flags, graphics_flags } => {
                Some((u64::from(system_flags & 0x7fff_fff0), u64::from(graphics_flags & 0xffff_fff0)))
            }
            _ => None,
        }
    }

    /// Opens an archive inside this one.
    pub fn open_child(&self, entry: &Entry) -> Result<Rpf9, RpfError> {
        let display = format!("{}/{}", self.display, self.entry_name(entry));
        // Archives are stored as they are, so they can be read in place.
        if !self.is_archive(entry) || entry.stored != entry.size {
            return Err(RpfError::NotAnArchive(display));
        }
        let base = self.base + entry.offset;
        let mut file = File::open(&self.path)?;
        file.seek(SeekFrom::Start(base))?;
        let mut head = [0u8; HEADER_LEN];
        file.read_exact(&mut head)?;
        let Some(header) = Header::parse(&head) else {
            return Err(RpfError::BadMagic(display, crate::word(&head, 0)));
        };
        let child = Rpf9 {
            path: self.path.clone(),
            display,
            header,
            size: entry.size,
            base,
            native: OnceLock::new(),
            entries: Mutex::new(None),
            names: Mutex::new(None),
        };
        if child.native().is_none() {
            return Err(RpfError::CorruptToc { archive: child.display, reason: "its table cannot be read".into() });
        }
        Ok(child)
    }

    pub fn header(&self) -> Header {
        self.header
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The archive's name for messages.
    pub fn display_name(&self) -> &str {
        &self.display
    }

    /// Size of the archive file.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// Size of the data area: everything after the table of contents.
    pub fn data_len(&self) -> u64 {
        self.size - u64::from(self.header.data_offset)
    }

    /// The entries if they have been loaded already.
    pub fn loaded(&self) -> Option<Arc<Vec<Entry>>> {
        self.entries.lock().ok()?.clone()
    }

    /// Sets the entries, for example from a stored index.
    pub fn set_entries(&self, entries: Vec<Entry>) {
        if let Ok(mut slot) = self.entries.lock() {
            *slot = Some(Arc::new(entries));
        }
    }

    /// The entries: in the order of the archive's own table if that is read,
    /// else sorted by offset. Then the first call loads them with the
    /// registered [`set_indexer`] loader, or scans the archive, which reads
    /// all of it. A failure is not remembered: the next call tries again.
    pub fn entries(&self) -> Result<Arc<Vec<Entry>>, RpfError> {
        let mut slot = self.entries.lock().map_err(|_| RpfError::Cancelled)?;
        if let Some(entries) = slot.as_ref() {
            return Ok(Arc::clone(entries));
        }
        let load = indexer().read().ok().and_then(|l| l.clone());
        let entries = Arc::new(match (self.native(), load) {
            (Some(native), _) => native.catalog.entries.iter().map(Entry::native).collect(),
            (None, Some(load)) => load(self)?,
            (None, None) => self.scan(&AtomicBool::new(false), &|_, _| {})?,
        });
        *slot = Some(Arc::clone(&entries));
        Ok(entries)
    }

    /// Walks the data area and returns what it finds, sorted by offset.
    /// `progress` gets the bytes read so far and the bytes to read; setting
    /// `cancel` stops with [`RpfError::Cancelled`].
    pub fn scan(&self, cancel: &AtomicBool, progress: &(dyn Fn(u64, u64) + Sync)) -> Result<Vec<Entry>, RpfError> {
        let start = u64::from(self.header.data_offset);
        let total = self.size - start;
        let segments: Vec<(u64, u64)> = (0..total.div_ceil(SEGMENT))
            .map(|i| (start + i * SEGMENT, (start + (i + 1) * SEGMENT).min(self.size)))
            .collect();
        let done = AtomicU64::new(0);
        let found: Vec<Vec<Stream>> = segments
            .par_iter()
            .map(|&(from, to)| {
                let mut scanner = Scanner::new(File::open(&self.path)?, self.size);
                scanner.streams(from, to, cancel, &mut |bytes| {
                    progress(done.fetch_add(bytes, Ordering::Relaxed) + bytes, total);
                })
            })
            .collect::<Result<_, RpfError>>()?;
        Ok(join_chunks(found.into_iter().flatten()))
    }

    /// The names found for entries of this archive. The first call asks the
    /// registered [`set_namer`] loader; without one, or without stored
    /// names, there are none.
    pub fn names(&self) -> Arc<Names> {
        let Ok(mut slot) = self.names.lock() else { return Arc::default() };
        if let Some(names) = slot.as_ref() {
            return Arc::clone(names);
        }
        // The names of models in the recovery view; the table has its own.
        let load = if self.is_native() { None } else { namer().read().ok().and_then(|l| l.clone()) };
        let names = Arc::new(load.and_then(|load| load(self)).unwrap_or_default());
        *slot = Some(Arc::clone(&names));
        names
    }

    /// Sets the names of entries.
    pub fn set_names(&self, names: Names) {
        if let Ok(mut slot) = self.names.lock() {
            *slot = Some(Arc::new(names));
        }
    }

    /// The name of an entry: `<extension>/<offset>.<extension>`, in large
    /// archives `<extension>/<GiB of the offset>/<offset>.<extension>`. An
    /// entry whose own name is known (see [`Names`]) is
    /// `models/<start of the name>/<name>`.
    pub fn entry_name(&self, entry: &Entry) -> String {
        self.entry_name_with(&self.names(), entry)
    }

    fn entry_name_with(&self, names: &Names, entry: &Entry) -> String {
        if let Some((native, e)) = self.native_entry(entry) {
            return native.paths[e.slot].clone();
        }
        if let Some(name) = names.get(entry.offset) {
            return format!("{NAMED}/{}/{name}", Names::group(name));
        }
        let ext = entry.extension();
        match self.bucket(entry) {
            Some(bucket) => format!("{ext}/{bucket}/{:012x}.{ext}", entry.offset),
            None => format!("{ext}/{:012x}.{ext}", entry.offset),
        }
    }

    fn bucket(&self, entry: &Entry) -> Option<String> {
        (self.size >= BUCKETED_FROM).then(|| format!("{:03}", entry.offset >> 30))
    }

    /// The entry with this name (case-insensitive).
    pub fn find(&self, entries: &[Entry], path: &str) -> Option<usize> {
        if let Some(native) = self.native() {
            // Entries are in the order of the slots.
            return native.find(path).filter(|slot| *slot < entries.len());
        }
        let names = self.names();
        let file = path.rsplit('/').next()?;
        let by_offset = file.split('.').next().and_then(|stem| u64::from_str_radix(stem, 16).ok());
        [by_offset, names.offset_of(file)].into_iter().flatten().find_map(|offset| {
            let i = entries.binary_search_by_key(&offset, |e| e.offset).ok()?;
            self.entry_name_with(&names, &entries[i]).eq_ignore_ascii_case(path).then_some(i)
        })
    }

    /// The folders and files directly in `dir` ("" for the top): folder
    /// names, and indices of the entries.
    pub fn list_dir(&self, entries: &[Entry], dir: &str) -> (Vec<String>, Vec<usize>) {
        let names = self.names();
        let dir = dir.trim_matches('/').to_ascii_lowercase();
        let mut dirs = BTreeSet::new();
        let mut files = Vec::new();
        for (i, e) in entries.iter().enumerate() {
            let name = self.entry_name_with(&names, e);
            // What comes after `dir/` in the entry's path.
            let rest = if dir.is_empty() {
                Some(name.as_str())
            } else {
                name.get(..dir.len())
                    .filter(|start| start.eq_ignore_ascii_case(&dir))
                    .and_then(|_| name[dir.len()..].strip_prefix('/'))
            };
            match rest.map(|rest| rest.split_once('/')) {
                Some(Some((folder, _))) => {
                    if !dirs.contains(folder) {
                        dirs.insert(folder.to_owned());
                    }
                }
                Some(None) => files.push(i),
                None => {}
            }
        }
        (dirs.into_iter().collect(), files)
    }

    /// Whether `dir` is a folder of the recovery view.
    pub fn is_dir(&self, entries: &[Entry], dir: &str) -> bool {
        let (dirs, files) = self.list_dir(entries, dir);
        dir.is_empty() || !dirs.is_empty() || !files.is_empty()
    }

    /// Writes an entry unpacked to `out` and returns the bytes written.
    pub fn copy_to(&self, entry: &Entry, out: &mut dyn Write) -> Result<u64, RpfError> {
        let name = self.entry_name(entry);
        if let Some((native, e)) = self.native_entry(entry) {
            return native.copy_to(&self.path, self.base, e, out).map_err(|reason| RpfError::Inflate {
                archive: self.display.clone(),
                entry: name,
                source: std::io::Error::new(std::io::ErrorKind::InvalidData, reason),
            });
        }
        let outside = || RpfError::OutOfBounds { archive: self.display.clone(), entry: name.clone() };
        if entry.offset < u64::from(self.header.data_offset)
            || entry.stored == 0
            || entry.size == 0
            || entry.streams == 0
            || entry.offset.checked_add(entry.stored).is_none_or(|end| end > self.size)
        {
            return Err(outside());
        }
        let mut file = File::open(&self.path)?;
        file.seek(SeekFrom::Start(entry.offset))?;
        let mut reader = BufReader::with_capacity(1 << 16, file.take(entry.stored));
        if entry.kind != Kind::Zlib {
            if entry.size != entry.stored || entry.streams != 1 {
                return Err(outside());
            }
            let written = std::io::copy(&mut reader, out)?;
            return if written == entry.size { Ok(written) } else { Err(outside()) };
        }
        let inflate = |source| RpfError::Inflate { archive: self.display.clone(), entry: name.clone(), source };
        let (mut read, mut written) = (0u64, 0u64);
        for _ in 0..entry.streams {
            // Streams start aligned; the entry itself does too.
            let padding = read.next_multiple_of(ALIGN) - read;
            if padding > 0 {
                let mut bytes = [0u8; ALIGN as usize];
                reader.read_exact(&mut bytes[..padding as usize])?;
            }
            let mut stream = flate2::bufread::ZlibDecoder::new(&mut reader);
            // A stale or corrupt index must not cause unlimited output before
            // the decoded length is checked. One extra byte detects overflow.
            let limit = entry.size.saturating_sub(written).saturating_add(1);
            written += std::io::copy(&mut (&mut stream).take(limit), out).map_err(inflate)?;
            read += padding + stream.total_in();
            if written > entry.size {
                return Err(inflate(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "unpacked size exceeds the index",
                )));
            }
        }
        if written != entry.size || read != entry.stored {
            let short = std::io::Error::new(std::io::ErrorKind::InvalidData, "unpacked size differs from the index");
            return Err(inflate(short));
        }
        Ok(written)
    }

    /// Whether entry `i + 1` is stored right after entry `i` (on the next
    /// 16-byte boundary): the parts of a resource follow each other so.
    pub fn follows(entries: &[Entry], i: usize) -> bool {
        match (entries.get(i), entries.get(i + 1)) {
            (Some(a), Some(b)) => b.kind == Kind::Zlib && b.offset == (a.offset + a.stored).next_multiple_of(ALIGN),
            _ => false,
        }
    }

    /// The first bytes of an entry, unpacked: up to `max`, or a little more.
    pub fn read_start(&self, entry: &Entry, max: usize) -> Result<Vec<u8>, RpfError> {
        struct Start(Vec<u8>, usize);
        impl Write for Start {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                if self.0.len() >= self.1 {
                    return Err(std::io::ErrorKind::WriteZero.into());
                }
                self.0.extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut start = Start(Vec::new(), max);
        match self.copy_to(entry, &mut start) {
            Err(e) if start.0.len() < max => Err(e),
            _ => Ok(start.0),
        }
    }

    /// Reads the entry with index `i` of [`Rpf9::entries`] unpacked into memory.
    pub fn read_at(&self, i: usize) -> Result<Vec<u8>, RpfError> {
        let entries = self.entries()?;
        let entry = entries.get(i).ok_or_else(|| RpfError::NotFound(format!("{}: entry {i}", self.display)))?;
        self.read(entry)
    }

    /// Reads an entry unpacked into memory (up to [`crate::MAX_UNPACKED`]).
    pub fn read(&self, entry: &Entry) -> Result<Vec<u8>, RpfError> {
        if entry.size > crate::MAX_UNPACKED {
            let source = std::io::Error::new(std::io::ErrorKind::InvalidData, "too large to read into memory");
            return Err(RpfError::Inflate { archive: self.display.clone(), entry: self.entry_name(entry), source });
        }
        let mut data = Vec::with_capacity(entry.size.min(crate::PREALLOCATE) as usize);
        self.copy_to(entry, &mut data)?;
        Ok(data)
    }
}

/// Joins the chunks of files stored in several streams: a stream that
/// unpacks to exactly [`CHUNK`] bytes continues in the stream right after
/// it. Streams that start inside an earlier one are dropped (a scanner that
/// started in the middle of a file can find such).
fn join_chunks(streams: impl Iterator<Item = Stream>) -> Vec<Entry> {
    let mut entries: Vec<Entry> = Vec::new();
    // Unpacked size of the stream before, when the next may continue it.
    let mut last_size = 0;
    for s in streams {
        if let Some(prev) = entries.last_mut() {
            let end = prev.offset + prev.stored;
            if s.offset < end {
                continue;
            }
            let continues = s.kind == Kind::Zlib
                && prev.kind == Kind::Zlib
                && last_size == CHUNK
                && s.offset == end.next_multiple_of(ALIGN);
            if continues {
                prev.stored = s.offset + s.stored - prev.offset;
                prev.size += s.size;
                prev.streams += 1;
                last_size = s.size;
                continue;
            }
        }
        last_size = s.size;
        entries.push(Entry {
            offset: s.offset,
            stored: s.stored,
            size: s.size,
            streams: 1,
            kind: s.kind,
            head: s.head,
            head_len: s.head_len,
        });
    }
    entries
}

/// Reads a file through a window that moves forward with the scan.
struct Scanner {
    file: File,
    size: u64,
    buf: Vec<u8>,
    /// File offset of `buf[0]`.
    base: u64,
    scratch: Vec<u8>,
}

impl Scanner {
    fn new(file: File, size: u64) -> Scanner {
        Scanner { file, size, buf: Vec::new(), base: 0, scratch: vec![0; 1 << 16] }
    }

    /// The bytes from `pos` on: as many as the window holds, none at the end
    /// of the file.
    fn at(&mut self, pos: u64) -> Result<&[u8], RpfError> {
        let end = self.base + self.buf.len() as u64;
        let have = if pos >= self.base && pos < end { (end - pos) as usize } else { 0 };
        if have < WINDOW_MIN && end.max(pos) < self.size || have == 0 {
            let len = (self.size.saturating_sub(pos)).min(WINDOW as u64) as usize;
            self.buf.resize(len, 0);
            self.file.seek(SeekFrom::Start(pos))?;
            self.file.read_exact(&mut self.buf)?;
            self.base = pos;
        }
        Ok(&self.buf[(pos - self.base) as usize..])
    }

    /// The streams that start in `from..to`; the last one may end after `to`.
    fn streams(
        &mut self,
        from: u64,
        to: u64,
        cancel: &AtomicBool,
        advanced: &mut dyn FnMut(u64),
    ) -> Result<Vec<Stream>, RpfError> {
        let mut found = Vec::new();
        let (mut pos, mut reported) = (from, from);
        while pos < to {
            if cancel.load(Ordering::Relaxed) {
                return Err(RpfError::Cancelled);
            }
            if pos - reported >= 16 << 20 {
                advanced(pos - reported);
                reported = pos;
            }
            let buf = self.at(pos)?;
            let len = buf.len().min((to - pos) as usize);
            let Some(i) = memchr::memchr3(0x78, b'K', b'A', &buf[..len]) else {
                pos += len as u64;
                continue;
            };
            let candidate = pos + i as u64;
            pos = match self.stream_at(candidate, cancel)? {
                Some(s) => {
                    let end = s.offset + s.stored;
                    found.push(s);
                    end
                }
                None => candidate + 1,
            };
        }
        advanced(to - reported.min(to));
        Ok(found)
    }

    /// The zlib stream, Bink 2 video or audio container that starts at `pos`,
    /// if one does.
    fn stream_at(&mut self, pos: u64, cancel: &AtomicBool) -> Result<Option<Stream>, RpfError> {
        let size = self.size;
        let start = self.at(pos)?;
        if start.len() >= 28 && &start[..3] == b"KB2" && start[3].is_ascii_lowercase() {
            // Bink 2: length without the first 8 bytes, frame count, …, width, height.
            let total = u64::from(crate::word(start, 4)) + 8;
            let (frames, width, height) = (crate::word(start, 8), crate::word(start, 20), crate::word(start, 24));
            let plausible = (1..=10_000_000).contains(&frames)
                && (1..=16_384).contains(&width)
                && (1..=16_384).contains(&height)
                && total >= 28
                && total <= size - pos;
            let mut head = [0u8; 16];
            head.copy_from_slice(&start[..16]);
            return Ok(plausible.then_some(Stream {
                offset: pos,
                stored: total,
                size: total,
                kind: Kind::Bink2,
                head,
                head_len: 16,
            }));
        }
        if pos.is_multiple_of(ALIGN)
            && let Some(total) = awc_len(start)
        {
            let mut head = [0u8; 16];
            head.copy_from_slice(&start[..16]);
            return Ok((total <= size - pos).then_some(Stream {
                offset: pos,
                stored: total,
                size: total,
                kind: Kind::Awc,
                head,
                head_len: 16,
            }));
        }
        if start.len() < 2 || start[0] != 0x78 || !matches!(start[1], 0x01 | 0x5E | 0x9C | 0xDA) {
            return Ok(None);
        }
        let mut inflater = flate2::Decompress::new(true);
        let mut scratch = std::mem::take(&mut self.scratch);
        let (mut head, mut head_len) = ([0u8; 16], 0usize);
        let result = loop {
            if cancel.load(Ordering::Relaxed) {
                self.scratch = scratch;
                return Err(RpfError::Cancelled);
            }
            let (read, written) = (inflater.total_in(), inflater.total_out());
            let input = match self.at(pos + read) {
                Ok(input) if !input.is_empty() => input,
                Ok(_) => break None,
                Err(e) => {
                    self.scratch = scratch;
                    return Err(e);
                }
            };
            let status = inflater.decompress(input, &mut scratch, flate2::FlushDecompress::None);
            let produced = (inflater.total_out() - written) as usize;
            let take = produced.min(16 - head_len);
            head[head_len..head_len + take].copy_from_slice(&scratch[..take]);
            head_len += take;
            match status {
                Ok(flate2::Status::StreamEnd) => break Some(()),
                Ok(_) if inflater.total_in() > read || produced > 0 => {}
                _ => break None,
            }
        };
        self.scratch = scratch;
        Ok(result.filter(|()| inflater.total_out() > 0).map(|()| Stream {
            offset: pos,
            stored: inflater.total_in(),
            size: inflater.total_out(),
            kind: Kind::Zlib,
            head,
            head_len: head_len as u8,
        }))
    }
}

/// The length of the audio container that starts here, from its own tables:
/// `ADAT`, version 1, flags, stream count, header length; optional tables
/// chosen by the flags; a word per stream (id, and in the top three bits its
/// number of chunks); a 64-bit word per chunk (type, size, offset). The
/// container ends where its last chunk does.
fn awc_len(start: &[u8]) -> Option<u64> {
    if start.len() < 16 || &start[..4] != b"ADAT" || start[4..6] != [1, 0] {
        return None;
    }
    let flags = u16::from_le_bytes([start[6], start[7]]);
    let count = crate::word(start, 8) as usize;
    let header = u64::from(crate::word(start, 12));
    if !(1..=1 << 16).contains(&count) {
        return None;
    }
    let mut at = 16;
    for (flag, len) in [(0x01, count * 2), (0x20, (count + 1) * 4), (0x40, 4)] {
        if flags & flag != 0 {
            at += len;
        }
    }
    let streams = start.get(at..at + count * 4)?;
    let chunks: usize = streams.as_chunks::<4>().0.iter().map(|w| usize::from(w[3] >> 5)).sum();
    at += count * 4;
    let table = start.get(at..at + chunks * 8)?;
    if (at + chunks * 8) as u64 > header {
        return None;
    }
    let mut end = header;
    for c in table.as_chunks::<8>().0 {
        let v = u64::from_le_bytes(*c);
        end = end.max((v & 0x0FFF_FFFF) + ((v >> 28) & 0x0FFF_FFFF));
    }
    Some(end)
}

const INDEX_RECORD: usize = 46;

/// The entries as bytes, to store them (see [`decode_index`]).
pub fn encode_index(entries: &[Entry]) -> Vec<u8> {
    let mut out = Vec::with_capacity(entries.len() * INDEX_RECORD);
    for e in entries {
        out.extend_from_slice(&e.offset.to_le_bytes());
        out.extend_from_slice(&e.stored.to_le_bytes());
        out.extend_from_slice(&e.size.to_le_bytes());
        out.extend_from_slice(&e.streams.to_le_bytes());
        out.push(match e.kind {
            Kind::Zlib => 0,
            Kind::Bink2 => 1,
            Kind::Awc => 2,
            // Read from the archive, never stored.
            Kind::Native { .. } => u8::MAX,
        });
        out.push(e.head_len.min(16));
        out.extend_from_slice(&e.head);
    }
    out
}

/// Entries stored with [`encode_index`]; `None` if the bytes are not such a
/// list or it is not sorted by offset.
pub fn decode_index(data: &[u8]) -> Option<Vec<Entry>> {
    let (records, rest) = data.as_chunks::<INDEX_RECORD>();
    if !rest.is_empty() {
        return None;
    }
    let u64_at = |r: &[u8], at: usize| Some(u64::from_le_bytes(r.get(at..at + 8)?.try_into().ok()?));
    let mut entries = Vec::with_capacity(records.len());
    for r in records {
        let kind = match r[28] {
            0 => Kind::Zlib,
            1 => Kind::Bink2,
            2 => Kind::Awc,
            _ => return None,
        };
        let entry = Entry {
            offset: u64_at(r, 0)?,
            stored: u64_at(r, 8)?,
            size: u64_at(r, 16)?,
            streams: crate::word(r, 24),
            kind,
            head: r[30..46].try_into().ok()?,
            head_len: r[29],
        };
        let end = entry.offset.checked_add(entry.stored)?;
        if entry.offset < HEADER_LEN as u64
            || entry.stored == 0
            || entry.size == 0
            || entry.streams == 0
            || entry.head_len > 16
            || u64::from(entry.head_len) > entry.size
            || (entry.kind != Kind::Zlib && (entry.stored != entry.size || entry.streams != 1))
            || u64::from(entry.streams) > entry.stored
            || end <= entry.offset
            || entries.last().is_some_and(|prev: &Entry| prev.offset + prev.stored > entry.offset)
        {
            return None;
        }
        entries.push(entry);
    }
    Some(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::ZlibEncoder;

    /// An audio container of one stream with two chunks, the last one
    /// ending at `end`; `flags` chooses the optional tables.
    fn awc(flags: u16, end: u32) -> Vec<u8> {
        let mut out = b"ADAT".to_vec();
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&flags.to_le_bytes());
        out.extend_from_slice(&1u32.to_le_bytes());
        let tables: u32 =
            [(0x01, 2), (0x20, 8), (0x40, 4)].iter().filter(|(f, _)| flags & f != 0).map(|(_, n)| n).sum();
        let header = 16 + tables + 4 + 16;
        out.extend_from_slice(&header.to_le_bytes());
        out.resize(16 + tables as usize, 0xEE);
        out.extend_from_slice(&(7u32 | (2 << 29)).to_le_bytes());
        let chunk = |kind: u64, size: u32, offset: u32| (kind << 56) | (u64::from(size) << 28) | u64::from(offset);
        out.extend_from_slice(&chunk(0x76, 32, header).to_le_bytes());
        out.extend_from_slice(&chunk(0x55, end - header - 32, header + 32).to_le_bytes());
        out.resize(end as usize, 0x55);
        out
    }

    #[test]
    fn named_entries_are_listed_by_their_names() {
        let names = Names::new([
            (0x300, "w_pistol.#dr".to_owned()),
            (0x200, "W_Pistol.#dr".to_owned()),
            (0x400, "odd name/..#ft".to_owned()),
            (0x500, String::new()),
        ]);
        assert_eq!(names.len(), 3);
        // The same name twice: the later entry gets a number.
        assert_eq!((names.get(0x200), names.get(0x300)), (Some("W_Pistol.#dr"), Some("w_pistol~2.#dr")));
        assert_eq!(names.get(0x400), Some("odd_name_..#ft"));
        assert_eq!(names.offset_of("w_PISTOL~2.#DR"), Some(0x300));
        assert_eq!((Names::group("w_pistol.#dr"), Names::group("plain.#dr")), ("w".to_owned(), "other".to_owned()));

        let (data, _) = archive();
        let dir = std::env::temp_dir().join(format!("rage-rpf9-names-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("named.rpf");
        std::fs::write(&path, &data).unwrap();
        let rpf = Rpf9::open(&path).unwrap();
        let entries = rpf.scan(&AtomicBool::new(false), &|_, _| {}).unwrap();
        rpf.set_names(Names::new([(entries[0].offset, "veh_car.#ft".to_owned())]));
        assert_eq!(rpf.entry_name(&entries[0]), "models/veh/veh_car.#ft");
        assert_eq!(rpf.find(&entries, "Models/VEH/veh_car.#ft"), Some(0));
        assert_eq!(rpf.find(&entries, "xml/000000000100.xml"), None);
        assert_eq!(rpf.find(&entries, &rpf.entry_name(&entries[1])), Some(1));
        assert_eq!(rpf.list_dir(&entries, "").0, ["bin", "bk2", "models", "psin"]);
        assert_eq!(rpf.list_dir(&entries, "models"), (vec!["veh".to_owned()], vec![]));
        assert_eq!(rpf.list_dir(&entries, "models/veh"), (vec![], vec![0]));
        // The start of a large entry: at least what was asked for, not all of it.
        let start = rpf.read_start(&entries[1], 100).unwrap();
        assert!(start.len() >= 100 && (start.len() as u64) < entries[1].size);
        assert_eq!(rpf.read_start(&entries[0], 1 << 20).unwrap(), rpf.read(&entries[0]).unwrap());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn finds_audio_containers_by_their_own_tables() {
        assert_eq!(awc_len(&awc(0xFF21, 0x900)), Some(0x900));
        assert_eq!(awc_len(&awc(0xFF61, 0x400)), Some(0x400));
        // Not version 1, no streams, a chunk table that is cut off.
        let mut other = awc(0xFF21, 0x900);
        other[4] = 2;
        assert_eq!(awc_len(&other), None);
        other = awc(0xFF21, 0x900);
        other[8] = 0;
        assert_eq!(awc_len(&other), None);
        assert_eq!(awc_len(&awc(0xFF21, 0x900)[..40]), None);

        let mut data = vec![0u8; 0x100];
        data[..4].copy_from_slice(&MAGIC.to_le_bytes());
        data[4..8].copy_from_slice(&0x100u32.to_le_bytes());
        data.extend(zlib(b"some text in front"));
        pad(&mut data);
        let at = data.len() as u64;
        data.extend(awc(0xFF21, 0x900));
        pad(&mut data);
        data.extend(zlib(b"and some after it"));
        let dir = std::env::temp_dir().join(format!("rage-rpf9-awc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("audio.rpf");
        std::fs::write(&path, &data).unwrap();
        let rpf = Rpf9::open(&path).unwrap();
        let entries = rpf.scan(&AtomicBool::new(false), &|_, _| {}).unwrap();
        let kinds: Vec<Kind> = entries.iter().map(|e| e.kind).collect();
        assert_eq!(kinds, [Kind::Zlib, Kind::Awc, Kind::Zlib]);
        assert_eq!((entries[1].offset, entries[1].stored, entries[1].extension().as_str()), (at, 0x900, "awc"));
        assert_eq!(rpf.read(&entries[1]).unwrap(), awc(0xFF21, 0x900));
        assert_eq!(decode_index(&encode_index(&entries)).as_deref(), Some(&entries[..]));
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn zlib(data: &[u8]) -> Vec<u8> {
        let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
        enc.write_all(data).unwrap();
        enc.finish().unwrap()
    }

    fn pad(out: &mut Vec<u8>) {
        out.resize(out.len().next_multiple_of(ALIGN as usize), 0);
    }

    /// An archive with an opaque table and, in its data area: a small XML
    /// file, a file of two and a half chunks, noise with a stream header that
    /// leads nowhere, a Bink 2 video, a tagged binary.
    fn archive() -> (Vec<u8>, Vec<u8>) {
        let mut out = vec![0u8; 0x100];
        out[..4].copy_from_slice(&MAGIC.to_le_bytes());
        out[4..8].copy_from_slice(&0x100u32.to_le_bytes());
        out[0x40..0x100].fill(0xA7);
        out.extend(zlib(b"<?xml version=\"1.0\"?><a/>"));
        pad(&mut out);
        let big: Vec<u8> = (0..CHUNK as usize * 2 + 1000).map(|i| (i * 31 % 251) as u8).collect();
        for chunk in big.chunks(CHUNK as usize) {
            out.extend(zlib(chunk));
            pad(&mut out);
        }
        out.extend([0x78, 0x9C, 0xFF, 0xFF, 0x13, 0x37, b'K', b'B', b'2', 0x00]);
        pad(&mut out);
        let mut video = vec![0u8; 64];
        video[..4].copy_from_slice(b"KB2j");
        video[4..8].copy_from_slice(&56u32.to_le_bytes());
        video[8..12].copy_from_slice(&30u32.to_le_bytes());
        video[20..24].copy_from_slice(&1920u32.to_le_bytes());
        video[24..28].copy_from_slice(&1080u32.to_le_bytes());
        out.extend(&video);
        pad(&mut out);
        out.extend(zlib(b"PSIN\x01\x00\x00\x00rest of a tagged file"));
        (out, big)
    }

    fn write(name: &str, data: &[u8]) -> PathBuf {
        let path = std::env::temp_dir().join(format!("openrage-rpf9-{}-{name}.rpf", std::process::id()));
        std::fs::write(&path, data).unwrap();
        path
    }

    #[test]
    fn scan_finds_and_joins_streams() {
        let (data, big) = archive();
        let path = write("scan", &data);
        let rpf = Rpf9::open(&path).unwrap();
        assert_eq!(rpf.header().data_offset, 0x100);
        let entries = rpf.entries().unwrap();
        let names: Vec<String> = entries.iter().map(|e| rpf.entry_name(e)).collect();
        let exts: Vec<&str> = names.iter().map(|n| n.split('/').next().unwrap()).collect();
        assert_eq!(exts, ["xml", "bin", "bk2", "psin"]);
        assert_eq!(names[0], "xml/000000000100.xml");
        assert_eq!((entries[1].streams, entries[1].size), (3, big.len() as u64));
        assert_eq!(rpf.read(&entries[1]).unwrap(), big);
        assert_eq!(rpf.read(&entries[0]).unwrap(), b"<?xml version=\"1.0\"?><a/>");
        assert_eq!(rpf.read(&entries[2]).unwrap().len(), 64);
        assert!(rpf.read(&entries[3]).unwrap().starts_with(b"PSIN"));

        assert_eq!(rpf.find(&entries, &names[3].to_ascii_uppercase()), Some(3));
        assert_eq!(rpf.find(&entries, "xml/000000000101.xml"), None);
        assert_eq!(rpf.find(&entries, &names[3].replace("psin/", "xml/")), None);
        let (dirs, files) = rpf.list_dir(&entries, "");
        assert_eq!((dirs, files.len()), (vec!["bin".into(), "bk2".into(), "psin".into(), "xml".into()], 0));
        assert_eq!(rpf.list_dir(&entries, "psin"), (vec![], vec![3]));
        assert!(rpf.is_dir(&entries, "bk2") && !rpf.is_dir(&entries, "ytd") && !rpf.is_dir(&entries, "xml/000"));

        assert_eq!(decode_index(&encode_index(&entries)).as_deref(), Some(&entries[..]));
        assert_eq!(decode_index(&encode_index(&entries)[1..]), None);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_stream_with_a_wrong_checksum_is_not_a_file() {
        let mut data = vec![0u8; 0x40];
        data[..4].copy_from_slice(&MAGIC.to_le_bytes());
        data[4..8].copy_from_slice(&0x40u32.to_le_bytes());
        let mut stream = zlib(b"some text that is long enough to matter");
        let last = stream.len() - 1;
        stream[last] ^= 0x55;
        data.extend(stream);
        let path = write("checksum", &data);
        assert_eq!(Rpf9::open(&path).unwrap().entries().unwrap().len(), 0);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn scan_can_be_cancelled_and_rejects_bad_headers() {
        let (data, _) = archive();
        let path = write("cancel", &data);
        let rpf = Rpf9::open(&path).unwrap();
        assert!(matches!(rpf.scan(&AtomicBool::new(true), &|_, _| {}), Err(RpfError::Cancelled)));
        let mut bad = data.clone();
        bad[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        std::fs::write(&path, &bad).unwrap();
        assert!(matches!(Rpf9::open(&path), Err(RpfError::CorruptToc { .. })));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn clear_header_reports_native_slots_and_transform_tag() {
        let mut bytes = [0u8; HEADER_LEN];
        bytes[..4].copy_from_slice(&MAGIC.to_le_bytes());
        bytes[4..8].copy_from_slice(&0x9580u32.to_le_bytes());
        bytes[0x24..0x28].copy_from_slice(&418u32.to_le_bytes());
        bytes[0x2e] = 9;
        let header = Header::parse(&bytes).unwrap();
        assert_eq!(header.data_offset, 0x9580);
        assert_eq!(header.entry_count, 418);
        assert_eq!(header.toc_transform, 9);
        assert!(Header::parse(&bytes[..HEADER_LEN - 1]).is_none());
    }

    #[test]
    fn a_stream_scan_honors_cancellation_during_inflate() {
        let (data, _) = archive();
        let path = write("cancel-inflate", &data);
        let mut scanner = Scanner::new(File::open(&path).unwrap(), data.len() as u64);
        assert!(matches!(scanner.stream_at(0x100, &AtomicBool::new(true)), Err(RpfError::Cancelled)));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn extraction_rejects_bad_lengths_and_truncated_stored_files() {
        let (data, _) = archive();
        let path = write("read-bounds", &data);
        let rpf = Rpf9::open(&path).unwrap();
        let entries = rpf.entries().unwrap();
        let mut entry = entries[0].clone();
        entry.size = 1;
        let mut out = Vec::new();
        assert!(rpf.copy_to(&entry, &mut out).is_err());
        assert!(out.len() <= 2);
        entry = entries[0].clone();
        entry.offset = 0;
        assert!(matches!(rpf.copy_to(&entry, &mut Vec::new()), Err(RpfError::OutOfBounds { .. })));

        // The file can be replaced after opening an archive. Stored media
        // must report a short read just as compressed entries do.
        let entry = &entries[2];
        std::fs::OpenOptions::new().write(true).open(&path).unwrap().set_len(entry.offset + 10).unwrap();
        assert!(rpf.copy_to(entry, &mut Vec::new()).is_err());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn cached_entries_reject_overflow_overlap_and_invalid_records() {
        let (data, _) = archive();
        let path = write("index-bounds", &data);
        let entries = Rpf9::open(&path).unwrap().entries().unwrap();
        let encoded = encode_index(&entries);
        for (at, bytes) in [
            (0, u64::MAX.to_le_bytes().to_vec()),
            (8, 0u64.to_le_bytes().to_vec()),
            (24, 0u32.to_le_bytes().to_vec()),
            (29, vec![17]),
            (INDEX_RECORD, entries[0].offset.to_le_bytes().to_vec()),
        ] {
            let mut bad = encoded.clone();
            bad[at..at + bytes.len()].copy_from_slice(&bytes);
            assert!(decode_index(&bad).is_none(), "accepted invalid field at {at}");
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn extensions_come_from_the_first_bytes() {
        let ext = |head: &[u8]| {
            let mut e =
                Entry { offset: 0, stored: 0, size: 0, streams: 1, kind: Kind::Zlib, head: [0; 16], head_len: 0 };
            e.head[..head.len()].copy_from_slice(head);
            e.head_len = head.len() as u8;
            e.extension()
        };
        assert_eq!(ext(b"\xEF\xBB\xBF<?xml version"), "xml");
        assert_eq!(ext(b"<Item>\r\n  <a>"), "xml");
        assert_eq!(ext(b"NAME = value\r\n"), "txt");
        assert_eq!(ext(b"PSIN\x00\x01\x02\x03"), "psin");
        assert_eq!(ext(b"0DRP\x00\x00\x00\x00"), "0drp");
        assert_eq!(ext(b"17MY\x00\x00\x00\x00"), "bin");
        assert_eq!(ext(b"\x00\x00\x00\x00"), "bin");
        assert_eq!(ext(b""), "bin");
    }
}
