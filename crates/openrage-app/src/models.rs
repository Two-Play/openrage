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
    /// Textures shared with other models (GTA VI).
    shared: Vec<Arc<Texture>>,
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
    /// Readable name ("Primary paint", "Glass", `normal_spec`, â€¦).
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
    /// A model put together from drawables (e.g. by a plugin), each shown
    /// once in place.
    pub fn from_drawables(drawables: Vec<(String, Drawable)>) -> Self {
        let parts = drawables
            .iter()
            .enumerate()
            .map(|(i, (name, _))| Part {
                name: name.clone(),
                group: String::new(),
                drawable: i,
                transform: math::IDENTITY,
                visible: true,
                bone: None,
            })
            .collect();
        let mut model = LoadedModel {
            drawables,
            parts,
            external: Vec::new(),
            materials: Vec::new(),
            shared: Vec::new(),
            shader_material: Vec::new(),
            skeleton: None,
            expressions: Vec::new(),
            ped: None,
        };
        model.collect_materials();
        model
    }

    /// Finds a texture by name: embedded in the drawable first, then next to the model.
    pub fn texture<'a>(&'a self, drawable: &'a Drawable, name: &str) -> Option<&'a Texture> {
        let find = |d: &'a TextureDictionary| {
            d.textures
                .iter()
                .find(|t| t.name.eq_ignore_ascii_case(name))
        };
        let own = drawable
            .textures
            .as_ref()
            .and_then(find)
            .or_else(|| self.external.iter().find_map(|(_, d)| find(d)));
        own.or_else(|| self.shared_texture(name).map(|t| &**t))
    }

    /// The texture of this name that is shared with other models.
    pub(crate) fn shared_texture(&self, name: &str) -> Option<&Arc<Texture>> {
        self.shared
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case(name))
    }

    /// Gives the model textures it shares with others.
    pub fn set_shared(&mut self, textures: Vec<Arc<Texture>>) {
        self.shared = textures;
        self.collect_materials();
    }

    /// Lets go of the shared textures: what shows the model has them.
    pub fn forget_shared(&mut self) {
        self.shared = Vec::new();
    }

    /// Dresses a GTA IV ped: for each (component, drawable, texture) â€” a
    /// component like `hand`, its drawable number (`hand_001_r`) and texture
    /// variant (0 `a`, 1 `b`, â€¦) â€” shows that drawable instead of the
    /// component's others and gives it that texture variant
    /// (`hand_diff_001_b_whi`). Choices the model lacks are left out.
    pub fn choose_gta4_components(&mut self, choices: &[(&str, u32, u32)]) {
        for &(component, drawable, texture) in choices {
            self.choose_gta4_component(component, drawable, texture);
        }
    }

    fn choose_gta4_component(&mut self, component: &str, drawable: u32, texture: u32) {
        let wanted = format!("{component}_{drawable:03}_");
        let of_component = |n: &str| is_component_variant(n, component, wanted.len());
        if !self
            .parts
            .iter()
            .any(|p| p.name.starts_with(&wanted) && of_component(&p.name))
        {
            return;
        }
        for p in self.parts.iter_mut().filter(|p| of_component(&p.name)) {
            p.visible = p.name.starts_with(&wanted);
        }
        let Some(letter) = char::from_u32('a' as u32 + texture).filter(|c| c.is_ascii_lowercase())
        else {
            return;
        };
        let diffuse = format!("{component}_diff_{drawable:03}_").to_ascii_lowercase();
        let variant = format!("{diffuse}{letter}");
        for k in 0..self.drawables.len() {
            if self.drawables[k].0.starts_with(&wanted) {
                self.retexture_gta4_drawable(k, &diffuse, &variant);
            }
        }
    }

    /// Gives drawable `k` the texture variant `variant` where it uses `diffuse`.
    fn retexture_gta4_drawable(&mut self, k: usize, diffuse: &str, variant: &str) {
        let d = &self.drawables[k].1;
        let found = d
            .textures
            .iter()
            .chain(self.external.iter().map(|(_, t)| t))
            .flat_map(|t| &t.textures)
            .find(|t| t.name.to_ascii_lowercase().starts_with(variant))
            .map(|t| t.name.clone());
        let Some(found) = found else { return };
        let rename = |n: &mut String| {
            if n.to_ascii_lowercase().starts_with(diffuse) {
                n.clone_from(&found);
            }
        };
        for shader in &mut self.drawables[k].1.shaders {
            shader.textures.iter_mut().for_each(rename);
            shader.samplers.iter_mut().for_each(|(_, n)| rename(n));
        }
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
        self.shader_material
            .get(drawable)?
            .get(shader as usize)
            .copied()
    }

    /// Colour of a drawable's shader under `look`, `None` if it is hidden.
    pub fn tint(&self, look: &Look, drawable: usize, shader: u16) -> Option<[f32; 4]> {
        match self
            .material_index(drawable, shader)
            .and_then(|m| look.materials.get(m))
        {
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
                let textured = shader
                    .base_texture()
                    .is_some_and(|t| self.texture(d, t).is_some());
                let index = match materials
                    .iter()
                    .position(|m| m.name_hash == shader.name_hash)
                {
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
    pub fn shown<'a>(
        &'a self,
        visible: &'a [bool],
    ) -> impl Iterator<Item = (&'a Part, &'a Drawable)> + 'a {
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

/// Ped component names (`head_000_r`, `uppr_001_u`, â€¦) by hash: drawable
/// dictionaries of peds only store the hashes.
/// The names of a `.ydd`'s drawables as the viewer shows them (and OBJ
/// export names their parts): ped components by name (`uppr_000_u`), others
/// `<file>#<hash>`, or `<file>#<index>` without a hash.
pub fn dictionary_names(stem: &str, hashes: &[u32]) -> Vec<String> {
    hashes
        .iter()
        .enumerate()
        .map(|(i, &hash)| match ped_component_names().get(&hash) {
            Some(n) => n.clone(),
            None if hash != 0 => format!("{stem}#{hash:08x}"),
            None => format!("{stem}#{i}"),
        })
        .collect()
}

/// Whether `name` is a drawable of `component` (`hand_001_r`: the component, a number, a letter).
fn is_component_variant(name: &str, component: &str, prefix_len: usize) -> bool {
    name.len() == prefix_len + 1
        && name.starts_with(&format!("{component}_"))
        && ped_component_names().contains_key(&joaat(name))
}

fn ped_component_names() -> &'static HashMap<u32, String> {
    static NAMES: OnceLock<HashMap<u32, String>> = OnceLock::new();
    NAMES.get_or_init(|| {
        let mut map = HashMap::new();
        // `suse` (special) is a GTA IV component.
        for comp in [
            "head", "berd", "hair", "uppr", "lowr", "hand", "feet", "teef", "accs", "task", "decl",
            "jbib", "suse",
        ] {
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
    names
        .iter()
        .filter(|n| ped_component_names().values().any(|v| v == *n))
        .count()
        >= 2
}

/// How many drawables of a (non-ped) dictionary are shown at first.
const SHOWN_AT_FIRST: usize = 64;

/// Which drawables of a dictionary to show at first. Peds: the first variant of
/// every component (head, upper body, legs, â€¦) â€” a whole person; other
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
    names
        .iter()
        .map(|n| best.values().any(|b| b == n))
        .collect()
}

/// Places a fragment's children on their bones. Wheels exist once per axle
/// side in the file and are copied to every wheel bone (turned around on the
/// right side).
/// Component groups of a streamed ped worth showing by default (accessories,
/// tasks and decals are left out).
const STREAMED_GROUPS: &[&str] = &[
    "head", "berd", "hair", "uppr", "lowr", "hand", "feet", "teef", "jbib",
];

type StreamedPed = (
    Vec<(String, Drawable)>,
    Vec<Part>,
    Vec<(String, TextureDictionary)>,
);

/// The first variant of every component in a streamed ped's folder
/// (`head_000_r.ydd`, `uppr_000_u.ydd`, â€¦) and its first texture variant
/// (`head_diff_000_a_whi.ytd`).
fn streamed_ped(rpf: &rage_rpf::Rpf7, folder: rage_rpf::EntryId) -> Result<StreamedPed> {
    let files: Vec<(String, rage_rpf::EntryId)> = rpf
        .children(folder)
        .map(|c| (rpf.entry(c).name.to_ascii_lowercase(), c))
        .collect();
    let (mut drawables, mut parts, mut textures) = (Vec::new(), Vec::new(), Vec::new());
    for group in STREAMED_GROUPS {
        let Some((stem, d, dict)) = streamed_component(rpf, &files, group) else {
            continue;
        };
        textures.extend(dict);
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

/// A ped component's name and drawable, with its texture dictionary (name and dictionary) if any.
type PedComponent = (String, Drawable, Option<(String, TextureDictionary)>);

type StreamedFile = (String, rage_rpf::EntryId);

/// The first (by name) of the files that start with `prefix` and end with `ext`.
fn first_streamed<'a>(
    files: &'a [StreamedFile],
    prefix: &str,
    ext: &str,
) -> Option<&'a StreamedFile> {
    let mut candidates: Vec<&StreamedFile> = files
        .iter()
        .filter(|(n, _)| n.starts_with(prefix) && n.ends_with(ext))
        .collect();
    candidates.sort();
    candidates.first().copied()
}

/// A component's first drawable (name and drawable) with its first texture variant, if any.
fn streamed_component(
    rpf: &rage_rpf::Rpf7,
    files: &[StreamedFile],
    group: &str,
) -> Option<PedComponent> {
    let (name, id) = first_streamed(files, &format!("{group}_"), ".ydd")?;
    let res = rpf.read_resource(*id).ok()?;
    let (_, d) = drawable::parse_ydd(&res)
        .ok()
        .and_then(|v| v.into_iter().next())?;
    let stem = name.trim_end_matches(".ydd").to_owned();
    // `head_000_r` â†’ textures `head_diff_000_a_*.ytd`.
    let variant = stem.split('_').nth(1).unwrap_or("000");
    let dict = streamed_textures(rpf, files, &format!("{group}_diff_{variant}_a"));
    Some((stem, d, dict))
}

fn streamed_textures(
    rpf: &rage_rpf::Rpf7,
    files: &[StreamedFile],
    prefix: &str,
) -> Option<(String, TextureDictionary)> {
    let (tname, tid) = first_streamed(files, prefix, ".ytd")?;
    let dict = rpf
        .read_resource(*tid)
        .map_err(anyhow::Error::from)
        .and_then(|r| Ok(rage_formats::ytd::parse(&r)?))
        .ok()?;
    Some((tname.clone(), dict))
}

fn fragment_parts(
    fragment: drawable::Fragment,
    name: &str,
) -> (Vec<(String, Drawable)>, Vec<Part>) {
    let drawable::Fragment {
        drawable: body,
        children,
        cloths,
    } = fragment;
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
    add_cloth_parts(cloths, &mut drawables, &mut parts);
    let wheels = add_child_parts(children, &skeleton, &world, &mut drawables, &mut parts);
    // Copy wheels to the wheel bones that have no drawable of their own.
    if !wheels.is_empty() {
        copy_wheels(&skeleton, &world, &wheels, &mut parts);
    }
    (drawables, parts)
}

/// Cloth (RDR2's flags and straps) in its rest pose: the game moves it.
fn add_cloth_parts(
    cloths: Vec<Drawable>,
    drawables: &mut Vec<(String, Drawable)>,
    parts: &mut Vec<Part>,
) {
    for (k, cloth) in cloths.into_iter().enumerate() {
        let label = if k == 0 {
            "cloth".to_owned()
        } else {
            format!("cloth_{k}")
        };
        let index = drawables.len();
        parts.push(Part {
            name: label.clone(),
            group: "Cloth".into(),
            drawable: index,
            transform: math::IDENTITY,
            visible: true,
            bone: None,
        });
        drawables.push((label, cloth));
    }
}

/// The fragment's children on their bones; returns the wheels (bone name, drawable).
fn add_child_parts(
    children: Vec<drawable::FragmentChild>,
    skeleton: &drawable::Skeleton,
    world: &[Mat4],
    drawables: &mut Vec<(String, Drawable)>,
    parts: &mut Vec<Part>,
) -> Vec<(String, usize)> {
    let mut wheels: Vec<(String, usize)> = Vec::new();
    for child in children {
        let bone = skeleton.find_tag(child.bone_tag);
        let bone_name = bone
            .map(|b| skeleton.bones[b].name.clone())
            .unwrap_or_else(|| format!("part_{:04x}", child.bone_tag));
        let index = drawables.len();
        drawables.push((bone_name.clone(), child.drawable));
        let transform = bone
            .and_then(|b| world.get(b))
            .copied()
            .unwrap_or(math::IDENTITY);
        // RDR2's blurred wheels for high speeds (`wheel_lf_fast`) stay hidden.
        let variant = is_wheel_variant(&bone_name);
        if bone_name.starts_with("wheel_") && !variant {
            wheels.push((bone_name.clone(), index));
        }
        let group = if bone_name.starts_with("wheel_") {
            "Wheels"
        } else {
            "Parts"
        };
        parts.push(Part {
            name: bone_name,
            group: group.into(),
            drawable: index,
            transform,
            visible: !variant,
            bone,
        });
    }
    wheels
}

fn is_right_wheel(n: &str) -> bool {
    n.strip_prefix("wheel_").is_some_and(|s| s.starts_with('r'))
}

fn is_rear_wheel(n: &str) -> bool {
    n.ends_with('r') || n.contains("_lm") || n.contains("_rm")
}

/// The wheel to copy onto wheel bone `n`: same side and axle if available,
/// else any wheel (front preferred).
fn wheel_source<'a>(wheels: &'a [(String, usize)], n: &str) -> Option<&'a (String, usize)> {
    wheels
        .iter()
        .find(|(w, _)| {
            is_rear_wheel(w) == is_rear_wheel(n) && is_right_wheel(w) == is_right_wheel(n)
        })
        .or_else(|| {
            wheels
                .iter()
                .find(|(w, _)| is_rear_wheel(w) == is_rear_wheel(n))
        })
        .or_else(|| wheels.first())
}

fn copy_wheels(
    skeleton: &drawable::Skeleton,
    world: &[Mat4],
    wheels: &[(String, usize)],
    parts: &mut Vec<Part>,
) {
    for (b, bone) in skeleton.bones.iter().enumerate() {
        let n = bone.name.to_ascii_lowercase();
        let is_wheel_bone = n.starts_with("wheel_") && !n.contains("mesh") && !is_wheel_variant(&n);
        if !is_wheel_bone || wheels.iter().any(|(w, _)| w.eq_ignore_ascii_case(&n)) {
            continue;
        }
        let Some((src_name, src)) = wheel_source(wheels, &n) else {
            continue;
        };
        let mut transform = world[b];
        if is_right_wheel(&n) != is_right_wheel(src_name) {
            transform = math::mul(&transform, &math::rotation_z(std::f32::consts::PI));
        }
        parts.push(Part {
            name: bone.name.clone(),
            group: "Wheels".into(),
            drawable: *src,
            transform,
            visible: true,
            bone: Some(b),
        });
    }
}

/// A vehicle's mod parts ([`crate::vehicle_mods`]), hidden at first, each on
/// the bone the game puts it on, grouped by kind (`Mods: bonnet`).
fn add_mod_parts(
    game_dir: &Path,
    keys: Option<Arc<GtaVKeys>>,
    vehicle: &str,
    drawables: &mut Vec<(String, Drawable)>,
    parts: &mut Vec<Part>,
) {
    let Some(skeleton) = drawables.first().and_then(|(_, d)| d.skeleton.clone()) else {
        return;
    };
    // Vehicles only: their skeleton has a chassis.
    if skeleton.find_name("chassis").is_none() {
        return;
    }
    let world = skeleton.world_transforms();
    let mods = crate::vehicle_mods::mod_parts(game_dir, keys.clone(), vehicle);
    let total = mods.len() as u64;
    for (k, m) in mods.into_iter().enumerate() {
        crate::progress::set("Loading the mod parts", k as u64, total);
        let Ok((Node::ArchiveFile(rpf, id), _)) =
            rage_mods::resolve(game_dir, &m.vpath, keys.clone())
        else {
            continue;
        };
        let Some(fragment) = rpf
            .read_resource(id)
            .ok()
            .and_then(|r| drawable::parse_fragment(&r).ok())
        else {
            continue;
        };
        if fragment.drawable.meshes.is_empty() {
            continue;
        }
        // By name, or by its hash where `carcols` keeps one.
        let bone = m.bone.as_deref().and_then(|b| {
            let hash = rage_formats::names::parse(&b.to_ascii_lowercase());
            skeleton.find_name(b).or_else(|| {
                skeleton
                    .bones
                    .iter()
                    .position(|x| rage_core::hash::joaat(&x.name.to_ascii_lowercase()) == hash)
            })
        });
        let kind = m
            .kind
            .as_deref()
            .map(crate::vehicle_mods::kind_label)
            .unwrap_or_else(|| "other".into());
        parts.push(Part {
            name: m.name.clone(),
            group: format!("Mods: {kind}"),
            drawable: drawables.len(),
            transform: bone
                .and_then(|b| world.get(b))
                .copied()
                .unwrap_or(math::IDENTITY),
            visible: false,
            bone,
        });
        let mut d = fragment.drawable;
        // Its own one-bone skeleton is not the vehicle's.
        d.skeleton = None;
        drawables.push((m.name, d));
    }
}

/// RDR2 wheel bones besides the wheel itself: the blurred one shown at
/// speed (`wheel_lf_fast`) and the one for upgrades (`wheel_lf_mod`).
fn is_wheel_variant(bone: &str) -> bool {
    let n = bone.to_ascii_lowercase();
    n.starts_with("wheel_") && (n.ends_with("_fast") || n.ends_with("_mod"))
}

/// File extensions of models: GTA V and RDR2 (drawables, fragments, dictionaries,
/// particles, collisions), GTA IV, and GTA VI (`#dr`, `#ft`, `#dd`, `#bn`, `#nv`).
const MODEL_EXTENSIONS: &[&str] = &[
    ".ydr", ".yft", ".ydd", ".ypt", ".ybn", ".wdr", ".wdd", ".wft", ".wbn", ".wbd", ".#dr", ".#ft",
    ".#dd", ".#bn", ".#nv",
];

pub fn is_model(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    MODEL_EXTENSIONS.iter().any(|ext| n.ends_with(ext)) || rage_formats::paths::is_path_file(&n)
}

/// Colour of a collision material, by what it is made of.
fn material_color(name: &str, index: u8) -> [f32; 4] {
    let n = name.to_ascii_uppercase();
    let rgb = |r: u8, g: u8, b: u8| [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0];
    let has = |words: &[&str]| words.iter().any(|w| n.contains(w));
    if has(&[
        "GRASS", "LEAVES", "BUSH", "TREE", "HEDGE", "MOSS", "FOLIAGE",
    ]) {
        rgb(110, 170, 80)
    } else if has(&["WATER", "PUDDLE"]) {
        rgb(80, 150, 220)
    } else if has(&["WOOD", "BARK", "TWIGS", "PLYWOOD"]) {
        rgb(170, 120, 75)
    } else if has(&["METAL", "STEEL", "ALUMIN", "IRON", "CHAIN", "RAIL", "CAR_"]) {
        rgb(120, 140, 170)
    } else if has(&["GLASS", "PERSPEX", "PLEXI"]) {
        rgb(150, 210, 230)
    } else if has(&[
        "SAND", "DIRT", "MUD", "SOIL", "GRAVEL", "CLAY", "STONE", "ROCK",
    ]) {
        rgb(190, 160, 110)
    } else if has(&[
        "CONCRETE", "TARMAC", "ASPHALT", "BRICK", "TILE", "PAVING", "CEMENT", "MARBLE", "PLASTER",
    ]) {
        rgb(175, 175, 180)
    } else if has(&[
        "CARPET", "CLOTH", "FABRIC", "LEATHER", "RUBBER", "FOAM", "PLASTIC",
    ]) {
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
    (
        x(0.0).clamp(0.0, 1.0),
        x(4.0).clamp(0.0, 1.0),
        x(2.0).clamp(0.0, 1.0),
    )
}

/// Names and colours for the GTA IV collision materials `bounds` use, from
/// `common/data/materials/materials.dat` of the game the archive belongs to.
fn register_gta4_materials(
    archive: &rage_rpf::Gta4Archive,
    bounds: &[(String, Drawable, rage_formats::ybn::Stats)],
) {
    static NAMES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    let names = NAMES.get_or_init(|| {
        crate::gta4::game_folder(archive.path())
            .and_then(|g| {
                rage_rpf::vfs::find_case_insensitive(&g, "common/data/materials/materials.dat")
            })
            .and_then(|p| std::fs::read(p).ok())
            .map(|d| rage_formats::ybn::material_names(&String::from_utf8_lossy(&d)))
            .unwrap_or_default()
    });
    for (_, _, stats) in bounds {
        for &m in &stats.materials {
            let label = names
                .get(m as usize)
                .cloned()
                .unwrap_or_else(|| format!("Material {m}"));
            let color = material_color(&label, m);
            rage_formats::shaders::register(
                rage_formats::wbn::MATERIAL_BASE + m as u32,
                label,
                color,
            );
        }
    }
}

/// Names of the collision materials, read once from the game's `materials.dat`.
/// The names of the game's collision materials (`materials.dat`), by type.
pub fn collision_material_names(game_dir: &Path, keys: Option<Arc<GtaVKeys>>) -> &'static [String] {
    static NAMES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    NAMES.get_or_init(|| {
        for path in [
            "update/update.rpf/common/data/materials/materials.dat",
            "common.rpf/data/materials/materials.dat",
        ] {
            if let Ok((Node::ArchiveFile(rpf, id), _)) =
                rage_mods::resolve(game_dir, path, keys.clone())
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
/// characters use `p_m_zero` â€¦), the face setup first, then one per part.
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
    let mut out: Vec<_> = dict
        .expressions
        .iter()
        .filter(|e| e.name.to_ascii_lowercase().ends_with("faceinit.expr"))
        .cloned()
        .collect();
    out.extend(dict.expressions.iter().filter(|e| wanted(e)).cloned());
    out
}

/// The drawables of a model, with how they are laid out.
type Assembled = (Vec<(String, Drawable)>, Vec<Part>);

/// A part showing the only drawable of a model, in place.
fn whole_part(stem: &str) -> Part {
    Part {
        name: stem.to_owned(),
        group: String::new(),
        drawable: 0,
        transform: math::IDENTITY,
        visible: true,
        bone: None,
    }
}

/// A finished model from its pieces.
fn assembled_model(
    (drawables, parts): Assembled,
    external: Vec<(String, TextureDictionary)>,
    skeleton: Option<drawable::Skeleton>,
) -> LoadedModel {
    let mut model = LoadedModel {
        drawables,
        parts,
        external,
        materials: Vec::new(),
        shared: Vec::new(),
        shader_material: Vec::new(),
        skeleton,
        expressions: Vec::new(),
        ped: None,
    };
    model.collect_materials();
    model
}

fn load_model(game_dir: &Path, keys: Option<Arc<GtaVKeys>>, vpath: &str) -> Result<LoadedModel> {
    let (node, _) = rage_mods::resolve(game_dir, vpath, keys.clone())?;
    match node {
        Node::Rpf8File(rpf, i) => load_rdr2(&rpf, i),
        Node::Gta4File(archive, id) => load_gta4(&archive, id),
        Node::Rpf9File(rpf, i) => load_gta6(game_dir, &rpf, i, vpath),
        Node::ArchiveFile(rpf, id) => load_gta5(game_dir, keys, &rpf, id),
        _ => bail!("{vpath} is not a file inside an archive"),
    }
}

fn gta6_collision(rpf: &rage_rpf::Rpf9, i: usize) -> Result<LoadedModel> {
    let (system, _) = crate::rpf9::resource(rpf, i)?;
    let (collision, stats) = rage_formats::bn6::parse(&system)?;
    for material in stats.materials {
        let label = format!("Material {material:04x}");
        let color = material_color(&label, material as u8);
        let id = rage_formats::ybn::MATERIAL_BASE + u32::from(material);
        rage_formats::shaders::register(id, label, color);
    }
    Ok(LoadedModel::from_drawables(vec![(
        "collision".to_owned(),
        collision,
    )]))
}

fn gta6_nav_mesh(rpf: &rage_rpf::Rpf9, i: usize) -> Result<LoadedModel> {
    let (mesh, _) = rage_formats::paths::parse_nv6(crate::rpf9::resource(rpf, i)?.0)?;
    Ok(LoadedModel::from_drawables(vec![(
        "nav mesh".to_owned(),
        mesh,
    )]))
}

/// A GTA VI model: a collision (`.#bn`), a nav mesh (`.#nv`) or a drawable.
fn load_gta6(game_dir: &Path, rpf: &rage_rpf::Rpf9, i: usize, vpath: &str) -> Result<LoadedModel> {
    let lower = vpath.to_ascii_lowercase();
    if lower.ends_with(".#bn") {
        return gta6_collision(rpf, i);
    }
    if lower.ends_with(".#nv") {
        return gta6_nav_mesh(rpf, i);
    }
    let mut drawables = crate::gta6_models::load(rpf, i)?;
    let textures = crate::gta6_textures::for_drawables(game_dir, &drawables);
    let name = vpath.rsplit('/').next().unwrap_or(vpath);
    // A dictionary's drawables have skeletons of their own.
    let worn_on = crate::gta6_models::bind(game_dir, name, &mut drawables);
    let skeleton = worn_on.or_else(|| match drawables.as_slice() {
        [(_, d)] => d.skeleton.clone(),
        _ => None,
    });
    let mut model = LoadedModel::from_drawables(drawables);
    model.skeleton = skeleton;
    if let Some(textures) = textures {
        model.add_textures("texture pools".to_owned(), textures);
    }
    Ok(model)
}

/// The file of a GTA V model being loaded.
struct Gta5File<'a> {
    rpf: &'a rage_rpf::Rpf7,
    id: rage_rpf::EntryId,
    name: &'a str,
    lower: &'a str,
    stem: &'a str,
    res: &'a rage_resource::Resource,
}

/// What a GTA V model file makes up.
enum Gta5Body {
    /// Drawables and parts, and the dictionaries found inside the file.
    Parts(Assembled, Vec<(String, TextureDictionary)>),
    /// A model that is finished already.
    Model(LoadedModel),
}

impl Gta5Body {
    fn plain(assembled: Assembled) -> Self {
        Self::Parts(assembled, Vec::new())
    }
}

fn load_gta5(
    game_dir: &Path,
    keys: Option<Arc<GtaVKeys>>,
    rpf: &rage_rpf::Rpf7,
    id: rage_rpf::EntryId,
) -> Result<LoadedModel> {
    let name = rpf.entry(id).name.clone();
    let res = rpf.read_resource(id)?;
    let lower = name.to_ascii_lowercase();
    let stem = name.split('.').next().unwrap_or(&name).to_owned();
    let file = Gta5File {
        rpf,
        id,
        name: &name,
        lower: &lower,
        stem: &stem,
        res: &res,
    };
    let ((mut drawables, mut parts), mut external) = match gta5_body(game_dir, keys.clone(), &file)?
    {
        Gta5Body::Parts(assembled, external) => (assembled, external),
        Gta5Body::Model(model) => return Ok(model),
    };

    if lower.ends_with(".yft") {
        add_mod_parts(game_dir, keys.clone(), &stem, &mut drawables, &mut parts);
    }
    let dir = rpf.entry(id).parent.unwrap_or(rage_rpf::Rpf7::ROOT);
    add_neighbor_textures(&file, dir, &mut external);
    if lower.ends_with(".yft") {
        add_shared_textures(game_dir, keys, &file, &drawables, &mut external);
    }
    // Peds keep their skeleton in a fragment next to the drawable dictionary.
    let skeleton = drawables
        .iter()
        .find_map(|(_, d)| d.skeleton.clone())
        .or_else(|| {
            let tid = rpf.find_child(dir, &format!("{stem}.yft"))?;
            drawable::parse_fragment(&rpf.read_resource(tid).ok()?)
                .ok()?
                .drawable
                .skeleton
        });
    if let Some(sk) = &skeleton {
        fit_to_skeleton(&mut drawables, sk);
    }
    Ok(assembled_model((drawables, parts), external, skeleton))
}

/// Builds the drawables and parts of a GTA V model file by its kind.
fn gta5_body(game_dir: &Path, keys: Option<Arc<GtaVKeys>>, f: &Gta5File) -> Result<Gta5Body> {
    let lower = f.lower;
    if rage_formats::paths::is_path_file(lower) {
        return path_body(f);
    }
    if lower.ends_with(".ybn") {
        return collision_body(game_dir, keys, f);
    }
    if lower.ends_with(".ypt") {
        return particle_body(f);
    }
    if lower.ends_with(".ydd") {
        return dictionary_body(f);
    }
    if lower.ends_with(".yft") {
        return fragment_body(f);
    }
    if lower.ends_with(".ydr") {
        let drawable = (f.stem.to_owned(), drawable::parse_ydr(f.res)?);
        return Ok(Gta5Body::plain((vec![drawable], vec![whole_part(f.stem)])));
    }
    bail!(
        "{} is not a model (.ydr, .yft, .ydd, .ypt, .ybn, .ynd, .yvr, .ywr)",
        f.name
    );
}

fn path_body(f: &Gta5File) -> Result<Gta5Body> {
    let ext = f.lower.rsplit('.').next().unwrap_or_default();
    let (path, _) = rage_formats::paths::parse(f.res, ext)?;
    Ok(Gta5Body::plain((
        vec![(f.stem.to_owned(), path)],
        vec![whole_part(f.stem)],
    )))
}

fn collision_body(game_dir: &Path, keys: Option<Arc<GtaVKeys>>, f: &Gta5File) -> Result<Gta5Body> {
    let (collision, stats) = rage_formats::ybn::parse(f.res)?;
    let names = collision_material_names(game_dir, keys);
    for &m in &stats.materials {
        let label = names
            .get(m as usize)
            .cloned()
            .unwrap_or_else(|| format!("Material {m}"));
        let color = material_color(&label, m);
        rage_formats::shaders::register(rage_formats::ybn::MATERIAL_BASE + m as u32, label, color);
    }
    Ok(Gta5Body::plain((
        vec![(f.stem.to_owned(), collision)],
        vec![whole_part(f.stem)],
    )))
}

/// Particle models (debris, sparks, â€¦) side by side, textured from the
/// dictionary inside the file.
fn particle_body(f: &Gta5File) -> Result<Gta5Body> {
    let ptfx = rage_formats::ypt::parse(f.res)?;
    if ptfx.drawables.is_empty() {
        bail!("{} contains no models", f.name);
    }
    let stem = f.stem;
    let drawables: Vec<(String, Drawable)> = ptfx
        .drawables
        .into_iter()
        .enumerate()
        .map(|(i, (hash, d))| {
            let label = rage_formats::names::value(hash);
            let label = if hash == 0 || label.starts_with("hash_") {
                format!("{stem}#{i}")
            } else {
                label
            };
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
    let external = ptfx
        .textures
        .map(|t| (format!("{stem}.ypt textures"), t))
        .into_iter()
        .collect();
    Ok(Gta5Body::Parts((drawables, parts), external))
}

fn dictionary_body(f: &Gta5File) -> Result<Gta5Body> {
    let parsed = drawable::parse_ydd(f.res)?;
    let hashes: Vec<u32> = parsed.iter().map(|(h, _)| *h).collect();
    let drawables: Vec<(String, Drawable)> = dictionary_names(f.stem, &hashes)
        .into_iter()
        .zip(parsed)
        .map(|(name, (_, d))| (name, d))
        .collect();
    let names: Vec<String> = drawables.iter().map(|(n, _)| n.clone()).collect();
    // Peds are assembled in place; other dictionaries are laid out in a grid.
    let layout = if is_ped_dictionary(&names) {
        vec![math::IDENTITY; drawables.len()]
    } else {
        grid_layout(&drawables)
    };
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
    Ok(Gta5Body::plain((drawables, parts)))
}

fn fragment_body(f: &Gta5File) -> Result<Gta5Body> {
    let folder = f
        .rpf
        .find_child(
            f.rpf.entry(f.id).parent.unwrap_or(rage_rpf::Rpf7::ROOT),
            f.stem,
        )
        .filter(|&folder| f.rpf.entry(folder).is_dir());
    let Some(folder) = folder else {
        return Ok(Gta5Body::plain(fragment_parts(
            drawable::parse_fragment(f.res)?,
            f.stem,
        )));
    };
    // A streamed ped (online characters, story characters): the fragment
    // holds the skeleton, the folder next to it the components.
    let skeleton = drawable::parse_fragment(f.res)?.drawable.skeleton;
    let (mut drawables, parts, textures) = streamed_ped(f.rpf, folder)?;
    if let Some(sk) = &skeleton {
        fit_to_skeleton(&mut drawables, sk);
    }
    Ok(Gta5Body::Model(assembled_model(
        (drawables, parts),
        textures,
        skeleton,
    )))
}

/// Textures usually live next to the model: adder.yft â†’ adder.ytd (+ vehshare.ytd for vehicles).
fn add_neighbor_textures(
    f: &Gta5File,
    dir: rage_rpf::EntryId,
    external: &mut Vec<(String, TextureDictionary)>,
) {
    let stem = f.stem;
    let mut candidates = vec![format!("{stem}.ytd"), format!("{stem}+hi.ytd")];
    if f.lower.ends_with(".yft") {
        candidates.push("vehshare.ytd".into());
    }
    let total = candidates.len() as u64;
    for (k, c) in candidates.into_iter().enumerate() {
        crate::progress::set("Loading textures", k as u64, total);
        if let Some(tid) = f.rpf.find_child(dir, &c)
            && let Ok(dict) = f
                .rpf
                .read_resource(tid)
                .map_err(anyhow::Error::from)
                .and_then(|r| rage_formats::ytd::parse(&r).context("texture dictionary"))
        {
            external.push((c, dict));
        }
    }
}

/// The shared dictionaries a vehicle needs besides its own.
fn vehicle_shared_names(
    drawables: &[(String, Drawable)],
    external: &[(String, TextureDictionary)],
) -> Vec<String> {
    // Vehicles from DLC packs have no vehshare.ytd beside them: use the game's own.
    let mut shared = Vec::new();
    if !external.iter().any(|(n, _)| n == "vehshare.ytd") {
        shared.push("vehshare.ytd".to_owned());
    }
    // Interiors come from shared dictionaries named after the texture
    // (supergt_interior â†’ vehicles_supergt_interior.ytd).
    for d in drawables.iter().map(|(_, d)| d) {
        for t in d.shaders.iter().filter_map(|s| s.base_texture()) {
            let t = t.to_ascii_lowercase();
            let dict = format!("vehicles_{t}.ytd");
            if t.ends_with("_interior") && !shared.contains(&dict) {
                shared.push(dict);
            }
        }
    }
    shared
}

fn add_shared_textures(
    game_dir: &Path,
    keys: Option<Arc<GtaVKeys>>,
    f: &Gta5File,
    drawables: &[(String, Drawable)],
    external: &mut Vec<(String, TextureDictionary)>,
) {
    let dir = f.rpf.entry(f.id).parent.unwrap_or(rage_rpf::Rpf7::ROOT);
    for name in vehicle_shared_names(drawables, external) {
        let local = f
            .rpf
            .find_child(dir, &name)
            .and_then(|tid| f.rpf.read_resource(tid).ok())
            .and_then(|r| rage_formats::ytd::parse(&r).ok());
        if let Some(dict) = local.or_else(|| base_vehicle_textures(game_dir, keys.clone(), &name)) {
            external.push((name, dict));
        }
    }
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
                        b = usize::try_from(own.bones[k].parent)
                            .ok()
                            .filter(|&p| p < own.bones.len());
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
            b = usize::try_from(own.bones[k].parent)
                .ok()
                .filter(|&p| p < own.bones.len());
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
    matches!(
        rage_formats::shaders::name(hash),
        Some("ped_cloth" | "ped_cloth_enveff")
    )
}

/// Binds the cloth meshes of a ped to the body bones nearest to each vertex
/// (the two closest bone segments, by inverse squared distance), so they
/// follow the animation like clothes on the body instead of being torn apart.
fn bind_cloth(d: &mut Drawable, skeleton: &drawable::Skeleton) {
    let cloth: Vec<bool> = d
        .shaders
        .iter()
        .map(|s| is_cloth_shader(s.name_hash))
        .collect();
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
        let (indices, weights) = mesh
            .positions
            .iter()
            .map(|p| nearest_bones(&segments, *p))
            .unzip();
        mesh.bone_indices = indices;
        mesh.bone_weights = weights;
    }
}

/// Binds vertices held by the skeleton's root to the nearest body bones.
/// Cloth parts pin some vertices to the root for the cloth simulation; the
/// root stays at the feet, so without it those vertices are left behind as
/// long strips when the ped moves.
fn bind_pinned(d: &mut Drawable, skeleton: &drawable::Skeleton) {
    let roots: Vec<usize> = skeleton
        .bones
        .iter()
        .enumerate()
        .filter(|(_, b)| b.parent < 0)
        .map(|(i, _)| i)
        .collect();
    let mut segments = None;
    for mesh in &mut d.meshes {
        for i in 0..mesh
            .bone_indices
            .len()
            .min(mesh.bone_weights.len())
            .min(mesh.positions.len())
        {
            let (b, w) = (mesh.bone_indices[i], mesh.bone_weights[i]);
            let pinned: f32 = (0..4)
                .filter(|&k| roots.contains(&(b[k] as usize)))
                .map(|k| w[k])
                .sum();
            let total: f32 = w.iter().sum();
            if total > 0.0 && pinned / total > 0.5 {
                let segments = segments.get_or_insert_with(|| body_segments(skeleton));
                if segments.is_empty() {
                    return;
                }
                (mesh.bone_indices[i], mesh.bone_weights[i]) =
                    nearest_bones(segments, mesh.positions[i]);
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
            let child = skeleton
                .bones
                .iter()
                .position(|c| usize::try_from(c.parent).ok() == Some(i) && main(&c.name));
            (i as u16, pos(i), child.map_or(pos(i), pos))
        })
        .collect()
}

fn nearest_bones(segments: &[(u16, [f32; 3], [f32; 3])], p: [f32; 3]) -> ([u16; 4], [f32; 4]) {
    let distance = |(_, a, b): &(u16, [f32; 3], [f32; 3])| {
        let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let ap = [p[0] - a[0], p[1] - a[1], p[2] - a[2]];
        let len2 = ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2];
        let t = if len2 > 1e-9 {
            ((ap[0] * ab[0] + ap[1] * ab[1] + ap[2] * ab[2]) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
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
    let (w1, w2) = (
        w(best.1),
        if second.1 < f32::MAX {
            w(second.1)
        } else {
            0.0
        },
    );
    let total = w1 + w2;
    ([best.0, second.0, 0, 0], [w1 / total, w2 / total, 0.0, 0.0])
}

/// A texture dictionary from the base game's vehicle archive (shared by many vehicles).
fn base_vehicle_textures(
    game_dir: &Path,
    keys: Option<Arc<GtaVKeys>>,
    name: &str,
) -> Option<TextureDictionary> {
    let path = format!("x64e.rpf/levels/gta5/vehicles.rpf/{name}");
    let (Node::ArchiveFile(rpf, id), _) = rage_mods::resolve(game_dir, &path, keys).ok()? else {
        return None;
    };
    rage_formats::ytd::parse(&rpf.read_resource(id).ok()?).ok()
}

/// Like [`grid_layout`], but every drawable is scaled to the same size: for
/// particle models, where a spark sits next to a large hoop and real sizes
/// would leave most of them invisible.
fn thumbnail_layout(drawables: &[(String, Drawable)]) -> Vec<Mat4> {
    let columns = (drawables.len().min(SHOWN_AT_FIRST) as f32)
        .sqrt()
        .ceil()
        .max(1.0) as usize;
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
            let extent = (0..3)
                .map(|k| hi[k] - lo[k])
                .fold(0.0f32, f32::max)
                .max(1e-3);
            let s = 1.0 / extent;
            let centre = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, lo[2]];
            let (col, row) = (
                (i % SHOWN_AT_FIRST) % columns,
                (i % SHOWN_AT_FIRST) / columns,
            );
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
            if lo[0] > hi[0] {
                ([0.0; 3], [0.0; 3])
            } else {
                (lo, hi)
            }
        })
        .collect();
    let cell = bounds
        .iter()
        .map(|(lo, hi)| (hi[0] - lo[0]).max(hi[1] - lo[1]))
        .fold(0.0f32, f32::max)
        * 1.2
        + 0.1;
    let shown = drawables.len().min(SHOWN_AT_FIRST);
    let columns = (shown as f32).sqrt().ceil().max(1.0) as usize;
    bounds
        .iter()
        .enumerate()
        .map(|(i, (lo, hi))| {
            let (col, row) = (
                (i % SHOWN_AT_FIRST) % columns,
                (i % SHOWN_AT_FIRST) / columns,
            );
            let centre = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, lo[2]];
            let mut m = math::IDENTITY;
            m[12] = col as f32 * cell - centre[0];
            m[13] = -(row as f32) * cell - centre[1];
            m[14] = -centre[2];
            m
        })
        .collect()
}

/// Group of a ped component (`uppr_001_u` â†’ "Upper body"); other drawables none.
fn ped_group(name: &str) -> String {
    if !ped_component_names().values().any(|v| v == name) {
        return String::new();
    }
    let component = name.split('_').next().unwrap_or_default();
    let label = PED_GROUP_LABELS
        .iter()
        .find(|(c, _)| *c == component)
        .map_or(component, |(_, label)| *label);
    label.to_owned()
}

/// Readable names of the ped component kinds.
const PED_GROUP_LABELS: &[(&str, &str)] = &[
    ("head", "Head"),
    ("berd", "Beard / mask"),
    ("hair", "Hair"),
    ("uppr", "Upper body"),
    ("lowr", "Legs"),
    ("hand", "Hands"),
    ("feet", "Feet"),
    ("teef", "Teeth"),
    ("accs", "Accessories"),
    ("task", "Gear"),
    ("decl", "Decals"),
    ("jbib", "Jacket"),
];

/// Exports the model as shown by `look` as `.glb` (one node per part,
/// material colours as base colour factors, hidden materials left out).
pub fn to_glb(model: &LoadedModel, name: &str, look: &Look) -> Option<Vec<u8>> {
    let meshes: Vec<(&str, &Drawable)> = model
        .drawables
        .iter()
        .map(|(n, d)| (n.as_str(), d))
        .collect();
    let nodes: Vec<rage_formats::gltf::Node> = model
        .shown(&look.parts)
        .map(|(p, _)| rage_formats::gltf::Node {
            name: p.name.clone(),
            mesh: p.drawable,
            transform: p.transform,
        })
        .collect();
    if nodes.is_empty() {
        return None;
    }
    Some(rage_formats::gltf::write_glb_scene_with_skins(
        name,
        &meshes,
        &nodes,
        &|d, t| model.texture(&model.drawables[d].1, t),
        &|d, s| model.tint(look, d, s as u16),
        &|d| {
            model
                .skeleton
                .as_ref()
                .or(model.drawables[d].1.skeleton.as_ref())
        },
    ))
}

/// Writes several models, each placed one or more times, as one `.glb`
/// scene (a map file, a cutscene's surroundings). Positions are moved so the
/// scene's centre is at the origin; `center` says where that was. These map
/// objects intentionally remain rigid rather than exporting model skeletons.
pub fn scene_to_glb(
    name: &str,
    items: &[(&LoadedModel, &Look, &[Mat4])],
    center: [f32; 3],
) -> Option<Vec<u8>> {
    let mut meshes: Vec<(&str, &Drawable)> = Vec::new();
    // For every scene drawable: its model and its index there.
    let mut owner: Vec<(usize, usize)> = Vec::new();
    let mut nodes = Vec::new();
    let recentre = math::from_trs(
        [-center[0], -center[1], -center[2]],
        [0.0, 0.0, 0.0, 1.0],
        [1.0; 3],
    );
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
fn load_gta4(
    archive: &Arc<rage_rpf::Gta4Archive>,
    id: rage_rpf::Gta4EntryId,
) -> Result<LoadedModel> {
    use rage_formats::wdr;
    let name = archive.entry(id).name.clone();
    let lower = name.to_ascii_lowercase();
    let stem = name.split('.').next().unwrap_or(&name).to_owned();
    let res = archive.read_resource(id)?;
    if lower == "player.wft"
        && archive
            .find_child(rage_rpf::Gta4Archive::ROOT, "head_000_r.wdr")
            .is_some()
    {
        let skeleton = wdr::parse_wft(&res)?
            .drawable
            .skeleton
            .context("the player has no skeleton")?;
        return gta4_player(archive, skeleton);
    }
    let (mut drawables, parts) = gta4_parts(archive, &res, &name, &lower, &stem)?;
    let external = gta4_textures(archive, &drawables, &lower, &stem);
    let skeleton = gta4_skeleton(archive, &drawables, &stem);
    if let Some(sk) = &skeleton
        && lower.ends_with(".wdd")
    {
        fit_to_skeleton(&mut drawables, sk);
    }
    Ok(assembled_model((drawables, parts), external, skeleton))
}

fn gta4_textures(
    archive: &Arc<rage_rpf::Gta4Archive>,
    drawables: &[(String, Drawable)],
    lower: &str,
    stem: &str,
) -> Vec<(String, TextureDictionary)> {
    crate::progress::set("Loading textures", 0, 0);
    let used: Vec<String> = drawables
        .iter()
        .flat_map(|(_, d)| d.shaders.iter().flat_map(|s| s.textures.iter().cloned()))
        .collect();
    let vehicle =
        lower.ends_with(".wft") && archive.display_name().eq_ignore_ascii_case("vehicles.img");
    let collision = lower.ends_with(".wbn") || lower.ends_with(".wbd");
    if collision {
        Vec::new()
    } else {
        crate::gta4::model_textures(archive, stem, &used, vehicle)
    }
}

/// Peds: the skeleton of the fragment next to the dictionary.
fn gta4_skeleton(
    archive: &rage_rpf::Gta4Archive,
    drawables: &[(String, Drawable)],
    stem: &str,
) -> Option<drawable::Skeleton> {
    drawables
        .iter()
        .find_map(|(_, d)| d.skeleton.clone())
        .or_else(|| {
            let tid = archive.find_child(rage_rpf::Gta4Archive::ROOT, &format!("{stem}.wft"))?;
            rage_formats::wdr::parse_wft(&archive.read_resource(tid).ok()?)
                .ok()?
                .drawable
                .skeleton
        })
}

/// Builds the drawables and parts of a GTA IV model file by its kind.
fn gta4_parts(
    archive: &rage_rpf::Gta4Archive,
    res: &rage_resource::Resource,
    name: &str,
    lower: &str,
    stem: &str,
) -> Result<Assembled> {
    if lower.ends_with(".wdr") {
        return Ok((
            vec![(stem.to_owned(), rage_formats::wdr::parse_wdr(res)?)],
            vec![whole_part(stem)],
        ));
    }
    if lower.ends_with(".wft") {
        return gta4_fragment_parts(archive, res, stem);
    }
    if lower.ends_with(".wdd") {
        return gta4_dictionary_parts(res, stem);
    }
    if lower.ends_with(".wbn") || lower.ends_with(".wbd") {
        return gta4_bound_parts(archive, res, lower, stem);
    }
    bail!("{name} is not a GTA IV model (.wdr, .wdd, .wft) or collision file (.wbn, .wbd)");
}

fn gta4_fragment_parts(
    archive: &rage_rpf::Gta4Archive,
    res: &rage_resource::Resource,
    stem: &str,
) -> Result<Assembled> {
    use rage_formats::wdr;
    let (mut drawables, mut parts) = fragment_parts(wdr::parse_wft(res)?, stem);
    // The collision, hidden at first (switch it on under Parts).
    if let Some(at) = wdr::fragment_bound(res)
        && let Ok((collision, stats)) = rage_formats::wbn::parse_bound(res, at)
        && !collision.meshes.is_empty()
    {
        let bounds = vec![("collision".to_owned(), collision, stats)];
        register_gta4_materials(archive, &bounds);
        for (name, d, _) in bounds {
            parts.push(Part {
                name: name.clone(),
                group: "Collision".into(),
                drawable: drawables.len(),
                transform: math::IDENTITY,
                visible: false,
                bone: None,
            });
            drawables.push((name, d));
        }
    }
    Ok((drawables, parts))
}

fn gta4_dictionary_parts(res: &rage_resource::Resource, stem: &str) -> Result<Assembled> {
    let drawables: Vec<(String, Drawable)> = rage_formats::wdr::parse_wdd(res)?
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
    let layout = if is_ped_dictionary(&names) {
        vec![math::IDENTITY; drawables.len()]
    } else {
        grid_layout(&drawables)
    };
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
    Ok((drawables, parts))
}

fn gta4_bound_parts(
    archive: &rage_rpf::Gta4Archive,
    res: &rage_resource::Resource,
    lower: &str,
    stem: &str,
) -> Result<Assembled> {
    let bounds: Vec<(String, Drawable, rage_formats::ybn::Stats)> = if lower.ends_with(".wbn") {
        let (d, s) = rage_formats::wbn::parse_wbn(res)?;
        vec![(stem.to_owned(), d, s)]
    } else {
        // A map area's dictionary names its bounds after the models they belong to.
        let index =
            crate::gta4::game_folder(archive.path()).map(|g| crate::gta4::texture_index(&g));
        rage_formats::wbn::parse_wbd(res)?
            .into_iter()
            .map(|(hash, d, s)| {
                let label = index
                    .as_ref()
                    .and_then(|i| i.model_name(hash))
                    .unwrap_or_else(|| format!("{hash:08x}"));
                (label, d, s)
            })
            .collect()
    };
    register_gta4_materials(archive, &bounds);
    let drawables: Vec<(String, Drawable)> = bounds.into_iter().map(|(n, d, _)| (n, d)).collect();
    let layout = if drawables.len() > 1 {
        grid_layout(&drawables)
    } else {
        vec![math::IDENTITY]
    };
    let parts = drawables
        .iter()
        .enumerate()
        .map(|(i, (n, _))| Part {
            name: n.clone(),
            group: String::new(),
            drawable: i,
            transform: layout[i],
            visible: i < SHOWN_AT_FIRST,
            bone: None,
        })
        .collect();
    Ok((drawables, parts))
}

/// Component groups of GTA IV's player shown by default (suits, `suse`, are left out).
const GTA4_PLAYER_GROUPS: &[&str] = &["head", "hair", "uppr", "lowr", "hand", "feet", "teef"];

/// GTA IV's player (`playerped.rpf/player.wft`): the fragment holds only the
/// skeleton; the body is the first variant of every component next to it
/// (`head_000_r.wdr`, `uppr_000_u.wdr`, â€¦) with its first texture variant
/// (`head_diff_000_a_whi.wtd`).
fn gta4_player(
    archive: &rage_rpf::Gta4Archive,
    skeleton: drawable::Skeleton,
) -> Result<LoadedModel> {
    use rage_rpf::Gta4Archive;
    let files: Vec<Gta4File> = archive
        .children(Gta4Archive::ROOT)
        .map(|c| (archive.entry(c).name.to_ascii_lowercase(), c))
        .collect();
    let (mut drawables, mut parts, mut external) = (Vec::new(), Vec::new(), Vec::new());
    if let Some((name, id)) = first_gta4(&files, "player.", ".wtd")
        && let Some(dict) = gta4_textures_of(archive, *id)
    {
        external.push((name.clone(), dict));
    }
    for group in GTA4_PLAYER_GROUPS {
        let Some((stem, d, dict)) = gta4_player_component(archive, &files, group) else {
            continue;
        };
        external.extend(dict);
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
        bail!("no body parts (head_000_r.wdr, â€¦) next to player.wft");
    }
    fit_to_skeleton(&mut drawables, &skeleton);
    Ok(assembled_model(
        (drawables, parts),
        external,
        Some(skeleton),
    ))
}

type Gta4File = (String, rage_rpf::Gta4EntryId);

/// The first (by name) of the files that start with `prefix` and end with `ext`.
fn first_gta4<'a>(files: &'a [Gta4File], prefix: &str, ext: &str) -> Option<&'a Gta4File> {
    files
        .iter()
        .filter(|(n, _)| n.starts_with(prefix) && n.ends_with(ext))
        .min_by(|a, b| a.0.cmp(&b.0))
}

fn gta4_textures_of(
    archive: &rage_rpf::Gta4Archive,
    id: rage_rpf::Gta4EntryId,
) -> Option<TextureDictionary> {
    archive
        .read_resource(id)
        .ok()
        .and_then(|r| rage_formats::ytd::parse_gta4(&r).ok())
}

/// A component's first drawable (name and drawable) with its first texture variant, if any.
fn gta4_player_component(
    archive: &rage_rpf::Gta4Archive,
    files: &[Gta4File],
    group: &str,
) -> Option<PedComponent> {
    let (name, id) = first_gta4(files, &format!("{group}_"), ".wdr")?;
    let d = archive
        .read_resource(*id)
        .map_err(anyhow::Error::from)
        .and_then(|r| Ok(rage_formats::wdr::parse_wdr(&r)?))
        .ok()?;
    let stem = name.trim_end_matches(".wdr").to_owned();
    let variant = stem.split('_').nth(1).unwrap_or("000");
    let dict = first_gta4(files, &format!("{group}_diff_{variant}_a"), ".wtd")
        .and_then(|(tname, tid)| Some((tname.clone(), gta4_textures_of(archive, *tid)?)));
    Some((stem, d, dict))
}

/// Models out of a GTA IV drawable dictionary (`.wdd` at `vpath`): one
/// model per wanted (name, name hash), each textured on its own.
pub fn load_gta4_members(
    game: &Path,
    vpath: &str,
    wanted: &[(String, u32)],
) -> Result<Vec<(String, LoadedModel)>> {
    let Node::Gta4File(archive, id) = rage_rpf::vfs::resolve(game, vpath, None)? else {
        bail!("{vpath} is not a file inside a GTA IV archive")
    };
    let all = rage_formats::wdr::parse_wdd(&archive.read_resource(id)?)?;
    let mut out = Vec::new();
    for (name, hash) in wanted {
        let Some((_, d)) = all.iter().find(|(h, _)| h == hash) else {
            continue;
        };
        let used: Vec<String> = d
            .shaders
            .iter()
            .flat_map(|s| s.textures.iter().cloned())
            .collect();
        let mut external = crate::gta4::model_textures(&archive, name, &used, false);
        // Textures kept with the dictionary (`m_y_grus_lo_02_p.wtd` for its props).
        if external.is_empty() {
            let file = vpath.rsplit('/').next().unwrap_or(vpath);
            let stem = file.rsplit_once('.').map_or(file, |(s, _)| s);
            external = crate::gta4::model_textures(&archive, stem, &used, false);
        }
        let part = Part {
            name: name.clone(),
            group: String::new(),
            drawable: 0,
            transform: math::IDENTITY,
            visible: true,
            bone: None,
        };
        let mut model = LoadedModel {
            drawables: vec![(name.clone(), d.clone())],
            parts: vec![part],
            external,
            materials: Vec::new(),
            shared: Vec::new(),
            shader_material: Vec::new(),
            skeleton: d.skeleton.clone(),
            expressions: Vec::new(),
            ped: None,
        };
        model.collect_materials();
        out.push((name.clone(), model));
    }
    Ok(out)
}

fn load_rdr2(rpf: &Arc<rage_rpf::Rpf8>, i: usize) -> Result<LoadedModel> {
    let entries = rpf.entries().context("archive table not readable")?;
    let entry = &entries[i];
    let full = rpf.entry_name(i);
    let name = full.rsplit('/').next().unwrap_or(&full).to_owned();
    let lower = name.to_ascii_lowercase();
    let stem = name.split('.').next().unwrap_or(&name).to_owned();
    if is_rdr2_ped_file(&full, rpf.display_name()) {
        let game =
            crate::rdr2::game_folder(rpf.path()).context("not inside an RDR2 game folder")?;
        return load_rdr2_ped(&game, &stem, 0);
    }
    crate::progress::set("Reading the model", 0, 0);
    let res = rpf.read_resource(entry)?;
    let mut drawables = rdr2_drawables(&res, &name, &lower, &stem)?;
    // A ped's parts (".ydd" of a metaped) are bound to its base skeleton,
    // kept in a fragment of its own; they then stand on one spot.
    let base = (lower.ends_with(".ydd"))
        .then(|| rdr2_base_skeleton(rpf, &stem))
        .flatten();
    if let Some(sk) = &base {
        fit_to_skeleton(&mut drawables, sk);
    }
    let parts = rdr2_parts(&mut drawables, &res, &lower, &stem, base.is_some())?;
    let mut external = rdr2_folder_textures(rpf, entries, &full, &stem);
    // The rest from shared dictionaries elsewhere, through the texture index.
    if let Some(game) = crate::rdr2::game_folder(rpf.path()) {
        rdr2_shared_textures(&game, &drawables, &mut external, 32);
        // Palette shaders: colours from the palettes (vehicles: their first variation).
        crate::palette::apply(
            &crate::palette::carvariations(&game),
            &stem,
            &mut drawables,
            &mut external,
        );
    }
    let skeleton = base.or_else(|| drawables.iter().find_map(|(_, d)| d.skeleton.clone()));
    Ok(assembled_model((drawables, parts), external, skeleton))
}

/// The drawables of an RDR2 model file by its kind (a fragment has none of its own).
fn rdr2_drawables(
    res: &rage_resource::Resource,
    name: &str,
    lower: &str,
    stem: &str,
) -> Result<Vec<(String, Drawable)>> {
    Ok(if lower.ends_with(".ydr") {
        vec![(stem.to_owned(), drawable::parse_ydr_rdr2(res)?)]
    } else if lower.ends_with(".ydd") {
        drawable::parse_ydd_rdr2(res)?
            .into_iter()
            .enumerate()
            .map(|(k, (hash, d))| {
                let label = rage_formats::names::value(hash);
                let label = if hash == 0 || label.starts_with("hash_") {
                    format!("{stem}#{k}")
                } else {
                    label
                };
                (label, d)
            })
            .collect()
    } else if lower.ends_with(".yft") {
        Vec::new()
    } else {
        bail!(
            "{name}: only .ydr, .ydd and .yft models of Red Dead Redemption 2 can be shown so far"
        );
    })
}

/// The parts of an RDR2 model; a fragment replaces `drawables` with its own.
fn rdr2_parts(
    drawables: &mut Vec<(String, Drawable)>,
    res: &rage_resource::Resource,
    lower: &str,
    stem: &str,
    has_base: bool,
) -> Result<Vec<Part>> {
    if lower.ends_with(".yft") {
        // Fragments: the body and its children (wheels) on their bones.
        let (d, p) = fragment_parts(drawable::parse_fragment_rdr2(res)?, stem);
        *drawables = d;
        return Ok(p);
    }
    let layout = if drawables.len() > 1 && !has_base {
        grid_layout(drawables)
    } else {
        vec![math::IDENTITY; drawables.len()]
    };
    Ok(drawables
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
        .collect())
}

/// Texture dictionaries in the same folder: `<stem>.ytd` first, then those
/// whose name shares the model's prefix, then the rest; at most eight.
fn rdr2_folder_textures(
    rpf: &rage_rpf::Rpf8,
    entries: &[rage_rpf::rpf8::Entry],
    full: &str,
    stem: &str,
) -> Vec<(String, TextureDictionary)> {
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
    external
}

/// Adds the shared texture dictionaries the drawables' shaders need (at most
/// `limit`), found through the texture index.
fn rdr2_shared_textures(
    game: &Path,
    drawables: &[(String, Drawable)],
    external: &mut Vec<(String, TextureDictionary)>,
    limit: usize,
) {
    let Some(index) = crate::rdr2::texture_index(game) else {
        return;
    };
    let have = |n: &str, ext: &[(String, TextureDictionary)]| {
        drawables.iter().any(|(_, d)| {
            d.textures
                .as_ref()
                .is_some_and(|t| t.textures.iter().any(|x| x.name.eq_ignore_ascii_case(n)))
        }) || ext
            .iter()
            .any(|(_, t)| t.textures.iter().any(|x| x.name.eq_ignore_ascii_case(n)))
    };
    let wanted: Vec<String> = drawables
        .iter()
        .flat_map(|(_, d)| d.shaders.iter().flat_map(|s| s.textures.iter().cloned()))
        .collect();
    // Which shared dictionaries are needed (one per texture not found yet).
    let mut needed: Vec<(String, crate::rdr2::Location)> = Vec::new();
    for name in wanted {
        if needed.len() >= limit || have(&name, external) {
            continue;
        }
        let Some(loc) = index.find(&name) else {
            continue;
        };
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
    base.ends_with(".ymt")
        && base != "metapeds.ymt"
        && archive.starts_with("metapeds")
        && archive.ends_with(".rpf")
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
        crate::progress::set(
            "Indexing the peds' models (once)",
            done as u64,
            total as u64,
        )
    })
    .context("no RDR2 keys")?;
    let textures = crate::rdr2::texture_index(game);
    crate::progress::set("Reading the outfits", 0, 0);
    let outfits = metaped::load_outfits(game, &index, ped)?;
    if outfits.is_empty() {
        bail!("{ped} has no outfits");
    }
    let chosen = outfit.min(outfits.len() - 1);
    let (mut drawables, owners) =
        outfit_drawables(game, &index, textures.as_deref(), &outfits[chosen].assets);
    if drawables.is_empty() {
        bail!("none of the parts of {ped}'s outfit {chosen} were found");
    }
    let base = ped_skeleton(game, ped, &mut drawables, &outfits[chosen].expressions);
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
    tint_parts(
        game,
        &mut drawables,
        &owners,
        &mut external,
        textures.as_deref(),
    );
    let skeleton = base.or_else(|| drawables.iter().find_map(|(_, d)| d.skeleton.clone()));
    let mut model = assembled_model((drawables, parts), external, skeleton);
    model.ped = Some(PedOutfit {
        name: ped.to_owned(),
        outfit: chosen,
        outfits: outfits.len(),
    });
    Ok(model)
}

/// The drawables of an outfit's assets, each from its drawable dictionary
/// (merged dictionaries hold several), with the asset each comes from.
fn outfit_drawables<'a>(
    game: &Path,
    index: &crate::metaped::AssetIndex,
    textures: Option<&crate::rdr2::TextureIndex>,
    assets: &'a [crate::metaped::Asset],
) -> (Vec<(String, Drawable)>, Vec<&'a crate::metaped::Asset>) {
    let mut dictionaries: HashMap<crate::rdr2::Location, Vec<(u32, Drawable)>> = HashMap::new();
    let mut drawables: Vec<(String, Drawable)> = Vec::new();
    // The outfit asset each drawable comes from.
    let mut owners: Vec<&crate::metaped::Asset> = Vec::new();
    for (k, a) in assets.iter().enumerate() {
        crate::progress::set("Reading the parts", k as u64, assets.len() as u64);
        let Some(at) = index.drawable(a.drawable) else {
            continue;
        };
        let list = cached_dictionary(game, &mut dictionaries, &at.file);
        let Some((_, d)) = list.get(at.index as usize) else {
            continue;
        };
        let mut d = d.clone();
        let name_of = |hash: u32| textures.and_then(|t| t.name_of(hash)).map(str::to_owned);
        if let Some(albedo) = name_of(a.albedo) {
            replace_texture(&mut d, "_ab", &albedo);
        }
        if let Some(normal) = name_of(a.normal) {
            replace_texture(&mut d, "_nm", &normal);
        }
        drawables.push((outfit_part_label(a.drawable, at), d));
        owners.push(a);
    }
    (drawables, owners)
}

