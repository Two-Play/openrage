//! Incremental index updates.
//!
//! Top-level archives are the unit of change: an archive whose size or
//! modification time differs from the stored values is dropped and re-read,
//! including all archives nested inside it. Loose files are re-listed on
//! every update (cheap). Archives are read in parallel; a single writer
//! commits each archive in its own transaction, so a cancelled update keeps
//! everything finished so far. GTA VI archives hold up to thousands of others
//! and are written in parts; what a cancelled update wrote of one is replaced
//! by the next.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use rage_core::hash::joaat;
use rage_crypto::GtaVKeys;
use rage_rpf::rpf9::Entry;
use rage_rpf::{EntryKind, Gta4Archive, Rpf7, Rpf9, vfs};
use rayon::prelude::*;
use rusqlite::params;

use crate::content::{content_for, is_indexable, is_indexable_gta6, rpf9_content, without_numbers};
use crate::{EntryTag, Index, IndexError, schema};

#[derive(Debug, Clone, Default)]
pub struct Progress {
    pub archives_done: usize,
    pub archives_total: usize,
    pub entries: u64,
    /// How much of the archives that changed is read, in bytes of theirs.
    pub bytes_done: u64,
    pub bytes_total: u64,
    /// Archive that was just written.
    pub current: String,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateStats {
    pub archives_indexed: usize,
    pub archives_removed: usize,
    pub archives_unchanged: usize,
    pub entries_added: u64,
    pub read_errors: usize,
    pub cancelled: bool,
    pub seconds: f32,
}

struct DiskArchive {
    rel: String,
    path: std::path::PathBuf,
    size: i64,
    mtime: i64,
    /// A GTA IV archive (`.img`, RPF2/RPF3) rather than RPF7.
    gta4: bool,
    /// A Red Dead Redemption 2 archive (RPF8) rather than RPF7.
    rdr2: bool,
    /// A GTA VI archive (RPF9) rather than RPF7.
    gta6: bool,
    /// How much of it is read, for the progress.
    read: AtomicU64,
}

/// Largest loose file whose text is indexed (GTA IV's text tables are ~6 MB).
const MAX_LOOSE_TEXT: u64 = 16 << 20;

struct Record {
    vpath: String,
    name: String,
    kind: EntryTag,
    size: u64,
    body: Option<String>,
    /// The hash of a file whose name is not known, from the archive's table.
    hash: Option<u32>,
}

#[derive(Default)]
struct Batch {
    rel: String,
    size: i64,
    mtime: i64,
    records: Vec<Record>,
    dictionary: HashSet<String>,
    errors: usize,
    /// More of the same archive follows.
    partial: bool,
}

pub(crate) fn stem(name: &str) -> &str {
    name.split_once('.').map_or(name, |(s, _)| s)
}

fn mtime_of(meta: &fs::Metadata) -> i64 {
    meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs() as i64)
}

/// Lists loose files/folders and top-level archives below the game directory.
/// Loose text files (GTA IV keeps `.ide`, `.dat` and `.gxt` outside its
/// archives) are indexed with their text.
fn scan_disk(game_dir: &Path) -> (Vec<Record>, Vec<DiskArchive>, HashSet<String>) {
    let mut dictionary = HashSet::new();
    let mut records = Vec::new();
    let mut archives = Vec::new();
    let mut stack = vec![(game_dir.to_path_buf(), String::new())];
    while let Some((dir, rel_dir)) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for entry in rd.flatten() {
            let Some((record, archive)) = scan_entry(&entry, &rel_dir, &mut dictionary) else { continue };
            if record.kind == EntryTag::Dir {
                stack.push((entry.path(), record.vpath.clone()));
            }
            archives.extend(archive);
            records.push(record);
        }
    }
    (records, archives, dictionary)
}

/// The record of one directory entry, and its archive if it is one; `None` if it is skipped.
fn scan_entry(
    entry: &fs::DirEntry,
    rel_dir: &str,
    dictionary: &mut HashSet<String>,
) -> Option<(Record, Option<DiskArchive>)> {
    let meta = entry.metadata().ok()?;
    let name = entry.file_name().to_string_lossy().into_owned();
    if name == ".openrage" {
        return None; // OpenRAGE's journal inside the mods folder
    }
    let rel = if rel_dir.is_empty() { name.clone() } else { format!("{rel_dir}/{name}") };
    let (kind, gta4) = classify_disk_entry(&meta, &name, &entry.path());
    let body = loose_text(entry, &meta, &name, kind, dictionary);
    let archive = (kind == EntryTag::Archive).then(|| DiskArchive {
        rel: rel.clone(),
        path: entry.path(),
        size: meta.len() as i64,
        mtime: mtime_of(&meta),
        gta4,
        rdr2: !gta4 && rage_rpf::Rpf8::file_is_rpf8(&entry.path()),
        gta6: !gta4 && Rpf9::file_is_rpf9(&entry.path()),
        read: AtomicU64::new(0),
    });
    Some((Record { vpath: rel, name, kind, size: meta.len(), body, hash: None }, archive))
}

