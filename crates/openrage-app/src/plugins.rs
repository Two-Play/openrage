//! Plugins in the application: which ones the user enabled, and the game
//! files they may read.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use openrage_plugins::{
    Action, Component, DirEntry, GameFiles, GameId, Host, ImageView, Input, LogLine, Material, Mesh, ModWrite, Model,
    OutputFile, Permission, Plugin, Registry,
};
use rage_crypto::GtaVKeys;
use rage_formats::drawable::{self, Drawable};
use rage_formats::math;
use rage_formats::texture::Texture;
use rage_formats::ytd::TextureDictionary;
use rage_rpf::vfs::Node;

use crate::config::Config;

/// What the user granted each enabled plugin.
pub fn grants(cfg: &Config) -> HashMap<String, Vec<Permission>> {
    cfg.plugins.iter().map(|(id, g)| (id.clone(), openrage_plugins::parse_grants(g))).collect()
}

/// All installed plugins, with their state from the settings.
pub fn registry(cfg: &Config) -> Registry {
    Registry::discover(&openrage_plugins::search_dirs(), &grants(cfg))
}

/// Enables a plugin with the permissions it asks for.
pub fn enable(cfg: &mut Config, id: &str, permissions: &[Permission]) {
    let grants: Vec<&str> = permissions.iter().map(|p| p.as_str()).collect();
    let grants = grants.join("+");
    match cfg.plugins.iter_mut().find(|(i, _)| i == id) {
        Some(entry) => entry.1 = grants,
        None => cfg.plugins.push((id.to_owned(), grants)),
    }
}

/// Disables a plugin; returns whether it was enabled.
pub fn disable(cfg: &mut Config, id: &str) -> bool {
    let before = cfg.plugins.len();
    cfg.plugins.retain(|(i, _)| i != id);
    cfg.plugins.len() != before
}

/// The game a folder holds.
pub fn game_id(dir: &Path) -> GameId {
    if dir.join("RDR2.exe").is_file() {
        GameId::Rdr2
    } else if crate::gta4::is_gta4_dir(dir) {
        GameId::Gta4
    } else {
        GameId::Gta5
    }
}

/// The game's files as plugins see them: read-only, with the changes in
/// `mods/` applied.
pub struct GameDir {
    dir: PathBuf,
    game: GameId,
    keys: Option<Arc<GtaVKeys>>,
    /// The model read last, for its textures (`load-texture` after `load-model`).
    last_model: std::sync::Mutex<Option<(String, Arc<crate::models::LoadedModel>)>>,
}

impl GameDir {
    pub fn new(dir: &Path, keys: Option<Arc<GtaVKeys>>) -> Self {
        Self { dir: dir.to_path_buf(), game: game_id(dir), keys, last_model: std::sync::Mutex::new(None) }
    }

    fn loaded_model(&self, vpath: &str) -> Result<Arc<crate::models::LoadedModel>> {
        if let Ok(last) = self.last_model.lock()
            && let Some((path, model)) = last.as_ref()
            && path.eq_ignore_ascii_case(vpath)
        {
            return Ok(Arc::clone(model));
        }
        if !crate::models::is_model(vpath) || is_collision_or_path(vpath) {
            bail!("{vpath} is not a model (.ydr, .yft, .ydd, .wdr, .wdd, .wft)");
        }
        let model = Arc::new(crate::models::load(&self.dir, self.keys.clone(), vpath)?);
        if let Ok(mut last) = self.last_model.lock() {
            *last = Some((vpath.to_owned(), Arc::clone(&model)));
        }
        Ok(model)
    }

    /// A texture dictionary of any game.
    fn dictionary(&self, vpath: &str) -> Result<TextureDictionary> {
        Ok(match self.resolve(vpath)? {
            Node::ArchiveFile(rpf, id) => rage_formats::ytd::parse(&rpf.read_resource(id)?)?,
            Node::Rpf8File(rpf, i) => {
                let entry = rpf.entries().and_then(|e| e.get(i)).context("archive table not readable")?;
                rage_formats::ytd::parse_rdr2(&rpf.read_resource(entry)?)?
            }
            Node::Gta4File(a, id) => rage_formats::ytd::parse_gta4(&a.read_resource(id)?)?,
            _ => bail!("{vpath} is not a texture dictionary inside an archive"),
        })
    }

