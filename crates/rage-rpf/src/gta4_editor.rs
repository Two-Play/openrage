//! Editing a GTA IV image archive (`.img`, IMG3) in place.
//!
//! Like [`crate::editor`] for RPF7: new and changed entries are appended at
//! the end (2048-byte blocks), and the table of contents after the 20-byte
//! header is rewritten — unencrypted, as in the few plain archives the game
//! ships. When the new table no longer fits in front of the first data block,
//! the entries in its way are copied to the end as well. Data of replaced or
//! removed entries stays as unreferenced space.
//!
//! Every commit returns a [`Backup`] of the overwritten start region and the
//! old length, taken before anything is written.

use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;

use rage_core::io::read_exact_at;
use rage_crypto::Gta4Key;

use crate::{Backup, CommitStats, RpfError};

const MAGIC: u32 = 0xA94E_2A52;
const BLOCK: u64 = 2048;
const HEADER: u64 = 20;

/// One change to an image archive. Names are matched ignoring ASCII case.
#[derive(Debug, Clone)]
pub enum ImgEdit {
    /// Adds or replaces an entry. Data starting with `RSC\x05` is stored as a
    /// resource (its type and flags taken from the header), anything else as a
    /// plain file.
    Set {
        name: String,
        data: Vec<u8>,
    },
    Remove {
        name: String,
    },
    Rename {
        from: String,
        to: String,
    },
}

#[derive(Debug, Clone)]
struct Item {
    name: String,
    size: u32,
    kind: u32,
    /// Block where the data starts; `None` for data not written yet.
    block: Option<u32>,
    blocks: u16,
    pad: u16,
    data: Option<Vec<u8>>,
}

fn word(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().expect("4 bytes"))
}

fn corrupt(path: &Path, reason: impl Into<String>) -> RpfError {
    RpfError::CorruptToc { archive: path.display().to_string(), reason: reason.into() }
}

/// Reads the table of contents (decrypting it if needed).
fn read_items(file: &File, path: &Path, key: Option<&Gta4Key>) -> Result<(Vec<Item>, u64), RpfError> {
    let mut head = [0u8; 20];
    read_exact_at(file, &mut head, 0)?;
    let encrypted = word(&head, 0) != MAGIC;
    if encrypted {
        let key = key.ok_or_else(|| RpfError::KeysRequired(path.display().to_string()))?;
        key.decrypt(&mut head[..16]);
        if word(&head, 0) != MAGIC {
            return Err(corrupt(path, "not an IMG3 archive"));
        }
    }
    let count = word(&head, 8) as usize;
    let toc_len = word(&head, 12) as usize;
    if toc_len < count * 16 || toc_len > 64 << 20 {
        return Err(corrupt(path, format!("{count} entries in a {toc_len}-byte table")));
    }
    let mut toc = vec![0u8; toc_len];
    read_exact_at(file, &mut toc, HEADER)?;
    if encrypted {
        key.expect("checked").decrypt(&mut toc);
    }
    let mut names = toc[count * 16..].split(|b| *b == 0);
    let mut items = Vec::with_capacity(count);
    for i in 0..count {
        let e = &toc[i * 16..i * 16 + 16];
        let name = names.next().ok_or_else(|| corrupt(path, "names are missing"))?;
        items.push(Item {
            name: String::from_utf8_lossy(name).into_owned(),
            size: word(e, 0),
            kind: word(e, 4),
            block: Some(word(e, 8)),
            blocks: u16::from_le_bytes([e[12], e[13]]),
            pad: u16::from_le_bytes([e[14], e[15]]),
            data: None,
        });
    }
    Ok((items, HEADER + toc_len as u64))
}

