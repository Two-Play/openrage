//! In-place editing of archives, including undo.

use std::path::{Path, PathBuf};

use rage_rpf::vfs::{self, Node};
use rage_rpf::{ArchiveEditor, ArchiveWriter, Backup, Rpf7, file_entry};

fn temp(name: &str, bytes: &[u8]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("openrage-editor-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("test.rpf");
    std::fs::write(&path, bytes).unwrap();
    path
}

fn base_archive() -> Vec<u8> {
    let mut w = ArchiveWriter::new();
    w.add_file("data/a.meta", b"<A/>".repeat(100)).unwrap();
    w.add_file("data/b.txt", b"bee".to_vec()).unwrap();
    w.add_file("top.xml", b"<top/>".to_vec()).unwrap();
    let mut nested = ArchiveWriter::new();
    nested.add_file("inner.txt", b"inner".to_vec()).unwrap();
    w.add_file("nested.rpf", nested.build().unwrap()).unwrap();
    w.build().unwrap()
}

fn read(path: &Path, inner: &str) -> Option<Vec<u8>> {
    match vfs::resolve(path.parent().unwrap(), &format!("test.rpf/{inner}"), None).ok()? {
        Node::ArchiveFile(rpf, id) => rpf.read(id).ok(),
        _ => None,
    }
}

fn listing(path: &Path) -> Vec<String> {
    let rpf = Rpf7::open(path, None).unwrap();
    let mut out = Vec::new();
    vfs::walk(&rpf, Rpf7::ROOT, "", true, &mut |_, _, p| out.push(p.to_owned()), &mut |e| panic!("{e}"));
    out
}

#[test]
fn edits_in_place_and_undoes_exactly() {
    let original = base_archive();
    let path = temp("edit", &original);

    let mut ed = ArchiveEditor::open(&path, None).unwrap();
    ed.set("data/b.txt", file_entry("data/b.txt", b"changed".to_vec()).unwrap()).unwrap();
    ed.set("new/file.dat", file_entry("new/file.dat", vec![7; 5000]).unwrap()).unwrap();
    assert_eq!(ed.remove("top.xml"), 1);
    ed.rename("data", "renamed").unwrap();
    ed.create_dir("empty/dir").unwrap();
    let mut backup: Option<Backup> = None;
    let stats = ed
        .commit(&mut |b| {
            backup = Some(b.clone());
            Ok(())
        })
        .unwrap();
    assert!(stats.changed);
    assert!(stats.new_len > original.len() as u64);

    assert_eq!(read(&path, "renamed/b.txt").unwrap(), b"changed");
    assert_eq!(read(&path, "renamed/a.meta").unwrap(), b"<A/>".repeat(100));
    assert_eq!(read(&path, "new/file.dat").unwrap(), vec![7; 5000]);
    assert_eq!(read(&path, "nested.rpf/inner.txt").unwrap(), b"inner");
    assert!(read(&path, "top.xml").is_none());
    assert!(listing(&path).contains(&"empty/dir".to_owned()));

    // Undo restores the file byte for byte.
    backup.unwrap().restore(&path).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), original);
}

#[test]
fn growing_table_relocates_entries() {
    let original = base_archive();
    let path = temp("grow", &original);
    let mut ed = ArchiveEditor::open(&path, None).unwrap();
    for i in 0..300 {
        let p = format!("many/file_with_a_long_name_{i:04}.txt");
        ed.set(&p, file_entry(&p, format!("content {i}").into_bytes()).unwrap()).unwrap();
    }
    let mut backup = None;
    let stats = ed
        .commit(&mut |b| {
            backup = Some(b.clone());
            Ok(())
        })
        .unwrap();
    assert!(stats.relocated > 0, "the larger table must push existing data out of the way");
    assert_eq!(read(&path, "data/a.meta").unwrap(), b"<A/>".repeat(100));
    assert_eq!(read(&path, "nested.rpf/inner.txt").unwrap(), b"inner");
    assert_eq!(read(&path, "many/file_with_a_long_name_0299.txt").unwrap(), b"content 299");
    backup.unwrap().restore(&path).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), original);
}

#[test]
fn refuses_undo_after_later_changes() {
    let path = temp("stale", &base_archive());
    let mut ed = ArchiveEditor::open(&path, None).unwrap();
    ed.set("x.txt", file_entry("x.txt", b"x".to_vec()).unwrap()).unwrap();
    let mut first = None;
    ed.commit(&mut |b| {
        first = Some(b.clone());
        Ok(())
    })
    .unwrap();
    let mut ed = ArchiveEditor::open(&path, None).unwrap();
    ed.set("y.txt", file_entry("y.txt", vec![1; 3000]).unwrap()).unwrap();
    ed.commit(&mut |_| Ok(())).unwrap();
    assert!(first.unwrap().restore(&path).is_err());
}
