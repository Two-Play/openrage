//! Export of drawables as binary glTF 2.0 (`.glb`), with textures embedded as PNG.
//!
//! GTA V is Z-up (x right, y forward); glTF is Y-up (x right, -z forward), so
//! positions and normals are converted as `(x, y, z) → (x, z, -y)`.

use std::fmt::Write;

use crate::drawable::Drawable;
use crate::texture::Texture;

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

fn finite(v: f32) -> f32 {
    if v.is_finite() { v } else { 0.0 }
}

/// Collects binary data and the JSON fragments describing it.
#[derive(Default)]
struct Builder {
    bin: Vec<u8>,
    views: Vec<String>,
    accessors: Vec<String>,
}

impl Builder {
    fn view(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        let offset = self.bin.len();
        self.bin.extend_from_slice(bytes);
        let target = target.map(|t| format!(",\"target\":{t}")).unwrap_or_default();
        self.views.push(format!("{{\"buffer\":0,\"byteOffset\":{offset},\"byteLength\":{}{target}}}", bytes.len()));
        self.views.len() - 1
    }

    fn floats<const N: usize>(&mut self, data: &[[f32; N]], kind: &str, bounds: bool) -> usize {
        let bytes: Vec<u8> = data.iter().flatten().flat_map(|v| finite(*v).to_le_bytes()).collect();
        let view = self.view(&bytes, Some(34962));
        let mut extra = String::new();
        if bounds {
            let mut min = [f32::MAX; N];
            let mut max = [f32::MIN; N];
            for v in data {
                for i in 0..N {
                    min[i] = min[i].min(finite(v[i]));
                    max[i] = max[i].max(finite(v[i]));
                }
            }
            let fmt = |a: [f32; N]| a.iter().map(|v| format!("{v}")).collect::<Vec<_>>().join(",");
            extra = format!(",\"min\":[{}],\"max\":[{}]", fmt(min), fmt(max));
        }
        self.accessors.push(format!(
            "{{\"bufferView\":{view},\"componentType\":5126,\"count\":{},\"type\":\"{kind}\"{extra}}}",
            data.len()
        ));
        self.accessors.len() - 1
    }

    fn indices(&mut self, data: &[u32]) -> usize {
        let bytes: Vec<u8> = data.iter().flat_map(|v| v.to_le_bytes()).collect();
        let view = self.view(&bytes, Some(34963));
        self.accessors.push(format!(
            "{{\"bufferView\":{view},\"componentType\":5125,\"count\":{},\"type\":\"SCALAR\"}}",
            data.len()
        ));
        self.accessors.len() - 1
    }
}

