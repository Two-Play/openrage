//! GTA IV: writing into the game's own files (originals kept), undo and
//! restore, on a tiny fake game. Its own test binary, because it points the
//! config folder at a temporary directory.

use std::path::Path;

use openrage_app::install::{
    Op, WriteTarget, apply, originals, put_loose, restore_original, set_write_target, undo_last, write_target,
};
use rage_rpf::Gta4Archive;

/// A plain (unencrypted) IMG3 archive with one entry `a.txt` = `abc`.
fn plain_img(path: &Path) {
    let names = b"a.txt\0";
    let mut out = Vec::new();
    for w in [0xA94E_2A52u32, 3, 1, (16 + names.len()) as u32] {
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
fn writes_into_the_game_and_restores() {
    let root = std::env::temp_dir().join(format!("openrage-gta4-ingame-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let game = root.join("GTAIV");
    std::fs::create_dir_all(game.join("pc")).unwrap();
    std::fs::create_dir_all(game.join("common/data")).unwrap();
    std::fs::write(game.join("GTAIV.exe"), b"").unwrap();
    plain_img(&game.join("pc/test.img"));
    std::fs::write(game.join("common/data/a.dat"), b"original").unwrap();
    let img_before = std::fs::read(game.join("pc/test.img")).unwrap();
    // SAFETY: the only test in this binary; nothing else reads the environment concurrently.
    unsafe { std::env::set_var("XDG_CONFIG_HOME", root.join("config")) };

    assert_eq!(write_target(&game), WriteTarget::Overlay);
    set_write_target(&game, WriteTarget::Game).unwrap();
    assert_eq!(write_target(&game), WriteTarget::Game);

    let applied =
        apply(&game, None, "pc/test.img", &[("b.txt".into(), Op::Set(b"bee".to_vec()))], "add b.txt").unwrap();
    assert_eq!(applied.path, game.join("pc/test.img"));
    put_loose(&game, "common/data/a.dat", b"changed", "change a.dat").unwrap();

    // Changed in place; nothing went into the update folder but OpenRAGE's own data.
    let a = Gta4Archive::open(&game.join("pc/test.img"), None).unwrap();
    let b = a.find_child(Gta4Archive::ROOT, "b.txt").unwrap();
    assert_eq!(a.read(b).unwrap(), b"bee");
    drop(a);
    assert_eq!(std::fs::read(game.join("common/data/a.dat")).unwrap(), b"changed");
    assert!(!game.join("update/pc").exists());
    assert_eq!(originals(&game).unwrap(), ["common/data/a.dat", "pc/test.img"]);
    let changes: Vec<String> = rage_mods::list_game_changes(&game).into_iter().map(|c| c.vpath).collect();
    assert_eq!(changes, ["common/data/a.dat", "pc/test.img/b.txt"]);

    // Undo puts both back byte for byte.
    undo_last(&game).unwrap().unwrap();
    undo_last(&game).unwrap().unwrap();
    assert_eq!(std::fs::read(game.join("pc/test.img")).unwrap(), img_before);
    assert_eq!(std::fs::read(game.join("common/data/a.dat")).unwrap(), b"original");

    // Restore after another change: the original comes back, its history goes.
    apply(&game, None, "pc/test.img", &[("a.txt".into(), Op::Remove)], "remove a.txt").unwrap();
    restore_original(&game, "pc/test.img").unwrap();
    assert_eq!(std::fs::read(game.join("pc/test.img")).unwrap(), img_before);
    assert!(undo_last(&game).unwrap().is_none());
    assert_eq!(originals(&game).unwrap(), ["common/data/a.dat"]);

    // A copy in the update folder would shadow the game's file: refused.
    set_write_target(&game, WriteTarget::Overlay).unwrap();
    put_loose(&game, "common/data/a.dat", b"overlay", "overlay copy").unwrap();
    set_write_target(&game, WriteTarget::Game).unwrap();
    assert!(put_loose(&game, "common/data/a.dat", b"x", "refused").is_err());

    let _ = std::fs::remove_dir_all(&root);
}
