//! Runs plugin components in wasmtime, one fresh sandbox per call.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use wasmtime::component::{Component, HasSelf, Linker, ResourceTable};
use wasmtime::{Engine, Store, StoreLimits, StoreLimitsBuilder};
use wasmtime_wasi::p2::pipe::MemoryOutputPipe;
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

use crate::manifest::{self, GameId, Permission};
use crate::registry::Plugin;

mod bindings {
    wasmtime::component::bindgen!({
        path: "wit",
        world: "plugin",
    });
}

use bindings::openrage::plugin::{host as wit_host, types as wit};

pub use wit::{DirEntry, Game, Image as ImageView, Info, Input, Level, OutputFile, Preview, Table, TreeNode, View};

/// How much a single call may use.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Linear memory of the plugin, in bytes.
    pub memory: usize,
    /// Wall-clock time of one call.
    pub time: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self { memory: 1 << 30, time: Duration::from_secs(60) }
    }
}

/// The game files a plugin may read, provided by the application (which
/// knows about archives, keys and the `mods` folder).
pub trait GameFiles: Send + Sync {
    /// The game that is open (`None`: none).
    fn game(&self) -> Option<GameId>;
    /// Reads a file by its virtual path (already checked to be relative and
    /// free of `..`).
    fn read(&self, vpath: &str) -> Result<Vec<u8>>;
    /// Lists a folder ("" for the game folder).
    fn list(&self, vpath: &str) -> Result<Vec<DirEntry>>;
}

/// No game open: plugins can only see the files they are handed.
pub struct NoGame;

impl GameFiles for NoGame {
    fn game(&self) -> Option<GameId> {
        None
    }
    fn read(&self, _: &str) -> Result<Vec<u8>> {
        bail!("no game is open")
    }
    fn list(&self, _: &str) -> Result<Vec<DirEntry>> {
        bail!("no game is open")
    }
}

/// A line a plugin logged (or printed to stdout/stderr).
#[derive(Debug, Clone)]
pub struct LogLine {
    pub level: Level,
    pub message: String,
}

/// The result of a call, with what the plugin logged on the way.
#[derive(Debug)]
pub struct Outcome<T> {
    pub value: Result<T>,
    pub log: Vec<LogLine>,
}

/// Compiles and runs plugins.
pub struct Host {
    engine: Engine,
    linker: Linker<Ctx>,
    limits: Limits,
}

/// How often the engine's clock ticks for call timeouts.
const TICK: Duration = Duration::from_millis(10);
/// Most bytes of stdout/stderr kept per call.
const OUTPUT_CAPACITY: usize = 64 * 1024;
/// Most log lines kept per call, and their length.
const MAX_LOG_LINES: usize = 1000;
const MAX_LOG_LINE: usize = 4096;

