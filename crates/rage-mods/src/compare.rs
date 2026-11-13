//! Finding what a mod actually changed.
//!
//! Entries are matched by name (case-insensitive). Two entries are considered
//! equal when their metadata matches and — for small entries, or always in
//! [`CompareMode::Deep`] — their decoded contents are identical. Raw bytes are
//! compared first; when they differ (e.g. NG-encrypted original vs. an
//! unencrypted OpenIV copy) the decoded data decides.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use rage_crypto::GtaVKeys;
use rage_rpf::vfs;
use rage_rpf::{EntryId, EntryKind, RawEntry, Rpf7};

use crate::overlay::{ModFile, ModFileKind, mods_dir, scan};

/// Entries up to this stored size are always compared by content.
const SMALL_ENTRY: u64 = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareMode {
    /// Metadata, plus content for small entries. Fast, but a large entry whose
    /// sizes did not change is assumed unchanged.
    Quick,
    /// Decode and compare every entry.
    Deep,
}

/// Status of an entry in a modded folder relative to the original.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Unchanged,
    Added,
    Changed,
    /// Only used for [`RemovedEntry`]: present in the original, missing in the mod.
    Removed,
}

#[derive(Debug, Clone)]
pub struct RemovedEntry {
    pub name: String,
    /// Folder or nested archive.
    pub is_dir: bool,
    pub is_archive: bool,
    pub size: u64,
}

/// Comparison of one folder of a modded archive with the original folder.
#[derive(Debug, Clone, Default)]
pub struct FolderDiff {
    /// Mark per child of the modded folder, keyed by lowercase name.
    pub marks: HashMap<String, Mark>,
    /// Children of the original folder that the mod no longer has.
    pub removed: Vec<RemovedEntry>,
}

fn same_raw(a: &RawEntry, b: &RawEntry) -> bool {
    match (a, b) {
        (RawEntry::Binary { data: x, .. }, RawEntry::Binary { data: y, .. }) => x == y,
        // The first 16 bytes are archive bookkeeping, not content.
        (RawEntry::Resource { data: x, .. }, RawEntry::Resource { data: y, .. }) => x.get(16..) == y.get(16..),
        _ => false,
    }
}

/// Decoded contents used for comparison (resources: inflated pages).
fn decoded(rpf: &Rpf7, id: EntryId) -> Option<Vec<u8>> {
    match rpf.entry(id).kind {
        EntryKind::Resource { .. } => rpf.read_resource_pages(id).ok(),
        _ => rpf.read(id).ok(),
    }
}

/// Whether two non-directory entries hold the same data.
fn files_equal(m: &Rpf7, mid: EntryId, b: &Rpf7, bid: EntryId, mode: CompareMode) -> bool {
    let (me, be) = (m.entry(mid), b.entry(bid));
    match (&me.kind, &be.kind) {
        (EntryKind::Binary { size: s1, .. }, EntryKind::Binary { size: s2, .. }) if s1 != s2 => return false,
        (
            EntryKind::Resource { system_flags: f1, graphics_flags: g1, .. },
            EntryKind::Resource { system_flags: f2, graphics_flags: g2, .. },
        ) if (f1, g1) != (f2, g2) => return false,
        (EntryKind::Binary { .. }, EntryKind::Binary { .. })
        | (EntryKind::Resource { .. }, EntryKind::Resource { .. }) => {}
        _ => return false,
    }
    let stored = me.size().max(be.size());
    if mode == CompareMode::Quick && stored > SMALL_ENTRY {
        // Large entry with identical metadata: assume unchanged in quick mode,
        // unless the stored sizes give it away.
        return match (&me.kind, &be.kind) {
            (EntryKind::Binary { stored_size: a, .. }, EntryKind::Binary { stored_size: b, .. }) => a == b,
            (EntryKind::Resource { size: a, .. }, EntryKind::Resource { size: b, .. }) => a == b,
            _ => false,
        };
    }
    if let (Ok(x), Ok(y)) = (m.read_raw_entry(mid), b.read_raw_entry(bid))
        && same_raw(&x, &y)
    {
        return true;
    }
    matches!((decoded(m, mid), decoded(b, bid)), (Some(x), Some(y)) if x == y)
}