/// Whether a directory entry is a folder, an archive or a file, and whether it is a GTA IV archive.
fn classify_disk_entry(meta: &fs::Metadata, name: &str, path: &Path) -> (EntryTag, bool) {
    if meta.is_dir() {
        return (EntryTag::Dir, false);
    }
    let lower = name.to_ascii_lowercase();
    let gta4 = (lower.ends_with(".img") || lower.ends_with(".rpf")) && Gta4Archive::file_is_gta4(path);
    if lower.ends_with(".rpf") || gta4 { (EntryTag::Archive, gta4) } else { (EntryTag::File, gta4) }
}

/// The text of a loose file that is indexed with it; its words go to the dictionary.
fn loose_text(
    entry: &fs::DirEntry,
    meta: &fs::Metadata,
    name: &str,
    kind: EntryTag,
    dictionary: &mut HashSet<String>,
) -> Option<String> {
    if kind == EntryTag::File
        && is_indexable(name)
        && meta.len() <= MAX_LOOSE_TEXT
        && let Ok(data) = fs::read(entry.path())
        && let Some((text, words)) = content_for(name, &data)
    {
        dictionary.extend(words);
        return Some(text);
    }
    None
}

/// Reads one GTA IV archive (`.img`, RPF2/RPF3) into a batch, with the key
/// registered by the application.
fn read_gta4_archive(archive: &DiskArchive, mut batch: Batch) -> Batch {
    let a = match Gta4Archive::open_default(&archive.path) {
        Ok(a) => a,
        Err(_) => {
            batch.errors += 1;
            return batch;
        }
    };
    vfs::walk_gta4(&a, Gta4Archive::ROOT, &archive.rel, &mut |id, path| {
        let e = a.entry(id);
        let kind = if e.is_dir() {
            EntryTag::Dir
        } else if e.resource_type().is_some() {
            EntryTag::Resource
        } else {
            EntryTag::File
        };
        let mut body = None;
        if kind == EntryTag::File && is_indexable(&e.name) {
            match a.read(id) {
                Ok(data) => {
                    if let Some((text, words)) = content_for(&e.name, &data) {
                        body = Some(text);
                        batch.dictionary.extend(words);
                    }
                }
                Err(_) => batch.errors += 1,
            }
        }
        batch.records.push(Record {
            vpath: path.to_owned(),
            name: e.name.clone(),
            kind,
            size: e.size(),
            body,
            hash: None,
        });
    });
    batch
}

/// Reads an RDR2 archive (RPF8) and the archives nested in it, with the keys
/// and file names the application registered (`rpf8::set_defaults`). Without
/// keys its entry table cannot be read: one read error.
fn read_rdr2_archive(archive: &DiskArchive, mut batch: Batch) -> Batch {
    use rage_rpf::Rpf8;
    fn walk(rpf: &Rpf8, prefix: &str, depth: u8, batch: &mut Batch) {
        let Some(entries) = rpf.entries() else {
            batch.errors += 1;
            return;
        };
        // RPF8 has no folders of its own: they come from the entries' paths.
        let mut dirs: HashSet<String> = HashSet::new();
        for (i, e) in entries.iter().enumerate() {
            let name = rpf.entry_name(i);
            let vpath = format!("{prefix}/{name}");
            let mut parts: Vec<&str> = name.split('/').collect();
            let base = parts.pop().unwrap_or_default().to_owned();
            let mut dir = prefix.to_owned();
            for part in parts {
                dir = format!("{dir}/{part}");
                if dirs.insert(dir.to_ascii_lowercase()) {
                    batch.records.push(Record {
                        vpath: dir.clone(),
                        name: part.to_owned(),
                        kind: EntryTag::Dir,
                        size: 0,
                        body: None,
                        hash: None,
                    });
                }
            }
            if e.ext_id == 0 {
                batch.records.push(Record {
                    vpath: vpath.clone(),
                    name: base,
                    kind: EntryTag::Archive,
                    size: e.size(),
                    body: None,
                    hash: None,
                });
                match rpf.open_child(e) {
                    Ok(child) if depth < rage_rpf::MAX_NESTING => walk(&child, &vpath, depth + 1, batch),
                    _ => batch.errors += 1,
                }
                continue;
            }
            let kind = if e.is_resource { EntryTag::Resource } else { EntryTag::File };
            let mut body = None;
            if kind == EntryTag::File && is_indexable(&base) {
                match rpf.read(e) {
                    Ok(data) => {
                        if let Some((text, words)) = content_for(&base, &data) {
                            body = Some(text);
                            batch.dictionary.extend(words);
                        }
                    }
                    Err(_) => batch.errors += 1,
                }
            }
            batch.records.push(Record { vpath, name: base, kind, size: e.size(), body, hash: None });
        }
    }
    match Rpf8::open_default(&archive.path) {
        Ok(rpf) => walk(&rpf, &archive.rel, 0, &mut batch),
        Err(_) => batch.errors += 1,
    }
    batch
}

