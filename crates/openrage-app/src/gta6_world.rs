//! The map of GTA VI: where its map files (`.#map`) are, what they place, and
//! the models for it.
//!
//! A map file covers a small piece of the world, and there are more than
//! 160,000, so reading all of them takes a minute or two. What is needed of
//! them is kept (`<cache dir>/openrage/gta6-world-<game>.bin`) until the search
//! index changes.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use rage_core::bytes::LeRead;
use rage_core::hash::joaat;
use rage_formats::drawable::Drawable;
use rage_formats::math::{self, Mat4};
use rage_rpf::Rpf9;
use rage_rpf::vfs::Node as Node9;
use rayon::prelude::*;

use crate::models::LoadedModel;
use crate::world::MapScene;

const MAGIC: &[u8; 8] = b"ORG6MAP2";

/// Levels of detail: the models seen close, and the ones that stand for whole
/// neighbourhoods from afar.
const CLOSE: [u8; 2] = [0, 5];
const FAR: u8 = 3;
/// The levels between, up to the far one.
const LEVELS: std::ops::RangeInclusive<u8> = 1..=FAR;
/// How well textures are read in views of many models.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quality {
    /// Seen from kilometres away.
    Far,
    Near,
    /// From up close: normal and specular maps too.
    Close,
}

impl Quality {
    fn side(self) -> u32 {
        match self {
            Self::Far => 256,
            Self::Near => 512,
            Self::Close => 1024,
        }
    }
}

/// Something a map file places.
#[derive(Debug, Clone, PartialEq)]
struct Object {
    /// Hash of the model's name.
    model: u32,
    position: [f32; 3],
    rotation: [f32; 4],
    scale: [f32; 2],
}

impl Object {
    fn transform(&self) -> Mat4 {
        let [x, y, z, w] = self.rotation;
        let [flat, high] = self.scale;
        math::from_trs(self.position, [-x, -y, -z, w], [flat, flat, high])
    }
}

/// An object that stands for others from afar: for those whose parent it is.
#[derive(Debug, Clone, PartialEq)]
struct Node {
    /// The map file (the hash of its name) and the place in it.
    key: (u32, u32),
    parent: Option<(u32, u32)>,
    level: u8,
    /// Nearer than this, the others are shown.
    reach: f32,
    object: Object,
}

/// What is kept of the map files.
#[derive(Debug, Default, PartialEq)]
pub struct Map {
    /// Map files with the box around what they place.
    files: Vec<(String, [f32; 3], [f32; 3])>,
    /// The objects that stand for others from afar.
    nodes: Vec<Node>,
    /// Models in dictionaries, by the hash of their name: the dictionary and
    /// the place in it.
    children: HashMap<u32, (String, u16)>,
}

/// Files of the index, by the archive they are in.
fn by_archive(list: Vec<(String, String)>, keep: impl Fn(&str) -> bool) -> BTreeMap<String, Vec<String>> {
    let mut found: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (_, vpath) in list.into_iter().filter(|(_, v)| keep(v)) {
        if let Some((archive, file)) = vpath.rsplit_once(".rpf/") {
            found.entry(format!("{archive}.rpf")).or_default().push(file.to_owned());
        }
    }
    found
}

fn each_file(
    game: &Path,
    archives: &BTreeMap<String, Vec<String>>,
    work: impl Fn(&Rpf9, usize, String) + Sync,
    done: &(dyn Fn() + Sync),
) {
    archives.par_iter().for_each(|(archive, files)| {
        let Ok((Node9::Rpf9Dir(rpf, _), _)) = rage_mods::resolve(game, archive, None) else { return };
        let Ok(entries) = rpf.entries() else { return };
        files.par_iter().for_each(|file| {
            if let Some(i) = rpf.find(&entries, file) {
                work(&rpf, i, format!("{archive}/{file}"));
            }
            done();
        });
    });
}

/// The part of the text between two others.
fn between<'a>(line: &'a str, start: &str, end: &str) -> Option<&'a str> {
    line.strip_prefix(start)?.split(end).next()
}

fn attributes<const N: usize>(line: &str, names: [&str; N]) -> Option<[f32; N]> {
    let mut out = [0.0; N];
    for (out, name) in out.iter_mut().zip(names) {
        let at = line.find(&format!(" {name}=\""))? + name.len() + 3;
        *out = line[at..].split('"').next()?.parse().ok()?;
    }
    Some(out)
}

/// The hash a name in the XML stands for.
fn name_hash(name: &str) -> u32 {
    let hash = name.strip_prefix("hash_").filter(|h| h.len() == 8).and_then(|h| u32::from_str_radix(h, 16).ok());
    hash.unwrap_or_else(|| joaat(name))
}

