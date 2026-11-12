//! Loading models (`.ydr`, `.yft`, `.ydd`) with the textures they use.

use std::path::Path;
use std::sync::Arc;

use std::collections::HashMap;
use std::sync::OnceLock;

use anyhow::{Context, Result, bail};
use rage_core::hash::joaat;
use rage_crypto::GtaVKeys;
use rage_formats::drawable::{self, Drawable};
use rage_formats::math::{self, Mat4};
use rage_formats::texture::Texture;
use rage_formats::ytd::TextureDictionary;
use rage_rpf::vfs::Node;

/// One placed piece of a model: a drawable at a position.
#[derive(Debug, Clone)]
pub struct Part {
    pub name: String,
    /// Logical group shown together (e.g. "Wheels", "Hair"); empty for none.
    pub group: String,
    /// Index into [`LoadedModel::drawables`].
    pub drawable: usize,
    /// Model-space placement.
    pub transform: Mat4,
    pub visible: bool,
    /// Skeleton bone the part is attached to (it follows the bone when animated).
    pub bone: Option<usize>,
}

pub struct LoadedModel {
    /// Drawables with display names (one for `.ydr`, the body and its children
    /// for `.yft`, several for `.ydd`).
    pub drawables: Vec<(String, Drawable)>,
    /// What is shown: drawables placed in the model (wheels appear four times).
    pub parts: Vec<Part>,
    /// Texture dictionaries found next to the model (`<name>.ytd`, `vehshare.ytd`).
    pub external: Vec<(String, TextureDictionary)>,
    /// Materials: shaders with the same name across all drawables.
    pub materials: Vec<MaterialSlot>,
    /// The skeleton skinned meshes are bound to (for peds: from the `.yft`
    /// next to the `.ydd`).
    pub skeleton: Option<drawable::Skeleton>,
    /// Material index per drawable and shader.
    shader_material: Vec<Vec<usize>>,
}

/// A material of a model: every shader with this name, in any part.
#[derive(Debug, Clone)]
pub struct MaterialSlot {
    pub name_hash: u32,
    /// Readable name ("Primary paint", "Glass", `normal_spec`, …).
    pub label: String,
    /// Takes its colour from the paint (vehicle_paint*).
    pub paint: bool,
    /// At least one of its shaders has a colour texture.
    pub textured: bool,
}

/// How a model is shown: which parts, and per material whether it is shown
/// and its colour (multiplied with the texture; the colour itself without one).
#[derive(Debug, Clone, PartialEq)]
pub struct Look {
    pub parts: Vec<bool>,
    pub materials: Vec<MaterialLook>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MaterialLook {
    pub visible: bool,
    pub tint: [f32; 4],
}

/// Colour of untextured surfaces (light grey).
pub const PLAIN: [f32; 4] = [0.72, 0.72, 0.74, 1.0];

impl LoadedModel {
    /// Finds a texture by name: embedded in the drawable first, then next to the model.
    pub fn texture<'a>(&'a self, drawable: &'a Drawable, name: &str) -> Option<&'a Texture> {
        let find = |d: &'a TextureDictionary| d.textures.iter().find(|t| t.name.eq_ignore_ascii_case(name));
        drawable.textures.as_ref().and_then(find).or_else(|| self.external.iter().find_map(|(_, d)| find(d)))
    }

    /// The default visibility of every part.
    pub fn default_visibility(&self) -> Vec<bool> {
        self.parts.iter().map(|p| p.visible).collect()
    }

    /// Default parts, every material shown in its own colours.
    pub fn default_look(&self) -> Look {
        Look {
            parts: self.default_visibility(),
            materials: self
                .materials
                .iter()
                .map(|m| MaterialLook {
                    visible: true,
                    tint: if m.textured {
                        [1.0; 4]
                    } else {
                        rage_formats::shaders::color(m.name_hash).unwrap_or(PLAIN)
                    },
                })
                .collect(),
        }
    }

    /// Material index of a drawable's shader.
    pub fn material_index(&self, drawable: usize, shader: u16) -> Option<usize> {
        self.shader_material.get(drawable)?.get(shader as usize).copied()
    }