/// An entry for new data.
fn item_for(name: &str, data: Vec<u8>) -> Result<Item, RpfError> {
    let blocks = (data.len() as u64).div_ceil(BLOCK);
    if blocks > u16::MAX as u64 {
        return Err(RpfError::BadPath(
            format!("{name}: {} bytes is too large for an image archive", data.len()).into(),
        ));
    }
    let pad = (blocks * BLOCK - data.len() as u64) as u16;
    let (size, kind) = match data.get(0..12) {
        Some(h) if &h[0..4] == b"RSC\x05" => (word(h, 8) | 0xC000_0000, word(h, 4)),
        _ => (data.len() as u32, 0),
    };
    Ok(Item { name: name.to_owned(), size, kind, block: None, blocks: blocks as u16, pad, data: Some(data) })
}

fn table(items: &[Item]) -> Vec<u8> {
    let mut out = Vec::with_capacity(items.len() * 16);
    for it in items {
        out.extend(it.size.to_le_bytes());
        out.extend(it.kind.to_le_bytes());
        out.extend(it.block.unwrap_or(0).to_le_bytes());
        out.extend(it.blocks.to_le_bytes());
        out.extend(it.pad.to_le_bytes());
    }
    for it in items {
        out.extend(it.name.as_bytes());
        out.push(0);
    }
    out
}

/// Applies `edits` to the archive at `path`. `key` is needed when its table
/// is encrypted (it is written back unencrypted). `before` receives the
/// backup before anything is written (to record it for undo); an error there
/// leaves the archive untouched.
pub fn commit_img_with(
    path: &Path,
    key: Option<&Gta4Key>,
    edits: &[ImgEdit],
    before: &mut dyn FnMut(&Backup) -> Result<(), RpfError>,
) -> Result<CommitStats, RpfError> {
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    let old_len = file.metadata()?.len();
    let (mut items, old_toc_end) = read_items(&file, path, key)?;
    let find = |items: &[Item], name: &str| items.iter().position(|i| i.name.eq_ignore_ascii_case(name));
    for edit in edits {
        match edit {
            ImgEdit::Set { name, data } => {
                let new = item_for(name, data.clone())?;
                match find(&items, name) {
                    Some(i) => items[i] = Item { name: items[i].name.clone(), ..new },
                    None => items.push(new),
                }
            }
            ImgEdit::Remove { name } => {
                let i = find(&items, name).ok_or_else(|| RpfError::NotFound(name.clone()))?;
                items.remove(i);
            }
            ImgEdit::Rename { from, to } => {
                if find(&items, to).is_some() {
                    return Err(RpfError::BadPath(format!("{to} already exists").into()));
                }
                let i = find(&items, from).ok_or_else(|| RpfError::NotFound(from.clone()))?;
                items[i].name = to.clone();
            }
        }
    }
    // Make room for the table: entries starting before its end move to the end.
    let mut stats = CommitStats { changed: !edits.is_empty(), ..Default::default() };
    loop {
        let toc_end = HEADER + table(&items).len() as u64;
        let in_the_way = items.iter().position(|i| i.block.is_some_and(|b| (b as u64) * BLOCK < toc_end));
        let Some(i) = in_the_way else { break };
        let len = items[i].blocks as u64 * BLOCK;
        let mut data = vec![0u8; len as usize];
        read_exact_at(&file, &mut data, items[i].block.expect("placed") as u64 * BLOCK)?;
        items[i].data = Some(data);
        items[i].block = None;
        stats.relocated += 1;
    }
    let new_toc_end = HEADER + table(&items).len() as u64;
    // New data goes at block boundaries behind both the old data and the new table.
    let mut end = old_len.max(new_toc_end).div_ceil(BLOCK) * BLOCK;
    for it in items.iter_mut().filter(|i| i.data.is_some()) {
        let padded = (it.data.as_ref().expect("filtered").len() as u64).div_ceil(BLOCK).max(1) * BLOCK;
        it.block = Some((end / BLOCK) as u32);
        end += padded;
    }
    let new_len = end.max(new_toc_end);
    // Back up the start region before writing anything.
    let region_len = old_toc_end.max(new_toc_end).min(old_len);
    let mut region = vec![0u8; region_len as usize];
    read_exact_at(&file, &mut region, 0)?;
    before(&Backup { old_len, new_len, region })?;
    for it in items.iter_mut().filter(|i| i.data.is_some()) {
        let data = it.data.take().expect("filtered");
        file.seek(SeekFrom::Start(it.block.expect("placed") as u64 * BLOCK))?;
        file.write_all(&data)?;
        let padded = (data.len() as u64).div_ceil(BLOCK).max(1) * BLOCK;
        file.write_all(&vec![0u8; (padded - data.len() as u64) as usize])?;
        stats.appended += padded;
    }
    let toc = table(&items);
    let mut head = Vec::with_capacity(HEADER as usize);
    head.extend(MAGIC.to_le_bytes());
    head.extend(3u32.to_le_bytes());
    head.extend((items.len() as u32).to_le_bytes());
    head.extend((toc.len() as u32).to_le_bytes());
    head.extend(16u16.to_le_bytes());
    head.extend(0xE9u16.to_le_bytes());
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&head)?;
    file.write_all(&toc)?;
    file.set_len(new_len)?;
    file.sync_all()?;
    stats.new_len = new_len;
    Ok(stats)
}

