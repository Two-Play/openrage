//! Runs the example and test plugins (built from `plugins/` for
//! wasm32-wasip2) in the real host.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::{Result, bail};
use openrage_plugins::{
    DirEntry, GameFiles, GameId, Host, Input, Level, Limits, NoGame, Permission, Plugin, Preview, Registry, View,
};

/// Builds the plugins once and lays them out as plugin folders.
fn plugin_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins");
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let status = Command::new(cargo)
            .current_dir(&root)
            .args(["build", "--release", "--target", "wasm32-wasip2", "-p", "strings", "-p", "probe"])
            .status()
            .expect("running cargo for the plugins");
        assert!(status.success(), "building the plugins failed (is the wasm32-wasip2 target installed?)");
        let out = std::env::temp_dir().join(format!("openrage-plugin-tests-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&out);
        for (id, folder) in [("strings", "examples/strings"), ("probe", "tests/probe")] {
            let dir = out.join(id);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::copy(root.join(folder).join("plugin.toml"), dir.join("plugin.toml")).unwrap();
            let wasm = root.join("target/wasm32-wasip2/release").join(format!("{id}.wasm"));
            std::fs::copy(wasm, dir.join(format!("{id}.wasm"))).unwrap();
        }
        out
    })
}

fn registry(grants: &[(&str, &[Permission])]) -> Registry {
    let grants: HashMap<String, Vec<Permission>> = grants.iter().map(|(id, p)| (id.to_string(), p.to_vec())).collect();
    let reg = Registry::discover(&[plugin_dir().to_path_buf()], &grants);
    assert!(reg.broken.is_empty(), "{:?}", reg.broken);
    reg
}

fn host() -> Host {
    Host::new(Limits { memory: 256 << 20, time: Duration::from_secs(3) }).unwrap()
}

/// A game with two files.
struct FakeGame;

impl GameFiles for FakeGame {
    fn game(&self) -> Option<GameId> {
        Some(GameId::Rdr2)
    }
    fn read(&self, vpath: &str) -> Result<Vec<u8>> {
        match vpath {
            "a.rpf/x.txt" => Ok(b"from the game".to_vec()),
            _ => bail!("{vpath} not found"),
        }
    }
    fn list(&self, vpath: &str) -> Result<Vec<DirEntry>> {
        match vpath {
            "" => Ok(vec![DirEntry { name: "a.rpf".into(), is_dir: true, size: 0 }]),
            _ => bail!("{vpath} not found"),
        }
    }
}

fn run(
    host: &Host,
    plugin: &Plugin,
    files: Arc<dyn GameFiles>,
    probe: &str,
    arg: &str,
) -> Result<Vec<(String, String)>> {
    let component = host.compile(plugin).unwrap();
    let input = Input { vpath: format!("some/dir/{probe}"), data: arg.as_bytes().to_vec() };
    let outcome = host.run_action(plugin, &component, files, "run", &[input]);
    outcome
        .value
        .map(|files| files.into_iter().map(|f| (f.name, String::from_utf8_lossy(&f.data).into_owned())).collect())
}

#[test]
fn strings_previews_and_exports() {
    let reg = registry(&[("strings", &[])]);
    let plugin = reg.preview_for("x64.rpf/unknown.bin", Some(GameId::Gta5)).expect("fallback preview");
    let host = host();
    let component = host.compile(plugin).unwrap();
    let data = b"\x00\x01Hello plugin\x00\x02".to_vec();
    let outcome = host.preview(plugin, &component, Arc::new(NoGame), "x64.rpf/unknown.bin", &data);
    let preview = outcome.value.unwrap();
    assert_eq!(preview.info[0].key, "Strings");
    assert_eq!(preview.info[0].value, "1");
    assert!(matches!(&preview.view, Some(View::Text(t)) if t.contains("Hello plugin")), "{preview:?}");

    let (plugin, action) = reg.actions_for("a.ydr", None)[0];
    assert_eq!(action.id, "export");
    let inputs = [Input { vpath: "x64.rpf/a.ydr".into(), data }];
    let out = host.run_action(plugin, &component, Arc::new(NoGame), "export", &inputs).value.unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].name, "a.ydr.strings.txt");
    assert_eq!(out[0].data, b"Hello plugin\n");
    let unknown = host.run_action(plugin, &component, Arc::new(NoGame), "nope", &inputs).value;
    assert!(unknown.is_err());
}