    /// Colour of a drawable's shader under `look`, `None` if it is hidden.
    pub fn tint(&self, look: &Look, drawable: usize, shader: u16) -> Option<[f32; 4]> {
        match self.material_index(drawable, shader).and_then(|m| look.materials.get(m)) {
            Some(m) if !m.visible => None,
            Some(m) => Some(m.tint),
            None => Some([1.0; 4]),
        }
    }

    /// Adds a texture dictionary the model uses (found elsewhere, e.g. through
    /// its archetype) and sorts the materials again.
    pub fn add_textures(&mut self, name: String, dict: TextureDictionary) {
        self.external.push((name, dict));
        self.collect_materials();
    }

    /// Groups materials of all drawables by shader name.
    fn collect_materials(&mut self) {
        let mut materials: Vec<MaterialSlot> = Vec::new();
        let mut shader_material = Vec::new();
        for (_, d) in &self.drawables {
            let mut per_shader = Vec::new();
            for shader in &d.shaders {
                let textured = shader.base_texture().is_some_and(|t| self.texture(d, t).is_some());
                let index = match materials.iter().position(|m| m.name_hash == shader.name_hash) {
                    Some(i) => {
                        materials[i].textured |= textured;
                        i
                    }
                    None => {
                        materials.push(MaterialSlot {
                            name_hash: shader.name_hash,
                            label: rage_formats::shaders::label(shader.name_hash),
                            paint: rage_formats::shaders::is_paint(shader.name_hash),
                            textured,
                        });
                        materials.len() - 1
                    }
                };
                per_shader.push(index);
            }
            shader_material.push(per_shader);
        }
        self.materials = materials;
        self.shader_material = shader_material;
    }

    /// Parts shown by `visible` (one flag per part) with their drawables.
    pub fn shown<'a>(&'a self, visible: &'a [bool]) -> impl Iterator<Item = (&'a Part, &'a Drawable)> + 'a {
        self.parts
            .iter()
            .zip(visible)
            .filter(|(_, v)| **v)
            .filter_map(|(p, _)| Some((p, &self.drawables.get(p.drawable)?.1)))
    }

    pub fn triangle_count(&self, visible: &[bool]) -> usize {
        self.shown(visible).map(|(_, d)| d.triangle_count()).sum()
    }

    pub fn vertex_count(&self, visible: &[bool]) -> usize {
        self.shown(visible).map(|(_, d)| d.vertex_count()).sum()
    }
}

/// Ped component names (`head_000_r`, `uppr_001_u`, …) by hash: drawable
/// dictionaries of peds only store the hashes.
fn ped_component_names() -> &'static HashMap<u32, String> {
    static NAMES: OnceLock<HashMap<u32, String>> = OnceLock::new();
    NAMES.get_or_init(|| {
        let mut map = HashMap::new();
        for comp in ["head", "berd", "hair", "uppr", "lowr", "hand", "feet", "teef", "accs", "task", "decl", "jbib"] {
            for n in 0..100 {
                for suffix in ["u", "r", "m"] {
                    let name = format!("{comp}_{n:03}_{suffix}");
                    map.insert(joaat(&name), name);
                }
            }
        }
        map
    })
}

fn is_ped_dictionary(names: &[String]) -> bool {
    names.iter().filter(|n| ped_component_names().values().any(|v| v == *n)).count() >= 2
}

/// How many drawables of a (non-ped) dictionary are shown at first.
const SHOWN_AT_FIRST: usize = 64;

/// Which drawables of a dictionary to show at first. Peds: the first variant of
/// every component (head, upper body, legs, …) — a whole person; other
/// dictionaries: all of them (laid out side by side), up to a limit.
fn default_visibility(names: &[String]) -> Vec<bool> {
    let component = |n: &str| n.split('_').next().map(str::to_owned);
    if !is_ped_dictionary(names) {
        return (0..names.len()).map(|i| i < SHOWN_AT_FIRST).collect();
    }
    let mut best: HashMap<String, &str> = HashMap::new();
    for n in names {
        if let Some(c) = component(n) {
            let entry = best.entry(c).or_insert(n.as_str());
            if n.as_str() < *entry {
                *entry = n.as_str();
            }
        }
    }
    names.iter().map(|n| best.values().any(|b| b == n)).collect()
}