/// What a map file holds.
#[derive(Default)]
struct Placements {
    /// Hash of the file's name.
    name: u32,
    /// The box around what it places.
    extent: Option<[[f32; 3]; 2]>,
    /// In the order of the file.
    entries: Vec<Entry>,
}

/// A placed object with its place among the levels of detail.
struct Entry {
    level: u8,
    parent: Option<(u32, u32)>,
    reach: f32,
    object: Object,
}

impl Placements {
    fn has_close(&self) -> bool {
        self.entries.iter().any(|e| CLOSE.contains(&e.level))
    }

    fn close(self) -> impl Iterator<Item = Object> {
        self.entries.into_iter().filter(|e| CLOSE.contains(&e.level)).map(|e| e.object)
    }
}

/// What a map file places. The XML is read by lines: there is a lot of it,
/// and it is our own.
fn read_map(xml: &str) -> Placements {
    let mut map = Placements::default();
    let (mut lo, mut hi) = (None, None);
    let mut now = Object { model: 0, position: [0.0; 3], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 2] };
    // The files with the objects that stand for this one's.
    let mut parents: Vec<u32> = Vec::new();
    let (mut in_parents, mut in_entities, mut in_extensions) = (false, false, false);
    let (mut reach, mut parent_index) = (0.0, -1.0);
    let value = |line: &str| attributes(line, ["value"]).map(|[v]| v);
    for line in xml.lines().map(str::trim) {
        if in_extensions {
            in_extensions = line != "</extensions>";
        } else if in_entities {
            if line == "<extensions>" || line.starts_with("<extensions ") && !line.ends_with("/>") {
                in_extensions = true;
            } else if line == "</entities>" {
                in_entities = false;
            } else if let Some(name) = between(line, "<archetypeName>", "<") {
                now.model = name_hash(name);
            } else if line.starts_with("<position ") {
                now.position = attributes(line, ["x", "y", "z"]).unwrap_or_default();
            } else if line.starts_with("<rotation ") {
                now.rotation = attributes(line, ["x", "y", "z", "w"]).unwrap_or([0.0, 0.0, 0.0, 1.0]);
            } else if line.starts_with("<scaleXY ") {
                now.scale[0] = value(line).unwrap_or(1.0);
            } else if line.starts_with("<scaleZ ") {
                now.scale[1] = value(line).unwrap_or(1.0);
            } else if line.starts_with("<parentIndex ") {
                parent_index = value(line).unwrap_or(-1.0);
            } else if line.starts_with("<childLodDist ") {
                reach = value(line).unwrap_or(0.0);
            } else if line.starts_with("<lodLevel ") {
                let level = value(line).map_or(u8::MAX, |v| v as u8);
                map.entries.push(Entry { level, parent: None, reach, object: now.clone() });
            } else if line.starts_with("<hash_84799847 ")
                && let (Some(entry), Some(file)) = (map.entries.last_mut(), value(line))
                && parent_index >= 0.0
                && file >= 0.0
            {
                entry.parent = parents.get(file as usize).map(|file| (*file, parent_index as u32));
            }
        } else if line.starts_with("<entities>") || line.starts_with("<entities ") {
            in_entities = !line.ends_with("/>");
        } else if line == "<hash_75D16ADF>" {
            in_parents = true;
        } else if line == "</hash_75D16ADF>" {
            in_parents = false;
        } else if let Some(name) = between(line, "<Item>", "<").filter(|_| in_parents) {
            parents.push(name_hash(name));
        } else if let Some(name) = between(line, "<name>", "<").filter(|_| map.name == 0) {
            map.name = name_hash(name);
        } else if line.starts_with("<entitiesExtentsMin") {
            lo = attributes(line, ["x", "y", "z"]);
        } else if line.starts_with("<entitiesExtentsMax") {
            hi = attributes(line, ["x", "y", "z"]);
        }
    }
    map.extent = lo.zip(hi).map(|(lo, hi)| [lo, hi]);
    map
}

/// The hashes of the names of a dictionary's models.
fn dictionary(system: &[u8]) -> Vec<u32> {
    let list = system.u64_at(32).and_then(|p| usize::try_from(p.checked_sub(0x5000_0000)?).ok());
    let count = system.u16_at(40).unwrap_or(0);
    let Some(list) = list else { return Vec::new() };
    (0..usize::from(count)).map_while(|i| system.u32_at(list.checked_add(i * 4)?)).collect()
}

