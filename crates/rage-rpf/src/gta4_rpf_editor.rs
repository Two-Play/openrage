//! Editing GTA IV packages (`.rpf`, RPF2 and RPF3) in place.
//!
//! Like [`crate::gta4_editor`] for image archives: new and changed files are
//! appended at 2048-byte boundaries and the table of contents at 0x800 is
//! rewritten — encrypted again when it was, as is the data of archives that
//! encrypt their files (`game.rpf`). Entries in the way of a grown table move
//! to the end. Data of replaced or removed files stays as unreferenced space.
//!
//! The table is laid out as the game's own: folders breadth first, each
//! folder's children contiguous and sorted by name (RPF2) or name hash
//! (RPF3), RPF2 names in entry order after the entries. Fields OpenRAGE does
//! not interpret (folder flags and the second word of folders, the bytes
//! after the entries in RPF3) are kept, so an archive rewritten without
//! changes has exactly its original table ([`rebuild_toc`]).
//!
//! RPF3 (audio) stores name hashes instead of names; its entries are named by
//! eight hex digits (`061b65da`), and new ones must be too.

use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;

use rage_core::io::read_exact_at;
use rage_crypto::Gta4Key;

use crate::{Backup, CommitStats, RpfError};

const TOC_OFFSET: u64 = 0x800;
const BLOCK: u64 = 2048;
const MAX_TOC: usize = 64 << 20;

/// One change to a package. Paths use `/`; names are matched ignoring ASCII case.
#[derive(Debug, Clone)]
pub enum RpfEdit {
    /// Adds or replaces a file. Data starting with `RSC\x05` is stored as a
    /// resource (type and flags from its header), anything else as a plain,
    /// uncompressed file. Missing folders are created.
    Set { path: String, data: Vec<u8> },
    /// Removes a file or a folder with everything in it.
    Remove { path: String },
    /// Renames or moves a file or folder inside the package.
    Rename { from: String, to: String },
    /// Creates an empty folder (and missing parents).
    CreateDir { path: String },
}

/// Where a file's bytes come from.
#[derive(Debug, Clone)]
enum Data {
    /// Still in the archive, as stored (`stored` bytes at `offset`).
    Archive { offset: u64, stored: u64 },
    /// Stored bytes to write (already encrypted when the archive encrypts data).
    Raw(Vec<u8>),
}

#[derive(Debug, Clone)]
enum Kind {
    Dir {
        children: Vec<Node>,
        w1: u32,
        flags: u32,
    },
    /// `w1`, `w3` as stored; the offset is in `data`. Resources keep their
    /// type in the low byte of `w2`.
    File {
        w1: u32,
        w3: u32,
        kind_byte: Option<u8>,
        data: Data,
    },
}

#[derive(Debug, Clone)]
struct Node {
    name: String,
    kind: Kind,
}

/// A package's table, read into a tree.
struct Package {
    rpf3: bool,
    head: [u8; 24],
    toc_len: usize,
    /// Decrypted table as read.
    toc: Vec<u8>,
    root: Node,
    data_key: Option<Gta4Key>,
    toc_key: Option<Gta4Key>,
}

fn word(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes(b[i..i + 4].try_into().expect("4 bytes"))
}

fn corrupt(path: &Path, reason: impl Into<String>) -> RpfError {
    RpfError::CorruptToc { archive: path.display().to_string(), reason: reason.into() }
}

fn hash_of(name: &str) -> Option<u32> {
    (name.len() == 8).then(|| u32::from_str_radix(name, 16).ok()).flatten()
}

