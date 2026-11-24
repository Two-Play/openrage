//! GTA VI archives (RPF9): the stored index of their recovery view, and
//! extraction.
//!
//! The recovery view is for archives whose own table cannot be read. Finding the files of an RPF9 archive means reading all of it (see
//! [`rage_rpf::rpf9`]), which takes minutes for the large ones. The result
//! is kept in the cache directory, one file per archive, and used as long as
//! the archive's size and modification time stay the same.

use std::collections::HashMap;
use std::fs;
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result};
use rage_rpf::rpf9::{self, Entry};
use rage_rpf::{Rpf9, RpfError, vfs};

use crate::extract::ExtractSummary;

/// Start of an index file; the digit changes with the format.
const INDEX_MAGIC: &[u8; 8] = b"ORP9IDX2";

/// Where the index of an archive is stored:
/// `<cache dir>/openrage/rpf9-<hash of the archive path>.idx`.
pub fn index_path(archive: &Path) -> Option<PathBuf> {
    crate::paths::game_cache(archive, "rpf9", "idx")
}

/// Size and modification time (seconds) of the archive: an index made for
/// other values is out of date.
fn identity(archive: &Path) -> Option<[u8; 16]> {
    let meta = fs::metadata(archive).ok()?;
    let modified = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_secs();
    let mut id = [0u8; 16];
    id[..8].copy_from_slice(&meta.len().to_le_bytes());
    id[8..].copy_from_slice(&modified.to_le_bytes());
    Some(id)
}

/// The stored index of an archive, if there is one and it is up to date.
pub fn load_index(archive: &Path) -> Option<Vec<Entry>> {
    load_index_from(&index_path(archive)?, archive)
}

fn load_index_from(index: &Path, archive: &Path) -> Option<Vec<Entry>> {
    let data = fs::read(index).ok()?;
    let rest = data.strip_prefix(INDEX_MAGIC)?.strip_prefix(&identity(archive)?)?;
    let entries = rpf9::decode_index(rest)?;
    let mut file = fs::File::open(archive).ok()?;
    let size = file.metadata().ok()?.len();
    let mut bytes = [0u8; rpf9::HEADER_LEN];
    file.read_exact(&mut bytes).ok()?;
    let header = rpf9::Header::parse(&bytes)?;
    entries
        .iter()
        .all(|entry| entry.offset >= u64::from(header.data_offset) && entry.offset + entry.stored <= size)
        .then_some(entries)
}

/// Stores the index of an archive.
pub fn store_index(archive: &Path, entries: &[Entry]) -> Result<()> {
    store_index_to(&index_path(archive).context("no cache directory")?, archive, entries)
}

fn store_index_to(index: &Path, archive: &Path, entries: &[Entry]) -> Result<()> {
    let id = identity(archive).context("archive not readable")?;
    if let Some(dir) = index.parent() {
        fs::create_dir_all(dir)?;
    }
    // Written beside and renamed: a stopped write leaves no half index.
    let partial = index.with_extension("idx.part");
    let mut data = Vec::from(INDEX_MAGIC);
    data.extend_from_slice(&id);
    data.extend_from_slice(&rpf9::encode_index(entries));
    fs::write(&partial, data)?;
    fs::rename(&partial, index)?;
    Ok(())
}

/// Start of a names file; the digit changes with the format.
const NAMES_MAGIC: &[u8; 8] = b"ORP9NAM2";
/// How much of a resource is read to find its name.
const NAME_PREFIX: usize = 256 << 10;

/// Where the names of an archive's models are stored, beside its index.
pub fn names_path(archive: &Path) -> Option<PathBuf> {
    crate::paths::game_cache(archive, "rpf9", "names")
}

/// The stored names of an archive, if there are any and they are up to date.
pub fn load_names(archive: &Path) -> Option<rpf9::Names> {
    let data = fs::read(names_path(archive)?).ok()?;
    let mut rest = data.strip_prefix(NAMES_MAGIC)?.strip_prefix(&identity(archive)?)?;
    let mut found = Vec::new();
    while !rest.is_empty() {
        let offset = u64::from_le_bytes(rest.get(..8)?.try_into().ok()?);
        let len = usize::from(u16::from_le_bytes(rest.get(8..10)?.try_into().ok()?));
        found.push((offset, String::from_utf8(rest.get(10..10 + len)?.to_vec()).ok()?));
        rest = &rest[10 + len..];
    }
    Some(rpf9::Names::new(found))
}

