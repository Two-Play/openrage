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
                let obj = inputs.iter().find(|i| i.picked).ok_or("choose an OBJ file")?;
                let model = parse_obj(&String::from_utf8_lossy(&obj.data))?;
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

fn export(vpath: &str) -> Result<Vec<OutputFile>, String> {
    let model = load_model(vpath)?;
    let name = file_name(vpath);
    let stem = safe(name.split('.').next().unwrap_or(name));

    let mut mtl = String::new();
    let mut files = Vec::new();
    let mut written: HashMap<String, Option<String>> = HashMap::new();
    for m in &model.materials {
        let (r, g, b, _) = m.color;
        let _ = writeln!(mtl, "newmtl {}\nKd {r:.4} {g:.4} {b:.4}", safe(&m.name));
        if !m.texture.is_empty() {
            let png = written.entry(m.texture.to_ascii_lowercase()).or_insert_with(|| {
                match load_texture(vpath, &m.texture).map_err(|e| e.to_string()).and_then(|img| encode_png(&img)) {
                    Ok(data) => {
                        let file = format!("{}.png", safe(&m.texture));
                        files.push(OutputFile::new(file.clone(), data));
                        Some(file)
                    }
                    Err(e) => {
                        log(Level::Warn, format!("texture {}: {e}", m.texture));
                        None
                    }
                }
            });
            if let Some(file) = png {
                let _ = writeln!(mtl, "map_Kd {file}");
            }
        }
        mtl.push('\n');
    }

    let mut obj = format!("# {name}, exported by OpenRAGE\nmtllib {stem}.mtl\n");
    let mut base = 1usize; // OBJ indices start at 1 and run across the file
    for (i, mesh) in model.meshes.iter().enumerate() {
        let n = mesh.positions.len() / 3;
        let part = if mesh.part.is_empty() { stem.clone() } else { safe(&mesh.part) };
        let _ = writeln!(obj, "o {part}_{i}\nusemtl {}", safe(&model.materials[mesh.material as usize].name));
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
                let k = base + k as usize;
                match (has_uv, has_n) {
                    (true, true) => {
                        let _ = write!(obj, " {k}/{k}/{k}");
                    }
                    (true, false) => {
                        let _ = write!(obj, " {k}/{k}");
                    }
                    (false, true) => {
                        let _ = write!(obj, " {k}//{k}");
                    }
                    (false, false) => {
                        let _ = write!(obj, " {k}");
                    }
                }
            }
            obj.push('\n');
        }
        base += n;
    }
    files.push(OutputFile::new(format!("{stem}.obj"), obj));
    files.push(OutputFile::new(format!("{stem}.mtl"), mtl));
    Ok(files)
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

/// Reads an OBJ file: `v`, `vt`, `vn`, `f` (polygons are fanned into
/// triangles, negative indices count from the end), `o`/`g` for parts and
/// `usemtl` for materials.
fn parse_obj(text: &str) -> Result<Model, String> {
    let (mut v, mut vt, mut vn) = (Vec::<[f32; 3]>::new(), Vec::<[f32; 2]>::new(), Vec::<[f32; 3]>::new());
    let mut materials: Vec<Material> = Vec::new();
    let mut meshes: Vec<Mesh> = Vec::new();
    // Per mesh: (position, uv, normal) index triple → vertex.
    let mut seen: HashMap<(usize, usize, usize), u32> = HashMap::new();
    let (mut part, mut material) = (String::new(), 0u32);
    let num = |s: Option<&str>, line: usize| -> Result<f32, String> {
        s.ok_or_else(|| format!("line {line}: number missing"))?
            .parse::<f32>()
            .map_err(|_| format!("line {line}: not a number"))
    };
    let resolve = |i: &str, len: usize, line: usize| -> Result<usize, String> {
        let k: i64 = i.parse().map_err(|_| format!("line {line}: bad index {i:?}"))?;
        let k = if k < 0 { len as i64 + k } else { k - 1 };
        if k < 0 || k as usize >= len {
            return Err(format!("line {line}: index {i} out of range"));
        }
        Ok(k as usize)
    };
    for (n, raw) in text.lines().enumerate() {
        let line = n + 1;
        let mut it = raw.split_whitespace();
        match it.next() {
            // Y-up → Z-up
            Some("v") => {
                let (x, y, z) = (num(it.next(), line)?, num(it.next(), line)?, num(it.next(), line)?);
                v.push([x, -z, y]);
            }
            Some("vt") => {
                let (u, t) = (num(it.next(), line)?, it.next().map_or(Ok(0.0), |s| num(Some(s), line))?);
                vt.push([u, 1.0 - t]);
            }
            Some("vn") => {
                let (x, y, z) = (num(it.next(), line)?, num(it.next(), line)?, num(it.next(), line)?);
                vn.push([x, -z, y]);
            }
            Some("o") | Some("g") => {
                part = it.collect::<Vec<_>>().join(" ");
                seen.clear();
            }
            Some("usemtl") => {
                let name = it.collect::<Vec<_>>().join(" ");
                material = match materials.iter().position(|m| m.name == name) {
                    Some(i) => i as u32,
                    None => {
                        // A stable colour per material name.
                        let h = name.bytes().fold(2166136261u32, |h, b| (h ^ u32::from(b)).wrapping_mul(16777619));
                        let c = |s: u32| 0.45 + 0.4 * ((h >> s) & 0xff) as f32 / 255.0;
                        materials.push(Material {
                            name,
                            texture: String::new(),
                            color: (c(0), c(8), c(16), 1.0),
                            image: None,
                        });
                        (materials.len() - 1) as u32
                    }
                };
                seen.clear();
            }
            Some("f") => {
                if materials.is_empty() {
                    materials.push(Material {
                        name: "default".into(),
                        texture: String::new(),
                        color: (0.7, 0.7, 0.7, 1.0),
                        image: None,
                    });
                }
                let need_new = meshes.last().is_none_or(|m| m.material != material || m.part != part);
                if need_new {
                    meshes.push(Mesh {
                        part: part.clone(),
                        positions: Vec::new(),
                        normals: Vec::new(),
                        uvs: Vec::new(),
                        indices: Vec::new(),
                        material,
                    });
                    seen.clear();
                }
                let mesh = meshes.last_mut().expect("just pushed");
                let mut corners = Vec::new();
                for corner in it {
                    let mut f = corner.split('/');
                    let p = resolve(f.next().unwrap_or(""), v.len(), line)?;
                    let t = match f.next() {
                        Some(s) if !s.is_empty() => resolve(s, vt.len(), line)? + 1,
                        _ => 0,
                    };
                    let nn = match f.next() {
                        Some(s) if !s.is_empty() => resolve(s, vn.len(), line)? + 1,
                        _ => 0,
                    };
                    let key = (p, t, nn);
                    let index = match seen.get(&key) {
                        Some(&i) => i,
                        None => {
                            let i = (mesh.positions.len() / 3) as u32;
                            mesh.positions.extend(v[p]);
                            mesh.uvs.extend(if t > 0 { vt[t - 1] } else { [0.0, 0.0] });
                            mesh.normals.extend(if nn > 0 { vn[nn - 1] } else { [0.0, 0.0, 1.0] });
                            seen.insert(key, i);
                            i
                        }
                    };
                    corners.push(index);
                }
                for k in 1..corners.len().saturating_sub(1) {
                    mesh.indices.extend([corners[0], corners[k], corners[k + 1]]);
                }
            }
            _ => {}
        }
    }
    if meshes.is_empty() {
        return Err("no faces in this OBJ file".into());
    }
    Ok(Model { meshes, materials })
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
