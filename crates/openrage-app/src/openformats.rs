//! OpenIV's openFormats for game files: exporting drawables, fragments,
//! drawable dictionaries and texture dictionaries as text files with DDS
//! textures, and importing them back onto the game file they came from.
//!
//! Importing fits the files onto the original (the template), like
//! replacing a model with an OBJ: the model keeps its shaders, skeleton,
//! physics and parts, and gets the meshes and detail levels of the files
//! (written by [`ydr_write`]) and their embedded textures. Texture
//! dictionaries get the textures of the `.otd`. What the writer cannot
//! change (shader parameters, the skeleton) is reported as a note when the
//! files differ from the game file there.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use rage_formats::drawable::{Bone, Skeleton};
use rage_formats::math;
use rage_formats::openformats::{self as of, ExportedFile, OfDrawable, OfGeometry, Source, component};
use rage_formats::texture::Texture;
use rage_formats::ydr_write::{self, NewMesh};
use rage_resource::{Resource, ResourceError};

/// The kinds of game files with an openFormats counterpart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `.ydr` ↔ `.odr`
    Drawable,
    /// `.yft` ↔ `.oft`
    Fragment,
    /// `.ydd` ↔ `.odd`
    Dictionary,
    /// `.ytd` ↔ `.otd`
    Textures,
}

impl Kind {
    /// The kind of a game file, by its name.
    pub fn of_game_file(name: &str) -> Option<Self> {
        let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
        Some(match ext.as_str() {
            "ydr" => Self::Drawable,
            "yft" => Self::Fragment,
            "ydd" => Self::Dictionary,
            "ytd" => Self::Textures,
            _ => return None,
        })
    }

    /// The extension of the main openFormats file.
    pub fn open_ext(self) -> &'static str {
        match self {
            Self::Drawable => "odr",
            Self::Fragment => "oft",
            Self::Dictionary => "odd",
            Self::Textures => "otd",
        }
    }

    /// Whether openFormats files can be imported onto files of this kind.
    pub fn can_import(self) -> bool {
        self != Self::Dictionary
    }
}

fn stem(name: &str) -> &str {
    let name = name.rsplit(['/', '\\']).next().unwrap_or(name);
    name.rsplit_once('.').map_or(name, |(s, _)| s)
}

/// The openFormats files of a game file (`name`: its file name).
pub fn export(res: &Resource, name: &str) -> Result<Vec<ExportedFile>> {
    let kind = Kind::of_game_file(name).with_context(|| format!("{name} has no openFormats counterpart"))?;
    let stem = stem(name);
    Ok(match kind {
        Kind::Drawable => of::export_odr(&of::read_ydr(res, stem)?),
        Kind::Fragment => of::export_oft(&of::read_yft(res, stem)?),
        Kind::Dictionary => of::export_odd(stem, &of::read_ydd(res)?),
        Kind::Textures => of::export_otd(stem, &of::read_ytd(res)?),
    })
}