/// Stores the names of an archive's models.
pub fn store_names(archive: &Path, names: &[(u64, String)]) -> Result<()> {
    let path = names_path(archive).context("no cache directory")?;
    let id = identity(archive).context("archive not readable")?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut data = Vec::from(NAMES_MAGIC);
    data.extend_from_slice(&id);
    for (offset, name) in names {
        data.extend_from_slice(&offset.to_le_bytes());
        data.extend_from_slice(&(name.len().min(usize::from(u16::MAX)) as u16).to_le_bytes());
        data.extend_from_slice(&name.as_bytes()[..name.len().min(usize::from(u16::MAX))]);
    }
    let partial = path.with_extension("names.part");
    fs::write(&partial, data)?;
    fs::rename(&partial, &path)?;
    Ok(())
}

/// Reads the start of every resource of an archive and returns the names
/// of those that are models; a dictionary of models is named by its offset. `progress` gets the resources read and their
/// number; setting `cancel` stops with what was found so far (`None`).
pub fn scan_names(
    rpf: &Rpf9,
    entries: &[Entry],
    cancel: &AtomicBool,
    progress: &(dyn Fn(u64, u64) + Sync),
) -> Option<Vec<(u64, String)>> {
    use rayon::prelude::*;
    let resources: Vec<&Entry> = entries.iter().filter(|e| e.is_resource()).collect();
    let (done, total) = (std::sync::atomic::AtomicU64::new(0), resources.len() as u64);
    let found: Vec<(u64, String)> = resources
        .par_iter()
        .filter_map(|e| {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            if n.is_multiple_of(256) || n == total {
                progress(n, total);
            }
            let start = rpf.read_start(e, NAME_PREFIX).ok()?;
            // A dictionary of models has no name of its own.
            let name = match rage_formats::drawable6::root(&start)? {
                rage_formats::drawable6::Root::Dictionary => format!("dictionary_{:012x}.#dd", e.offset),
                _ => rage_formats::drawable6::name(&start)?,
            };
            Some((e.offset, name))
        })
        .collect();
    (!cancel.load(Ordering::Relaxed)).then_some(found)
}

/// What [`register`] reports to: the archive's file name with the bytes read
/// and to read, or `None` when the scan is over.
pub type Report = dyn Fn(Option<(&str, u64, u64)>) + Send + Sync;

/// Makes [`Rpf9::entries`] use the stored index, and scan and store it when
/// there is none. `report` gets the archive's file name with the bytes read
/// and to read while scanning, and `None` when the scan is over.
pub fn register(report: Arc<Report>) {
    let _ = REPORT.set(Arc::clone(&report));
    rpf9::set_indexer(Arc::new(move |rpf: &Rpf9| index(rpf, &AtomicBool::new(false), &*report)));
    rpf9::set_namer(Arc::new(|rpf: &Rpf9| load_names(rpf.path())));
}

static REPORT: OnceLock<Arc<Report>> = OnceLock::new();

/// One lock per archive: two threads that want the same index (the window
/// and the background run) scan it once; the second finds it stored.
fn scan_lock(archive: &Path) -> Arc<Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();
    let mut locks = LOCKS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    Arc::clone(locks.entry(archive.to_path_buf()).or_default())
}

/// The entries of an archive: from the stored index, or scanned and stored.
fn index(rpf: &Rpf9, cancel: &AtomicBool, report: &Report) -> Result<Vec<Entry>, RpfError> {
    let lock = scan_lock(rpf.path());
    let _scanning = lock.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(entries) = load_index(rpf.path()) {
        return Ok(entries);
    }
    let name = rpf.path().file_name().unwrap_or_default().to_string_lossy().into_owned();
    report(Some((&name, 0, rpf.data_len())));
    let scanned = rpf.scan(cancel, &|done, total| report(Some((&name, done, total))));
    report(None);
    let entries = scanned?;
    // The index only saves time: failing to store it is no error.
    let _ = store_index(rpf.path(), &entries);
    Ok(entries)
}

