//! Runs plugin components in wasmtime, one fresh sandbox per call.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use wasmtime::component::{Component, HasSelf, Linker, ResourceTable};
use wasmtime::{Engine, Store, StoreLimits, StoreLimitsBuilder};
use wasmtime_wasi::p2::pipe::MemoryOutputPipe;
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

use crate::manifest::{self, ActionKind, GameId, Permission};
use crate::registry::Plugin;

mod bindings {
    wasmtime::component::bindgen!({
        path: "wit",
        world: "plugin",
    });
}

use bindings::openrage::plugin::{host as wit_host, types as wit};

pub use wit::{
    DirEntry, Game, Image as ImageView, Info, Input, Level, Material, Mesh, Model, OutputFile, Preview, Table,
    TreeNode, View,
};

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
    /// A model as the explorer shows it (see `load-model`).
    fn model(&self, vpath: &str) -> Result<Model> {
        bail!("{vpath}: models cannot be read here")
    }
    /// A texture of a texture dictionary or a model (see `load-texture`).
    fn texture(&self, vpath: &str, name: &str) -> Result<ImageView> {
        bail!("{vpath}: textures of {name} cannot be read here")
    }
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

/// The name of a log level, for people (`info`, `warning`, …).
pub fn level_name(level: Level) -> &'static str {
    match level {
        Level::Debug => "debug",
        Level::Info => "info",
        Level::Warn => "warning",
        Level::Error => "error",
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
    /// Changes to game files the plugin asked for (only from successful
    /// `modify` actions; empty otherwise). Applying them is up to the caller.
    pub writes: Vec<ModWrite>,
}

/// A change to a game file a plugin asked for (`write-mod-file`).
#[derive(Debug, Clone)]
pub struct ModWrite {
    /// Checked to be relative and free of `..`, with `/` separators.
    pub vpath: String,
    pub data: Vec<u8>,
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
/// Most bytes one call may ask to write into the mods folder.
const MAX_WRITES: usize = 1 << 30;

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
        let handed = vec![vpath.replace('\\', "/")];
        self.call(plugin, component, files, false, handed, |p, store| {
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
            return Outcome {
                value: Err(anyhow!("{} has no action {action:?}", plugin.manifest.id)),
                log: Vec::new(),
                writes: Vec::new(),
            };
        }
        let may_write = plugin.manifest.action(action).is_some_and(|a| a.kind == ActionKind::Modify);
        let handed = inputs.iter().filter(|i| !i.picked).map(|i| i.vpath.replace('\\', "/")).collect();
        self.call(plugin, component, files, may_write, handed, |p, store| {
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
        may_write: bool,
        handed: Vec<String>,
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
            may_write,
            handed,
            writes: Vec::new(),
            written: 0,
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
        // Changes count only when the action succeeded.
        let writes = match value {
            Ok(_) => std::mem::take(&mut store.data_mut().writes),
            Err(_) => Vec::new(),
        };
        Outcome { value, log, writes }
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
        Some(View::Model(model)) => check_model(model)?,
        Some(View::Text(_)) | None => {}
    }
    Ok(())
}

/// Most vertices a plugin's model may have.
const MAX_VERTICES: usize = 20_000_000;

/// Rejects models the viewer could not show safely.
fn check_model(model: &Model) -> Result<()> {
    let mut vertices = 0usize;
    for (i, m) in model.meshes.iter().enumerate() {
        let n = m.positions.len() / 3;
        vertices += n;
        let bad = m.positions.len() % 3 != 0
            || (!m.normals.is_empty() && m.normals.len() != n * 3)
            || (!m.uvs.is_empty() && m.uvs.len() != n * 2)
            || m.indices.len() % 3 != 0
            || m.indices.iter().any(|&k| k as usize >= n)
            || m.material as usize >= model.materials.len();
        if bad {
            bail!("the plugin returned a model whose mesh {i} does not fit together");
        }
    }
    if vertices > MAX_VERTICES {
        bail!("the plugin returned a model with more than {MAX_VERTICES} vertices");
    }
    for m in &model.materials {
        if let Some(img) = &m.image
            && (img.width == 0
                || img.height == 0
                || img.width > 16384
                || img.height > 16384
                || img.rgba.len() as u64 != u64::from(img.width) * u64::from(img.height) * 4)
        {
            bail!("the plugin returned a texture whose size does not match its pixels");
        }
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
    /// The call is a `modify` action: `write-mod-file` works.
    may_write: bool,
    /// The game files the user handed to this call: readable without
    /// `read-game`.
    handed: Vec<String>,
    writes: Vec<ModWrite>,
    /// Bytes in `writes`, against [`MAX_WRITES`].
    written: usize,
}

impl Ctx {
    /// A path the plugin may read: one it was handed, or any with `read-game`.
    fn readable(&self, vpath: &str) -> Result<String, String> {
        let vpath = checked_vpath(vpath)?;
        if !self.handed.iter().any(|h| h.eq_ignore_ascii_case(&vpath)) {
            self.require(Permission::ReadGame)?;
        }
        Ok(vpath)
    }

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
        let vpath = self.readable(&vpath)?;
        self.files.read(&vpath).map_err(|e| format!("{e:#}"))
    }

    fn load_model(&mut self, vpath: String) -> Result<Model, String> {
        let vpath = self.readable(&vpath)?;
        self.files.model(&vpath).map_err(|e| format!("{e:#}"))
    }

    fn load_texture(&mut self, vpath: String, name: String) -> Result<ImageView, String> {
        let vpath = self.readable(&vpath)?;
        self.files.texture(&vpath, &name).map_err(|e| format!("{e:#}"))
    }

    fn list_dir(&mut self, vpath: String) -> Result<Vec<DirEntry>, String> {
        self.require(Permission::ReadGame)?;
        let vpath = if vpath.trim_matches(['/', '\\']).is_empty() { String::new() } else { checked_vpath(&vpath)? };
        self.files.list(&vpath).map_err(|e| format!("{e:#}"))
    }

    fn log(&mut self, level: Level, message: String) {
        push_log(&mut self.log, level, &message);
    }

    fn write_mod_file(&mut self, vpath: String, data: Vec<u8>) -> Result<(), String> {
        self.require(Permission::WriteMods)?;
        if !self.may_write {
            return Err("game files can only be changed in actions of kind \"modify\"".into());
        }
        let vpath = checked_vpath(&vpath)?;
        // The same file again: the last content wins.
        if let Some(i) = self.writes.iter().position(|w| w.vpath.eq_ignore_ascii_case(&vpath)) {
            self.written -= self.writes.remove(i).data.len();
        }
        if self.written + data.len() > MAX_WRITES {
            return Err(format!("an action may change at most {} MB of files", MAX_WRITES >> 20));
        }
        self.written += data.len();
        self.writes.push(ModWrite { vpath, data });
        Ok(())
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
