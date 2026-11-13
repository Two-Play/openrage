//! The game world for showing places: where every map placement file
//! (`.ymap`) lies, what every archetype (`.ytyp`) looks like (its texture
//! dictionary, and for interiors the objects inside), and the objects near a
//! point.
//!
//! Reading all of this takes a few seconds, so it is cached
//! (`<cache dir>/openrage/world-<game>.tsv`) and rebuilt when the search
//! index changes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use rage_crypto::GtaVKeys;
use rage_rpf::Rpf7;
use rage_rpf::vfs::{self, Node};
use rayon::prelude::*;

use rage_formats::math::{self, Mat4};

use crate::detect::collect_rpfs;

const CACHE_VERSION: &str = "openrage-world 3";

/// An object inside an interior, relative to the interior.
#[derive(Debug, Clone)]
pub struct InteriorObject {
    pub archetype: String,
    pub position: [f32; 3],
    pub rotation: [f32; 4],
}

#[derive(Debug, Clone, Default)]
pub struct Archetype {
    /// Texture dictionary name (without `.ytd`), if any.
    pub textures: Option<String>,
    /// For interiors (`CMloArchetypeDef`): the objects inside.
    pub interior: Vec<InteriorObject>,
}

#[derive(Debug, Clone, Default)]
pub struct World {
    /// Map placement files with their streaming extents.
    pub maps: Vec<(String, [f32; 3], [f32; 3])>,
    /// Archetypes by lowercase name.
    pub archetypes: HashMap<String, Archetype>,
}

/// A placed object: its archetype and where it stands in the world.
#[derive(Debug, Clone)]
pub struct Placed {
    pub archetype: String,
    pub transform: Mat4,
}

fn cache_path(game_dir: &Path) -> Option<PathBuf> {
    let index = crate::index_path(game_dir)?;
    let name = index.file_name()?.to_string_lossy().replace("index-", "world-").replace(".sqlite", ".tsv");
    Some(index.with_file_name(name))
}

fn vec3(n: roxmltree::Node) -> Option<[f32; 3]> {
    Some([n.attribute("x")?.parse().ok()?, n.attribute("y")?.parse().ok()?, n.attribute("z")?.parse().ok()?])
}

fn quat(n: roxmltree::Node) -> Option<[f32; 4]> {
    let [x, y, z] = vec3(n)?;
    Some([x, y, z, n.attribute("w")?.parse().ok()?])
}

fn child<'a, 'i>(n: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    n.children().find(|c| c.has_tag_name(name))
}

fn text(n: roxmltree::Node, name: &str) -> Option<String> {
    child(n, name).and_then(|c| c.text()).map(|t| t.trim().to_ascii_lowercase()).filter(|t| !t.is_empty())
}

fn parse_doc(xml: &str) -> Option<roxmltree::Document<'_>> {
    roxmltree::Document::parse_with_options(
        xml,
        roxmltree::ParsingOptions { allow_dtd: false, nodes_limit: u32::MAX, ..Default::default() },
    )
    .ok()
}

/// Archetypes of one `.ytyp`.
fn read_ytyp(xml: &str) -> Vec<(String, Archetype)> {
    let Some(doc) = parse_doc(xml) else { return Vec::new() };
    let Some(list) = child(doc.root_element(), "archetypes") else { return Vec::new() };
    let mut out = Vec::new();
    for item in list.children().filter(|c| c.has_tag_name("Item")) {
        let Some(name) = text(item, "name") else { continue };
        let mut a = Archetype { textures: text(item, "textureDictionary"), interior: Vec::new() };
        if item.attribute("type") == Some("CMloArchetypeDef") {
            // The interior's own objects, and those of its entity sets (rooms
            // and furnishings that can be switched on; often the walls too).
            let mut lists: Vec<roxmltree::Node> = child(item, "entities").into_iter().collect();
            if let Some(sets) = child(item, "entitySets") {
                for set in sets.children().filter(|c| c.has_tag_name("Item")) {
                    lists.extend(child(set, "entities"));
                }
            }
            for e in lists.iter().flat_map(|l| l.children()).filter(|c| c.has_tag_name("Item")) {
                let (Some(arch), Some(p), Some(r)) = (
                    text(e, "archetypeName"),
                    child(e, "position").and_then(vec3),
                    child(e, "rotation").and_then(quat),
                ) else {
                    continue;
                };
                a.interior.push(InteriorObject { archetype: arch, position: p, rotation: r });
            }
        }
        out.push((name, a));
    }
    out
}

