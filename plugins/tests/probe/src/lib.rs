//! Test plugin: each input's file name says what to do, its data is the
//! argument. Exercises the host's limits and checks.

use openrage_plugin_sdk::prelude::*;
use openrage_plugin_sdk::{file_name, host};

struct Probe;

impl Plugin for Probe {
    fn preview(vpath: String, data: Vec<u8>) -> Result<Preview, String> {
        log(Level::Info, format!("previewing {vpath}"));
        println!("stdout line");
        let ext = vpath.rsplit('.').next().unwrap_or_default();
        Ok(match ext {
            "tree" => Preview::tree(vec![TreeNode::new(None, "root", ""), TreeNode::new(Some(0), "child", "1")]),
            "table" => Preview::table(vec!["a".into(), "b".into()], vec![vec!["1".into(), "2".into()]]),
            "image" => Preview::image(2, 2, vec![255; 16]),
            "badimage" => Preview::image(2, 2, vec![255; 3]),
            _ => Preview::text(format!("{} bytes", data.len())).with("Size", data.len()),
        })
    }

    fn run_action(action: String, inputs: Vec<Input>) -> Result<Vec<OutputFile>, String> {
        if action != "run" && action != "modify" {
            return Err(format!("unknown action {action:?}"));
        }
        let mut out = Vec::new();
        for input in inputs {
            let arg = String::from_utf8_lossy(&input.data).into_owned();
            match file_name(&input.vpath) {
                "read" => out.push(OutputFile::new("read.bin", host::read_file(&arg)?)),
                "list" => {
                    let names: Vec<String> = host::list_dir(&arg)?.into_iter().map(|e| e.name).collect();
                    out.push(OutputFile::new("list.txt", names.join("\n")));
                }
                "game" => out.push(OutputFile::new("game.txt", format!("{:?}", host::current_game()))),
                "escape" => out.push(OutputFile::new("../escape.txt", "x")),
                // Writes the argument into a game file, twice (the last one counts).
                "write" => {
                    host::write_mod_file("a.rpf/x.txt", b"first")?;
                    host::write_mod_file("a.rpf/x.txt", arg.as_bytes())?;
                }
                // Writes "written by probe" to the path given as argument.
                "write-to" => host::write_mod_file(arg.trim(), b"written by probe")?,
                "write-escape" => host::write_mod_file("../outside.txt", b"x")?,
                "write-then-fail" => {
                    host::write_mod_file("a.rpf/x.txt", b"x")?;
                    return Err("failed after writing".into());
                }
                "env" => {
                    let vars: Vec<String> = std::env::vars().map(|(k, _)| k).collect();
                    let files = std::fs::read_dir("/").map(|d| d.count()).unwrap_or(0);
                    out.push(OutputFile::new("env.txt", format!("vars={} root-entries={files}", vars.len())));
                }
                "panic" => panic!("probe panicked"),
                "spin" => {
                    let mut n: u64 = 0;
                    loop {
                        n = std::hint::black_box(n.wrapping_add(1));
                    }
                }
                "alloc" => {
                    let mut hog: Vec<Vec<u8>> = Vec::new();
                    loop {
                        hog.push(vec![1; 64 << 20]);
                        std::hint::black_box(&hog);
                    }
                }
                other => return Err(format!("unknown probe {other:?}")),
            }
        }
        Ok(out)
    }
}

openrage_plugin_sdk::export_plugin!(Probe);