/// Places a fragment's children on their bones. Wheels exist once per axle
/// side in the file and are copied to every wheel bone (turned around on the
/// right side).
/// Component groups of a streamed ped worth showing by default (accessories,
/// tasks and decals are left out).
const STREAMED_GROUPS: &[&str] = &["head", "berd", "hair", "uppr", "lowr", "hand", "feet", "teef", "jbib"];

type StreamedPed = (Vec<(String, Drawable)>, Vec<Part>, Vec<(String, TextureDictionary)>);

/// The first variant of every component in a streamed ped's folder
/// (`head_000_r.ydd`, `uppr_000_u.ydd`, …) and its first texture variant
/// (`head_diff_000_a_whi.ytd`).
fn streamed_ped(rpf: &rage_rpf::Rpf7, folder: rage_rpf::EntryId) -> Result<StreamedPed> {
    let files: Vec<(String, rage_rpf::EntryId)> =
        rpf.children(folder).map(|c| (rpf.entry(c).name.to_ascii_lowercase(), c)).collect();
    let (mut drawables, mut parts, mut textures) = (Vec::new(), Vec::new(), Vec::new());
    for group in STREAMED_GROUPS {
        let mut candidates: Vec<&(String, rage_rpf::EntryId)> =
            files.iter().filter(|(n, _)| n.starts_with(&format!("{group}_")) && n.ends_with(".ydd")).collect();
        candidates.sort();
        let Some((name, id)) = candidates.first() else { continue };
        let Ok(res) = rpf.read_resource(*id) else { continue };
        let Some((_, d)) = drawable::parse_ydd(&res).ok().and_then(|v| v.into_iter().next()) else { continue };
        let stem = name.trim_end_matches(".ydd").to_owned();
        // `head_000_r` → textures `head_diff_000_a_*.ytd`.
        let variant = stem.split('_').nth(1).unwrap_or("000");
        let prefix = format!("{group}_diff_{variant}_a");
        let mut ytds: Vec<&(String, rage_rpf::EntryId)> =
            files.iter().filter(|(n, _)| n.starts_with(&prefix) && n.ends_with(".ytd")).collect();
        ytds.sort();
        if let Some((tname, tid)) = ytds.first()
            && let Ok(dict) =
                rpf.read_resource(*tid).map_err(anyhow::Error::from).and_then(|r| Ok(rage_formats::ytd::parse(&r)?))
        {
            textures.push((tname.clone(), dict));
        }
        parts.push(Part {
            name: stem.clone(),
            group: ped_group(&stem),
            drawable: drawables.len(),
            transform: math::IDENTITY,
            visible: true,
            bone: None,
        });
        drawables.push((stem, d));
    }
    if drawables.is_empty() {
        anyhow::bail!("no components in the ped's folder");
    }
    Ok((drawables, parts, textures))
}

fn fragment_parts(fragment: drawable::Fragment, name: &str) -> (Vec<(String, Drawable)>, Vec<Part>) {
    let drawable::Fragment { drawable: body, children } = fragment;
    let skeleton = body.skeleton.clone().unwrap_or_default();
    let world = skeleton.world_transforms();
    let mut drawables = vec![(name.to_owned(), body)];
    let mut parts = vec![Part {
        name: name.to_owned(),
        group: "Body".into(),
        drawable: 0,
        transform: math::IDENTITY,
        visible: true,
        bone: None,
    }];
    let mut wheels: Vec<(String, usize)> = Vec::new();
    for child in children {
        let bone = skeleton.find_tag(child.bone_tag);
        let bone_name =
            bone.map(|b| skeleton.bones[b].name.clone()).unwrap_or_else(|| format!("part_{:04x}", child.bone_tag));
        let index = drawables.len();
        drawables.push((bone_name.clone(), child.drawable));
        let transform = bone.and_then(|b| world.get(b)).copied().unwrap_or(math::IDENTITY);
        if bone_name.starts_with("wheel_") {
            wheels.push((bone_name.clone(), index));
        }
        let group = if bone_name.starts_with("wheel_") { "Wheels" } else { "Parts" };
        parts.push(Part { name: bone_name, group: group.into(), drawable: index, transform, visible: true, bone });
    }
    // Copy wheels to the wheel bones that have no drawable of their own.
    if !wheels.is_empty() {
        let is_right = |n: &str| n.strip_prefix("wheel_").is_some_and(|s| s.starts_with('r'));
        let is_rear = |n: &str| n.ends_with('r') || n.contains("_lm") || n.contains("_rm");
        for (b, bone) in skeleton.bones.iter().enumerate() {
            let n = bone.name.to_ascii_lowercase();
            let is_wheel_bone = n.starts_with("wheel_") && !n.contains("mesh");
            if !is_wheel_bone || wheels.iter().any(|(w, _)| w.eq_ignore_ascii_case(&n)) {
                continue;
            }
            // Same side and axle if available, else any wheel (front preferred).
            let source = wheels
                .iter()
                .find(|(w, _)| is_rear(w) == is_rear(&n) && is_right(w) == is_right(&n))
                .or_else(|| wheels.iter().find(|(w, _)| is_rear(w) == is_rear(&n)))
                .or_else(|| wheels.first())
                .map(|(w, i)| (w.clone(), *i));
            let Some((src_name, src)) = source else { continue };
            let mut transform = world[b];
            if is_right(&n) != is_right(&src_name) {
                transform = math::mul(&transform, &math::rotation_z(std::f32::consts::PI));
            }
            parts.push(Part {
                name: bone.name.clone(),
                group: "Wheels".into(),
                drawable: src,
                transform,
                visible: true,
                bone: Some(b),
            });
        }
    }
    (drawables, parts)
}