/// Finds and stores the names of an archive's models, unless they are
/// stored already, and gives them to `rpf`.
pub fn ensure_names(rpf: &Rpf9, entries: &[Entry], cancel: &AtomicBool, report: &Report) {
    if rpf.is_native() {
        return;
    }
    let lock = scan_lock(rpf.path());
    let _scanning = lock.lock().unwrap_or_else(|e| e.into_inner());
    if load_names(rpf.path()).is_some() {
        return;
    }
    let name = format!("{} (model names)", rpf.path().file_name().unwrap_or_default().to_string_lossy());
    report(Some((&name, 0, 1)));
    let found = scan_names(rpf, entries, cancel, &|done, total| report(Some((&name, done, total))));
    report(None);
    if let Some(found) = found {
        // The names only help reading the list: failing to store them is no error.
        let _ = store_names(rpf.path(), &found);
        rpf.set_names(rpf9::Names::new(found));
    }
}

/// The names for the archive a command works on: read from the models once
/// if they are not stored yet. For programs without a background run.
pub fn names_now(rpf: &Rpf9) {
    if let (Ok(entries), Some(report)) = (rpf.entries(), REPORT.get()) {
        ensure_names(rpf, &entries, &AtomicBool::new(false), &**report);
    }
}

/// The run started by [`index_in_background`], to stop it.
static BACKGROUND: Mutex<Option<Arc<AtomicBool>>> = Mutex::new(None);

/// Stops the background run, if one is going.
pub fn stop_background() {
    if let Some(stop) = BACKGROUND.lock().unwrap_or_else(|e| e.into_inner()).take() {
        stop.store(true, Ordering::Relaxed);
    }
}

/// Indexes the archives below `game_dir` that have no stored index yet, one
/// after the other (smallest first), on a thread of its own. Opening an
/// archive meanwhile waits for its index or makes it. A run started before
/// is stopped.
pub fn index_in_background(game_dir: &Path) {
    stop_background();
    let stop = Arc::new(AtomicBool::new(false));
    *BACKGROUND.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::clone(&stop));
    let game_dir = game_dir.to_path_buf();
    std::thread::spawn(move || {
        let mut archives = Vec::new();
        crate::detect::collect_archives(&game_dir, &["rpf"], &mut archives);
        archives.retain(|p| {
            Rpf9::file_is_rpf9(p)
                && (load_index(p).is_none() || names_path(p).is_none_or(|n| !n.is_file() || load_names(p).is_none()))
        });
        archives.sort_by_key(|p| fs::metadata(p).map_or(0, |m| m.len()));
        let quiet: &Report = &|_| {};
        let report: &Report = REPORT.get().map_or(quiet, |r| &**r);
        for archive in archives {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            let Ok(rpf) = Rpf9::open(&archive) else { continue };
            if rpf.is_native() {
                continue;
            }
            let Ok(entries) = index(&rpf, &stop, report) else { continue };
            ensure_names(&rpf, &entries, &stop, report);
        }
    });
}

/// The entries of an archive if they are at hand without scanning it.
pub fn known_entries(rpf: &Rpf9) -> Option<Arc<Vec<Entry>>> {
    rpf.loaded().or_else(|| if rpf.is_native() { rpf.entries().ok() } else { load_index(rpf.path()).map(Arc::new) })
}

/// Totals of an archive's entries.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Coverage {
    pub files: u64,
    /// Bytes of the archive the files take.
    pub stored: u64,
    /// Bytes the files unpack to.
    pub unpacked: u64,
    /// Bytes of the data area that belong to no file: padding, and data that
    /// is neither zlib, Bink 2 nor an audio container.
    pub unread: u64,
}

pub fn coverage(rpf: &Rpf9, entries: &[Entry]) -> Coverage {
    let stored: u64 = entries.iter().map(|e| e.stored).sum();
    Coverage {
        files: entries.len() as u64,
        stored,
        unpacked: entries.iter().map(|e| e.size).sum(),
        unread: rpf.data_len().saturating_sub(stored),
    }
}

