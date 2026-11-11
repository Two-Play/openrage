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
                .map(|m| MaterialLook { visible: true, tint: if m.textured { [1.0; 4] } else { PLAIN } })
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
    n.ends_with(".ydr") || n.ends_with(".yft") || n.ends_with(".ydd") || n.ends_with(".ypt")
}

/// Loads a model as the game sees it (mods included).
pub fn load(game_dir: &Path, keys: Option<Arc<GtaVKeys>>, vpath: &str) -> Result<LoadedModel> {
    let (node, _) = rage_mods::resolve(game_dir, vpath, keys)?;
    let Node::ArchiveFile(rpf, id) = node else { bail!("{vpath} is not a file inside an archive") };
    let name = rpf.entry(id).name.clone();
    let res = rpf.read_resource(id)?;
    let lower = name.to_ascii_lowercase();
    let stem = name.split('.').next().unwrap_or(&name).to_owned();
    let mut external = Vec::new();
    let (drawables, parts) = if lower.ends_with(".ypt") {
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
        bail!("{name} is not a model (.ydr, .yft, .ydd, .ypt)");
    };

    // Textures usually live next to the model: adder.yft → adder.ytd (+ vehshare.ytd for vehicles).
    let dir = rpf.entry(id).parent.unwrap_or(rage_rpf::Rpf7::ROOT);
    let mut candidates = vec![format!("{stem}.ytd"), format!("{stem}+hi.ytd")];
    if lower.ends_with(".yft") {
        candidates.push("vehshare.ytd".into());
    }
    for c in candidates {
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
