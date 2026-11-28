//! The `dirs` crate ignores the environment on Windows, so the config and cache folders cannot be
//! redirected there; these tests run where they can.
#![cfg(unix)]

mod common;

use common::{Sandbox, path_str, write};

const MANIFEST: &str = r#"
id = "demo"
name = "Demo"
version = "1.0.0"
api = "0.3"
description = "A demonstration plugin."
authors = ["Someone"]
license = "MIT"
permissions = ["read-game"]

[[tools]]
id = "conv"
title = "Converter"
[[tools.fields]]
id = "value"
label = "Value"
[[tools.fields]]
id = "as"
label = "As"
kind = "choice"
choices = ["a", "b"]
[[tools.fields]]
id = "signed"
label = "Signed"
kind = "check"
"#;

fn with_plugin() -> Sandbox {
    let s = Sandbox::new();
    let dir = s.app_config().join("plugins").join("demo");
    write(&dir.join("plugin.toml"), MANIFEST.as_bytes());
    write(&dir.join("plugin.wasm"), b"not a component");
    s
}

#[test]
fn plugin_list_without_plugins_names_the_user_folder() {
    let s = Sandbox::new();
    let out = s.run_without_game(&["plugin", "list"]).success();
    let user = s.app_config().join("plugins");
    assert_eq!(out.lines()[0], "Plugin folders:");
    assert_eq!(out.lines()[1], format!("  {}", path_str(&user)));
    assert_eq!(out.lines().last().unwrap(), &"No plugins installed.");
}

#[test]
fn plugin_list_shows_an_installed_plugin_with_its_status() {
    let s = with_plugin();
    let out = s.run_without_game(&["plugin", "list"]).success();
    let row = out.stdout.lines().find(|l| l.starts_with("demo")).expect(&out.stdout);
    assert_eq!(row.split_whitespace().collect::<Vec<_>>(), ["demo", "1.0.0", "disabled", "Demo"]);
}

#[test]
fn plugin_list_works_with_a_game_folder_too() {
    let s = with_plugin();
    let out = s.run(&["plugin", "list"]).success();
    assert!(out.stdout.contains("demo"), "{}", out.stdout);
}

#[test]
fn plugin_info_shows_manifest_details() {
    let s = with_plugin();
    let out = s.run_without_game(&["plugin", "info", "demo"]).success();
    let l = out.lines();
    assert_eq!(l[0], "Demo 1.0.0 (demo)");
    assert_eq!(l[1], "A demonstration plugin.");
    assert!(l[2].starts_with("Folder:      ") && l[2].ends_with("demo"), "{}", out.stdout);
    assert_eq!(l[3], "Status:      disabled");
    assert_eq!(l[4], "Authors:     Someone");
    assert_eq!(l[5], "Licence:     MIT");
    assert_eq!(l[6], "Games:       all");
    assert!(l[7].starts_with("Permission:  read-game — "), "{}", out.stdout);
    assert_eq!(l[8], "Tool:        conv — Converter");
    assert_eq!(l[9], "               --set value=…  Value (text, starts as \"\")");
    assert_eq!(l[10], "               --set as=…  As (a/b, starts as \"a\")");
    assert_eq!(l[11], "               --set signed=…  Signed (true/false, starts as \"false\")");
}

#[test]
fn plugin_info_of_an_unknown_plugin_fails() {
    let s = Sandbox::new();
    let out = s.run_without_game(&["plugin", "info", "x"]).failure();
    assert_eq!(out.stderr, "error: no plugin \"x\" is installed (see `openrage plugin list`)\n");
}

#[test]
fn enable_needs_confirmation_when_permissions_are_asked() {
    let s = with_plugin();
    let out = s.run_without_game(&["plugin", "enable", "demo"]).failure();
    assert!(out.stdout.starts_with("Demo asks for:\n  read-game: "), "{}", out.stdout);
    assert_eq!(out.stderr, "error: Grant and enable it? needs confirmation: pass --yes to run without asking\n");
}

#[test]
fn enable_with_yes_grants_and_disable_revokes() {
    let s = with_plugin();
    let out = s.run_without_game(&["plugin", "enable", "demo", "--yes"]).success();
    assert_eq!(out.lines().last().unwrap(), &"Enabled Demo.");
    let list = s.run_without_game(&["plugin", "list"]).success();
    let row = list.stdout.lines().find(|l| l.starts_with("demo")).unwrap();
    assert!(row.contains("enabled"), "{row}");

    assert_eq!(s.run_without_game(&["plugin", "disable", "demo"]).success().lines(), ["Disabled demo."]);
    assert_eq!(s.run_without_game(&["plugin", "disable", "demo"]).success().lines(), ["demo was not enabled."]);
}

