//! The `dirs` crate ignores the environment on Windows, so the config and cache folders cannot be
//! redirected there; these tests run where they can.
#![cfg(unix)]

mod common;

use common::Sandbox;

fn indexed() -> Sandbox {
    let s = Sandbox::with_game();
    s.run(&["index"]).success();
    s
}

#[test]
fn index_reports_the_archives_and_entries_it_read() {
    let s = Sandbox::with_game();
    let out = s.run(&["index"]).success();
    assert!(out.lines()[0].starts_with("Indexed 1 archives, removed 0, unchanged 0 in "), "{}", out.stdout);
    assert!(out.lines()[0].ends_with("s (0 read errors)."), "{}", out.stdout);
    assert!(
        out.lines()[1].starts_with("Index: 1 archives, 13 entries, 5 text documents, 15 dictionary strings, "),
        "{}",
        out.stdout
    );
    assert!(out.stderr.contains("[1/1] common.rpf (11 entries)"), "{}", out.stderr);
}

#[test]
fn index_is_kept_in_the_cache_folder_of_the_sandbox() {
    let s = Sandbox::with_game();
    let out = s.run(&["index"]).success();
    let cache = s.app_cache();
    assert!(out.stdout.contains(&common::path_str(&cache)), "{}", out.stdout);
    assert!(common::read_dir_names(&cache).iter().any(|n| n.starts_with("index-") && n.ends_with(".sqlite")));
}

#[test]
fn indexing_again_skips_unchanged_archives() {
    let s = indexed();
    let out = s.run(&["index"]).success();
    assert!(out.lines()[0].starts_with("Indexed 0 archives, removed 0, unchanged 1 in "), "{}", out.stdout);
}

#[test]
fn indexing_notices_a_removed_archive() {
    let s = indexed();
    std::fs::remove_file(s.game.join("common.rpf")).unwrap();
    let out = s.run(&["index"]).success();
    assert!(out.lines()[0].starts_with("Indexed 0 archives, removed 1, unchanged 0 in "), "{}", out.stdout);
}

#[test]
fn search_by_name_finds_the_file() {
    let s = indexed();
    let out = s.run(&["search", "peds"]).success();
    assert_eq!(out.lines(), ["joaat(\"peds\") = 0x8DA12117", "name      196B  common.rpf/data/peds.pso"]);
    assert!(out.stderr.starts_with("1 hits in "), "{}", out.stderr);
}

#[test]
fn search_finds_text_strings_and_names_together() {
    let s = indexed();
    let out = s.run(&["search", "hello"]).success();
    assert_eq!(
        out.lines(),
        [
            "joaat(\"hello\") = 0xC8FD181B",
            "name       31B  common.rpf/data/hello.txt",
            "text       56B  common.rpf/text/american.gxt2",
            "      └ 0x827DFA62 Hello there",
        ]
    );
    assert!(out.stderr.starts_with("2 hits in "), "{}", out.stderr);
}

#[test]
fn search_by_hash_finds_the_text_entry() {
    let s = indexed();
    let out = s.run(&["search", "0x827DFA62"]).success();
    assert_eq!(
        out.lines(),
        [
            "Hash 0x827DFA62 = 2189294178 (signed -2105673118)",
            "  no known string for this hash",
            "text       56B  common.rpf/text/american.gxt2",
            "      └ 0x827DFA62 Hello there",
        ]
    );
}

#[test]
fn search_for_a_hash_without_hits_says_so() {
    let s = indexed();
    let out = s.run(&["search", "0x2D9E6D6D"]).success();
    assert_eq!(out.lines(), ["Hash 0x2D9E6D6D = 765357421 (signed 765357421)", "  no known string for this hash"]);
    assert!(out.stderr.starts_with("0 hits in "), "{}", out.stderr);
}

#[test]
fn search_without_a_match_prints_the_label_hash_only() {
    let s = indexed();
    let out = s.run(&["search", "zzzz"]).success();
    assert_eq!(out.lines(), ["joaat(\"zzzz\") = 0x16B09C64"]);
    assert!(out.stderr.starts_with("0 hits in "), "{}", out.stderr);
}

#[test]
fn search_json_describes_the_hits() {
    let s = indexed();
    let out = s.run(&["--json", "search", "config"]).success();
    let v: serde_json::Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(
        v,
        serde_json::json!({
            "hash": null,
            "hash_strings": [],
            "hits": [{"line": null, "match": "name", "path": "common.rpf/data/config.xml", "size": 80}],
            "limited": false,
            "query": "config",
            "query_hash": "0x4B5D12FD",
        })
    );
}

#[test]
fn search_limit_cuts_the_list() {
    let s = indexed();
    let out = s.run(&["--json", "search", "hello", "-n", "1"]).success();
    let v: serde_json::Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(v["hits"].as_array().unwrap().len(), 1);
    assert_eq!(v["limited"], true);
}

#[test]
fn search_language_all_includes_every_language() {
    let s = indexed();
    let out = s.run(&["search", "Hello there", "--lang", "all"]).success();
    assert!(out.stdout.contains("      └ 0x827DFA62 Hello there"), "{}", out.stdout);
}

fn with_german_texts() -> Sandbox {
    let s = Sandbox::with_game();
    let mut w = rage_rpf::ArchiveWriter::new();
    let table =
        rage_formats::gxt2::write(&[rage_formats::gxt2::Entry { hash: 0x1111_2222, text: "Hallo Welt".into() }]);
    w.add_file("global.gxt2", table).unwrap();
    common::write(&s.game.join("german_final.rpf"), &w.build().unwrap());
    s.run(&["index"]).success();
    s
}

#[test]
fn search_language_keeps_only_that_languages_texts() {
    let s = with_german_texts();
    let out = s.run(&["search", "Hallo", "--lang", "german"]).success();
    assert!(out.stdout.contains("german_final.rpf/global.gxt2"), "{}", out.stdout);
    assert!(out.stdout.contains("      └ 0x11112222 Hallo Welt"), "{}", out.stdout);
    let out = s.run(&["search", "Hallo", "--lang", "french"]).success();
    assert!(!out.stdout.contains("Hallo Welt"), "{}", out.stdout);
    let out = s.run(&["search", "Hallo", "--lang", "all"]).success();
    assert!(out.stdout.contains("Hallo Welt"), "{}", out.stdout);
}

#[test]
fn search_language_follows_the_locale_of_the_environment() {
    let s = with_german_texts();
    let mut c = s.command();
    c.env("LANG", "fr_FR.UTF-8").arg("--game").arg(&s.game).args(["search", "Hallo"]);
    let out = common::Out::from(c.output().unwrap()).success();
    assert!(!out.stdout.contains("Hallo Welt"), "{}", out.stdout);
    let mut c = s.command();
    c.env("LANG", "de_DE.UTF-8").arg("--game").arg(&s.game).args(["search", "Hallo"]);
    let out = common::Out::from(c.output().unwrap()).success();
    assert!(out.stdout.contains("Hallo Welt"), "{}", out.stdout);
}

#[test]
fn search_with_an_unknown_language_fails() {
    let s = indexed();
    let out = s.run(&["search", "x", "--lang", "klingon"]).failure();
    assert!(out.stderr.starts_with("error: unknown language \"klingon\"; use one of "), "{}", out.stderr);
    assert!(out.stderr.trim_end().ends_with("or `all`"), "{}", out.stderr);
}
