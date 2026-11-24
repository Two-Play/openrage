//! The map of GTA VI: where its map files (`.#map`) are, what they place, and
//! the models for it.
//!
//! A map file covers a small piece of the world, and there are more than
//! 160,000, so reading all of them takes a minute or two. What is needed of
//! them is kept (`<cache dir>/openrage/gta6-world-<game>.bin`) until the search
//! index changes.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use rage_core::bytes::LeRead;
use rage_core::hash::joaat;
use rage_formats::drawable::Drawable;
use rage_formats::math::{self, Mat4};
use rage_rpf::Rpf9;
use rage_rpf::vfs::Node;
use rayon::prelude::*;

use crate::models::LoadedModel;
use crate::world::MapScene;

const MAGIC: &[u8; 8] = b"ORG6MAP1";

/// Levels of detail: the models seen close, and the ones that stand for whole
/// neighbourhoods from afar.
const CLOSE: [u8; 2] = [0, 5];
const FAR: u8 = 3;
/// The sides of textures in views of many models.
const SIDE_NEAR: u32 = 256;
const SIDE_FAR: u32 = 128;

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

/// What is kept of the map files.
#[derive(Debug, Default, PartialEq)]
pub struct Map {
    /// Map files with the box around what they place.
    files: Vec<(String, [f32; 3], [f32; 3])>,
    /// The objects that stand for the world from afar.
    far: Vec<Object>,
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
        let Ok((Node::Rpf9Dir(rpf, _), _)) = rage_mods::resolve(game, archive, None) else { return };
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

/// Objects with their level of detail, and the box around them.
type Placements = (Vec<(u8, Object)>, Option<[[f32; 3]; 2]>);

/// What a map file places, with the level of detail, and the box around it.
/// The XML is read by lines: there is a lot of it, and it is our own.
fn read_map(xml: &str) -> Placements {
    let mut objects = Vec::new();
    let (mut lo, mut hi) = (None, None);
    let mut now = Object { model: 0, position: [0.0; 3], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 2] };
    let mut inside = false;
    let value = |line: &str| attributes(line, ["value"]).map(|[v]| v);
    for line in xml.lines().map(str::trim) {
        if line.starts_with("<entities>") || line.starts_with("<entities ") {
            inside = !line.ends_with("/>");
        } else if line == "</entities>" {
            inside = false;
        } else if line.starts_with("<entitiesExtentsMin") {
            lo = attributes(line, ["x", "y", "z"]);
        } else if line.starts_with("<entitiesExtentsMax") {
            hi = attributes(line, ["x", "y", "z"]);
        } else if inside {
            if let Some(name) = between(line, "<archetypeName>", "<") {
                now.model = name_hash(name);
            } else if line.starts_with("<position ") {
                now.position = attributes(line, ["x", "y", "z"]).unwrap_or_default();
            } else if line.starts_with("<rotation ") {
                now.rotation = attributes(line, ["x", "y", "z", "w"]).unwrap_or([0.0, 0.0, 0.0, 1.0]);
            } else if line.starts_with("<scaleXY ") {
                now.scale[0] = value(line).unwrap_or(1.0);
            } else if line.starts_with("<scaleZ ") {
                now.scale[1] = value(line).unwrap_or(1.0);
            } else if line.starts_with("<lodLevel ") {
                let level = value(line).map_or(u8::MAX, |v| v as u8);
                objects.push((level, now.clone()));
            }
        }
    }
    (objects, lo.zip(hi).map(|(lo, hi)| [lo, hi]))
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
            let (objects, extent) = read_map(&xml.text);
            let mut map = lock();
            if let Some([lo, hi]) = extent
                && objects.iter().any(|(level, _)| CLOSE.contains(level))
            {
                map.files.push((vpath, lo, hi));
            }
            map.far.extend(objects.into_iter().filter(|(level, _)| *level == FAR).map(|(_, o)| o));
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
    out.extend((map.far.len() as u32).to_le_bytes());
    for o in &map.far {
        out.extend(o.model.to_le_bytes());
        floats(&mut out, &o.position);
        floats(&mut out, &o.rotation);
        floats(&mut out, &o.scale);
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
        let model = reader.number()?;
        let mut next = || reader.number();
        map.far.push(Object {
            model,
            position: floats(&mut next)?,
            rotation: floats(&mut next)?,
            scale: floats(&mut next)?,
        });
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

/// Puts objects together: every model once, with all its places.
fn scene(
    game: &Path,
    map: &Map,
    objects: &[Object],
    side: u32,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<MapScene> {
    let files = model_files(game)?;
    let mut places: HashMap<u32, Vec<Mat4>> = HashMap::new();
    for o in objects {
        places.entry(o.model).or_default().push(o.transform());
    }
    let mut missing = Vec::new();
    let mut wanted = Vec::new();
    for (model, places) in places {
        let source = match (files.get(&model), map.children.get(&model)) {
            (Some(file), _) => Source::File(file.clone()),
            (None, Some((file, k))) => Source::Child(file.clone(), *k),
            (None, None) => {
                missing.push((rage_formats::names::value(model), places.len()));
                continue;
            }
        };
        wanted.push((source, places));
    }
    missing.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let (total, count) = (wanted.len(), &AtomicUsize::new(0));
    progress(0, total);
    // Archives are opened once: that takes longer than reading a model.
    let mut archives: BTreeMap<String, Vec<(String, Source, Vec<Mat4>)>> = BTreeMap::new();
    for (source, places) in wanted {
        let (Source::File(vpath) | Source::Child(vpath, _)) = &source;
        if let Some((archive, file)) = vpath.rsplit_once(".rpf/") {
            archives.entry(format!("{archive}.rpf")).or_default().push((file.to_owned(), source, places));
        }
    }
    let models = archives
        .into_par_iter()
        .flat_map(|(archive, files)| {
            let opened = match rage_mods::resolve(game, &archive, None) {
                Ok((Node::Rpf9Dir(rpf, _), _)) => rpf.entries().ok().map(|entries| (rpf, entries)),
                _ => None,
            };
            files.into_par_iter().filter_map(move |(file, source, places)| {
                let model = opened.as_ref().and_then(|(rpf, entries)| {
                    let i = rpf.find(entries, &file)?;
                    crate::progress::quietly(|| load_model(game, rpf, i, &source, side)).ok()
                });
                progress(count.fetch_add(1, Ordering::Relaxed) + 1, total);
                model.map(|(name, model)| (name, model, places))
            })
        })
        .collect();
    Ok(MapScene { models, objects: objects.len(), missing })
}

fn load_model(game: &Path, rpf: &Rpf9, i: usize, source: &Source, side: u32) -> Result<(String, LoadedModel)> {
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
    let textures = crate::gta6_textures::for_drawables_up_to(game, &drawables, side);
    let mut model = LoadedModel::from_drawables(drawables);
    if let Some(textures) = textures {
        model.add_textures("texture pools".to_owned(), textures);
    }
    Ok((name.trim_end_matches(".#dr").trim_end_matches(".#ft").to_owned(), model))
}

/// The whole map, as it looks from afar.
pub fn overview(game: &Path, progress: &Progress) -> Result<MapScene> {
    let map = load(game, &|done, total| progress("Reading the game's map files", done, total))?;
    anyhow::ensure!(!map.far.is_empty(), "the map files of this game have no distant view");
    scene(game, &map, &map.far, SIDE_FAR, &|done, total| progress("Loading the map's models", done, total))
}

/// Gets what is being done, how much of it is done, and how much there is.
pub type Progress<'a> = dyn Fn(&str, usize, usize) + Sync + 'a;

/// How far around a map file's objects its surroundings are shown (metres).
pub const AROUND: f32 = 120.0;

/// A map file, with what the others place around it.
pub fn place(game: &Path, vpath: &str, around: f32, progress: &Progress) -> Result<MapScene> {
    let (own, extent) = read_map(&crate::metadata::load(game, None, vpath)?.text);
    let map = load(game, &|done, total| progress("Reading the game's map files", done, total))?;
    let close = |objects: Vec<(u8, Object)>| -> Vec<Object> {
        objects.into_iter().filter(|(level, _)| CLOSE.contains(level)).map(|(_, o)| o).collect()
    };
    let has_close = own.iter().any(|(level, _)| CLOSE.contains(level));
    let mut objects = if has_close { close(own) } else { own.into_iter().map(|(_, o)| o).collect() };
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
                let near = close(read_map(&xml.text).0).into_iter().filter(|o| near(&o.position));
                found.lock().unwrap_or_else(|e| e.into_inner()).extend(near);
            },
            &|| {},
        );
        objects.extend(found.into_inner().unwrap_or_else(|e| e.into_inner()));
    }
    scene(game, &map, &objects, SIDE_NEAR, &|done, total| progress("Loading the map's models", done, total))
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<CMapData>
 <entitiesExtentsMin x="1" y="2" z="3" />
 <entitiesExtentsMax x="4" y="5" z="6" />
 <entities>
  <Item type="CEntityDef">
   <rotation x="0" y="0" z="0" w="1" />
   <position x="10" y="20" z="30" />
   <archetypeName>Prop_A</archetypeName>
   <scaleXY value="2" />
   <scaleZ value="3" />
   <lodLevel value="5" />
  </Item>
  <Item type="CEntityDef">
   <position x="0" y="0" z="0" />
   <archetypeName>hash_0000ABCD</archetypeName>
   <scaleXY value="1" />
   <scaleZ value="1" />
   <lodLevel value="3" />
  </Item>
 </entities>
</CMapData>"#;

    #[test]
    fn map_files_are_read() {
        let (objects, extent) = read_map(XML);
        assert_eq!(extent, Some([[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]]));
        assert_eq!(objects.len(), 2);
        assert_eq!((objects[0].0, objects[0].1.model), (5, joaat("prop_a")));
        assert_eq!(objects[0].1.scale, [2.0, 3.0]);
        assert_eq!(objects[0].1.transform()[12..15], [10.0, 20.0, 30.0]);
        assert_eq!((objects[1].0, objects[1].1.model), (3, 0xABCD));
    }

    #[test]
    fn the_cache_reads_back() {
        let map = Map {
            files: vec![("a.rpf/b.#map".into(), [1.0; 3], [2.0; 3])],
            far: read_map(XML).0.into_iter().map(|(_, o)| o).collect(),
            children: HashMap::from([(7, ("c.rpf/d.#dd".to_owned(), 3))]),
        };
        let data = encode(&map, 5);
        assert_eq!(decode(&data, 5).unwrap(), map);
        assert!(decode(&data, 6).is_none());
        for n in 0..data.len() {
            assert!(decode(&data[..n], 5).is_none());
        }
    }
}