#[test]
fn enable_of_an_unknown_plugin_fails() {
    let s = Sandbox::new();
    let out = s.run_without_game(&["plugin", "enable", "nope", "--yes"]).failure();
    assert_eq!(out.stderr, "error: no plugin \"nope\" is installed (see `openrage plugin list`)\n");
}

#[test]
fn a_plugin_with_a_broken_manifest_is_listed_as_broken() {
    let s = Sandbox::new();
    let dir = s.app_config().join("plugins").join("bad");
    write(&dir.join("plugin.toml"), b"id = \"Bad Id\"\nname = \"Bad\"\nversion = \"1\"\napi = \"0.3\"\n");
    let out = s.run_without_game(&["plugin", "list"]).success();
    let line = out.stdout.lines().find(|l| l.starts_with("broken: ")).expect(&out.stdout);
    assert!(line.contains("id \"Bad Id\": use lowercase letters, digits and '-' (at most 64)"), "{line}");
}

#[test]
fn a_tool_of_a_disabled_plugin_is_refused() {
    let s = with_plugin();
    let out = s.run_without_game(&["plugin", "tool", "demo", "conv"]).failure();
    assert_eq!(out.stderr, "error: Demo is not enabled (`openrage plugin enable demo`)\n");
}

#[test]
fn a_tool_with_a_bad_field_value_is_refused_before_running() {
    let s = with_plugin();
    s.run_without_game(&["plugin", "enable", "demo", "--yes"]).success();
    let out = s.run_without_game(&["plugin", "tool", "demo", "conv", "--set", "as=z"]).failure();
    assert_eq!(out.stderr, "error: as: \"z\" is not one of a, b\n");
    let out = s.run_without_game(&["plugin", "tool", "demo", "conv", "--set", "signed=maybe"]).failure();
    assert_eq!(out.stderr, "error: signed: use true or false\n");
    let out = s.run_without_game(&["plugin", "tool", "demo", "conv", "--set", "nofield=1"]).failure();
    assert_eq!(out.stderr, "error: Converter has no field \"nofield\" (it has: value, as, signed)\n");
    let out = s.run_without_game(&["plugin", "tool", "demo", "conv", "--set", "value"]).failure();
    assert_eq!(out.stderr, "error: --set value: write it as FIELD=VALUE\n");
}

#[test]
fn an_unknown_tool_lists_the_tools_of_the_plugin() {
    let s = with_plugin();
    s.run_without_game(&["plugin", "enable", "demo", "--yes"]).success();
    let out = s.run_without_game(&["plugin", "tool", "demo", "other"]).failure();
    assert_eq!(out.stderr, "error: Demo has no tool \"other\" (it has: conv)\n");
}

#[test]
fn a_tool_whose_component_is_not_valid_fails_to_compile() {
    let s = with_plugin();
    s.run_without_game(&["plugin", "enable", "demo", "--yes"]).success();
    let out = s.run_without_game(&["plugin", "tool", "demo", "conv"]).failure();
    let wasm = path_str(&s.app_config().join("plugins/demo/plugin.wasm"));
    assert!(
        out.stderr.starts_with(&format!(
            "error: loading {wasm}: failed to parse WebAssembly module: magic header not detected"
        )),
        "{}",
        out.stderr
    );
    assert_eq!(out.stdout, "");
}

#[test]
fn preview_without_an_enabled_plugin_for_the_file_fails() {
    let s = Sandbox::with_game();
    let out = s.run(&["plugin", "preview", "common.rpf/data/hello.txt"]).failure();
    assert_eq!(out.stderr, "error: no enabled plugin previews common.rpf/data/hello.txt\n");
}

#[test]
fn preview_of_a_disk_file_without_a_plugin_fails_the_same_way() {
    let s = Sandbox::with_game();
    write(&s.path("x.bin"), b"data");
    let out = s.run_without_game(&["plugin", "preview", "--file", &path_str(&s.path("x.bin"))]).failure();
    assert_eq!(out.stderr, "error: no enabled plugin previews x.bin\n");
}