/// A `.ymap`'s streaming extents.
fn ymap_extents(xml: &str) -> Option<([f32; 3], [f32; 3])> {
    let doc = parse_doc(xml)?;
    let root = doc.root_element();
    Some((child(root, "streamingExtentsMin").and_then(vec3)?, child(root, "streamingExtentsMax").and_then(vec3)?))
}

/// Reads every map placement and archetype file of the game.
pub fn build(game_dir: &Path, keys: Option<Arc<GtaVKeys>>) -> Result<World> {
    let mut rpfs = Vec::new();
    collect_rpfs(game_dir, &mut rpfs);
    let parts: Vec<World> = rpfs
        .par_iter()
        .filter_map(|p| {
            let rel = p.strip_prefix(game_dir).unwrap_or(p).to_string_lossy().replace('\\', "/");
            let rpf = Rpf7::open(p, keys.clone()).ok()?;
            let mut part = World::default();
            vfs::walk(
                &rpf,
                Rpf7::ROOT,
                &rel,
                true,
                &mut |a, id, path| {
                    let name = a.entry(id).name.to_ascii_lowercase();
                    let is_map = name.ends_with(".ymap");
                    if !is_map && !name.ends_with(".ytyp") {
                        return;
                    }
                    let Ok(xml) = a.read(id).map_err(anyhow::Error::from).and_then(|d| crate::metadata::from_bytes(&d))
                    else {
                        return;
                    };
                    if is_map {
                        if let Some((lo, hi)) = ymap_extents(&xml.text) {
                            part.maps.push((path.to_owned(), lo, hi));
                        }
                    } else {
                        part.archetypes.extend(read_ytyp(&xml.text));
                    }
                },
                &mut |_| {},
            );
            Some(part)
        })
        .collect();
    let mut world = World::default();
    for p in parts {
        world.maps.extend(p.maps);
        world.archetypes.extend(p.archetypes);
    }
    // The same map file in several places (base game, updates, DLC): the update or DLC wins.
    let mut by_name: HashMap<String, usize> = HashMap::new();
    let mut maps = Vec::new();
    for m in world.maps {
        let name = m.0.rsplit('/').next().unwrap_or(&m.0).to_owned();
        match by_name.get(&name) {
            Some(&i) if !m.0.starts_with("update") => {
                let _ = i;
            }
            Some(&i) => maps[i] = m,
            None => {
                by_name.insert(name, maps.len());
                maps.push(m);
            }
        }
    }
    world.maps = maps;
    Ok(world)
}

/// The first line of the cache: its format and the index state it was built from.
fn cache_header(index_changed: Option<i64>) -> String {
    format!("{CACHE_VERSION}\t{}", index_changed.unwrap_or(0))
}

fn save(world: &World, path: &Path, index_changed: Option<i64>) -> Result<()> {
    let mut s = cache_header(index_changed);
    s.push('\n');
    for (p, lo, hi) in &world.maps {
        s.push_str(&format!("Y\t{p}\t{} {} {}\t{} {} {}\n", lo[0], lo[1], lo[2], hi[0], hi[1], hi[2]));
    }
    for (name, a) in &world.archetypes {
        s.push_str(&format!("A\t{name}\t{}\n", a.textures.as_deref().unwrap_or("")));
        for o in &a.interior {
            let (p, r) = (o.position, o.rotation);
            s.push_str(&format!(
                "M\t{name}\t{}\t{} {} {}\t{} {} {} {}\n",
                o.archetype, p[0], p[1], p[2], r[0], r[1], r[2], r[3]
            ));
        }
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, s)?;
    Ok(())
}

