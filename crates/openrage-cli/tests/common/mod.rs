#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};

use rage_formats::gxt2;
use rage_formats::texture::{Dimension, Format, Texture};
use rage_formats::ytd::{Layout, Stored, TextureDictionary};
use rage_rpf::ArchiveWriter;

pub const HELLO: &[u8] = b"hello from a synthetic archive\n";
pub const CONFIG_XML: &[u8] =
    b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Config><Item name=\"a\">1</Item></Config>\n";

/// A scratch game folder with its own config, cache and data directories.
pub struct Sandbox {
    pub root: PathBuf,
    pub game: PathBuf,
    pub home: PathBuf,
}

static COUNTER: AtomicU32 = AtomicU32::new(0);

impl Sandbox {
    pub fn new() -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("openrage-cli-test-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let game = root.join("game");
        let home = root.join("home");
        std::fs::create_dir_all(&game).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        Self { root, game, home }
    }

    /// A sandbox whose game folder holds `common.rpf` and a loose `readme.txt`.
    pub fn with_game() -> Self {
        let s = Self::new();
        std::fs::write(s.game.join("common.rpf"), sample_archive()).unwrap();
        std::fs::write(s.game.join("readme.txt"), b"loose readme\n").unwrap();
        s
    }

    pub fn command(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_openrage"));
        c.env_clear();
        if let Some(path) = std::env::var_os("PATH") {
            c.env("PATH", path);
        }
        if let Some(root) = std::env::var_os("SystemRoot") {
            c.env("SystemRoot", root);
        }
        let config = self.home.join("config");
        let cache = self.home.join("cache");
        let data = self.home.join("data");
        for d in [&config, &cache, &data] {
            std::fs::create_dir_all(d).unwrap();
        }
        c.env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("XDG_CONFIG_HOME", &config)
            .env("XDG_CACHE_HOME", &cache)
            .env("XDG_DATA_HOME", &data)
            .env("APPDATA", &config)
            .env("LOCALAPPDATA", &cache)
            .env("LANG", "C")
            .current_dir(&self.root);
        c
    }

    /// Runs `openrage --game <game folder> <args>`.
    pub fn run(&self, args: &[&str]) -> Out {
        let mut c = self.command();
        c.arg("--game").arg(&self.game).args(args);
        Out::from(c.output().expect("run openrage"))
    }

    /// Runs `openrage <args>` with no game folder given.
    pub fn run_without_game(&self, args: &[&str]) -> Out {
        let mut c = self.command();
        c.args(args);
        Out::from(c.output().expect("run openrage"))
    }

    /// Where the app keeps its per-user config: `dirs` reads `XDG_CONFIG_HOME` on Linux and `HOME` on macOS.
    pub fn app_config(&self) -> PathBuf {
        if cfg!(target_os = "macos") {
            self.home.join("Library").join("Application Support").join("openrage")
        } else {
            self.home.join("config").join("openrage")
        }
    }

