//! Editing an RPF8 archive (Red Dead Redemption 2) in place.
//!
//! Works like the RPF7 [`ArchiveEditor`](crate::ArchiveEditor): new and
//! changed entries are appended at the end of the file, and the header and
//! entry table at the start are rewritten. Entries whose data lies where a
//! grown table goes are copied to the end as they are stored (the cipher of
//! an entry does not depend on its position). Data of replaced or removed
//! entries stays in the file, unreferenced.
//!
//! What is written:
//! - The entry table in **plain text** (decryption tag 0xFF). OpenRAGE can
//!   only decrypt RDR2's cipher, not encrypt with it; the game reads plain
//!   tables (its own tools write them, and mods built this way load).
//! - The 256-byte signature after the header is kept as it was. It no
//!   longer matches the changed table; the game does not reject it for
//!   archives loaded through a mod loader.
//! - Entries untouched by the change keep their stored bytes: encrypted,
//!   compressed and signed as before.
//! - New data is stored as is: not encrypted, not compressed, not signed.
//!   Resources are written from an `RSC8` file ([`Rpf8::resource_file`]).
//! - The archive's name table: rewritten with every known path when it was
//!   readable (plain text), otherwise kept byte for byte.
//!
//! RPF8 has no folders; paths are only hashed. A path given here is hashed
//! the way [`path_hash`] does it, or names an unnamed entry as
//! `hash/XXXXXXXX.ext`.
//!
//! Every commit hands a [`Backup`] of the overwritten start of the file and
//! the old length to the caller before anything is written; it undoes the
//! commit exactly ([`Backup::restore`]).

use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::RpfError;
use crate::rpf7::editor::{Backup, CommitStats};
use crate::rpf8::names::NameIndex;
use crate::rpf8::{
    Compression, ENTRY_LEN, Entry, HEADER_LEN, MAGIC, PLAINTEXT, Rdr2Keys, Rpf8, SIGNATURE_LEN, extension, path_hash,
    resource_file_flags,
};

fn err(msg: impl Into<String>) -> RpfError {
    RpfError::CorruptToc { archive: "RPF8 editor".into(), reason: msg.into() }
}

/// What an entry's data will be after the commit.
enum Data {
    /// Stored bytes at this offset of the original file.
    Existing(u64),
    /// New bytes, stored as is.
    New(Vec<u8>),
}

struct Slot {
    entry: Entry,
    data: Data,
}

pub struct Rpf8Editor {
    path: PathBuf,
    rpf: Rpf8,
    slots: Vec<Slot>,
    /// The original name table, when it is not plain text (kept as is).
    opaque_names: Option<Vec<u8>>,
    /// Paths of new or renamed entries, for callers that keep names elsewhere.
    added_names: Vec<String>,
    changed: bool,
}

/// Parses `hash/XXXXXXXX[.ext]`, the name of an entry whose path is unknown.
fn hash_name(path: &str, platform: char) -> Option<(u32, Option<u8>)> {
    let rest = path.strip_prefix("hash/")?;
    let (hex, ext) = match rest.split_once('.') {
        Some((h, e)) => (h, Some(e)),
        None => (rest, None),
    };
    if hex.len() != 8 {
        return None;
    }
    let hash = u32::from_str_radix(hex, 16).ok()?;
    let ext = match ext {
        Some(e) => Some(crate::rpf8::extension_id(&e.to_ascii_lowercase(), platform)?),
        None => None,
    };
    Some((hash, ext))
}

fn normalize(path: &str) -> String {
    path.replace('\\', "/").split('/').filter(|s| !s.is_empty()).collect::<Vec<_>>().join("/")
}

/// Whether a name table is plain text: NUL-separated printable paths.
fn plain_names(blob: &[u8]) -> bool {
    !blob.is_empty()
        && blob
            .split(|&b| b == 0)
            .filter(|n| !n.is_empty())
            .all(|n| std::str::from_utf8(n).is_ok_and(|s| s.bytes().all(|b| b.is_ascii_graphic() || b == b' ')))
}

