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
    /// The ped's expressions for its parts (face and helper bones), in the
    /// order they run.
    pub expressions: Vec<rage_formats::expression::Expression>,
    /// An RDR2 ped put together from one of its outfits.
    pub ped: Option<PedOutfit>,
    /// Material index per drawable and shader.
    shader_material: Vec<Vec<usize>>,
}

/// Which outfit of an RDR2 ped a model shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PedOutfit {
    /// The ped's name (its outfit file without `.ymt`).
    pub name: String,
    pub outfit: usize,
    pub outfits: usize,
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
        // `suse` (special) is a GTA IV component.
        for comp in
            ["head", "berd", "hair", "uppr", "lowr", "hand", "feet", "teef", "accs", "task", "decl", "jbib", "suse"]
        {
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
        || n.ends_with(".wdr")
        || n.ends_with(".wdd")
        || n.ends_with(".wft")
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
    let mut model = load_model(game_dir, keys.clone(), vpath)?;
    if model.skeleton.is_some() && model.drawables.len() > 1 {
        let name = vpath.rsplit('/').next().unwrap_or(vpath);
        let stem = name.split('.').next().unwrap_or(name).to_ascii_lowercase();
        model.expressions = ped_expressions(game_dir, keys, &stem, &model.drawables);
    }
    Ok(model)
}

/// Where the game keeps the peds' expressions.
const EXPRESSIONS: &str = "x64c.rpf/anim/expressions.rpf";

/// The expressions of a ped: its dictionary (named after the ped; the story
/// characters use `p_m_zero` …), the face setup first, then one per part.
fn ped_expressions(
    game_dir: &Path,
    keys: Option<Arc<GtaVKeys>>,
    ped: &str,
    drawables: &[(String, Drawable)],
) -> Vec<rage_formats::expression::Expression> {
    let dictionary = match ped {
        "player_zero" => "p_m_zero".to_owned(),
        "player_one" => "p_m_one".to_owned(),
        "player_two" => "p_m_two".to_owned(),
        other => other.to_owned(),
    };
    let Some(dict) = [dictionary.as_str(), "default"].iter().find_map(|d| {
        let (Node::ArchiveFile(rpf, id), _) =
            rage_mods::resolve(game_dir, &format!("{EXPRESSIONS}/{d}.yed"), keys.clone()).ok()?
        else {
            return None;
        };
        rage_formats::expression::parse(&rpf.read_resource(id).ok()?).ok()
    }) else {
        return Vec::new();
    };
    let wanted = |e: &rage_formats::expression::Expression| {
        let n = e.name.to_ascii_lowercase();
        let n = n.trim_start_matches("pack:/").trim_end_matches(".expr");
        drawables.iter().any(|(d, _)| d.eq_ignore_ascii_case(n))
    };
    let mut out: Vec<_> =
        dict.expressions.iter().filter(|e| e.name.to_ascii_lowercase().ends_with("faceinit.expr")).cloned().collect();
    out.extend(dict.expressions.iter().filter(|e| wanted(e)).cloned());
    out
}

