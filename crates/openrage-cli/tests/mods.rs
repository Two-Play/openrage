//! The `dirs` crate ignores the environment on Windows, so the config and cache folders cannot be
//! redirected there; these tests run where they can.
#![cfg(unix)]

mod common;

use common::{Sandbox, path_str, write};

fn sandbox_with_files() -> Sandbox {
    let s = Sandbox::with_game();
    write(&s.path("new.txt"), b"new file\n");
    write(&s.path("replacement.txt"), b"replaced\n");
    s
}

fn mods_archive(s: &Sandbox, inner: &str) -> Vec<u8> {
    let out = s.path("extracted");
    s.run(&["extract", &format!("mods/common.rpf/{inner}"), "-o", &path_str(&out)]).success();
    let name = inner.rsplit('/').next().unwrap();
    std::fs::read(out.join(name)).unwrap()
}

#[test]
fn put_copies_the_archive_into_the_mods_folder_and_adds_the_file() {
    let s = sandbox_with_files();
    let out = s.run(&["put", &path_str(&s.path("new.txt")), "common.rpf/data/new.txt"]).success();
    let written = path_str(&s.game.join("mods").join("common.rpf"));
    assert_eq!(
        out.lines()[0],
        "Copied the archive into the overlay folder first (mods; GTA IV: update; RDR2: lml/replace)."
    );
    assert!(out.lines()[1].starts_with(&format!("Written to {written} (9B appended, ")), "{}", out.stdout);
    assert!(out.lines()[1].ends_with("s). Undo with `openrage undo`."), "{}", out.stdout);

    let listing = s.run(&["ls", "mods/common.rpf/data"]).success();
    assert!(listing.lines().contains(&"file           9B       new.txt"), "{}", listing.stdout);
    assert_eq!(mods_archive(&s, "data/new.txt"), b"new file\n");
}

#[test]
fn put_leaves_the_game_archive_untouched() {
    let s = sandbox_with_files();
    let before = std::fs::read(s.game.join("common.rpf")).unwrap();
    s.run(&["put", &path_str(&s.path("new.txt")), "common.rpf/data/new.txt"]).success();
    assert_eq!(std::fs::read(s.game.join("common.rpf")).unwrap(), before);
    let listing = s.run(&["ls", "common.rpf/data"]).success();
    assert!(!listing.stdout.contains("new.txt"), "{}", listing.stdout);
}

#[test]
fn put_replaces_an_existing_file_in_the_archive() {
    let s = sandbox_with_files();
    s.run(&["put", &path_str(&s.path("replacement.txt")), "common.rpf/data/hello.txt"]).success();
    assert_eq!(mods_archive(&s, "data/hello.txt"), b"replaced\n");
}

#[test]
fn a_second_put_does_not_copy_the_archive_again() {
    let s = sandbox_with_files();
    s.run(&["put", &path_str(&s.path("new.txt")), "common.rpf/data/new.txt"]).success();
    let out = s.run(&["put", &path_str(&s.path("replacement.txt")), "common.rpf/data/hello.txt"]).success();
    assert!(out.lines()[0].starts_with("Written to "), "{}", out.stdout);
    assert_eq!(out.lines().len(), 1);
}

#[test]
fn put_of_a_loose_file_goes_into_the_mods_folder() {
    let s = sandbox_with_files();
    let out = s.run(&["put", &path_str(&s.path("new.txt")), "loose/new.txt"]).success();
    let written = path_str(&s.game.join("mods").join("loose").join("new.txt"));
    assert_eq!(out.lines(), [format!("Written to {written} (into the mods folder). Undo with `openrage undo`.")]);
    assert_eq!(std::fs::read(&written).unwrap(), b"new file\n");
}

#[test]
fn put_of_a_missing_local_file_fails() {
    let s = sandbox_with_files();
    let missing = path_str(&s.path("missing.txt"));
    let out = s.run(&["put", &missing, "common.rpf/data/x.txt"]).failure();
    assert!(out.stderr.starts_with(&format!("error: reading {missing}: ")), "{}", out.stderr);
}