fn build(game: &Path, index: &rage_index::Index, progress: &(dyn Fn(usize, usize) + Sync)) -> Result<Map> {
    let maps = by_archive(index.files_with_extensions(&[".#map"])?, |_| true);
    let dictionaries = by_archive(index.files_with_extensions(&[".#dd"])?, |v| v.contains("/map/"));
    let total = maps.values().chain(dictionaries.values()).map(Vec::len).sum();
    let count = AtomicUsize::new(0);
    let done = || {
        let n = count.fetch_add(1, Ordering::Relaxed) + 1;
        if n.is_multiple_of(500) {
            progress(n, total);
        }
    };
    let map = Mutex::new(Map::default());
    let lock = || map.lock().unwrap_or_else(|e| e.into_inner());
    each_file(
        game,
        &maps,
        |rpf, i, vpath| {
            let Ok(xml) = crate::metadata::from_rpf9(rpf, i) else { return };
            let placements = read_map(&xml.text);
            let mut map = lock();
            if let Some([lo, hi]) = placements.extent.filter(|_| placements.has_close()) {
                map.files.push((vpath, lo, hi));
            }
            let name = placements.name;
            let nodes = placements.entries.into_iter().enumerate().filter(|(_, e)| LEVELS.contains(&e.level));
            map.nodes.extend(nodes.map(|(k, e)| Node {
                key: (name, k as u32),
                parent: e.parent,
                level: e.level,
                reach: e.reach,
                object: e.object,
            }));
        },
        &done,
    );
    each_file(
        game,
        &dictionaries,
        |rpf, i, vpath| {
            let Ok(entries) = rpf.entries() else { return };
            let Some((system, _)) = rpf.resource_parts(&entries[i]) else { return };
            let Ok(start) = rpf.read_start(&entries[i], system as usize) else { return };
            let names = dictionary(&start);
            let mut map = lock();
            for (k, name) in names.into_iter().enumerate() {
                map.children.entry(name).or_insert_with(|| (vpath.clone(), k as u16));
            }
        },
        &done,
    );
    let mut map = map.into_inner().unwrap_or_else(|e| e.into_inner());
    map.files.sort_by(|a, b| a.0.cmp(&b.0));
    map.nodes.sort_by_key(|n| n.key);
    Ok(map)
}

fn cache_path(game: &Path) -> Option<PathBuf> {
    crate::paths::game_cache(game, "gta6-world", "bin")
}

fn encode(map: &Map, stamp: i64) -> Vec<u8> {
    let mut out = Vec::from(MAGIC);
    out.extend(stamp.to_le_bytes());
    let text = |out: &mut Vec<u8>, s: &str| {
        out.extend((s.len() as u32).to_le_bytes());
        out.extend(s.as_bytes());
    };
    let floats = |out: &mut Vec<u8>, v: &[f32]| v.iter().for_each(|f| out.extend(f.to_le_bytes()));
    out.extend((map.files.len() as u32).to_le_bytes());
    for (path, lo, hi) in &map.files {
        text(&mut out, path);
        floats(&mut out, lo);
        floats(&mut out, hi);
    }
    out.extend((map.nodes.len() as u32).to_le_bytes());
    for n in &map.nodes {
        let parent = n.parent.unwrap_or((u32::MAX, u32::MAX));
        for v in [n.key.0, n.key.1, parent.0, parent.1, u32::from(n.level), n.object.model] {
            out.extend(v.to_le_bytes());
        }
        floats(&mut out, &[n.reach]);
        floats(&mut out, &n.object.position);
        floats(&mut out, &n.object.rotation);
        floats(&mut out, &n.object.scale);
    }
    // Dictionaries once, then their models.
    let mut by_path: BTreeMap<&str, Vec<(u32, u16)>> = BTreeMap::new();
    for (name, (path, k)) in &map.children {
        by_path.entry(path).or_default().push((*name, *k));
    }
    out.extend((by_path.len() as u32).to_le_bytes());
    for (path, models) in by_path {
        text(&mut out, path);
        out.extend((models.len() as u32).to_le_bytes());
        for (name, k) in models {
            out.extend(name.to_le_bytes());
            out.extend(u32::from(k).to_le_bytes());
        }
    }
    out
}