#[test]
fn pack_without_a_manifest_names_the_missing_file() {
    let s = Sandbox::new();
    let dir = s.path("empty");
    std::fs::create_dir_all(&dir).unwrap();
    let out = s.run_without_game(&["plugin", "pack", &path_str(&dir)]).failure();
    assert!(
        out.stderr.starts_with(&format!("error: reading {}", path_str(&dir.join("plugin.toml")))),
        "{}",
        out.stderr
    );
}

#[test]
fn install_of_a_file_that_is_no_package_fails() {
    let s = Sandbox::new();
    write(&s.path("junk.orp"), b"this is not a zip");
    let out = s.run_without_game(&["plugin", "install", &path_str(&s.path("junk.orp")), "--yes"]).failure();
    let junk = path_str(&s.path("junk.orp"));
    assert!(
        out.stderr.starts_with(&format!("error: {junk} is not a valid plugin package: not a ZIP file: ")),
        "{}",
        out.stderr
    );
}

#[test]
fn uninstall_needs_confirmation_and_refuses_unknown_plugins_quietly() {
    let s = Sandbox::new();
    let out = s.run_without_game(&["plugin", "uninstall", "demo"]).failure();
    assert_eq!(out.stderr, "error: Remove the plugin demo? needs confirmation: pass --yes to run without asking\n");
}

#[test]
fn uninstall_with_yes_removes_the_plugin_folder() {
    let s = with_plugin();
    let folder = s.app_config().join("plugins").join("demo");
    let out = s.run_without_game(&["plugin", "uninstall", "demo", "--yes"]).success();
    assert_eq!(out.lines(), [format!("Removed {}.", path_str(&folder))]);
    assert!(!folder.exists());
}

#[test]
fn licenses_lists_the_credits_and_dependencies() {
    let s = Sandbox::new();
    let out = s.run_without_game(&["licenses"]).success();
    assert!(out.lines()[0].starts_with("OpenRAGE ") && out.lines()[0].contains("GPL-3.0-only"), "{}", out.lines()[0]);
    assert!(out.stdout.contains("Credits (NOTICE.md):\n  CodeWalker\n"));
    assert!(out.stdout.contains(" third-party crates:\n"));
    assert!(out.stdout.contains("\n  anyhow "));
    assert!(out.stdout.trim_end().ends_with("`notice` for NOTICE.md) or `--all`."), "{}", out.stdout);
}

#[test]
fn licenses_text_prints_the_gpl() {
    let s = Sandbox::new();
    let out = s.run_without_game(&["licenses", "--text", "openrage"]).success();
    assert_eq!(out.lines()[0], "GNU GENERAL PUBLIC LICENSE");
    assert!(out.stdout.contains("Version 3, 29 June 2007"));
}

#[test]
fn licenses_text_notice_prints_the_notice() {
    let s = Sandbox::new();
    let out = s.run_without_game(&["licenses", "--text", "NOTICE"]).success();
    assert!(out.stdout.contains("CodeWalker"), "{}", out.stdout);
}

#[test]
fn licenses_text_of_a_dependency_prints_its_header_and_text() {
    let s = Sandbox::new();
    let out = s.run_without_game(&["licenses", "--text", "anyhow"]).success();
    assert!(
        out.lines()[0].starts_with("anyhow ") && out.lines()[0].contains(" — MIT OR Apache-2.0"),
        "{}",
        out.lines()[0]
    );
    assert!(out.stdout.len() > 200);
}

#[test]
fn licenses_text_of_an_unknown_crate_fails() {
    let s = Sandbox::new();
    let out = s.run_without_game(&["licenses", "--text", "nonexistent-crate"]).failure();
    assert_eq!(out.stderr, "error: no crate named nonexistent-crate; `openrage licenses` lists them\n");
}

#[test]
fn licenses_all_prints_a_section_per_crate() {
    let s = Sandbox::new();
    let out = s.run_without_game(&["licenses", "--all"]).success();
    assert!(out.stdout.starts_with("==== "), "{}", &out.stdout[..40]);
    assert!(out.stdout.contains("==== anyhow "));
}

#[test]
fn asi_and_oiv_need_the_right_game() {
    let s = Sandbox::with_game();
    let out = s.run(&["asi"]).failure();
    assert_eq!(out.stderr, format!("error: {} is not a GTA V folder: ASI mods are for GTA V\n", path_str(&s.game)));
    let out = s.run(&["oiv"]).failure();
    assert!(out.stderr.contains("OpenIV packages (.oiv)"), "{}", out.stderr);
}
