//! The `dirs` crate ignores the environment on Windows, so the config and cache folders cannot be
//! redirected there; these tests run where they can.
#![cfg(unix)]

mod common;

use common::{Sandbox, path_str, write};

#[test]
fn xml_prints_a_plain_xml_file_as_it_is() {
    let s = Sandbox::with_game();
    let out = s.run(&["xml", "common.rpf/data/config.xml"]).success();
    assert_eq!(
        out.lines(),
        ["<?xml version=\"1.0\" encoding=\"UTF-8\"?>", "<Config><Item name=\"a\">1</Item></Config>"]
    );
}

#[test]
fn xml_converts_a_pso_file_to_xml() {
    let s = Sandbox::with_game();
    let out = s.run(&["xml", "common.rpf/data/peds.pso"]).success();
    assert!(out.stdout.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<CPedModelInfo__InitDataList>\n"));
    assert!(out.stdout.contains("<Name>hash_B779A091</Name>\n   <Radius value=\"1.5\" />"), "{}", out.stdout);
    assert!(out.stdout.contains("<Radius value=\"-2\" />"), "{}", out.stdout);
}

#[test]
fn xml_reads_a_loose_file_on_disk() {
    let s = Sandbox::with_game();
    write(&s.game.join("loose.xml"), b"<Root><A/></Root>");
    let out = s.run(&["xml", "loose.xml"]).success();
    assert_eq!(out.lines(), ["<Root><A/></Root>"]);
}

#[test]
fn xml_writes_to_a_file_and_names_the_encoding() {
    let s = Sandbox::with_game();
    let file = s.path("peds.xml");
    let out = s.run(&["xml", "common.rpf/data/peds.pso", "-o", &path_str(&file)]).success();
    assert_eq!(out.lines(), [format!("Wrote {} (PSO, CPedModelInfo__InitDataList)", path_str(&file))]);
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("<Radius value=\"1.5\" />"));
}

#[test]
fn xml_writes_a_plain_file_with_an_unknown_root() {
    let s = Sandbox::with_game();
    let file = s.path("config.xml");
    let out = s.run(&["xml", "common.rpf/data/config.xml", "-o", &path_str(&file)]).success();
    assert_eq!(out.lines(), [format!("Wrote {} (XML text, ?)", path_str(&file))]);
    assert_eq!(std::fs::read(&file).unwrap(), common::CONFIG_XML);
}

#[test]
fn xml_tree_prints_structures_and_fields() {
    let s = Sandbox::with_game();
    let out = s.run(&["xml", "--tree", "common.rpf/data/peds.pso"]).success();
    assert_eq!(
        out.lines(),
        [
            "CPedModelInfo::InitDataList",
            "  Init datas (2)",
            "    · Item type: CPedModelInfo__InitData",
            "    #1 · hash_B779A091",
            "      · Name: hash_B779A091",
            "      · Radius: 1.5",
            "    #2 · hash_C1E908D2",
            "      · Name: hash_C1E908D2",
            "      · Radius: -2",
        ]
    );
    assert!(out.stderr.starts_with("4 structures, "), "{}", out.stderr);
}

#[test]
fn xml_of_a_file_that_is_not_metadata_fails() {
    let s = Sandbox::with_game();
    let out = s.run(&["xml", "common.rpf/data/hello.txt"]).failure();
    assert_eq!(out.stderr, "error: unknown metadata encoding\n");
}

#[test]
fn xml_of_a_missing_file_fails() {
    let s = Sandbox::with_game();
    let out = s.run(&["xml", "common.rpf/data/nope.xml"]).failure();
    assert_eq!(out.stderr, "error: path not found: common.rpf/data/nope.xml\n");
}

#[test]
fn xml_out_and_tree_cannot_be_combined() {
    let s = Sandbox::with_game();
    let out = s.run(&["xml", "common.rpf/data/config.xml", "--tree", "-o", "x.xml"]).failure();
    assert!(out.stderr.contains("cannot be used with"), "{}", out.stderr);
}