fn decode(data: &[u8], stamp: i64) -> Option<Map> {
    let rest = data.strip_prefix(MAGIC)?.strip_prefix(&stamp.to_le_bytes())?;
    fn floats<const N: usize>(number: &mut impl FnMut() -> Option<u32>) -> Option<[f32; N]> {
        let mut out = [0.0; N];
        for f in &mut out {
            *f = f32::from_bits(number()?);
        }
        Some(out)
    }
    let mut reader = Reader { data: rest, at: 0 };
    let mut map = Map::default();
    for _ in 0..reader.number()? {
        let path = reader.text()?;
        let (lo, hi) = (floats(&mut || reader.number())?, floats(&mut || reader.number())?);
        map.files.push((path, lo, hi));
    }
    for _ in 0..reader.number()? {
        let mut next = || reader.number();
        let (key, parent) = ((next()?, next()?), (next()?, next()?));
        let (level, model) = (next()? as u8, next()?);
        let [reach] = floats(&mut next)?;
        let object =
            Object { model, position: floats(&mut next)?, rotation: floats(&mut next)?, scale: floats(&mut next)? };
        map.nodes.push(Node { key, parent: (parent.0 != u32::MAX).then_some(parent), level, reach, object });
    }
    for _ in 0..reader.number()? {
        let path = reader.text()?;
        for _ in 0..reader.number()? {
            let (name, k) = (reader.number()?, reader.number()?);
            map.children.insert(name, (path.clone(), k as u16));
        }
    }
    Some(map)
}

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn number(&mut self) -> Option<u32> {
        let v = self.data.u32_at(self.at)?;
        self.at += 4;
        Some(v)
    }

    fn text(&mut self) -> Option<String> {
        let len = self.number()? as usize;
        let bytes = self.data.get(self.at..self.at.checked_add(len)?)?;
        self.at += len;
        String::from_utf8(bytes.to_vec()).ok()
    }
}

/// The game's map: kept, or read from its map files. `progress` gets the files
/// read and their number.
pub fn load(game: &Path, progress: &(dyn Fn(usize, usize) + Sync)) -> Result<Arc<Map>> {
    static LOADED: Mutex<Option<(PathBuf, i64, Arc<Map>)>> = Mutex::new(None);
    let index = open_index(game)?;
    let stamp = index.changed_at()?.unwrap_or(0);
    let mut loaded = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((of, at, map)) = &*loaded
        && of == game
        && *at == stamp
    {
        return Ok(Arc::clone(map));
    }
    let path = cache_path(game).context("no cache directory")?;
    let map = match std::fs::read(&path).ok().and_then(|data| decode(&data, stamp)) {
        Some(map) => map,
        None => {
            let map = rage_rpf::in_own_threads(|| build(game, &index_of(game)?, progress))?;
            let partial = path.with_extension("bin.part");
            // The cache only saves time.
            let _ = std::fs::write(&partial, encode(&map, stamp)).and_then(|()| std::fs::rename(&partial, &path));
            map
        }
    };
    let map = Arc::new(map);
    *loaded = Some((game.to_path_buf(), stamp, Arc::clone(&map)));
    Ok(map)
}

fn index_of(game: &Path) -> Result<rage_index::Index> {
    let path = crate::index_path(game).filter(|p| p.exists()).context("the search index is not built yet")?;
    Ok(rage_index::Index::open(&path)?)
}

fn open_index(game: &Path) -> Result<rage_index::Index> {
    let index = index_of(game)?;
    anyhow::ensure!(index.is_complete()?, "the search index is still being built");
    Ok(index)
}

/// Where a model is.
#[derive(Debug, Clone)]
enum Source {
    File(String),
    Child(String, u16),
}

/// The files of models, by the hash of their name.
fn model_files(game: &Path) -> Result<HashMap<u32, String>> {
    let list = open_index(game)?.files_with_extensions(&[".#dr", ".#ft"])?;
    let mut found = HashMap::with_capacity(list.len());
    for (name, vpath) in list {
        if let Some((stem, _)) = name.rsplit_once('.') {
            found.entry(joaat(stem)).or_insert(vpath);
        }
    }
    Ok(found)
}

/// A loaded model: the hash of its name, its name, and itself.
pub type Loaded = (u32, String, LoadedModel);

/// Where the game's models are.
struct Models<'a> {
    game: &'a Path,
    files: &'a HashMap<u32, String>,
    map: &'a Map,
}

