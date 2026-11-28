//! Example plugin: Wavefront OBJ in both directions.
//!
//! - Preview: `.obj` files in the 3D view. A plugin sees only the file it
//!   is handed, so materials get a colour from their name (the `.mtl` next
//!   to it is not read).
//! - Export: game models as `.obj` + `.mtl` + one PNG per texture, through
//!   `load-model` and `load-texture`: the plugin never parses game formats.
//! - Replace: a drawable's meshes with an OBJ, through `build-model` (which
//!   keeps the drawable's shaders and textures) and `write-mod-file`.
//!
//! The game is Z-up with DirectX texture coordinates (v down); OBJ is
//! usually Y-up with v up, so both are converted.

use std::collections::HashMap;
use std::fmt::Write as _;

use openrage_plugin_sdk::bindings::openrage::plugin::host::{build_model, load_model, load_texture};
use openrage_plugin_sdk::bindings::openrage::plugin::types::{Material, Mesh, Model};
use openrage_plugin_sdk::file_name;
use openrage_plugin_sdk::prelude::*;

struct ObjTools;

impl Plugin for ObjTools {
    fn preview(_vpath: String, data: Vec<u8>) -> Result<Preview, String> {
        let model = parse_obj(&String::from_utf8_lossy(&data))?;
        let vertices: usize = model.meshes.iter().map(|m| m.positions.len() / 3).sum();
        let triangles: usize = model.meshes.iter().map(|m| m.indices.len() / 3).sum();
        let materials = model.materials.len();
        Ok(Preview { info: Vec::new(), view: Some(View::Model(model)) }
            .with("Vertices", vertices)
            .with("Triangles", triangles)
            .with("Materials", materials))
    }

    fn run_action(action: String, inputs: Vec<Input>) -> Result<Vec<OutputFile>, String> {
        match action.as_str() {
            "export" => {
                let mut out = Vec::new();
                for input in inputs.iter().filter(|i| !i.picked) {
                    out.extend(export(&input.vpath)?);
                }
                Ok(out)
            }
            "replace" => {
                let picked = |ext: &'static str| {
                    inputs.iter().filter(move |i| i.picked && i.vpath.to_ascii_lowercase().ends_with(ext))
                };
                let obj = picked(".obj").next().ok_or("choose an OBJ file")?;
                let mut model = parse_obj(&String::from_utf8_lossy(&obj.data))?;
                let pngs: Vec<&Input> = picked(".png").collect();
                for mtl in picked(".mtl") {
                    apply_mtl(&mut model, &String::from_utf8_lossy(&mtl.data), &pngs);
                }
                for target in inputs.iter().filter(|i| !i.picked) {
                    // The drawable keeps its shaders and textures; OpenRAGE writes the meshes.
                    let file = build_model(&target.vpath, &model)?;
                    write_mod_file(&target.vpath, &file)?;
                    let triangles: usize = model.meshes.iter().map(|m| m.indices.len() / 3).sum();
                    log(
                        Level::Info,
                        format!(
                            "{}: {} meshes, {triangles} triangles from {}",
                            target.vpath,
                            model.meshes.len(),
                            obj.vpath
                        ),
                    );
                }
                Ok(Vec::new())
            }
            _ => Err(format!("unknown action {action:?}")),
        }
    }
}

openrage_plugin_sdk::export_plugin!(ObjTools);

/// A file name made of safe characters.
fn safe(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect()
}

/// The PNG file names written so far, by lower-case texture name (`None` for a texture that failed).
type Written = HashMap<String, Option<String>>;

fn export(vpath: &str) -> Result<Vec<OutputFile>, String> {
    let model = load_model(vpath)?;
    let name = file_name(vpath);
    let stem = safe(name.split('.').next().unwrap_or(name));

    let mut mtl = String::new();
    let mut files = Vec::new();
    let mut written = Written::new();
    for m in &model.materials {
        write_material(&mut mtl, vpath, m, &mut files, &mut written);
    }

    let mut obj = format!("# {name}, exported by OpenRAGE\nmtllib {stem}.mtl\n");
    let mut base = 1usize; // OBJ indices start at 1 and run across the file
    for (i, mesh) in model.meshes.iter().enumerate() {
        let part = if mesh.part.is_empty() { stem.clone() } else { safe(&mesh.part) };
        let _ = writeln!(obj, "o {part}_{i}\nusemtl {}", safe(&model.materials[mesh.material as usize].name));
        write_mesh(&mut obj, mesh, base);
        base += mesh.positions.len() / 3;
    }
    files.push(OutputFile::new(format!("{stem}.obj"), obj));
    files.push(OutputFile::new(format!("{stem}.mtl"), mtl));
    Ok(files)
}

