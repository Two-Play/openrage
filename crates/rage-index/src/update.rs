//! Incremental index updates.
//!
//! Top-level archives are the unit of change: an archive whose size or
//! modification time differs from the stored values is dropped and re-read,
//! including all archives nested inside it. Loose files are re-listed on
//! every update (cheap). Archives are read in parallel; a single writer
//! commits each archive in its own transaction, so a cancelled update keeps
//! everything finished so far.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use rage_core::hash::joaat;
use rage_crypto::GtaVKeys;
use rage_rpf::{EntryKind, Gta4Archive, Rpf7, vfs};
use rayon::prelude::*;
use rusqlite::params;

use crate::content::{content_for, is_indexable};
use crate::{EntryTag, Index, IndexError, schema};

#[derive(Debug, Clone, Default)]
pub struct Progress {
    pub archives_done: usize,
    pub archives_total: usize,
    pub entries: u64,
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
}

/// Largest loose file whose text is indexed (GTA IV's text tables are ~6 MB).
const MAX_LOOSE_TEXT: u64 = 16 << 20;

struct Record {
    vpath: String,
    name: String,
    kind: EntryTag,
    size: u64,
    body: Option<String>,
}

struct Batch {
    rel: String,
    size: i64,
    mtime: i64,
    records: Vec<Record>,
    dictionary: HashSet<String>,
    errors: usize,
}

fn stem(name: &str) -> &str {
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
            let Ok(meta) = entry.metadata() else { continue };
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == ".openrage" {
                continue; // OpenRAGE's journal inside the mods folder
            }
            let rel = if rel_dir.is_empty() { name.clone() } else { format!("{rel_dir}/{name}") };
            let lower = name.to_ascii_lowercase();
            let gta4 = !meta.is_dir()
                && (lower.ends_with(".img") || lower.ends_with(".rpf"))
                && Gta4Archive::file_is_gta4(&entry.path());
            let is_rpf = !meta.is_dir() && (lower.ends_with(".rpf") || gta4);
            let kind = if meta.is_dir() {
                EntryTag::Dir
            } else if is_rpf {
                EntryTag::Archive
            } else {
                EntryTag::File
            };
            let mut body = None;
            if kind == EntryTag::File
                && is_indexable(&name)
                && meta.len() <= MAX_LOOSE_TEXT
                && let Ok(data) = fs::read(entry.path())
                && let Some((text, words)) = content_for(&name, &data)
            {
                body = Some(text);
                dictionary.extend(words);
            }
            records.push(Record { vpath: rel.clone(), name, kind, size: meta.len(), body });
            if meta.is_dir() {
                stack.push((entry.path(), rel));
            } else if is_rpf {
                archives.push(DiskArchive {
                    rel,
                    path: entry.path(),
                    size: meta.len() as i64,
                    mtime: mtime_of(&meta),
                    gta4,
                });
            }
        }
    }
    (records, archives, dictionary)
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
        batch.records.push(Record { vpath: path.to_owned(), name: e.name.clone(), kind, size: e.size(), body });
    });
    batch
}

/// Reads one top-level archive (and everything nested in it) into a batch.
fn read_archive(archive: &DiskArchive, keys: Option<Arc<GtaVKeys>>) -> Batch {
    let mut batch = Batch {
        rel: archive.rel.clone(),
        size: archive.size,
        mtime: archive.mtime,
        records: Vec::new(),
        dictionary: HashSet::new(),
        errors: 0,
    };
    if archive.gta4 {
        return read_gta4_archive(archive, batch);
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
            batch.records.push(Record { vpath: path.to_owned(), name: e.name.clone(), kind, size: e.size(), body });
        },
        &mut |_| errors.set(errors.get() + 1),
    );
    batch.errors += errors.get();
    batch
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
        let stored: HashMap<String, (i64, i64, i64)> = {
            let mut stmt = self.conn.prepare("SELECT path, id, size, mtime FROM sources")?;
            stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, (r.get(1)?, r.get(2)?, r.get(3)?))))?
                .collect::<Result<_, _>>()?
        };
        let on_disk: HashMap<&str, &DiskArchive> = archives.iter().map(|a| (a.rel.as_str(), a)).collect();
        let mut stale: Vec<i64> = Vec::new();
        for (path, &(id, size, mtime)) in &stored {
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
        let mut todo: Vec<&DiskArchive> = archives
            .iter()
            .filter(|a| stored.get(&a.rel).is_none_or(|&(_, size, mtime)| size != a.size || mtime != a.mtime))
            .collect();
        todo.sort_by_key(|a| std::cmp::Reverse(a.size)); // big ones first for better parallelism

        {
            let tx = self.conn.transaction()?;
            for id in &stale {
                tx.execute("DELETE FROM content WHERE rowid IN (SELECT id FROM entries WHERE source = ?1)", [id])?;
                tx.execute("DELETE FROM entries WHERE source = ?1", [id])?;
                tx.execute("DELETE FROM sources WHERE id = ?1", [id])?;
            }
            let loose_batch =
                Batch { rel: String::new(), size: 0, mtime: 0, records: loose, dictionary: loose_words, errors: 0 };
            write_batch(&tx, &loose_batch)?;
            tx.commit()?;
        }

        let total = todo.len();
        let mut done = 0usize;
        let (sender, receiver) = mpsc::sync_channel::<Batch>(4);
        let conn = &mut self.conn;
        let result: Result<(), IndexError> = std::thread::scope(|scope| {
            scope.spawn(move || {
                todo.par_iter().for_each_with(sender, |s, archive| {
                    if !cancel.load(Ordering::Relaxed) {
                        let _ = s.send(read_archive(archive, keys.clone()));
                    }
                });
            });
            for batch in receiver {
                let tx = conn.transaction()?;
                let added = write_batch(&tx, &batch)?;
                tx.commit()?;
                done += 1;
                stats.archives_indexed += 1;
                stats.entries_added += added;
                stats.read_errors += batch.errors;
                progress(&Progress {
                    archives_done: done,
                    archives_total: total,
                    entries: stats.entries_added,
                    current: batch.rel,
                });
            }
            Ok(())
        });
        result?;

        stats.cancelled = cancel.load(Ordering::Relaxed) && done < total;
        if !stats.cancelled {
            let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
            schema::set_meta(&self.conn, "updated_at", &now.to_string())?;
            self.conn.execute_batch("PRAGMA optimize;")?;
        }
        stats.seconds = started.elapsed().as_secs_f32();
        Ok(stats)
    }
}

fn write_batch(tx: &rusqlite::Transaction<'_>, batch: &Batch) -> Result<u64, IndexError> {
    tx.execute(
        "INSERT INTO sources(path, size, mtime) VALUES(?1, ?2, ?3)",
        params![batch.rel, batch.size, batch.mtime],
    )?;
    let source = tx.last_insert_rowid();
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
            joaat(&r.name) as i64,
            joaat(stem(&r.name)) as i64,
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
}
