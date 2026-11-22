//! Reading Wavefront OBJ models for import, and fitting them to a drawable's
//! shaders.
//!
//! OBJ is usually Y-up with texture coordinates growing upwards; the game is
//! Z-up with DirectX coordinates, so `(x, y, z) → (x, −z, y)` and `v → 1 − v`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use rage_crypto::GtaVKeys;
use rage_formats::texture::Texture;
use rage_formats::ydr_write::{Blend, MAX_VERTICES, NewMaterial, NewMesh, Place, ShaderInfo};
use rage_resource::Resource;
use rage_rpf::vfs::Node;

/// Triangles of one material.
#[derive(Debug, Clone, Default)]
pub struct ObjMesh {
    pub material: String,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    /// The objects or groups (`o`, `g`) the triangles came from, and per
    /// vertex the index of its one in this list.
    pub part_names: Vec<String>,
    pub parts: Vec<u16>,
}

/// Reads an OBJ file: `v`, `vt`, `vn`, `f` (polygons fanned into triangles,
/// negative indices from the end), `usemtl`, and `o`/`g` (the part each
/// vertex belongs to). One mesh per material, in the order they first appear.
pub fn read(text: &str) -> Result<Vec<ObjMesh>> {
    let (mut v, mut vt, mut vn) = (Vec::<[f32; 3]>::new(), Vec::<[f32; 2]>::new(), Vec::<[f32; 3]>::new());
    let mut meshes: Vec<ObjMesh> = Vec::new();
    let mut seen: Vec<HashMap<(usize, usize, usize, usize), u32>> = Vec::new();
    // Part names, and the current one (index into `names`).
    let mut names: Vec<String> = vec![String::new()];
    let mut part = 0usize;
    let mut current = 0usize;
    let mut has_uv = false;
    let mut has_normals = false;
    let num = |s: Option<&str>, line: usize| -> Result<f32> {
        match s.map(str::parse::<f32>) {
            Some(Ok(x)) => Ok(x),
            _ => bail!("line {line}: number missing or invalid"),
        }
    };
    let index = |s: &str, len: usize, line: usize| -> Result<usize> {
        let k: i64 = s.parse().map_err(|_| anyhow::anyhow!("line {line}: bad index {s:?}"))?;
        let k = if k < 0 { len as i64 + k } else { k - 1 };
        if k < 0 || k as usize >= len {
            bail!("line {line}: index {s} out of range");
        }
        Ok(k as usize)
    };
    let mesh_for = |name: &str, meshes: &mut Vec<ObjMesh>, seen: &mut Vec<HashMap<_, u32>>| -> usize {
        match meshes.iter().position(|m| m.material == name) {
            Some(i) => i,
            None => {
                meshes.push(ObjMesh { material: name.to_owned(), ..Default::default() });
                seen.push(HashMap::new());
                meshes.len() - 1
            }
        }
    };
    let mut material = String::from("default");
    let mut started = false;
    for (n, raw) in text.lines().enumerate() {
        let line = n + 1;
        let mut it = raw.split_whitespace();
        match it.next() {
            Some("v") => {
                let (x, y, z) = (num(it.next(), line)?, num(it.next(), line)?, num(it.next(), line)?);
                v.push([x, -z, y]);
            }
            Some("vt") => {
                let u = num(it.next(), line)?;
                let t = it.next().map_or(Ok(0.0), |s| num(Some(s), line))?;
                vt.push([u, 1.0 - t]);
            }
            Some("vn") => {
                let (x, y, z) = (num(it.next(), line)?, num(it.next(), line)?, num(it.next(), line)?);
                vn.push([x, -z, y]);
            }
            Some("o" | "g") => {
                let name = it.collect::<Vec<_>>().join(" ");
                part = names.iter().position(|n| *n == name).unwrap_or_else(|| {
                    names.push(name);
                    names.len() - 1
                });
            }
            Some("usemtl") => {
                material = it.collect::<Vec<_>>().join(" ");
                started = false;
            }
            Some("f") => {
                if !started {
                    current = mesh_for(&material, &mut meshes, &mut seen);
                    started = true;
                }
                let mut corners = Vec::new();
                for corner in it {
                    let mut f = corner.split('/');
                    let p = index(f.next().unwrap_or(""), v.len(), line)?;
                    let t = match f.next() {
                        Some(s) if !s.is_empty() => {
                            has_uv = true;
                            index(s, vt.len(), line)? + 1
                        }
                        _ => 0,
                    };
                    let nn = match f.next() {
                        Some(s) if !s.is_empty() => {
                            has_normals = true;
                            index(s, vn.len(), line)? + 1
                        }
                        _ => 0,
                    };
                    let mesh = &mut meshes[current];
                    let key = (p, t, nn, part);
                    let i = match seen[current].get(&key) {
                        Some(&i) => i,
                        None => {
                            let i = mesh.positions.len() as u32;
                            let k = match mesh.part_names.iter().position(|n| *n == names[part]) {
                                Some(k) => k,
                                None => {
                                    mesh.part_names.push(names[part].clone());
                                    mesh.part_names.len() - 1
                                }
                            };
                            mesh.parts.push(k as u16);
                            mesh.positions.push(v[p]);
                            mesh.uvs.push(if t > 0 { vt[t - 1] } else { [0.0, 0.0] });
                            mesh.normals.push(if nn > 0 { vn[nn - 1] } else { [0.0, 0.0, 0.0] });
                            seen[current].insert(key, i);
                            i
                        }
                    };
                    corners.push(i);
                }
                if corners.len() < 3 {
                    bail!("line {line}: a face needs at least three corners");
                }
                for k in 1..corners.len() - 1 {
                    meshes[current].indices.extend([corners[0], corners[k], corners[k + 1]]);
                }
            }
            _ => {}
        }
    }
    meshes.retain(|m| !m.indices.is_empty());
    if meshes.is_empty() {
        bail!("the OBJ file has no faces");
    }
    for m in &mut meshes {
        if !has_uv {
            m.uvs.clear();
        }
        // Normals only when every corner had one; otherwise they are computed.
        if !has_normals || m.normals.contains(&[0.0; 3]) {
            m.normals.clear();
        }
    }
    Ok(meshes)
}

/// What a `.mtl` says about a material: its colour texture (`map_Kd`) and
/// colour (`Kd`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MtlMaterial {
    pub texture: Option<PathBuf>,
    pub colour: Option<[f32; 3]>,
}