/// The drawables of a drawable dictionary, read once.
fn cached_dictionary<'a>(
    game: &Path,
    dictionaries: &'a mut HashMap<crate::rdr2::Location, Vec<(u32, Drawable)>>,
    file: &crate::rdr2::Location,
) -> &'a [(u32, Drawable)] {
    if !dictionaries.contains_key(file) {
        let read = crate::metaped::open_location(game, file)
            .and_then(|(rpf, i)| rpf.read_resource(rpf.entries()?.get(i)?).ok())
            .and_then(|res| drawable::parse_ydd_rdr2(&res).ok())
            .unwrap_or_default();
        dictionaries.insert(file.clone(), read);
    }
    dictionaries.get(file).map_or(&[], Vec::as_slice)
}

/// The part's name: that of its drawable, else `<file>#<index>`.
fn outfit_part_label(hash: u32, at: &crate::metaped::DrawableAt) -> String {
    let label = rage_formats::names::value(hash);
    if label.starts_with("hash_") {
        let file = at
            .file
            .chain
            .last()
            .map(|h| rage_formats::names::value(*h))
            .unwrap_or_default();
        format!("{file}#{}", at.index)
    } else {
        label
    }
}

/// The base skeleton of a ped, with its face rig, fitted to the drawables;
/// the outfit's values shape the drawables' faces and bodies.
fn ped_skeleton(
    game: &Path,
    ped: &str,
    drawables: &mut [(String, Drawable)],
    shapes: &[(u32, f32)],
) -> Option<drawable::Skeleton> {
    // The base skeleton: as the ped list names it, else guessed from the name.
    crate::progress::set("Reading the skeleton", 0, 0);
    let fragment = rdr2_ped_base(game, ped);
    let base = rdr2_ped_skeleton(game, ped);
    // Human heads name face bones the base skeletons lack: add them from
    // the one face rig the game has.
    let base = base.map(|mut sk| {
        if let Some(rig) = rdr2_face_rig(game) {
            let wanted = |t: u16| {
                drawables
                    .iter()
                    .any(|(_, d)| d.meshes.iter().any(|m| m.bone_tags.contains(&t)))
            };
            add_face_rig(&mut sk, &rig, wanted);
        }
        sk
    });
    if let Some(sk) = &base {
        fit_to_skeleton(drawables, sk);
        if let Some(frag) = &fragment
            && !shapes.is_empty()
        {
            crate::progress::set("Shaping the face and body", 0, 0);
            shape_rdr2_ped(game, frag, sk, drawables, shapes);
        }
    }
    base
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
    let Some(palettes) = crate::metaped::palette_dictionary(game) else {
        return;
    };
    let mut baked: Vec<Texture> = Vec::new();
    for ((_, d), a) in drawables.iter_mut().zip(owners) {
        let Some((name, palette)) = tint_source(a, index, &tinted, &palettes) else {
            continue;
        };
        let new_name = format!("{name}+{}_{}_{}", a.tints[0], a.tints[1], a.tints[2]);
        if !baked.iter().any(|t| t.name == new_name) {
            let Some(texture) = bake_tinted(d, external, name, palette, (&new_name, a.tints))
            else {
                continue;
            };
            baked.push(texture);
        }
        use_tinted(d, name, &new_name);
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

/// The colour texture's name and the palette of a part that is to be tinted.
fn tint_source<'i, 'p>(
    a: &crate::metaped::Asset,
    index: Option<&'i crate::rdr2::TextureIndex>,
    tinted: &crate::metaped::TintedAlbedos,
    palettes: &'p TextureDictionary,
) -> Option<(&'i str, &'p Texture)> {
    let name = index.and_then(|i| i.name_of(a.albedo));
    if a.palette == 0 || !tinted.is_tinted(a.albedo, name) {
        return None;
    }
    let palette = palettes
        .textures
        .iter()
        .find(|t| joaat(&t.name.to_ascii_lowercase()) == a.palette)?;
    Some((name?, palette))
}

/// The colour texture `name` of the drawable (or next to it) coloured with
/// `palette`'s rows; `(new_name, tints)` are the new texture's name and tints.
fn bake_tinted(
    d: &Drawable,
    external: &[(String, TextureDictionary)],
    name: &str,
    palette: &Texture,
    (new_name, tints): (&str, [u8; 3]),
) -> Option<Texture> {
    let find = |dict: &TextureDictionary| {
        dict.textures
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case(name))
            .cloned()
    };
    let albedo = d
        .textures
        .as_ref()
        .and_then(find)
        .or_else(|| external.iter().find_map(|(_, x)| find(x)))?;
    let (Some(rgba), Some(pal)) = (albedo.to_rgba(), palette.to_rgba()) else {
        return None;
    };
    let data = crate::tint::bake(
        &rgba,
        &pal,
        palette.width as usize,
        palette.height as usize,
        tints,
    );
    Some(Texture {
        name: new_name.to_owned(),
        width: albedo.width.max(1),
        height: albedo.height.max(1),
        depth: 1,
        levels: 1,
        format: rage_formats::texture::Format::R8G8B8A8_UNORM,
        dimension: albedo.dimension,
        usage: albedo.usage,
        data,
    })
}

/// Makes the drawable's shaders use the texture `new_name` instead of `name`.
fn use_tinted(d: &mut Drawable, name: &str, new_name: &str) {
    for shader in &mut d.shaders {
        for t in shader
            .textures
            .iter_mut()
            .filter(|t| t.eq_ignore_ascii_case(name))
        {
            *t = new_name.to_owned();
        }
        for (_, t) in shader
            .samplers
            .iter_mut()
            .filter(|(_, t)| t.eq_ignore_ascii_case(name))
        {
            *t = new_name.to_owned();
        }
    }
}

/// The face rig of human heads: the base skeletons lack the face bones that
/// heads are bound to and that face shapes move. Only one model in the game
/// carries them in its own skeleton (53 of the 93 bones a head names; the
/// others have no definition and follow the head).
const RDR2_FACE_RIG: &str =
    "packs_1.rpf/packs/base/models/metapeds_cutscene_a_f.rpf/cs_creoledoctor_ms1_merge_2.ydd";

/// The face rig skeleton (read once).
fn rdr2_face_rig(game: &Path) -> Option<drawable::Skeleton> {
    static CACHE: OnceLock<
        std::sync::Mutex<HashMap<std::path::PathBuf, Option<drawable::Skeleton>>>,
    > = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(r) = cache.lock().ok().and_then(|c| c.get(game).cloned()) {
        return r;
    }
    let rig = (|| {
        let (Node::Rpf8File(rpf, i), _) = rage_mods::resolve(game, RDR2_FACE_RIG, None).ok()?
        else {
            return None;
        };
        let res = rpf.read_resource(rpf.entries()?.get(i)?).ok()?;
        let skeletons = drawable::parse_ydd_rdr2(&res)
            .ok()?
            .into_iter()
            .filter_map(|(_, d)| d.skeleton);
        skeletons
            .filter(|s| s.find_name("FACIAL_C_FacialRoot").is_some())
            .max_by_key(|s| s.bones.len())
    })();
    if let Ok(mut c) = cache.lock() {
        c.insert(game.to_path_buf(), rig.clone());
    }
    rig
}

/// Adds the bones of a face rig the skeleton lacks (when the parts use one
/// of them): the rig's root hangs from the head, the rest keep their parents.
fn add_face_rig(
    skeleton: &mut drawable::Skeleton,
    rig: &drawable::Skeleton,
    used: impl Fn(u16) -> bool,
) {
    let Some(head) = skeleton.find_name("SKEL_Head") else {
        return;
    };
    if !rig
        .bones
        .iter()
        .any(|b| skeleton.find_tag(b.tag).is_none() && used(b.tag))
    {
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
        skeleton.bones.push(drawable::Bone {
            parent: parent as i16,
            ..b.clone()
        });
    }
}

/// Where RDR2 keeps the expression dictionaries (`<base skeleton>.yed`).
const RDR2_EXPRESSIONS: &str = "anim_0.rpf/anim/expressions.rpf";

/// The expressions of a base skeleton that shape a ped's face and body from
/// its outfit's values (face features, body build, muscles).
const RDR2_SHAPING: [&str; 3] = [
    "pack:/bcustom.expr",
    "pack:/meta_cust.expr",
    "pack:/muscles.expr",
];

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
    let expressions = shaping_expressions(game, fragment);
    if expressions.is_empty() {
        return 0;
    }
    let still = rage_formats::clip::Animation::empty();
    let binding = crate::anim::Binding::new(skeleton, &still).with_expressions(
        skeleton,
        &still,
        &expressions,
    );
    let controls: crate::expr::Controls = values
        .iter()
        .filter(|(id, _)| *id <= 0xFFFF)
        .map(|(id, v)| ((*id as u16, 64u8), [*v; 4]))
        .collect();
    let skin = binding.skin_with(skeleton, &still, 0.0, controls);
    for (_, d) in drawables.iter_mut() {
        for mesh in &mut d.meshes {
            skin_mesh(mesh, &skin);
        }
    }
    expressions.len()
}