pub fn is_model(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.ends_with(".ydr")
        || n.ends_with(".yft")
        || n.ends_with(".ydd")
        || n.ends_with(".ypt")
        || n.ends_with(".ybn")
        || rage_formats::paths::is_path_file(&n)
}

/// Colour of a collision material, by what it is made of.
fn material_color(name: &str, index: u8) -> [f32; 4] {
    let n = name.to_ascii_uppercase();
    let rgb = |r: u8, g: u8, b: u8| [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0];
    let has = |words: &[&str]| words.iter().any(|w| n.contains(w));
    if has(&["GRASS", "LEAVES", "BUSH", "TREE", "HEDGE", "MOSS", "FOLIAGE"]) {
        rgb(110, 170, 80)
    } else if has(&["WATER", "PUDDLE"]) {
        rgb(80, 150, 220)
    } else if has(&["WOOD", "BARK", "TWIGS", "PLYWOOD"]) {
        rgb(170, 120, 75)
    } else if has(&["METAL", "STEEL", "ALUMIN", "IRON", "CHAIN", "RAIL", "CAR_"]) {
        rgb(120, 140, 170)
    } else if has(&["GLASS", "PERSPEX", "PLEXI"]) {
        rgb(150, 210, 230)
    } else if has(&["SAND", "DIRT", "MUD", "SOIL", "GRAVEL", "CLAY", "STONE", "ROCK"]) {
        rgb(190, 160, 110)
    } else if has(&["CONCRETE", "TARMAC", "ASPHALT", "BRICK", "TILE", "PAVING", "CEMENT", "MARBLE", "PLASTER"]) {
        rgb(175, 175, 180)
    } else if has(&["CARPET", "CLOTH", "FABRIC", "LEATHER", "RUBBER", "FOAM", "PLASTIC"]) {
        rgb(200, 130, 160)
    } else {
        // Anything else: a stable, soft colour per material.
        let h = (index as f32 * 0.618_034) % 1.0;
        let (r, g, b) = hue(h);
        [0.45 + 0.4 * r, 0.45 + 0.4 * g, 0.45 + 0.4 * b, 1.0]
    }
}

fn hue(h: f32) -> (f32, f32, f32) {
    let x = |o: f32| ((h * 6.0 + o) % 6.0 - 3.0).abs() - 1.0;
    (x(0.0).clamp(0.0, 1.0), x(4.0).clamp(0.0, 1.0), x(2.0).clamp(0.0, 1.0))
}

/// Names of the collision materials, read once from the game's `materials.dat`.
fn collision_material_names(game_dir: &Path, keys: Option<Arc<GtaVKeys>>) -> &'static [String] {
    static NAMES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    NAMES.get_or_init(|| {
        for path in ["update/update.rpf/common/data/materials/materials.dat", "common.rpf/data/materials/materials.dat"]
        {
            if let Ok((Node::ArchiveFile(rpf, id), _)) = rage_mods::resolve(game_dir, path, keys.clone())
                && let Ok(data) = rpf.read(id)
            {
                let names = rage_formats::ybn::material_names(&String::from_utf8_lossy(&data));
                if !names.is_empty() {
                    return names;
                }
            }
        }
        Vec::new()
    })
}

