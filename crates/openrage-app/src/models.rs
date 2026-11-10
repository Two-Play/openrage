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
    /// Index into [`LoadedModel::drawables`].
    pub drawable: usize,
    /// Model-space placement.
    pub transform: Mat4,
    pub visible: bool,
}

pub struct LoadedModel {
    /// Drawables with display names (one for `.ydr`, the body and its children
    /// for `.yft`, several for `.ydd`).
    pub drawables: Vec<(String, Drawable)>,
    /// What is shown: drawables placed in the model (wheels appear four times).
    pub parts: Vec<Part>,
    /// Texture dictionaries found next to the model (`<name>.ytd`, `vehshare.ytd`).
    pub external: Vec<(String, TextureDictionary)>,
}

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

/// Which drawables of a dictionary to show at first. Peds: the first variant of
/// every component (head, upper body, legs, …) — a whole person; otherwise
/// the first drawable.
fn default_visibility(names: &[String]) -> Vec<bool> {
    let component = |n: &str| n.split('_').next().map(str::to_owned);
    let is_ped = names.iter().filter(|n| ped_component_names().values().any(|v| v == *n)).count() >= 2;
    if !is_ped {
        return (0..names.len()).map(|i| i == 0).collect();
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
    let mut parts = vec![Part { name: name.to_owned(), drawable: 0, transform: math::IDENTITY, visible: true }];
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
        parts.push(Part { name: bone_name, drawable: index, transform, visible: true });
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
            parts.push(Part { name: bone.name.clone(), drawable: src, transform, visible: true });
        }
    }
    (drawables, parts)
}

pub fn is_model(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.ends_with(".ydr") || n.ends_with(".yft") || n.ends_with(".ydd")
}

/// Loads a model as the game sees it (mods included).
pub fn load(game_dir: &Path, keys: Option<Arc<GtaVKeys>>, vpath: &str) -> Result<LoadedModel> {
    let (node, _) = rage_mods::resolve(game_dir, vpath, keys)?;
    let Node::ArchiveFile(rpf, id) = node else { bail!("{vpath} is not a file inside an archive") };
    let name = rpf.entry(id).name.clone();
    let res = rpf.read_resource(id)?;
    let lower = name.to_ascii_lowercase();
    let stem = name.split('.').next().unwrap_or(&name).to_owned();
    let (drawables, parts) = if lower.ends_with(".ydd") {
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
        let parts = default_visibility(&names)
            .into_iter()
            .enumerate()
            .map(|(i, visible)| Part { name: names[i].clone(), drawable: i, transform: math::IDENTITY, visible })
            .collect();
        (drawables, parts)
    } else if lower.ends_with(".yft") {
        fragment_parts(drawable::parse_fragment(&res)?, &stem)
    } else if lower.ends_with(".ydr") {
        let part = Part { name: stem.clone(), drawable: 0, transform: math::IDENTITY, visible: true };
        (vec![(stem.clone(), drawable::parse_ydr(&res)?)], vec![part])
    } else {
        bail!("{name} is not a model (.ydr, .yft, .ydd)");
    };

    // Textures usually live next to the model: adder.yft → adder.ytd (+ vehshare.ytd for vehicles).
    let mut external = Vec::new();
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
    Ok(LoadedModel { drawables, parts, external })
}

/// Exports the parts shown by `visible` as `.glb` (one node per part).
pub fn to_glb(model: &LoadedModel, name: &str, visible: &[bool]) -> Option<Vec<u8>> {
    let meshes: Vec<(&str, &Drawable)> = model.drawables.iter().map(|(n, d)| (n.as_str(), d)).collect();
    let nodes: Vec<rage_formats::gltf::Node> = model
        .shown(visible)
        .map(|(p, _)| rage_formats::gltf::Node { name: p.name.clone(), mesh: p.drawable, transform: p.transform })
        .collect();
    if nodes.is_empty() {
        return None;
    }
    Some(rage_formats::gltf::write_glb_scene(name, &meshes, &nodes, &|d, t| model.texture(&model.drawables[d].1, t)))
}
