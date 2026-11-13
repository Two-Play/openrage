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
use rage_formats::ydr_write::{MAX_VERTICES, NewMaterial, NewMesh};
use rage_rpf::vfs::Node;

/// Triangles of one material.
#[derive(Debug, Clone, Default)]
pub struct ObjMesh {
    pub material: String,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

/// Reads an OBJ file: `v`, `vt`, `vn`, `f` (polygons fanned into triangles,
/// negative indices from the end) and `usemtl`. One mesh per material, in the
/// order they first appear.
pub fn read(text: &str) -> Result<Vec<ObjMesh>> {
    let (mut v, mut vt, mut vn) = (Vec::<[f32; 3]>::new(), Vec::<[f32; 2]>::new(), Vec::<[f32; 3]>::new());
    let mut meshes: Vec<ObjMesh> = Vec::new();
    let mut seen: Vec<HashMap<(usize, usize, usize), u32>> = Vec::new();
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
                    let key = (p, t, nn);
                    let i = match seen[current].get(&key) {
                        Some(&i) => i,
                        None => {
                            let i = mesh.positions.len() as u32;
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
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().with_context(|| format!("{} is not a PNG", path.display()))?;
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
        png::ColorType::Indexed => bail!("{}: indexed PNG could not be expanded", path.display()),
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
        Ok(Self { shaders, shader, textures, materials: Vec::new() })
    }

    /// The meshes of one level, their `shader` the index of their material.
    pub fn level(&mut self, meshes: Vec<ObjMesh>) -> Vec<NewMesh> {
        let mut out = Vec::new();
        for m in meshes {
            let k = match self.materials.iter().position(|(name, _)| *name == m.material) {
                Some(k) => k,
                None => {
                    let i = self.materials.len();
                    let shader = self
                        .shader
                        .unwrap_or_else(|| *self.shaders.get(i).unwrap_or(self.shaders.last().expect("checked")));
                    let texture = self.textures.get(&m.material).cloned();
                    self.materials.push((m.material.clone(), NewMaterial { shader, texture }));
                    i
                }
            };
            out.extend(split(NewMesh {
                shader: k as u16,
                positions: m.positions,
                normals: m.normals,
                uvs: m.uvs,
                colors: Vec::new(),
                indices: m.indices,
            }));
        }
        out
    }
}

/// An Enhanced drawable with embedded textures, to lend its texture
/// structure to a model that has none: one next to `vpath` in its archive,
/// else any in that archive.
pub fn find_texture_donor(
    game_dir: &Path,
    vpath: &str,
    keys: Option<Arc<GtaVKeys>>,
) -> Option<rage_resource::Resource> {
    let parent = vpath.rsplit_once('/').map_or("", |(p, _)| p);
    let Ok((Node::ArchiveDir(rpf, dir), _)) = rage_mods::resolve(game_dir, parent, keys) else { return None };
    let donates = |r: &rage_resource::Resource| rage_formats::ydr_write::can_donate_textures(r);
    for c in rpf.children(dir) {
        if rpf.entry(c).name.to_ascii_lowercase().ends_with(".ydr")
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
            if found.is_some() || seen > 5000 || !p.to_ascii_lowercase().ends_with(".ydr") {
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
    fn fits_materials_to_shaders_and_splits_big_meshes() {
        let mesh = |name: &str| ObjMesh {
            material: name.into(),
            positions: vec![[0.0; 3]; 3],
            normals: Vec::new(),
            uvs: Vec::new(),
            indices: vec![0, 1, 2],
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