/// The materials of the `.mtl` files an OBJ names (`mtllib`), by name;
/// texture paths relative to the `.mtl` files.
pub fn materials(obj_text: &str, obj_dir: &Path) -> HashMap<String, MtlMaterial> {
    let mut out: HashMap<String, MtlMaterial> = HashMap::new();
    for line in obj_text.lines() {
        let Some(lib) = line.trim().strip_prefix("mtllib ") else { continue };
        let mtl = obj_dir.join(lib.trim());
        let Ok(text) = std::fs::read_to_string(&mtl) else { continue };
        let dir = mtl.parent().unwrap_or(obj_dir).to_path_buf();
        let mut current = String::new();
        for l in text.lines() {
            let l = l.trim();
            if let Some(name) = l.strip_prefix("newmtl ") {
                current = name.trim().to_owned();
                out.entry(current.clone()).or_default();
            } else if let Some(rest) = l.strip_prefix("map_Kd ") {
                // Options (`-s 1 1 1 …`) come first; the file name is last.
                if let Some(file) = rest.split_whitespace().last() {
                    out.entry(current.clone()).or_default().texture = Some(dir.join(file.replace('\\', "/")));
                }
            } else if let Some(rest) = l.strip_prefix("Kd ") {
                let v: Vec<f32> = rest.split_whitespace().filter_map(|x| x.parse().ok()).collect();
                if v.len() >= 3 {
                    out.entry(current.clone()).or_default().colour = Some([v[0], v[1], v[2]]);
                }
            }
        }
    }
    out
}

/// A texture name from a material or file name: lowercase letters, digits
/// and `_`.
pub fn texture_name(name: &str) -> String {
    let n: String =
        name.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).take(60).collect();
    if n.is_empty() { "texture".to_owned() } else { n }
}

/// Reads a PNG as 8-bit RGBA: (width, height, pixels).
pub fn read_png(path: &Path) -> Result<(u32, u32, Vec<u8>)> {
    let data = std::fs::read(path).with_context(|| format!("opening {}", path.display()))?;
    decode_png(&data).with_context(|| format!("reading {}", path.display()))
}

/// Decodes a PNG as 8-bit RGBA: (width, height, pixels).
pub fn decode_png(data: &[u8]) -> Result<(u32, u32, Vec<u8>)> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(data));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().context("not a PNG")?;
    let mut buf = vec![0u8; reader.output_buffer_size().context("PNG too large")?];
    let info = reader.next_frame(&mut buf)?;
    let pixels = &buf[..info.buffer_size()];
    let rgba = match info.color_type {
        png::ColorType::Rgba => pixels.to_vec(),
        png::ColorType::Rgb => pixels.as_chunks::<3>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => {
            pixels.as_chunks::<2>().0.iter().flat_map(|p| [p[0], p[0], p[0], p[1]]).collect()
        }
        png::ColorType::Grayscale => pixels.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => bail!("indexed PNG could not be expanded"),
    };
    Ok((info.width, info.height, rgba))
}

/// The template shader a material is named after: OpenRAGE's OBJ export
/// names materials `<shader kind>_<texture>` and their images after the
/// texture, so exported models go back on the shaders they came from.
pub fn shader_named(infos: &[ShaderInfo], material: &str, texture: Option<&str>) -> Option<u16> {
    let material = material.to_ascii_lowercase();
    let texture = texture.map(str::to_ascii_lowercase);
    infos
        .iter()
        .find(|i| {
            i.texture.as_deref().map(str::to_ascii_lowercase).is_some_and(|t| {
                material == t || material.ends_with(&format!("_{t}")) || texture.as_deref() == Some(t.as_str())
            })
        })
        .map(|i| i.index)
}

/// A material's texture, unless its shader cannot take one (then the shader
/// keeps its own): (texture, whether one was given and not used).
pub fn fit_texture(infos: &[ShaderInfo], shader: u16, texture: Option<Texture>) -> (Option<Texture>, bool) {
    let takes = infos.iter().find(|i| i.index == shader).is_none_or(|i| i.takes_texture);
    match texture {
        Some(_) if !takes => (None, true),
        t => (t, false),
    }
}

/// What a part's name says, without what tools add: after a dot
/// (`door_dside_f.001`), a level suffix (`_lod1`); with and without a number
/// at the end (`door_dside_f_3`, from OpenRAGE's OBJ export).
fn name_candidates(part: &str) -> (String, Option<String>) {
    let lower = part.trim().to_ascii_lowercase();
    let base = lower.split('.').next().unwrap_or("");
    let base = (1..=3).find_map(|k| base.strip_suffix(&format!("_lod{k}"))).unwrap_or(base);
    let numbered = base
        .rsplit_once('_')
        .filter(|(_, n)| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()))
        .map(|(b, _)| b.to_owned());
    (base.to_owned(), numbered)
}

/// The index of the name in `names` a part is named after (names that keep
/// their number, such as `extra_1`, first).
fn find_name<S: AsRef<str>>(names: &[S], part: &str) -> Option<usize> {
    let (base, numbered) = name_candidates(part);
    // OBJ export writes names with `_` for other characters (`ped#1a2b` → `ped_1a2b`).
    let safe = |n: &str| -> String {
        n.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c.to_ascii_lowercase() } else { '_' }).collect()
    };
    let find = |n: &str| names.iter().position(|b| safe(b.as_ref()) == safe(n));
    find(&base).or_else(|| numbered.and_then(|n| find(&n)))
}

/// Puts parts of a model on the bones of a skeleton by name (vehicles:
/// `door_dside_f`, `bonnet` …); other parts go on the body.
#[derive(Debug, Clone)]
pub struct Bones {
    names: Vec<String>,
    body: u16,
    /// Bones with a model of their own (vehicle wheels): parts named like
    /// them are left out.
    own_models: Vec<String>,
    /// Parts put on a bone (part, bone name), parts that matched none, and
    /// parts left out.
    pub matched: Vec<(String, String)>,
    pub unmatched: Vec<String>,
    pub left_out: Vec<String>,
}

impl Bones {
    /// `names`: the skeleton's bone names in order. The body is the bone
    /// `bodyshell` (vehicles), else `chassis`, else the first.
    pub fn new(names: Vec<String>) -> Self {
        let find = |n: &str| names.iter().position(|b| b.eq_ignore_ascii_case(n));
        let body = find("bodyshell").or_else(|| find("chassis")).unwrap_or(0) as u16;
        Self { names, body, own_models: Vec::new(), matched: Vec::new(), unmatched: Vec::new(), left_out: Vec::new() }
    }

    /// Whether the skeleton is a vehicle's (it has `chassis` or `bodyshell`):
    /// parts go on bones by name. Other skinned models (peds) take their
    /// weights from the original mesh ([`Weights`]).
    pub fn is_vehicle(names: &[String]) -> bool {
        names.iter().any(|n| n.eq_ignore_ascii_case("chassis") || n.eq_ignore_ascii_case("bodyshell"))
    }

    /// Bones that have a model of their own (a fragment's wheels): parts
    /// named like them are left out, as the game draws those models there.
    pub fn with_own_models(mut self, bones: Vec<String>) -> Self {
        self.own_models = bones;
        self
    }

    /// The skeleton index of the bone a part is named after.
    fn find(&self, part: &str) -> Option<usize> {
        find_name(&self.names, part)
    }