fn read_package(file: &File, path: &Path, key: Option<&Gta4Key>) -> Result<Package, RpfError> {
    let mut head = [0u8; 24];
    read_exact_at(file, &mut head, 0)?;
    let rpf3 = match &head[..4] {
        b"RPF2" => false,
        b"RPF3" => true,
        _ => return Err(corrupt(path, "not an RPF2 or RPF3 package")),
    };
    let toc_len = word(&head, 4) as usize;
    let count = word(&head, 8) as usize;
    if count == 0 || toc_len > MAX_TOC || toc_len < count * 16 || !toc_len.is_multiple_of(16) {
        return Err(corrupt(path, format!("{count} entries in a {toc_len}-byte table")));
    }
    let need = || key.cloned().ok_or_else(|| RpfError::KeysRequired(path.display().to_string()));
    let toc_key = if word(&head, 16) != 0 { Some(need()?) } else { None };
    let data_key = if !rpf3 && word(&head, 20) != 0 { Some(need()?) } else { None };
    let mut toc = vec![0u8; toc_len];
    read_exact_at(file, &mut toc, TOC_OFFSET)?;
    if let Some(k) = &toc_key {
        k.decrypt(&mut toc);
    }
    let names = &toc[count * 16..];
    let name_of = |i: usize| -> String {
        let w0 = word(&toc, i * 16);
        if rpf3 {
            return format!("{w0:08x}");
        }
        let rest = names.get(w0 as usize..).unwrap_or_default();
        let end = rest.iter().position(|b| *b == 0).unwrap_or(rest.len());
        String::from_utf8_lossy(&rest[..end]).into_owned()
    };
    fn build(
        i: usize,
        depth: usize,
        toc: &[u8],
        count: usize,
        name_of: &dyn Fn(usize) -> String,
        path: &Path,
    ) -> Result<Node, RpfError> {
        if depth > 64 {
            return Err(corrupt(path, "folders nested too deeply"));
        }
        let e = &toc[i * 16..i * 16 + 16];
        let (w1, w2, w3) = (word(e, 4), word(e, 8), word(e, 12));
        let kind = if w2 & 0x8000_0000 != 0 {
            let first = (w2 & 0x7FFF_FFFF) as usize;
            let n = (w3 & 0x3FFF_FFFF) as usize;
            if first <= i || first + n > count {
                return Err(corrupt(path, format!("folder {i} lists entries {first}..{}", first + n)));
            }
            let children =
                (first..first + n).map(|c| build(c, depth + 1, toc, count, name_of, path)).collect::<Result<_, _>>()?;
            Kind::Dir { children, w1, flags: w3 & 0xC000_0000 }
        } else if w3 & 0xC000_0000 == 0xC000_0000 {
            let offset = (w2 & 0x7FFF_FF00) as u64;
            Kind::File { w1, w3, kind_byte: Some(w2 as u8), data: Data::Archive { offset, stored: w1 as u64 } }
        } else {
            let stored = (w3 & 0xBFFF_FFFF) as u64;
            Kind::File { w1, w3, kind_byte: None, data: Data::Archive { offset: w2 as u64, stored } }
        };
        Ok(Node { name: if i == 0 { name_of(0) } else { name_of(i) }, kind })
    }
    let root = build(0, 0, &toc, count, &name_of, path)?;
    if !matches!(root.kind, Kind::Dir { .. }) {
        return Err(corrupt(path, "the first entry is not a folder"));
    }
    Ok(Package { rpf3, head, toc_len, toc, root, data_key, toc_key })
}

/// Sort key of a child: RPF2 by lowercase name, RPF3 by name hash.
fn sort_key(rpf3: bool, name: &str) -> (u32, String) {
    if rpf3 { (hash_of(name).unwrap_or(0), String::new()) } else { (0, name.to_ascii_lowercase()) }
}

impl Package {
    fn check_name(&self, name: &str) -> Result<(), RpfError> {
        if name.is_empty() || name.contains(['/', '\\', '\0']) {
            return Err(RpfError::Edit(format!("{name:?} is not a valid name")));
        }
        if self.rpf3 && hash_of(name).is_none() {
            return Err(RpfError::Edit(format!(
                "{name}: audio packages name their entries by eight hex digits (e.g. 061b65da)"
            )));
        }
        Ok(())
    }