fn floats<const N: usize>(s: &str) -> Option<[f32; N]> {
    let v: Vec<f32> = s.split(' ').filter_map(|x| x.parse().ok()).collect();
    v.try_into().ok()
}

fn load_cache(path: &Path, index_changed: Option<i64>) -> Option<World> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut lines = text.lines();
    if lines.next()? != cache_header(index_changed) {
        return None;
    }
    let mut w = World::default();
    for line in lines {
        let f: Vec<&str> = line.split('\t').collect();
        match f.as_slice() {
            ["Y", p, lo, hi] => w.maps.push(((*p).to_owned(), floats(lo)?, floats(hi)?)),
            ["A", name, txd] => {
                w.archetypes.entry((*name).to_owned()).or_default().textures =
                    (!txd.is_empty()).then(|| (*txd).to_owned());
            }
            ["M", name, arch, p, r] => {
                let o = InteriorObject { archetype: (*arch).to_owned(), position: floats(p)?, rotation: floats(r)? };
                w.archetypes.entry((*name).to_owned()).or_default().interior.push(o);
            }
            _ => {}
        }
    }
    Some(w)
}

/// The world, from the cache or read from the game (and cached). The cache is
/// rebuilt when the indexed archives changed.
pub fn load(game_dir: &Path, keys: Option<Arc<GtaVKeys>>) -> Result<World> {
    // Archetype names are hashes in the files; the index knows the names.
    if rage_formats::names::registered() == 0
        && let Some(index) = crate::index_path(game_dir).filter(|p| p.exists())
        && let Ok(names) = rage_index::Index::open(&index).and_then(|i| i.known_names())
    {
        rage_formats::names::register(names);
    }
    let path = cache_path(game_dir).context("no cache directory")?;
    // Rebuilt when the game's archives changed (as the index saw them), not
    // each time the index merely checked them: building reads every map file
    // of the game.
    let index_changed = crate::index_path(game_dir)
        .filter(|p| p.exists())
        .and_then(|p| rage_index::Index::open(&p).ok()?.changed_at().ok().flatten());
    if let Some(w) = load_cache(&path, index_changed) {
        return Ok(w);
    }
    let world = build(game_dir, keys)?;
    let _ = save(&world, &path, index_changed);
    Ok(world)
}

/// Entities placed within `radius` of `center` (high-detail ones; interiors
/// expanded into the objects inside them), in world coordinates.
pub fn objects_near(
    world: &World,
    game_dir: &Path,
    keys: Option<Arc<GtaVKeys>>,
    center: [f32; 3],
    radius: f32,
) -> Vec<Placed> {
    let within = |p: [f32; 3], r: f32| (0..3).map(|k| (p[k] - center[k]).powi(2)).sum::<f32>() <= r * r;
    let nearby: Vec<&String> = world
        .maps
        .iter()
        .filter(|(_, lo, hi)| (0..3).all(|k| center[k] + radius >= lo[k] && center[k] - radius <= hi[k]))
        .map(|(vpath, ..)| vpath)
        .collect();
    // Each map file is read and converted on its own; in parallel, in the
    // order of the world's list.
    nearby
        .par_iter()
        .map(|vpath| {
            let Some(xml) = read_map(game_dir, keys.clone(), vpath) else { return Vec::new() };
            map_entities(world, &xml)
                .into_iter()
                .filter(|e| {
                    // Room shells are large and placed by a corner: a wider reach.
                    let reach =
                        if e.in_interior && e.placed.archetype.contains("shell") { radius * 2.5 } else { radius };
                    let t = e.placed.transform;
                    e.high_detail && within([t[12], t[13], t[14]], reach)
                })
                .map(|e| e.placed)
                .collect()
        })
        .collect::<Vec<Vec<Placed>>>()
        .into_iter()
        .flatten()
        .collect()
}

/// An entity of a map file.
#[derive(Debug, Clone)]
pub struct MapEntity {
    pub placed: Placed,
    /// A high-detail entity (`LODTYPES_DEPTH_HD` / `ORPHANHD`), not a
    /// lower-detail stand-in for distant views.
    pub high_detail: bool,
    /// Placed inside an interior (relative to the interior instance).
    pub in_interior: bool,
}