/// Loads a model as the game sees it (mods included). Reports its steps
/// to [`crate::progress`].
pub fn load(game_dir: &Path, keys: Option<Arc<GtaVKeys>>, vpath: &str) -> Result<LoadedModel> {
    crate::progress::set("Reading the model", 0, 0);
    let result = load_inner(game_dir, keys, vpath);
    crate::progress::clear();
    result
}

fn load_inner(game_dir: &Path, keys: Option<Arc<GtaVKeys>>, vpath: &str) -> Result<LoadedModel> {
    let (node, _) = rage_mods::resolve(game_dir, vpath, keys.clone())?;
    if let Node::Rpf8File(rpf, i) = node {
        return load_rdr2(&rpf, i);
    }
    let Node::ArchiveFile(rpf, id) = node else { bail!("{vpath} is not a file inside an archive") };
    let name = rpf.entry(id).name.clone();
    let res = rpf.read_resource(id)?;
    let lower = name.to_ascii_lowercase();
    let stem = name.split('.').next().unwrap_or(&name).to_owned();
    let mut external = Vec::new();
    let (drawables, parts) = if rage_formats::paths::is_path_file(&lower) {
        let ext = lower.rsplit('.').next().unwrap_or_default();
        let (path, _) = rage_formats::paths::parse(&res, ext)?;
        let part = Part {
            name: stem.clone(),
            group: String::new(),
            drawable: 0,
            transform: math::IDENTITY,
            visible: true,
            bone: None,
        };
        (vec![(stem.clone(), path)], vec![part])
    } else if lower.ends_with(".ybn") {
        let (collision, stats) = rage_formats::ybn::parse(&res)?;
        let names = collision_material_names(game_dir, keys.clone());
        for &m in &stats.materials {
            let label = names.get(m as usize).cloned().unwrap_or_else(|| format!("Material {m}"));
            let color = material_color(&label, m);
            rage_formats::shaders::register(rage_formats::ybn::MATERIAL_BASE + m as u32, label, color);
        }
        let part = Part {
            name: stem.clone(),
            group: String::new(),
            drawable: 0,
            transform: math::IDENTITY,
            visible: true,
            bone: None,
        };
        (vec![(stem.clone(), collision)], vec![part])
    } else if lower.ends_with(".ypt") {
        // Particle models (debris, sparks, …) side by side, textured from the
        // dictionary inside the file.
        let ptfx = rage_formats::ypt::parse(&res)?;
        if ptfx.drawables.is_empty() {
            bail!("{name} contains no models");
        }
        let drawables: Vec<(String, Drawable)> = ptfx
            .drawables
            .into_iter()
            .enumerate()
            .map(|(i, (hash, d))| {
                let label = rage_formats::names::value(hash);
                let label = if hash == 0 || label.starts_with("hash_") { format!("{stem}#{i}") } else { label };
                (label, d)
            })
            .collect();
        let layout = thumbnail_layout(&drawables);
        let parts = drawables
            .iter()
            .enumerate()
            .map(|(i, (n, _))| Part {
                name: n.clone(),
                group: String::new(),
                drawable: i,
                transform: layout[i],
                visible: true,
                bone: None,
            })
            .collect();
        if let Some(t) = ptfx.textures {
            external.push((format!("{stem}.ypt textures"), t));
        }
        (drawables, parts)
    } else if lower.ends_with(".ydd") {
        let drawables: Vec<(String, Drawable)> = drawable::parse_ydd(&res)?
            .into_iter()
            .enumerate()
            .map(|(i, (hash, d))| {
                let name = match ped_component_names().get(&hash) {
                    Some(n) => n.clone(),
                    None if hash != 0 => format!("{stem}#{hash:08x}"),
                    None => format!("{stem}#{i}"),
                };
                (name, d)
            })
            .collect();
        let names: Vec<String> = drawables.iter().map(|(n, _)| n.clone()).collect();
        // Peds are assembled in place; other dictionaries are laid out in a grid.
        let layout =
            if is_ped_dictionary(&names) { vec![math::IDENTITY; drawables.len()] } else { grid_layout(&drawables) };
        let parts = default_visibility(&names)
            .into_iter()
            .enumerate()
            .map(|(i, visible)| Part {
                name: names[i].clone(),
                group: ped_group(&names[i]),
                drawable: i,
                transform: layout[i],
                visible,
                bone: None,
            })
            .collect();
        (drawables, parts)
    } else if lower.ends_with(".yft")
        && let Some(folder) = rpf
            .find_child(rpf.entry(id).parent.unwrap_or(rage_rpf::Rpf7::ROOT), &stem)
            .filter(|&f| rpf.entry(f).is_dir())
    {
        // A streamed ped (online characters, story characters): the fragment
        // holds the skeleton, the folder next to it the components.
        let skeleton = drawable::parse_fragment(&res)?.drawable.skeleton;
        let (drawables, parts, textures) = streamed_ped(&rpf, folder)?;
        external.extend(textures);
        let mut model =
            LoadedModel { drawables, parts, external, materials: Vec::new(), shader_material: Vec::new(), skeleton };
        model.collect_materials();
        return Ok(model);
    } else if lower.ends_with(".yft") {
        fragment_parts(drawable::parse_fragment(&res)?, &stem)
    } else if lower.ends_with(".ydr") {
        let part = Part {
            name: stem.clone(),
            group: String::new(),
            drawable: 0,
            transform: math::IDENTITY,
            visible: true,
            bone: None,
        };
        (vec![(stem.clone(), drawable::parse_ydr(&res)?)], vec![part])
    } else {
        bail!("{name} is not a model (.ydr, .yft, .ydd, .ypt, .ybn, .ynd, .yvr, .ywr)");
    };

    // Textures usually live next to the model: adder.yft → adder.ytd (+ vehshare.ytd for vehicles).
    let dir = rpf.entry(id).parent.unwrap_or(rage_rpf::Rpf7::ROOT);
    let mut candidates = vec![format!("{stem}.ytd"), format!("{stem}+hi.ytd")];
    if lower.ends_with(".yft") {
        candidates.push("vehshare.ytd".into());
    }
    let total = candidates.len() as u64;
    for (k, c) in candidates.into_iter().enumerate() {
        crate::progress::set("Loading textures", k as u64, total);
        if let Some(tid) = rpf.find_child(dir, &c)
            && let Ok(dict) = rpf
                .read_resource(tid)
                .map_err(anyhow::Error::from)
                .and_then(|r| rage_formats::ytd::parse(&r).context("texture dictionary"))
        {
            external.push((c, dict));
        }
    }
    // Peds keep their skeleton in a fragment next to the drawable dictionary.
    let skeleton = drawables.iter().find_map(|(_, d)| d.skeleton.clone()).or_else(|| {
        let tid = rpf.find_child(dir, &format!("{stem}.yft"))?;
        drawable::parse_fragment(&rpf.read_resource(tid).ok()?).ok()?.drawable.skeleton
    });
    let mut model =
        LoadedModel { drawables, parts, external, materials: Vec::new(), shader_material: Vec::new(), skeleton };
    model.collect_materials();
    Ok(model)
}