    /// The folder at `segs`, created on the way when `create`.
    fn dir_mut(&mut self, segs: &[&str], create: bool) -> Result<&mut Vec<Node>, RpfError> {
        for s in segs {
            self.check_name(s)?;
        }
        let mut node = &mut self.root;
        for seg in segs {
            let Kind::Dir { children, .. } = &mut node.kind else { unreachable!("folders only") };
            let pos = match children.iter().position(|c| c.name.eq_ignore_ascii_case(seg)) {
                Some(p) => p,
                None if create => {
                    children.push(Node {
                        name: seg.to_string(),
                        kind: Kind::Dir { children: Vec::new(), w1: 0, flags: 0 },
                    });
                    children.len() - 1
                }
                None => return Err(RpfError::NotFound(segs.join("/"))),
            };
            node = &mut children[pos];
            if !matches!(node.kind, Kind::Dir { .. }) {
                return Err(RpfError::Edit(format!("{seg} is a file, not a folder")));
            }
        }
        let Kind::Dir { children, .. } = &mut node.kind else { unreachable!("checked") };
        Ok(children)
    }

    fn take(&mut self, path: &str) -> Result<Node, RpfError> {
        let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let (name, parent) = segs.split_last().ok_or_else(|| RpfError::Edit("cannot change the root".into()))?;
        let children = self.dir_mut(parent, false)?;
        let pos = children
            .iter()
            .position(|c| c.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| RpfError::NotFound(path.to_owned()))?;
        Ok(children.remove(pos))
    }

    fn put(&mut self, path: &str, mut node: Node, replace: bool) -> Result<(), RpfError> {
        let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let (name, parent) = segs.split_last().ok_or_else(|| RpfError::Edit("cannot change the root".into()))?;
        self.check_name(name)?;
        let children = self.dir_mut(parent, true)?;
        match children.iter().position(|c| c.name.eq_ignore_ascii_case(name)) {
            Some(p) if replace && !matches!(children[p].kind, Kind::Dir { .. }) => {
                node.name = children[p].name.clone();
                children[p] = node;
            }
            Some(_) => return Err(RpfError::Edit(format!("{path} already exists"))),
            None => {
                node.name = name.to_string();
                children.push(node);
            }
        }
        Ok(())
    }

    fn apply(&mut self, edit: &RpfEdit) -> Result<(), RpfError> {
        match edit {
            RpfEdit::Set { path, data } => {
                let mut stored = data.clone();
                if let Some(k) = &self.data_key {
                    k.encrypt(&mut stored);
                }
                let kind = match rage_resource::rsc5_header(data) {
                    Some((kind, flags)) => Kind::File {
                        w1: data.len() as u32,
                        w3: flags | 0xC000_0000,
                        kind_byte: Some(kind as u8),
                        data: Data::Raw(stored),
                    },
                    None => Kind::File {
                        w1: data.len() as u32,
                        w3: data.len() as u32,
                        kind_byte: None,
                        data: Data::Raw(stored),
                    },
                };
                if data.len() > 0x3FFF_FFFF {
                    return Err(RpfError::Edit(format!("{path}: too large for a package")));
                }
                self.put(path, Node { name: String::new(), kind }, true)
            }
            RpfEdit::Remove { path } => self.take(path).map(|_| ()),
            RpfEdit::Rename { from, to } => {
                let node = self.take(from)?;
                self.put(to, node, false)
            }
            RpfEdit::CreateDir { path } => {
                let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
                self.dir_mut(&segs, true).map(|_| ())
            }
        }
    }

    /// Sorts every folder's children as the game does.
    fn sort(&mut self) {
        fn sort(node: &mut Node, rpf3: bool) {
            if let Kind::Dir { children, .. } = &mut node.kind {
                children.sort_by_cached_key(|c| sort_key(rpf3, &c.name));
                children.iter_mut().for_each(|c| sort(c, rpf3));
            }
        }
        let rpf3 = self.rpf3;
        sort(&mut self.root, rpf3);
    }

    /// Entries in table order (breadth first; [`Self::sort`] first), as
    /// (node, index of its first child for folders).
    fn layout(&self) -> Vec<(&Node, usize)> {
        let mut order: Vec<(&Node, usize)> = vec![(&self.root, 0)];
        let mut next = 1;
        let mut i = 0;
        while i < order.len() {
            if let Kind::Dir { children, .. } = &order[i].0.kind {
                order[i].1 = next;
                next += children.len();
                order.extend(children.iter().map(|c| (c, 0)));
            }
            i += 1;
        }
        order
    }