/// Reads a map file (`.ymap`) as XML.
pub fn read_map(game_dir: &Path, keys: Option<Arc<GtaVKeys>>, vpath: &str) -> Option<String> {
    crate::metadata::load(game_dir, keys, vpath).ok().map(|x| x.text)
}

/// Every entity of a map file (as XML) in world coordinates; interiors are
/// expanded into the objects inside them.
pub fn map_entities(world: &World, xml: &str) -> Vec<MapEntity> {
    let mut out = Vec::new();
    let Some(doc) = parse_doc(xml) else { return out };
    let Some(entities) = child(doc.root_element(), "entities") else { return out };
    for e in entities.children().filter(|c| c.has_tag_name("Item")) {
        let lod = text(e, "lodLevel").unwrap_or_default();
        let high_detail = lod.ends_with("_hd") || lod.ends_with("orphanhd");
        let (Some(arch), Some(p), Some(r)) =
            (text(e, "archetypeName"), child(e, "position").and_then(vec3), child(e, "rotation").and_then(quat))
        else {
            continue;
        };
        let interior = world.archetypes.get(&arch).map(|a| a.interior.as_slice()).unwrap_or_default();
        if e.attribute("type") == Some("CMloInstanceDef") || !interior.is_empty() {
            // An interior: its objects, placed relative to the instance.
            let place = math::from_trs(p, conjugate(r), [1.0; 3]);
            for o in interior {
                let local = math::from_trs(o.position, conjugate(o.rotation), [1.0; 3]);
                out.push(MapEntity {
                    placed: Placed { archetype: o.archetype.clone(), transform: math::mul(&place, &local) },
                    high_detail,
                    in_interior: true,
                });
            }
            continue;
        }
        let scale_xy = child(e, "scaleXY").and_then(|n| n.attribute("value")?.parse().ok()).unwrap_or(1.0);
        let scale_z = child(e, "scaleZ").and_then(|n| n.attribute("value")?.parse().ok()).unwrap_or(1.0);
        out.push(MapEntity {
            placed: Placed {
                archetype: arch,
                transform: math::from_trs(p, conjugate(r), [scale_xy, scale_xy, scale_z]),
            },
            high_detail,
            in_interior: false,
        });
    }
    out
}

/// A map file put together: its models with their placements.
pub struct MapScene {
    pub models: Vec<(String, crate::models::LoadedModel, Vec<Mat4>)>,
    /// Objects placed in the file (interiors expanded).
    pub objects: usize,
    /// Archetypes whose model was not found, with how often they are placed.
    pub missing: Vec<(String, usize)>,
}

/// The entities of a map file that a scene of it shows: its high-detail
/// objects, or, in a file that only holds low-detail stand-ins (`…_lod`),
/// those.
pub fn scene_entities(world: &World, xml: &str) -> Vec<Placed> {
    let all = map_entities(world, xml);
    let detailed = all.iter().any(|e| e.high_detail);
    all.into_iter()
        .filter(|e| {
            if detailed {
                e.high_detail && shown_in_scenes(&e.placed.archetype)
            } else {
                !e.placed.archetype.starts_with("hash_")
            }
        })
        .map(|e| e.placed)
        .collect()
}