/// Like [`grid_layout`], but every drawable is scaled to the same size: for
/// particle models, where a spark sits next to a large hoop and real sizes
/// would leave most of them invisible.
fn thumbnail_layout(drawables: &[(String, Drawable)]) -> Vec<Mat4> {
    let columns = (drawables.len().min(SHOWN_AT_FIRST) as f32).sqrt().ceil().max(1.0) as usize;
    drawables
        .iter()
        .enumerate()
        .map(|(i, (_, d))| {
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for p in d.meshes.iter().flat_map(|m| &m.positions) {
                for k in 0..3 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
            if lo[0] > hi[0] {
                (lo, hi) = ([0.0; 3], [0.0; 3]);
            }
            let extent = (0..3).map(|k| hi[k] - lo[k]).fold(0.0f32, f32::max).max(1e-3);
            let s = 1.0 / extent;
            let centre = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, lo[2]];
            let (col, row) = ((i % SHOWN_AT_FIRST) % columns, (i % SHOWN_AT_FIRST) / columns);
            let mut m = math::IDENTITY;
            m[0] = s;
            m[5] = s;
            m[10] = s;
            m[12] = col as f32 * 1.3 - centre[0] * s;
            m[13] = -(row as f32) * 1.3 - centre[1] * s;
            m[14] = -centre[2] * s;
            m
        })
        .collect()
}