/// Writes exported files into `dir`; returns the path of the main file
/// (the last one).
pub fn write_files(dir: &Path, files: &[ExportedFile]) -> Result<PathBuf> {
    let mut main = None;
    for f in files {
        let path = dir.join(&f.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(&path, &f.data).with_context(|| format!("writing {}", path.display()))?;
        main = Some(path);
    }
    main.context("nothing to export")
}

/// A game file made from openFormats files.
#[derive(Debug)]
pub struct Imported {
    /// The new game file (RSC7); `None` when the files are the game file's.
    pub file: Option<Vec<u8>>,
    /// What was taken over and what was kept, for people.
    pub notes: Vec<String>,
}

/// Options for [`import_with`].
#[derive(Debug, Clone, Copy, Default)]
pub struct ImportOptions {
    /// Write the meshes anew even when they are the game file's.
    pub always_rebuild_meshes: bool,
}

/// Imports the openFormats file at `main` onto the game file `template`
/// (read from an entry with these page flags; `name` its file name).
pub fn import(template: &Resource, flags: (u32, u32), name: &str, main: &Path) -> Result<Imported> {
    let dir = main.parent().unwrap_or(Path::new(".")).to_path_buf();
    let src = move |p: &str| -> Result<Vec<u8>, ResourceError> {
        let path = dir.join(p);
        std::fs::read(&path).map_err(|e| ResourceError::Format(format!("{}: {e}", path.display())))
    };
    let file = main.file_name().context("no file name")?.to_string_lossy().into_owned();
    import_with(template, flags, name, &src, &file, ImportOptions::default())
}

/// Like [`import`], reading the files through `src` (`main`: the path of the
/// main file for it).
pub fn import_with(
    template: &Resource,
    flags: (u32, u32),
    name: &str,
    src: Source,
    main: &str,
    options: ImportOptions,
) -> Result<Imported> {
    let kind = Kind::of_game_file(name).with_context(|| format!("{name} has no openFormats counterpart"))?;
    let ext = main.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    if ext != kind.open_ext() {
        bail!("a .{} file goes onto a .{}, not onto {name}", ext, game_ext(&ext).unwrap_or("?"));
    }
    match kind {
        Kind::Drawable => {
            let d = of::parse_odr(src, main)?;
            import_drawable(template, of::read_ydr(template, stem(name))?, &d, options, Vec::new())
        }
        Kind::Fragment => {
            let f = of::parse_oft(src, main)?;
            let t = of::read_yft(template, stem(name))?;
            let mut notes = Vec::new();
            if !t.children.is_empty() || !f.children.is_empty() {
                notes.push(format!(
                    "the fragment's {} parts (wheels, …) and its physics are kept from the game file",
                    t.children.len()
                ));
            }
            import_drawable(template, t.drawable, &f.drawable, options, notes)
        }
        Kind::Textures => import_textures(template, flags, &of::parse_otd(src, main)?),
        Kind::Dictionary => bail!("drawable dictionaries (.ydd) cannot be imported yet, only exported"),
    }
}

/// The game file extension an openFormats extension belongs to.
pub fn game_ext(open_ext: &str) -> Option<&'static str> {
    Some(match open_ext.to_ascii_lowercase().as_str() {
        "odr" => "ydr",
        "oft" => "yft",
        "odd" => "ydd",
        "otd" => "ytd",
        _ => return None,
    })
}

fn import_textures(template: &Resource, flags: (u32, u32), textures: &[of::OfTexture]) -> Result<Imported> {
    use rage_formats::ytd_write::{Edit, apply};
    let dict = rage_formats::ytd::parse(template)?;
    let mut edits = Vec::new();
    let mut notes = Vec::new();
    for t in textures {
        let mut t = t.texture.clone();
        match dict.textures.iter().find(|o| of::same_texture_name(&o.name, &t.name)) {
            Some(old) if same_texture(old, &t) => continue,
            Some(old) => t.name = old.name.clone(),
            None => notes.push(format!("added {}", t.name)),
        }
        edits.push(Edit::Set(t));
    }
    for old in &dict.textures {
        if !textures.iter().any(|t| of::same_texture_name(&old.name, &t.texture.name)) {
            notes.push(format!("removed {}", old.name));
            edits.push(Edit::Remove(old.name.clone()));
        }
    }
    if edits.is_empty() {
        return Ok(Imported { file: None, notes: vec!["the textures are the game file's".into()] });
    }
    let changed = edits.len();
    let r = apply(template, flags.0, flags.1, &dict, &edits)?;
    notes.insert(0, format!("{changed} texture change(s)"));
    Ok(Imported { file: Some(r.file), notes })
}

fn same_texture(a: &Texture, b: &Texture) -> bool {
    (a.width, a.height, a.depth, a.levels, a.format, a.dimension)
        == (b.width, b.height, b.depth, b.levels, b.format, b.dimension)
        && a.data == b.data
}

fn skeleton(s: &of::OfSkeleton) -> Skeleton {
    Skeleton {
        bones: s
            .bones
            .iter()
            .map(|b| Bone {
                name: b.name.clone(),
                tag: b.tag,
                parent: b.parent,
                translation: b.translation,
                rotation: b.rotation,
                scale: b.scale,
            })
            .collect(),
    }
}