impl Models<'_> {
    fn source(&self, model: u32) -> Option<Source> {
        let file = self.files.get(&model).map(|file| Source::File(file.clone()));
        file.or_else(|| self.map.children.get(&model).map(|(file, k)| Source::Child(file.clone(), *k)))
    }

    /// Loads models; those that cannot be read are left out.
    fn load(&self, wanted: &[u32], quality: Quality, progress: &(dyn Fn(usize, usize) + Sync)) -> Vec<Loaded> {
        // Archives are opened once: that takes longer than reading a model.
        let mut archives: BTreeMap<String, Vec<(String, u32, Source)>> = BTreeMap::new();
        for &model in wanted {
            let Some(source) = self.source(model) else { continue };
            let (Source::File(vpath) | Source::Child(vpath, _)) = &source;
            if let Some((archive, file)) = vpath.rsplit_once(".rpf/") {
                archives.entry(format!("{archive}.rpf")).or_default().push((file.to_owned(), model, source));
            }
        }
        let total = archives.values().map(Vec::len).sum();
        let count = &AtomicUsize::new(0);
        progress(0, total);
        archives
            .into_par_iter()
            .flat_map(|(archive, files)| {
                let opened = match rage_mods::resolve(self.game, &archive, None) {
                    Ok((Node9::Rpf9Dir(rpf, _), _)) => rpf.entries().ok().map(|entries| (rpf, entries)),
                    _ => None,
                };
                files.into_par_iter().filter_map(move |(file, model, source)| {
                    let loaded = opened.as_ref().and_then(|(rpf, entries)| {
                        let i = rpf.find(entries, &file)?;
                        crate::progress::quietly(|| load_model(self.game, rpf, i, &source, quality)).ok()
                    });
                    progress(count.fetch_add(1, Ordering::Relaxed) + 1, total);
                    loaded.map(|(name, loaded)| (model, name, loaded))
                })
            })
            .collect()
    }
}

