//! Editing a top-level archive file in place, the way OpenIV does.
//!
//! Changes never move existing data around more than necessary: new and
//! changed entries are appended at the end of the file, and the table of
//! contents at the start is rewritten. Only when the new table no longer fits
//! in front of the first data block are the entries in its way copied to the
//! end as well. Data of replaced or removed entries stays in the file as
//! unreferenced space (like OpenIV, which offers a separate "defragment").
//!
//! Every commit hands a [`Backup`] of the overwritten start region and the old
//! file length to the caller *before* anything is written, which is enough to
//! undo the commit exactly ([`Backup::restore`]).

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rage_crypto::GtaVKeys;

use crate::toc::{self, BLOCK, Meta, TocFile, err};
use crate::writer::{RawEntry, existing_meta};
use crate::{Encryption, EntryId, EntryKind, Rpf7, RpfError, vfs};

/// The bytes a commit overwrites, enough to restore the previous state.
#[derive(Debug, Clone)]
pub struct Backup {
    /// File length before the commit.
    pub old_len: u64,
    /// File length after the commit.
    pub new_len: u64,
    /// Previous contents of the start of the file that the new table of contents overwrites.
    pub region: Vec<u8>,
}

impl Backup {
    /// Undoes the commit this backup was taken for. Refuses if the file was
    /// changed again since (its length differs from `new_len`).
    pub fn restore(&self, path: &Path) -> Result<(), RpfError> {
        let mut f = OpenOptions::new().read(true).write(true).open(path)?;
        let len = f.metadata()?.len();
        if len != self.new_len {
            return Err(RpfError::CorruptToc {
                archive: path.display().to_string(),
                reason: format!("archive changed since (length {len}, expected {})", self.new_len),
            });
        }
        f.seek(SeekFrom::Start(0))?;
        f.write_all(&self.region)?;
        f.set_len(self.old_len)?;
        f.sync_all()?;
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
pub struct CommitStats {
    pub changed: bool,
    /// Bytes appended (new data plus relocated entries).
    pub appended: u64,
    /// Entries moved to the end because the table of contents grew.
    pub relocated: usize,
    pub new_len: u64,
}

enum Slot {
    Existing { id: EntryId, offset: u64, meta: Meta },
    New(RawEntry),
}

pub struct ArchiveEditor {
    path: PathBuf,
    rpf: Rpf7,
    /// Lowercase path → (display path, slot).
    files: BTreeMap<String, (String, Slot)>,
    /// Directories that exist explicitly (also empty ones): lowercase → display path.
    dirs: BTreeMap<String, String>,
    changed: bool,
}

fn normalize(path: &str) -> String {
    path.replace('\\', "/").split('/').filter(|s| !s.is_empty()).collect::<Vec<_>>().join("/")
}

fn under(key: &str, prefix: &str) -> bool {
    key == prefix || key.starts_with(&format!("{prefix}/"))
}

impl ArchiveEditor {
    /// Opens a top-level archive file for editing.
    pub fn open(path: &Path, keys: Option<Arc<GtaVKeys>>) -> Result<Self, RpfError> {
        let rpf = Rpf7::open(path, keys)?;
        if rpf.encryption() == Encryption::Aes {
            return Err(err(format!("{}: AES-encrypted archives cannot be edited", path.display())));
        }
        let mut files = BTreeMap::new();
        let mut dirs = BTreeMap::new();
        let mut failure = None;
        vfs::walk(
            &rpf,
            Rpf7::ROOT,
            "",
            false,
            &mut |a, id, p| {
                let e = a.entry(id);
                if e.is_dir() {
                    dirs.insert(p.to_ascii_lowercase(), p.to_owned());
                    return;
                }
                let offset = match e.kind {
                    EntryKind::Binary { offset, .. } | EntryKind::Resource { offset, .. } => offset,
                    EntryKind::Directory { .. } => unreachable!(),
                };
                match existing_meta(a, id, p) {
                    Ok(meta) => {
                        files.insert(p.to_ascii_lowercase(), (p.to_owned(), Slot::Existing { id, offset, meta }));
                    }
                    Err(e) => failure = Some(e),
                }
            },
            &mut |_| {},
        );
        if let Some(e) = failure {
            return Err(e);
        }
        Ok(Self { path: path.to_owned(), rpf, files, dirs, changed: false })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The archive as it was when opened (for reading entries, e.g. nested archives).
    pub fn archive(&self) -> &Rpf7 {
        &self.rpf
    }

    /// Whether `path` is a file (`Some(false)`), a directory (`Some(true)`) or absent.
    pub fn exists(&self, path: &str) -> Option<bool> {
        let key = normalize(path).to_ascii_lowercase();
        if self.files.contains_key(&key) {
            return Some(false);
        }
        let is_dir = self.dirs.contains_key(&key) || self.files.keys().any(|k| k.starts_with(&format!("{key}/")));
        is_dir.then_some(true)
    }

    /// Adds or replaces a file entry.
    pub fn set(&mut self, path: &str, entry: RawEntry) -> Result<(), RpfError> {
        let display = normalize(path);
        if display.is_empty() {
            return Err(err("empty path"));
        }
        let key = display.to_ascii_lowercase();
        if self.dirs.contains_key(&key) {
            return Err(err(format!("{display} is a directory")));
        }
        self.files.insert(key, (display, Slot::New(entry)));
        self.changed = true;
        Ok(())
    }

    /// Creates a (possibly empty) directory.
    pub fn create_dir(&mut self, path: &str) -> Result<(), RpfError> {
        let display = normalize(path);
        let key = display.to_ascii_lowercase();
        if self.files.contains_key(&key) {
            return Err(err(format!("{display} is a file")));
        }
        self.dirs.insert(key, display);
        self.changed = true;
        Ok(())
    }

    /// Removes a file or a directory with everything below it. Returns the number of entries removed.
    pub fn remove(&mut self, path: &str) -> usize {
        let key = normalize(path).to_ascii_lowercase();
        let before = self.files.len() + self.dirs.len();
        self.files.retain(|k, _| !under(k, &key));
        self.dirs.retain(|k, _| !under(k, &key));
        let removed = before - self.files.len() - self.dirs.len();
        self.changed |= removed > 0;
        removed
    }

    /// Renames (moves) a file or directory to `to` (a full path inside the archive).
    pub fn rename(&mut self, from: &str, to: &str) -> Result<(), RpfError> {
        let (from_key, to) = (normalize(from).to_ascii_lowercase(), normalize(to));
        let to_key = to.to_ascii_lowercase();
        if self.exists(from).is_none() {
            return Err(RpfError::NotFound(from.to_owned()));
        }
        if from_key != to_key && self.exists(&to).is_some() {
            return Err(err(format!("{to} already exists")));
        }
        let moved: Vec<String> = self.files.keys().filter(|k| under(k, &from_key)).cloned().collect();
        for k in moved {
            let (display, slot) = self.files.remove(&k).expect("listed");
            let rest = &display[from_key.len()..];
            let new_display = format!("{to}{rest}");
            self.files.insert(new_display.to_ascii_lowercase(), (new_display, slot));
        }
        let moved: Vec<String> = self.dirs.keys().filter(|k| under(k, &from_key)).cloned().collect();
        for k in moved {
            let display = self.dirs.remove(&k).expect("listed");
            let new_display = format!("{to}{}", &display[from_key.len()..]);
            self.dirs.insert(new_display.to_ascii_lowercase(), new_display);
        }
        self.changed = true;
        Ok(())
    }

    /// Writes all changes. `before_write` receives the backup and must persist
    /// it (e.g. to a journal) before the archive is touched; an error aborts.
    pub fn commit(
        self,
        before_write: &mut dyn FnMut(&Backup) -> Result<(), RpfError>,
    ) -> Result<CommitStats, RpfError> {
        if !self.changed {
            return Ok(CommitStats::default());
        }
        let old_len = std::fs::metadata(&self.path)?.len();
        let entries: Vec<(&String, &Slot)> = self.files.values().map(|(p, s)| (p, s)).collect();
        let dirs: Vec<&str> = self.dirs.values().map(String::as_str).collect();
        let meta = |s: &Slot| match s {
            Slot::Existing { meta, .. } => *meta,
            Slot::New(r) => r.meta(),
        };
        let mut files: Vec<TocFile<'_>> = entries
            .iter()
            .map(|(p, s)| TocFile {
                path: p,
                meta: meta(s),
                block: match s {
                    Slot::Existing { offset, .. } => (offset / BLOCK) as u32,
                    Slot::New(_) => 0,
                },
            })
            .collect();
        let (probe, _) = toc::build(&files, &dirs)?;
        let toc_end = (probe.len() as u64).div_ceil(BLOCK) * BLOCK;

        // Which entries need their data written at the end of the file?
        let mut cursor = old_len.div_ceil(BLOCK).max(1) * BLOCK;
        cursor = cursor.max(toc_end);
        let mut writes: Vec<(u64, std::borrow::Cow<'_, [u8]>, Option<u32>)> = Vec::new();
        let mut relocated = 0;
        for (i, (_, slot)) in entries.iter().enumerate() {
            let bytes: Option<std::borrow::Cow<'_, [u8]>> = match slot {
                Slot::New(r) => Some(r.data().into()),
                Slot::Existing { id, offset, .. } if *offset < toc_end => {
                    relocated += 1;
                    let raw = self.rpf.read_raw_entry(*id)?;
                    Some(match raw {
                        RawEntry::Binary { data, .. } | RawEntry::Resource { data, .. } => data.into(),
                    })
                }
                Slot::Existing { .. } => None,
            };
            if let Some(bytes) = bytes {
                files[i].block = (cursor / BLOCK) as u32;
                let large = match (slot, files[i].meta) {
                    (Slot::New(_), Meta::Resource { len, .. }) => Some(len),
                    _ => None,
                };
                let len = bytes.len() as u64;
                writes.push((cursor, bytes, large));
                cursor += len.div_ceil(BLOCK) * BLOCK;
            }
        }
        let (mut table, _) = toc::build(&files, &dirs)?;
        table.resize(toc_end as usize, 0);

        // Back up what the new table overwrites, then write.
        let mut region = vec![0u8; toc_end.min(old_len) as usize];
        let mut f = OpenOptions::new().read(true).write(true).open(&self.path)?;
        f.read_exact(&mut region)?;
        let backup = Backup { old_len, new_len: cursor, region };
        before_write(&backup)?;

        let appended: u64 = writes.iter().map(|(_, b, _)| b.len() as u64).sum();
        for (at, bytes, large) in &writes {
            f.seek(SeekFrom::Start(*at))?;
            if let Some(len) = large {
                let mut h = bytes[..16].to_vec();
                toc::patch_large_resource_header(&mut h, *len);
                f.write_all(&h)?;
                f.write_all(&bytes[16..])?;
            } else {
                f.write_all(bytes)?;
            }
        }
        f.set_len(cursor)?;
        f.sync_data()?;
        f.seek(SeekFrom::Start(0))?;
        f.write_all(&table)?;
        f.sync_all()?;
        Ok(CommitStats { changed: true, appended, relocated, new_len: cursor })
    }
}
