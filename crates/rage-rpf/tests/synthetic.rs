//! Parses hand-built unencrypted ("OPEN") RPF7 archives, so the reader can be
//! tested without game files or keys.

use std::io::Write;
use std::path::PathBuf;

use flate2::Compression;
use flate2::write::DeflateEncoder;
use rage_rpf::vfs::{self, Node};
use rage_rpf::{EntryKind, Rpf7, RpfError};

const OPEN: u32 = 0x4E45_504F;

enum Item {
    Dir { name: &'static str, first: u32, count: u32 },
    File { name: &'static str, data: Vec<u8>, compress: bool },
    Resource { name: &'static str, body: Vec<u8>, system_flags: u32, graphics_flags: u32 },
}

fn build(items: &[Item]) -> Vec<u8> {
    let mut names = Vec::new();
    let mut name_offsets = Vec::new();
    for item in items {
        let name = match item {
            Item::Dir { name, .. } | Item::File { name, .. } | Item::Resource { name, .. } => *name,
        };
        name_offsets.push(names.len() as u32);
        names.extend_from_slice(name.as_bytes());
        names.push(0);
    }
    while names.len() % 16 != 0 {
        names.push(0);
    }

    let toc_len = 16 + items.len() * 16 + names.len();
    let mut data_area = Vec::new();
    let data_start = toc_len.div_ceil(512) * 512;
    let mut entries = Vec::new();
    for (item, &name_off) in items.iter().zip(&name_offsets) {
        let mut e = [0u8; 16];
        let mut place = |bytes: &[u8]| -> u32 {
            let block = ((data_start + data_area.len()) / 512) as u32;
            data_area.extend_from_slice(bytes);
            while data_area.len() % 512 != 0 {
                data_area.push(0);
            }
            block
        };
        match item {
            Item::Dir { first, count, .. } => {
                e[0..4].copy_from_slice(&name_off.to_le_bytes());
                e[4..8].copy_from_slice(&0x7FFF_FF00u32.to_le_bytes());
                e[8..12].copy_from_slice(&first.to_le_bytes());
                e[12..16].copy_from_slice(&count.to_le_bytes());
            }
            Item::File { data, compress, .. } => {
                let stored = if *compress {
                    let mut enc = DeflateEncoder::new(Vec::new(), Compression::default());
                    enc.write_all(data).unwrap();
                    enc.finish().unwrap()
                } else {
                    data.clone()
                };
                let block = place(&stored);
                let stored_size = if *compress { stored.len() as u32 } else { 0 };
                e[0..2].copy_from_slice(&(name_off as u16).to_le_bytes());
                e[2..5].copy_from_slice(&stored_size.to_le_bytes()[..3]);
                e[5..8].copy_from_slice(&block.to_le_bytes()[..3]);
                e[8..12].copy_from_slice(&(data.len() as u32).to_le_bytes());
            }
            Item::Resource { body, system_flags, graphics_flags, .. } => {
                let mut enc = DeflateEncoder::new(Vec::new(), Compression::default());
                enc.write_all(body).unwrap();
                let mut stored = vec![0xAAu8; 16]; // in-archive bookkeeping bytes
                stored.extend(enc.finish().unwrap());
                let block = place(&stored) | 0x80_0000;
                e[0..2].copy_from_slice(&(name_off as u16).to_le_bytes());
                e[2..5].copy_from_slice(&(stored.len() as u32).to_le_bytes()[..3]);
                e[5..8].copy_from_slice(&block.to_le_bytes()[..3]);
                e[8..12].copy_from_slice(&system_flags.to_le_bytes());
                e[12..16].copy_from_slice(&graphics_flags.to_le_bytes());
            }
        }
        entries.extend_from_slice(&e);
    }

    let mut out = Vec::new();
    out.extend_from_slice(&0x5250_4637u32.to_le_bytes());
    out.extend_from_slice(&(items.len() as u32).to_le_bytes());
    out.extend_from_slice(&(names.len() as u32).to_le_bytes());
    out.extend_from_slice(&OPEN.to_le_bytes());
    out.extend_from_slice(&entries);
    out.extend_from_slice(&names);
    out.resize(data_start, 0);
    out.extend_from_slice(&data_area);
    out
}

fn sample_archive() -> Vec<u8> {
    let inner = build(&[
        Item::Dir { name: "", first: 1, count: 1 },
        Item::File { name: "deep.txt", data: b"nested content".to_vec(), compress: true },
    ]);
    build(&[
        Item::Dir { name: "", first: 1, count: 3 },
        Item::Dir { name: "data", first: 4, count: 1 },
        Item::File { name: "hello.txt", data: b"hello rage".repeat(50), compress: true },
        Item::File { name: "inner.rpf", data: inner, compress: false },
        Item::Resource {
            name: "thing.ymt",
            body: vec![1, 2, 3, 4, 5, 6, 7, 8],
            system_flags: 0x2004_0820,
            graphics_flags: 0x3000_0000,
        },
    ])
}

fn write_temp(name: &str, bytes: &[u8]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("openrage-test-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("sample.rpf");
    std::fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn reads_tree_files_resources_and_nested_archives() {
    let path = write_temp("tree", &sample_archive());
    let rpf = Rpf7::open(&path, None).expect("open");
    assert_eq!(rpf.entries().len(), 5);

    let hello = rpf.find_child(Rpf7::ROOT, "HELLO.TXT").expect("case-insensitive lookup");
    assert_eq!(rpf.read(hello).unwrap(), b"hello rage".repeat(50));

    let data = rpf.find_child(Rpf7::ROOT, "data").unwrap();
    let thing = rpf.find_child(data, "thing.ymt").unwrap();
    assert_eq!(rpf.path_of(thing), "data/thing.ymt");
    assert_eq!(rpf.entry(thing).resource_version(), Some(0x23));
    let exported = rpf.read(thing).unwrap();
    assert_eq!(&exported[0..4], b"RSC7");
    assert_eq!(u32::from_le_bytes(exported[4..8].try_into().unwrap()), 0x23);
    assert_eq!(rpf.read_resource_pages(thing).unwrap(), vec![1, 2, 3, 4, 5, 6, 7, 8]);

    let inner_id = rpf.find_child(Rpf7::ROOT, "inner.rpf").unwrap();
    assert!(rpf.entry(inner_id).is_archive());
    let inner = rpf.open_child(inner_id).unwrap();
    let deep = inner.find_child(Rpf7::ROOT, "deep.txt").unwrap();
    assert_eq!(inner.read(deep).unwrap(), b"nested content");
}

#[test]
fn vfs_resolves_through_nested_archives() {
    let path = write_temp("vfs", &sample_archive());
    let root = path.parent().unwrap();
    match vfs::resolve(root, "Sample.rpf/inner.rpf/deep.txt", None).unwrap() {
        Node::ArchiveFile(rpf, id) => assert_eq!(rpf.read(id).unwrap(), b"nested content"),
        _ => panic!("expected an archive file"),
    }
    assert!(matches!(vfs::resolve(root, "sample.rpf/missing", None), Err(RpfError::NotFound(_))));

    let mut seen = Vec::new();
    let Node::ArchiveDir(rpf, dir) = vfs::resolve(root, "sample.rpf", None).unwrap() else { panic!() };
    vfs::walk(&rpf, dir, "", true, &mut |_, _, p| seen.push(p.to_owned()), &mut |e| panic!("{e}"));
    assert_eq!(seen, ["data", "data/thing.ymt", "hello.txt", "inner.rpf", "inner.rpf/deep.txt"]);
}

#[test]
fn rejects_corrupt_tables() {
    let mut bytes = sample_archive();
    // Point the root directory's child range past the end of the table.
    bytes[16 + 8..16 + 12].copy_from_slice(&100u32.to_le_bytes());
    let path = write_temp("corrupt", &bytes);
    assert!(matches!(Rpf7::open(&path, None), Err(RpfError::CorruptToc { .. })));

    let path = write_temp("magic", b"NOPE0000000000000000");
    assert!(matches!(Rpf7::open(&path, None), Err(RpfError::BadMagic(..))));
}

#[test]
fn entry_kinds_are_detected() {
    let path = write_temp("kinds", &sample_archive());
    let rpf = Rpf7::open(&path, None).unwrap();
    let labels: Vec<_> = rpf.entries().iter().map(|e| e.kind_label()).collect();
    assert_eq!(labels, ["dir", "dir", "file", "rpf", "resource"]);
    assert!(matches!(rpf.entry(2).kind, EntryKind::Binary { encrypted: false, .. }));
}

#[test]
fn writer_round_trip_including_raw_copies() {
    use rage_rpf::{ArchiveWriter, RawEntry};

    // Copy everything from the sample archive raw, then modify it like a mod would.
    let path = write_temp("writer-src", &sample_archive());
    let src = Rpf7::open(&path, None).unwrap();
    let mut w = ArchiveWriter::new();
    let mut copied = 0;
    vfs::walk(
        &src,
        Rpf7::ROOT,
        "",
        false,
        &mut |a, id, p| {
            if !a.entry(id).is_dir() {
                w.add_raw(p, a.read_raw_entry(id).unwrap());
                copied += 1;
            }
        },
        &mut |e| panic!("{e}"),
    );
    assert_eq!(copied, 3);
    w.add_file("hello.txt", b"replaced".to_vec()).unwrap();
    w.add_file("new/dir/added.meta", b"<Item>NEW</Item>".repeat(20)).unwrap();
    assert_eq!(w.remove("inner.rpf"), 1);
    let rsc = src.read(src.find_child(src.find_child(Rpf7::ROOT, "data").unwrap(), "thing.ymt").unwrap()).unwrap();
    w.add_resource_file("data/copy.ymt", rsc).unwrap();
    w.add_raw("x", RawEntry::Binary { data: vec![], compressed: false, size: 0, encrypted: false });
    assert_eq!(w.remove("X"), 1, "removal is case-insensitive");

    let out = write_temp("writer-out", &w.build().unwrap());
    let rpf = Rpf7::open(&out, None).expect("written archive opens");
    assert_eq!(rpf.encryption(), rage_rpf::Encryption::Open);
    let mut seen = Vec::new();
    vfs::walk(&rpf, Rpf7::ROOT, "", true, &mut |_, _, p| seen.push(p.to_owned()), &mut |e| panic!("{e}"));
    // Depth-first walk over sorted children.
    assert_eq!(seen, ["data", "data/copy.ymt", "data/thing.ymt", "hello.txt", "new", "new/dir", "new/dir/added.meta"]);

    let read = |p: &str| match vfs::resolve(out.parent().unwrap(), &format!("sample.rpf/{p}"), None).unwrap() {
        Node::ArchiveFile(a, id) => a.read(id).unwrap(),
        _ => panic!("{p} is not a file"),
    };
    assert_eq!(read("hello.txt"), b"replaced");
    assert_eq!(read("new/dir/added.meta"), b"<Item>NEW</Item>".repeat(20));
    let data = rpf.find_child(Rpf7::ROOT, "data").unwrap();
    for name in ["thing.ymt", "copy.ymt"] {
        let id = rpf.find_child(data, name).unwrap();
        assert_eq!(rpf.read_resource_pages(id).unwrap(), vec![1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(rpf.entry(id).resource_version(), Some(0x23));
    }
}

#[test]
fn refuses_archives_that_contain_themselves() {
    let mut bytes = sample_archive();
    // Point `inner.rpf` (entry 3) at the whole outer archive: offset 0, full size.
    let e = 16 + 3 * 16;
    bytes[e + 5..e + 8].fill(0);
    let len = bytes.len() as u32;
    bytes[e + 8..e + 12].copy_from_slice(&len.to_le_bytes());
    let path = write_temp("self", &bytes);
    let rpf = Rpf7::open(&path, None).expect("open");
    assert!(matches!(rpf.open_child(3), Err(RpfError::NotAnArchive(_))));
}

#[test]
fn stops_at_deeply_nested_archives() {
    let mut archive = build(&[
        Item::Dir { name: "", first: 1, count: 1 },
        Item::File { name: "deep.txt", data: b"x".to_vec(), compress: false },
    ]);
    for _ in 0..rage_rpf::MAX_NESTING + 2 {
        archive = build(&[
            Item::Dir { name: "", first: 1, count: 1 },
            Item::File { name: "x.rpf", data: archive, compress: false },
        ]);
    }
    let path = write_temp("deep", &archive);
    let rpf = Rpf7::open(&path, None).expect("open");
    let mut errors = Vec::new();
    vfs::walk(&rpf, Rpf7::ROOT, "", true, &mut |_, _, _| {}, &mut |e| errors.push(e));
    assert!(matches!(errors.as_slice(), [RpfError::TooDeep(_)]), "{errors:?}");
}