/// Larger members of GTA VI archives are not read for their text.
const MAX_GTA6_TEXT: u64 = 64 << 20;
/// Archives with gigabytes of metadata that is all numbers.
const GTA6_NUMBERS: &str = "/audio/exterior_occlusion/";
/// As many are read and converted at a time, and written in one transaction.
const TEXTS_AT_ONCE: usize = 64;

/// Reads a GTA VI archive (RPF9) and the archives nested in it. Only archives
/// with a readable table are indexed: the recovery view of others takes a scan
/// of the whole file, and has no names. Every nested archive is a part of its
/// own, sent when it is read: opening one takes a while, as it learns names
/// from its members. Returns the last part, or `None` if cancelled.
fn read_gta6_archive(
    archive: &DiskArchive,
    mut batch: Batch,
    cancel: &AtomicBool,
    part: &(dyn Fn(Batch) + Sync),
) -> Option<Batch> {
    match Rpf9::open(&archive.path) {
        Ok(rpf) if rpf.is_native() => {
            let walk = Gta6Walk { cancel, part, new_part: &|| archive.part(), read: &|bytes| archive.add_read(bytes) };
            walk_gta6(&rpf, &archive.rel, 0, &walk);
        }
        // Not readable with this executable: tried again by the next update.
        _ => {
            batch.errors += 1;
            batch.partial = true;
        }
    }
    (!cancel.load(Ordering::Relaxed)).then_some(batch)
}

/// What a walk over a GTA VI archive reports to: cancellation, parts of the result, and bytes read.
#[derive(Clone, Copy)]
struct Gta6Walk<'a> {
    cancel: &'a AtomicBool,
    part: &'a (dyn Fn(Batch) + Sync),
    new_part: &'a (dyn Fn() -> Batch + Sync),
    read: &'a (dyn Fn(u64) + Sync),
}

impl Gta6Walk<'_> {
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// A nested archive and its path.
type Gta6Child<'a> = (&'a Entry, String);
/// A file whose text is read: entry, path, name and kind.
type Gta6Text<'a> = (&'a Entry, String, String, EntryTag);
/// A record with the text read for it.
type Gta6Read = (Record, Result<Option<(String, Vec<String>)>, rage_rpf::RpfError>);

fn walk_gta6(rpf: &Rpf9, prefix: &str, depth: u8, w: &Gta6Walk) {
    let mut batch = (w.new_part)();
    let Ok(entries) = rpf.entries() else {
        batch.errors += 1;
        return (w.part)(batch);
    };
    let (children, texts) = list_gta6_entries(rpf, &entries, prefix, &mut batch);
    if w.cancelled() {
        return;
    }
    (w.part)(batch);
    // A few at a time: converted metadata is several times its size.
    for chunk in texts.chunks(TEXTS_AT_ONCE) {
        let found = read_gta6_texts(rpf, chunk, w.cancel);
        let mut batch = (w.new_part)();
        add_gta6_texts(&mut batch, found);
        if w.cancelled() {
            return;
        }
        (w.part)(batch);
    }
    walk_gta6_children(rpf, &children, depth, w);
}

