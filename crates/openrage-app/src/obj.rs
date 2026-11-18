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
use rage_formats::ydr_write::{MAX_VERTICES, NewMaterial, NewMesh, ShaderInfo};
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

/// Puts OBJ meshes of one or more detail levels on materials: every OBJ
/// material (by name, across levels) becomes one [`NewMaterial`] on a
/// template shader (in order, the last taking the rest, or all on `shader`)
/// with its texture, if `textures` has one.
pub struct Fitter<'a> {
    shaders: &'a [u16],
    shader: Option<u16>,
    textures: &'a HashMap<String, Texture>,
    /// (OBJ material name, material)
    pub materials: Vec<(String, NewMaterial)>,
    bones: Option<Bones>,
    infos: Vec<ShaderInfo>,
    /// Materials whose texture was not used: their shader takes none, so
    /// it keeps its own.
    pub textures_kept: Vec<String>,
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

/// Puts parts of a model on the bones of a skeleton by name (vehicles:
/// `door_dside_f`, `bonnet`, `wheel_lf` …); other parts go on the body.
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

    /// Bones that have a model of their own (a fragment's wheels): parts
    /// named like them are left out, as the game draws those models there.
    pub fn with_own_models(mut self, bones: Vec<String>) -> Self {
        self.own_models = bones;
        self
    }

    /// The skeleton index of the bone a part is named after.
    fn find(&self, part: &str) -> Option<usize> {
        let lower = part.trim().to_ascii_lowercase();
        let base = lower.split('.').next().unwrap_or("");
        let base = (1..=3).find_map(|k| base.strip_suffix(&format!("_lod{k}"))).unwrap_or(base);
        let find = |n: &str| self.names.iter().position(|b| b.eq_ignore_ascii_case(n));
        // OBJ exports number their objects (`door_dside_f_3`); bones such as
        // `extra_1` keep their number when they exist.
        let numbered = base
            .rsplit_once('_')
            .filter(|(_, n)| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()))
            .map(|(b, _)| b);
        find(base).or_else(|| numbered.and_then(find))
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

impl<'a> Fitter<'a> {
    pub fn new(shaders: &'a [u16], shader: Option<u16>, textures: &'a HashMap<String, Texture>) -> Result<Self> {
        if shaders.is_empty() {
            bail!("the template has no shader meshes can use");
        }
        if let Some(s) = shader
            && !shaders.contains(&s)
        {
            bail!("shader {s} cannot be used; the template has {shaders:?}");
        }
        Ok(Self {
            shaders,
            shader,
            textures,
            materials: Vec::new(),
            bones: None,
            infos: Vec::new(),
            textures_kept: Vec::new(),
        })
    }

    /// What the template's shaders look like ([`rage_formats::ydr_write::shader_infos`]):
    /// materials named after a shader's texture go on it, and textures go
    /// only to shaders that can take one.
    pub fn with_shader_info(mut self, infos: Vec<ShaderInfo>) -> Self {
        self.infos = infos;
        self
    }

    /// Puts the parts on these bones (templates with skinned meshes).
    pub fn with_bones(mut self, bones: Bones) -> Self {
        self.bones = Some(bones);
        self
    }

    /// The bones parts went on, after [`Self::level`].
    pub fn bones(&self) -> Option<&Bones> {
        self.bones.as_ref()
    }

    /// The meshes of one level, their `shader` the index of their material.
    pub fn level(&mut self, meshes: Vec<ObjMesh>) -> Vec<NewMesh> {
        let mut out = Vec::new();
        for m in meshes {
            let m = match &mut self.bones {
                Some(b) => {
                    let out: Vec<bool> = m.part_names.iter().map(|p| b.leaves_out(p)).collect();
                    if out.contains(&true) { without_triangles(m, |part| out[part as usize]) } else { m }
                }
                None => m,
            };
            if m.indices.is_empty() {
                continue;
            }
            let k = match self.materials.iter().position(|(name, _)| *name == m.material) {
                Some(k) => k,
                None => {
                    let i = self.materials.len();
                    let texture = self.textures.get(&m.material).cloned();
                    let shader = self.shader.unwrap_or_else(|| {
                        shader_named(&self.infos, &m.material, texture.as_ref().map(|t| t.name.as_str()))
                            .unwrap_or_else(|| *self.shaders.get(i).unwrap_or(self.shaders.last().expect("checked")))
                    });
                    let (texture, kept) = fit_texture(&self.infos, shader, texture);
                    if kept {
                        self.textures_kept.push(m.material.clone());
                    }
                    self.materials.push((m.material.clone(), NewMaterial { shader, texture }));
                    i
                }
            };
            let bones = match &mut self.bones {
                Some(b) => {
                    let of: Vec<u16> = m.part_names.iter().map(|p| b.bone_of(p)).collect();
                    m.parts.iter().map(|&p| of[p as usize]).collect()
                }
                None => Vec::new(),
            };
            out.extend(split(NewMesh {
                shader: k as u16,
                positions: m.positions,
                normals: m.normals,
                uvs: m.uvs,
                colors: Vec::new(),
                indices: m.indices,
                bones,
            }));
        }
        out
    }
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
    let Ok((Node::ArchiveDir(rpf, dir), _)) = rage_mods::resolve(game_dir, parent, keys) else { return None };
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
    found
}

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
        let out = fit.level(meshes);
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
        let out = fit.level(read(text).unwrap());
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
        assert!(plain.level(read(text).unwrap()).iter().all(|m| m.bones.is_empty()));
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
        let hi = fit.level(vec![mesh("a"), mesh("b"), mesh("c")]);
        let lod = fit.level(vec![mesh("c"), mesh("d")]);
        let shaders: Vec<u16> = fit.materials.iter().map(|(_, m)| m.shader).collect();
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
}