    fn resolve(&self, vpath: &str) -> Result<Node> {
        Ok(rage_mods::resolve(&self.dir, vpath, self.keys.clone())?.0)
    }
}

impl GameFiles for GameDir {
    fn game(&self) -> Option<GameId> {
        Some(self.game)
    }

    fn model(&self, vpath: &str) -> Result<Model> {
        Ok(to_plugin_model(&*self.loaded_model(vpath)?))
    }

    fn texture(&self, vpath: &str, name: &str) -> Result<ImageView> {
        let lower = vpath.to_ascii_lowercase();
        let texture = if lower.ends_with(".ytd") || lower.ends_with(".wtd") {
            let dict = self.dictionary(vpath)?;
            dict.textures.into_iter().find(|t| t.name.eq_ignore_ascii_case(name))
        } else {
            let model = self.loaded_model(vpath)?;
            model.drawables.iter().find_map(|(_, d)| model.texture(d, name)).cloned()
        };
        let t = texture.with_context(|| format!("{vpath} has no texture {name:?}"))?;
        let rgba = t.to_rgba().with_context(|| format!("texture {name:?}: format {:?} cannot be decoded", t.format))?;
        Ok(ImageView { width: t.width, height: t.height, rgba })
    }

    fn read(&self, vpath: &str) -> Result<Vec<u8>> {
        Ok(match self.resolve(vpath)? {
            Node::PlainFile(p) => fs::read(p)?,
            Node::ArchiveFile(rpf, id) => rpf.read(id)?,
            Node::Rpf8File(rpf, i) => match rpf.entries().and_then(|e| e.get(i)) {
                Some(entry) => rpf.read(entry)?,
                None => bail!("{vpath}: not found"),
            },
            Node::Gta4File(a, id) => a.read(id)?,
            Node::Dir(_) | Node::ArchiveDir(..) | Node::Rpf8Dir(..) | Node::Gta4Dir(..) => {
                bail!("{vpath} is a folder")
            }
        })
    }

    fn list(&self, vpath: &str) -> Result<Vec<DirEntry>> {
        let mut out = Vec::new();
        match self.resolve(vpath)? {
            Node::Dir(dir) => {
                // A folder of the game, merged with the same folder in `mods`.
                let mut dirs = vec![dir];
                if let Some(mods) = rage_mods::mods_dir(&self.dir)
                    && let Some(m) = rage_rpf::vfs::find_case_insensitive(&mods, vpath)
                {
                    dirs.push(m);
                }
                if vpath.is_empty() {
                    dirs.truncate(1);
                }
                for d in dirs {
                    for e in fs::read_dir(&d)?.flatten() {
                        let name = e.file_name().to_string_lossy().into_owned();
                        if out.iter().any(|o: &DirEntry| o.name.eq_ignore_ascii_case(&name)) {
                            continue;
                        }
                        let meta = e.metadata()?;
                        let is_dir = meta.is_dir() || name.to_ascii_lowercase().ends_with(".rpf");
                        out.push(DirEntry { name, is_dir, size: if meta.is_dir() { 0 } else { meta.len() } });
                    }
                }
            }
            Node::ArchiveDir(rpf, id) => {
                for c in rpf.children(id) {
                    let e = rpf.entry(c);
                    out.push(DirEntry { name: e.name.clone(), is_dir: e.is_dir() || e.is_archive(), size: e.size() });
                }
            }
            Node::Rpf8Dir(rpf, dir) => {
                let (dirs, files) = rpf.list_dir(&dir);
                for d in dirs {
                    out.push(DirEntry { name: d, is_dir: true, size: 0 });
                }
                for i in files {
                    let name = rpf.entry_name(i);
                    let size = rpf.entries().and_then(|e| e.get(i)).map_or(0, |e| e.size());
                    out.push(DirEntry {
                        name: name.rsplit('/').next().unwrap_or(&name).to_owned(),
                        is_dir: false,
                        size,
                    });
                }
            }
            Node::Gta4Dir(a, dir) => {
                for c in a.children(dir) {
                    let e = a.entry(c);
                    out.push(DirEntry { name: e.name.clone(), is_dir: e.is_dir(), size: e.size() });
                }
            }
            Node::PlainFile(_) | Node::ArchiveFile(..) | Node::Rpf8File(..) | Node::Gta4File(..) => {
                bail!("{vpath} is a file")
            }
        }
        out.sort_by_key(|e| (!e.is_dir, e.name.to_ascii_lowercase()));
        Ok(out)
    }
}