#[test]
fn disabled_plugins_are_not_offered() {
    let reg = registry(&[]);
    assert!(reg.preview_for("a.bin", None).is_none());
    assert!(reg.actions_for("a.bin", None).is_empty());
}

#[test]
fn views_and_logs() {
    let reg = registry(&[("probe", &[Permission::ReadGame])]);
    let plugin = reg.get("probe").unwrap();
    let host = host();
    let component = host.compile(plugin).unwrap();
    let preview = |name: &str| -> (Result<Preview>, Vec<String>) {
        let o = host.preview(plugin, &component, Arc::new(NoGame), name, b"abc");
        (o.value, o.log.into_iter().map(|l| format!("{:?}: {}", l.level, l.message)).collect())
    };
    let (p, log) = preview("a.probe");
    assert_eq!(p.unwrap().info[0].value, "3");
    assert_eq!(log, ["Level::Info: previewing a.probe", "Level::Info: stdout line"]);
    assert!(matches!(preview("a.tree").0.unwrap().view, Some(View::Tree(n)) if n.len() == 2));
    assert!(matches!(preview("a.table").0.unwrap().view, Some(View::Table(_))));
    assert!(matches!(preview("a.image").0.unwrap().view, Some(View::Image(i)) if i.width == 2));
    assert!(preview("a.badimage").0.is_err());
    let _ = Level::Debug;
}

#[test]
fn reading_the_game_needs_the_permission() {
    let host = host();
    let with = registry(&[("probe", &[Permission::ReadGame])]);
    let probe = with.get("probe").unwrap();
    let game: Arc<dyn GameFiles> = Arc::new(FakeGame);
    assert_eq!(run(&host, probe, game.clone(), "read", "a.rpf/x.txt").unwrap()[0].1, "from the game");
    assert_eq!(run(&host, probe, game.clone(), "list", "").unwrap()[0].1, "a.rpf");
    assert_eq!(run(&host, probe, game.clone(), "game", "").unwrap()[0].1, "Game::Rdr2");
    for escape in ["/etc/passwd", "../secret.txt", "a.rpf/../../x", "C:/Windows/win.ini"] {
        let e = run(&host, probe, game.clone(), "read", escape).unwrap_err().to_string();
        assert!(e.contains("not a path inside the game folder"), "{escape}: {e}");
    }

    // Granted before, but without read-game: needs approval, and does not run.
    let without = registry(&[("probe", &[])]);
    assert!(!without.get("probe").unwrap().enabled());
    // Forcing it to run anyway (as a caller bug would) still cannot read.
    let mut forced = without.get("probe").unwrap().clone();
    forced.granted.clear();
    let e = run(&host, &forced, game, "read", "a.rpf/x.txt").unwrap_err().to_string();
    assert!(e.contains("permission read-game"), "{e}");
}

#[test]
fn the_sandbox_holds() {
    let reg = registry(&[("probe", &[Permission::ReadGame])]);
    let probe = reg.get("probe").unwrap();
    let host = host();
    let game: Arc<dyn GameFiles> = Arc::new(FakeGame);

    // No environment and no file system.
    assert_eq!(run(&host, probe, game.clone(), "env", "").unwrap()[0].1, "vars=0 root-entries=0");
    // Output names cannot leave the save folder.
    assert!(run(&host, probe, game.clone(), "escape", "").unwrap_err().to_string().contains("not allowed"));
    // Crashes, endless loops and memory hogs end the call, not OpenRAGE.
    assert!(run(&host, probe, game.clone(), "panic", "").is_err());
    let started = std::time::Instant::now();
    let e = run(&host, probe, game.clone(), "spin", "").unwrap_err();
    assert!(format!("{e:#}").contains("took longer than 3 s"), "{e:#}");
    assert!(started.elapsed() < Duration::from_secs(20));
    assert!(run(&host, probe, game.clone(), "alloc", "").is_err());
    // And the host still works afterwards.
    assert_eq!(run(&host, probe, game, "game", "").unwrap()[0].1, "Game::Rdr2");
}