/// A material's entry in the `.mtl`; its texture is exported as PNG the first time it comes up.
fn write_material(mtl: &mut String, vpath: &str, m: &Material, files: &mut Vec<OutputFile>, written: &mut Written) {
    let (r, g, b, _) = m.color;
    let _ = writeln!(mtl, "newmtl {}\nKd {r:.4} {g:.4} {b:.4}", safe(&m.name));
    if !m.texture.is_empty() {
        let png =
            written.entry(m.texture.to_ascii_lowercase()).or_insert_with(|| export_texture(vpath, &m.texture, files));
        if let Some(file) = png {
            let _ = writeln!(mtl, "map_Kd {file}");
        }
    }
    mtl.push('\n');
}

/// Adds a texture as a PNG file and gives its name; a texture that cannot be read is logged and skipped.
fn export_texture(vpath: &str, texture: &str, files: &mut Vec<OutputFile>) -> Option<String> {
    match load_texture(vpath, texture).map_err(|e| e.to_string()).and_then(|img| encode_png(&img)) {
        Ok(data) => {
            let file = format!("{}.png", safe(texture));
            files.push(OutputFile::new(file.clone(), data));
            Some(file)
        }
        Err(e) => {
            log(Level::Warn, format!("texture {texture}: {e}"));
            None
        }
    }
}

/// The vertices and faces of a mesh; face indices start after the `base` vertices before it.
fn write_mesh(obj: &mut String, mesh: &Mesh, base: usize) {
    for p in mesh.positions.as_chunks::<3>().0 {
        // Z-up → Y-up
        let _ = writeln!(obj, "v {} {} {}", p[0], p[2], -p[1]);
    }
    for t in mesh.uvs.as_chunks::<2>().0 {
        let _ = writeln!(obj, "vt {} {}", t[0], 1.0 - t[1]);
    }
    for v in mesh.normals.as_chunks::<3>().0 {
        let _ = writeln!(obj, "vn {} {} {}", v[0], v[2], -v[1]);
    }
    let (has_uv, has_n) = (!mesh.uvs.is_empty(), !mesh.normals.is_empty());
    for tri in mesh.indices.as_chunks::<3>().0 {
        obj.push('f');
        for &k in tri {
            write_corner(obj, base + k as usize, has_uv, has_n);
        }
        obj.push('\n');
    }
}

/// One corner of a face: the vertex, and the same index for the uv and normal that exist.
fn write_corner(obj: &mut String, k: usize, has_uv: bool, has_n: bool) {
    let _ = match (has_uv, has_n) {
        (true, true) => write!(obj, " {k}/{k}/{k}"),
        (true, false) => write!(obj, " {k}/{k}"),
        (false, true) => write!(obj, " {k}//{k}"),
        (false, false) => write!(obj, " {k}"),
    };
}

/// Colours and textures from a `.mtl`: a material's `map_Kd` becomes its
/// pixels when that PNG was picked too; a material with only `Kd` gets a
/// small texture of that colour.
fn apply_mtl(model: &mut Model, mtl: &str, pngs: &[&Input]) {
    let mut current: Option<usize> = None;
    for line in mtl.lines() {
        let line = line.trim();
        if let Some(name) = line.strip_prefix("newmtl ") {
            current = model.materials.iter().position(|m| m.name == name.trim());
        } else if let (Some(k), Some(rest)) = (current, line.strip_prefix("Kd ")) {
            let v: Vec<f32> = rest.split_whitespace().filter_map(|x| x.parse().ok()).collect();
            if v.len() >= 3 {
                let m = &mut model.materials[k];
                m.color = (v[0], v[1], v[2], 1.0);
                if m.image.is_none() {
                    let px = [v[0], v[1], v[2]].map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8);
                    m.image = Some(Image {
                        width: 4,
                        height: 4,
                        rgba: (0..16).flat_map(|_| [px[0], px[1], px[2], 255]).collect(),
                    });
                    m.texture = format!("{}_colour", m.name);
                }
            }
        } else if let (Some(k), Some(rest)) = (current, line.strip_prefix("map_Kd ")) {
            let Some(file) = rest.split_whitespace().last() else { continue };
            let base = file.rsplit(['/', '\\']).next().unwrap_or(file);
            match pngs.iter().find(|p| file_name(&p.vpath).eq_ignore_ascii_case(base)).map(|p| decode_png(&p.data)) {
                Some(Ok(img)) => {
                    let m = &mut model.materials[k];
                    m.texture = base.rsplit_once('.').map_or(base, |(s, _)| s).to_owned();
                    m.image = Some(img);
                }
                Some(Err(e)) => log(Level::Warn, format!("{base}: {e}")),
                None => log(
                    Level::Warn,
                    format!("{base} was not picked; material {} keeps the model's texture", model.materials[k].name),
                ),
            }
        }
    }
}