    /// The table for files at the given offsets (parallel to the layout).
    fn table(&self, order: &[(&Node, usize)], offsets: &[u64], toc_len: usize) -> Result<Vec<u8>, RpfError> {
        let count = order.len();
        let mut entries = Vec::with_capacity(count * 16);
        let mut names = Vec::new();
        for ((node, first), offset) in order.iter().zip(offsets) {
            let w0 = if self.rpf3 {
                hash_of(&node.name).unwrap_or(0)
            } else {
                let at = names.len() as u32;
                names.extend_from_slice(node.name.as_bytes());
                names.push(0);
                at
            };
            let (w1, w2, w3) = match &node.kind {
                Kind::Dir { children, w1, flags } => (*w1, 0x8000_0000 | *first as u32, flags | children.len() as u32),
                Kind::File { w1, w3, kind_byte: Some(t), .. } => (*w1, *offset as u32 | *t as u32, *w3),
                Kind::File { w1, w3, kind_byte: None, .. } => (*w1, *offset as u32, *w3),
            };
            for w in [w0, w1, w2, w3] {
                entries.extend_from_slice(&w.to_le_bytes());
            }
        }
        if entries.len() + names.len() > toc_len {
            return Err(RpfError::Edit("table does not fit".into()));
        }
        let toc = if self.rpf3 {
            // No names in RPF3: keep the bytes after the entries where they were.
            let old_count = word(&self.head, 8) as usize;
            let mut toc = self.toc.clone();
            toc.resize(toc_len, 0);
            let old_end = (old_count * 16).min(toc.len());
            if entries.len() < old_end {
                toc[entries.len()..old_end].fill(0);
            }
            toc[..entries.len()].copy_from_slice(&entries);
            toc
        } else {
            let mut toc = entries;
            toc.extend_from_slice(&names);
            toc.resize(toc_len, 0);
            toc
        };
        Ok(toc)
    }

    /// Table size for `count` entries: the old one if it still fits, else
    /// grown in 2048-byte steps.
    fn toc_len_for(&self, order: &[(&Node, usize)]) -> usize {
        let names: usize = if self.rpf3 { 0 } else { order.iter().map(|(n, _)| n.name.len() + 1).sum() };
        let need = order.len() * 16 + names;
        if need <= self.toc_len { self.toc_len } else { need.next_multiple_of(BLOCK as usize) }
    }
}

/// Reads the package's table and lays it out again without changes. Returns
/// (decrypted table as stored, table as OpenRAGE writes it); they are equal
/// when the writer reproduces the game's layout.
pub fn rebuild_toc(path: &Path, key: Option<&Gta4Key>) -> Result<(Vec<u8>, Vec<u8>), RpfError> {
    let file = File::open(path)?;
    let mut pkg = read_package(&file, path, key)?;
    let toc_len = pkg.toc_len;
    let original = pkg.toc.clone();
    pkg.sort();
    let order = pkg.layout();
    let offsets: Vec<u64> = order
        .iter()
        .map(|(n, _)| match &n.kind {
            Kind::File { data: Data::Archive { offset, .. }, .. } => *offset,
            _ => 0,
        })
        .collect();
    let rebuilt = pkg.table(&order, &offsets, toc_len)?;
    Ok((original, rebuilt))
}