fn load_model(game_dir: &Path, keys: Option<Arc<GtaVKeys>>, vpath: &str) -> Result<LoadedModel> {
    let (node, _) = rage_mods::resolve(game_dir, vpath, keys.clone())?;
    if let Node::Rpf8File(rpf, i) = node {
        return load_rdr2(&rpf, i);
    }
    if let Node::Gta4File(archive, id) = node {
        return load_gta4(&archive, id);
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
        let (mut drawables, parts, textures) = streamed_ped(&rpf, folder)?;
        external.extend(textures);
        if let Some(sk) = &skeleton {
            fit_to_skeleton(&mut drawables, sk);
        }
        let mut model = LoadedModel {
            drawables,
            parts,
            external,
            materials: Vec::new(),
            shader_material: Vec::new(),
            skeleton,
            expressions: Vec::new(),
            ped: None,
        };
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
    if lower.ends_with(".yft") {
        // Vehicles from DLC packs have no vehshare.ytd beside them: use the game's own.
        let mut shared = Vec::new();
        if !external.iter().any(|(n, _)| n == "vehshare.ytd") {
            shared.push("vehshare.ytd".to_owned());
        }
        // Interiors come from shared dictionaries named after the texture
        // (supergt_interior → vehicles_supergt_interior.ytd).
        for d in drawables.iter().map(|(_, d)| d) {
            for t in d.shaders.iter().filter_map(|s| s.base_texture()) {
                let t = t.to_ascii_lowercase();
                let dict = format!("vehicles_{t}.ytd");
                if t.ends_with("_interior") && !shared.contains(&dict) {
                    shared.push(dict);
                }
            }
        }
        for name in shared {
            let local = rpf
                .find_child(dir, &name)
                .and_then(|tid| rpf.read_resource(tid).ok())
                .and_then(|r| rage_formats::ytd::parse(&r).ok());
            if let Some(dict) = local.or_else(|| base_vehicle_textures(game_dir, keys.clone(), &name)) {
                external.push((name, dict));
            }
        }
    }
    // Peds keep their skeleton in a fragment next to the drawable dictionary.
    let skeleton = drawables.iter().find_map(|(_, d)| d.skeleton.clone()).or_else(|| {
        let tid = rpf.find_child(dir, &format!("{stem}.yft"))?;
        drawable::parse_fragment(&rpf.read_resource(tid).ok()?).ok()?.drawable.skeleton
    });
    let mut drawables = drawables;
    if let Some(sk) = &skeleton {
        fit_to_skeleton(&mut drawables, sk);
    }
    let mut model = LoadedModel {
        drawables,
        parts,
        external,
        materials: Vec::new(),
        shader_material: Vec::new(),
        skeleton,
        expressions: Vec::new(),
        ped: None,
    };
    model.collect_materials();
    Ok(model)
}

/// Makes the meshes of every drawable use the model's skeleton: blend indices
/// of a drawable with its own skeleton point into that one (whose bone order
/// may differ, as in the story characters' components), so they are mapped
/// by bone tag; cloth meshes are bound to the body.
fn fit_to_skeleton(drawables: &mut [(String, Drawable)], skeleton: &drawable::Skeleton) {
    for (_, d) in drawables.iter_mut() {
        if let Some(own) = &d.skeleton {
            // A bone the main skeleton lacks (RDR2's facial bones) follows its
            // nearest ancestor that it has, else the head.
            let head = skeleton.find_name("SKEL_Head");
            let map: Vec<u16> = (0..own.bones.len())
                .map(|i| {
                    let mut b = Some(i);
                    let mut guard = 0;
                    while let Some(k) = b.filter(|_| guard < 512) {
                        if let Some(m) = skeleton.find_tag(own.bones[k].tag) {
                            return m as u16;
                        }
                        b = usize::try_from(own.bones[k].parent).ok().filter(|&p| p < own.bones.len());
                        guard += 1;
                    }
                    head.unwrap_or(i) as u16
                })
                .collect();
            if map.iter().enumerate().any(|(i, &m)| i as u16 != m) {
                // Meshes that name their bones by tag are bound below.
                for mesh in d.meshes.iter_mut().filter(|m| m.bone_tags.is_empty()) {
                    for b in &mut mesh.bone_indices {
                        *b = b.map(|i| map.get(i as usize).copied().unwrap_or(i));
                    }
                }
            }
        }
        bind_by_tags(d, skeleton);
        bind_cloth(d, skeleton);
        bind_pinned(d, skeleton);
    }
}

/// RDR2 ped parts name their bones by tag ([`drawable::Mesh::bone_tags`]):
/// each index becomes the skeleton bone with that tag. A tag the skeleton
/// lacks (hair bones of the part's own skeleton) follows the part's own
/// ancestors to one it has, else the head.
fn bind_by_tags(d: &mut Drawable, skeleton: &drawable::Skeleton) {
    let head = skeleton.find_name("SKEL_Head").unwrap_or(0) as u16;
    let own = d.skeleton.clone();
    let resolve = |tag: u16| -> u16 {
        if let Some(b) = skeleton.find_tag(tag) {
            return b as u16;
        }
        let Some(own) = &own else { return head };
        let mut b = own.find_tag(tag);
        let mut guard = 0;
        while let Some(k) = b.filter(|_| guard < 512) {
            if let Some(m) = skeleton.find_tag(own.bones[k].tag) {
                return m as u16;
            }
            b = usize::try_from(own.bones[k].parent).ok().filter(|&p| p < own.bones.len());
            guard += 1;
        }
        head
    };
    for mesh in &mut d.meshes {
        if mesh.bone_tags.is_empty() {
            continue;
        }
        let map: Vec<u16> = mesh.bone_tags.iter().map(|&t| resolve(t)).collect();
        for b in &mut mesh.bone_indices {
            *b = b.map(|i| map.get(i as usize).copied().unwrap_or(head));
        }
        mesh.bone_tags.clear();
    }
}

/// Shaders of ped meshes the game moves by cloth simulation (skirts, coats).
/// Their blend weights are not skinning data, so skinning them as such tears
/// them into spikes.
fn is_cloth_shader(hash: u32) -> bool {
    matches!(rage_formats::shaders::name(hash), Some("ped_cloth" | "ped_cloth_enveff"))
}

/// Binds the cloth meshes of a ped to the body bones nearest to each vertex
/// (the two closest bone segments, by inverse squared distance), so they
/// follow the animation like clothes on the body instead of being torn apart.
fn bind_cloth(d: &mut Drawable, skeleton: &drawable::Skeleton) {
    let cloth: Vec<bool> = d.shaders.iter().map(|s| is_cloth_shader(s.name_hash)).collect();
    if !cloth.contains(&true) {
        return;
    }
    let segments = body_segments(skeleton);
    if segments.is_empty() {
        return;
    }
    for mesh in &mut d.meshes {
        if !cloth.get(mesh.shader as usize).copied().unwrap_or(false) {
            continue;
        }
        let (indices, weights) = mesh.positions.iter().map(|p| nearest_bones(&segments, *p)).unzip();
        mesh.bone_indices = indices;
        mesh.bone_weights = weights;
    }
}

/// Binds vertices held by the skeleton's root to the nearest body bones.
/// Cloth parts pin some vertices to the root for the cloth simulation; the
/// root stays at the feet, so without it those vertices are left behind as
/// long strips when the ped moves.
fn bind_pinned(d: &mut Drawable, skeleton: &drawable::Skeleton) {
    let roots: Vec<usize> = skeleton.bones.iter().enumerate().filter(|(_, b)| b.parent < 0).map(|(i, _)| i).collect();
    let mut segments = None;
    for mesh in &mut d.meshes {
        for i in 0..mesh.bone_indices.len().min(mesh.bone_weights.len()).min(mesh.positions.len()) {
            let (b, w) = (mesh.bone_indices[i], mesh.bone_weights[i]);
            let pinned: f32 = (0..4).filter(|&k| roots.contains(&(b[k] as usize))).map(|k| w[k]).sum();
            let total: f32 = w.iter().sum();
            if total > 0.0 && pinned / total > 0.5 {
                let segments = segments.get_or_insert_with(|| body_segments(skeleton));
                if segments.is_empty() {
                    return;
                }
                (mesh.bone_indices[i], mesh.bone_weights[i]) = nearest_bones(segments, mesh.positions[i]);
            }
        }
    }
}

/// The body's main bones as segments from the bone to its first main child
/// (fingers, toes, face and helper bones left out).
fn body_segments(skeleton: &drawable::Skeleton) -> Vec<(u16, [f32; 3], [f32; 3])> {
    let world = skeleton.world_transforms();
    let main = |name: &str| {
        let n = name.to_ascii_lowercase();
        n.starts_with("skel_") && n != "skel_root" && !n.contains("finger") && !n.contains("toe")
    };
    let pos = |i: usize| [world[i][12], world[i][13], world[i][14]];
    skeleton
        .bones
        .iter()
        .enumerate()
        .filter(|(_, b)| main(&b.name))
        .map(|(i, _)| {
            let child = skeleton.bones.iter().position(|c| usize::try_from(c.parent).ok() == Some(i) && main(&c.name));
            (i as u16, pos(i), child.map_or(pos(i), pos))
        })
        .collect()
}

fn nearest_bones(segments: &[(u16, [f32; 3], [f32; 3])], p: [f32; 3]) -> ([u16; 4], [f32; 4]) {
    let distance = |(_, a, b): &(u16, [f32; 3], [f32; 3])| {
        let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let ap = [p[0] - a[0], p[1] - a[1], p[2] - a[2]];
        let len2 = ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2];
        let t =
            if len2 > 1e-9 { ((ap[0] * ab[0] + ap[1] * ab[1] + ap[2] * ab[2]) / len2).clamp(0.0, 1.0) } else { 0.0 };
        let d = [ap[0] - t * ab[0], ap[1] - t * ab[1], ap[2] - t * ab[2]];
        d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
    };
    let (mut best, mut second) = ((0u16, f32::MAX), (0u16, f32::MAX));
    for s in segments {
        let d = distance(s);
        if d < best.1 {
            second = best;
            best = (s.0, d);
        } else if d < second.1 {
            second = (s.0, d);
        }
    }
    let w = |d: f32| 1.0 / (d + 1e-4);
    let (w1, w2) = (w(best.1), if second.1 < f32::MAX { w(second.1) } else { 0.0 });
    let total = w1 + w2;
    ([best.0, second.0, 0, 0], [w1 / total, w2 / total, 0.0, 0.0])
}

