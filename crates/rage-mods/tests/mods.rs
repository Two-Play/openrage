//! A tiny fake game with a `mods` folder, built with the archive writer.

use std::path::PathBuf;

use rage_mods::{ChangeKind, CompareMode, Mark, ModFileKind, Origin};
use rage_rpf::{ArchiveWriter, Rpf7};

fn archive(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut w = ArchiveWriter::new();
    for (path, data) in files {
        w.add_file(path, data.to_vec()).unwrap();
    }
    w.build().unwrap()
}

fn fake_game(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("openrage-mods-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let big: Vec<u8> = (0..400_000u32).map(|i| (i % 251) as u8).collect();
    let nested_base = archive(&[("deep.txt", b"original")]);
    let nested_mod = archive(&[("deep.txt", b"modded!!")]);
    let base = archive(&[
        ("a.txt", b"same"),
        ("old.txt", b"gone in mod"),
        ("data/x.meta", b"<Value>1</Value>"),
        ("data/big.bin", &big),
        ("nested.rpf", &nested_base),
    ]);
    let modded = archive(&[
        ("a.txt", b"same"),
        ("data/x.meta", b"<Value>2</Value>"),
        ("data/new.meta", b"<New/>"),
        ("data/big.bin", &big),
        ("nested.rpf", &nested_mod),
    ]);
    let same = archive(&[("s.txt", b"s")]);
    std::fs::create_dir_all(root.join("mods/update")).unwrap();
    std::fs::write(root.join("common.rpf"), base).unwrap();
    std::fs::write(root.join("same.rpf"), &same).unwrap();
    std::fs::write(root.join("mods/common.rpf"), modded).unwrap();
    std::fs::write(root.join("mods/same.rpf"), &same).unwrap();
    std::fs::write(root.join("mods/update/new.rpf"), archive(&[("n.txt", b"n")])).unwrap();
    root
}

#[test]
fn scans_mods_folder() {
    let game = fake_game("scan");
    let files = rage_mods::scan(&game);
    let summary: Vec<(&str, ModFileKind)> = files.iter().map(|f| (f.rel.as_str(), f.kind)).collect();
    assert_eq!(
        summary,
        [
            ("common.rpf", ModFileKind::Replaces),
            ("same.rpf", ModFileKind::Replaces),
            ("update/new.rpf", ModFileKind::New)
        ]
    );
}

#[test]
fn lists_changes() {
    let game = fake_game("changes");
    for mode in [CompareMode::Quick, CompareMode::Deep] {
        let mut changes: Vec<(String, ChangeKind)> = rage_mods::list_changes(&game, None, mode, &mut |_, _, _| {})
            .into_iter()
            .map(|c| (c.vpath, c.kind))
            .collect();
        changes.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            changes,
            [
                ("common.rpf/data/new.meta".to_owned(), ChangeKind::Added),
                ("common.rpf/data/x.meta".to_owned(), ChangeKind::Modified),
                ("common.rpf/nested.rpf/deep.txt".to_owned(), ChangeKind::Modified),
                ("common.rpf/old.txt".to_owned(), ChangeKind::Removed),
                ("same.rpf".to_owned(), ChangeKind::IdenticalCopy),
                ("update/new.rpf".to_owned(), ChangeKind::NewFile),
            ],
            "{mode:?}"
        );
    }
}

#[test]
fn marks_folders() {
    let game = fake_game("marks");
    let m = Rpf7::open(&game.join("mods/common.rpf"), None).unwrap();
    let b = Rpf7::open(&game.join("common.rpf"), None).unwrap();
    let diff = rage_mods::compare_folders(&m, Rpf7::ROOT, &b, Rpf7::ROOT, CompareMode::Quick);
    assert_eq!(diff.marks["a.txt"], Mark::Unchanged);
    assert_eq!(diff.marks["data"], Mark::Changed);
    assert_eq!(diff.marks["nested.rpf"], Mark::Changed);
    assert_eq!(diff.removed.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["old.txt"]);

    let (md, bd) = (m.find_child(Rpf7::ROOT, "data").unwrap(), b.find_child(Rpf7::ROOT, "data").unwrap());
    let diff = rage_mods::compare_folders(&m, md, &b, bd, CompareMode::Quick);
    assert_eq!(diff.marks["new.meta"], Mark::Added);
    assert_eq!(diff.marks["x.meta"], Mark::Changed);
    assert_eq!(diff.marks["big.bin"], Mark::Unchanged);
}

#[test]
fn resolves_overlay() {
    let game = fake_game("overlay");
    let origin = |p: &str| rage_mods::resolve(&game, p, None).map(|(_, o)| o).unwrap();
    assert_eq!(origin(""), Origin::Merged);
    assert_eq!(origin("common.rpf/data/x.meta"), Origin::Mods);
    assert_eq!(origin("COMMON.RPF"), Origin::Mods);
    assert_eq!(origin("update/new.rpf"), Origin::Mods);
    match rage_mods::resolve(&game, "common.rpf/data/x.meta", None).unwrap().0 {
        rage_rpf::vfs::Node::ArchiveFile(rpf, id) => assert_eq!(rpf.read(id).unwrap(), b"<Value>2</Value>"),
        _ => panic!("expected a file"),
    }
}