/// Applies `edits` to the package at `path`. `key` decrypts and encrypts the
/// table (and data where the archive encrypts it). `before` receives the
/// backup before anything is written; an error there leaves the file untouched.
pub fn commit_rpf_with(
    path: &Path,
    key: Option<&Gta4Key>,
    edits: &[RpfEdit],
    before: &mut dyn FnMut(&Backup) -> Result<(), RpfError>,
) -> Result<CommitStats, RpfError> {
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    let old_len = file.metadata()?.len();
    let mut pkg = read_package(&file, path, key)?;
    let old_toc_end = TOC_OFFSET + pkg.toc_len as u64;
    for edit in edits {
        pkg.apply(edit)?;
    }
    pkg.sort();
    let mut stats = CommitStats { changed: !edits.is_empty(), ..Default::default() };

    // Size the table, then move entries starting inside it to the end.
    let (toc_len, mut placed) = {
        let order = pkg.layout();
        let toc_len = pkg.toc_len_for(&order);
        let toc_end = TOC_OFFSET + toc_len as u64;
        // (offset if kept, bytes to write)
        let mut placed: Vec<(Option<u64>, Option<Vec<u8>>)> = Vec::with_capacity(order.len());
        for (node, _) in &order {
            placed.push(match &node.kind {
                Kind::Dir { .. } => (None, None),
                Kind::File { data: Data::Archive { offset, stored }, .. } if *offset < toc_end => {
                    let mut bytes = vec![0u8; *stored as usize];
                    read_exact_at(&file, &mut bytes, *offset)?;
                    stats.relocated += 1;
                    (None, Some(bytes))
                }
                Kind::File { data: Data::Archive { offset, .. }, .. } => (Some(*offset), None),
                Kind::File { data: Data::Raw(bytes), .. } => (None, Some(bytes.clone())),
            });
        }
        (toc_len, placed)
    };
    let new_toc_end = TOC_OFFSET + toc_len as u64;
    let mut end = old_len.max(new_toc_end).div_ceil(BLOCK) * BLOCK;
    let mut offsets = Vec::with_capacity(placed.len());
    for (kept, bytes) in &placed {
        offsets.push(match (kept, bytes) {
            (Some(o), _) => *o,
            (None, Some(b)) => {
                let at = end;
                end += (b.len() as u64).div_ceil(BLOCK).max(1) * BLOCK;
                at
            }
            (None, None) => 0,
        });
    }
    if end > 0x7FFF_FF00 {
        return Err(RpfError::Edit("the package would grow past 2 GiB".into()));
    }
    let new_len = end.max(new_toc_end);
    let mut toc = {
        let order = pkg.layout();
        pkg.table(&order, &offsets, toc_len)?
    };

    // Back up the start region before writing anything.
    let region_len = old_toc_end.max(new_toc_end).min(old_len);
    let mut region = vec![0u8; region_len as usize];
    read_exact_at(&file, &mut region, 0)?;
    before(&Backup { old_len, new_len, region })?;

    for ((_, bytes), offset) in placed.iter_mut().zip(&offsets) {
        let Some(bytes) = bytes.take() else { continue };
        file.seek(SeekFrom::Start(*offset))?;
        file.write_all(&bytes)?;
        let padded = (bytes.len() as u64).div_ceil(BLOCK).max(1) * BLOCK;
        file.write_all(&vec![0u8; (padded - bytes.len() as u64) as usize])?;
        stats.appended += padded;
    }
    if let Some(k) = &pkg.toc_key {
        k.encrypt(&mut toc);
    }
    let mut head = pkg.head;
    head[4..8].copy_from_slice(&(toc_len as u32).to_le_bytes());
    head[8..12].copy_from_slice(&(offsets.len() as u32).to_le_bytes());
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&head)?;
    file.seek(SeekFrom::Start(TOC_OFFSET))?;
    file.write_all(&toc)?;
    file.set_len(new_len)?;
    file.sync_all()?;
    stats.new_len = new_len;
    Ok(stats)
}