#[test]
fn rm_removes_a_file_in_the_mods_copy() {
    let s = sandbox_with_files();
    let out = s.run(&["rm", "common.rpf/data/config.xml"]).success();
    assert!(out.lines()[0].starts_with("Copied the archive into the overlay folder first"), "{}", out.stdout);
    let listing = s.run(&["ls", "-r", "mods/common.rpf"]).success();
    assert!(!listing.stdout.contains("config.xml"), "{}", listing.stdout);
    assert!(listing.stdout.contains("data/hello.txt"), "{}", listing.stdout);
    let original = s.run(&["ls", "-r", "common.rpf"]).success();
    assert!(original.stdout.contains("data/config.xml"), "{}", original.stdout);
}

#[test]
fn rm_of_a_missing_entry_fails() {
    let s = sandbox_with_files();
    let out = s.run(&["rm", "common.rpf/nope"]).failure();
    assert_eq!(out.stderr, "error: nope: no such entry\n");
}

#[test]
fn rm_of_a_loose_mods_file_deletes_it() {
    let s = sandbox_with_files();
    s.run(&["put", &path_str(&s.path("new.txt")), "loose/new.txt"]).success();
    let out = s.run(&["rm", "loose/new.txt"]).success();
    assert_eq!(out.lines(), ["Removed loose/new.txt from the mods folder. Undo with `openrage undo`."]);
    assert!(!s.game.join("mods/loose/new.txt").exists());
}

#[test]
fn mkdir_creates_a_folder_in_the_archive() {
    let s = sandbox_with_files();
    let out = s.run(&["mkdir", "common.rpf/newdir"]).success();
    assert!(out.lines().last().unwrap().starts_with("Written to "), "{}", out.stdout);
    let listing = s.run(&["ls", "mods/common.rpf"]).success();
    assert!(listing.lines().iter().any(|l| l.starts_with("dir ") && l.ends_with("newdir/")), "{}", listing.stdout);
}

#[test]
fn mv_renames_a_file_inside_the_archive() {
    let s = sandbox_with_files();
    s.run(&["mv", "common.rpf/data/hello.txt", "common.rpf/data/renamed.txt"]).success();
    let listing = s.run(&["ls", "mods/common.rpf/data"]).success();
    assert!(listing.stdout.contains("renamed.txt"), "{}", listing.stdout);
    assert!(!listing.stdout.contains("hello.txt"), "{}", listing.stdout);
    assert_eq!(mods_archive(&s, "data/renamed.txt"), common::HELLO);
}

#[test]
fn mv_moves_a_file_into_another_folder() {
    let s = sandbox_with_files();
    s.run(&["mkdir", "common.rpf/newdir"]).success();
    s.run(&["mv", "common.rpf/data/hello.txt", "common.rpf/newdir/hello.txt"]).success();
    let listing = s.run(&["ls", "-r", "mods/common.rpf"]).success();
    assert!(listing.lines().iter().any(|l| l.ends_with("newdir/hello.txt")), "{}", listing.stdout);
    assert!(!listing.lines().iter().any(|l| l.ends_with("data/hello.txt")), "{}", listing.stdout);
}

#[test]
fn mv_of_a_missing_entry_fails() {
    let s = sandbox_with_files();
    let out = s.run(&["mv", "common.rpf/nope", "common.rpf/x"]).failure();
    assert_eq!(out.stderr, "error: path not found: nope\n");
}

#[test]
fn history_lists_the_changes_newest_first() {
    let s = sandbox_with_files();
    s.run(&["put", &path_str(&s.path("new.txt")), "common.rpf/data/new.txt"]).success();
    s.run(&["rm", "common.rpf/data/config.xml"]).success();
    let out = s.run(&["history"]).success();
    assert_eq!(
        out.lines(),
        [
            "#3    common.rpf  Removed common.rpf/data/config.xml",
            "#2    common.rpf  Imported new.txt into common.rpf/data/new.txt",
            "#1    common.rpf  Copied common.rpf into the mods folder",
        ]
    );
}