/// Adds the folders and the files that are not read for their text to the batch; gives the nested archives and
/// the files whose text is read.
fn list_gta6_entries<'a>(
    rpf: &Rpf9,
    entries: &'a [Entry],
    prefix: &str,
    batch: &mut Batch,
) -> (Vec<Gta6Child<'a>>, Vec<Gta6Text<'a>>) {
    // Folders come from the entries' paths.
    let mut dirs: HashSet<String> = HashSet::new();
    let mut children = Vec::new();
    let mut texts = Vec::new();
    for e in entries.iter() {
        let name = rpf.entry_name(e);
        let vpath = format!("{prefix}/{name}");
        let mut parts: Vec<&str> = name.split('/').collect();
        let base = parts.pop().unwrap_or_default().to_owned();
        push_new_folders(batch, &mut dirs, prefix, parts);
        let is_archive = rpf.is_archive(e);
        if is_archive {
            children.push((e, vpath.clone()));
        }
        let kind = gta6_kind(rpf, e, is_archive);
        if kind != EntryTag::Archive
            && is_indexable_gta6(&base)
            && e.size <= MAX_GTA6_TEXT
            && !vpath.contains(GTA6_NUMBERS)
        {
            texts.push((e, vpath, base, kind));
            continue;
        }
        let hash = rpf.unnamed_hash(e);
        batch.records.push(Record { vpath, name: base, kind, size: e.size, body: None, hash });
    }
    (children, texts)
}

fn gta6_kind(rpf: &Rpf9, entry: &Entry, is_archive: bool) -> EntryTag {
    if is_archive {
        EntryTag::Archive
    } else if rpf.resource_parts(entry).is_some() {
        EntryTag::Resource
    } else {
        EntryTag::File
    }
}

/// Adds a record for each folder of an entry's path that was not seen yet.
fn push_new_folders(batch: &mut Batch, dirs: &mut HashSet<String>, prefix: &str, folders: Vec<&str>) {
    let mut dir = prefix.to_owned();
    for folder in folders {
        dir = format!("{dir}/{folder}");
        if dirs.insert(dir.to_ascii_lowercase()) {
            batch.records.push(Record {
                vpath: dir.clone(),
                name: folder.to_owned(),
                kind: EntryTag::Dir,
                size: 0,
                body: None,
                hash: None,
            });
        }
    }
}

/// Reads the texts of a few files at a time, in parallel, until cancelled.
fn read_gta6_texts(rpf: &Rpf9, chunk: &[Gta6Text], cancel: &AtomicBool) -> Vec<Gta6Read> {
    chunk
        .par_iter()
        .filter(|_| !cancel.load(Ordering::Relaxed))
        .map(|(e, vpath, base, kind)| {
            let content = rpf9_content(rpf, e, base).map(|found| {
                let is_text = is_indexable(base) || base.to_ascii_lowercase().ends_with(".#ldbmt");
                found.map(|(text, words)| (if is_text { text } else { without_numbers(&text) }, words))
            });
            let hash = rpf.unnamed_hash(e);
            let record =
                Record { vpath: vpath.clone(), name: base.clone(), kind: *kind, size: e.size, body: None, hash };
            (record, content)
        })
        .collect()
}

fn add_gta6_texts(batch: &mut Batch, found: Vec<Gta6Read>) {
    for (mut record, content) in found {
        match content {
            Ok(Some((text, words))) => {
                record.body = Some(text);
                batch.dictionary.extend(words);
            }
            Ok(None) => {}
            Err(_) => batch.errors += 1,
        }
        batch.records.push(record);
    }
}

fn walk_gta6_children(rpf: &Rpf9, children: &[Gta6Child], depth: u8, w: &Gta6Walk) {
    children.par_iter().for_each(|(e, vpath)| {
        if w.cancelled() {
            return;
        }
        match rpf.open_child(e) {
            Ok(child) if depth < rage_rpf::MAX_NESTING => {
                walk_gta6(&child, vpath, depth + 1, &Gta6Walk { read: &|_| {}, ..*w });
            }
            _ => (w.part)(Batch { errors: 1, ..(w.new_part)() }),
        }
        (w.read)(e.size);
    });
}

impl DiskArchive {
    fn batch(&self) -> Batch {
        Batch { rel: self.rel.clone(), size: self.size, mtime: self.mtime, ..Default::default() }
    }

    fn add_read(&self, bytes: u64) {
        self.read.fetch_add(bytes, Ordering::Relaxed);
    }

    fn bytes_read(&self) -> u64 {
        self.read.load(Ordering::Relaxed).min(self.size as u64)
    }

    /// For some of its entries.
    fn part(&self) -> Batch {
        Batch { partial: true, ..self.batch() }
    }
}

/// Reads one top-level archive (and everything nested in it) into a batch;
/// a GTA VI archive into several, of which all but the last go to `part`.
/// `None` if cancelled.
fn read_archive(
    archive: &DiskArchive,
    keys: Option<Arc<GtaVKeys>>,
    cancel: &AtomicBool,
    part: &(dyn Fn(Batch) + Sync),
) -> Option<Batch> {
    if archive.gta6 {
        return read_gta6_archive(archive, archive.batch(), cancel, part);
    }
    Some(read_other_archive(archive, keys))
}