fn decode_png(data: &[u8]) -> Result<Image, String> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(data));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; reader.output_buffer_size().ok_or("PNG too large")?];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let px = &buf[..info.buffer_size()];
    let rgba = match info.color_type {
        png::ColorType::Rgba => px.to_vec(),
        png::ColorType::Rgb => px.as_chunks::<3>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => px.as_chunks::<2>().0.iter().flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Grayscale => px.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return Err("indexed PNG".into()),
    };
    Ok(Image { width: info.width, height: info.height, rgba })
}

fn encode_png(img: &Image) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, img.width, img.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(&img.rgba).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())?;
    Ok(out)
}

fn obj_number(s: Option<&str>, line: usize) -> Result<f32, String> {
    s.ok_or_else(|| format!("line {line}: number missing"))?
        .parse::<f32>()
        .map_err(|_| format!("line {line}: not a number"))
}

/// Three numbers, turned from Y-up to Z-up.
fn obj_vector(it: &mut std::str::SplitWhitespace<'_>, line: usize) -> Result<[f32; 3], String> {
    let (x, y, z) = (obj_number(it.next(), line)?, obj_number(it.next(), line)?, obj_number(it.next(), line)?);
    Ok([x, -z, y])
}

/// Two numbers (the second optional), with v flipped.
fn obj_uv(it: &mut std::str::SplitWhitespace<'_>, line: usize) -> Result<[f32; 2], String> {
    let (u, t) = (obj_number(it.next(), line)?, it.next().map_or(Ok(0.0), |s| obj_number(Some(s), line))?);
    Ok([u, 1.0 - t])
}

/// A 1-based (or negative, from the end) index into a list of `len` items.
fn obj_index(i: &str, len: usize, line: usize) -> Result<usize, String> {
    let k: i64 = i.parse().map_err(|_| format!("line {line}: bad index {i:?}"))?;
    let k = if k < 0 { len as i64 + k } else { k - 1 };
    if k < 0 || k as usize >= len {
        return Err(format!("line {line}: index {i} out of range"));
    }
    Ok(k as usize)
}

/// An optional face index plus one (0 when absent or empty).
fn optional_index(part: Option<&str>, len: usize, line: usize) -> Result<usize, String> {
    match part {
        Some(s) if !s.is_empty() => Ok(obj_index(s, len, line)? + 1),
        _ => Ok(0),
    }
}

const FNV_OFFSET_BASIS: u32 = 2166136261;
const FNV_PRIME: u32 = 16777619;

/// The vertex data of the file so far.
#[derive(Default)]
struct ObjSource {
    v: Vec<[f32; 3]>,
    vt: Vec<[f32; 2]>,
    vn: Vec<[f32; 3]>,
}

impl ObjSource {
    /// The vertex of a `p/t/n` corner, added to `mesh` unless `seen` has it.
    fn corner(
        &self,
        seen: &mut HashMap<(usize, usize, usize), u32>,
        mesh: &mut Mesh,
        corner: &str,
        line: usize,
    ) -> Result<u32, String> {
        let mut f = corner.split('/');
        let p = obj_index(f.next().unwrap_or(""), self.v.len(), line)?;
        let t = optional_index(f.next(), self.vt.len(), line)?;
        let nn = optional_index(f.next(), self.vn.len(), line)?;
        let key = (p, t, nn);
        if let Some(&i) = seen.get(&key) {
            return Ok(i);
        }
        let i = (mesh.positions.len() / 3) as u32;
        mesh.positions.extend(self.v[p]);
        mesh.uvs.extend(if t > 0 { self.vt[t - 1] } else { [0.0, 0.0] });
        mesh.normals.extend(if nn > 0 { self.vn[nn - 1] } else { [0.0, 0.0, 1.0] });
        seen.insert(key, i);
        Ok(i)
    }
}