/// Whether two subtrees (directories or nested archives) are identical.
fn subtree_equal(m: &Rpf7, mdir: EntryId, b: &Rpf7, bdir: EntryId, mode: CompareMode) -> bool {
    let mut changes = Vec::new();
    diff_dirs(m, mdir, b, bdir, "", mode, &mut changes, true);
    changes.is_empty()
}

fn entry_equal(m: &Rpf7, mid: EntryId, b: &Rpf7, bid: EntryId, mode: CompareMode) -> bool {
    let (me, be) = (m.entry(mid), b.entry(bid));
    match (me.is_dir(), be.is_dir(), me.is_archive(), be.is_archive()) {
        (true, true, _, _) => subtree_equal(m, mid, b, bid, mode),
        (false, false, true, true) => match (m.open_child(mid), b.open_child(bid)) {
            (Ok(mc), Ok(bc)) => subtree_equal(&mc, Rpf7::ROOT, &bc, Rpf7::ROOT, mode),
            _ => files_equal(m, mid, b, bid, mode),
        },
        (false, false, false, false) => files_equal(m, mid, b, bid, mode),
        _ => false,
    }
}

/// Compares the children of one folder (for marking a listing).
pub fn compare_folders(m: &Rpf7, mdir: EntryId, b: &Rpf7, bdir: EntryId, mode: CompareMode) -> FolderDiff {
    let base_children: HashMap<String, EntryId> =
        b.children(bdir).map(|id| (b.entry(id).name.to_ascii_lowercase(), id)).collect();
    let mut diff = FolderDiff::default();
    for mid in m.children(mdir) {
        let key = m.entry(mid).name.to_ascii_lowercase();
        let mark = match base_children.get(&key) {
            None => Mark::Added,
            Some(&bid) if entry_equal(m, mid, b, bid, mode) => Mark::Unchanged,
            Some(_) => Mark::Changed,
        };
        diff.marks.insert(key, mark);
    }
    for (key, &bid) in &base_children {
        if !diff.marks.contains_key(key) {
            let e = b.entry(bid);
            diff.removed.push(RemovedEntry {
                name: e.name.clone(),
                is_dir: e.is_dir() || e.is_archive(),
                is_archive: e.is_archive(),
                size: e.size(),
            });
        }
    }
    diff.removed.sort_by_key(|x| x.name.to_lowercase());
    diff
}

/// Like [`compare_folders`] for GTA IV archives (`.img`, RPF2/RPF3). Entries
/// at the same place with the same size are taken as unchanged (an edited
/// copy keeps untouched entries where they were); others are compared by
/// their stored bytes.
pub fn compare_gta4_folders(
    m: &rage_rpf::Gta4Archive,
    mdir: rage_rpf::Gta4EntryId,
    b: &rage_rpf::Gta4Archive,
    bdir: rage_rpf::Gta4EntryId,
) -> FolderDiff {
    use rage_rpf::Gta4EntryKind as K;
    let place = |k: &K| match k {
        K::Dir { .. } => None,
        K::File { offset, stored, .. } | K::Resource { offset, stored, .. } => Some((*offset, *stored)),
    };
    let base_children: HashMap<String, rage_rpf::Gta4EntryId> =
        b.children(bdir).map(|id| (b.entry(id).name.to_ascii_lowercase(), id)).collect();
    let mut diff = FolderDiff::default();
    for mid in m.children(mdir) {
        let me = m.entry(mid);
        let key = me.name.to_ascii_lowercase();
        let mark = match base_children.get(&key) {
            None => Mark::Added,
            Some(&bid) => {
                let be = b.entry(bid);
                let same = match (place(&me.kind), place(&be.kind)) {
                    (None, None) => true,
                    (Some(x), Some(y)) if x == y => true,
                    (Some((_, xs)), Some((_, ys))) if xs == ys => {
                        matches!((m.read(mid), b.read(bid)), (Ok(x), Ok(y)) if x == y)
                    }
                    _ => false,
                };
                if same { Mark::Unchanged } else { Mark::Changed }
            }
        };
        diff.marks.insert(key, mark);
    }
    for (key, &bid) in &base_children {
        if !diff.marks.contains_key(key) {
            let e = b.entry(bid);
            diff.removed.push(RemovedEntry {
                name: e.name.clone(),
                is_dir: e.is_dir(),
                is_archive: false,
                size: e.size(),
            });
        }
    }
    diff.removed.sort_by_key(|x| x.name.to_lowercase());
    diff
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    /// A whole file that only exists in mods.
    NewFile,
    /// A replaced top-level file that is not an archive (or could not be opened).
    ReplacedFile,
    /// A replaced file whose content is identical to the original (a redundant copy).
    IdenticalCopy,
    Added,
    Removed,
    Modified,
}