#[test]
fn history_json_has_ids_and_descriptions() {
    let s = sandbox_with_files();
    s.run(&["put", &path_str(&s.path("new.txt")), "common.rpf/data/new.txt"]).success();
    let out = s.run(&["--json", "history"]).success();
    let v: serde_json::Value = serde_json::from_str(&out.stdout).unwrap();
    let rows = v.as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["id"], 2);
    assert_eq!(rows[0]["description"], "Imported new.txt into common.rpf/data/new.txt");
    assert_eq!(rows[0]["path"], "common.rpf");
    assert_eq!(rows[0]["in_game_folder"], false);
    assert_eq!(rows[1]["description"], "Copied common.rpf into the mods folder");
}

#[test]
fn history_without_a_mods_folder_says_so() {
    let s = Sandbox::with_game();
    assert_eq!(s.run(&["history"]).success().lines(), ["No mods folder."]);
}

#[test]
fn undo_reverts_the_last_change() {
    let s = sandbox_with_files();
    s.run(&["put", &path_str(&s.path("replacement.txt")), "common.rpf/data/hello.txt"]).success();
    s.run(&["rm", "common.rpf/data/config.xml"]).success();
    let out = s.run(&["undo"]).success();
    assert_eq!(out.lines(), ["Undone: Removed common.rpf/data/config.xml (common.rpf)"]);
    let listing = s.run(&["ls", "mods/common.rpf/data"]).success();
    assert!(listing.stdout.contains("config.xml"), "{}", listing.stdout);
    assert_eq!(mods_archive(&s, "data/hello.txt"), b"replaced\n");

    let out = s.run(&["undo"]).success();
    assert_eq!(out.lines(), ["Undone: Imported replacement.txt into common.rpf/data/hello.txt (common.rpf)"]);
    assert_eq!(mods_archive(&s, "data/hello.txt"), common::HELLO);
    assert_eq!(s.run(&["history"]).success().lines().len(), 1);
}

#[test]
fn undo_without_changes_says_nothing_to_undo() {
    let s = Sandbox::with_game();
    assert_eq!(s.run(&["undo"]).success().lines(), ["Nothing to undo."]);
}

#[test]
fn mods_lists_the_replaced_archives() {
    let s = sandbox_with_files();
    s.run(&["put", &path_str(&s.path("new.txt")), "common.rpf/data/new.txt"]).success();
    let out = s.run(&["mods"]).success();
    assert_eq!(out.lines()[0], format!("1 files in {}", path_str(&s.game.join("mods"))));
    assert!(out.lines()[1].starts_with("replaces ") && out.lines()[1].ends_with("  common.rpf"), "{}", out.stdout);
}

#[test]
fn mods_changes_compares_the_archives_with_the_original() {
    let s = sandbox_with_files();
    s.run(&["put", &path_str(&s.path("new.txt")), "common.rpf/data/new.txt"]).success();
    s.run(&["rm", "common.rpf/data/config.xml"]).success();
    let out = s.run(&["mods", "--changes"]).success();
    assert!(out.stdout.contains("added                 9B  common.rpf/data/new.txt"), "{}", out.stdout);
    assert!(out.stdout.contains("common.rpf/data/config.xml"), "{}", out.stdout);
    assert!(out.stderr.contains("2 changes ("), "{}", out.stderr);
}

#[test]
fn mods_without_a_mods_folder_says_so() {
    let s = Sandbox::with_game();
    let out = s.run(&["mods"]).success();
    assert_eq!(out.lines(), [format!("No mods folder in {}.", path_str(&s.game))]);
}

#[test]
fn originals_reports_that_nothing_was_changed_in_place() {
    let s = Sandbox::with_game();
    assert_eq!(s.run(&["originals"]).success().lines(), ["No game files were changed in place."]);
}