/// The shaping expressions of the ped's base skeleton; none when it has no dictionary.
fn shaping_expressions(game: &Path, fragment: &str) -> Vec<rage_formats::expression::Expression> {
    let stem = fragment.trim_end_matches(".yft");
    let Ok((Node::Rpf8File(rpf, i), _)) =
        rage_mods::resolve(game, &format!("{RDR2_EXPRESSIONS}/{stem}.yed"), None)
    else {
        return Vec::new();
    };
    let Some(dict) = rpf
        .entries()
        .and_then(|e| e.get(i))
        .and_then(|e| rpf.read_resource(e).ok())
        .and_then(|res| rage_formats::expression::parse_rdr2(&res).ok())
    else {
        return Vec::new();
    };
    RDR2_SHAPING
        .iter()
        .filter_map(|n| {
            dict.expressions
                .iter()
                .find(|e| e.name.eq_ignore_ascii_case(n))
                .cloned()
        })
        .collect()
}

/// Bakes a pose into a mesh's vertices (and normals).
fn skin_mesh(mesh: &mut drawable::Mesh, skin: &[Mat4]) {
    if mesh.bone_indices.len() != mesh.positions.len()
        || mesh.bone_weights.len() != mesh.positions.len()
    {
        return;
    }
    for (k, p) in mesh.positions.iter_mut().enumerate() {
        *p = crate::anim::skin_point(skin, mesh.bone_indices[k], mesh.bone_weights[k], *p, false);
    }
    if mesh.normals.len() == mesh.bone_indices.len() {
        for (k, n) in mesh.normals.iter_mut().enumerate() {
            let v =
                crate::anim::skin_point(skin, mesh.bone_indices[k], mesh.bone_weights[k], *n, true);
            let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            *n = if len > 1e-6 {
                [v[0] / len, v[1] / len, v[2] / len]
            } else {
                *n
            };
        }
    }
}