/// Places drawables side by side in rows (centred on their bounds), so a
/// dictionary of unrelated objects shows all of them.
fn grid_layout(drawables: &[(String, Drawable)]) -> Vec<Mat4> {
    let bounds: Vec<([f32; 3], [f32; 3])> = drawables
        .iter()
        .map(|(_, d)| {
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for p in d.meshes.iter().flat_map(|m| &m.positions) {
                for i in 0..3 {
                    lo[i] = lo[i].min(p[i]);
                    hi[i] = hi[i].max(p[i]);
                }
            }
            if lo[0] > hi[0] { ([0.0; 3], [0.0; 3]) } else { (lo, hi) }
        })
        .collect();
    let cell = bounds.iter().map(|(lo, hi)| (hi[0] - lo[0]).max(hi[1] - lo[1])).fold(0.0f32, f32::max) * 1.2 + 0.1;
    let shown = drawables.len().min(SHOWN_AT_FIRST);
    let columns = (shown as f32).sqrt().ceil().max(1.0) as usize;
    bounds
        .iter()
        .enumerate()
        .map(|(i, (lo, hi))| {
            let (col, row) = ((i % SHOWN_AT_FIRST) % columns, (i % SHOWN_AT_FIRST) / columns);
            let centre = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, lo[2]];
            let mut m = math::IDENTITY;
            m[12] = col as f32 * cell - centre[0];
            m[13] = -(row as f32) * cell - centre[1];
            m[14] = -centre[2];
            m
        })
        .collect()
}

/// Group of a ped component (`uppr_001_u` → "Upper body"); other drawables none.
fn ped_group(name: &str) -> String {
    if !ped_component_names().values().any(|v| v == name) {
        return String::new();
    }
    let label = match name.split('_').next().unwrap_or_default() {
        "head" => "Head",
        "berd" => "Beard / mask",
        "hair" => "Hair",
        "uppr" => "Upper body",
        "lowr" => "Legs",
        "hand" => "Hands",
        "feet" => "Feet",
        "teef" => "Teeth",
        "accs" => "Accessories",
        "task" => "Gear",
        "decl" => "Decals",
        "jbib" => "Jacket",
        other => other,
    };
    label.to_owned()
}

/// Exports the model as shown by `look` as `.glb` (one node per part,
/// material colours as base colour factors, hidden materials left out).
pub fn to_glb(model: &LoadedModel, name: &str, look: &Look) -> Option<Vec<u8>> {
    let meshes: Vec<(&str, &Drawable)> = model.drawables.iter().map(|(n, d)| (n.as_str(), d)).collect();
    let nodes: Vec<rage_formats::gltf::Node> = model
        .shown(&look.parts)
        .map(|(p, _)| rage_formats::gltf::Node { name: p.name.clone(), mesh: p.drawable, transform: p.transform })
        .collect();
    if nodes.is_empty() {
        return None;
    }
    Some(rage_formats::gltf::write_glb_scene(
        name,
        &meshes,
        &nodes,
        &|d, t| model.texture(&model.drawables[d].1, t),
        &|d, s| model.tint(look, d, s as u16),
    ))
}