/// Loads every object of a map file with its textures (in parallel).
pub fn load_map(
    world: &World,
    files: &Files,
    game_dir: &Path,
    keys: Option<Arc<GtaVKeys>>,
    vpath: &str,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<MapScene> {
    let xml = read_map(game_dir, keys.clone(), vpath).with_context(|| format!("cannot read {vpath}"))?;
    let placed = scene_entities(world, &xml);
    let mut missing: HashMap<String, usize> = HashMap::new();
    for p in &placed {
        if !files.models.contains_key(&p.archetype) {
            *missing.entry(p.archetype.clone()).or_default() += 1;
        }
    }
    let objects = placed.len();
    let models = load_placed(world, files, game_dir, keys, &placed, progress);
    let mut missing: Vec<(String, usize)> = missing.into_iter().collect();
    missing.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    Ok(MapScene { models, objects, missing })
}

/// Whether a map object is shown in scenes: proxies, low-detail stand-ins and
/// unnamed archetypes are left out.
pub fn shown_in_scenes(archetype: &str) -> bool {
    !(archetype.contains("proxy") || archetype.contains("_lod") || archetype.starts_with("hash_"))
}

/// Model and texture files by name (from the search index): updates and DLC win.
#[derive(Debug, Clone, Default)]
pub struct Files {
    /// `name` (without extension) → path of its `.ydr`, `.yft` or `.ydd`.
    pub models: HashMap<String, String>,
    /// `name` (without extension) → path of its `.ytd`.
    pub textures: HashMap<String, String>,
}

pub fn files(game_dir: &Path) -> Result<Files> {
    let index = crate::index_path(game_dir).filter(|p| p.exists()).context("the search index is not built yet")?;
    let list = rage_index::Index::open(&index)?.files_with_extensions(&[".ydr", ".yft", ".ydd", ".ytd"])?;
    let mut out = Files::default();
    for (name, vpath) in list {
        let Some((stem, ext)) = name.rsplit_once('.') else { continue };
        let map = if ext == "ytd" { &mut out.textures } else { &mut out.models };
        // `_hi` models are the high-detail copies of vehicles; the plain name is used.
        let keep = map.get(stem).is_none_or(|old: &String| !old.starts_with("update") || vpath.starts_with("update"));
        if keep {
            map.insert(stem.to_owned(), vpath);
        }
    }
    Ok(out)
}

/// Loads every archetype of `placed` once, with its texture dictionary, and
/// returns the models with all placements of each. Proxies and low-detail
/// stand-ins are left out.
pub fn load_models(
    world: &World,
    files: &Files,
    game_dir: &Path,
    keys: Option<Arc<GtaVKeys>>,
    placed: &[Placed],
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Vec<(String, crate::models::LoadedModel, Vec<Mat4>)> {
    let shown: Vec<Placed> = placed.iter().filter(|p| shown_in_scenes(&p.archetype)).cloned().collect();
    load_placed(world, files, game_dir, keys, &shown, progress)
}

/// Loads every archetype of `placed` once, with its texture dictionary.
fn load_placed(
    world: &World,
    files: &Files,
    game_dir: &Path,
    keys: Option<Arc<GtaVKeys>>,
    placed: &[Placed],
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Vec<(String, crate::models::LoadedModel, Vec<Mat4>)> {
    let mut by_archetype: HashMap<&str, Vec<Mat4>> = HashMap::new();
    for p in placed {
        by_archetype.entry(p.archetype.as_str()).or_default().push(p.transform);
    }
    let list: Vec<(&str, Vec<Mat4>)> = by_archetype.into_iter().collect();
    let (total, done) = (list.len(), std::sync::atomic::AtomicUsize::new(0));
    progress(0, total);
    list.into_par_iter()
        .filter_map(|(name, transforms)| {
            let finished = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            progress(finished, total);
            let vpath = files.models.get(name)?;
            let mut model = crate::progress::quietly(|| crate::models::load(game_dir, keys.clone(), vpath)).ok()?;
            if let Some(txd) = world.archetypes.get(name).and_then(|a| a.textures.as_ref())
                && let Some(tpath) = files.textures.get(txd)
                && let Ok((Node::ArchiveFile(rpf, id), _)) = rage_mods::resolve(game_dir, tpath, keys.clone())
                && let Ok(dict) =
                    rpf.read_resource(id).map_err(anyhow::Error::from).and_then(|r| Ok(rage_formats::ytd::parse(&r)?))
            {
                model.add_textures(format!("{txd}.ytd"), dict);
            }
            Some((name.to_owned(), model, transforms))
        })
        .collect()
}

/// The box around a scene's placed models (their shown parts).
pub fn scene_bounds(
    items: &[(&crate::models::LoadedModel, &crate::models::Look, &[Mat4])],
) -> Option<([f32; 3], [f32; 3])> {
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for (model, look, placements) in items {
        let Some((a, b)) = crate::render::bounds(model, &look.parts) else { continue };
        for t in placements.iter() {
            for corner in 0..8 {
                let c = [
                    if corner & 1 == 0 { a[0] } else { b[0] },
                    if corner & 2 == 0 { a[1] } else { b[1] },
                    if corner & 4 == 0 { a[2] } else { b[2] },
                ];
                let p = math::transform_point(t, c);
                for k in 0..3 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
        }
    }
    (lo[0] <= hi[0]).then_some((lo, hi))
}

/// Map files store entity rotations inverted.
fn conjugate(q: [f32; 4]) -> [f32; 4] {
    [-q[0], -q[1], -q[2], q[3]]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_scenes_show_high_detail_objects() {
        let mut world = World::default();
        let int = Archetype {
            textures: None,
            interior: vec![InteriorObject {
                archetype: "wall_01".into(),
                position: [1.0, 0.0, 0.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
            }],
        };
        world.archetypes.insert("int_room".into(), int);
        let item = |kind: &str, name: &str, lod: &str, x: f32| {
            format!(
                r#"<Item type="{kind}"><archetypeName>{name}</archetypeName><lodLevel>{lod}</lodLevel><position x="{x}" y="0" z="0" /><rotation x="0" y="0" z="0" w="1" /></Item>"#
            )
        };
        let xml = format!(
            "<CMapData><entities>{}{}{}{}</entities></CMapData>",
            item("CEntityDef", "dt1_build", "LODTYPES_DEPTH_HD", 10.0),
            item("CEntityDef", "dt1_build_lod", "LODTYPES_DEPTH_LOD", 10.0),
            item("CEntityDef", "dt1_proxy", "LODTYPES_DEPTH_ORPHANHD", 0.0),
            item("CMloInstanceDef", "int_room", "LODTYPES_DEPTH_HD", 100.0),
        );
        let all = map_entities(&world, &xml);
        assert_eq!(all.len(), 4);
        assert!(
            all.iter().any(|e| e.in_interior && e.placed.archetype == "wall_01" && e.placed.transform[12] == 101.0)
        );
        let shown: Vec<String> = scene_entities(&world, &xml).into_iter().map(|p| p.archetype).collect();
        assert_eq!(shown, vec!["dt1_build", "wall_01"], "high detail only, no proxies");
        // A file of stand-ins only shows them.
        let lod_only = format!(
            "<CMapData><entities>{}</entities></CMapData>",
            item("CEntityDef", "dt1_build_lod", "LODTYPES_DEPTH_LOD", 0.0)
        );
        assert_eq!(scene_entities(&world, &lod_only).len(), 1);
    }

    #[test]
    fn reads_archetypes_and_interiors() {
        let xml = r#"<CMapTypes><archetypes>
  <Item type="CBaseArchetypeDef"><name>Prop_A</name><textureDictionary>tx_a</textureDictionary></Item>
  <Item type="CMloArchetypeDef"><name>int_room</name><textureDictionary />
   <entities><Item type="CEntityDef"><archetypeName>wall_01</archetypeName>
    <position x="1" y="2" z="3" /><rotation x="0" y="0" z="0" w="1" /></Item></entities></Item>
</archetypes></CMapTypes>"#;
        let a = read_ytyp(xml);
        assert_eq!(a[0].0, "prop_a");
        assert_eq!(a[0].1.textures.as_deref(), Some("tx_a"));
        assert_eq!(a[1].1.interior.len(), 1);
        assert_eq!(a[1].1.interior[0].archetype, "wall_01");
        let ext = ymap_extents(
            r#"<CMapData><streamingExtentsMin x="-1" y="-2" z="-3" /><streamingExtentsMax x="1" y="2" z="3" /></CMapData>"#,
        );
        assert_eq!(ext, Some(([-1.0, -2.0, -3.0], [1.0, 2.0, 3.0])));
    }
}