    /// Whether a part is left out: named after a bone with a model of its own.
    pub fn leaves_out(&mut self, part: &str) -> bool {
        let out =
            self.find(part).is_some_and(|k| self.own_models.iter().any(|b| b.eq_ignore_ascii_case(&self.names[k])));
        if out && !self.left_out.iter().any(|p| p == part) {
            self.left_out.push(part.to_owned());
        }
        out
    }

    /// The bone a part goes on: by its name, ignoring case and what tools
    /// add: after a dot (`door_dside_f.001`), a level suffix (`_lod1`) or a
    /// number (`door_dside_f_3`, from OpenRAGE's OBJ export).
    pub fn bone_of(&mut self, part: &str) -> u16 {
        match self.find(part) {
            Some(k) => {
                if !part.is_empty() && !self.matched.iter().any(|(p, _)| p == part) {
                    self.matched.push((part.to_owned(), self.names[k].clone()));
                }
                k as u16
            }
            None => {
                if !part.is_empty() && !self.unmatched.iter().any(|p| p == part) {
                    self.unmatched.push(part.to_owned());
                }
                self.body
            }
        }
    }

    /// The name of the bone parts without a bone of their own go on.
    pub fn body_name(&self) -> &str {
        self.names.get(self.body as usize).map_or("", String::as_str)
    }
}

/// Bone weights for new vertices of a model blended between bones (peds,
/// clothing), taken from the nearest vertex of the original meshes: an OBJ
/// file has no place for weights, so a new mesh moves as the original did
/// where it lies.
#[derive(Debug, Clone)]
pub struct Weights {
    points: Vec<[f32; 3]>,
    blends: Vec<Blend>,
    min: [f32; 3],
    cell: f32,
    cells: HashMap<[i32; 3], Vec<u32>>,
}

impl Weights {
    /// From the original meshes' vertices that carry bones; `None` if none do.
    pub fn from_meshes(meshes: &[rage_formats::drawable::Mesh]) -> Option<Self> {
        let mut points = Vec::new();
        let mut blends = Vec::new();
        for m in meshes {
            if m.bone_indices.len() != m.positions.len() || m.bone_weights.len() != m.positions.len() {
                continue;
            }
            for i in 0..m.positions.len() {
                if m.positions[i].iter().all(|v| v.is_finite()) {
                    points.push(m.positions[i]);
                    blends.push(Blend { bones: m.bone_indices[i], weights: m.bone_weights[i] });
                }
            }
        }
        if points.is_empty() {
            return None;
        }
        let (mut min, mut max) = ([f32::MAX; 3], [f32::MIN; 3]);
        for p in &points {
            for k in 0..3 {
                min[k] = min[k].min(p[k]);
                max[k] = max[k].max(p[k]);
            }
        }
        // About four points per cell.
        let volume = (0..3).map(|k| (max[k] - min[k]).max(1e-3)).product::<f32>();
        let cell = (volume / points.len() as f32 * 4.0).cbrt().max(1e-3);
        let mut cells: HashMap<[i32; 3], Vec<u32>> = HashMap::new();
        let mut w = Self { points, blends, min, cell, cells: HashMap::new() };
        for (i, p) in w.points.iter().enumerate() {
            cells.entry(w.cell_of(*p)).or_default().push(i as u32);
        }
        w.cells = cells;
        Some(w)
    }

    fn cell_of(&self, p: [f32; 3]) -> [i32; 3] {
        std::array::from_fn(|k| ((p[k] - self.min[k]) / self.cell).floor().clamp(-1e6, 1e6) as i32)
    }

    /// The bones and weights of the original vertex nearest to `p`.
    pub fn nearest(&self, p: [f32; 3]) -> Blend {
        let c = self.cell_of(p);
        let (mut best, mut best_d) = (0usize, f32::MAX);
        let d2 = |q: [f32; 3]| (0..3).map(|k| (q[k] - p[k]) * (q[k] - p[k])).sum::<f32>();
        // Rings of cells around p until no closer point can be in the next one.
        for r in 0..=8i32 {
            for x in -r..=r {
                for y in -r..=r {
                    for z in -r..=r {
                        if x.abs().max(y.abs()).max(z.abs()) != r {
                            continue;
                        }
                        if let Some(list) = self.cells.get(&[c[0] + x, c[1] + y, c[2] + z]) {
                            for &i in list {
                                let d = d2(self.points[i as usize]);
                                if d < best_d {
                                    (best, best_d) = (i as usize, d);
                                }
                            }
                        }
                    }
                }
            }
            let reach = r as f32 * self.cell;
            if best_d < f32::MAX && best_d <= reach * reach {
                break;
            }
        }
        // Far from the original mesh: search everything.
        let reach = 8.0 * self.cell;
        if best_d > reach * reach {
            for (i, q) in self.points.iter().enumerate() {
                let d = d2(*q);
                if d < best_d {
                    (best, best_d) = (i, d);
                }
            }
        }
        self.blends[best]
    }
}

/// A place of the template that parts of a model go to: the main drawable,
/// a fragment child's model (a wheel), a drawable of a `.ydd`.
#[derive(Debug, Clone)]
pub struct Target {
    pub place: Place,
    /// Parts named like one of these go here (a wheel bone, a dictionary
    /// drawable's name); see [`Fitter::with_targets`] for the others.
    pub names: Vec<String>,
    /// From model space to the place's own (a wheel: its bone's inverse).
    to_local: Option<rage_formats::math::Mat4>,
    shaders: Vec<u16>,
    infos: Vec<ShaderInfo>,
    bones: Option<Bones>,
    weights: Option<Weights>,
    /// (OBJ material name, material)
    pub materials: Vec<(String, NewMaterial)>,
    /// Materials whose texture was not used: their shader takes none, so
    /// it keeps its own.
    pub textures_kept: Vec<String>,
    /// Parts that went here.
    pub parts: Vec<String>,
}

impl Target {
    /// `shaders`: the template shaders meshes there can use.
    pub fn new(place: Place, names: Vec<String>, shaders: Vec<u16>) -> Self {
        Self {
            place,
            names,
            to_local: None,
            shaders,
            infos: Vec::new(),
            bones: None,
            weights: None,
            materials: Vec::new(),
            textures_kept: Vec::new(),
            parts: Vec::new(),
        }
    }

    /// What the shaders look like ([`rage_formats::ydr_write::shader_infos_at`]).
    pub fn with_shader_info(mut self, infos: Vec<ShaderInfo>) -> Self {
        self.infos = infos;
        self
    }

    /// Meshes there are in the space of a bone placed at `transform` (model
    /// space, column-major): the OBJ's positions are moved into it.
    pub fn in_bone_space(mut self, transform: &rage_formats::math::Mat4) -> Self {
        self.to_local = Some(rage_formats::math::inverse_affine(transform));
        self
    }