impl ChangeKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::NewFile => "new file",
            Self::ReplacedFile => "replaced",
            Self::IdenticalCopy => "identical copy",
            Self::Added => "added",
            Self::Removed => "removed",
            Self::Modified => "modified",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Change {
    /// Virtual path relative to the game (as the game sees it).
    pub vpath: String,
    pub kind: ChangeKind,
    pub is_dir: bool,
    pub size: u64,
}

/// Recursive diff of two archive folders. Added/removed folders are reported
/// once, not per entry. With `stop_at_first`, returns after the first change.
#[allow(clippy::too_many_arguments)]
fn diff_dirs(
    m: &Rpf7,
    mdir: EntryId,
    b: &Rpf7,
    bdir: EntryId,
    prefix: &str,
    mode: CompareMode,
    out: &mut Vec<Change>,
    stop_at_first: bool,
) {
    let join = |name: &str| if prefix.is_empty() { name.to_owned() } else { format!("{prefix}/{name}") };
    let base_children: HashMap<String, EntryId> =
        b.children(bdir).map(|id| (b.entry(id).name.to_ascii_lowercase(), id)).collect();
    let mut matched = std::collections::HashSet::new();
    for mid in m.children(mdir) {
        if stop_at_first && !out.is_empty() {
            return;
        }
        let me = m.entry(mid);
        let key = me.name.to_ascii_lowercase();
        let path = join(&me.name);
        let container = me.is_dir() || me.is_archive();
        let Some(&bid) = base_children.get(&key) else {
            out.push(Change { vpath: path, kind: ChangeKind::Added, is_dir: container, size: me.size() });
            continue;
        };
        matched.insert(key);
        let be = b.entry(bid);
        match (me.is_dir(), be.is_dir(), me.is_archive(), be.is_archive()) {
            (true, true, _, _) => diff_dirs(m, mid, b, bid, &path, mode, out, stop_at_first),
            (false, false, true, true) => match (m.open_child(mid), b.open_child(bid)) {
                (Ok(mc), Ok(bc)) => diff_dirs(&mc, Rpf7::ROOT, &bc, Rpf7::ROOT, &path, mode, out, stop_at_first),
                _ if !files_equal(m, mid, b, bid, mode) => {
                    out.push(Change { vpath: path, kind: ChangeKind::Modified, is_dir: false, size: me.size() })
                }
                _ => {}
            },
            (false, false, false, false) => {
                if !files_equal(m, mid, b, bid, mode) {
                    out.push(Change { vpath: path, kind: ChangeKind::Modified, is_dir: false, size: me.size() });
                }
            }
            _ => out.push(Change { vpath: path, kind: ChangeKind::Modified, is_dir: container, size: me.size() }),
        }
    }
    for (key, &bid) in &base_children {
        if stop_at_first && !out.is_empty() {
            return;
        }
        if !matched.contains(key) {
            let be = b.entry(bid);
            out.push(Change {
                vpath: join(&be.name),
                kind: ChangeKind::Removed,
                is_dir: be.is_dir() || be.is_archive(),
                size: be.size(),
            });
        }
    }
}