/// The part of a texture name before its variation (`pant_mf1_061_c3_000_ab`
/// â†’ `pant_mf1_061`): everything before the first `_c<digit>` token.
fn texture_stem(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    let tokens: Vec<&str> = lower.split('_').collect();
    let end = tokens
        .iter()
        .position(|t| {
            t.len() >= 2 && t.starts_with('c') && t[1..].chars().all(|c| c.is_ascii_digit())
        })
        .unwrap_or(tokens.len());
    tokens[..end].join("_")
}

/// Puts `texture` in place of the shader textures ending in `suffix` (`_ab`,
/// `_nm`) that belong to the same piece (same name stem); when none does,
/// in place of all of them.
fn replace_texture(d: &mut Drawable, suffix: &str, texture: &str) {
    let stem = texture_stem(texture);
    let ends = |n: &str| n.to_ascii_lowercase().ends_with(suffix);
    let same = d.shaders.iter().any(|s| {
        s.textures
            .iter()
            .any(|t| ends(t) && texture_stem(t) == stem)
    });
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

/// The base skeleton of an RDR2 ped part: `a_c_cow_merge_4` â†’ `meta_base_cow`,
/// `a_c_bear_01_merge_2` â†’ `meta_base_bear_01` (the longest name that exists,
/// with or without the `a_c_`/`cs_` prefix).
pub(crate) fn rdr2_base_skeleton(rpf: &rage_rpf::Rpf8, stem: &str) -> Option<drawable::Skeleton> {
    rdr2_base_skeleton_in(&crate::rdr2::game_folder(rpf.path())?, stem)
}

/// The file of an RDR2 ped's base skeleton, as the ped list names it (`meta_base_male.yft`).
pub(crate) fn rdr2_ped_base(game: &Path, ped: &str) -> Option<String> {
    let peds = crate::metaped::peds(game).ok()?;
    peds.iter()
        .find(|p| p.name.eq_ignore_ascii_case(ped))
        .map(crate::metaped::base_fragment)
}

/// The skeleton an RDR2 ped is put together on: as the ped list names it,
/// else guessed from the ped's name.
pub(crate) fn rdr2_ped_skeleton(game: &Path, ped: &str) -> Option<drawable::Skeleton> {
    rdr2_ped_base(game, ped)
        .and_then(|frag| rdr2_skeleton_named(game, &frag))
        .or_else(|| rdr2_base_skeleton_in(game, ped))
}

/// The archive with RDR2's base skeletons (`metaped_fragments.rpf`).
pub(crate) fn rdr2_base_skeletons(game: &Path) -> Option<Arc<rage_rpf::Rpf8>> {
    match rage_mods::resolve(game, RDR2_BASE_SKELETONS, None).ok()?.0 {
        Node::Rpf8Dir(frags, _) => Some(frags),
        _ => None,
    }
}

/// A base skeleton by its file name (`meta_base_male.yft`).
pub(crate) fn rdr2_skeleton_named(game: &Path, file: &str) -> Option<drawable::Skeleton> {
    let (Node::Rpf8Dir(frags, _), _) = rage_mods::resolve(game, RDR2_BASE_SKELETONS, None).ok()?
    else {
        return None;
    };
    let entry = frags.entries()?.get(frags.find(file)?)?.clone();
    drawable::parse_fragment_rdr2(&frags.read_resource(&entry).ok()?)
        .ok()?
        .drawable
        .skeleton
}

fn rdr2_base_skeleton_in(game: &Path, stem: &str) -> Option<drawable::Skeleton> {
    let (Node::Rpf8Dir(frags, _), _) = rage_mods::resolve(game, RDR2_BASE_SKELETONS, None).ok()?
    else {
        return None;
    };
    let lower = stem.to_ascii_lowercase();
    let tokens: Vec<&str> = lower.split('_').collect();
    let starts = [0usize, 1, 2].into_iter().filter(|&s| s < tokens.len());
    for start in starts {
        for end in (start + 1..=tokens.len()).rev() {
            let name = format!("meta_base_{}.yft", tokens[start..end].join("_"));
            if let Some(skeleton) = rooted_skeleton(&frags, &name)? {
                return Some(skeleton);
            }
        }
    }
    None
}

/// The skeleton of the base fragment `name` if the archive has it and it has a root;
/// `None` when a found fragment cannot be read.
fn rooted_skeleton(frags: &rage_rpf::Rpf8, name: &str) -> Option<Option<drawable::Skeleton>> {
    let Some(i) = frags.find(name) else {
        return Some(None);
    };
    let entry = frags.entries()?.get(i)?.clone();
    let skeleton = drawable::parse_fragment_rdr2(&frags.read_resource(&entry).ok()?)
        .ok()?
        .drawable
        .skeleton;
    Some(skeleton.filter(|s| {
        s.bones
            .iter()
            .any(|b| b.name.eq_ignore_ascii_case("SKEL_ROOT"))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ped_textures() {
        assert_eq!(texture_stem("pant_mf1_061_c3_000_ab"), "pant_mf1_061");
        assert_eq!(
            texture_stem("Head_MR1_sc08_rough_c0_046_ab"),
            "head_mr1_sc08_rough"
        );
        assert_eq!(texture_stem("eyes_gen_0000_c0_999_ab"), "eyes_gen_0000");
        assert!(is_rdr2_ped_file(
            "cs_johnmarston.ymt",
            "packs_0.rpf/packs/base/data/metapeds/metapeds.rpf"
        ));
        assert!(is_rdr2_ped_file(
            "a_c_cow.ymt",
            "update_4.rpf/x/metapeds_val.rpf"
        ));
        assert!(!is_rdr2_ped_file(
            "metapeds.ymt",
            "packs_0.rpf/x/metapeds.rpf"
        ));
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
        let mut base = drawable::Skeleton {
            bones: vec![bone("SKEL_ROOT", 0, -1), bone("SKEL_Head", 31086, 0)],
        };
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
        assert_eq!(
            base.bones[2].parent, 1,
            "the rig's root hangs from the head"
        );
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
            bones: vec![
                bone("SKEL_ROOT", 0, -1),
                bone("SKEL_Pelvis", 11816, 0),
                bone("SKEL_Head", 31086, 1),
            ],
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
    fn rdr2_wheels_on_their_bones() {
        let bone = |name: &str, tag: u16, x: f32| drawable::Bone {
            name: name.into(),
            tag,
            parent: if tag == 0 { -1 } else { 0 },
            rotation: [0.0, 0.0, 0.0, 1.0],
            translation: [x, 0.0, 0.0],
            scale: [1.0; 3],
        };
        let names = [
            "chassis",
            "wheel_lf",
            "wheel_lf_fast",
            "wheel_rf",
            "wheel_rf_fast",
            "wheel_rf_mod",
        ];
        let bones = names.iter().enumerate().map(|(i, n)| {
            let x = match (i, n.contains("_r")) {
                (0, _) => 0.0,
                (_, true) => 1.0,
                _ => -1.0,
            };
            bone(n, i as u16, x)
        });
        let body = Drawable {
            skeleton: Some(drawable::Skeleton {
                bones: bones.collect(),
            }),
            ..Default::default()
        };
        let wheel = |tag| drawable::FragmentChild {
            bone_tag: tag,
            drawable: Drawable::default(),
        };
        let cloth = Drawable {
            meshes: vec![drawable::Mesh::default()],
            ..Default::default()
        };
        let fragment = drawable::Fragment {
            drawable: body,
            children: vec![wheel(1), wheel(2)],
            cloths: vec![cloth],
        };
        let (drawables, parts) = fragment_parts(fragment, "cart");
        assert_eq!(drawables.len(), 4);
        let shown: Vec<(&str, bool)> = parts.iter().map(|p| (p.name.as_str(), p.visible)).collect();
        // The cloth in place; the blurred wheel hidden; only the plain right wheel gets a copy.
        let expected = [
            ("cart", true),
            ("cloth", true),
            ("wheel_lf", true),
            ("wheel_lf_fast", false),
            ("wheel_rf", true),
        ];
        assert_eq!(shown, expected);
        assert_eq!(
            (parts[1].group.as_str(), parts[1].transform),
            ("Cloth", math::IDENTITY)
        );
        assert_eq!(parts[4].drawable, 2);
        assert_eq!(parts[4].transform[12], 1.0);
    }

    #[test]
    fn cloth_follows_the_nearest_bones() {
        // A thigh going down from the pelvis, and the spine going up.
        let segments = [
            (1u16, [0.0, 0.0, 0.0], [0.0, 0.0, -0.4]),
            (2, [0.0, 0.0, 0.0], [0.0, 0.0, 0.5]),
        ];
        let (bones, weights) = nearest_bones(&segments, [0.1, 0.0, -0.3]);
        assert_eq!(bones[0], 1);
        assert!(weights[0] > weights[1] && (weights.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        let (bones, _) = nearest_bones(&segments, [0.1, 0.0, 0.4]);
        assert_eq!(bones[0], 2);
    }
}