/// Like [`commit_rpf_with`], returning the backup.
pub fn commit_rpf(path: &Path, key: Option<&Gta4Key>, edits: &[RpfEdit]) -> Result<(Backup, CommitStats), RpfError> {
    let mut taken = None;
    let stats = commit_rpf_with(path, key, edits, &mut |b| {
        taken = Some(b.clone());
        Ok(())
    })?;
    Ok((taken.expect("backup taken"), stats))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Gta4Archive;

    /// A plain RPF2 package: `/` with `data/a.txt` ("abc") and `b.txt` ("bee").
    fn plain_rpf2(path: &Path) {
        let names = b"/\0b.txt\0data\0a.txt\0";
        let mut toc = Vec::new();
        let entry = |toc: &mut Vec<u8>, w: [u32; 4]| w.iter().for_each(|x| toc.extend_from_slice(&x.to_le_bytes()));
        entry(&mut toc, [0, 2, 0x8000_0001, 2]); // root: entries 1..3
        entry(&mut toc, [2, 3, 0x1800, 3]); // b.txt
        entry(&mut toc, [8, 0, 0x8000_0003, 1]); // data/: entry 3
        entry(&mut toc, [13, 3, 0x1000, 3]); // data/a.txt
        toc.extend_from_slice(names);
        toc.resize(0x800, 0);
        let mut out = Vec::new();
        out.extend_from_slice(b"RPF2");
        for w in [0x800u32, 4, 0, 0, 0] {
            out.extend_from_slice(&w.to_le_bytes());
        }
        out.resize(0x800, 0);
        out.extend_from_slice(&toc);
        out.extend_from_slice(b"abc");
        out.resize(0x1800, 0);
        out.extend_from_slice(b"bee");
        out.resize(0x2000, 0);
        std::fs::write(path, out).unwrap();
    }

    fn read(a: &Gta4Archive, path: &str) -> Option<Vec<u8>> {
        let mut id = Gta4Archive::ROOT;
        for seg in path.split('/') {
            id = a.find_child(id, seg)?;
        }
        a.read(id).ok()
    }

    #[test]
    fn rebuilds_its_own_table_exactly() {
        let dir = std::env::temp_dir().join(format!("openrage-rpfedit-a-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.rpf");
        plain_rpf2(&path);
        let (original, rebuilt) = rebuild_toc(&path, None).unwrap();
        assert_eq!(original, rebuilt);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn edits_and_undoes() {
        let dir = std::env::temp_dir().join(format!("openrage-rpfedit-b-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.rpf");
        plain_rpf2(&path);
        let original = std::fs::read(&path).unwrap();
        // Many new files: the table outgrows its 0x800 bytes, so the first data moves.
        let mut edits: Vec<RpfEdit> =
            (0..150).map(|i| RpfEdit::Set { path: format!("new/file_{i:03}.dat"), data: vec![i as u8; 10] }).collect();
        edits.push(RpfEdit::Set { path: "data/a.txt".into(), data: b"changed".to_vec() });
        edits.push(RpfEdit::Set { path: "thing.wdr".into(), data: b"RSC\x05\x6e\0\0\0\x10\x00\x0c\xd8zz".to_vec() });
        edits.push(RpfEdit::Rename { from: "B.TXT".into(), to: "data/moved.txt".into() });
        edits.push(RpfEdit::CreateDir { path: "empty/inner".into() });
        let (backup, stats) = commit_rpf(&path, None, &edits).unwrap();
        assert!(stats.relocated >= 1);
        let a = Gta4Archive::open(&path, None).unwrap();
        assert_eq!(read(&a, "data/a.txt").unwrap(), b"changed");
        assert_eq!(read(&a, "data/moved.txt").unwrap(), b"bee");
        assert_eq!(read(&a, "new/file_120.dat").unwrap(), vec![120u8; 10]);
        assert!(a.find_child(Gta4Archive::ROOT, "b.txt").is_none());
        let empty = a.find_child(Gta4Archive::ROOT, "empty").unwrap();
        assert!(a.find_child(empty, "inner").is_some());
        let r = a.find_child(Gta4Archive::ROOT, "thing.wdr").unwrap();
        assert_eq!(a.entry(r).resource_type(), Some(110));
        drop(a);
        // The rewritten table is laid out the way it would be rebuilt.
        let (stored, rebuilt) = rebuild_toc(&path, None).unwrap();
        assert_eq!(stored, rebuilt);
        let (backup2, _) = commit_rpf(&path, None, &[RpfEdit::Remove { path: "new".into() }]).unwrap();
        assert!(Gta4Archive::open(&path, None).unwrap().find_child(Gta4Archive::ROOT, "new").is_none());
        backup2.restore(&path).unwrap();
        backup.restore(&path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), original);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn audio_names_are_hashes() {
        let pkg = Package {
            rpf3: true,
            head: [0; 24],
            toc_len: 0x800,
            toc: vec![0; 0x800],
            root: Node { name: String::new(), kind: Kind::Dir { children: Vec::new(), w1: 0, flags: 0 } },
            data_key: None,
            toc_key: None,
        };
        assert!(pkg.check_name("061b65da").is_ok());
        assert!(pkg.check_name("speech.ivaud").is_err());
    }
}
