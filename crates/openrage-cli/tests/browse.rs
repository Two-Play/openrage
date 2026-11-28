//! The `dirs` crate ignores the environment on Windows, so the config and cache folders cannot be
//! redirected there; these tests run where they can.
#![cfg(unix)]

mod common;

use common::{CONFIG_XML, HELLO, Sandbox, path_str, read_dir_names, write};

#[test]
fn ls_lists_the_game_folder_with_kind_and_size() {
    let s = Sandbox::with_game();
    let out = s.run(&["ls"]).success();
    assert_eq!(out.lines().len(), 2);
    assert!(out.lines()[0].starts_with("rpf "), "{}", out.stdout);
    assert!(out.lines()[0].ends_with("common.rpf/"), "{}", out.stdout);
    assert_eq!(out.lines()[1], "file          13B       readme.txt");
    assert_eq!(out.stderr, "");
}

#[test]
fn ls_of_an_archive_lists_its_top_level() {
    let s = Sandbox::with_game();
    let out = s.run(&["ls", "common.rpf"]).success();
    let names: Vec<&str> = out.lines().iter().map(|l| l.rsplit("  ").next().unwrap()).collect();
    assert_eq!(names, ["data/", "inner.rpf/", "text/", "textures/"]);
}

#[test]
fn ls_of_a_folder_in_an_archive_lists_its_files() {
    let s = Sandbox::with_game();
    let out = s.run(&["ls", "common.rpf/data"]).success();
    assert_eq!(
        out.lines(),
        ["file          80B       config.xml", "file          31B       hello.txt", "file         196B       peds.pso",]
    );
}

#[test]
fn ls_recursive_walks_nested_archives() {
    let s = Sandbox::with_game();
    let out = s.run(&["ls", "-r", "common.rpf"]).success();
    assert!(out.lines().contains(&"file          12B       inner.rpf/deep/note.txt"), "{}", out.stdout);
    assert!(out.lines().contains(&"file          31B       data/hello.txt"), "{}", out.stdout);
    assert!(
        out.lines()
            .iter()
            .any(|l| l.starts_with("resource") && l.contains(" v13  ") && l.ends_with("textures/test.ytd"))
    );
    assert!(out.lines().iter().any(|l| l.starts_with("dir ") && l.ends_with("inner.rpf/deep/")));
}

#[test]
fn ls_of_a_nested_archive_lists_its_root() {
    let s = Sandbox::with_game();
    let out = s.run(&["ls", "common.rpf/inner.rpf"]).success();
    assert_eq!(out.lines(), ["dir            0B       deep/"]);
}

#[test]
fn ls_json_prints_one_array_of_objects() {
    let s = Sandbox::with_game();
    let out = s.run(&["--json", "ls", "common.rpf/data"]).success();
    let rows: serde_json::Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(
        rows,
        serde_json::json!([
            {"folder": false, "kind": "file", "path": "config.xml", "size": 80, "version": ""},
            {"folder": false, "kind": "file", "path": "hello.txt", "size": 31, "version": ""},
            {"folder": false, "kind": "file", "path": "peds.pso", "size": 196, "version": ""},
        ])
    );
}

#[test]
fn ls_json_of_the_game_folder_marks_folders() {
    let s = Sandbox::with_game();
    write(&s.game.join("loose/a.txt"), b"a");
    let out = s.run(&["--json", "ls"]).success();
    let rows: serde_json::Value = serde_json::from_str(&out.stdout).unwrap();
    let rows = rows.as_array().unwrap();
    let kinds: Vec<(&str, bool)> =
        rows.iter().map(|r| (r["path"].as_str().unwrap(), r["folder"].as_bool().unwrap())).collect();
    assert_eq!(kinds, [("common.rpf", true), ("loose", true), ("readme.txt", false)]);
    assert_eq!(rows[1]["kind"], "dir");
}

#[test]
fn ls_of_a_file_inside_an_archive_describes_the_file() {
    let s = Sandbox::with_game();
    let out = s.run(&["ls", "common.rpf/data/hello.txt"]).success();
    assert_eq!(out.lines(), ["file          31B       data/hello.txt"]);
}

#[test]
fn ls_of_a_disk_file_prints_its_path() {
    let s = Sandbox::with_game();
    let out = s.run(&["ls", "readme.txt"]).success();
    let path = path_str(&s.game.join("readme.txt"));
    assert_eq!(out.lines(), [format!("file          13B       {path}")]);
}

#[test]
fn ls_of_a_disk_folder_lists_its_entries() {
    let s = Sandbox::with_game();
    write(&s.game.join("loose/sub/a.txt"), b"a");
    let out = s.run(&["ls", "loose"]).success();
    assert_eq!(out.lines().len(), 1);
    assert!(out.lines()[0].starts_with("dir "), "{}", out.stdout);
    assert!(out.lines()[0].ends_with("sub/"), "{}", out.stdout);
}