fn read_other_archive(archive: &DiskArchive, keys: Option<Arc<GtaVKeys>>) -> Batch {
    let mut batch = archive.batch();
    if archive.gta4 {
        return read_gta4_archive(archive, batch);
    }
    if archive.rdr2 {
        return read_rdr2_archive(archive, batch);
    }
    let rpf = match Rpf7::open(&archive.path, keys) {
        Ok(r) => r,
        Err(_) => {
            batch.errors += 1;
            return batch;
        }
    };
    let errors = std::cell::Cell::new(0usize);
    vfs::walk(
        &rpf,
        Rpf7::ROOT,
        &archive.rel,
        true,
        &mut |a, id, path| {
            let e = a.entry(id);
            let kind = match e.kind {
                EntryKind::Directory { .. } => EntryTag::Dir,
                EntryKind::Binary { .. } if e.is_archive() => EntryTag::Archive,
                EntryKind::Binary { .. } => EntryTag::File,
                EntryKind::Resource { .. } => EntryTag::Resource,
            };
            let mut body = None;
            if kind == EntryTag::File && is_indexable(&e.name) {
                match a.read(id) {
                    Ok(data) => {
                        if let Some((text, words)) = content_for(&e.name, &data) {
                            body = Some(text);
                            batch.dictionary.extend(words);
                        }
                    }
                    Err(_) => errors.set(errors.get() + 1),
                }
            }
            batch.records.push(Record {
                vpath: path.to_owned(),
                name: e.name.clone(),
                kind,
                size: e.size(),
                body,
                hash: None,
            });
        },
        &mut |_| errors.set(errors.get() + 1),
    );
    batch.errors += errors.get();
    batch
}

/// A source's id, size and modification time as stored in the index.
type StoredSource = (i64, i64, i64);

/// What the receiving side of an update knows about the archives to read.
struct UpdateRun<'a> {
    total: usize,
    bytes_total: u64,
    counted: &'a [&'a DiskArchive],
}

/// Reads one archive and sends what it yields, unless cancelled.
fn read_one(s: &mpsc::SyncSender<Batch>, archive: &DiskArchive, keys: &Option<Arc<GtaVKeys>>, cancel: &AtomicBool) {
    if cancel.load(Ordering::Relaxed) {
        return;
    }
    let part = |batch| {
        let _ = s.send(batch);
    };
    if let Some(batch) = read_archive(archive, keys.clone(), cancel, &part) {
        archive.add_read(archive.size as u64);
        part(batch);
    }
}

/// Reads `todo` in parallel and sends the batches; runs on its own thread.
fn read_all(
    todo: Vec<&DiskArchive>,
    sender: mpsc::SyncSender<Batch>,
    keys: Option<Arc<GtaVKeys>>,
    cancel: &AtomicBool,
) {
    let read = move || {
        todo.par_iter().for_each_with(sender, |s, archive| read_one(s, archive, &keys, cancel));
    };
    // Threads of its own: what is opened meanwhile does not wait
    // behind the archives, and keeps some processors.
    let threads = std::thread::available_parallelism().map_or(4, |n| (n.get() * 3 / 4).max(1));
    match rayon::ThreadPoolBuilder::new().num_threads(threads).build() {
        Ok(pool) => pool.install(read),
        Err(_) => read(),
    }
}

/// Writes each batch in its own transaction and reports the progress.
/// Returns how many archives are complete.
fn write_received(
    conn: &mut rusqlite::Connection,
    receiver: mpsc::Receiver<Batch>,
    run: &UpdateRun<'_>,
    stats: &mut UpdateStats,
    progress: &mut dyn FnMut(&Progress),
) -> Result<usize, IndexError> {
    let mut done = 0usize;
    for batch in receiver {
        let tx = conn.transaction()?;
        let added = write_batch(&tx, &batch)?;
        tx.commit()?;
        if !batch.partial {
            done += 1;
            stats.archives_indexed += 1;
        }
        stats.entries_added += added;
        stats.read_errors += batch.errors;
        progress(&Progress {
            archives_done: done,
            archives_total: run.total,
            entries: stats.entries_added,
            bytes_done: run.counted.iter().map(|a| a.bytes_read()).sum(),
            bytes_total: run.bytes_total,
            current: batch.rel,
        });
    }
    Ok(done)
}

/// The archives of the index, by path.
fn stored_sources(conn: &rusqlite::Connection) -> Result<HashMap<String, StoredSource>, IndexError> {
    let mut stmt = conn.prepare("SELECT path, id, size, mtime FROM sources")?;
    Ok(stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, (r.get(1)?, r.get(2)?, r.get(3)?))))?
        .collect::<Result<_, _>>()?)
}