    pub fn app_cache(&self) -> PathBuf {
        if cfg!(target_os = "macos") {
            self.home.join("Library").join("Caches").join("openrage")
        } else {
            self.home.join("cache").join("openrage")
        }
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

pub struct Out {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

impl From<Output> for Out {
    fn from(o: Output) -> Self {
        Self {
            ok: o.status.success(),
            stdout: String::from_utf8_lossy(&o.stdout).replace("\r\n", "\n"),
            stderr: String::from_utf8_lossy(&o.stderr).replace("\r\n", "\n"),
        }
    }
}

impl Out {
    pub fn lines(&self) -> Vec<&str> {
        self.stdout.lines().collect()
    }

    #[track_caller]
    pub fn success(self) -> Self {
        assert!(self.ok, "expected success, stderr: {}\nstdout: {}", self.stderr, self.stdout);
        self
    }

    #[track_caller]
    pub fn failure(self) -> Self {
        assert!(!self.ok, "expected failure, stdout: {}", self.stdout);
        self
    }
}

/// A PSO file with a list of two items (a name hash and a float radius each).
pub fn pso_file() -> Vec<u8> {
    use rage_core::hash::{jenkins_bytes, joaat};
    let h = |name: &str| jenkins_bytes(name.as_bytes());
    let (root, item) = (h("CPedModelInfo__InitDataList"), h("CPedModelInfo__InitData"));
    let mut items = Vec::new();
    for (name, radius) in [("adder", 1.5f32), ("banshee", -2.0)] {
        items.extend(joaat(name).to_be_bytes());
        items.extend(radius.to_be_bytes());
    }
    let mut root_data = 1u32.to_be_bytes().to_vec();
    root_data.extend([0, 0, 0, 0, 0, 2, 0, 2, 0, 0, 0, 0]);
    let mut psin = b"PSIN\0\0\0\0pppppppp".to_vec();
    psin.extend(&items);
    psin.extend(&root_data);
    let len = psin.len() as u32;
    psin[4..8].copy_from_slice(&len.to_be_bytes());
    let mut pmap = b"PMAP".to_vec();
    pmap.extend(48u32.to_be_bytes());
    pmap.extend(2u32.to_be_bytes());
    pmap.extend([0, 2, b'p', b'p']);
    for (name, off, len) in [(item, 16u32, 16u32), (root, 32, 16)] {
        for v in [name, off, 0, len] {
            pmap.extend(v.to_be_bytes());
        }
    }
    let entry = |name: u32, kind: u8, sub: u8, offset: u16, key: u32| {
        let mut e = name.to_be_bytes().to_vec();
        e.extend([kind, sub]);
        e.extend(offset.to_be_bytes());
        e.extend(key.to_be_bytes());
        e
    };
    let structure = |length: u32, entries: Vec<Vec<u8>>| {
        let mut s = (entries.len() as u32).to_be_bytes().to_vec();
        s.extend(length.to_be_bytes());
        s.extend(0u32.to_be_bytes());
        s.extend(entries.concat());
        s
    };
    let root_info = structure(16, vec![entry(0x100, 0x0c, 0, 0, item), entry(h("InitDatas"), 0x0d, 0, 0, 0)]);
    let item_info = structure(8, vec![entry(h("Name"), 0x0b, 7, 0, 0), entry(h("Radius"), 0x07, 0, 4, 0)]);
    let index_len = 12 + 2 * 8;
    let mut psch = b"PSCH".to_vec();
    psch.extend(((index_len + root_info.len() + item_info.len()) as u32).to_be_bytes());
    psch.extend(2u32.to_be_bytes());
    psch.extend(root.to_be_bytes());
    psch.extend((index_len as u32).to_be_bytes());
    psch.extend(item.to_be_bytes());
    psch.extend(((index_len + root_info.len()) as u32).to_be_bytes());
    psch.extend(root_info);
    psch.extend(item_info);
    [psin, pmap, psch].concat()
}

pub fn gxt2_table() -> Vec<u8> {
    gxt2::write(&[
        gxt2::Entry { hash: rage_core::hash::joaat("HELLO_LABEL"), text: "Hello there".into() },
        gxt2::Entry { hash: rage_core::hash::joaat("BYE_LABEL"), text: "Goodbye now".into() },
    ])
}

fn texture(name: &str, size: u32, format: Format, fill: u8) -> Texture {
    let t = Texture {
        name: name.into(),
        width: size,
        height: size,
        depth: 1,
        levels: 1,
        format,
        dimension: Dimension::Texture2D,
        usage: 20,
        data: Vec::new(),
    };
    let n = t.expected_size().unwrap();
    Texture { data: vec![fill; n], ..t }
}

pub fn ytd_file() -> Vec<u8> {
    let layout = Layout::Legacy;
    let textures = vec![texture("alpha", 8, Format::BC1_UNORM, 1), texture("beta", 4, Format::B8G8R8A8_UNORM, 2)];
    let mut root = vec![0u8; 64];
    root[8] = 1;
    let dict = TextureDictionary {
        stored: textures.iter().map(|_| Stored { template: vec![0; layout.texture_size()], data_ptr: 0 }).collect(),
        textures,
        errors: Vec::new(),
        layout,
        version: 13,
        root,
    };
    rage_formats::ytd_write::rebuild(&dict).unwrap()
}

pub fn inner_archive() -> Vec<u8> {
    let mut w = ArchiveWriter::new();
    w.add_file("deep/note.txt", b"nested note\n".to_vec()).unwrap();
    w.build().unwrap()
}

pub fn sample_archive() -> Vec<u8> {
    let mut w = ArchiveWriter::new();
    w.add_file("data/hello.txt", HELLO.to_vec()).unwrap();
    w.add_file("data/config.xml", CONFIG_XML.to_vec()).unwrap();
    w.add_file("text/american.gxt2", gxt2_table()).unwrap();
    w.add_resource_file("textures/test.ytd", ytd_file()).unwrap();
    w.add_file("data/peds.pso", pso_file()).unwrap();
    w.add_file("inner.rpf", inner_archive()).unwrap();
    w.build().unwrap()
}

pub fn write(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, bytes).unwrap();
}

pub fn read_dir_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> =
        std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    names
}

pub fn path_str(path: &Path) -> String {
    path.to_str().unwrap().to_owned()
}