#[test]
fn ls_of_a_missing_path_fails_with_the_path_in_the_message() {
    let s = Sandbox::with_game();
    let out = s.run(&["ls", "nothere"]).failure();
    assert_eq!(out.stderr, "error: path not found: nothere\n");
    assert_eq!(out.stdout, "");
    let out = s.run(&["ls", "common.rpf/nonexistent"]).failure();
    assert_eq!(out.stderr, "error: path not found: common.rpf/nonexistent\n");
}

#[test]
fn ls_of_a_file_that_is_not_an_archive_names_the_magic() {
    let s = Sandbox::with_game();
    write(&s.game.join("bad.rpf"), b"not an archive at all");
    let out = s.run(&["ls", "bad.rpf"]).failure();
    assert_eq!(
        out.stderr,
        format!("error: {}: not an RPF7 archive (magic 0x20746f6e)\n", path_str(&s.game.join("bad.rpf")))
    );
}

#[test]
fn ls_of_a_truncated_archive_reports_the_damaged_table() {
    let s = Sandbox::with_game();
    write(&s.game.join("trunc.rpf"), &common::sample_archive()[..40]);
    let out = s.run(&["ls", "trunc.rpf"]).failure();
    assert!(out.stderr.contains("corrupt table of contents: table of contents exceeds archive size"), "{}", out.stderr);
}

#[test]
fn commands_without_a_game_folder_say_how_to_configure_one() {
    let s = Sandbox::new();
    let out = s.run_without_game(&["ls"]).failure();
    assert_eq!(out.stderr, "error: no game directory configured; run `openrage --game <DIR> keys` first\n");
    assert_eq!(out.stdout, "");
    let out = s.run_without_game(&["keys"]).failure();
    assert_eq!(out.stderr, "error: no game directory given; pass --game <DIR>\n");
}

#[test]
fn the_game_folder_can_come_from_the_environment() {
    let s = Sandbox::with_game();
    let mut c = s.command();
    c.env("OPENRAGE_GAME_DIR", &s.game).arg("ls");
    let out = common::Out::from(c.output().unwrap()).success();
    assert_eq!(out.lines().len(), 2);
}

#[test]
fn keys_without_an_executable_in_the_game_folder_fails() {
    let s = Sandbox::with_game();
    let out = s.run(&["keys"]).failure();
    assert_eq!(out.stderr, "error: no GTA5_Enhanced.exe or GTA5.exe in the game directory\n");
}

#[test]
fn an_unknown_subcommand_is_a_usage_error() {
    let s = Sandbox::new();
    let out = s.run_without_game(&["bogus"]).failure();
    assert!(out.stderr.starts_with("error: unrecognized subcommand 'bogus'"), "{}", out.stderr);
}

#[test]
fn help_describes_the_tool() {
    let s = Sandbox::new();
    let out = s.run_without_game(&["--help"]).success();
    assert_eq!(out.lines()[0], "Open-source explorer for RAGE engine game files");
    assert!(out.stdout.contains("--game <GAME>"));
}

#[test]
fn info_of_an_archive_prints_format_encryption_and_entry_counts() {
    let s = Sandbox::with_game();
    let out = s.run(&["info", "common.rpf"]).success();
    let l = out.lines();
    assert_eq!(l[0], format!("Archive:     {}", path_str(&s.game.join("common.rpf"))));
    assert_eq!(l[1], "Format:      RPF7");
    assert_eq!(l[3], "Encryption:  Open");
    assert_eq!(l[4], "Entries:     10 (4 dirs, 4 files, 1 resources, 1 archives)");
}

#[test]
fn info_of_a_nested_archive_counts_its_entries() {
    let s = Sandbox::with_game();
    let out = s.run(&["info", "common.rpf/inner.rpf"]).success();
    assert!(out.stdout.contains("Entries:     3 (2 dirs, 1 files, 0 resources, 0 archives)"), "{}", out.stdout);
}

#[test]
fn info_of_a_file_shows_its_kind_and_size() {
    let s = Sandbox::with_game();
    let out = s.run(&["info", "common.rpf/data/hello.txt"]).success();
    assert!(out.stdout.contains("Kind:        file"), "{}", out.stdout);
    assert!(out.stdout.contains("size: 31"), "{}", out.stdout);
}

#[test]
fn info_of_a_resource_shows_its_version() {
    let s = Sandbox::with_game();
    let out = s.run(&["info", "common.rpf/textures/test.ytd"]).success();
    assert!(out.stdout.contains("Kind:        resource"), "{}", out.stdout);
    assert!(out.stdout.contains("Resource version: 13"), "{}", out.stdout);
}

#[test]
fn info_of_a_folder_in_an_archive_says_dir() {
    let s = Sandbox::with_game();
    let out = s.run(&["info", "common.rpf/data"]).success();
    assert!(out.stdout.contains("Kind:        dir"), "{}", out.stdout);
}

#[test]
fn info_of_a_disk_file_says_it_is_not_inside_an_archive() {
    let s = Sandbox::with_game();
    let out = s.run(&["info", "readme.txt"]).success();
    assert_eq!(out.lines(), [format!("{} is not inside an archive", path_str(&s.game.join("readme.txt")))]);
}