/// Collision and path files count as models in the explorer, but plugins
/// get only real models.
fn is_collision_or_path(vpath: &str) -> bool {
    let lower = vpath.to_ascii_lowercase();
    lower.ends_with(".ybn")
        || lower.ends_with(".wbn")
        || lower.ends_with(".wbd")
        || lower.ends_with(".ypt")
        || rage_formats::paths::is_path_file(&lower)
}

/// Neutral grey for materials without a colour of their own.
const GREY: (f32, f32, f32, f32) = (0.7, 0.7, 0.7, 1.0);

/// A model as plugins see it: the parts shown by default, placed in model
/// space, one material per shader kind.
pub fn to_plugin_model(model: &crate::models::LoadedModel) -> Model {
    let mut materials: Vec<(u32, Material)> = Vec::new();
    let mut meshes = Vec::new();
    let visible = model.default_visibility();
    // Child drawables of fragments (wheels, doors) have no shaders of their
    // own: they use the main drawable's, as the renderer does.
    let main_shaders = model.drawables.first().map(|(_, d)| d.shaders.as_slice()).unwrap_or_default();
    for (part, d) in model.shown(&visible) {
        let shaders = if d.shaders.is_empty() { main_shaders } else { d.shaders.as_slice() };
        for m in &d.meshes {
            let Some(shader) = shaders.get(m.shader as usize) else { continue };
            let texture = shader.base_texture().unwrap_or_default().to_owned();
            // Same shader kind and texture: one material.
            let key = shader.name_hash ^ rage_core::hash::joaat(&texture);
            let material = match materials.iter().position(|(k, _)| *k == key) {
                Some(i) => i,
                None => {
                    let kind = rage_formats::shaders::name(shader.name_hash)
                        .map(str::to_owned)
                        .unwrap_or_else(|| format!("shader_{:08x}", shader.name_hash));
                    let name = if texture.is_empty() { kind } else { format!("{kind}_{texture}") };
                    let color =
                        rage_formats::shaders::color(shader.name_hash).map_or(GREY, |c| (c[0], c[1], c[2], c[3]));
                    materials.push((key, Material { name, texture, color, image: None }));
                    materials.len() - 1
                }
            };
            let flat =
                |v: &[[f32; 3]], f: &dyn Fn([f32; 3]) -> [f32; 3]| v.iter().flat_map(|p| f(*p)).collect::<Vec<_>>();
            meshes.push(Mesh {
                part: part.name.clone(),
                positions: flat(&m.positions, &|p| math::transform_point(&part.transform, p)),
                normals: flat(&m.normals, &|n| math::transform_dir(&part.transform, n)),
                uvs: m.uvs.iter().flatten().copied().collect(),
                indices: m.indices.clone(),
                material: material as u32,
            });
        }
    }
    Model { meshes, materials: materials.into_iter().map(|(_, m)| m).collect() }
}