/// Like [`commit_img_with`], returning the backup.
pub fn commit_img(path: &Path, key: Option<&Gta4Key>, edits: &[ImgEdit]) -> Result<(Backup, CommitStats), RpfError> {
    let mut taken = None;
    let stats = commit_img_with(path, key, edits, &mut |b| {
        taken = Some(b.clone());
        Ok(())
    })?;
    Ok((taken.expect("backup taken"), stats))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Gta4Archive;

    fn plain_img(path: &Path) {
        let names = b"a.txt\0";
        let mut out = Vec::new();
        for w in [MAGIC, 3, 1, (16 + names.len()) as u32] {
            out.extend(w.to_le_bytes());
        }
        out.extend(16u16.to_le_bytes());
        out.extend(0u16.to_le_bytes());
        for w in [3u32, 0, 1] {
            out.extend(w.to_le_bytes());
        }
        out.extend(1u16.to_le_bytes());
        out.extend(2045u16.to_le_bytes());
        out.extend(names);
        out.resize(2048, 0);
        out.extend(b"abc");
        out.resize(4096, 0);
        std::fs::write(path, out).unwrap();
    }

    #[test]
    fn edits_and_undoes() {
        let dir = std::env::temp_dir().join(format!("openrage-imgedit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.img");
        plain_img(&path);
        let original = std::fs::read(&path).unwrap();
        // Many new entries: the table outgrows the first block, so a.txt moves.
        let mut edits: Vec<ImgEdit> =
            (0..200).map(|i| ImgEdit::Set { name: format!("file_{i:03}.dat"), data: vec![i as u8; 10] }).collect();
        edits.push(ImgEdit::Set { name: "thing.wdr".into(), data: b"RSC\x05\x6e\0\0\0\x10\x00\x0c\xd8zz".to_vec() });
        edits.push(ImgEdit::Rename { from: "A.TXT".into(), to: "b.txt".into() });
        let (backup, stats) = commit_img(&path, None, &edits).unwrap();
        assert_eq!(stats.relocated, 1);
        let a = Gta4Archive::open(&path, None).unwrap();
        assert_eq!(a.len(), 203);
        let b = a.find_child(Gta4Archive::ROOT, "b.txt").unwrap();
        assert_eq!(a.read(b).unwrap(), b"abc");
        let f = a.find_child(Gta4Archive::ROOT, "file_150.dat").unwrap();
        assert_eq!(a.read(f).unwrap(), vec![150u8; 10]);
        let r = a.find_child(Gta4Archive::ROOT, "thing.wdr").unwrap();
        assert_eq!(a.entry(r).resource_type(), Some(110));
        drop(a);
        let (backup2, _) = commit_img(&path, None, &[ImgEdit::Remove { name: "b.txt".into() }]).unwrap();
        assert!(Gta4Archive::open(&path, None).unwrap().find_child(Gta4Archive::ROOT, "b.txt").is_none());
        backup2.restore(&path).unwrap();
        backup.restore(&path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), original);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