/// Whether two values read from text and from the game are the same (the
/// text keeps seven significant digits).
fn close(a: f32, b: f32) -> bool {
    a == b || (a - b).abs() <= 1e-5 + 1e-5 * a.abs().max(b.abs()) || (a.is_nan() && b.is_nan())
}

fn same_geometry(a: &OfGeometry, b: &OfGeometry) -> bool {
    a.shader == b.shader
        && a.decl == b.decl
        && a.indices == b.indices
        && a.attributes.len() == b.attributes.len()
        && a.attributes.iter().zip(&b.attributes).all(|((ka, va), (kb, vb))| {
            ka == kb && va.len() == vb.len() && va.iter().flatten().zip(vb.iter().flatten()).all(|(x, y)| close(*x, *y))
        })
}

/// Whether the files' detail levels are the game file's.
fn same_meshes(a: &OfDrawable, b: &OfDrawable) -> bool {
    a.lods.iter().zip(&b.lods).all(|(la, lb)| {
        la.models.len() == lb.models.len()
            && la.models.iter().zip(&lb.models).all(|(ma, mb)| {
                ma.bone == mb.bone
                    && ma.geometries.len() == mb.geometries.len()
                    && ma.geometries.iter().zip(&mb.geometries).all(|(x, y)| same_geometry(x, y))
            })
    })
}

/// The game file's shader `s` (the files' shader `i`): the same index when the names agree.
fn shader_index(tpl: &OfDrawable, i: usize, s: &of::OfShader) -> Result<usize> {
    match tpl.shaders.get(i) {
        Some(t) if t.name.eq_ignore_ascii_case(&s.name) => Ok(i),
        _ => tpl.shaders.iter().position(|t| t.name.eq_ignore_ascii_case(&s.name)).with_context(|| {
            format!("shader {i} ({}.sps) is not one of the model's shaders; its shaders cannot be changed", s.name)
        }),
    }
}

fn same_param_value(a: &of::OfValue, b: &of::OfValue) -> bool {
    match (a, b) {
        (of::OfValue::Texture(a), of::OfValue::Texture(b)) => a.eq_ignore_ascii_case(b),
        (of::OfValue::Floats(a), of::OfValue::Floats(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| close(*x, *y))
        }
        _ => false,
    }
}

fn shader_params_differ(s: &of::OfShader, t: &of::OfShader) -> bool {
    s.params.len() != t.params.len()
        || s.params
            .iter()
            .any(|(n, v)| !t.params.iter().any(|(tn, tv)| tn.eq_ignore_ascii_case(n) && same_param_value(v, tv)))
}

/// Shaders: the files' shader i is the game file's shader of that name,
/// the same index when the names agree.
fn map_shaders(tpl: &OfDrawable, d: &OfDrawable, notes: &mut Vec<String>) -> Result<Vec<u16>> {
    let mut shader_map = Vec::new();
    for (i, s) in d.shaders.iter().enumerate() {
        let index = shader_index(tpl, i, s)?;
        if shader_params_differ(s, &tpl.shaders[index]) {
            notes.push(format!("shader {i} ({}): changed parameters are not imported, the model's stay", s.name));
        }
        shader_map.push(index as u16);
    }
    Ok(shader_map)
}

/// A bone of the files' skeleton → the game file's (by tag, else name).
fn map_bone(file_skel: Option<&of::OfSkeleton>, tpl_skel: Option<&of::OfSkeleton>, i: usize) -> Option<usize> {
    let (Some(fs), Some(ts)) = (file_skel, tpl_skel) else { return Some(i) };
    let b = fs.bones.get(i)?;
    ts.bones
        .iter()
        .position(|t| t.tag == b.tag && t.name.eq_ignore_ascii_case(&b.name))
        .or_else(|| ts.bones.iter().position(|t| t.tag == b.tag))
        .or_else(|| ts.bones.iter().position(|t| t.name.eq_ignore_ascii_case(&b.name)))
}

