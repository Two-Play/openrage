//! Writing into the mods folder and undoing it, on a tiny fake game.

use std::path::{Path, PathBuf};

use openrage_app::install::{Op, apply, apply_at, undo_last};
use openrage_app::journal::{Journal, Kind};
use rage_rpf::ArchiveWriter;
use rage_rpf::vfs::{self, Node};

fn fake_game(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("openrage-install-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("update")).unwrap();
    let mut nested = ArchiveWriter::new();
    nested.add_file("deep/inner.txt", b"inner".to_vec()).unwrap();
    let mut w = ArchiveWriter::new();
    w.add_file("data/a.txt", b"original".to_vec()).unwrap();
    w.add_file("dlc.rpf", nested.build().unwrap()).unwrap();
    std::fs::write(root.join("update/Update.rpf"), w.build().unwrap()).unwrap();
    root
}

fn read(root: &Path, vpath: &str) -> Option<Vec<u8>> {
    match rage_mods::resolve(root, vpath, None).ok()?.0 {
        Node::ArchiveFile(rpf, id) => rpf.read(id).ok(),
        _ => None,
    }
}

#[test]
fn edits_go_to_mods_and_can_be_undone() {
    let game = fake_game("flow");
    let original = std::fs::read(game.join("update/Update.rpf")).unwrap();

    // Case-insensitive path, change inside a nested archive: copies into mods first.
    let a = apply_at(&game, None, "UPDATE/update.rpf/dlc.rpf/deep/inner.txt", Op::Set(b"changed".to_vec()), "first")
        .unwrap();
    assert!(a.created);
    assert!(a.path.ends_with("mods/update/Update.rpf"), "keeps the game's casing: {}", a.path.display());
    assert_eq!(read(&game, "update/update.rpf/dlc.rpf/deep/inner.txt").unwrap(), b"changed");

    // Several operations in one go, in place.
    let ops = vec![
        ("data/a.txt".to_owned(), Op::Rename("data/renamed.txt".to_owned())),
        ("new/b.txt".to_owned(), Op::Set(b"bee".to_vec())),
        ("empty".to_owned(), Op::CreateDir),
        ("dlc.rpf/deep".to_owned(), Op::Rename("dlc.rpf/shallow".to_owned())),
    ];
    let b = apply(&game, None, "update/update.rpf", &ops, "second").unwrap();
    assert!(!b.created);
    assert_eq!(read(&game, "update/update.rpf/data/renamed.txt").unwrap(), b"original");
    assert_eq!(read(&game, "update/update.rpf/new/b.txt").unwrap(), b"bee");
    assert_eq!(read(&game, "update/update.rpf/dlc.rpf/shallow/inner.txt").unwrap(), b"changed");
    assert!(read(&game, "update/update.rpf/data/a.txt").is_none());

    let mods = game.join("mods");
    let entries = Journal::new(&mods).entries().unwrap();
    let kinds: Vec<&str> = entries
        .iter()
        .map(|e| match e.kind {
            Kind::Created => "created",
            Kind::Edited { .. } => "edited",
            Kind::FileWritten { .. } => "file",
        })
        .collect();
    assert_eq!(kinds, ["edited", "edited", "created"]);
    assert_eq!(entries[0].description, "second");

    // The game's own file was never touched.
    assert_eq!(std::fs::read(game.join("update/Update.rpf")).unwrap(), original);

    // Undo step by step.
    assert_eq!(undo_last(&game).unwrap().unwrap().description, "second");
    assert_eq!(read(&game, "update/update.rpf/data/a.txt").unwrap(), b"original");
    assert_eq!(read(&game, "update/update.rpf/dlc.rpf/deep/inner.txt").unwrap(), b"changed");
    undo_last(&game).unwrap();
    assert_eq!(std::fs::read(mods.join("update/Update.rpf")).unwrap(), original, "back to a plain copy");
    undo_last(&game).unwrap();
    assert!(!mods.join("update/Update.rpf").exists(), "copy removed again");
    assert!(undo_last(&game).unwrap().is_none());
    assert!(matches!(vfs::resolve(&game, "update/update.rpf/data/a.txt", None).unwrap(), Node::ArchiveFile(..)));
}