/// Puts objects together: every model once, with all its places.
fn scene(
    game: &Path,
    map: &Map,
    objects: &[Object],
    quality: Quality,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<MapScene> {
    let files = model_files(game)?;
    let models = Models { game, files: &files, map };
    let mut places: HashMap<u32, Vec<Mat4>> = HashMap::new();
    for o in objects {
        places.entry(o.model).or_default().push(o.transform());
    }
    let (wanted, unknown): (Vec<u32>, Vec<u32>) = places.keys().partition(|m| models.source(**m).is_some());
    let mut missing: Vec<(String, usize)> =
        unknown.into_iter().map(|m| (rage_formats::names::value(m), places[&m].len())).collect();
    missing.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let loaded = models.load(&wanted, quality, progress);
    let models = loaded
        .into_iter()
        .map(|(model, name, loaded)| (name, loaded, places.remove(&model).unwrap_or_default()))
        .collect();
    Ok(MapScene { models, objects: objects.len(), missing })
}

fn load_model(game: &Path, rpf: &Rpf9, i: usize, source: &Source, quality: Quality) -> Result<(String, LoadedModel)> {
    let (Source::File(vpath) | Source::Child(vpath, _)) = source;
    let mut drawables: Vec<(String, Drawable)> = crate::gta6_models::load(rpf, i)?;
    if let Source::Child(_, k) = source {
        let k = usize::from(*k);
        anyhow::ensure!(k < drawables.len(), "no such model in the dictionary");
        drawables = vec![drawables.swap_remove(k)];
    }
    let name = match source {
        Source::File(_) => vpath.rsplit('/').next().unwrap_or(vpath).to_owned(),
        Source::Child(..) => drawables[0].0.clone(),
    };
    let textures = crate::gta6_textures::shared_for(game, &drawables, quality.side(), quality == Quality::Close);
    let mut model = LoadedModel::from_drawables(drawables);
    model.set_shared(textures);
    Ok((name.trim_end_matches(".#dr").trim_end_matches(".#ft").to_owned(), model))
}

/// The whole map, as it looks from afar.
pub fn overview(game: &Path, progress: &Progress) -> Result<MapScene> {
    let map = load(game, &|done, total| progress("Reading the game's map files", done, total))?;
    let far: Vec<Object> = map.nodes.iter().filter(|n| n.level == FAR).map(|n| n.object.clone()).collect();
    anyhow::ensure!(!far.is_empty(), "the map files of this game have no distant view");
    scene(game, &map, &far, Quality::Far, &|done, total| progress("Loading the map's models", done, total))
}

/// Gets what is being done, how much of it is done, and how much there is.
pub type Progress<'a> = dyn Fn(&str, usize, usize) + Sync + 'a;

/// How far around a map file's objects its surroundings are shown (metres).
pub const AROUND: f32 = 120.0;

/// A map file, with what the others place around it.
pub fn place(game: &Path, vpath: &str, around: f32, progress: &Progress) -> Result<MapScene> {
    let own = read_map(&crate::metadata::load(game, None, vpath)?.text);
    let map = load(game, &|done, total| progress("Reading the game's map files", done, total))?;
    let (extent, has_close) = (own.extent, own.has_close());
    let mut objects: Vec<Object> =
        if has_close { own.close().collect() } else { own.entries.into_iter().map(|e| e.object).collect() };
    if let Some([lo, hi]) = extent.filter(|_| has_close) {
        let middle = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
        let near = |p: &[f32; 3]| (0..2).all(|k| (p[k] - middle[k]).abs() <= around);
        let others = map.files.iter().filter(|(path, lo, hi)| {
            path != vpath && (0..2).all(|k| lo[k] <= middle[k] + around && hi[k] >= middle[k] - around)
        });
        let others = by_archive(others.map(|(path, ..)| (String::new(), path.clone())).collect(), |_| true);
        let found = Mutex::new(Vec::new());
        each_file(
            game,
            &others,
            |rpf, i, _| {
                let Ok(xml) = crate::metadata::from_rpf9(rpf, i) else { return };
                let near = read_map(&xml.text).close().filter(|o| near(&o.position));
                found.lock().unwrap_or_else(|e| e.into_inner()).extend(near);
            },
            &|| {},
        );
        objects.extend(found.into_inner().unwrap_or_else(|e| e.into_inner()));
    }
    scene(game, &map, &objects, Quality::Near, &|done, total| progress("Loading the map's models", done, total))
}

/// How near the camera a map file has to be for its close objects (metres).
const NEAR: f32 = 200.0;
/// How near objects have to be that stand for others but have none above them.
const ALONE: [f32; 4] = [0.0, 800.0, 4000.0, f32::MAX];

/// The whole map in the detail that fits where the camera is.
pub struct Lod {
    game: PathBuf,
    map: Arc<Map>,
    files: HashMap<u32, String>,
    /// Nodes by their key.
    by_key: HashMap<(u32, u32), u32>,
    /// What each node stands for, among the nodes.
    children: Vec<Vec<u32>>,
    roots: Vec<u32>,
}

/// The close objects of a map file, with the node they are part of.
#[derive(Default)]
pub struct Close {
    objects: Vec<(Option<u32>, Object)>,
}

/// What to draw: models with their places, and what is missing for more
/// detail, the most wanted first.
#[derive(Default)]
pub struct Cut {
    pub draw: HashMap<u32, Vec<Mat4>>,
    pub wanted: Vec<u32>,
}

fn distance(a: &[f32; 3], b: &[f32; 3]) -> f32 {
    (0..3).map(|k| (a[k] - b[k]).powi(2)).sum::<f32>().sqrt()
}

/// How far a point is from a box.
fn distance_to_box(p: &[f32; 3], lo: &[f32; 3], hi: &[f32; 3]) -> f32 {
    distance(p, &std::array::from_fn(|k| p[k].clamp(lo[k], hi[k].max(lo[k]))))
}

impl Lod {
    pub fn open(game: &Path, progress: &Progress) -> Result<Self> {
        let map = load(game, &|done, total| progress("Reading the game's map files", done, total))?;
        Ok(Self::new(game, map, model_files(game)?))
    }

    fn new(game: &Path, map: Arc<Map>, files: HashMap<u32, String>) -> Self {
        let by_key: HashMap<(u32, u32), u32> = map.nodes.iter().enumerate().map(|(i, n)| (n.key, i as u32)).collect();
        let mut children = vec![Vec::new(); map.nodes.len()];
        let mut roots = Vec::new();
        for (i, node) in map.nodes.iter().enumerate() {
            // Only of a higher level, so there are no circles.
            let parent = node.parent.and_then(|key| by_key.get(&key).copied());
            match parent.filter(|p| map.nodes[*p as usize].level > node.level) {
                Some(p) => children[p as usize].push(i as u32),
                None => roots.push(i as u32),
            }
        }
        Self { game: game.to_owned(), map, files, by_key, children, roots }
    }

    /// The models of the farthest level.
    pub fn far(&self) -> Vec<u32> {
        let far = self.map.nodes.iter().filter(|n| n.level == FAR).map(|n| n.object.model);
        far.collect::<HashSet<u32>>().into_iter().collect()
    }

    /// Whether the game has this model.
    pub fn has_model(&self, model: u32) -> bool {
        self.files.contains_key(&model) || self.map.children.contains_key(&model)
    }

    /// Loads models.
    pub fn load(&self, models: &[u32], quality: Quality, progress: &(dyn Fn(usize, usize) + Sync)) -> Vec<Loaded> {
        let of = Models { game: &self.game, files: &self.files, map: &self.map };
        of.load(models, quality, progress)
    }

    /// The map files with close objects near a place.
    pub fn near(&self, eye: &[f32; 3], range: f32) -> HashSet<u32> {
        let near = |(_, lo, hi): &&(String, [f32; 3], [f32; 3])| distance_to_box(eye, lo, hi) < NEAR * range;
        (0..).zip(&self.map.files).filter(|(_, f)| near(f)).map(|(i, _)| i).collect()
    }

    /// Reads the close objects of map files.
    pub fn read(&self, files: &[u32]) -> Vec<(u32, Close)> {
        let number: HashMap<&str, u32> = files.iter().map(|i| (self.map.files[*i as usize].0.as_str(), *i)).collect();
        let by_archive = by_archive(number.keys().map(|p| (String::new(), (*p).to_owned())).collect(), |_| true);
        let found = Mutex::new(Vec::new());
        each_file(
            &self.game,
            &by_archive,
            |rpf, i, vpath| {
                let Some(&number) = number.get(vpath.as_str()) else { return };
                let entries = crate::metadata::from_rpf9(rpf, i).map(|x| read_map(&x.text).entries).unwrap_or_default();
                let close = entries.into_iter().filter(|e| CLOSE.contains(&e.level));
                let node = |e: &Entry| e.parent.and_then(|key| self.by_key.get(&key).copied());
                let objects = close.map(|e| (node(&e), e.object)).collect();
                found.lock().unwrap_or_else(|e| e.into_inner()).push((number, Close { objects }));
            },
            &|| {},
        );
        found.into_inner().unwrap_or_else(|e| e.into_inner())
    }

    /// What to draw from a place, `range` times as far in detail as the game
    /// does, of the models there are, with the close
    /// objects read so far. An object gives way to those it stands for when
    /// all of them are there.
    pub fn cut<'a>(
        &self,
        eye: &[f32; 3],
        range: f32,
        loaded: &dyn Fn(u32) -> bool,
        unreadable: &dyn Fn(u32) -> bool,
        close: impl Iterator<Item = &'a Close>,
    ) -> Cut {
        let mut of_node: HashMap<u32, Vec<&Object>> = HashMap::new();
        let mut cut = Choice {
            lod: self,
            eye,
            range,
            loaded,
            unreadable,
            close: HashMap::new(),
            draw: Vec::new(),
            wanted: HashMap::new(),
        };
        for (node, object) in close.flat_map(|c| &c.objects) {
            match node {
                Some(node) => of_node.entry(*node).or_default().push(object),
                None if distance(eye, &object.position) < NEAR * range && cut.take(object) => cut.draw.push(object),
                None => {}
            }
        }
        cut.close = of_node;
        for &root in &self.roots {
            let node = &self.map.nodes[root as usize];
            if distance(eye, &node.object.position) < ALONE[usize::from(node.level.min(FAR))] {
                cut.visit(root);
            }
        }
        let mut wanted: Vec<(u32, f32)> = cut.wanted.into_iter().collect();
        wanted.sort_by(|a, b| a.1.total_cmp(&b.1));
        let mut draw: HashMap<u32, Vec<Mat4>> = HashMap::new();
        for object in cut.draw {
            draw.entry(object.model).or_default().push(object.transform());
        }
        Cut { draw, wanted: wanted.into_iter().map(|(m, _)| m).collect() }
    }
}