/// Extracts from a GTA VI archive into `out`: one file (`file`), or
/// everything below folder `dir`. Files are written unpacked, straight to
/// disk. Unreadable files are listed in `failures` and skipped.
pub fn extract(
    rpf: &Rpf9,
    dir: &str,
    file: Option<usize>,
    out: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&ExtractSummary, &str),
) -> Result<ExtractSummary> {
    let entries = rpf.entries()?;
    let mut summary = ExtractSummary { target: out.to_path_buf(), ..Default::default() };
    let mut write = |i: usize, rel: &str, s: &mut ExtractSummary| -> Result<()> {
        let target = out.join(vfs::safe_relative_path(rel).with_context(|| format!("unsafe path {rel}"))?);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut writer = BufWriter::new(fs::File::create(&target)?);
        match rpf.copy_to(&entries[i], &mut writer).and_then(|n| Ok(writer.flush().map(|()| n)?)) {
            Ok(bytes) => {
                s.files += 1;
                s.bytes += bytes;
            }
            Err(e) => {
                drop(writer);
                let _ = fs::remove_file(&target);
                s.failures.push(e.to_string());
            }
        }
        progress(s, rel);
        Ok(())
    };
    if let Some(i) = file {
        let name = rpf.entry_name(&entries[i]);
        let base = name.rsplit('/').next().unwrap_or(&name).to_owned();
        write(i, &base, &mut summary)?;
        summary.target = out.join(base);
        return Ok(summary);
    }
    // Everything below `dir`, with its path from there on.
    let prefix = if dir.is_empty() { String::new() } else { format!("{}/", dir.to_ascii_lowercase()) };
    for (i, e) in entries.iter().enumerate() {
        let name = rpf.entry_name(e);
        let Some(rel) = name.get(..prefix.len()).filter(|s| s.eq_ignore_ascii_case(&prefix)).map(|s| &name[s.len()..])
        else {
            continue;
        };
        if cancel.load(Ordering::Relaxed) {
            summary.cancelled = true;
            break;
        }
        write(i, rel, &mut summary)?;
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::ZlibEncoder;

    fn archive(dir: &Path) -> PathBuf {
        let mut data = vec![0u8; 0x40];
        data[..4].copy_from_slice(b"9FPR");
        data[4..8].copy_from_slice(&0x40u32.to_le_bytes());
        for text in [&b"<?xml version=\"1.0\"?><a/>"[..], b"PSIN\x00\x01binary"] {
            let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
            enc.write_all(text).unwrap();
            data.extend(enc.finish().unwrap());
            data.resize(data.len().next_multiple_of(16), 0);
        }
        let path = dir.join("test.rpf");
        fs::write(&path, data).unwrap();
        path
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("openrage-app-rpf9-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_index_is_stored_and_dropped_when_the_archive_changes() {
        let dir = temp("index");
        let path = archive(&dir);
        let rpf = Rpf9::open(&path).unwrap();
        let entries = rpf.scan(&AtomicBool::new(false), &|_, _| {}).unwrap();
        assert_eq!(entries.len(), 2);
        let index = dir.join("test.idx");
        assert_eq!(load_index_from(&index, &path), None);
        store_index_to(&index, &path, &entries).unwrap();
        assert_eq!(load_index_from(&index, &path).as_deref(), Some(&entries[..]));
        let mut outside = entries.clone();
        outside[1].offset = fs::metadata(&path).unwrap().len();
        store_index_to(&index, &path, &outside).unwrap();
        assert_eq!(load_index_from(&index, &path), None);
        store_index_to(&index, &path, &entries).unwrap();
        let c = coverage(&rpf, &entries);
        assert_eq!((c.files, c.unpacked), (2, 25 + 12));
        assert_eq!(c.stored + c.unread, rpf.data_len());

        let mut longer = fs::read(&path).unwrap();
        longer.extend([0u8; 16]);
        fs::write(&path, longer).unwrap();
        assert_eq!(load_index_from(&index, &path), None);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn extracts_a_folder_and_a_single_file() {
        let dir = temp("extract");
        let path = archive(&dir);
        let rpf = Rpf9::open(&path).unwrap();
        rpf.set_entries(rpf.scan(&AtomicBool::new(false), &|_, _| {}).unwrap());
        let quiet = AtomicBool::new(false);

        let all = extract(&rpf, "", None, &dir.join("all"), &quiet, &mut |_, _| {}).unwrap();
        assert_eq!((all.files, all.failures.len()), (2, 0));
        assert_eq!(fs::read(dir.join("all/xml/000000000040.xml")).unwrap(), b"<?xml version=\"1.0\"?><a/>");

        let psin = extract(&rpf, "PSIN", None, &dir.join("psin"), &quiet, &mut |_, _| {}).unwrap();
        assert_eq!(psin.files, 1);
        assert!(
            fs::read_dir(dir.join("psin"))
                .unwrap()
                .flatten()
                .all(|f| f.file_name().to_string_lossy().ends_with(".psin"))
        );

        let one = extract(&rpf, "", Some(0), &dir.join("one"), &quiet, &mut |_, _| {}).unwrap();
        assert_eq!(one.target, dir.join("one/000000000040.xml"));
        assert!(one.target.is_file());
        fs::remove_dir_all(dir).unwrap();
    }
}