impl Host {
    pub fn new(limits: Limits) -> Result<Self> {
        let mut config = wasmtime::Config::new();
        config.wasm_component_model(true);
        config.epoch_interruption(true);
        let engine = Engine::new(&config)?;

        // Advances the engine's epoch so long-running calls hit their deadline.
        let weak = engine.weak();
        std::thread::Builder::new().name("plugin-clock".into()).spawn(move || {
            while let Some(engine) = weak.upgrade() {
                engine.increment_epoch();
                drop(engine);
                std::thread::sleep(TICK);
            }
        })?;

        let mut linker = Linker::new(&engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?;
        bindings::Plugin::add_to_linker::<Ctx, HasSelf<Ctx>>(&mut linker, |c| c)?;
        Ok(Self { engine, linker, limits })
    }

    /// Compiles a plugin's component. Do it once per plugin and keep the
    /// result; compiling takes a moment.
    pub fn compile(&self, plugin: &Plugin) -> Result<Component> {
        let path = plugin.wasm_path();
        let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        Component::new(&self.engine, &bytes)
            .map_err(anyhow::Error::from)
            .with_context(|| format!("loading {}", path.display()))
    }

    /// Asks the plugin to preview a file.
    pub fn preview(
        &self,
        plugin: &Plugin,
        component: &Component,
        files: Arc<dyn GameFiles>,
        vpath: &str,
        data: &[u8],
    ) -> Outcome<Preview> {
        self.call(plugin, component, files, |p, store| {
            let preview = p.call_preview(store, vpath, data)?.map_err(|e| anyhow!("{e}"))?;
            check_preview(&preview)?;
            Ok(preview)
        })
    }

    /// Runs one of the plugin's actions on the files the user picked.
    pub fn run_action(
        &self,
        plugin: &Plugin,
        component: &Component,
        files: Arc<dyn GameFiles>,
        action: &str,
        inputs: &[Input],
    ) -> Outcome<Vec<OutputFile>> {
        if plugin.manifest.action(action).is_none() {
            return Outcome { value: Err(anyhow!("{} has no action {action:?}", plugin.manifest.id)), log: Vec::new() };
        }
        self.call(plugin, component, files, |p, store| {
            let outputs = p.call_run_action(store, action, inputs)?.map_err(|e| anyhow!("{e}"))?;
            check_outputs(&outputs)?;
            Ok(outputs)
        })
    }

    fn call<T>(
        &self,
        plugin: &Plugin,
        component: &Component,
        files: Arc<dyn GameFiles>,
        f: impl FnOnce(&bindings::Plugin, &mut Store<Ctx>) -> Result<T>,
    ) -> Outcome<T> {
        let stdout = MemoryOutputPipe::new(OUTPUT_CAPACITY);
        let stderr = MemoryOutputPipe::new(OUTPUT_CAPACITY);
        // No arguments, environment, preopened folders or sockets.
        let wasi = WasiCtxBuilder::new()
            .stdout(stdout.clone())
            .stderr(stderr.clone())
            .allow_tcp(false)
            .allow_udp(false)
            .allow_ip_name_lookup(false)
            .build();
        let ctx = Ctx {
            wasi,
            table: ResourceTable::new(),
            limits: StoreLimitsBuilder::new().memory_size(self.limits.memory).instances(16).tables(64).build(),
            files,
            permissions: plugin.granted.clone(),
            log: Vec::new(),
        };
        let mut store = Store::new(&self.engine, ctx);
        store.limiter(|c| &mut c.limits);
        let ticks = (self.limits.time.as_millis() / TICK.as_millis()).max(1) as u64;
        store.set_epoch_deadline(ticks);
        store.epoch_deadline_trap();

        let value = bindings::Plugin::instantiate(&mut store, component, &self.linker)
            .map_err(anyhow::Error::from)
            .context("starting the plugin")
            .and_then(|p| f(&p, &mut store))
            .map_err(|e| explain_trap(e, &self.limits));

        let mut log = std::mem::take(&mut store.data_mut().log);
        for (pipe, level) in [(&stdout, Level::Info), (&stderr, Level::Warn)] {
            let text = String::from_utf8_lossy(&pipe.contents()).into_owned();
            for line in text.lines().filter(|l| !l.trim().is_empty()) {
                push_log(&mut log, level, line);
            }
        }
        Outcome { value, log }
    }
}

/// Turns wasmtime's traps into messages a user understands.
fn explain_trap(e: anyhow::Error, limits: &Limits) -> anyhow::Error {
    match e.downcast_ref::<wasmtime::Trap>() {
        Some(wasmtime::Trap::Interrupt) => {
            anyhow!("the plugin took longer than {} s and was stopped", limits.time.as_secs())
        }
        Some(wasmtime::Trap::UnreachableCodeReached) => e.context("the plugin crashed (see its log)"),
        _ => e,
    }
}

fn push_log(log: &mut Vec<LogLine>, level: Level, message: &str) {
    if log.len() >= MAX_LOG_LINES {
        return;
    }
    let mut message = message.to_owned();
    if message.len() > MAX_LOG_LINE {
        let mut end = MAX_LOG_LINE;
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        message.truncate(end);
        message.push('…');
    }
    log.push(LogLine { level, message });
}

/// Rejects previews the GUI could not show safely.
fn check_preview(p: &Preview) -> Result<()> {
    match &p.view {
        Some(View::Image(img)) => {
            if img.width == 0 || img.height == 0 || img.width > 16384 || img.height > 16384 {
                bail!("the plugin returned an image of {}×{} pixels", img.width, img.height);
            }
            if img.rgba.len() as u64 != u64::from(img.width) * u64::from(img.height) * 4 {
                bail!("the plugin returned an image whose size does not match its pixels");
            }
        }
        Some(View::Tree(nodes)) => {
            for (i, n) in nodes.iter().enumerate() {
                if n.parent.is_some_and(|p| p as usize >= i) {
                    bail!("the plugin returned a tree whose node {i} comes before its parent");
                }
            }
        }
        Some(View::Table(t)) => {
            if t.rows.iter().any(|r| r.len() != t.columns.len()) {
                bail!("the plugin returned a table with rows of the wrong length");
            }
        }
        Some(View::Text(_)) | None => {}
    }
    Ok(())
}

/// Output names must stay inside the folder the user saves them to.
fn check_outputs(outputs: &[OutputFile]) -> Result<()> {
    for (i, o) in outputs.iter().enumerate() {
        if manifest::safe_relative(&o.name).is_none() {
            bail!("the plugin returned a file named {:?}, which is not allowed", o.name);
        }
        if outputs[..i].iter().any(|p| p.name.eq_ignore_ascii_case(&o.name)) {
            bail!("the plugin returned {:?} twice", o.name);
        }
    }
    Ok(())
}

/// What one call's sandbox holds.
struct Ctx {
    wasi: WasiCtx,
    table: ResourceTable,
    limits: StoreLimits,
    files: Arc<dyn GameFiles>,
    permissions: Vec<Permission>,
    log: Vec<LogLine>,
}

impl Ctx {
    fn require(&self, p: Permission) -> Result<(), String> {
        if self.permissions.contains(&p) {
            Ok(())
        } else {
            Err(format!("permission {p} is needed (add it to plugin.toml; the user grants it)"))
        }
    }
}

impl WasiView for Ctx {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView { ctx: &mut self.wasi, table: &mut self.table }
    }
}