/// The ids of the sources to drop; counts the archives that stay and the ones that are gone.
fn stale_sources(
    stored: &HashMap<String, StoredSource>,
    archives: &[DiskArchive],
    stats: &mut UpdateStats,
) -> Vec<i64> {
    let on_disk: HashMap<&str, &DiskArchive> = archives.iter().map(|a| (a.rel.as_str(), a)).collect();
    let mut stale: Vec<i64> = Vec::new();
    for (path, &(id, size, mtime)) in stored {
        if path.is_empty() {
            stale.push(id); // loose files are always re-listed
            continue;
        }
        match on_disk.get(path.as_str()) {
            Some(a) if a.size == size && a.mtime == mtime => stats.archives_unchanged += 1,
            _ => {
                stale.push(id);
                if !path.is_empty() {
                    stats.archives_removed += 1;
                }
            }
        }
    }
    stale
}

/// The archives that are new or differ from the stored ones, big ones first.
fn archives_to_read<'a>(archives: &'a [DiskArchive], stored: &HashMap<String, StoredSource>) -> Vec<&'a DiskArchive> {
    let mut todo: Vec<&DiskArchive> = archives
        .iter()
        .filter(|a| stored.get(&a.rel).is_none_or(|&(_, size, mtime)| size != a.size || mtime != a.mtime))
        .collect();
    todo.sort_by_key(|a| std::cmp::Reverse(a.size)); // big ones first for better parallelism
    // GTA VI archives are only indexed if the game's executable gives the
    // keys. Asking one here also loads those before the threads that read
    // archives wait for each other to do it.
    if !todo.iter().rev().filter(|a| a.gta6).any(|a| Rpf9::open(&a.path).is_ok_and(|rpf| rpf.is_native())) {
        todo.retain(|a| !a.gta6);
    }
    todo
}

impl Index {
    /// Brings the index up to date with the game directory.
    pub fn update(
        &mut self,
        game_dir: &Path,
        keys: Option<Arc<GtaVKeys>>,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(&Progress),
    ) -> Result<UpdateStats, IndexError> {
        let started = Instant::now();
        let mut stats = UpdateStats::default();
        let (loose, archives, loose_words) = scan_disk(game_dir);

        // Which archives changed?
        let stored = stored_sources(&self.conn)?;
        let stale = stale_sources(&stored, &archives, &mut stats);
        let todo = archives_to_read(&archives, &stored);
        self.drop_stale(&stale, Batch { records: loose, dictionary: loose_words, ..Default::default() })?;

        let counted = todo.clone();
        let run =
            UpdateRun { total: todo.len(), bytes_total: todo.iter().map(|a| a.size as u64).sum(), counted: &counted };
        let (sender, receiver) = mpsc::sync_channel::<Batch>(4);
        let conn = &mut self.conn;
        let result: Result<usize, IndexError> = std::thread::scope(|scope| {
            scope.spawn(move || read_all(todo, sender, keys, cancel));
            write_received(conn, receiver, &run, &mut stats, progress)
        });
        let done = result?;

        stats.cancelled = cancel.load(Ordering::Relaxed) && done < run.total;
        if !stats.cancelled {
            self.record_update_time(&stats)?;
        }
        stats.seconds = started.elapsed().as_secs_f32();
        Ok(stats)
    }

    /// Removes the stale sources and writes the loose files, in one transaction.
    fn drop_stale(&mut self, stale: &[i64], loose_batch: Batch) -> Result<(), IndexError> {
        let tx = self.conn.transaction()?;
        for id in stale {
            tx.execute("DELETE FROM content WHERE rowid IN (SELECT id FROM entries WHERE source = ?1)", [id])?;
            tx.execute("DELETE FROM entries WHERE source = ?1", [id])?;
            tx.execute("DELETE FROM sources WHERE id = ?1", [id])?;
        }
        write_batch(&tx, &loose_batch)?;
        tx.commit()?;
        Ok(())
    }

    fn record_update_time(&self, stats: &UpdateStats) -> Result<(), IndexError> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        // Caches built from the index (the world map) only need rebuilding
        // when its contents changed, not after every check. Indexes from
        // before `changed_at` last changed at their last update.
        let changed = stats.archives_indexed > 0 || stats.archives_removed > 0;
        let previous = schema::get_meta(&self.conn, "changed_at")?.or(schema::get_meta(&self.conn, "updated_at")?);
        match previous {
            Some(at) if !changed => schema::set_meta(&self.conn, "changed_at", &at)?,
            _ => schema::set_meta(&self.conn, "changed_at", &now.to_string())?,
        }
        schema::set_meta(&self.conn, "updated_at", &now.to_string())?;
        self.conn.execute_batch("PRAGMA optimize;")?;
        Ok(())
    }
}