/// Writes a drawable as `.glb`. `lookup` resolves texture names (embedded
/// dictionary, a `.ytd` next to the model, …); missing textures leave the
/// material untextured.
pub fn write_glb<'a>(d: &Drawable, name: &str, lookup: &dyn Fn(&str) -> Option<&'a Texture>) -> Vec<u8> {
    let mut b = Builder::default();
    let mut materials = Vec::new();
    let mut images = Vec::new();
    let mut textures = Vec::new();
    let mut image_of: Vec<(String, usize)> = Vec::new();

    for (si, shader) in d.shaders.iter().enumerate() {
        let tex_name = shader.base_texture().map(str::to_owned);
        let mut pbr = String::from("\"metallicFactor\":0,\"roughnessFactor\":0.8");
        if let Some(tn) = &tex_name {
            let existing = image_of.iter().find(|(n, _)| n.eq_ignore_ascii_case(tn)).map(|(_, i)| *i);
            let tex_index = existing.or_else(|| {
                let t = lookup(tn)?;
                let rgba = t.to_rgba()?;
                let png = crate::png(t.width, t.height, &rgba).ok()?;
                let view = b.view(&png, None);
                images.push(format!("{{\"name\":\"{}\",\"bufferView\":{view},\"mimeType\":\"image/png\"}}", esc(tn)));
                textures.push(format!("{{\"source\":{},\"sampler\":0}}", images.len() - 1));
                image_of.push((tn.clone(), textures.len() - 1));
                Some(textures.len() - 1)
            });
            if let Some(ti) = tex_index {
                pbr.push_str(&format!(",\"baseColorTexture\":{{\"index\":{ti}}}"));
            }
        }
        let label = tex_name.unwrap_or_else(|| format!("shader_{si}"));
        materials.push(format!(
            "{{\"name\":\"{}\",\"pbrMetallicRoughness\":{{{pbr}}},\"alphaMode\":\"MASK\",\"alphaCutoff\":0.3,\"doubleSided\":true}}",
            esc(&label)
        ));
    }

    let mut primitives = Vec::new();
    for mesh in &d.meshes {
        if mesh.positions.is_empty() || mesh.indices.is_empty() {
            continue;
        }
        let positions: Vec<[f32; 3]> = mesh.positions.iter().map(|p| [p[0], p[2], -p[1]]).collect();
        let mut attrs = format!("\"POSITION\":{}", b.floats(&positions, "VEC3", true));
        if mesh.normals.len() == mesh.positions.len() {
            let normals: Vec<[f32; 3]> = mesh
                .normals
                .iter()
                .map(|n| {
                    let v = [n[0], n[2], -n[1]];
                    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
                    if len > 1e-6 { [v[0] / len, v[1] / len, v[2] / len] } else { [0.0, 1.0, 0.0] }
                })
                .collect();
            attrs.push_str(&format!(",\"NORMAL\":{}", b.floats(&normals, "VEC3", false)));
        }
        if mesh.uvs.len() == mesh.positions.len() {
            attrs.push_str(&format!(",\"TEXCOORD_0\":{}", b.floats(&mesh.uvs, "VEC2", false)));
        }
        if mesh.colors.len() == mesh.positions.len() {
            attrs.push_str(&format!(",\"COLOR_0\":{}", b.floats(&mesh.colors, "VEC4", false)));
        }
        let idx = b.indices(&mesh.indices);
        let material = if (mesh.shader as usize) < materials.len() {
            format!(",\"material\":{}", mesh.shader)
        } else {
            String::new()
        };
        primitives.push(format!("{{\"attributes\":{{{attrs}}},\"indices\":{idx}{material}}}"));
    }

    let mut json = String::new();
    let _ = write!(
        json,
        "{{\"asset\":{{\"version\":\"2.0\",\"generator\":\"OpenRAGE\"}},\"scene\":0,\"scenes\":[{{\"nodes\":[0]}}],\
         \"nodes\":[{{\"name\":\"{n}\",\"mesh\":0}}],\"meshes\":[{{\"name\":\"{n}\",\"primitives\":[{}]}}]",
        primitives.join(","),
        n = esc(name)
    );
    if !materials.is_empty() {
        let _ = write!(json, ",\"materials\":[{}]", materials.join(","));
    }
    if !textures.is_empty() {
        let _ = write!(
            json,
            ",\"textures\":[{}],\"images\":[{}],\"samplers\":[{{\"magFilter\":9729,\"minFilter\":9987,\"wrapS\":10497,\"wrapT\":10497}}]",
            textures.join(","),
            images.join(",")
        );
    }
    while !b.bin.len().is_multiple_of(4) {
        b.bin.push(0);
    }
    let _ = write!(
        json,
        ",\"accessors\":[{}],\"bufferViews\":[{}],\"buffers\":[{{\"byteLength\":{}}}]}}",
        b.accessors.join(","),
        b.views.join(","),
        b.bin.len()
    );
    let mut json = json.into_bytes();
    while !json.len().is_multiple_of(4) {
        json.push(b' ');
    }

    let total = 12 + 8 + json.len() + 8 + b.bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&json);
    out.extend_from_slice(&(b.bin.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&b.bin);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drawable::{Mesh, Shader};

    #[test]
    fn writes_valid_glb_structure() {
        let d = Drawable {
            meshes: vec![Mesh {
                positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                normals: vec![[0.0, 0.0, 1.0]; 3],
                uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
                colors: Vec::new(),
                indices: vec![0, 1, 2],
                shader: 0,
            }],
            shaders: vec![Shader { name_hash: 1, textures: vec!["body".into()] }],
            ..Default::default()
        };
        let glb = write_glb(&d, "tri", &|_| None);
        assert_eq!(&glb[0..4], b"glTF");
        assert_eq!(u32::from_le_bytes(glb[8..12].try_into().unwrap()) as usize, glb.len());
        let json_len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        let json = std::str::from_utf8(&glb[20..20 + json_len]).unwrap();
        assert!(json.contains("\"POSITION\"") && json.contains("\"NORMAL\"") && json.contains("\"material\":0"));
        // Z-up → Y-up: the vertex at y=1 ends up at z=-1.
        assert!(json.contains("\"min\":[0,0,-1]"), "{json}");
    }
}