    pub fn with_bones(mut self, bones: Bones) -> Self {
        self.bones = Some(bones);
        self
    }

    pub fn with_weights(mut self, weights: Weights) -> Self {
        self.weights = Some(weights);
        self
    }

    pub fn bones(&self) -> Option<&Bones> {
        self.bones.as_ref()
    }

    /// Whether the new vertices take their weights from the original mesh.
    pub fn has_weights(&self) -> bool {
        self.weights.is_some()
    }
}

/// Puts OBJ meshes of one or more detail levels on the places of a template
/// ([`Target`]) and on materials: every OBJ material (by name, across levels)
/// of a place becomes one [`NewMaterial`] on a template shader (in order, the
/// last taking the rest, or all on `shader`) with its texture, if `textures`
/// has one.
pub struct Fitter<'a> {
    shader: Option<u16>,
    textures: &'a HashMap<String, Texture>,
    targets: Vec<Target>,
    /// The target parts named after none go to.
    default: Option<usize>,
    /// Parts named after no place, when there is no default place.
    pub unplaced: Vec<String>,
    /// Materials named `#n` whose shader `n` cannot be used where they are.
    pub unusable: Vec<String>,
}

impl<'a> Fitter<'a> {
    /// For the main drawable only, whose meshes can use `shaders`.
    pub fn new(shaders: &[u16], shader: Option<u16>, textures: &'a HashMap<String, Texture>) -> Result<Self> {
        Self::with_targets(vec![Target::new(Place::Main, Vec::new(), shaders.to_vec())], Some(0), shader, textures)
    }

    /// For several places; parts named after none go to `targets[default]`
    /// (none: they are left out, listed in [`Self::unplaced`]).
    pub fn with_targets(
        targets: Vec<Target>,
        default: Option<usize>,
        shader: Option<u16>,
        textures: &'a HashMap<String, Texture>,
    ) -> Result<Self> {
        if let Some(t) = targets.iter().find(|t| t.shaders.is_empty()) {
            bail!("{:?}: the template has no shader meshes can use there", t.place);
        }
        if targets.is_empty() {
            bail!("the template has no place for meshes");
        }
        if let Some(s) = shader
            && !targets.iter().any(|t| t.shaders.contains(&s))
        {
            bail!("shader {s} cannot be used; the template has {:?}", targets[0].shaders);
        }
        Ok(Self { shader, textures, targets, default, unplaced: Vec::new(), unusable: Vec::new() })
    }

    /// What the template's shaders look like ([`rage_formats::ydr_write::shader_infos`]):
    /// materials named after a shader's texture go on it, and textures go
    /// only to shaders that can take one. For the first place.
    pub fn with_shader_info(mut self, infos: Vec<ShaderInfo>) -> Self {
        self.targets[0].infos = infos;
        self
    }

    /// Puts the parts of the first place on these bones (templates with skinned meshes).
    pub fn with_bones(mut self, bones: Bones) -> Self {
        self.targets[0].bones = Some(bones);
        self
    }

    /// The bones parts of the first place went on, after [`Self::level`].
    pub fn bones(&self) -> Option<&Bones> {
        self.targets[0].bones.as_ref()
    }

    pub fn targets(&self) -> &[Target] {
        &self.targets
    }

    pub fn into_targets(self) -> Vec<Target> {
        self.targets
    }

    /// The materials of the first place.
    pub fn materials(&self) -> &[(String, NewMaterial)] {
        &self.targets[0].materials
    }

    /// The target a part goes to.
    fn target_of(&mut self, part: &str) -> Option<usize> {
        let named = self.targets.iter().position(|t| find_name(&t.names, part).is_some());
        let k = named.or(self.default);
        match k {
            Some(k) if !self.targets[k].parts.iter().any(|p| p == part) => self.targets[k].parts.push(part.to_owned()),
            None if !self.unplaced.iter().any(|p| p == part) => self.unplaced.push(part.to_owned()),
            _ => {}
        }
        k
    }

    /// The meshes of one level per place (in the order of the targets),
    /// their `shader` the index of their material at that place.
    pub fn level(&mut self, meshes: Vec<ObjMesh>) -> Vec<Vec<NewMesh>> {
        let mut out: Vec<Vec<NewMesh>> = vec![Vec::new(); self.targets.len()];
        for m in meshes {
            let of: Vec<Option<usize>> = m.part_names.iter().map(|p| self.target_of(p)).collect();
            for (k, slot) in out.iter_mut().enumerate() {
                let piece = if of.iter().all(|t| *t == Some(k)) {
                    m.clone()
                } else if of.contains(&Some(k)) {
                    without_triangles(m.clone(), |part| of[part as usize] != Some(k))
                } else {
                    continue;
                };
                slot.extend(self.fit(k, piece));
            }
        }
        out
    }

    /// One mesh at a place: moved into its space, on a material, its
    /// vertices on bones.
    fn fit(&mut self, k: usize, mut m: ObjMesh) -> Vec<NewMesh> {
        let shader_opt = self.shader;
        let textures = self.textures;
        let unusable = &mut self.unusable;
        let t = &mut self.targets[k];
        // Only the parts that still have vertices here (others went to other places).
        let present = |m: &ObjMesh| -> Vec<bool> {
            let mut p = vec![false; m.part_names.len()];
            m.parts.iter().for_each(|&k| p[k as usize] = true);
            p
        };
        if let Some(b) = &mut t.bones {
            let here = present(&m);
            let out: Vec<bool> = m.part_names.iter().zip(&here).map(|(p, &h)| h && b.leaves_out(p)).collect();
            if out.contains(&true) {
                m = without_triangles(m, |part| out[part as usize]);
            }
        }
        if m.indices.is_empty() {
            return Vec::new();
        }
        if let Some(to) = &t.to_local {
            m.positions.iter_mut().for_each(|p| *p = rage_formats::math::transform_point(to, *p));
            m.normals.iter_mut().for_each(|n| *n = rage_formats::math::transform_dir(to, *n));
        }
        let i = match t.materials.iter().position(|(name, _)| *name == m.material) {
            Some(i) => i,
            None => {
                let i = t.materials.len();
                let texture = textures.get(&m.material).cloned();
                let last = *t.shaders.last().expect("checked");
                let explicit = m.material.strip_prefix('#').and_then(|n| n.parse::<u16>().ok());
                if explicit.is_some_and(|s| !t.shaders.contains(&s)) && !unusable.contains(&m.material) {
                    unusable.push(m.material.clone());
                }
                let shader = explicit
                    .filter(|s| t.shaders.contains(s))
                    .or(shader_opt.filter(|s| t.shaders.contains(s)))
                    .or_else(|| shader_named(&t.infos, &m.material, texture.as_ref().map(|t| t.name.as_str())))
                    .filter(|s| t.shaders.contains(s))
                    .unwrap_or_else(|| *t.shaders.get(i).unwrap_or(&last));
                let (texture, kept) = fit_texture(&t.infos, shader, texture);
                if kept {
                    t.textures_kept.push(m.material.clone());
                }
                t.materials.push((m.material.clone(), NewMaterial { shader, texture }));
                i
            }
        };
        let bones = match &mut t.bones {
            Some(b) => {
                let here = present(&m);
                let of: Vec<u16> =
                    m.part_names.iter().zip(&here).map(|(p, &h)| if h { b.bone_of(p) } else { 0 }).collect();
                m.parts.iter().map(|&p| of[p as usize]).collect()
            }
            None => Vec::new(),
        };
        let blend = match &t.weights {
            Some(w) => m.positions.iter().map(|p| w.nearest(*p)).collect(),
            None => Vec::new(),
        };
        split(NewMesh {
            shader: i as u16,
            positions: m.positions,
            normals: m.normals,
            uvs: m.uvs,
            colors: Vec::new(),
            indices: m.indices,
            bones,
            blend,
        })
    }
}