fn write_batch(tx: &rusqlite::Transaction<'_>, batch: &Batch) -> Result<u64, IndexError> {
    // An archive that is not complete matches no size on disk.
    let (size, mtime) = if batch.partial { (-1, -1) } else { (batch.size, batch.mtime) };
    let source: i64 = tx.query_row(
        "INSERT INTO sources(path, size, mtime) VALUES(?1, ?2, ?3)
         ON CONFLICT(path) DO UPDATE SET size = excluded.size, mtime = excluded.mtime RETURNING id",
        params![batch.rel, size, mtime],
        |r| r.get(0),
    )?;
    let mut entry = tx.prepare_cached(
        "INSERT INTO entries(source, vpath, name, kind, size, name_hash, stem_hash, has_text)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )?;
    let mut text = tx.prepare_cached("INSERT INTO content(rowid, body) VALUES(?1, ?2)")?;
    for r in &batch.records {
        entry.execute(params![
            source,
            r.vpath,
            r.name,
            r.kind as i64,
            r.size as i64,
            r.hash.unwrap_or_else(|| joaat(&r.name)) as i64,
            r.hash.unwrap_or_else(|| joaat(stem(&r.name))) as i64,
            r.body.is_some() as i64
        ])?;
        if let Some(body) = &r.body {
            text.execute(params![tx.last_insert_rowid(), body])?;
        }
    }
    let mut dict = tx.prepare_cached("INSERT OR IGNORE INTO hashes(hash, string) VALUES(?1, ?2)")?;
    for word in &batch.dictionary {
        dict.execute(params![joaat(word) as i64, word])?;
    }
    Ok(batch.records.len() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plain IMG3 archive with a text file and a (fake) resource.
    fn img(files: &[(&str, &[u8], bool)]) -> Vec<u8> {
        let names: Vec<u8> = files.iter().flat_map(|(n, _, _)| n.bytes().chain([0])).collect();
        let toc_len = files.len() * 16 + names.len();
        let mut out = Vec::new();
        for w in [0xA94E_2A52u32, 3, files.len() as u32, toc_len as u32] {
            out.extend(w.to_le_bytes());
        }
        out.extend(16u16.to_le_bytes());
        out.extend(0u16.to_le_bytes());
        for (k, (_, data, resource)) in files.iter().enumerate() {
            let size = if *resource { 0xC000_0000 } else { data.len() as u32 };
            out.extend(size.to_le_bytes());
            out.extend((if *resource { 110u32 } else { 0 }).to_le_bytes());
            out.extend((k as u32 + 1).to_le_bytes());
            out.extend(1u16.to_le_bytes());
            out.extend(0u16.to_le_bytes());
        }
        out.extend(&names);
        for (_, data, _) in files {
            out.resize(out.len().div_ceil(2048) * 2048, 0);
            out.extend(*data);
        }
        out.resize(out.len().div_ceil(2048) * 2048, 0);
        out
    }

    #[test]
    fn indexes_gta4_archives_and_loose_text() {
        let dir = std::env::temp_dir().join(format!("openrage-index-gta4-{}", std::process::id()));
        let game = dir.join("game");
        fs::create_dir_all(game.join("pc/data/maps")).unwrap();
        fs::create_dir_all(game.join("common/data")).unwrap();
        let data = img(&[
            ("area.ide", b"objs\nlamp_post_01, area_txd, 100, 0\nend\n", false),
            ("lamp_post_01.wdr", b"RSC\x05", true),
        ]);
        fs::write(game.join("pc/data/maps/area.img"), data).unwrap();
        fs::write(game.join("common/data/handling.dat"), b"ADMIRAL 1600.0 2.0\n").unwrap();
        let mut index = Index::open(&dir.join("index.sqlite")).unwrap();
        let stats = index.update(&game, None, &AtomicBool::new(false), &mut |_| {}).unwrap();
        assert_eq!((stats.archives_indexed, stats.read_errors), (1, 0));
        let hits =
            |q: &str| -> Vec<String> { index.search(q, 20, None).unwrap().hits.into_iter().map(|h| h.vpath).collect() };
        assert!(hits("lamp_post_01").contains(&"pc/data/maps/area.img/lamp_post_01.wdr".to_owned()));
        assert!(hits("area_txd").contains(&"pc/data/maps/area.img/area.ide".to_owned()));
        assert!(hits("ADMIRAL").contains(&"common/data/handling.dat".to_owned()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn gta6_archives_without_their_keys_are_left_for_later() {
        let dir = std::env::temp_dir().join(format!("openrage-index-gta6-{}", std::process::id()));
        let game = dir.join("game");
        fs::create_dir_all(&game).unwrap();
        let mut data = vec![0u8; 128];
        data[..4].copy_from_slice(b"9FPR");
        data[4..8].copy_from_slice(&64u32.to_le_bytes());
        fs::write(game.join("common.rpf"), data).unwrap();
        let mut index = Index::open(&dir.join("index.sqlite")).unwrap();
        let stats = index.update(&game, None, &AtomicBool::new(false), &mut |_| {}).unwrap();
        assert_eq!((stats.archives_indexed, stats.read_errors, stats.cancelled), (0, 0, false));
        // The archive itself is found, but is not taken for indexed.
        assert_eq!(index.search("common.rpf", 5, None).unwrap().hits.len(), 1);
        assert_eq!(index.stats().unwrap().archives, 0);
        assert!(index.is_complete().unwrap());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_archive_written_in_part_is_read_again() {
        let dir = std::env::temp_dir().join(format!("openrage-index-part-{}", std::process::id()));
        let game = dir.join("game");
        fs::create_dir_all(&game).unwrap();
        let path = game.join("area.img");
        fs::write(&path, img(&[("a.ide", b"objs\nend\n", false)])).unwrap();
        let meta = fs::metadata(&path).unwrap();
        let mut index = Index::open(&dir.join("index.sqlite")).unwrap();
        let part = |partial, name: &str| Batch {
            rel: "area.img".into(),
            size: meta.len() as i64,
            mtime: mtime_of(&meta),
            records: vec![Record {
                vpath: format!("area.img/{name}"),
                name: name.into(),
                kind: EntryTag::File,
                size: 1,
                body: Some("stale_text".into()),
                hash: None,
            }],
            partial,
            ..Default::default()
        };
        let write = |index: &mut Index, batch: Batch| {
            let tx = index.conn.transaction().unwrap();
            write_batch(&tx, &batch).unwrap();
            tx.commit().unwrap();
        };
        let update = |index: &mut Index| index.update(&game, None, &AtomicBool::new(false), &mut |_| {}).unwrap();
        // Parts are written to one archive; without the last it is replaced.
        write(&mut index, part(true, "one.txt"));
        write(&mut index, part(true, "two.txt"));
        assert_eq!(index.search("stale_text", 5, None).unwrap().hits.len(), 2);
        assert_eq!(index.stats().unwrap().archives, 1);
        assert_eq!(update(&mut index).archives_indexed, 1);
        assert!(index.search("stale_text", 5, None).unwrap().hits.is_empty());
        // With it, it is complete.
        write(&mut index, part(true, "one.txt"));
        write(&mut index, part(false, "two.txt"));
        assert_eq!(update(&mut index).archives_indexed, 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn changed_at_moves_only_when_archives_change() {
        let dir = std::env::temp_dir().join(format!("openrage-index-changed-{}", std::process::id()));
        let game = dir.join("game");
        fs::create_dir_all(game.join("pc/data/maps")).unwrap();
        let archive = game.join("pc/data/maps/area.img");
        fs::write(&archive, img(&[("a.ide", b"objs\nend\n", false)])).unwrap();
        let mut index = Index::open(&dir.join("index.sqlite")).unwrap();
        let mut update = || index.update(&game, None, &AtomicBool::new(false), &mut |_| {}).unwrap();
        update();
        let first = Index::open(&dir.join("index.sqlite")).unwrap().changed_at().unwrap();
        assert!(first.is_some());
        // Simulate an older index: rewrite the stamp, then check without changes.
        let stamp = |index: &Index, value: &str| schema::set_meta(&index.conn, "changed_at", value).unwrap();
        stamp(&Index::open(&dir.join("index.sqlite")).unwrap(), "100");
        let mut index = Index::open(&dir.join("index.sqlite")).unwrap();
        index.update(&game, None, &AtomicBool::new(false), &mut |_| {}).unwrap();
        assert_eq!(index.changed_at().unwrap(), Some(100), "an unchanged game keeps the stamp");
        // A changed archive (here: larger) moves it.
        let big = vec![b'x'; 5000];
        fs::write(&archive, img(&[("b.ide", b"objs\nend\n", false), ("c.dat", &big, false)])).unwrap();
        let stats = index.update(&game, None, &AtomicBool::new(false), &mut |_| {}).unwrap();
        assert_eq!(stats.archives_indexed, 1);
        assert!(index.changed_at().unwrap() > Some(100));
        let _ = fs::remove_dir_all(&dir);
    }
}