#[derive(Default)]
struct ObjParser {
    src: ObjSource,
    materials: Vec<Material>,
    meshes: Vec<Mesh>,
    /// Per mesh: (position, uv, normal) index triple → vertex.
    seen: HashMap<(usize, usize, usize), u32>,
    part: String,
    material: u32,
}

impl ObjParser {
    fn line(&mut self, line: usize, raw: &str) -> Result<(), String> {
        let mut it = raw.split_whitespace();
        match it.next() {
            Some(kind @ ("v" | "vt" | "vn")) => self.vertex_line(kind, &mut it, line)?,
            Some("o" | "g") => {
                self.part = it.collect::<Vec<_>>().join(" ");
                self.seen.clear();
            }
            Some("usemtl") => {
                self.use_material(it.collect::<Vec<_>>().join(" "));
                self.seen.clear();
            }
            Some("f") => self.face(it, line)?,
            _ => {}
        }
        Ok(())
    }

    fn vertex_line(&mut self, kind: &str, it: &mut std::str::SplitWhitespace<'_>, line: usize) -> Result<(), String> {
        match kind {
            "v" => self.src.v.push(obj_vector(it, line)?),
            "vt" => self.src.vt.push(obj_uv(it, line)?),
            _ => self.src.vn.push(obj_vector(it, line)?),
        }
        Ok(())
    }

    fn use_material(&mut self, name: String) {
        self.material = match self.materials.iter().position(|m| m.name == name) {
            Some(i) => i as u32,
            None => {
                // A stable colour per material name.
                let h = name.bytes().fold(FNV_OFFSET_BASIS, |h, b| (h ^ u32::from(b)).wrapping_mul(FNV_PRIME));
                let c = |s: u32| 0.45 + 0.4 * ((h >> s) & 0xff) as f32 / 255.0;
                self.materials.push(Material {
                    name,
                    texture: String::new(),
                    color: (c(0), c(8), c(16), 1.0),
                    image: None,
                });
                (self.materials.len() - 1) as u32
            }
        };
    }

    fn face(&mut self, corners: std::str::SplitWhitespace<'_>, line: usize) -> Result<(), String> {
        if self.materials.is_empty() {
            self.materials.push(Material {
                name: "default".into(),
                texture: String::new(),
                color: (0.7, 0.7, 0.7, 1.0),
                image: None,
            });
        }
        let (part, material) = (&self.part, self.material);
        let need_new = self.meshes.last().is_none_or(|m| m.material != material || m.part != *part);
        if need_new {
            self.meshes.push(Mesh {
                part: self.part.clone(),
                positions: Vec::new(),
                normals: Vec::new(),
                uvs: Vec::new(),
                indices: Vec::new(),
                material,
            });
            self.seen.clear();
        }
        let mesh = self.meshes.last_mut().expect("just pushed");
        let mut indices = Vec::new();
        for corner in corners {
            indices.push(self.src.corner(&mut self.seen, mesh, corner, line)?);
        }
        for k in 1..indices.len().saturating_sub(1) {
            mesh.indices.extend([indices[0], indices[k], indices[k + 1]]);
        }
        Ok(())
    }
}

/// Reads an OBJ file: `v`, `vt`, `vn`, `f` (polygons are fanned into
/// triangles, negative indices count from the end), `o`/`g` for parts and
/// `usemtl` for materials.
fn parse_obj(text: &str) -> Result<Model, String> {
    let mut parser = ObjParser::default();
    for (n, raw) in text.lines().enumerate() {
        parser.line(n + 1, raw)?;
    }
    if parser.meshes.is_empty() {
        return Err("no faces in this OBJ file".into());
    }
    Ok(Model { meshes: parser.meshes, materials: parser.materials })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_quad_with_two_materials() {
        let obj = "v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nvt 0 0\nvt 1 1\nusemtl red\nf 1/1 2/2 3/2 4/1\nusemtl blue\nf -1 -2 -3\n";
        let model = parse_obj(obj).unwrap();
        assert_eq!(model.materials.len(), 2);
        assert_eq!(model.meshes.len(), 2);
        assert_eq!(model.meshes[0].indices, [0, 1, 2, 0, 2, 3]);
        // Y-up → Z-up: (1, 1, 0) becomes (1, 0, 1); v flipped.
        assert_eq!(&model.meshes[0].positions[6..9], &[1.0, 0.0, 1.0]);
        assert_eq!(&model.meshes[0].uvs[0..2], &[0.0, 1.0]);
        assert!(parse_obj("v 0 0 0\nf 1 2 3").is_err());
    }
}