/// The places of a template that an OBJ's parts can go to, as
/// [`Fitter::with_targets`] takes them: the main drawable (with its bones for
/// vehicles, weights from the original for other skinned models) and the
/// models of fragment children (wheels: parts named after their bone), or the
/// drawables of a `.ydd` (`names`: theirs, by index; parts named after one go
/// to it, the rest to `drawable` if given). Returns the targets and the default.
pub fn targets(
    template: &Resource,
    dictionary: Option<(&[String], Option<usize>)>,
) -> Result<(Vec<Target>, Option<usize>)> {
    use rage_formats::ydr_write as w;
    let place_target = |place: Place, names: Vec<String>| -> Result<Target> {
        let mut t = Target::new(place, names, w::usable_shaders_at(template, place)?)
            .with_shader_info(w::shader_infos_at(template, place)?);
        if w::is_skinned_at(template, place)? {
            let original = w::read_at(template, place)?;
            let bones: Option<Vec<String>> =
                original.skeleton.as_ref().map(|s| s.bones.iter().map(|b| b.name.clone()).collect());
            match bones {
                Some(names) if Bones::is_vehicle(&names) => {
                    t = t.with_bones(Bones::new(names).with_own_models(w::bones_with_own_models(template)))
                }
                _ => {
                    if let Some(weights) = Weights::from_meshes(&original.meshes) {
                        t = t.with_weights(weights);
                    }
                }
            }
        }
        Ok(t)
    };
    if let Some((names, drawable)) = dictionary {
        let targets = (0..names.len())
            .map(|i| place_target(Place::Entry(i), vec![names[i].clone()]))
            .collect::<Result<Vec<_>>>()?;
        return Ok((targets, drawable));
    }
    let mut targets = vec![place_target(Place::Main, Vec::new())?];
    for (i, child) in w::child_models(template).into_iter().enumerate() {
        // Children this writer cannot fill (no usable geometry) keep their models.
        let Ok(t) = place_target(Place::Child(i), vec![child.bone.clone()]) else { continue };
        if t.shaders.is_empty() {
            continue;
        }
        targets.push(t.in_bone_space(&child.transform));
    }
    Ok((targets, Some(0)))
}

/// What [`import`] needs besides the model.
#[derive(Debug, Clone, Copy, Default)]
pub struct ImportOptions<'a> {
    /// Every material on this shader (where it can be used).
    pub shader: Option<u16>,
    /// `.ydd`: the drawable parts named after none go to (its name or index).
    pub drawable: Option<&'a str>,
}

/// A written model and where its parts went.
pub struct Imported {
    pub file: Vec<u8>,
    pub targets: Vec<Target>,
    /// Parts of a `.ydd` model named after none of its drawables (left out).
    pub unplaced: Vec<String>,
    /// Detail levels of the closest place written.
    pub levels: usize,
}

/// The places of the model at `vpath` (a `.ydr`, `.yft` or `.ydd` of GTA V
/// Enhanced) parts can go to ([`targets`]).
pub fn targets_of(template: &Resource, vpath: &str, drawable: Option<&str>) -> Result<(Vec<Target>, Option<usize>)> {
    let lower = vpath.to_ascii_lowercase();
    if !lower.ends_with(".ydd") {
        if drawable.is_some() {
            bail!("--drawable is for drawable dictionaries (.ydd)");
        }
        return targets(template, None);
    }
    let stem = lower.rsplit('/').next().unwrap_or(&lower).trim_end_matches(".ydd");
    let hashes = rage_formats::ydr_write::entry_hashes(template)?;
    let names = crate::models::dictionary_names(stem, &hashes);
    let default = match drawable {
        None => None,
        Some(d) => Some(
            find_name(&names, d)
                .or_else(|| d.parse::<usize>().ok().filter(|&i| i < names.len()))
                .with_context(|| format!("{vpath} has no drawable {d:?}; it has {}", names.join(", ")))?,
        ),
    };
    targets(template, Some((&names, default)))
}

/// Writes OBJ meshes (one list per detail level) into the model `template`
/// at `vpath`: every part to its place ([`targets_of`]), checked to read back.
/// `donor` is asked for a texture structure when the model has none.
pub fn import(
    template: &Resource,
    vpath: &str,
    levels: Vec<Vec<ObjMesh>>,
    textures: &HashMap<String, Texture>,
    options: ImportOptions,
    donor: impl FnOnce() -> Option<Resource>,
) -> Result<Imported> {
    use rage_formats::ydr_write as w;
    let (targets, default) = targets_of(template, vpath, options.drawable)?;
    let mut fitter = Fitter::with_targets(targets, default, options.shader, textures)?;
    let fitted: Vec<Vec<Vec<NewMesh>>> = levels.into_iter().map(|l| fitter.level(l)).collect();
    if !fitter.unusable.is_empty() {
        bail!("materials {}: no such shader can be used there", fitter.unusable.join(", "));
    }
    let parts = parts_of(&fitter, &fitted)?;
    let needs_donor = parts.iter().any(|(_, _, m)| m.iter().any(|m| m.texture.is_some()));
    let donor = (needs_donor && !w::can_donate_textures(template)).then(donor).flatten();
    let lods: Vec<Vec<&[NewMesh]>> = parts.iter().map(|(_, l, _)| l.iter().map(Vec::as_slice).collect()).collect();
    let write_parts: Vec<w::Part> = parts
        .iter()
        .zip(&lods)
        .map(|((place, _, mats), l)| w::Part { place: *place, lods: l, materials: Some(mats) })
        .collect();
    let file = w::replace_parts(template, &write_parts, donor.as_ref())?;
    // What the game gets must read back.
    let back = Resource::from_rsc7(&file)?;
    if vpath.to_ascii_lowercase().ends_with(".ydd") {
        rage_formats::drawable::parse_ydd(&back).context("the built model does not read back")?;
    } else {
        w::read_back(&file).context("the built model does not read back")?;
    }
    for (place, _, _) in &parts {
        w::read_at(&back, *place).with_context(|| format!("{place:?} does not read back"))?;
    }
    let levels = parts.first().map_or(0, |(_, l, _)| l.len());
    let unplaced = std::mem::take(&mut fitter.unplaced);
    Ok(Imported { file, targets: fitter.into_targets(), unplaced, levels })
}