impl wit::Host for Ctx {}

impl wit_host::Host for Ctx {
    fn current_game(&mut self) -> Game {
        match self.files.game() {
            Some(GameId::Gta5) => Game::Gta5,
            Some(GameId::Rdr2) => Game::Rdr2,
            Some(GameId::Gta4) => Game::Gta4,
            None => Game::Other,
        }
    }

    fn read_file(&mut self, vpath: String) -> Result<Vec<u8>, String> {
        self.require(Permission::ReadGame)?;
        let vpath = checked_vpath(&vpath)?;
        self.files.read(&vpath).map_err(|e| format!("{e:#}"))
    }

    fn list_dir(&mut self, vpath: String) -> Result<Vec<DirEntry>, String> {
        self.require(Permission::ReadGame)?;
        let vpath = if vpath.trim_matches(['/', '\\']).is_empty() { String::new() } else { checked_vpath(&vpath)? };
        self.files.list(&vpath).map_err(|e| format!("{e:#}"))
    }

    fn log(&mut self, level: Level, message: String) {
        push_log(&mut self.log, level, &message);
    }
}

/// A virtual path a plugin asked for: relative and without `..`, so it can
/// only name files of the game.
fn checked_vpath(vpath: &str) -> Result<String, String> {
    match manifest::safe_relative(vpath.trim_matches(['/', '\\'])) {
        Some(_) if !vpath.starts_with(['/', '\\']) => Ok(vpath.replace('\\', "/")),
        _ => Err(format!("{vpath:?} is not a path inside the game folder")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vpaths_stay_in_the_game() {
        assert_eq!(checked_vpath("x64a.rpf\\a.ydr").unwrap(), "x64a.rpf/a.ydr");
        for bad in ["/etc/passwd", "../x", "a/../../b", "C:/Windows/x", "\\\\server\\x", ""] {
            assert!(checked_vpath(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn checks_outputs_and_previews() {
        let file = |name: &str| OutputFile { name: name.into(), data: Vec::new() };
        assert!(check_outputs(&[file("a.txt"), file("sub/b.txt")]).is_ok());
        assert!(check_outputs(&[file("../a.txt")]).is_err());
        assert!(check_outputs(&[file("a.txt"), file("A.TXT")]).is_err());
        let image = |w, h, n| Preview {
            info: Vec::new(),
            view: Some(View::Image(ImageView { width: w, height: h, rgba: vec![0; n] })),
        };
        assert!(check_preview(&image(2, 2, 16)).is_ok());
        assert!(check_preview(&image(2, 2, 15)).is_err());
        let tree = Preview {
            info: Vec::new(),
            view: Some(View::Tree(vec![TreeNode { parent: Some(0), label: "x".into(), value: String::new() }])),
        };
        assert!(check_preview(&tree).is_err());
    }
}
