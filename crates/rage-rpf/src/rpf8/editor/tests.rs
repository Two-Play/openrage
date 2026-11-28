use super::*;
use crate::rpf8::RSC8_MAGIC;

/// A plain RPF8 archive with these files (stored as is) and, if `names`, a
/// plain name table. Entries are packed right after the table, without
/// slack, like the game's archives.
fn build(files: &[(&str, &[u8])], names: bool) -> Vec<u8> {
    let mut entries: Vec<(Entry, &[u8])> = files
        .iter()
        .map(|(p, d)| {
            let (hash, ext) = path_hash(p, 'y');
            let mut e = Entry::new_plain(hash, ext, false, d.len() as u64);
            e.path = Some((*p).to_owned());
            (e, *d)
        })
        .collect();
    entries.sort_by_key(|(e, _)| e.hash);
    let table_end = (HEADER_LEN + SIGNATURE_LEN) as usize + entries.len() * ENTRY_LEN;
    let mut data = Vec::new();
    let mut at = table_end.next_multiple_of(16) as u64;
    for (e, d) in &mut entries {
        e.offset = at;
        e.size_on_disk = (d.len() as u64).next_multiple_of(16);
        data.extend(*d);
        data.resize(data.len().next_multiple_of(16), 0);
        at += e.size_on_disk;
    }
    let blob: Vec<u8> =
        if names { files.iter().flat_map(|(p, _)| [p.as_bytes(), b"\0"].concat()).collect() } else { Vec::new() };
    let mut out = Vec::new();
    out.extend(MAGIC.to_le_bytes());
    out.extend((entries.len() as u32).to_le_bytes());
    out.extend((blob.len() as u32).to_le_bytes());
    out.extend(PLAINTEXT.to_le_bytes());
    out.extend([b'y', 0]);
    out.extend((0..=255u8).collect::<Vec<_>>());
    for (e, _) in &entries {
        out.extend(e.to_bytes());
    }
    out.resize(table_end.next_multiple_of(16), 0);
    out.extend(data);
    out.extend(blob);
    out
}

fn temp(name: &str, bytes: &[u8]) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("openrage-rpf8-edit-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("t.rpf");
    std::fs::write(&path, bytes).unwrap();
    (dir, path)
}

fn read(rpf: &Rpf8, path: &str) -> Vec<u8> {
    let i = rpf.find(path).unwrap_or_else(|| panic!("{path} missing"));
    rpf.read(&rpf.entries().unwrap()[i]).unwrap()
}

fn commit(editor: Rpf8Editor) -> (CommitStats, Backup) {
    let mut kept = None;
    let stats = editor
        .commit(&mut |b| {
            kept = Some(b.clone());
            Ok(())
        })
        .unwrap();
    (stats, kept.unwrap())
}

#[test]
fn entries_round_trip_through_bytes() {
    let mut raw = [0u8; 24];
    raw[..8]
        .copy_from_slice(&(0x1234_5678u64 | 3 << 32 | 7 << 40 | 5 << 48 | 1 << 56 | 1 << 58 | 1 << 63).to_le_bytes());
    raw[8..16].copy_from_slice(&(0x40u64 >> 4 | (0x1230u64 >> 4) << 28 | 2 << 59).to_le_bytes());
    raw[16..].copy_from_slice(&0xABCD_EF01u64.to_le_bytes());
    let e = Entry::parse(&raw);
    assert_eq!(e.to_bytes(), raw, "unknown bits survive");
}