fn files_identical(a: &Path, b: &Path) -> bool {
    use std::io::Read;
    let (Ok(fa), Ok(fb)) = (std::fs::File::open(a), std::fs::File::open(b)) else { return false };
    if fa.metadata().map(|m| m.len()).ok() != fb.metadata().map(|m| m.len()).ok() {
        return false;
    }
    let (mut ra, mut rb) = (std::io::BufReader::new(fa), std::io::BufReader::new(fb));
    let (mut x, mut y) = (vec![0u8; 1 << 16], vec![0u8; 1 << 16]);
    loop {
        let n = match ra.read(&mut x) {
            Ok(n) => n,
            Err(_) => return false,
        };
        if n == 0 {
            return true;
        }
        if rb.read_exact(&mut y[..n]).is_err() || x[..n] != y[..n] {
            return false;
        }
    }
}

/// Every change of every file in the mods folder, relative to the game.
///
/// `progress` is called before each mod file with (index, total, path).
pub fn list_changes(
    game_dir: &Path,
    keys: Option<Arc<GtaVKeys>>,
    mode: CompareMode,
    progress: &mut dyn FnMut(usize, usize, &str),
) -> Vec<Change> {
    let Some(mods) = mods_dir(game_dir) else { return Vec::new() };
    let files: Vec<ModFile> = scan(game_dir);
    let mut out = Vec::new();
    for (i, f) in files.iter().enumerate() {
        progress(i, files.len(), &f.rel);
        if f.kind == ModFileKind::New {
            out.push(Change { vpath: f.rel.clone(), kind: ChangeKind::NewFile, is_dir: false, size: f.size });
            continue;
        }
        let (Some(mod_path), Some(base_path)) =
            (vfs::find_case_insensitive(&mods, &f.rel), vfs::find_case_insensitive(game_dir, &f.rel))
        else {
            continue;
        };
        if files_identical(&mod_path, &base_path) {
            out.push(Change { vpath: f.rel.clone(), kind: ChangeKind::IdenticalCopy, is_dir: false, size: f.size });
            continue;
        }
        // GTA IV archives: one level of entries, compared by name.
        if f.is_archive && rage_rpf::Gta4Archive::file_is_gta4(&base_path) {
            let pair = rage_rpf::Gta4Archive::open_default(&mod_path)
                .ok()
                .zip(rage_rpf::Gta4Archive::open_default(&base_path).ok());
            if let Some((m, b)) = pair {
                let d = compare_gta4_folders(&m, rage_rpf::Gta4Archive::ROOT, &b, rage_rpf::Gta4Archive::ROOT);
                let before = out.len();
                let mut names: Vec<(&String, &Mark)> = d.marks.iter().filter(|(_, k)| **k != Mark::Unchanged).collect();
                names.sort_by(|a, b| a.0.cmp(b.0));
                for (key, mark) in names {
                    let id = m.find_child(rage_rpf::Gta4Archive::ROOT, key);
                    let (name, size) = id.map_or((key.clone(), 0), |i| (m.entry(i).name.clone(), m.entry(i).size()));
                    let kind = if *mark == Mark::Added { ChangeKind::Added } else { ChangeKind::Modified };
                    out.push(Change { vpath: format!("{}/{name}", f.rel), kind, is_dir: false, size });
                }
                for r in &d.removed {
                    out.push(Change {
                        vpath: format!("{}/{}", f.rel, r.name),
                        kind: ChangeKind::Removed,
                        is_dir: false,
                        size: r.size,
                    });
                }
                if out.len() == before {
                    out.push(Change {
                        vpath: f.rel.clone(),
                        kind: ChangeKind::IdenticalCopy,
                        is_dir: false,
                        size: f.size,
                    });
                }
                continue;
            }
        }
        let archives = if f.is_archive {
            Rpf7::open(&mod_path, keys.clone()).ok().zip(Rpf7::open(&base_path, keys.clone()).ok())
        } else {
            None
        };
        match archives {
            Some((m, b)) => {
                let before = out.len();
                diff_dirs(&m, Rpf7::ROOT, &b, Rpf7::ROOT, &f.rel, mode, &mut out, false);
                if out.len() == before {
                    out.push(Change {
                        vpath: f.rel.clone(),
                        kind: ChangeKind::IdenticalCopy,
                        is_dir: false,
                        size: f.size,
                    });
                }
            }
            None => {
                out.push(Change { vpath: f.rel.clone(), kind: ChangeKind::ReplacedFile, is_dir: false, size: f.size })
            }
        }
    }
    out
}