#[test]
fn text_prints_the_entries_of_a_text_table() {
    let s = Sandbox::with_game();
    let out = s.run(&["text", "common.rpf/text/american.gxt2"]).success();
    assert_eq!(out.lines(), ["Version 2 30", "{", "\t0x827DFA62 = Hello there", "\t0x8F88A90D = Goodbye now", "}"]);
}

#[test]
fn text_writes_the_oxt_form_to_a_file() {
    let s = Sandbox::with_game();
    let file = s.path("american.oxt");
    let out = s.run(&["text", "common.rpf/text/american.gxt2", "-o", &path_str(&file)]).success();
    assert_eq!(out.lines(), [format!("Wrote {} (2 entries)", path_str(&file))]);
    let text = std::fs::read_to_string(&file).unwrap().replace("\r\n", "\n");
    assert_eq!(text, "Version 2 30\n{\n\t0x827DFA62 = Hello there\n\t0x8F88A90D = Goodbye now\n}\n");
}

#[test]
fn text_of_a_file_that_is_no_text_table_fails() {
    let s = Sandbox::with_game();
    let out = s.run(&["text", "common.rpf/data/hello.txt"]).failure();
    assert_eq!(out.stderr, "error: not a text table (.gxt2, .nametable, .gxt) or placement file (.wpl)\n");
}

#[test]
fn textures_lists_the_dictionary() {
    let s = Sandbox::with_game();
    let out = s.run(&["textures", "common.rpf/textures/test.ytd"]).success();
    assert_eq!(
        out.lines(),
        [
            "alpha                                8x8     BC1            mips  1       32B  diffuse",
            "beta                                 4x4     B8G8R8A8       mips  1       64B  diffuse",
        ]
    );
}

#[test]
fn textures_exports_dds_files() {
    let s = Sandbox::with_game();
    let dir = s.path("tex");
    let out = s.run(&["textures", "common.rpf/textures/test.ytd", "-o", &path_str(&dir)]).success();
    assert!(out.stdout.ends_with(&format!("Exported 2 textures to {}\n", path_str(&dir))), "{}", out.stdout);
    assert_eq!(common::read_dir_names(&dir), ["alpha.dds", "beta.dds"]);
    let alpha = std::fs::read(dir.join("alpha.dds")).unwrap();
    assert_eq!(&alpha[0..4], b"DDS ");
    assert!(alpha.ends_with(&[1u8; 32]));
}

#[test]
fn textures_exports_png_files() {
    let s = Sandbox::with_game();
    let dir = s.path("tex");
    s.run(&["textures", "common.rpf/textures/test.ytd", "-o", &path_str(&dir), "--png"]).success();
    assert_eq!(common::read_dir_names(&dir), ["alpha.png", "beta.png"]);
    let png = std::fs::read(dir.join("beta.png")).unwrap();
    assert_eq!(&png[0..8], b"\x89PNG\r\n\x1a\n");
}

#[test]
fn textures_of_a_file_without_textures_fails() {
    let s = Sandbox::with_game();
    let out = s.run(&["textures", "common.rpf/data/hello.txt"]).failure();
    assert_eq!(out.stderr, "error: path not found: data/hello.txt is not a resource\n");
    assert_eq!(out.stdout, "");
}

#[test]
fn model_of_a_texture_dictionary_is_refused() {
    let s = Sandbox::with_game();
    let out = s.run(&["model", "common.rpf/textures/test.ytd"]).failure();
    assert_eq!(out.stderr, "error: test.ytd is not a model (.ydr, .yft, .ydd, .ypt, .ybn, .ynd, .yvr, .ywr)\n");
}

#[test]
fn model_of_a_missing_file_fails() {
    let s = Sandbox::with_game();
    let out = s.run(&["model", "common.rpf/nothing.ydr"]).failure();
    assert_eq!(out.stderr, "error: path not found: common.rpf/nothing.ydr\n");
}

#[test]
fn particles_of_a_plain_file_is_refused() {
    let s = Sandbox::with_game();
    let out = s.run(&["particles", "common.rpf/data/hello.txt"]).failure();
    assert_eq!(out.stderr, "error: path not found: data/hello.txt is not a resource\n");
}