/// A plugin's model for the 3D view: one drawable per part, one shader per
/// material, the materials' pixels as embedded textures.
pub fn from_plugin_model(model: &Model, name: &str) -> crate::models::LoadedModel {
    let mut textures = Vec::new();
    let shaders: Vec<drawable::Shader> = model
        .materials
        .iter()
        .enumerate()
        .map(|(i, m)| {
            // Plugin materials get hashes of their own, so they never take a game shader's colour.
            let name_hash = rage_core::hash::joaat(&format!("plugin:{i}:{}", m.name));
            let (r, g, b, a) = m.color;
            rage_formats::shaders::register(name_hash, m.name.clone(), [r, g, b, a]);
            let mut texture_names = Vec::new();
            let mut samplers = Vec::new();
            if let Some(img) = &m.image {
                let tex = if m.texture.is_empty() { format!("{}_texture", m.name) } else { m.texture.clone() };
                textures.push(Texture {
                    name: tex.clone(),
                    width: img.width,
                    height: img.height,
                    depth: 1,
                    levels: 1,
                    format: rage_formats::texture::Format::R8G8B8A8_UNORM,
                    dimension: rage_formats::texture::Dimension::Texture2D,
                    usage: 0,
                    data: img.rgba.clone(),
                });
                texture_names.push(tex.clone());
                samplers.push((drawable::param::DIFFUSE_TEX, tex));
            }
            drawable::Shader { name_hash, textures: texture_names, samplers }
        })
        .collect();
    let dictionary = (!textures.is_empty()).then(|| TextureDictionary {
        textures,
        errors: Vec::new(),
        layout: rage_formats::ytd::Layout::Enhanced,
        version: 0,
        root: Vec::new(),
        stored: Vec::new(),
    });
    // One drawable per part, in the order the parts first appear, each with
    // the shaders of the materials it uses.
    let mut parts: Vec<(String, Vec<drawable::Mesh>, Vec<u32>)> = Vec::new();
    for m in &model.meshes {
        let chunk = |v: &[f32]| v.as_chunks::<3>().0.to_vec();
        let mesh = drawable::Mesh {
            positions: chunk(&m.positions),
            normals: chunk(&m.normals),
            uvs: m.uvs.as_chunks::<2>().0.to_vec(),
            indices: m.indices.clone(),
            ..Default::default()
        };
        let part = if m.part.is_empty() { name.to_owned() } else { m.part.clone() };
        let at = match parts.iter().position(|(n, _, _)| *n == part) {
            Some(i) => i,
            None => {
                parts.push((part, Vec::new(), Vec::new()));
                parts.len() - 1
            }
        };
        let (_, list, used) = &mut parts[at];
        let local = match used.iter().position(|&u| u == m.material) {
            Some(i) => i,
            None => {
                used.push(m.material);
                used.len() - 1
            }
        };
        list.push(drawable::Mesh { shader: local as u16, ..mesh });
    }
    let drawables = parts
        .into_iter()
        .map(|(part, meshes, used)| {
            let shaders: Vec<drawable::Shader> = used.iter().map(|&u| shaders[u as usize].clone()).collect();
            let mut min = [f32::MAX; 3];
            let mut max = [f32::MIN; 3];
            for p in meshes.iter().flat_map(|m| &m.positions) {
                for k in 0..3 {
                    min[k] = min[k].min(p[k]);
                    max[k] = max[k].max(p[k]);
                }
            }
            if min[0] > max[0] {
                (min, max) = ([0.0; 3], [0.0; 3]);
            }
            let d = Drawable {
                name: part.clone(),
                meshes,
                shaders,
                textures: dictionary.clone(),
                bounds_min: min,
                bounds_max: max,
                skeleton: None,
            };
            (part, d)
        })
        .collect();
    crate::models::LoadedModel::from_drawables(drawables)
}

/// A file the user chose from disk for an action that asks for files (`pick`).
pub struct Picked {
    pub name: String,
    pub data: Vec<u8>,
}

/// What running an action produced.
#[derive(Default)]
pub struct RunResult {
    /// Files to save, named relative to where the user saves them.
    pub outputs: Vec<OutputFile>,
    /// Changes to game files (see [`apply_writes`]).
    pub writes: Vec<ModWrite>,
    pub log: Vec<LogLine>,
    /// Files the action ran on.
    pub files: usize,
    /// Files it failed on, with the reason (batch runs go on after a failure).
    pub failures: Vec<(String, String)>,
    pub cancelled: bool,
}

/// Most files one batch run handles.
const MAX_BATCH: usize = 200_000;

/// Whether a virtual path is a folder or an archive (something to list).
fn is_folder(files: &dyn GameFiles, vpath: &str) -> bool {
    files.list(vpath).is_ok()
}

/// The files below `root` (folders and archives included) that `action` applies to.
fn batch_files(files: &dyn GameFiles, root: &str, action: &Action) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut stack = vec![root.trim_end_matches('/').to_owned()];
    while let Some(dir) = stack.pop() {
        let mut entries = files.list(&dir)?;
        entries.reverse();
        for e in entries {
            let path = if dir.is_empty() { e.name.clone() } else { format!("{dir}/{}", e.name) };
            if e.is_dir {
                stack.push(path);
            } else if openrage_plugins::manifest::matches(&action.extensions, &e.name, true) {
                out.push(path);
                if out.len() > MAX_BATCH {
                    bail!("{root} holds more than {MAX_BATCH} matching files; pick a smaller folder");
                }
            }
        }
    }
    Ok(out)
}