#[test]
fn write_target_shows_and_validates_the_target() {
    let s = Sandbox::with_game();
    let shown = "Changes go into the mods folder; the game folder stays untouched.";
    assert_eq!(s.run(&["write-target"]).success().lines(), [shown]);
    assert_eq!(s.run(&["write-target", "mods"]).success().lines(), [shown]);
    assert_eq!(
        s.run(&["write-target", "game"]).failure().stderr,
        "error: writing into the game folder is only offered for GTA IV\n"
    );
    assert_eq!(
        s.run(&["write-target", "bogus"]).failure().stderr,
        "error: unknown write target \"bogus\": use `mods` or `game`\n"
    );
}

#[test]
fn textures_set_replaces_pixel_data_in_the_mods_copy() {
    let s = Sandbox::with_game();
    let dir = s.path("tex");
    s.run(&["textures", "common.rpf/textures/test.ytd", "-o", &path_str(&dir)]).success();
    let dds = dir.join("alpha.dds");
    let mut bytes = std::fs::read(&dds).unwrap();
    let n = bytes.len();
    bytes[n - 32..].fill(9);
    std::fs::write(&dds, bytes).unwrap();

    let arg = format!("alpha={}", path_str(&dds));
    let out = s.run(&["textures", "common.rpf/textures/test.ytd", "--set", &arg]).success();
    assert_eq!(
        out.lines()[0],
        "common.rpf/textures/test.ytd: 1 replaced/added (0 new), 0 removed — pixel data replaced in place"
    );
    assert!(out.lines()[1].starts_with("Written to "), "{}", out.stdout);

    let after = s.run(&["textures", "mods/common.rpf/textures/test.ytd", "-o", &path_str(&s.path("after"))]).success();
    assert!(after.stdout.contains("alpha"), "{}", after.stdout);
    let alpha = std::fs::read(s.path("after/alpha.dds")).unwrap();
    assert!(alpha.ends_with(&[9u8; 32]));
    assert_eq!(s.run(&["history"]).success().lines()[0], "#2    common.rpf  Put texture alpha into test.ytd");
}

#[test]
fn textures_remove_drops_a_texture_from_the_mods_copy() {
    let s = Sandbox::with_game();
    let out = s.run(&["textures", "common.rpf/textures/test.ytd", "--remove", "beta"]).success();
    assert_eq!(
        out.lines()[0],
        "common.rpf/textures/test.ytd: 0 replaced/added (0 new), 1 removed — dictionary rebuilt"
    );
    let after = s.run(&["textures", "mods/common.rpf/textures/test.ytd"]).success();
    assert_eq!(after.lines().len(), 1);
    assert!(after.lines()[0].starts_with("alpha "), "{}", after.stdout);
}

#[test]
fn textures_set_needs_name_equals_file() {
    let s = Sandbox::with_game();
    let out = s.run(&["textures", "common.rpf/textures/test.ytd", "--set", "alpha"]).failure();
    assert_eq!(out.stderr, "error: --set expects name=file.dds\n");
}

#[test]
fn repack_writes_a_new_archive_with_the_edits() {
    let s = sandbox_with_files();
    let out_file = s.path("repacked.rpf");
    let set = format!("data/added.txt={}", path_str(&s.path("new.txt")));
    let out = s
        .run(&["repack", "common.rpf", "-o", &path_str(&out_file), "--set", &set, "--remove", "data/hello.txt"])
        .success();
    assert!(out.stdout.starts_with(&format!("Wrote {} (", path_str(&out_file))), "{}", out.stdout);
    assert!(out.stdout.trim_end().ends_with(") with 2 edits."), "{}", out.stdout);
    let listing = s.run(&["ls", "-r", &path_str(&out_file)]).success();
    assert!(listing.lines().iter().any(|l| l.ends_with("data/added.txt")), "{}", listing.stdout);
    assert!(!listing.stdout.contains("hello.txt"), "{}", listing.stdout);
}

#[test]
fn repack_of_something_that_is_no_archive_fails() {
    let s = Sandbox::with_game();
    let out = s.run(&["repack", "common.rpf/data", "-o", &path_str(&s.path("x.rpf"))]).failure();
    assert_eq!(out.stderr, "error: common.rpf/data is not an archive\n");
}