impl Rpf8Editor {
    /// Opens a top-level archive file for editing. Its entry table must be
    /// readable: plain, or encrypted with a key in `keys`.
    pub fn open(path: &Path, keys: Option<Arc<Rdr2Keys>>, names: Option<Arc<NameIndex>>) -> Result<Self, RpfError> {
        let mut rpf = Rpf8::open_with_keys(path, keys)?;
        if let Some(n) = names {
            rpf.set_names(n);
        }
        let entries = rpf
            .entries()
            .ok_or_else(|| RpfError::KeysRequired(format!("{} (its entry table is encrypted)", path.display())))?
            .to_vec();
        let header = rpf.header();
        let opaque_names = if header.names_length == 0 {
            None
        } else {
            let blob = rpf.read_name_table()?;
            (!plain_names(&blob)).then_some(blob)
        };
        let slots = entries.into_iter().map(|entry| Slot { data: Data::Existing(entry.offset), entry }).collect();
        Ok(Self { path: path.to_owned(), rpf, slots, opaque_names, added_names: Vec::new(), changed: false })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The archive as it was when opened (for reading entries, e.g. nested archives).
    pub fn archive(&self) -> &Rpf8 {
        &self.rpf
    }

    pub fn platform(&self) -> char {
        self.rpf.platform()
    }

    /// Paths given to new or renamed entries in this session.
    pub fn added_names(&self) -> &[String] {
        &self.added_names
    }

    /// The display name of a slot, as [`Rpf8::entry_name`] would give it.
    fn name_of(&self, slot: &Slot) -> String {
        slot.entry.display_name(self.platform())
    }

    /// The slot a path names: by its known path, as `hash/…`, or by its hash.
    fn find(&self, path: &str) -> Option<usize> {
        let path = normalize(path);
        if let Some(i) = self.slots.iter().position(|s| self.name_of(s).eq_ignore_ascii_case(&path)) {
            return Some(i);
        }
        // Entries the reader shows under `hash/` with an extension of its
        // own choosing (audio containers): match by hash alone.
        if let Some((hash, ext)) = hash_name(&path, self.platform()) {
            return self
                .slots
                .iter()
                .position(|s| s.entry.hash == hash && ext.is_none_or(|e| e == s.entry.ext_id))
                .or_else(|| self.slots.iter().position(|s| s.entry.hash == hash && s.entry.path.is_none()));
        }
        let (hash, ext) = path_hash(&path, self.platform());
        self.slots.iter().position(|s| s.entry.hash == hash && s.entry.ext_id == ext)
    }

    /// Whether the archive has an entry at `path`.
    pub fn exists(&self, path: &str) -> bool {
        self.find(path).is_some()
    }

    /// Whether `dir` is a folder: some entry's path starts with it.
    pub fn is_dir(&self, dir: &str) -> bool {
        let prefix = format!("{}/", normalize(dir).to_ascii_lowercase());
        self.slots.iter().any(|s| self.name_of(s).to_ascii_lowercase().starts_with(&prefix))
    }

    /// The hash and extension id a new entry at `path` gets.
    fn new_key(&self, path: &str) -> (u32, u8) {
        match hash_name(path, self.platform()) {
            Some((hash, Some(ext))) => (hash, ext),
            Some((hash, None)) => (hash, 0xFF),
            None => path_hash(path, self.platform()),
        }
    }

    /// Adds or replaces the file at `path`. Data starting with an `RSC8`
    /// header becomes a resource; anything else a plain file.
    pub fn set(&mut self, path: &str, data: Vec<u8>) -> Result<(), RpfError> {
        let path = normalize(path);
        if path.is_empty() {
            return Err(err("empty path"));
        }
        let (mut entry, data) = match resource_file_flags(&data) {
            Some(flags) => {
                let size = (flags & 0xFFFF_FFF0) + (flags >> 32 & 0xFFFF_FFF0);
                let mut stored = data;
                stored.truncate(16 + size as usize);
                (Entry::new_plain(0, 0, true, flags), stored)
            }
            None => {
                let len = data.len() as u64;
                (Entry::new_plain(0, 0, false, len), data)
            }
        };
        match self.find(&path) {
            Some(i) => {
                let old = &self.slots[i].entry;
                if old.is_resource != entry.is_resource {
                    let what = |r: bool| if r { "a resource (RSC8 file)" } else { "a plain file" };
                    return Err(err(format!(
                        "{path} is {} in the archive; the new data is {}",
                        what(old.is_resource),
                        what(entry.is_resource)
                    )));
                }
                entry.hash = old.hash;
                entry.ext_id = old.ext_id;
                entry.path = old.path.clone();
                self.slots[i] = Slot { entry, data: Data::New(data) };
            }
            None => {
                if self.is_dir(&path) {
                    return Err(err(format!("{path} is a folder")));
                }
                let (hash, ext) = self.new_key(&path);
                if entry.is_resource && ext == 0xFF {
                    return Err(err(format!("{path}: resources need one of the archive's resource extensions")));
                }
                entry.hash = hash;
                entry.ext_id = ext;
                entry.path = hash_name(&path, self.platform()).is_none().then(|| path.clone());
                if entry.path.is_some() {
                    self.added_names.push(path.clone());
                }
                self.slots.push(Slot { entry, data: Data::New(data) });
            }
        }
        self.changed = true;
        Ok(())
    }

    /// Removes a file, or every file below a folder. Returns how many entries went.
    pub fn remove(&mut self, path: &str) -> usize {
        let path = normalize(path);
        let prefix = format!("{}/", path.to_ascii_lowercase());
        let single = self.find(&path);
        let before = self.slots.len();
        let names: Vec<String> = self.slots.iter().map(|s| self.name_of(s).to_ascii_lowercase()).collect();
        let mut i = 0;
        self.slots.retain(|_| {
            let keep = Some(i) != single && !names[i].starts_with(&prefix);
            i += 1;
            keep
        });
        let removed = before - self.slots.len();
        self.changed |= removed > 0;
        removed
    }

    /// Renames a file, or every file below a folder: the entries get the
    /// hashes of their new paths.
    pub fn rename(&mut self, from: &str, to: &str) -> Result<(), RpfError> {
        let (from, to) = (normalize(from), normalize(to));
        if to.is_empty() {
            return Err(err("empty path"));
        }
        let moves: Vec<(usize, String)> = match self.find(&from) {
            Some(i) => vec![(i, to.clone())],
            None => {
                let prefix = format!("{}/", from.to_ascii_lowercase());
                self.slots
                    .iter()
                    .enumerate()
                    .filter_map(|(i, s)| {
                        let name = self.name_of(s);
                        name.to_ascii_lowercase()
                            .starts_with(&prefix)
                            .then(|| (i, format!("{to}{}", &name[from.len()..])))
                    })
                    .collect()
            }
        };
        if moves.is_empty() {
            return Err(RpfError::NotFound(from));
        }
        for (i, new) in &moves {
            if hash_name(new, self.platform()).is_some() {
                return Err(err(format!("{new}: give the entry a real name")));
            }
            let (hash, ext) = path_hash(new, self.platform());
            let old = &self.slots[*i].entry;
            if ext != old.ext_id {
                let shown = |id: u8| extension(id, self.platform()).unwrap_or_else(|| "none".into());
                return Err(err(format!(
                    "{new}: the extension cannot change ({} → {})",
                    shown(old.ext_id),
                    shown(ext)
                )));
            }
            if let Some(j) = self.find(new).filter(|j| j != i) {
                return Err(err(format!("{} already exists", self.name_of(&self.slots[j]))));
            }
            let e = &mut self.slots[*i].entry;
            e.hash = hash;
            e.path = Some(new.clone());
            self.added_names.push(new.clone());
        }
        self.changed = true;
        Ok(())
    }

    /// The name table to write: every known path (plain), or the original one.
    fn name_table(&self) -> Vec<u8> {
        if let Some(blob) = &self.opaque_names {
            return blob.clone();
        }
        if self.rpf.header().names_length == 0 && self.added_names.is_empty() {
            return Vec::new();
        }
        let mut paths: Vec<&str> = self.slots.iter().filter_map(|s| s.entry.path.as_deref()).collect();
        paths.sort_unstable();
        paths.dedup();
        let mut out = Vec::new();
        for p in paths {
            out.extend(p.as_bytes());
            out.push(0);
        }
        out
    }

    /// Writes all changes. `before_write` receives the backup and must keep
    /// it (e.g. in a journal) before the archive is touched; an error aborts.
    pub fn commit(
        mut self,
        before_write: &mut dyn FnMut(&Backup) -> Result<(), RpfError>,
    ) -> Result<CommitStats, RpfError> {
        if !self.changed {
            return Ok(CommitStats::default());
        }
        let old_len = std::fs::metadata(&self.path)?.len();
        // The game binary-searches the table by hash.
        self.slots.sort_by_key(|s| s.entry.hash);
        let count = self.slots.len();
        let table_end = HEADER_LEN + SIGNATURE_LEN + (count * ENTRY_LEN) as u64;
        let data_start = table_end.next_multiple_of(16);

        // Where every entry's data goes: unchanged entries stay unless the
        // table now covers them; the rest is appended after the old end.
        let cursor = old_len.next_multiple_of(16).max(data_start);
        let (writes, cursor, relocated) = self.place_data(data_start, cursor)?;
        // The name table ends the archive, which ends on a 16-byte boundary
        // (nested archives are as long as their entry's 16-byte rounded size).
        let names = self.name_table();
        let names_at = (cursor + names.len() as u64).next_multiple_of(16) - names.len() as u64;
        let new_len = names_at + names.len() as u64;

        let mut f = OpenOptions::new().read(true).write(true).open(&self.path)?;
        let mut region = vec![0u8; data_start.min(old_len) as usize];
        f.read_exact(&mut region)?;
        let table = self.build_table(data_start, names.len(), &region);

        let backup = Backup { old_len, new_len, region };
        before_write(&backup)?;

        let appended: u64 = writes.iter().map(|(_, b)| b.len() as u64).sum();
        write_archive(&mut f, &writes, &names, names_at, new_len, &table)?;
        Ok(CommitStats { changed: true, appended, relocated, new_len })
    }

    /// Assigns offsets to the entries whose data is written (new ones and
    /// those the table now covers). Returns the writes, the cursor after
    /// them and the number of relocated entries.
    fn place_data(&mut self, data_start: u64, mut cursor: u64) -> Result<(Writes, u64, usize), RpfError> {
        let mut writes = Writes::new();
        let mut relocated = 0;
        let mut stored: Vec<Vec<u8>> = Vec::new();
        for slot in &self.slots {
            if let Data::Existing(offset) = slot.data
                && offset < data_start
                && slot.entry.size_on_disk > 0
            {
                stored.push(self.rpf.read_stored(&slot.entry)?);
                relocated += 1;
            }
        }
        let mut moved = stored.into_iter();
        for slot in &mut self.slots {
            place_slot(slot, data_start, &mut cursor, &mut moved, &mut writes)?;
        }
        Ok((writes, cursor, relocated))
    }

    /// The new table: header, signature block, entries, padded to `data_start`.
    fn build_table(&self, data_start: u64, names_len: usize, region: &[u8]) -> Vec<u8> {
        let header = self.rpf.header();
        let mut table = Vec::with_capacity(data_start as usize);
        table.extend(MAGIC.to_le_bytes());
        table.extend((self.slots.len() as u32).to_le_bytes());
        table.extend((names_len as u32).to_le_bytes());
        table.extend(PLAINTEXT.to_le_bytes());
        // Platform letter and the byte after it, as they were.
        table.push(header.platform as u8);
        table.push(region.get(15).copied().unwrap_or(0));
        // The signature block stays.
        table.extend(&region[HEADER_LEN as usize..(HEADER_LEN + SIGNATURE_LEN) as usize]);
        for slot in &self.slots {
            table.extend(slot.entry.to_bytes());
        }
        table.resize(data_start as usize, 0);
        table
    }
}

/// Data to write: (offset, bytes).
type Writes = Vec<(u64, Vec<u8>)>;

/// Largest `size_on_disk` and offset an entry can hold (28 bits, in 16-byte units).
const MAX_SIZE_ON_DISK: u64 = 0x0FFF_FFFF << 4;
const MAX_OFFSET: u64 = 0x7FFF_FFFF << 4;

/// Gives one slot its new data position, if its data is written again.
fn place_slot(
    slot: &mut Slot,
    data_start: u64,
    cursor: &mut u64,
    moved: &mut impl Iterator<Item = Vec<u8>>,
    writes: &mut Writes,
) -> Result<(), RpfError> {
    let bytes = match &mut slot.data {
        Data::New(d) => Some(std::mem::take(d)),
        Data::Existing(offset) if *offset < data_start && slot.entry.size_on_disk > 0 => {
            Some(moved.next().expect("read above"))
        }
        Data::Existing(_) => None,
    };
    if let Some(bytes) = bytes {
        let len = bytes.len() as u64;
        slot.entry.offset = if len == 0 { 0 } else { *cursor };
        if matches!(slot.data, Data::New(_)) {
            slot.entry.size_on_disk = len.next_multiple_of(16);
            slot.entry.compression = Compression::None;
        }
        if len > 0 {
            writes.push((*cursor, bytes));
            *cursor += len.next_multiple_of(16);
        }
    }
    check_entry_size(&slot.entry)
}

fn check_entry_size(entry: &Entry) -> Result<(), RpfError> {
    if entry.size_on_disk > MAX_SIZE_ON_DISK || entry.offset > MAX_OFFSET {
        return Err(err(format!("{}: too large for an RPF8 entry", self_name(entry))));
    }
    Ok(())
}

fn write_archive(
    f: &mut std::fs::File,
    writes: &[(u64, Vec<u8>)],
    names: &[u8],
    names_at: u64,
    new_len: u64,
    table: &[u8],
) -> Result<(), RpfError> {
    for (at, bytes) in writes {
        f.seek(SeekFrom::Start(*at))?;
        f.write_all(bytes)?;
    }
    write_names(f, names, names_at, new_len)?;
    f.seek(SeekFrom::Start(0))?;
    f.write_all(table)?;
    f.sync_all()?;
    Ok(())
}

/// Writes the name table at the end and cuts the file to its new length.
fn write_names(f: &mut std::fs::File, names: &[u8], names_at: u64, new_len: u64) -> Result<(), RpfError> {
    f.set_len(names_at)?;
    f.seek(SeekFrom::Start(names_at))?;
    f.write_all(names)?;
    f.set_len(new_len)?;
    f.sync_data()?;
    Ok(())
}

/// An RPF8 archive without entries (plain table, empty signature, PC), to
/// fill with [`Rpf8Editor`].
pub fn empty_archive() -> Vec<u8> {
    let mut out = Vec::with_capacity((HEADER_LEN + SIGNATURE_LEN) as usize);
    out.extend(MAGIC.to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend(PLAINTEXT.to_le_bytes());
    out.extend([b'y', 0]);
    out.resize((HEADER_LEN + SIGNATURE_LEN) as usize, 0);
    out
}

fn self_name(e: &Entry) -> String {
    e.path.clone().unwrap_or_else(|| format!("hash/{:08X}", e.hash))
}

#[cfg(test)]
mod tests;