/// Runs an action on files and folders of the game (folders only for batch
/// actions: then once for every matching file inside, with `progress` called
/// before each file; returning `false` from it cancels). Without folders,
/// all files go to the plugin in one call. `picked` files go with every call.
#[allow(clippy::too_many_arguments)]
pub fn run_action(
    host: &Host,
    plugin: &Plugin,
    component: &Component,
    files: Arc<dyn GameFiles>,
    action_id: &str,
    targets: &[String],
    picked: &[Picked],
    progress: &mut dyn FnMut(usize, usize, &str) -> bool,
) -> Result<RunResult> {
    let action = plugin.manifest.action(action_id).with_context(|| format!("no action {action_id:?}"))?;
    if !action.pick.is_empty() && picked.is_empty() {
        bail!("{} needs a file from disk ({})", action.title, action.pick.join(", "));
    }
    let picked_inputs: Vec<Input> =
        picked.iter().map(|p| Input { vpath: p.name.clone(), data: p.data.clone(), picked: true }).collect();
    let mut result = RunResult::default();

    // (path, where outputs go relative to the save folder)
    let mut jobs: Vec<(String, String)> = Vec::new();
    let mut batch = false;
    for target in targets {
        let target = target.replace('\\', "/");
        if is_folder(&*files, &target) {
            if !action.batch {
                bail!("{} works on files, not on folders like {target}", action.title);
            }
            batch = true;
            let top = target.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_owned();
            for path in batch_files(&*files, &target, action)? {
                let rel = path[target.trim_end_matches('/').len()..].trim_start_matches('/');
                let dir = rel.rsplit_once('/').map_or("", |(d, _)| d);
                let prefix =
                    [top.as_str(), dir].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join("/");
                jobs.push((path, prefix));
            }
        } else {
            jobs.push((target, String::new()));
        }
    }

    let finish = |result: &mut RunResult, outcome: openrage_plugins::Outcome<Vec<OutputFile>>, prefix: &str| {
        result.log.extend(outcome.log);
        for w in outcome.writes {
            result.writes.retain(|o| !o.vpath.eq_ignore_ascii_case(&w.vpath));
            result.writes.push(w);
        }
        outcome.value.map(|outputs| {
            for mut o in outputs {
                if !prefix.is_empty() {
                    o.name = format!("{prefix}/{}", o.name);
                }
                result.outputs.push(o);
            }
        })
    };

    if !batch {
        let mut inputs = Vec::new();
        for (path, _) in &jobs {
            let data = files.read(path).with_context(|| format!("reading {path}"))?;
            inputs.push(Input { vpath: path.clone(), data, picked: false });
        }
        inputs.extend(picked_inputs);
        result.files = jobs.len();
        let outcome = host.run_action(plugin, component, Arc::clone(&files), action_id, &inputs);
        finish(&mut result, outcome, "")?;
        return Ok(result);
    }

    let total = jobs.len();
    for (i, (path, prefix)) in jobs.iter().enumerate() {
        if !progress(i, total, path) {
            result.cancelled = true;
            return Ok(result);
        }
        let data = match files.read(path) {
            Ok(d) => d,
            Err(e) => {
                result.failures.push((path.clone(), format!("{e:#}")));
                continue;
            }
        };
        let mut inputs = vec![Input { vpath: path.clone(), data, picked: false }];
        inputs.extend(picked_inputs.iter().cloned());
        let outcome = host.run_action(plugin, component, Arc::clone(&files), action_id, &inputs);
        result.files += 1;
        if let Err(e) = finish(&mut result, outcome, prefix) {
            result.failures.push((path.clone(), format!("{e:#}")));
        }
    }
    // Two files of a batch may produce the same name in one folder: keep the first.
    let mut seen = std::collections::HashSet::new();
    result.outputs.retain(|o| seen.insert(o.name.to_ascii_lowercase()));
    progress(total, total, "");
    Ok(result)
}

/// A History group name for one run of a plugin action (with the time, so
/// each run can be undone on its own).
pub fn group_name(description: &str) -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    format!("{description} ({})", utc_minute(secs))
}

/// `YYYY-MM-DD HH:MM` (UTC) of a Unix time.
fn utc_minute(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let (hour, minute) = ((secs % 86_400) / 3600, (secs % 3600) / 60);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02} UTC")
}

/// Loose files plugins may not write: anything a game or a loader could run.
const BLOCKED: &[&str] = &["asi", "dll", "exe", "bat", "cmd", "com", "ps1", "vbs", "js", "sh", "scr", "msi", "lnk"];