/// Loads a Red Dead Redemption 2 model (`.ydr`, `.ydd`). Map objects take
/// their textures from shared dictionaries in the same archive folder, so
/// those are loaded too (the largest few by name match first).
fn load_rdr2(rpf: &Arc<rage_rpf::Rpf8>, i: usize) -> Result<LoadedModel> {
    let entries = rpf.entries().context("archive table not readable")?;
    let entry = &entries[i];
    let full = rpf.entry_name(i);
    let name = full.rsplit('/').next().unwrap_or(&full).to_owned();
    let lower = name.to_ascii_lowercase();
    let stem = name.split('.').next().unwrap_or(&name).to_owned();
    crate::progress::set("Reading the model", 0, 0);
    let res = rpf.read_resource(entry)?;
    let drawables: Vec<(String, Drawable)> = if lower.ends_with(".ydr") {
        vec![(stem.clone(), drawable::parse_ydr_rdr2(&res)?)]
    } else if lower.ends_with(".ydd") {
        drawable::parse_ydd_rdr2(&res)?
            .into_iter()
            .enumerate()
            .map(|(k, (hash, d))| {
                let label = rage_formats::names::value(hash);
                let label = if hash == 0 || label.starts_with("hash_") { format!("{stem}#{k}") } else { label };
                (label, d)
            })
            .collect()
    } else if lower.ends_with(".yft") {
        vec![(stem.clone(), drawable::parse_fragment_rdr2(&res)?.drawable)]
    } else {
        bail!("{name}: only .ydr, .ydd and .yft models of Red Dead Redemption 2 can be shown so far");
    };
    let layout = if drawables.len() > 1 { grid_layout(&drawables) } else { vec![math::IDENTITY] };
    let parts = drawables
        .iter()
        .enumerate()
        .map(|(k, (n, _))| Part {
            name: n.clone(),
            group: String::new(),
            drawable: k,
            transform: layout[k],
            visible: true,
            bone: None,
        })
        .collect();
    // Texture dictionaries in the same folder: `<stem>.ytd` first, then those
    // whose name shares the model's prefix, then the rest; at most eight.
    let dir = full.rsplit_once('/').map_or("", |(d, _)| d).to_owned();
    let (_, files) = rpf.list_dir(&dir);
    let prefix: String = stem.chars().take_while(|c| *c != '_').collect();
    let mut ytds: Vec<(i32, usize, String)> = files
        .into_iter()
        .filter_map(|k| {
            let n = rpf.entry_name(k);
            let base = n.rsplit('/').next().unwrap_or(&n).to_owned();
            let lb = base.to_ascii_lowercase();
            if !lb.ends_with(".ytd") {
                return None;
            }
            let rank = if lb == format!("{}.ytd", stem.to_ascii_lowercase()) {
                0
            } else if !prefix.is_empty() && lb.starts_with(&prefix.to_ascii_lowercase()) {
                1
            } else {
                2
            };
            Some((rank, k, base))
        })
        .collect();
    ytds.sort();
    let mut external = Vec::new();
    let folder: Vec<(i32, usize, String)> = ytds.into_iter().take(8).collect();
    let folder_count = folder.len() as u64;
    for (n, (_, k, base)) in folder.into_iter().enumerate() {
        crate::progress::set("Loading textures", n as u64, folder_count);
        if let Ok(dict) = rpf
            .read_resource(&entries[k])
            .map_err(anyhow::Error::from)
            .and_then(|r| rage_formats::ytd::parse_rdr2(&r).context("texture dictionary"))
        {
            external.push((base, dict));
        }
    }
    // The rest from shared dictionaries elsewhere, through the texture index.
    if let Some(game) = crate::rdr2::game_folder(rpf.path())
        && let Some(index) = crate::rdr2::texture_index(&game)
    {
        let have = |n: &str, ext: &[(String, TextureDictionary)]| {
            drawables.iter().any(|(_, d)| {
                d.textures.as_ref().is_some_and(|t| t.textures.iter().any(|x| x.name.eq_ignore_ascii_case(n)))
            }) || ext.iter().any(|(_, t)| t.textures.iter().any(|x| x.name.eq_ignore_ascii_case(n)))
        };
        let wanted: Vec<String> =
            drawables.iter().flat_map(|(_, d)| d.shaders.iter().flat_map(|s| s.textures.iter().cloned())).collect();
        // Which shared dictionaries are needed (one per texture not found yet).
        let mut needed: Vec<(String, crate::rdr2::Location)> = Vec::new();
        for name in wanted {
            if needed.len() >= 32 || have(&name, &external) {
                continue;
            }
            let Some(loc) = index.find(&name) else { continue };
            if needed.iter().any(|(_, l)| l == loc) {
                continue;
            }
            needed.push((name, loc.clone()));
        }
        let total = needed.len() as u64;
        for (k, (name, loc)) in needed.into_iter().enumerate() {
            crate::progress::set("Loading shared textures", k as u64, total);
            if let Some(dict) = crate::rdr2::open_dictionary(&game, &loc) {
                external.push((format!("{name} (shared)"), dict));
            }
        }
    }
    let skeleton = drawables.iter().find_map(|(_, d)| d.skeleton.clone());
    let mut model =
        LoadedModel { drawables, parts, external, materials: Vec::new(), shader_material: Vec::new(), skeleton };
    model.collect_materials();
    Ok(model)
}