/// Embedded textures: the changed and new ones, renamed to the game file's spelling.
fn changed_textures(tpl: &OfDrawable, d: &OfDrawable, notes: &mut Vec<String>) -> Vec<Texture> {
    let mut changed = Vec::new();
    for t in &d.textures {
        let mut t = t.texture.clone();
        match tpl.textures.iter().find(|o| of::same_texture_name(&o.texture.name, &t.name)) {
            Some(o) if same_texture(&o.texture, &t) => continue,
            Some(o) => t.name = o.texture.name.clone(),
            None => notes.push(format!("embedded texture {} added (no shader of the model uses it yet)", t.name)),
        }
        changed.push(t);
    }
    for o in &tpl.textures {
        if !d.textures.iter().any(|t| of::same_texture_name(&o.texture.name, &t.texture.name)) {
            notes.push(format!("embedded texture {} is kept (shaders use textures by name)", o.texture.name));
        }
    }
    changed
}

fn note_skeleton_kept(file_skel: Option<&of::OfSkeleton>, tpl_skel: Option<&of::OfSkeleton>, notes: &mut Vec<String>) {
    if let (Some(a), Some(b)) = (file_skel, tpl_skel)
        && (a.bones.len() != b.bones.len()
            || a.bones.iter().zip(&b.bones).any(|(x, y)| x.tag != y.tag || x.name != y.name))
    {
        notes.push(format!("the skeleton is kept from the game file ({} bones)", b.bones.len()));
    }
}

/// The embedded textures `changed` put into the rebuilt file (else the template).
fn replace_textures(
    template: &Resource,
    file: Option<&[u8]>,
    changed: &[Texture],
    notes: &mut Vec<String>,
) -> Result<Vec<u8>> {
    let base = match file {
        Some(f) => Resource::from_rsc7(f)?,
        None => template.clone(),
    };
    notes.push(format!(
        "embedded textures replaced: {}",
        changed.iter().map(|t| t.name.as_str()).collect::<Vec<_>>().join(", ")
    ));
    Ok(ydr_write::replace_embedded_textures(&base, changed)?)
}

fn import_drawable(
    template: &Resource,
    tpl: OfDrawable,
    d: &OfDrawable,
    options: ImportOptions,
    mut notes: Vec<String>,
) -> Result<Imported> {
    let shader_map = map_shaders(&tpl, d, &mut notes)?;
    let file_skel = d.skeleton.as_ref();
    let tpl_skel = tpl.skeleton.as_ref();
    note_skeleton_kept(file_skel, tpl_skel, &mut notes);
    let bone_map = |i: usize| map_bone(file_skel, tpl_skel, i);

    let mut file = None;
    if options.always_rebuild_meshes || !same_meshes(d, &tpl) {
        file = Some(write_meshes(template, &tpl, d, &shader_map, &bone_map, &mut notes)?);
    } else {
        notes.push("the meshes are the game file's; they stay as they are".into());
    }

    let changed = changed_textures(&tpl, d, &mut notes);
    if !changed.is_empty() {
        file = Some(replace_textures(template, file.as_deref(), &changed, &mut notes)?);
    }
    if file.is_none() {
        notes.push("nothing differs from the game file".into());
    }
    Ok(Imported { file, notes })
}