/// The levels per target, as [`rage_formats::ydr_write::Part`]s need them:
/// for every target with meshes in the closest level, its levels up to the
/// first empty one, and its materials.
#[allow(clippy::type_complexity)]
pub fn parts_of(
    fitter: &Fitter,
    levels: &[Vec<Vec<NewMesh>>],
) -> Result<Vec<(Place, Vec<Vec<NewMesh>>, Vec<NewMaterial>)>> {
    let mut out = Vec::new();
    for (k, t) in fitter.targets().iter().enumerate() {
        let lods: Vec<Vec<NewMesh>> =
            levels.iter().map(|l| l.get(k).cloned().unwrap_or_default()).take_while(|l| !l.is_empty()).collect();
        if lods.is_empty() {
            if levels.iter().any(|l| l.get(k).is_some_and(|m| !m.is_empty())) {
                bail!("{:?}: meshes only for further detail levels, none for the closest", t.place);
            }
            continue;
        }
        out.push((t.place, lods, t.materials.iter().map(|(_, m)| m.clone()).collect()));
    }
    if out.is_empty() {
        bail!("no meshes for the model: every part was left out");
    }
    Ok(out)
}

/// An Enhanced drawable with embedded textures (or a texture dictionary),
/// to lend its texture structure to a model that has none: one next to
/// `vpath` in its archive, else any in that archive.
pub fn find_texture_donor(
    game_dir: &Path,
    vpath: &str,
    keys: Option<Arc<GtaVKeys>>,
) -> Option<rage_resource::Resource> {
    let parent = vpath.rsplit_once('/').map_or("", |(p, _)| p);
    let Ok((Node::ArchiveDir(rpf, dir), _)) = rage_mods::resolve(game_dir, parent, keys.clone()) else { return None };
    let donates = |r: &rage_resource::Resource| rage_formats::ydr_write::can_donate_textures(r);
    // Drawables first, then texture dictionaries (vehicle archives have only
    // fragments and dictionaries).
    let kinds = |name: &str| {
        let n = name.to_ascii_lowercase();
        n.ends_with(".ydr") || n.ends_with(".ytd")
    };
    let mut children: Vec<_> = rpf.children(dir).collect();
    children.sort_by_key(|&c| !rpf.entry(c).name.to_ascii_lowercase().ends_with(".ydr"));
    for c in children {
        if kinds(&rpf.entry(c).name)
            && let Ok(r) = rpf.read_resource(c)
            && donates(&r)
        {
            return Some(r);
        }
    }
    let mut found = None;
    let mut seen = 0;
    rage_rpf::vfs::walk(
        &rpf,
        rage_rpf::Rpf7::ROOT,
        "",
        true,
        &mut |a, id, p| {
            if found.is_some() || seen > 5000 || !kinds(p) {
                return;
            }
            seen += 1;
            if let Ok(r) = a.read_resource(id)
                && donates(&r)
            {
                found = Some(r);
            }
        },
        &mut |_| {},
    );
    // Archives of fragments only (vehicle mod parts): the game's shared vehicle textures.
    found.or_else(|| {
        let Ok((Node::ArchiveFile(rpf, id), _)) = rage_mods::resolve(game_dir, SHARED_TEXTURES, keys) else {
            return None;
        };
        rpf.read_resource(id).ok().filter(donates)
    })
}

/// An OBJ model as a collision: every triangle, with the material type
/// its OBJ material is named after (`CONCRETE`, `metal_solid_medium`: a name
/// of the game's `materials.dat`, ignoring case; others get the original's
/// first material). Returns the mesh and per OBJ material the name it matched.
pub fn collision_mesh(
    meshes: &[ObjMesh],
    names: &[String],
) -> (rage_formats::ybn_write::CollisionMesh, Vec<(String, Option<String>)>) {
    let mut out = rage_formats::ybn_write::CollisionMesh::default();
    let mut matched = Vec::new();
    for m in meshes {
        let k = names.iter().position(|n| n.eq_ignore_ascii_case(m.material.trim()));
        if !matched.iter().any(|(n, _): &(String, Option<String>)| *n == m.material) {
            matched.push((m.material.clone(), k.map(|k| names[k].clone())));
        }
        let base = out.positions.len() as u32;
        out.positions.extend_from_slice(&m.positions);
        out.indices.extend(m.indices.iter().map(|i| i + base));
        out.materials.extend(std::iter::repeat_n(k.and_then(|k| u8::try_from(k).ok()), m.indices.len() / 3));
    }
    (out, matched)
}

/// A collision with a triangle mesh (a `.ybn`; `true`: a `.ydr`) to lend
/// its structure to a model whose collision has none (only boxes, spheres):
/// one in the archive of `vpath`, else the first one of the game.
pub fn find_collision_donor(
    game_dir: &Path,
    vpath: &str,
    keys: Option<Arc<GtaVKeys>>,
) -> Option<(rage_resource::Resource, bool)> {
    use rage_formats::ybn_write::can_donate;
    let look = |rpf: &rage_rpf::Rpf7, from: rage_rpf::EntryId, limit: usize| {
        let mut found = None;
        let mut seen = 0;
        rage_rpf::vfs::walk(
            rpf,
            from,
            "",
            true,
            &mut |a, id, p| {
                let ydr = p.to_ascii_lowercase().ends_with(".ydr");
                if found.is_some() || seen > limit || !(ydr || p.to_ascii_lowercase().ends_with(".ybn")) {
                    return;
                }
                seen += 1;
                if let Ok(r) = a.read_resource(id)
                    && can_donate(&r, ydr)
                {
                    found = Some((r, ydr));
                }
            },
            &mut |_| {},
        );
        found
    };
    let parent = vpath.rsplit_once('/').map_or("", |(p, _)| p);
    if let Ok((Node::ArchiveDir(rpf, _), _)) = rage_mods::resolve(game_dir, parent, keys.clone())
        && let Some(f) = look(&rpf, rage_rpf::Rpf7::ROOT, 5000)
    {
        return Some(f);
    }
    let mut rpfs = Vec::new();
    crate::detect::collect_rpfs(game_dir, &mut rpfs);
    rpfs.sort();
    rpfs.iter().find_map(|p| {
        let rpf = rage_rpf::Rpf7::open(p, keys.clone()).ok()?;
        look(&rpf, rage_rpf::Rpf7::ROOT, 2000)
    })
}