#[test]
fn edits_read_back_and_undo_exactly() {
    let original = build(&[("data/a.txt", b"first file"), ("data/b.txt", b"second"), ("data/c.ymt", b"meta")], true);
    let (dir, path) = temp("edit", &original);

    let mut ed = Rpf8Editor::open(&path, None, None).unwrap();
    ed.set("data/a.txt", b"replaced, and longer than before".to_vec()).unwrap();
    // Two new entries grow the table over the first entry's data.
    ed.set("data/new.txt", b"brand new".to_vec()).unwrap();
    ed.set("data/more/d.txt", vec![7; 40]).unwrap();
    ed.set("data/more/e.txt", vec![8; 3]).unwrap();
    ed.set("data/more/f.txt", Vec::new()).unwrap();
    assert_eq!(ed.remove("data/b.txt"), 1);
    ed.rename("data/c.ymt", "data/renamed.ymt").unwrap();
    assert!(ed.rename("data/renamed.ymt", "data/x.ydr").is_err(), "extension ids cannot change");
    let (stats, backup) = commit(ed);
    assert!(stats.changed);
    assert!(stats.relocated >= 1, "the grown table pushed an entry to the end");
    assert_eq!(std::fs::metadata(&path).unwrap().len() % 16, 0);

    let rpf = Rpf8::open(&path).unwrap();
    assert!(!rpf.header().is_encrypted());
    assert_eq!(rpf.entries().unwrap().len(), 6);
    assert_eq!(read(&rpf, "data/more/f.txt"), b"");
    assert_eq!(read(&rpf, "data/a.txt"), b"replaced, and longer than before");
    assert_eq!(read(&rpf, "data/new.txt"), b"brand new");
    assert_eq!(read(&rpf, "data/more/d.txt"), vec![7; 40]);
    assert_eq!(read(&rpf, "data/renamed.ymt"), b"meta");
    assert!(rpf.find("data/b.txt").is_none());
    let hashes: Vec<u32> = rpf.entries().unwrap().iter().map(|e| e.hash).collect();
    assert!(hashes.is_sorted(), "sorted by hash for the game's binary search");
    // The signature block is kept.
    let now = std::fs::read(&path).unwrap();
    assert_eq!(now[16..272], original[16..272]);

    backup.restore(&path).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), original, "undo gives the original bytes back");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn resources_are_stored_from_rsc8_files() {
    let original = build(&[("a.txt", b"x")], false);
    let (dir, path) = temp("rsc", &original);
    let pages = vec![0x5Au8; 0x30];
    // 0x20 bytes of virtual pages, 0x10 of physical ones, version 2.
    let flags: u64 = 0x20 | 0x10 << 32 | 2;
    let mut file = Vec::new();
    file.extend(RSC8_MAGIC.to_le_bytes());
    file.extend(crate::rpf8::uncompressed_info(2).to_le_bytes());
    file.extend(flags.to_le_bytes());
    file.extend(&pages);
    let mut ed = Rpf8Editor::open(&path, None, None).unwrap();
    assert!(ed.set("textures/t.ytd", b"not a resource".to_vec()).is_ok());
    assert!(ed.set("textures/t.ytd", file.clone()).is_err(), "a plain file cannot become a resource");
    assert!(ed.remove("textures/t.ytd") == 1);
    ed.set("textures/t.ytd", file.clone()).unwrap();
    commit(ed);
    let rpf = Rpf8::open(&path).unwrap();
    let e = &rpf.entries().unwrap()[rpf.find("textures/t.ytd").expect("named by the new name table")];
    assert!(e.is_resource);
    assert_eq!(e.size_or_flags, flags);
    let res = rpf.read_resource(e).unwrap();
    assert_eq!(res.system.len(), 0x20);
    assert_eq!(res.graphics.len(), 0x10);
    assert_eq!(rpf.resource_file(e).unwrap(), file, "exported again as the same file");
    let loose = crate::rpf8::parse_resource_file(&file).unwrap();
    assert_eq!((loose.version, loose.system, loose.graphics), (res.version, res.system, res.graphics));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn unreadable_name_tables_are_kept() {
    let mut original = build(&[("a.txt", b"alpha"), ("b.txt", b"beta")], true);
    // Scramble the name table, as the game's encrypted ones look.
    let n = original.len();
    for b in &mut original[n - 12..] {
        *b ^= 0x9C;
    }
    let (dir, path) = temp("names", &original);
    let blob = original[n - 12..].to_vec();
    let mut ed = Rpf8Editor::open(&path, None, None).unwrap();
    let (ha, _) = path_hash("a.txt", 'y');
    ed.set(&format!("hash/{ha:08X}"), b"ALPHA".to_vec()).unwrap();
    ed.set("c.txt", b"gamma".to_vec()).unwrap();
    assert_eq!(ed.added_names().len(), 1);
    commit(ed);
    let now = std::fs::read(&path).unwrap();
    assert_eq!(now[now.len() - 12..], blob[..]);
    let rpf = Rpf8::open(&path).unwrap();
    assert_eq!(read(&rpf, &format!("hash/{ha:08X}")), b"ALPHA");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn nested_archives_open_after_a_rebuild() {
    let inner = build(&[("x.txt", b"inside")], true);
    let outer = build(&[("data/inner.rpf", &inner), ("data/y.txt", b"outside")], true);
    let (dir, path) = temp("nested", &outer);
    // Change the nested archive in a file of its own, then put it back.
    let inner_path = dir.join("inner.rpf");
    let rpf = Rpf8::open(&path).unwrap();
    std::fs::write(&inner_path, read(&rpf, "data/inner.rpf")).unwrap();
    let mut sub = Rpf8Editor::open(&inner_path, None, None).unwrap();
    sub.set("x.txt", b"changed inside".to_vec()).unwrap();
    sub.set("z.txt", b"new inside".to_vec()).unwrap();
    commit(sub);
    let mut ed = Rpf8Editor::open(&path, None, None).unwrap();
    ed.set("data/inner.rpf", std::fs::read(&inner_path).unwrap()).unwrap();
    commit(ed);

    let rpf = Rpf8::open(&path).unwrap();
    let e = &rpf.entries().unwrap()[rpf.find("data/inner.rpf").unwrap()];
    let child = rpf.open_child(e).unwrap();
    assert_eq!(read(&child, "x.txt"), b"changed inside");
    assert_eq!(read(&child, "z.txt"), b"new inside");
    assert_eq!(read(&rpf, "data/y.txt"), b"outside");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn archives_grow_from_nothing() {
    let (dir, path) = temp("empty", &empty_archive());
    assert_eq!(Rpf8::open(&path).unwrap().entries().unwrap().len(), 0);
    let mut ed = Rpf8Editor::open(&path, None, None).unwrap();
    ed.set("x64/data/a.ymt", b"a".to_vec()).unwrap();
    assert!(ed.set("x64/data", b"b".to_vec()).is_err(), "a folder of another entry");
    commit(ed);
    let rpf = Rpf8::open(&path).unwrap();
    assert_eq!(read(&rpf, "x64/data/a.ymt"), b"a");
    std::fs::remove_dir_all(dir).ok();
}