/// A cut in the making.
struct Choice<'a> {
    lod: &'a Lod,
    eye: &'a [f32; 3],
    range: f32,
    loaded: &'a dyn Fn(u32) -> bool,
    unreadable: &'a dyn Fn(u32) -> bool,
    close: HashMap<u32, Vec<&'a Object>>,
    draw: Vec<&'a Object>,
    /// Models to load, with how near they are.
    wanted: HashMap<u32, f32>,
}

impl<'a> Choice<'a> {
    fn expects(&self, model: u32) -> bool {
        self.lod.has_model(model) && !(self.unreadable)(model)
    }

    /// Whether an object's model is loaded; otherwise it is wanted. One the
    /// game does not have does not count.
    fn take(&mut self, object: &Object) -> bool {
        if (self.loaded)(object.model) {
            return true;
        }
        if self.expects(object.model) {
            let far = distance(self.eye, &object.position);
            let known = self.wanted.entry(object.model).or_insert(far);
            *known = known.min(far);
        }
        false
    }

    /// Draws a node or what it stands for; gives whether something stands
    /// where it is.
    fn visit(&mut self, node: u32) -> bool {
        let lod = self.lod;
        let (n, children) = (&lod.map.nodes[node as usize], &lod.children[node as usize]);
        if distance(self.eye, &n.object.position) < n.reach * self.range {
            let before = self.draw.len();
            let mut all = true;
            let mut any = false;
            for child in children {
                any = true;
                all &= self.visit(*child);
            }
            for object in self.close.remove(&node).unwrap_or_default() {
                any = true;
                if self.take(object) {
                    self.draw.push(object);
                } else {
                    all &= !self.expects(object.model);
                }
            }
            if any && all {
                return true;
            }
            self.draw.truncate(before);
        }
        if self.take(&n.object) {
            self.draw.push(&n.object);
            return true;
        }
        !self.expects(n.object.model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<CMapData>
 <name>hash_00000077</name>
 <hash_75D16ADF>
  <Item>hash_00000099</Item>
 </hash_75D16ADF>
 <entitiesExtentsMin x="1" y="2" z="3" />
 <entitiesExtentsMax x="4" y="5" z="6" />
 <entities>
  <Item type="CEntityDef">
   <rotation x="0" y="0" z="0" w="1" />
   <position x="10" y="20" z="30" />
   <archetypeName>Prop_A</archetypeName>
   <scaleXY value="2" />
   <scaleZ value="3" />
   <parentIndex value="-1" />
   <childLodDist value="0" />
   <lodLevel value="5" />
   <hash_84799847 value="-1" />
  </Item>
  <Item type="CEntityDef">
   <extensions>
    <position x="9" y="9" z="9" />
   </extensions>
   <position x="0" y="0" z="0" />
   <archetypeName>hash_0000ABCD</archetypeName>
   <scaleXY value="1" />
   <scaleZ value="1" />
   <parentIndex value="4" />
   <childLodDist value="60" />
   <lodLevel value="2" />
   <hash_84799847 value="0" />
  </Item>
 </entities>
</CMapData>"#;

    #[test]
    fn map_files_are_read() {
        let map = read_map(XML);
        assert_eq!((map.name, map.extent), (0x77, Some([[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]])));
        let [a, b] = &map.entries[..] else { panic!("two entries") };
        assert_eq!((a.level, a.object.model, a.parent), (5, joaat("prop_a"), None));
        assert_eq!(a.object.scale, [2.0, 3.0]);
        assert_eq!(a.object.transform()[12..15], [10.0, 20.0, 30.0]);
        assert_eq!((b.level, b.object.model, b.parent, b.reach), (2, 0xABCD, Some((0x99, 4)), 60.0));
    }

    fn node(key: (u32, u32), parent: Option<(u32, u32)>, level: u8, model: u32) -> Node {
        let object = Object { model, position: [0.0; 3], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 2] };
        Node { key, parent, level, reach: 100.0, object }
    }

    #[test]
    fn the_cache_reads_back() {
        let map = Map {
            files: vec![("a.rpf/b.#map".into(), [1.0; 3], [2.0; 3])],
            nodes: vec![node((1, 0), None, 3, 7), node((2, 0), Some((1, 0)), 2, 8)],
            children: HashMap::from([(7, ("c.rpf/d.#dd".to_owned(), 3))]),
        };
        let data = encode(&map, 5);
        assert_eq!(decode(&data, 5).unwrap(), map);
        assert!(decode(&data, 6).is_none());
        for n in 0..data.len() {
            assert!(decode(&data[..n], 5).is_none());
        }
    }

    #[test]
    fn detail_follows_the_camera() {
        let map = Map {
            files: Vec::new(),
            nodes: vec![
                node((1, 0), None, 3, 10),
                node((2, 0), Some((1, 0)), 2, 20),
                node((2, 1), Some((1, 0)), 2, 21),
            ],
            children: HashMap::from([10, 20, 21, 30].map(|m| (m, (String::new(), 0)))),
        };
        let lod = Lod::new(Path::new(""), Arc::new(map), HashMap::new());
        let object = Object { model: 30, position: [0.0; 3], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 2] };
        let close = Close { objects: vec![(Some(1), object)] };
        let cut = |at: f32, loaded: &[u32]| {
            let cut = lod.cut(&[at, 0.0, 0.0], 1.0, &|m| loaded.contains(&m), &|_| false, std::iter::once(&close));
            let mut draw: Vec<u32> = cut.draw.into_keys().collect();
            draw.sort_unstable();
            (draw, cut.wanted.len())
        };
        // Afar, the far one; close, what it stands for once all of it is there.
        assert_eq!(cut(500.0, &[10, 20, 21, 30]), (vec![10], 0));
        assert_eq!(cut(50.0, &[10, 20]), (vec![10], 2));
        assert_eq!(cut(50.0, &[10, 20, 21]), (vec![20, 21], 1));
        assert_eq!(cut(50.0, &[10, 20, 21, 30]), (vec![21, 30], 0));
    }
}