/// A texture dictionary every GTA V installation has, as the last donor.
const SHARED_TEXTURES: &str = "x64e.rpf/levels/gta5/vehicles.rpf/vehshare.ytd";

/// The mesh without the triangles whose first corner belongs to a part
/// `drop` names (by index into its part names); unused vertices go too.
fn without_triangles(m: ObjMesh, drop: impl Fn(u16) -> bool) -> ObjMesh {
    let mut out = ObjMesh { material: m.material, part_names: m.part_names, ..Default::default() };
    let mut map: HashMap<u32, u32> = HashMap::new();
    for t in m.indices.as_chunks::<3>().0 {
        if drop(m.parts[t[0] as usize]) {
            continue;
        }
        for &i in t {
            let k = *map.entry(i).or_insert_with(|| {
                let i = i as usize;
                out.positions.push(m.positions[i]);
                out.parts.push(m.parts[i]);
                if !m.normals.is_empty() {
                    out.normals.push(m.normals[i]);
                }
                if !m.uvs.is_empty() {
                    out.uvs.push(m.uvs[i]);
                }
                out.positions.len() as u32 - 1
            });
            out.indices.push(k);
        }
    }
    out
}

/// Splits a mesh into pieces of at most [`MAX_VERTICES`] vertices.
pub fn split(m: NewMesh) -> Vec<NewMesh> {
    if m.positions.len() <= MAX_VERTICES {
        return vec![m];
    }
    let mut out = Vec::new();
    let mut piece = NewMesh { shader: m.shader, ..Default::default() };
    let mut map: HashMap<u32, u32> = HashMap::new();
    for t in m.indices.as_chunks::<3>().0 {
        let new = t.iter().filter(|i| !map.contains_key(i)).count();
        if piece.positions.len() + new > MAX_VERTICES {
            out.push(std::mem::replace(&mut piece, NewMesh { shader: m.shader, ..Default::default() }));
            map.clear();
        }
        for &i in t {
            let k = *map.entry(i).or_insert_with(|| {
                let k = piece.positions.len() as u32;
                piece.positions.push(m.positions[i as usize]);
                if !m.normals.is_empty() {
                    piece.normals.push(m.normals[i as usize]);
                }
                if !m.uvs.is_empty() {
                    piece.uvs.push(m.uvs[i as usize]);
                }
                if !m.colors.is_empty() {
                    piece.colors.push(m.colors[i as usize]);
                }
                if !m.bones.is_empty() {
                    piece.bones.push(m.bones[i as usize]);
                }
                if !m.blend.is_empty() {
                    piece.blend.push(m.blend[i as usize]);
                }
                k
            });
            piece.indices.push(k);
        }
    }
    if !piece.indices.is_empty() {
        out.push(piece);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_materials_and_converts_axes() {
        let obj = "v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nvt 0 0\nvt 1 1\nusemtl a\nf 1/1 2/2 3/2 4/1\nusemtl b\nf 1 2 3\nusemtl a\nf 2 3 4\n";
        let m = read(obj).unwrap();
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].material, "a");
        assert_eq!(m[0].indices.len(), 9, "both 'a' groups in one mesh");
        assert_eq!(m[0].positions[2], [1.0, 0.0, 1.0], "Y-up → Z-up");
        assert_eq!(m[0].uvs[0], [0.0, 1.0], "v flipped");
        assert!(m[0].normals.is_empty(), "no normals given: computed later");
        assert!(read("v 0 0 0\nf 1 2 3").is_err());
    }

    #[test]
    fn reads_mtl_files() {
        let dir = std::env::temp_dir().join(format!("openrage-mtl-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("tex")).unwrap();
        std::fs::write(
            dir.join("m.mtl"),
            "newmtl Paint\nKd 1 0.5 0\n\nnewmtl Glass\nKd 0.2 0.2 0.2\nmap_Kd -s 1 1 1 tex\\glass.png\n",
        )
        .unwrap();
        let m = materials("mtllib m.mtl\nv 0 0 0\n", &dir);
        assert_eq!(m["Paint"], MtlMaterial { texture: None, colour: Some([1.0, 0.5, 0.0]) });
        assert_eq!(m["Glass"].texture.as_deref(), Some(dir.join("tex/glass.png").as_path()));
        assert_eq!(texture_name("Vehicle Paint #1"), "vehicle_paint__1");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parts_go_on_the_bones_of_their_names() {
        let text = "v 0 0 0\nv 1 0 0\nv 0 1 0\nv 1 1 0\n\
            o bodyshell_mesh\nusemtl paint\nf 1 2 3\n\
            o door_dside_f.001\nusemtl paint\nf 2 4 3\n\
            g BONNET\nusemtl glass\nf 1 2 4\n";
        let meshes = read(text).unwrap();
        assert_eq!(meshes.len(), 2);
        let paint = &meshes[0];
        assert_eq!(paint.part_names, ["bodyshell_mesh", "door_dside_f.001"]);
        assert_eq!(paint.positions.len(), 6, "a corner shared by two parts is two vertices");
        assert_eq!(paint.parts, [0, 0, 0, 1, 1, 1]);

        let skeleton = ["chassis", "bonnet", "door_dside_f", "bodyshell"].map(String::from).to_vec();
        let textures = HashMap::new();
        let mut fit = Fitter::new(&[0], None, &textures).unwrap().with_bones(Bones::new(skeleton));
        let out = fit.level(meshes).swap_remove(0);
        assert_eq!(out[0].bones, [3, 3, 3, 2, 2, 2], "unknown parts go on the body (bodyshell)");
        assert_eq!(out[1].bones, [1, 1, 1]);
        let bones = fit.bones().unwrap();
        assert_eq!(bones.unmatched, ["bodyshell_mesh"]);
        assert_eq!(bones.body_name(), "bodyshell");
        assert!(bones.matched.contains(&("door_dside_f.001".into(), "door_dside_f".into())));
        let mut b = Bones::new(["chassis", "extra_1", "bonnet"].map(String::from).to_vec());
        assert_eq!((b.bone_of("bonnet_12"), b.bone_of("extra_1"), b.bone_of("extra_1_4")), (2, 1, 1));

        // Parts named after a bone with a model of its own (wheels) are left out.
        let skeleton = ["chassis", "bodyshell", "wheel_lf"].map(String::from).to_vec();
        let wheels = Bones::new(skeleton).with_own_models(vec!["wheel_lf".into()]);
        let mut fit = Fitter::new(&[0], None, &textures).unwrap().with_bones(wheels);
        let text = "v 0 0 0\nv 1 0 0\nv 0 1 0\nv 1 1 0\no body_0\nf 1 2 3\no wheel_lf_7\nf 2 4 3\n";
        let out = fit.level(read(text).unwrap()).swap_remove(0);
        assert_eq!((out.len(), out[0].positions.len(), out[0].indices.len()), (1, 3, 3));
        assert_eq!(fit.bones().unwrap().left_out, ["wheel_lf_7"]);

        // Materials named after a shader's texture go on it; a shader without
        // a colour slot keeps its own texture.
        let infos = vec![
            ShaderInfo { index: 0, texture: Some("paint".into()), takes_texture: false },
            ShaderInfo { index: 4, texture: Some("Plate01".into()), takes_texture: true },
        ];
        assert_eq!(shader_named(&infos, "plate_shader_plate01", None), Some(4));
        assert_eq!(shader_named(&infos, "x", Some("plate01")), Some(4));
        assert_eq!(shader_named(&infos, "glass", None), None);
        let tex = rage_formats::ydr_write::texture_from_rgba("t", 4, 4, &[9; 64]).unwrap();
        assert!(fit_texture(&infos, 0, Some(tex.clone())).1);
        assert!(fit_texture(&infos, 4, Some(tex)).0.is_some());

        // Without bones: none.
        let mut plain = Fitter::new(&[0], None, &textures).unwrap();
        assert!(plain.level(read(text).unwrap())[0].iter().all(|m| m.bones.is_empty()));
    }

    #[test]
    fn fits_materials_to_shaders_and_splits_big_meshes() {
        let mesh = |name: &str| ObjMesh {
            material: name.into(),
            positions: vec![[0.0; 3]; 3],
            normals: Vec::new(),
            uvs: Vec::new(),
            indices: vec![0, 1, 2],
            ..Default::default()
        };
        let textures = HashMap::new();
        let mut fit = Fitter::new(&[0, 2], None, &textures).unwrap();
        let hi = fit.level(vec![mesh("a"), mesh("b"), mesh("c")]).swap_remove(0);
        let lod = fit.level(vec![mesh("c"), mesh("d")]).swap_remove(0);
        let shaders: Vec<u16> = fit.materials().iter().map(|(_, m)| m.shader).collect();
        assert_eq!(shaders, [0, 2, 2, 2], "in order, the last taking the rest");
        assert_eq!(hi.iter().map(|m| m.shader).collect::<Vec<_>>(), [0, 1, 2]);
        assert_eq!(lod.iter().map(|m| m.shader).collect::<Vec<_>>(), [2, 3], "materials shared across levels by name");
        assert!(Fitter::new(&[0], Some(5), &textures).is_err());

        // 30,000 separate triangles (90,000 vertices): two pieces.
        let n = 30_000 * 3;
        let big =
            NewMesh { shader: 0, positions: vec![[0.0; 3]; n], indices: (0..n as u32).collect(), ..Default::default() };
        let pieces = split(big);
        assert_eq!(pieces.len(), 2);
        assert!(pieces.iter().all(|p| p.positions.len() <= MAX_VERTICES));
        assert_eq!(pieces.iter().map(|p| p.indices.len()).sum::<usize>(), n);
    }

    #[test]
    fn parts_go_to_their_places() {
        // A body with bones and a wheel model on `wheel_lf` at (1, 2, 3).
        let skeleton = ["chassis", "bodyshell", "wheel_lf", "wheel_rf"].map(String::from).to_vec();
        let own = vec!["wheel_lf".to_owned(), "wheel_rf".to_owned()];
        let body = Target::new(Place::Main, Vec::new(), vec![0]).with_bones(Bones::new(skeleton).with_own_models(own));
        let mut at = rage_formats::math::IDENTITY;
        (at[12], at[13], at[14]) = (1.0, 2.0, 3.0);
        let wheel = Target::new(Place::Child(0), vec!["wheel_lf".into()], vec![4]).in_bone_space(&at);
        let textures = HashMap::new();
        let mut fit = Fitter::with_targets(vec![body, wheel], Some(0), None, &textures).unwrap();
        let text = "v 0 0 0\nv 1 0 0\nv 0 1 0\nv 1 1 0\n\
            o body_0\nusemtl paint\nf 1 2 3\no wheel_lf_7\nusemtl tyre\nf 2 4 3\no wheel_rf_8\nusemtl tyre\nf 1 2 4\n";
        let out = fit.level(read(text).unwrap());
        assert_eq!(out[0].len(), 1, "the body");
        assert_eq!(out[0][0].indices.len(), 3, "without the wheels");
        assert_eq!(out[1][0].positions.len(), 3, "the wheel");
        // OBJ (1, 0, 0) is (1, 0, 0) in the game, minus the bone's place.
        assert_eq!(out[1][0].positions[0], [0.0, -2.0, -3.0]);
        assert!(out[1][0].bones.is_empty(), "the wheel model is fixed");
        let t = fit.targets();
        assert_eq!(t[1].materials[0].1.shader, 4, "the wheel's own shaders");
        assert_eq!(t[1].parts, ["wheel_lf_7"]);
        assert_eq!(t[0].bones().unwrap().left_out, ["wheel_rf_8"], "the copy the game makes of wheel_lf");
        let levels = vec![out];
        let parts = parts_of(&fit, &levels).unwrap();
        assert_eq!(parts.iter().map(|p| p.0).collect::<Vec<_>>(), [Place::Main, Place::Child(0)]);

        // Dictionaries: parts by drawable name (also as OBJ export writes them), the rest to the default.
        let a = Target::new(Place::Entry(0), vec!["uppr_000_u".into()], vec![0]);
        let b = Target::new(Place::Entry(1), vec!["ped#1a2b".into()], vec![0]);
        let mut fit = Fitter::with_targets(vec![a, b], None, None, &textures).unwrap();
        let text = "v 0 0 0\nv 1 0 0\nv 0 1 0\no ped_1a2b_3\nf 1 2 3\no other\nf 1 2 3\n";
        let out = fit.level(read(text).unwrap());
        assert!(out[0].is_empty());
        assert_eq!(out[1].len(), 1);
        assert_eq!(fit.unplaced, ["other"]);
    }

    #[test]
    fn weights_come_from_the_nearest_vertex() {
        let mesh = rage_formats::drawable::Mesh {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 5.0, 0.0]],
            bone_indices: vec![[1, 0, 0, 0], [2, 3, 0, 0], [4, 0, 0, 0]],
            bone_weights: vec![[1.0, 0.0, 0.0, 0.0], [0.5, 0.5, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0]],
            ..Default::default()
        };
        let w = Weights::from_meshes(&[mesh]).unwrap();
        assert_eq!(w.nearest([0.9, 0.1, 0.0]).bones, [2, 3, 0, 0]);
        assert_eq!(w.nearest([0.1, 0.0, 0.0]).weights, [1.0, 0.0, 0.0, 0.0]);
        assert_eq!(w.nearest([0.0, 4.0, 1.0]).bones[0], 4);
        assert_eq!(w.nearest([0.0, 400.0, 0.0]).bones[0], 4, "far away: still the nearest");
        assert!(Weights::from_meshes(&[rage_formats::drawable::Mesh::default()]).is_none());
    }
}