#[test]
fn info_of_a_damaged_archive_fails() {
    let s = Sandbox::with_game();
    write(&s.game.join("bad.rpf"), b"not an archive at all");
    let out = s.run(&["info", "bad.rpf"]).failure();
    assert!(out.stderr.contains("not an RPF7 archive"), "{}", out.stderr);
}

#[test]
fn extract_writes_one_file_from_an_archive() {
    let s = Sandbox::with_game();
    let out_dir = s.path("out");
    let out = s.run(&["extract", "common.rpf/data/hello.txt", "-o", &path_str(&out_dir)]).success();
    assert_eq!(out.lines(), [format!("Extracted 1 files (31B) to {}", path_str(&out_dir.join("hello.txt")))]);
    assert_eq!(std::fs::read(out_dir.join("hello.txt")).unwrap(), HELLO);
}

#[test]
fn extract_writes_a_folder_of_an_archive() {
    let s = Sandbox::with_game();
    let out_dir = s.path("out");
    let out = s.run(&["extract", "common.rpf/data", "-o", &path_str(&out_dir)]).success();
    assert!(out.stdout.starts_with("Extracted 3 files (307B) to "), "{}", out.stdout);
    assert_eq!(read_dir_names(&out_dir.join("data")), ["config.xml", "hello.txt", "peds.pso"]);
    assert_eq!(std::fs::read(out_dir.join("data/config.xml")).unwrap(), CONFIG_XML);
}

#[test]
fn extract_a_whole_archive_keeps_nested_archives_as_files() {
    let s = Sandbox::with_game();
    let out_dir = s.path("out");
    s.run(&["extract", "common.rpf", "-o", &path_str(&out_dir)]).success();
    let root = out_dir.join("common.rpf");
    assert_eq!(read_dir_names(&root), ["data", "inner.rpf", "text", "textures"]);
    assert!(root.join("inner.rpf").is_file());
    assert_eq!(std::fs::read(root.join("data/hello.txt")).unwrap(), HELLO);
    assert_eq!(std::fs::read(root.join("text/american.gxt2")).unwrap(), common::gxt2_table());
}

#[test]
fn extract_with_expand_archives_unpacks_nested_archives() {
    let s = Sandbox::with_game();
    let out_dir = s.path("out");
    s.run(&["extract", "common.rpf", "-o", &path_str(&out_dir), "--expand-archives"]).success();
    let note = out_dir.join("common.rpf/inner.rpf/deep/note.txt");
    assert_eq!(std::fs::read(note).unwrap(), b"nested note\n");
}

#[test]
fn extract_a_resource_writes_an_rsc7_file() {
    let s = Sandbox::with_game();
    let out_dir = s.path("out");
    s.run(&["extract", "common.rpf/textures/test.ytd", "-o", &path_str(&out_dir)]).success();
    let bytes = std::fs::read(out_dir.join("test.ytd")).unwrap();
    assert_eq!(&bytes[0..4], b"RSC7");
}

#[test]
fn extract_of_a_missing_entry_fails() {
    let s = Sandbox::with_game();
    let out = s.run(&["extract", "common.rpf/data/nope", "-o", &path_str(&s.path("out"))]).failure();
    assert_eq!(out.stderr, "error: path not found: common.rpf/data/nope\n");
}

#[test]
fn extract_of_a_disk_folder_is_refused() {
    let s = Sandbox::with_game();
    write(&s.game.join("loose/a.txt"), b"a");
    let out = s.run(&["extract", "loose", "-o", &path_str(&s.path("out"))]).failure();
    assert_eq!(out.stderr, "error: loose is not inside an archive\n");
}

#[test]
fn find_matches_names_in_every_archive() {
    let s = Sandbox::with_game();
    let out = s.run(&["find", "hello"]).success();
    assert_eq!(out.lines(), ["common.rpf/data/hello.txt"]);
    assert!(out.stderr.contains("1 matches in 11 entries"), "{}", out.stderr);
}

#[test]
fn find_can_be_limited_to_a_folder() {
    let s = Sandbox::with_game();
    let out = s.run(&["find", "txt", "--in", "common.rpf/data"]).success();
    assert_eq!(out.lines(), ["common.rpf/data/hello.txt"]);
    assert!(out.stderr.contains("1 matches in 3 entries"), "{}", out.stderr);
}

#[test]
fn find_warns_about_damaged_archives_and_goes_on() {
    let s = Sandbox::with_game();
    write(&s.game.join("bad.rpf"), b"not an archive at all");
    let out = s.run(&["find", "hello"]).success();
    assert_eq!(out.lines(), ["common.rpf/data/hello.txt"]);
    assert!(out.stderr.contains("warning: ") && out.stderr.contains("not an RPF7 archive"), "{}", out.stderr);
}

#[test]
fn find_without_a_match_prints_only_the_count() {
    let s = Sandbox::with_game();
    let out = s.run(&["find", "zzz"]).success();
    assert_eq!(out.stdout, "");
    assert!(out.stderr.contains("0 matches in 11 entries"), "{}", out.stderr);
}