/// The files' detail levels written onto the template.
fn write_meshes(
    template: &Resource,
    tpl: &OfDrawable,
    d: &OfDrawable,
    shader_map: &[u16],
    bone_map: &dyn Fn(usize) -> Option<usize>,
    notes: &mut Vec<String>,
) -> Result<Vec<u8>> {
    let skinned_template = ydr_write::skinned_bones(template)?.is_some();
    let world = tpl.skeleton.as_ref().map(|s| skeleton(s).world_transforms()).unwrap_or_default();
    let mut levels: Vec<Vec<NewMesh>> = Vec::new();
    for (k, lod) in d.lods.iter().enumerate() {
        if lod.models.is_empty() {
            continue;
        }
        // The bone the game file's model of this level is attached to: the
        // written model is a copy of it.
        let target = tpl.lods.get(k).and_then(|l| l.models.first()).or_else(|| tpl.lods[0].models.first());
        let target_bone = target.map_or(0, |m| m.bone as usize);
        let mut meshes = Vec::new();
        for m in &lod.models {
            let bone = bone_map(m.bone as usize)
                .with_context(|| format!("model on bone {}, which the game file's skeleton does not have", m.bone))?;
            // Fixed models: from their bone's space to the target bone's. A
            // model on bone 0 is in model space (as the game's models are read).
            let space = |b: usize| if b == 0 { Some(&math::IDENTITY) } else { world.get(b) };
            let to_target = (!skinned_template && bone != target_bone)
                .then(|| {
                    let (a, b) = (space(bone)?, space(target_bone)?);
                    Some(math::mul(&math::inverse_affine(b), a))
                })
                .flatten();
            for g in &m.geometries {
                let shader = *shader_map
                    .get(g.shader as usize)
                    .with_context(|| format!("a geometry uses shader {}, which the files do not have", g.shader))?;
                let xyz = |c: usize| -> Vec<[f32; 3]> {
                    g.attribute(c).map(|v| v.iter().map(|p| [p[0], p[1], p[2]]).collect()).unwrap_or_default()
                };
                let mut mesh = NewMesh {
                    shader,
                    positions: xyz(component::POSITION),
                    normals: xyz(component::NORMAL),
                    uvs: g
                        .attribute(component::TEXCOORD0)
                        .map(|v| v.iter().map(|t| [t[0], t[1]]).collect())
                        .unwrap_or_default(),
                    colors: g
                        .attribute(component::COLOUR0)
                        .map(|v| v.iter().map(|c| c.map(|x| x / 255.0)).collect())
                        .unwrap_or_default(),
                    indices: g.indices.clone(),
                    bones: Vec::new(),
                    blend: Vec::new(),
                };
                if let Some(t) = &to_target {
                    mesh.positions.iter_mut().for_each(|p| *p = math::transform_point(t, *p));
                    mesh.normals.iter_mut().for_each(|n| *n = math::transform_dir(t, *n));
                }
                if skinned_template {
                    mesh.bones = match (g.attribute(component::BLEND_INDICES), g.attribute(component::BLEND_WEIGHTS)) {
                        (Some(idx), Some(w)) => idx
                            .iter()
                            .zip(w)
                            .map(|(i, w)| {
                                let k = (0..4).max_by(|&a, &b| w[a].total_cmp(&w[b])).unwrap_or(0);
                                bone_map(i[k] as usize).map(|b| b as u16).with_context(|| {
                                    format!(
                                        "a vertex moves with bone {}, which the game file's skeleton does not have",
                                        i[k]
                                    )
                                })
                            })
                            .collect::<Result<_>>()?,
                        _ => vec![bone as u16; mesh.positions.len()],
                    };
                    // All four bones with their weights (peds, clothing).
                    if let (Some(idx), Some(w)) =
                        (g.attribute(component::BLEND_INDICES), g.attribute(component::BLEND_WEIGHTS))
                    {
                        mesh.blend = idx
                            .iter()
                            .zip(w)
                            .map(|(i, w)| {
                                let mut b = ydr_write::Blend::default();
                                for k in 0..4 {
                                    if w[k] > 0.0 {
                                        b.bones[k] = bone_map(i[k] as usize).unwrap_or(0) as u16;
                                        b.weights[k] = w[k];
                                    }
                                }
                                let sum: f32 = b.weights.iter().sum();
                                if sum > 0.0 {
                                    b.weights.iter_mut().for_each(|x| *x /= sum);
                                }
                                b
                            })
                            .collect();
                    }
                }
                meshes.extend(crate::obj::split(mesh));
            }
        }
        levels.push(meshes);
    }
    if levels.is_empty() {
        bail!("the files have no meshes");
    }
    if levels.len() < d.lods.iter().rposition(|l| !l.models.is_empty()).map_or(0, |i| i + 1) {
        notes.push("empty detail levels in between were left out".into());
    }
    let refs: Vec<&[NewMesh]> = levels.iter().map(Vec::as_slice).collect();
    let file = ydr_write::replace_lods(template, &refs, None, None)?;
    let vertices: usize = levels.iter().flatten().map(|m| m.positions.len()).sum();
    notes.push(format!("meshes written: {} detail level(s), {vertices} vertices", levels.len()));
    Ok(file)
}