/// A texture dictionary from the base game's vehicle archive (shared by many vehicles).
fn base_vehicle_textures(game_dir: &Path, keys: Option<Arc<GtaVKeys>>, name: &str) -> Option<TextureDictionary> {
    let path = format!("x64e.rpf/levels/gta5/vehicles.rpf/{name}");
    let (Node::ArchiveFile(rpf, id), _) = rage_mods::resolve(game_dir, &path, keys).ok()? else { return None };
    rage_formats::ytd::parse(&rpf.read_resource(id).ok()?).ok()
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

/// Writes several models, each placed one or more times, as one `.glb`
/// scene (a map file, a cutscene's surroundings). Positions are moved so the
/// scene's centre is at the origin; `center` says where that was.
pub fn scene_to_glb(name: &str, items: &[(&LoadedModel, &Look, &[Mat4])], center: [f32; 3]) -> Option<Vec<u8>> {
    let mut meshes: Vec<(&str, &Drawable)> = Vec::new();
    // For every scene drawable: its model and its index there.
    let mut owner: Vec<(usize, usize)> = Vec::new();
    let mut nodes = Vec::new();
    let recentre = math::from_trs([-center[0], -center[1], -center[2]], [0.0, 0.0, 0.0, 1.0], [1.0; 3]);
    for (m, (model, look, placements)) in items.iter().enumerate() {
        let first = meshes.len();
        for (k, (n, d)) in model.drawables.iter().enumerate() {
            meshes.push((n.as_str(), d));
            owner.push((m, k));
        }
        for t in placements.iter() {
            let placed = math::mul(&recentre, t);
            for (part, _) in model.shown(&look.parts) {
                nodes.push(rage_formats::gltf::Node {
                    name: part.name.clone(),
                    mesh: first + part.drawable,
                    transform: math::mul(&placed, &part.transform),
                });
            }
        }
    }
    if nodes.is_empty() {
        return None;
    }
    Some(rage_formats::gltf::write_glb_scene(
        name,
        &meshes,
        &nodes,
        &|d, t| {
            let (m, k) = owner[d];
            items[m].0.texture(&items[m].0.drawables[k].1, t)
        },
        &|d, sh| {
            let (m, k) = owner[d];
            items[m].0.tint(items[m].1, k, sh as u16)
        },
    ))
}

/// Loads a Red Dead Redemption 2 model (`.ydr`, `.ydd`). Map objects take
/// their textures from shared dictionaries in the same archive folder, so
/// those are loaded too (the largest few by name match first).
/// A GTA IV model (`.wdr`, `.wdd`, `.wft`), textured from its own
/// dictionary, the one named after it and the ones the `.ide` files give it.
fn load_gta4(archive: &Arc<rage_rpf::Gta4Archive>, id: rage_rpf::Gta4EntryId) -> Result<LoadedModel> {
    use rage_formats::wdr;
    let name = archive.entry(id).name.clone();
    let lower = name.to_ascii_lowercase();
    let stem = name.split('.').next().unwrap_or(&name).to_owned();
    let res = archive.read_resource(id)?;
    let (mut drawables, parts) = if lower.ends_with(".wdr") {
        let part = Part {
            name: stem.clone(),
            group: String::new(),
            drawable: 0,
            transform: math::IDENTITY,
            visible: true,
            bone: None,
        };
        (vec![(stem.clone(), wdr::parse_wdr(&res)?)], vec![part])
    } else if lower.ends_with(".wft") {
        fragment_parts(wdr::parse_wft(&res)?, &stem)
    } else if lower.ends_with(".wdd") {
        let drawables: Vec<(String, Drawable)> = wdr::parse_wdd(&res)?
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
    } else {
        bail!("{name} is not a GTA IV model (.wdr, .wdd, .wft)");
    };
    crate::progress::set("Loading textures", 0, 0);
    let used: Vec<String> =
        drawables.iter().flat_map(|(_, d)| d.shaders.iter().flat_map(|s| s.textures.iter().cloned())).collect();
    let vehicle = lower.ends_with(".wft") && archive.display_name().eq_ignore_ascii_case("vehicles.img");
    let external = crate::gta4::model_textures(archive, &stem, &used, vehicle);
    // Peds: the skeleton of the fragment next to the dictionary.
    let skeleton = drawables.iter().find_map(|(_, d)| d.skeleton.clone()).or_else(|| {
        let tid = archive.find_child(rage_rpf::Gta4Archive::ROOT, &format!("{stem}.wft"))?;
        wdr::parse_wft(&archive.read_resource(tid).ok()?).ok()?.drawable.skeleton
    });
    if let Some(sk) = &skeleton
        && lower.ends_with(".wdd")
    {
        fit_to_skeleton(&mut drawables, sk);
    }
    let mut model = LoadedModel {
        drawables,
        parts,
        external,
        materials: Vec::new(),
        shader_material: Vec::new(),
        skeleton,
        expressions: Vec::new(),
        ped: None,
    };
    model.collect_materials();
    Ok(model)
}

fn load_rdr2(rpf: &Arc<rage_rpf::Rpf8>, i: usize) -> Result<LoadedModel> {
    let entries = rpf.entries().context("archive table not readable")?;
    let entry = &entries[i];
    let full = rpf.entry_name(i);
    let name = full.rsplit('/').next().unwrap_or(&full).to_owned();
    let lower = name.to_ascii_lowercase();
    let stem = name.split('.').next().unwrap_or(&name).to_owned();
    if is_rdr2_ped_file(&full, rpf.display_name()) {
        let game = crate::rdr2::game_folder(rpf.path()).context("not inside an RDR2 game folder")?;
        return load_rdr2_ped(&game, &stem, 0);
    }
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
    // A ped's parts (".ydd" of a metaped) are bound to its base skeleton,
    // kept in a fragment of its own; they then stand on one spot.
    let mut drawables = drawables;
    let base = (lower.ends_with(".ydd")).then(|| rdr2_base_skeleton(rpf, &stem)).flatten();
    if let Some(sk) = &base {
        fit_to_skeleton(&mut drawables, sk);
    }
    let layout = if drawables.len() > 1 && base.is_none() {
        grid_layout(&drawables)
    } else {
        vec![math::IDENTITY; drawables.len()]
    };
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
    if let Some(game) = crate::rdr2::game_folder(rpf.path()) {
        rdr2_shared_textures(&game, &drawables, &mut external, 32);
    }
    let skeleton = base.or_else(|| drawables.iter().find_map(|(_, d)| d.skeleton.clone()));
    let mut model = LoadedModel {
        drawables,
        parts,
        external,
        materials: Vec::new(),
        shader_material: Vec::new(),
        skeleton,
        expressions: Vec::new(),
        ped: None,
    };
    model.collect_materials();
    Ok(model)
}

/// Adds the shared texture dictionaries the drawables' shaders need (at most
/// `limit`), found through the texture index.
fn rdr2_shared_textures(
    game: &Path,
    drawables: &[(String, Drawable)],
    external: &mut Vec<(String, TextureDictionary)>,
    limit: usize,
) {
    let Some(index) = crate::rdr2::texture_index(game) else { return };
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
        if needed.len() >= limit || have(&name, external) {
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
        if let Some(dict) = crate::rdr2::open_dictionary(game, &loc) {
            external.push((format!("{name} (shared)"), dict));
        }
    }
}

/// Whether an RDR2 file is a ped's outfit file: a `.ymt` in one of the
/// `metapeds*.rpf` archives (not the ped list `metapeds.ymt` itself).
pub fn is_rdr2_ped_file(name: &str, archive: &str) -> bool {
    let name = name.to_ascii_lowercase();
    let base = name.rsplit('/').next().unwrap_or(&name);
    let archive = archive.to_ascii_lowercase().replace('\\', "/");
    let archive = archive.rsplit('/').next().unwrap_or(&archive);
    base.ends_with(".ymt") && base != "metapeds.ymt" && archive.starts_with("metapeds") && archive.ends_with(".rpf")
}

/// Puts an RDR2 ped together from one of its outfits: every part's model on
/// the ped's base skeleton, with the outfit's colour and normal textures.
/// Reports its steps to [`crate::progress`].
pub fn load_rdr2_ped(game: &Path, ped: &str, outfit: usize) -> Result<LoadedModel> {
    let result = load_rdr2_ped_inner(game, ped, outfit);
    crate::progress::clear();
    result
}

fn load_rdr2_ped_inner(game: &Path, ped: &str, outfit: usize) -> Result<LoadedModel> {
    use crate::metaped;
    crate::progress::set("Indexing the peds' models (once)", 0, 0);
    let index = metaped::index(game, |done, total| {
        crate::progress::set("Indexing the peds' models (once)", done as u64, total as u64)
    })
    .context("no RDR2 keys")?;
    let textures = crate::rdr2::texture_index(game);
    crate::progress::set("Reading the outfits", 0, 0);
    let outfits = metaped::load_outfits(game, &index, ped)?;
    if outfits.is_empty() {
        bail!("{ped} has no outfits");
    }
    let chosen = outfit.min(outfits.len() - 1);
    let assets = &outfits[chosen].assets;
    // Models: each from its drawable dictionary (merged dictionaries hold several).
    let mut dictionaries: HashMap<crate::rdr2::Location, Vec<(u32, Drawable)>> = HashMap::new();
    let mut drawables: Vec<(String, Drawable)> = Vec::new();
    // The outfit asset each drawable comes from.
    let mut owners: Vec<&metaped::Asset> = Vec::new();
    for (k, a) in assets.iter().enumerate() {
        crate::progress::set("Reading the parts", k as u64, assets.len() as u64);
        let Some(at) = index.drawable(a.drawable) else { continue };
        if !dictionaries.contains_key(&at.file) {
            let read = metaped::open_location(game, &at.file)
                .and_then(|(rpf, i)| rpf.read_resource(rpf.entries()?.get(i)?).ok())
                .and_then(|res| drawable::parse_ydd_rdr2(&res).ok())
                .unwrap_or_default();
            dictionaries.insert(at.file.clone(), read);
        }
        let Some((_, d)) = dictionaries.get(&at.file).and_then(|list| list.get(at.index as usize)) else { continue };
        let mut d = d.clone();
        let name_of = |hash: u32| textures.as_ref().and_then(|t| t.name_of(hash)).map(str::to_owned);
        if let Some(albedo) = name_of(a.albedo) {
            replace_texture(&mut d, "_ab", &albedo);
        }
        if let Some(normal) = name_of(a.normal) {
            replace_texture(&mut d, "_nm", &normal);
        }
        let label = rage_formats::names::value(a.drawable);
        let label = if label.starts_with("hash_") {
            let file = at.file.chain.last().map(|h| rage_formats::names::value(*h)).unwrap_or_default();
            format!("{file}#{}", at.index)
        } else {
            label
        };
        drawables.push((label, d));
        owners.push(a);
    }
    if drawables.is_empty() {
        bail!("none of the parts of {ped}'s outfit {chosen} were found");
    }
    // The base skeleton: as the ped list names it, else guessed from the name.
    crate::progress::set("Reading the skeleton", 0, 0);
    let fragment = metaped::peds(game)
        .ok()
        .and_then(|peds| peds.iter().find(|p| p.name.eq_ignore_ascii_case(ped)).map(metaped::base_fragment));
    let base = fragment
        .as_deref()
        .and_then(|frag| rdr2_skeleton_named(game, frag))
        .or_else(|| rdr2_base_skeleton_in(game, ped));
    // Human heads name face bones the base skeletons lack: add them from
    // the one face rig the game has.
    let base = base.map(|mut sk| {
        if let Some(rig) = rdr2_face_rig(game) {
            let wanted = |t: u16| drawables.iter().any(|(_, d)| d.meshes.iter().any(|m| m.bone_tags.contains(&t)));
            add_face_rig(&mut sk, &rig, wanted);
        }
        sk
    });
    if let Some(sk) = &base {
        fit_to_skeleton(&mut drawables, sk);
        let shapes = &outfits[chosen].expressions;
        if let Some(frag) = &fragment
            && !shapes.is_empty()
        {
            crate::progress::set("Shaping the face and body", 0, 0);
            shape_rdr2_ped(game, frag, sk, &mut drawables, shapes);
        }
    }
    let parts = drawables
        .iter()
        .enumerate()
        .map(|(k, (n, _))| Part {
            name: n.clone(),
            group: String::new(),
            drawable: k,
            transform: math::IDENTITY,
            visible: true,
            bone: None,
        })
        .collect();
    let mut external = Vec::new();
    rdr2_shared_textures(game, &drawables, &mut external, 96);
    crate::progress::set("Colouring with tint palettes", 0, 0);
    tint_parts(game, &mut drawables, &owners, &mut external, textures.as_deref());
    let skeleton = base.or_else(|| drawables.iter().find_map(|(_, d)| d.skeleton.clone()));
    let mut model = LoadedModel {
        drawables,
        parts,
        external,
        materials: Vec::new(),
        shader_material: Vec::new(),
        skeleton,
        expressions: Vec::new(),
        ped: Some(PedOutfit { name: ped.to_owned(), outfit: chosen, outfits: outfits.len() }),
    };
    model.collect_materials();
    Ok(model)
}

/// Colours the tinted colour textures of ped parts with their palette rows
/// ([`crate::tint`]): each becomes a new texture (named after the texture
/// and its tints) that the part's shaders use instead.
fn tint_parts(
    game: &Path,
    drawables: &mut [(String, Drawable)],
    owners: &[&crate::metaped::Asset],
    external: &mut Vec<(String, TextureDictionary)>,
    index: Option<&crate::rdr2::TextureIndex>,
) {
    let tinted = crate::metaped::tinted_albedos(game);
    let Some(palettes) = crate::metaped::palette_dictionary(game) else { return };
    let mut baked: Vec<Texture> = Vec::new();
    for ((_, d), a) in drawables.iter_mut().zip(owners) {
        let name = index.and_then(|i| i.name_of(a.albedo));
        if a.palette == 0 || !tinted.is_tinted(a.albedo, name) {
            continue;
        }
        let Some(name) = name else { continue };
        let Some(palette) = palettes.textures.iter().find(|t| joaat(&t.name.to_ascii_lowercase()) == a.palette) else {
            continue;
        };
        let new_name = format!("{name}+{}_{}_{}", a.tints[0], a.tints[1], a.tints[2]);
        if !baked.iter().any(|t| t.name == new_name) {
            let find =
                |dict: &TextureDictionary| dict.textures.iter().find(|t| t.name.eq_ignore_ascii_case(name)).cloned();
            let Some(albedo) =
                d.textures.as_ref().and_then(find).or_else(|| external.iter().find_map(|(_, x)| find(x)))
            else {
                continue;
            };
            let (Some(rgba), Some(pal)) = (albedo.to_rgba(), palette.to_rgba()) else { continue };
            let data = crate::tint::bake(&rgba, &pal, palette.width as usize, palette.height as usize, a.tints);
            baked.push(Texture {
                name: new_name.clone(),
                width: albedo.width.max(1),
                height: albedo.height.max(1),
                depth: 1,
                levels: 1,
                format: rage_formats::texture::Format::R8G8B8A8_UNORM,
                dimension: albedo.dimension,
                usage: albedo.usage,
                data,
            });
        }
        for shader in &mut d.shaders {
            for t in shader.textures.iter_mut().filter(|t| t.eq_ignore_ascii_case(name)) {
                *t = new_name.clone();
            }
            for (_, t) in shader.samplers.iter_mut().filter(|(_, t)| t.eq_ignore_ascii_case(name)) {
                *t = new_name.clone();
            }
        }
    }
    if baked.is_empty() {
        return;
    }
    // A dictionary of their own, shaped like the palettes' one.
    let mut dict = (*palettes).clone();
    dict.stored.clear();
    dict.errors.clear();
    dict.textures = baked;
    external.push(("tinted (palettes)".to_owned(), dict));
}

/// The face rig of human heads: the base skeletons lack the face bones that
/// heads are bound to and that face shapes move. Only one model in the game
/// carries them in its own skeleton (53 of the 93 bones a head names; the
/// others have no definition and follow the head).
const RDR2_FACE_RIG: &str = "packs_1.rpf/packs/base/models/metapeds_cutscene_a_f.rpf/cs_creoledoctor_ms1_merge_2.ydd";

/// The face rig skeleton (read once).
fn rdr2_face_rig(game: &Path) -> Option<drawable::Skeleton> {
    static CACHE: OnceLock<std::sync::Mutex<HashMap<std::path::PathBuf, Option<drawable::Skeleton>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(r) = cache.lock().ok().and_then(|c| c.get(game).cloned()) {
        return r;
    }
    let rig = (|| {
        let (Node::Rpf8File(rpf, i), _) = rage_mods::resolve(game, RDR2_FACE_RIG, None).ok()? else { return None };
        let res = rpf.read_resource(rpf.entries()?.get(i)?).ok()?;
        let skeletons = drawable::parse_ydd_rdr2(&res).ok()?.into_iter().filter_map(|(_, d)| d.skeleton);
        skeletons.filter(|s| s.find_name("FACIAL_C_FacialRoot").is_some()).max_by_key(|s| s.bones.len())
    })();
    if let Ok(mut c) = cache.lock() {
        c.insert(game.to_path_buf(), rig.clone());
    }
    rig
}

/// Adds the bones of a face rig the skeleton lacks (when the parts use one
/// of them): the rig's root hangs from the head, the rest keep their parents.
fn add_face_rig(skeleton: &mut drawable::Skeleton, rig: &drawable::Skeleton, used: impl Fn(u16) -> bool) {
    let Some(head) = skeleton.find_name("SKEL_Head") else { return };
    if !rig.bones.iter().any(|b| skeleton.find_tag(b.tag).is_none() && used(b.tag)) {
        return;
    }
    let mut placed: Vec<Option<usize>> = vec![None; rig.bones.len()];
    for (i, b) in rig.bones.iter().enumerate() {
        if let Some(existing) = skeleton.find_tag(b.tag) {
            placed[i] = Some(existing);
            continue;
        }
        let parent = match usize::try_from(b.parent).ok().filter(|&p| p < i) {
            Some(p) => placed[p].unwrap_or(head),
            None => head,
        };
        placed[i] = Some(skeleton.bones.len());
        skeleton.bones.push(drawable::Bone { parent: parent as i16, ..b.clone() });
    }
}

/// Where RDR2 keeps the expression dictionaries (`<base skeleton>.yed`).
const RDR2_EXPRESSIONS: &str = "anim_0.rpf/anim/expressions.rpf";

/// The expressions of a base skeleton that shape a ped's face and body from
/// its outfit's values (face features, body build, muscles).
const RDR2_SHAPING: [&str; 3] = ["pack:/bcustom.expr", "pack:/meta_cust.expr", "pack:/muscles.expr"];

/// Shapes a ped's face and body: its outfit's values (by id) are the
/// control values (track type 64) the base skeleton's shaping expressions
/// read; the bone poses they give are baked into the parts' vertices.
/// Returns how many expressions ran.
fn shape_rdr2_ped(
    game: &Path,
    fragment: &str,
    skeleton: &drawable::Skeleton,
    drawables: &mut [(String, Drawable)],
    values: &[(u32, f32)],
) -> usize {
    let stem = fragment.trim_end_matches(".yft");
    let Ok((Node::Rpf8File(rpf, i), _)) = rage_mods::resolve(game, &format!("{RDR2_EXPRESSIONS}/{stem}.yed"), None)
    else {
        return 0;
    };
    let Some(dict) = rpf
        .entries()
        .and_then(|e| e.get(i))
        .and_then(|e| rpf.read_resource(e).ok())
        .and_then(|res| rage_formats::expression::parse_rdr2(&res).ok())
    else {
        return 0;
    };
    let expressions: Vec<_> = RDR2_SHAPING
        .iter()
        .filter_map(|n| dict.expressions.iter().find(|e| e.name.eq_ignore_ascii_case(n)).cloned())
        .collect();
    if expressions.is_empty() {
        return 0;
    }
    let still = rage_formats::clip::Animation::empty();
    let binding = crate::anim::Binding::new(skeleton, &still).with_expressions(skeleton, &still, &expressions);
    let controls: crate::expr::Controls =
        values.iter().filter(|(id, _)| *id <= 0xFFFF).map(|(id, v)| ((*id as u16, 64u8), [*v; 4])).collect();
    let skin = binding.skin_with(skeleton, &still, 0.0, controls);
    for (_, d) in drawables.iter_mut() {
        for mesh in &mut d.meshes {
            if mesh.bone_indices.len() != mesh.positions.len() || mesh.bone_weights.len() != mesh.positions.len() {
                continue;
            }
            for (k, p) in mesh.positions.iter_mut().enumerate() {
                *p = crate::anim::skin_point(&skin, mesh.bone_indices[k], mesh.bone_weights[k], *p, false);
            }
            if mesh.normals.len() == mesh.bone_indices.len() {
                for (k, n) in mesh.normals.iter_mut().enumerate() {
                    let v = crate::anim::skin_point(&skin, mesh.bone_indices[k], mesh.bone_weights[k], *n, true);
                    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
                    *n = if len > 1e-6 { [v[0] / len, v[1] / len, v[2] / len] } else { *n };
                }
            }
        }
    }
    expressions.len()
}

/// The part of a texture name before its variation (`pant_mf1_061_c3_000_ab`
/// → `pant_mf1_061`): everything before the first `_c<digit>` token.
fn texture_stem(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    let tokens: Vec<&str> = lower.split('_').collect();
    let end = tokens
        .iter()
        .position(|t| t.len() >= 2 && t.starts_with('c') && t[1..].chars().all(|c| c.is_ascii_digit()))
        .unwrap_or(tokens.len());
    tokens[..end].join("_")
}

/// Puts `texture` in place of the shader textures ending in `suffix` (`_ab`,
/// `_nm`) that belong to the same piece (same name stem); when none does,
/// in place of all of them.
fn replace_texture(d: &mut Drawable, suffix: &str, texture: &str) {
    let stem = texture_stem(texture);
    let ends = |n: &str| n.to_ascii_lowercase().ends_with(suffix);
    let same = d.shaders.iter().any(|s| s.textures.iter().any(|t| ends(t) && texture_stem(t) == stem));
    for shader in &mut d.shaders {
        let fits = |t: &str| ends(t) && (!same || texture_stem(t) == stem);
        for t in shader.textures.iter_mut().filter(|t| fits(t)) {
            *t = texture.to_owned();
        }
        for (_, t) in shader.samplers.iter_mut().filter(|(_, t)| fits(t)) {
            *t = texture.to_owned();
        }
    }
}

/// Where RDR2 keeps the peds' base skeletons (`meta_base_<ped>.yft`).
const RDR2_BASE_SKELETONS: &str = "packs_0.rpf/packs/base/models/metaped_fragments.rpf";

/// The base skeleton of an RDR2 ped part: `a_c_cow_merge_4` → `meta_base_cow`,
/// `a_c_bear_01_merge_2` → `meta_base_bear_01` (the longest name that exists,
/// with or without the `a_c_`/`cs_` prefix).
fn rdr2_base_skeleton(rpf: &rage_rpf::Rpf8, stem: &str) -> Option<drawable::Skeleton> {
    rdr2_base_skeleton_in(&crate::rdr2::game_folder(rpf.path())?, stem)
}

/// A base skeleton by its file name (`meta_base_male.yft`).
fn rdr2_skeleton_named(game: &Path, file: &str) -> Option<drawable::Skeleton> {
    let (Node::Rpf8Dir(frags, _), _) = rage_mods::resolve(game, RDR2_BASE_SKELETONS, None).ok()? else {
        return None;
    };
    let entry = frags.entries()?.get(frags.find(file)?)?.clone();
    drawable::parse_fragment_rdr2(&frags.read_resource(&entry).ok()?).ok()?.drawable.skeleton
}

fn rdr2_base_skeleton_in(game: &Path, stem: &str) -> Option<drawable::Skeleton> {
    let (Node::Rpf8Dir(frags, _), _) = rage_mods::resolve(game, RDR2_BASE_SKELETONS, None).ok()? else {
        return None;
    };
    let lower = stem.to_ascii_lowercase();
    let tokens: Vec<&str> = lower.split('_').collect();
    let starts = [0usize, 1, 2].into_iter().filter(|&s| s < tokens.len());
    for start in starts {
        for end in (start + 1..=tokens.len()).rev() {
            let name = format!("meta_base_{}.yft", tokens[start..end].join("_"));
            let Some(i) = frags.find(&name) else { continue };
            let entry = frags.entries()?.get(i)?.clone();
            let skeleton = drawable::parse_fragment_rdr2(&frags.read_resource(&entry).ok()?).ok()?.drawable.skeleton;
            if skeleton.as_ref().is_some_and(|s| s.bones.iter().any(|b| b.name.eq_ignore_ascii_case("SKEL_ROOT"))) {
                return skeleton;
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ped_textures() {
        assert_eq!(texture_stem("pant_mf1_061_c3_000_ab"), "pant_mf1_061");
        assert_eq!(texture_stem("Head_MR1_sc08_rough_c0_046_ab"), "head_mr1_sc08_rough");
        assert_eq!(texture_stem("eyes_gen_0000_c0_999_ab"), "eyes_gen_0000");
        assert!(is_rdr2_ped_file("cs_johnmarston.ymt", "packs_0.rpf/packs/base/data/metapeds/metapeds.rpf"));
        assert!(is_rdr2_ped_file("a_c_cow.ymt", "update_4.rpf/x/metapeds_val.rpf"));
        assert!(!is_rdr2_ped_file("metapeds.ymt", "packs_0.rpf/x/metapeds.rpf"));
        assert!(!is_rdr2_ped_file("shop_items.ymt", "packs_0.rpf"));
        let shader = |names: &[&str]| drawable::Shader {
            name_hash: 0,
            textures: names.iter().map(|n| n.to_string()).collect(),
            samplers: names.iter().map(|n| (0, n.to_string())).collect(),
        };
        let mut d = Drawable {
            shaders: vec![
                shader(&["hand_MR1_000_c0_000_ab", "hand_MR1_000_c0_000_nm"]),
                shader(&["hand_MR1_001_c0_000_ab", "hand_MR1_001_c0_000_nm"]),
            ],
            ..Default::default()
        };
        // Only the piece the texture belongs to changes.
        replace_texture(&mut d, "_ab", "hand_mr1_001_c0_015_ab");
        assert_eq!(d.shaders[0].textures[0], "hand_MR1_000_c0_000_ab");
        assert_eq!(d.shaders[1].textures[0], "hand_mr1_001_c0_015_ab");
        assert_eq!(d.shaders[1].samplers[0].1, "hand_mr1_001_c0_015_ab");
        // No piece of that name: every colour texture changes.
        replace_texture(&mut d, "_nm", "other_c0_000_nm");
        assert!(d.shaders.iter().all(|s| s.textures[1] == "other_c0_000_nm"));
    }

    #[test]
    fn face_rig_hangs_from_the_head() {
        let bone = |name: &str, tag: u16, parent: i16| drawable::Bone {
            name: name.into(),
            tag,
            parent,
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
            ..Default::default()
        };
        let mut base = drawable::Skeleton { bones: vec![bone("SKEL_ROOT", 0, -1), bone("SKEL_Head", 31086, 0)] };
        let rig = drawable::Skeleton {
            bones: vec![
                bone("FACIAL_C_FacialRoot", 3280, -1),
                bone("FACIAL_C_Skull", 4956, 0),
                bone("FACIAL_L_Temple", 13658, 0),
            ],
        };
        // Not used by any part: nothing added.
        add_face_rig(&mut base, &rig, |_| false);
        assert_eq!(base.bones.len(), 2);
        add_face_rig(&mut base, &rig, |t| t == 4956);
        assert_eq!(base.bones.len(), 5);
        assert_eq!(base.bones[2].parent, 1, "the rig's root hangs from the head");
        assert_eq!(base.bones[3].parent, 2);
        assert_eq!(base.find_tag(13658), Some(4));
    }

    #[test]
    fn ped_parts_bind_by_tag() {
        let bone = |name: &str, tag: u16, parent: i16| drawable::Bone {
            name: name.into(),
            tag,
            parent,
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
            ..Default::default()
        };
        let base = drawable::Skeleton {
            bones: vec![bone("SKEL_ROOT", 0, -1), bone("SKEL_Pelvis", 11816, 0), bone("SKEL_Head", 31086, 1)],
        };
        // Hair: the part's own skeleton has bones the base lacks.
        let d = Drawable {
            skeleton: Some(drawable::Skeleton {
                bones: vec![bone("MH_HairRoot", 500, -1), bone("MH_Hair_00", 501, 0)],
            }),
            meshes: vec![drawable::Mesh {
                bone_indices: vec![[0, 1, 2, 0]],
                bone_weights: vec![[0.5, 0.25, 0.25, 0.0]],
                bone_tags: vec![31086, 11816, 501],
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut parts = vec![("hair".to_owned(), d)];
        fit_to_skeleton(&mut parts, &base);
        let mesh = &parts[0].1.meshes[0];
        // Head, pelvis, and the hair bone falls back to the head.
        assert_eq!(mesh.bone_indices[0], [2, 1, 2, 2]);
        assert!(mesh.bone_tags.is_empty());
    }

    #[test]
    fn cloth_follows_the_nearest_bones() {
        // A thigh going down from the pelvis, and the spine going up.
        let segments = [(1u16, [0.0, 0.0, 0.0], [0.0, 0.0, -0.4]), (2, [0.0, 0.0, 0.0], [0.0, 0.0, 0.5])];
        let (bones, weights) = nearest_bones(&segments, [0.1, 0.0, -0.3]);
        assert_eq!(bones[0], 1);
        assert!(weights[0] > weights[1] && (weights.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        let (bones, _) = nearest_bones(&segments, [0.1, 0.0, 0.4]);
        assert_eq!(bones[0], 2);
    }
}