/// Applies the changes a plugin asked for to the `mods` folder: files inside
/// archives through the archive editor, loose files directly; everything is
/// recorded in the journal under `group` so History can undo it at once.
/// Returns one line per changed archive or file.
pub fn apply_writes(
    game_dir: &Path,
    keys: Option<Arc<GtaVKeys>>,
    writes: &[ModWrite],
    description: &str,
    group: &str,
) -> Result<Vec<String>> {
    use crate::install::{self, Op};
    let mut per_archive: Vec<(String, Vec<(String, Op)>)> = Vec::new();
    let mut loose: Vec<(String, &[u8])> = Vec::new();
    for w in writes {
        match install::split(game_dir, &w.vpath) {
            Some(t) if t.inner.is_empty() => bail!("{} is an archive; plugins change files inside it", w.vpath),
            Some(t) => match per_archive.iter_mut().find(|(a, _)| a.eq_ignore_ascii_case(&t.archive)) {
                Some((_, ops)) => ops.push((t.inner, Op::Set(w.data.clone()))),
                None => per_archive.push((t.archive, vec![(t.inner, Op::Set(w.data.clone()))])),
            },
            None => {
                let rel = w.vpath.strip_prefix("mods/").unwrap_or(&w.vpath);
                if rel.split('/').any(|seg| seg.to_ascii_lowercase().ends_with(".rpf")) {
                    bail!("{}: no such archive in the game", w.vpath);
                }
                let ext = openrage_plugins::manifest::extension(rel);
                if BLOCKED.contains(&ext.as_str()) {
                    bail!("{}: plugins may not write .{ext} files", w.vpath);
                }
                loose.push((rel.to_owned(), &w.data));
            }
        }
    }
    let mut done = Vec::new();
    for (archive, ops) in &per_archive {
        let applied = install::apply_grouped(game_dir, keys.clone(), archive, ops, description, Some(group))?;
        let copied = if applied.created { " (copied there first)" } else { "" };
        done.push(format!("{} file(s) in mods/{archive}{copied}", ops.len()));
    }
    if !loose.is_empty() {
        let mods = install::mods_dir_or_create(game_dir)?;
        let journal = crate::journal::Journal::new(&mods).grouped(Some(group));
        for (rel, data) in loose {
            journal.write_file(false, &rel, data, description)?;
            done.push(format!("mods/{rel}"));
        }
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_writes_to_mods_with_undo() {
        let tmp = std::env::temp_dir().join(format!("openrage-app-plugin-writes-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("common")).unwrap();
        let mut w = rage_rpf::ArchiveWriter::new();
        w.add_file("data/a.txt", b"original".to_vec()).unwrap();
        fs::write(tmp.join("test.rpf"), w.build().unwrap()).unwrap();
        let write = |vpath: &str, data: &[u8]| ModWrite { vpath: vpath.into(), data: data.to_vec() };

        let done = apply_writes(
            &tmp,
            None,
            &[
                write("test.rpf/data/a.txt", b"patched"),
                write("test.rpf/data/b.txt", b"new"),
                write("common/x.meta", b"loose"),
            ],
            "Test: patch",
            "Test: patch (now)",
        )
        .unwrap();
        assert_eq!(done, ["2 file(s) in mods/test.rpf (copied there first)", "mods/common/x.meta"]);
        let game = GameDir::new(&tmp, None);
        assert_eq!(game.read("test.rpf/data/a.txt").unwrap(), b"patched");
        assert_eq!(game.read("test.rpf/data/b.txt").unwrap(), b"new");
        assert_eq!(fs::read(tmp.join("mods/common/x.meta")).unwrap(), b"loose");
        // The game's own archive is untouched.
        let original = rage_rpf::Rpf7::open(&tmp.join("test.rpf"), None).unwrap();
        assert_eq!(original.entries().len(), 3);

        for bad in ["test.rpf", "missing.rpf/a.txt", "dinput8.dll", "scripts/x.asi"] {
            assert!(apply_writes(&tmp, None, &[write(bad, b"x")], "Test", "Test (bad)").is_err(), "{bad}");
        }

        // One History group: undone at once.
        let journal = crate::journal::Journal::new(&tmp.join("mods"));
        assert_eq!(journal.groups().unwrap(), [("Test: patch (now)".to_owned(), 3)]);
        journal.undo_group("Test: patch (now)").unwrap();
        assert_eq!(game.read("test.rpf/data/a.txt").unwrap(), b"original");
        assert!(!tmp.join("mods/common/x.meta").exists());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn converts_models_both_ways() {
        let tri = |shader: u16| drawable::Mesh {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            normals: vec![[0.0, 0.0, 1.0]; 3],
            uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
            indices: vec![0, 1, 2],
            shader,
            ..Default::default()
        };
        let shader = |t: &str| drawable::Shader {
            name_hash: rage_core::hash::joaat("default"),
            textures: vec![t.to_owned()],
            samplers: vec![(drawable::param::DIFFUSE_SAMPLER, t.to_owned())],
        };
        let body = Drawable {
            name: "body".into(),
            meshes: vec![tri(0), tri(1)],
            shaders: vec![shader("paint"), shader("glass")],
            textures: None,
            bounds_min: [0.0; 3],
            bounds_max: [1.0; 3],
            skeleton: None,
        };
        // A wheel without shaders of its own uses the body's.
        let wheel = Drawable { name: "wheel".into(), meshes: vec![tri(1)], shaders: Vec::new(), ..body.clone() };
        let loaded = crate::models::LoadedModel::from_drawables(vec![("body".into(), body), ("wheel".into(), wheel)]);
        let model = to_plugin_model(&loaded);
        assert_eq!(model.meshes.len(), 3);
        assert_eq!(model.materials.len(), 2);
        assert_eq!(model.materials[1].texture, "glass");
        assert_eq!(model.meshes[2].part, "wheel");
        assert_eq!(model.meshes[2].material, 1);

        let back = from_plugin_model(&model, "x");
        assert_eq!(back.drawables.len(), 2);
        let (_, wheel) = &back.drawables[1];
        // The wheel only carries the one material it uses.
        assert_eq!(wheel.shaders.len(), 1);
        assert_eq!(wheel.meshes[0].shader, 0);
        assert_eq!(wheel.meshes[0].positions[1], [1.0, 0.0, 0.0]);
    }

    #[test]
    fn formats_times() {
        assert_eq!(utc_minute(0), "1970-01-01 00:00 UTC");
        assert_eq!(utc_minute(1_794_571_380), "2026-11-13 12:03 UTC");
        assert_eq!(utc_minute(951_782_400), "2000-02-29 00:00 UTC");
    }

    #[test]
    fn enabling_stores_grants() {
        let mut cfg = Config::default();
        enable(&mut cfg, "strings", &[]);
        enable(&mut cfg, "probe", &[Permission::ReadGame]);
        enable(&mut cfg, "strings", &[Permission::ReadGame]);
        assert_eq!(cfg.plugins, [("strings".into(), "read-game".into()), ("probe".into(), "read-game".into())]);
        assert_eq!(grants(&cfg)["probe"], [Permission::ReadGame]);
        assert!(disable(&mut cfg, "strings"));
        assert!(!disable(&mut cfg, "strings"));
        assert_eq!(cfg.plugins.len(), 1);
    }

    #[test]
    fn reads_and_lists_a_game_folder_with_mods() {
        let tmp = std::env::temp_dir().join(format!("openrage-app-plugins-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("common/data")).unwrap();
        fs::create_dir_all(tmp.join("mods/common/data")).unwrap();
        fs::write(tmp.join("common/data/a.meta"), "base").unwrap();
        fs::write(tmp.join("common/data/b.meta"), "base b").unwrap();
        fs::write(tmp.join("mods/common/data/a.meta"), "modded").unwrap();
        fs::write(tmp.join("mods/common/data/c.meta"), "new").unwrap();
        let game = GameDir::new(&tmp, None);
        assert_eq!(game.game(), Some(GameId::Gta5));
        assert_eq!(game.read("common/data/a.meta").unwrap(), b"modded");
        assert_eq!(game.read("common/data/b.meta").unwrap(), b"base b");
        let names: Vec<_> = game.list("common/data").unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(names, ["a.meta", "b.meta", "c.meta"]);
        assert!(game.list("").unwrap().iter().any(|e| e.name == "common" && e.is_dir));
        assert!(game.read("common").is_err());
        let _ = fs::remove_dir_all(&tmp);
    }
}
