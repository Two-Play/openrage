//! Writing drawables (`.ydr`) of GTA V Enhanced (resource version 159) with
//! new geometry, on top of an existing drawable as the template.
//!
//! Shaders are the hard part of a drawable: each one expects its own
//! parameter blocks and vertex layout. So nothing of that is invented here:
//! the template's system and graphics memory is kept as it is, at the same
//! addresses (the resource builder puts the first block of each segment at
//! offset 0, so every pointer inside stays valid). New geometries, vertex and
//! index buffers are added behind it, each copied from a template geometry
//! with the same shader (and so its vertex layout), and the drawable's high
//! detail model, level-of-detail lists and bounds are pointed at them. The
//! old geometry stays in the file unused.
//!
//! Enhanced layout used here (measured on the game's drawables):
//! * drawable: `0x08` pages info, `0x20` bounding sphere (centre, radius),
//!   `0x30`/`0x40` bounding box (`w` kept), `0x50..0x70` model lists of the
//!   four detail levels, `0x70` their distances;
//! * model list: pointer to the model pointers, count and capacity (u16);
//! * model (0x30): `0x08` geometry pointers, `0x10` count and capacity,
//!   `0x18` bounding boxes (one per geometry, after one around all of them
//!   when there are several; `w` repeats `x`), `0x20` shader index per
//!   geometry (u16), `0x28` skeleton binding;
//! * geometry (0xA0): `0x18` vertex buffer, `0x38` index buffer, `0x58`
//!   index count, `0x5C` triangle count, `0x60` vertex count (u16), `0x70`
//!   stride, `0x78` vertex data;
//! * vertex buffer (0x40): `0x08` count, `0x0C` stride, `0x18` data, `0x30` a
//!   0x20-byte buffer object, `0x38` declaration; index buffer (0x40): `0x08`
//!   count, `0x0C` index size, `0x18` data, `0x30` buffer object;
//! * declaration: 52 offsets (u32), 52 bytes (the stride), 52 DXGI formats;
//!   slots 0 position, 4 normal, 8 tangent, 16/20 blend weights/indices,
//!   24/25 colours, 28–35 texture coordinates;
//! * shader group: `0x08` embedded texture dictionary, `0x10` shader
//!   pointers, `0x18` count and capacity (u16);
//! * shader (0x40): `0x08` constant buffers, `0x10` texture references (one
//!   pointer per texture slot), `0x20` parameter info: number of textures
//!   at 1, of parameters at 4, then (name hash, slot) pairs from 8 (slot kind
//!   in the low two bits, 0 = texture; the rest is the texture index);
//! * texture reference (0x50): a texture without data, `0x28` its name; the
//!   game finds it by name, first in the embedded dictionary;
//! * embedded dictionary: like a `.ytd`'s root (0x40): `0x20` name hashes
//!   (sorted), `0x30` texture pointers, counts at `0x28`/`0x2A`/`0x38`/`0x3A`.
//!
//! With [`NewMaterial`]s, every material gets its own copy of a template
//! shader (so several materials can use the same kind of shader), and
//! materials with a texture get it embedded and bound to the copy's colour
//! slot.

use rage_resource::builder::{BlockId, Builder, Segment};
use rage_resource::{Resource, ResourceError};

use crate::drawable::{Gen, VERSION_YDR_ENHANCED, param};
use crate::texture::{Dimension, Format, Texture};
use crate::ytd::Layout as DictLayout;
use crate::ytd_write::{guess_usage, name_hash, write_fields};

const SYS: u64 = 0x5000_0000;
const GFX: u64 = 0x6000_0000;

const MODEL_SIZE: usize = 0x30;
const GEOMETRY_SIZE: usize = 0xA0;
const BUFFER_SIZE: usize = 0x40;
const BUFFER_OBJECT_SIZE: usize = 0x20;
const SHADER_SIZE: usize = 0x40;
const TEXTURE_REF_SIZE: usize = 0x50;
const DICTIONARY_ROOT_SIZE: usize = 0x40;

/// Most vertices of one geometry (16-bit indices).
pub const MAX_VERTICES: usize = 0xFFFF;

/// A material of the new model: drawn with a copy of one of the template's
/// shaders, optionally with a colour texture of its own.
#[derive(Debug, Clone)]
pub struct NewMaterial {
    /// The template shader to copy.
    pub shader: u16,
    /// Embedded and bound to the copy's colour slot ([`texture_from_rgba`]).
    pub texture: Option<Texture>,
}

/// A colour texture from 8-bit RGBA pixels: BGRA with all mip levels (each
/// half the size, box filtered).
pub fn texture_from_rgba(name: &str, width: u32, height: u32, rgba: &[u8]) -> Result<Texture, ResourceError> {
    if width == 0 || height == 0 || width > 8192 || height > 8192 || rgba.len() != (width * height * 4) as usize {
        return Err(bad(format!("texture {name}: {width}×{height} with {} bytes of pixels", rgba.len())));
    }
    let levels = 32 - width.min(height).leading_zeros();
    let mut data = Vec::new();
    let (mut w, mut h) = (width as usize, height as usize);
    let mut level: Vec<u8> = rgba.to_vec();
    for l in 0..levels {
        if l > 0 {
            let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
            let mut next = vec![0u8; nw * nh * 4];
            for y in 0..nh {
                for x in 0..nw {
                    for c in 0..4 {
                        let mut sum = 0u32;
                        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                            let (sx, sy) = ((x * 2 + dx).min(w - 1), (y * 2 + dy).min(h - 1));
                            sum += u32::from(level[(sy * w + sx) * 4 + c]);
                        }
                        next[(y * nw + x) * 4 + c] = ((sum + 2) / 4) as u8;
                    }
                }
            }
            level = next;
            (w, h) = (nw, nh);
        }
        // RGBA → BGRA
        data.extend(level.as_chunks::<4>().0.iter().flat_map(|p| [p[2], p[1], p[0], p[3]]));
    }
    Ok(Texture {
        name: name.to_owned(),
        width,
        height,
        depth: 1,
        levels,
        format: Format::B8G8R8A8_UNORM,
        dimension: Dimension::Texture2D,
        usage: guess_usage(name),
        data,
    })
}

/// A mesh to write: triangles of one shader of the template.
#[derive(Debug, Clone, Default)]
pub struct NewMesh {
    /// Index into the template's shaders.
    pub shader: u16,
    pub positions: Vec<[f32; 3]>,
    /// One per vertex, or empty (then computed from the faces).
    pub normals: Vec<[f32; 3]>,
    /// One per vertex (DirectX convention: v down), or empty.
    pub uvs: Vec<[f32; 2]>,
    /// RGBA 0–1 per vertex, or empty (white).
    pub colors: Vec<[f32; 4]>,
    /// Three per triangle.
    pub indices: Vec<u32>,
}

fn bad(msg: impl Into<String>) -> ResourceError {
    ResourceError::Format(msg.into())
}

/// A geometry of the template to copy for one shader.
struct TemplateGeometry {
    geometry: u64,
    vertex_buffer: u64,
    index_buffer: u64,
    model: u64,
    stride: usize,
    layout: Layout,
}

/// The slots of a vertex declaration that hold something.
#[derive(Debug, Clone)]
struct Layout {
    /// (slot, byte offset, DXGI format)
    slots: Vec<(usize, usize, u8)>,
}

fn format_size(format: u8) -> Option<usize> {
    Some(match format {
        2 => 16,                // R32G32B32A32_FLOAT
        6 => 12,                // R32G32B32_FLOAT
        10 => 8,                // R16G16B16A16_FLOAT
        16 => 8,                // R32G32_FLOAT
        24 | 28 | 30 | 34 => 4, // R10G10B10A2_UNORM, R8G8B8A8_UNORM, R8G8B8A8_UINT, R16G16_FLOAT
        _ => return None,
    })
}

fn read_layout(res: &Resource, decl: u64, stride: usize) -> Result<Layout, ResourceError> {
    let offsets = res.bytes(decl, 52 * 4)?;
    let formats = res.bytes(decl + 52 * 4 + 52, 52)?;
    let mut slots = Vec::new();
    for slot in 0..52 {
        let f = formats[slot];
        if f == 0 {
            continue;
        }
        let off = u32::from_le_bytes(offsets[slot * 4..slot * 4 + 4].try_into().unwrap()) as usize;
        let size =
            format_size(f).ok_or_else(|| bad(format!("vertex slot {slot} has format {f}, which cannot be written")))?;
        if off + size > stride {
            return Err(bad(format!("vertex slot {slot} lies outside the vertex")));
        }
        slots.push((slot, off, f));
    }
    if !slots.iter().any(|&(s, _, f)| s == 0 && f == 6) {
        return Err(bad("the template's vertices have no float3 positions"));
    }
    Ok(Layout { slots })
}

/// The template's geometries, one per shader index (the first found).
fn template_geometries(res: &Resource) -> Result<Vec<(u16, TemplateGeometry)>, ResourceError> {
    let mut out: Vec<(u16, TemplateGeometry)> = Vec::new();
    for lod in 0..4 {
        let list = res.u64(SYS + 0x50 + lod * 8)?;
        if list == 0 {
            continue;
        }
        let models = res.u64(list)?;
        for m in 0..res.u16(list + 8)? as u64 {
            let model = res.u64(models + m * 8)?;
            if model == 0 {
                continue;
            }
            let binding = res.u32(model + 0x28)?;
            if (binding >> 8) & 0xFF != 0 {
                continue; // skinned: needs bone data this writer does not produce
            }
            let geoms = res.u64(model + 0x08)?;
            let mapping = res.u64(model + 0x20)?;
            for g in 0..res.u16(model + 0x10)? as u64 {
                let geometry = res.u64(geoms + g * 8)?;
                let shader = if mapping != 0 { res.u16(mapping + g * 2)? } else { 0 };
                if out.iter().any(|(s, _)| *s == shader) || res.u64(geometry + 0x68)? != 0 {
                    continue;
                }
                let vertex_buffer = res.u64(geometry + 0x18)?;
                let index_buffer = res.u64(geometry + 0x38)?;
                let stride = res.u32(vertex_buffer + 0x0C)? as usize;
                let layout = read_layout(res, res.u64(vertex_buffer + 0x38)?, stride)?;
                out.push((shader, TemplateGeometry { geometry, vertex_buffer, index_buffer, model, stride, layout }));
            }
        }
    }
    Ok(out)
}

/// The shader indices of the template that meshes can use.
pub fn usable_shaders(template: &Resource) -> Result<Vec<u16>, ResourceError> {
    check_template(template)?;
    let mut s: Vec<u16> = template_geometries(template)?.into_iter().map(|(s, _)| s).collect();
    s.sort_unstable();
    Ok(s)
}

fn check_template(template: &Resource) -> Result<(), ResourceError> {
    if template.version != VERSION_YDR_ENHANCED {
        return Err(bad(format!(
            "the template is version {}; only GTA V Enhanced drawables (version {VERSION_YDR_ENHANCED}) can be written so far",
            template.version
        )));
    }
    let _ = Gen::Enhanced;
    Ok(())
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 1e-12 { [v[0] / l, v[1] / l, v[2] / l] } else { [0.0, 0.0, 1.0] }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Face normals averaged per vertex.
fn smooth_normals(m: &NewMesh) -> Vec<[f32; 3]> {
    let mut n = vec![[0.0f32; 3]; m.positions.len()];
    for t in m.indices.as_chunks::<3>().0 {
        let [a, b, c] = [t[0] as usize, t[1] as usize, t[2] as usize];
        let f = cross(sub(m.positions[b], m.positions[a]), sub(m.positions[c], m.positions[a]));
        for i in [a, b, c] {
            for k in 0..3 {
                n[i][k] += f[k];
            }
        }
    }
    n.into_iter().map(normalize).collect()
}

/// Tangents from the texture coordinates (`w` is the handedness).
fn tangents(m: &NewMesh, normals: &[[f32; 3]]) -> Vec<[f32; 4]> {
    let n = m.positions.len();
    if m.uvs.len() != n {
        return normals
            .iter()
            .map(|nv| {
                let t = normalize(cross(*nv, if nv[2].abs() < 0.9 { [0.0, 0.0, 1.0] } else { [1.0, 0.0, 0.0] }));
                [t[0], t[1], t[2], 1.0]
            })
            .collect();
    }
    let (mut tan, mut bit) = (vec![[0.0f32; 3]; n], vec![[0.0f32; 3]; n]);
    for t in m.indices.as_chunks::<3>().0 {
        let [a, b, c] = [t[0] as usize, t[1] as usize, t[2] as usize];
        let (e1, e2) = (sub(m.positions[b], m.positions[a]), sub(m.positions[c], m.positions[a]));
        let (du1, dv1) = (m.uvs[b][0] - m.uvs[a][0], m.uvs[b][1] - m.uvs[a][1]);
        let (du2, dv2) = (m.uvs[c][0] - m.uvs[a][0], m.uvs[c][1] - m.uvs[a][1]);
        let det = du1 * dv2 - du2 * dv1;
        if det.abs() < 1e-12 {
            continue;
        }
        let r = 1.0 / det;
        let sd = [(e1[0] * dv2 - e2[0] * dv1) * r, (e1[1] * dv2 - e2[1] * dv1) * r, (e1[2] * dv2 - e2[2] * dv1) * r];
        let td = [(e2[0] * du1 - e1[0] * du2) * r, (e2[1] * du1 - e1[1] * du2) * r, (e2[2] * du1 - e1[2] * du2) * r];
        for i in [a, b, c] {
            for k in 0..3 {
                tan[i][k] += sd[k];
                bit[i][k] += td[k];
            }
        }
    }
    (0..n)
        .map(|i| {
            let nv = normals[i];
            // Gram-Schmidt against the normal.
            let t = tan[i];
            let d = dot(nv, t);
            let t = normalize([t[0] - nv[0] * d, t[1] - nv[1] * d, t[2] - nv[2] * d]);
            let w = if dot(cross(nv, t), bit[i]) < 0.0 { -1.0 } else { 1.0 };
            [t[0], t[1], t[2], w]
        })
        .collect()
}

fn f16_bits(v: f32) -> u16 {
    let x = v.to_bits();
    let sign = ((x >> 16) & 0x8000) as u16;
    let exp = ((x >> 23) & 0xFF) as i32 - 127 + 15;
    let man = x & 0x7F_FFFF;
    if exp <= 0 {
        sign
    } else if exp >= 31 {
        sign | 0x7C00
    } else {
        sign | ((exp as u16) << 10) | ((man >> 13) as u16)
    }
}

fn encode(format: u8, v: [f32; 4], out: &mut [u8]) {
    let put_f32 = |out: &mut [u8], n: usize| {
        for (k, x) in v.iter().take(n).enumerate() {
            out[k * 4..k * 4 + 4].copy_from_slice(&x.to_le_bytes());
        }
    };
    match format {
        2 => put_f32(out, 4),
        6 => put_f32(out, 3),
        16 => put_f32(out, 2),
        10 | 34 => {
            let n = if format == 10 { 4 } else { 2 };
            for (k, x) in v.iter().take(n).enumerate() {
                out[k * 2..k * 2 + 2].copy_from_slice(&f16_bits(*x).to_le_bytes());
            }
        }
        28 => {
            for k in 0..4 {
                out[k] = (v[k].clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        }
        30 => {
            for k in 0..4 {
                out[k] = v[k].clamp(0.0, 255.0) as u8;
            }
        }
        24 => {
            let s10 = |x: f32| ((x.clamp(-1.0, 1.0) * 511.0).round() as i32 as u32) & 0x3FF;
            let a = (v[3].clamp(0.0, 1.0) * 3.0).round() as u32;
            let packed = s10(v[0]) | s10(v[1]) << 10 | s10(v[2]) << 20 | a << 30;
            out[..4].copy_from_slice(&packed.to_le_bytes());
        }
        _ => {}
    }
}

/// The vertices of a mesh in a template layout.
fn vertex_data(m: &NewMesh, layout: &Layout, stride: usize) -> Result<Vec<u8>, ResourceError> {
    let n = m.positions.len();
    let normals = if m.normals.len() == n { m.normals.clone() } else { smooth_normals(m) };
    let needs_tangents = layout.slots.iter().any(|&(s, _, _)| (8..12).contains(&s));
    let tangents = if needs_tangents { tangents(m, &normals) } else { Vec::new() };
    let mut data = vec![0u8; n * stride];
    for i in 0..n {
        let vertex = &mut data[i * stride..(i + 1) * stride];
        for &(slot, off, format) in &layout.slots {
            let p = m.positions[i];
            let nv = normals[i];
            let uv = m.uvs.get(i).copied().unwrap_or([0.0, 0.0]);
            let value = match slot {
                0..=3 => [p[0], p[1], p[2], 1.0],
                4..=7 => [nv[0], nv[1], nv[2], 0.0],
                8..=11 => tangents[i],
                // Blend data belongs to skinned meshes, which are not written.
                16..=23 => return Err(bad("the template's vertices carry bone weights (skinned)")),
                24 => m.colors.get(i).copied().unwrap_or([1.0; 4]),
                25..=27 => [1.0; 4],
                28..=35 => [uv[0], uv[1], 0.0, 0.0],
                _ => [0.0; 4],
            };
            encode(format, value, &mut vertex[off..]);
        }
    }
    Ok(data)
}

fn put_u16(d: &mut [u8], at: usize, v: u16) {
    d[at..at + 2].copy_from_slice(&v.to_le_bytes());
}
fn put_u32(d: &mut [u8], at: usize, v: u32) {
    d[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
fn put_f32(d: &mut [u8], at: usize, v: f32) {
    d[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// Bounds of a set of points: (min, max).
fn bounds<'a>(points: impl Iterator<Item = &'a [f32; 3]>) -> ([f32; 3], [f32; 3]) {
    let mut min = [f32::MAX; 3];
    let mut max = [f32::MIN; 3];
    for p in points {
        for k in 0..3 {
            min[k] = min[k].min(p[k]);
            max[k] = max[k].max(p[k]);
        }
    }
    (min, max)
}

/// Writes a box as the game stores it: min, `w` = min x, max, `w` = max x.
fn put_box(d: &mut [u8], at: usize, (min, max): ([f32; 3], [f32; 3])) {
    for k in 0..3 {
        put_f32(d, at + k * 4, min[k]);
        put_f32(d, at + 16 + k * 4, max[k]);
    }
    put_f32(d, at + 12, min[0]);
    put_f32(d, at + 28, max[0]);
}

/// A drawable with `meshes` as its geometry, built on `template` (an Enhanced
/// `.ydr`, read with [`Resource`]). Every mesh uses one of the template's
/// shaders ([`usable_shaders`]); the result has one detail level.
pub fn replace_meshes(template: &Resource, meshes: &[NewMesh]) -> Result<Vec<u8>, ResourceError> {
    build(template, &[meshes], None, None)
}

/// Like [`replace_meshes`], but every mesh's `shader` is an index into
/// `materials`: each material gets its own copy of a template shader, with
/// its texture embedded. Textures need a texture structure to copy: from the
/// template's embedded dictionary, else from `donor` (an Enhanced drawable
/// with one, or an Enhanced `.ydr`/`.ytd` resource).
pub fn replace_with_materials(
    template: &Resource,
    meshes: &[NewMesh],
    materials: &[NewMaterial],
    donor: Option<&Resource>,
) -> Result<Vec<u8>, ResourceError> {
    build(template, &[meshes], Some(materials), donor)
}

/// Like [`replace_with_materials`] (or [`replace_meshes`] without
/// `materials`), with one to four detail levels: `lods[0]` is shown close
/// up, each further one from further away. Distances come from the
/// template's levels where it has them (else 50 m, doubling per level); the
/// last level is seen as far as the template's furthest.
pub fn replace_lods(
    template: &Resource,
    lods: &[&[NewMesh]],
    materials: Option<&[NewMaterial]>,
    donor: Option<&Resource>,
) -> Result<Vec<u8>, ResourceError> {
    build(template, lods, materials, donor)
}

/// Whether `res` (an Enhanced drawable or texture dictionary) has a texture
/// structure that [`replace_with_materials`] can copy (as the `donor`).
pub fn can_donate_textures(res: &Resource) -> bool {
    texture_templates(res).is_some()
}

/// The first texture structure and dictionary root of an Enhanced resource:
/// its embedded dictionary (drawable) or its root (texture dictionary).
fn texture_templates(res: &Resource) -> Option<(Vec<u8>, Vec<u8>)> {
    let root = if res.version == VERSION_YDR_ENHANCED {
        let group = res.u64(SYS + 0x10).ok().filter(|&g| g != 0)?;
        res.u64(group + 0x08).ok().filter(|&t| t != 0)?
    } else {
        SYS
    };
    let count = res.u16(root + 0x38).ok()?;
    if count == 0 {
        return None;
    }
    let first = res.u64(res.u64(root + 0x30).ok()?).ok()?;
    let texture = res.bytes(first, DictLayout::Enhanced.texture_size()).ok()?.to_vec();
    let root = res.bytes(root, DICTIONARY_ROOT_SIZE).ok()?.to_vec();
    Some((root, texture))
}

fn build(
    template: &Resource,
    lods: &[&[NewMesh]],
    materials: Option<&[NewMaterial]>,
    donor: Option<&Resource>,
) -> Result<Vec<u8>, ResourceError> {
    check_template(template)?;
    if lods.is_empty() || lods.len() > 4 || lods.iter().any(|l| l.is_empty()) {
        return Err(bad("give one to four detail levels, each with meshes"));
    }
    let geometries = template_geometries(template)?;
    let usable: Vec<u16> = geometries.iter().map(|(s, _)| *s).collect();
    if let Some(mats) = materials {
        for (k, mat) in mats.iter().enumerate() {
            if !usable.contains(&mat.shader) {
                return Err(bad(format!("material {k} uses shader {}, the template only has {usable:?}", mat.shader)));
            }
        }
    }
    // The template shader each mesh is drawn with (its vertex layout).
    let source = |m: &NewMesh| -> Option<u16> {
        match materials {
            Some(mats) => mats.get(m.shader as usize).map(|mat| mat.shader),
            None => Some(m.shader),
        }
    };
    for (level, meshes) in lods.iter().enumerate() {
        for (i, m) in meshes.iter().enumerate() {
            let what = format!("level {level}, mesh {i}");
            let n = m.positions.len();
            match source(m) {
                Some(s) if usable.contains(&s) => {}
                _ if materials.is_some() => {
                    return Err(bad(format!("{what} uses material {}, which does not exist", m.shader)));
                }
                _ => return Err(bad(format!("{what} uses shader {}, the template only has {usable:?}", m.shader))),
            }
            if n == 0 || m.indices.is_empty() || m.indices.len() % 3 != 0 {
                return Err(bad(format!("{what} has no triangles")));
            }
            if n > MAX_VERTICES {
                return Err(bad(format!("{what} has {n} vertices; split it (at most {MAX_VERTICES} per mesh)")));
            }
            if m.indices.iter().any(|&k| k as usize >= n) {
                return Err(bad(format!("{what} has triangles pointing at missing vertices")));
            }
            if (!m.normals.is_empty() && m.normals.len() != n)
                || (!m.uvs.is_empty() && m.uvs.len() != n)
                || (!m.colors.is_empty() && m.colors.len() != n)
            {
                return Err(bad(format!("{what} has per-vertex data of the wrong length")));
            }
            if m.positions.iter().flatten().any(|v| !v.is_finite()) {
                return Err(bad(format!("{what} has positions that are not numbers")));
            }
        }
    }
    let template_of = |shader: u16| &geometries.iter().find(|(s, _)| *s == shader).expect("checked").1;

    let mut b = Builder::new();
    // The template stays where it was: first block of each segment, offset 0.
    let sys = b.add(Segment::System, template.system.clone());
    if !template.graphics.is_empty() {
        b.add(Segment::Graphics, template.graphics.clone());
    }
    let copy = |addr: u64, len: usize| -> Result<Vec<u8>, ResourceError> {
        if !(SYS..GFX).contains(&addr) {
            return Err(bad("template structure outside system memory"));
        }
        Ok(template.bytes(addr, len)?.to_vec())
    };
    b.add_pages_info(sys, 0x08);

    // Materials: one shader copy each (indices after the template's shaders).
    let group = template.u64(SYS + 0x10)?;
    let first_new = if group != 0 { template.u16(group + 0x18)? } else { 0 };
    if let Some(mats) = materials {
        if group == 0 {
            return Err(bad("the template has no shader group"));
        }
        add_materials(&mut b, sys, template, group, mats, donor)?;
    }
    let output_shader = |m: &NewMesh| if materials.is_some() { first_new + m.shader } else { m.shader };

    // The template's detail levels: model list, distance, render mask.
    let mut levels = Vec::new();
    for k in 0..4u64 {
        let list = template.u64(SYS + 0x50 + k * 8)?;
        let model = if list != 0 && template.u16(list + 8)? > 0 { template.u64(template.u64(list)?)? } else { 0 };
        let distance = f32::from_le_bytes(template.bytes(SYS + 0x70 + k * 4, 4)?.try_into().unwrap());
        levels.push((model, distance, template.u32(SYS + 0x80 + k * 4)?));
    }
    let template_levels = levels.iter().take_while(|l| l.0 != 0).count();
    let far = levels.iter().map(|l| l.1).fold(0.0f32, f32::max);
    let far = if far > 0.0 { far } else { 9998.0 };

    let mut lists = Vec::new();
    for (level, meshes) in lods.iter().enumerate() {
        let mut geometry_blocks: Vec<BlockId> = Vec::new();
        let mut boxes = Vec::new();
        for m in meshes.iter() {
            let t = template_of(source(m).expect("checked"));
            let data = vertex_data(m, &t.layout, t.stride)?;
            let vdata = b.add(Segment::System, data);
            let mut idx = Vec::with_capacity(m.indices.len() * 2);
            for &k in &m.indices {
                idx.extend_from_slice(&(k as u16).to_le_bytes());
            }
            let idata = b.add(Segment::System, idx);

            let mut vb = copy(t.vertex_buffer, BUFFER_SIZE)?;
            put_u32(&mut vb, 0x08, m.positions.len() as u32);
            put_u32(&mut vb, 0x0C, t.stride as u32);
            let vb_obj = b.add(Segment::System, copy(template.u64(t.vertex_buffer + 0x30)?, BUFFER_OBJECT_SIZE)?);
            let vb = b.add(Segment::System, vb);
            b.pointer(vb, 0x18, vdata, 0);
            b.pointer(vb, 0x30, vb_obj, 0);

            let mut ib = copy(t.index_buffer, BUFFER_SIZE)?;
            put_u32(&mut ib, 0x08, m.indices.len() as u32);
            put_u32(&mut ib, 0x0C, 2);
            let ib_obj = b.add(Segment::System, copy(template.u64(t.index_buffer + 0x30)?, BUFFER_OBJECT_SIZE)?);
            let ib = b.add(Segment::System, ib);
            b.pointer(ib, 0x18, idata, 0);
            b.pointer(ib, 0x30, ib_obj, 0);

            let mut g = copy(t.geometry, GEOMETRY_SIZE)?;
            put_u32(&mut g, 0x58, m.indices.len() as u32);
            put_u32(&mut g, 0x5C, (m.indices.len() / 3) as u32);
            put_u16(&mut g, 0x60, m.positions.len() as u16);
            put_u16(&mut g, 0x70, t.stride as u16);
            let g = b.add(Segment::System, g);
            b.pointer(g, 0x18, vb, 0);
            b.pointer(g, 0x38, ib, 0);
            b.pointer(g, 0x78, vdata, 0);
            geometry_blocks.push(g);
            boxes.push(bounds(m.positions.iter()));
        }
        let n = meshes.len();
        let all = bounds(meshes.iter().flat_map(|m| &m.positions));

        // The model: the template's at this level (its flags), else the one
        // the first mesh's template geometry belongs to.
        let source_model = match levels.get(level) {
            Some(&(model, _, _)) if model != 0 => model,
            _ => template_of(source(&meshes[0]).expect("checked")).model,
        };
        let mut model = copy(source_model, MODEL_SIZE)?;
        if template.u32(source_model + 0x28)? >> 8 & 0xFF != 0 {
            model = copy(template_of(source(&meshes[0]).expect("checked")).model, MODEL_SIZE)?;
        }
        put_u16(&mut model, 0x10, n as u16);
        put_u16(&mut model, 0x12, n as u16);
        let pointers = b.add(Segment::System, vec![0u8; 8 * n]);
        for (i, g) in geometry_blocks.iter().enumerate() {
            b.pointer(pointers, i * 8, *g, 0);
        }
        let box_count = if n > 1 { n + 1 } else { 1 };
        let mut aabbs = vec![0u8; 32 * box_count];
        if n > 1 {
            put_box(&mut aabbs, 0, all);
        }
        for (i, bx) in boxes.iter().enumerate() {
            put_box(&mut aabbs, 32 * (i + usize::from(n > 1)), *bx);
        }
        let aabbs = b.add(Segment::System, aabbs);
        let mut shader_indices = vec![0u8; (2 * n).div_ceil(16) * 16];
        for (i, m) in meshes.iter().enumerate() {
            put_u16(&mut shader_indices, i * 2, output_shader(m));
        }
        let shader_indices = b.add(Segment::System, shader_indices);
        let model = b.add(Segment::System, model);
        b.pointer(model, 0x08, pointers, 0);
        b.pointer(model, 0x18, aabbs, 0);
        b.pointer(model, 0x20, shader_indices, 0);

        let models = b.add(Segment::System, vec![0u8; 16]);
        b.pointer(models, 0, model, 0);
        let mut list = vec![0u8; 16];
        put_u16(&mut list, 8, 1);
        put_u16(&mut list, 10, 1);
        let list = b.add(Segment::System, list);
        b.pointer(list, 0, models, 0);
        lists.push(list);
    }

    // Distances: the template's where it has a level that is not its last;
    // 50 m doubling per level otherwise; the last level as far as the
    // template's furthest.
    let mut distances = [far; 4];
    for (level, d) in distances.iter_mut().enumerate().take(lods.len() - 1) {
        let fallback = 50.0 * (1 << level) as f32;
        *d = if level + 1 < template_levels && levels[level].1 > 0.0 { levels[level].1 } else { fallback.min(far) };
    }
    let mask_of = |level: usize| match levels.get(level) {
        Some(&(model, _, mask)) if model != 0 => mask,
        _ => levels[0].2,
    };

    let all = bounds(lods[0].iter().flat_map(|m| &m.positions));
    let centre = [(all.0[0] + all.1[0]) / 2.0, (all.0[1] + all.1[1]) / 2.0, (all.0[2] + all.1[2]) / 2.0];
    let radius = lods[0]
        .iter()
        .flat_map(|m| &m.positions)
        .map(|p| dot(sub(*p, centre), sub(*p, centre)).sqrt())
        .fold(0.0f32, f32::max);
    let root = 0usize;
    {
        let d = b.data_mut(sys);
        for (k, c) in centre.iter().enumerate() {
            put_f32(d, root + 0x20 + k * 4, *c);
            put_f32(d, root + 0x30 + k * 4, all.0[k]);
            put_f32(d, root + 0x40 + k * 4, all.1[k]);
        }
        put_f32(d, root + 0x2C, radius);
        d[root + 0x50..root + 0x70].fill(0);
        for (k, distance) in distances.iter().enumerate() {
            put_f32(d, root + 0x70 + k * 4, *distance);
            put_u32(d, root + 0x80 + k * 4, if k < lods.len() { mask_of(k) } else { 0 });
        }
    }
    for (k, list) in lists.into_iter().enumerate() {
        b.pointer(sys, root + 0x50 + k * 8, list, 0);
    }
    b.build(VERSION_YDR_ENHANCED)
}

/// The colour texture slot of a template shader: its index into the texture
/// references.
fn colour_slot(res: &Resource, shader: u64) -> Result<usize, ResourceError> {
    let infos = res.u64(shader + 0x20)?;
    if infos == 0 {
        return Err(bad("the shader has no parameter information"));
    }
    let total = res.u8(infos + 4)? as u64;
    for k in 0..total {
        let (hash, slot) = (res.u32(infos + 8 + k * 8)?, res.u32(infos + 12 + k * 8)?);
        if slot & 3 == 0 && (hash == param::DIFFUSE_TEX || hash == param::DIFFUSE_SAMPLER) {
            return Ok((slot >> 2) as usize);
        }
    }
    Err(bad("the shader has no colour texture slot"))
}

/// Adds one copy of a template shader per material, the materials'
/// textures to the embedded dictionary and binds them to the copies.
fn add_materials(
    b: &mut Builder,
    sys: BlockId,
    template: &Resource,
    group: u64,
    materials: &[NewMaterial],
    donor: Option<&Resource>,
) -> Result<(), ResourceError> {
    let old_list = template.u64(group + 0x10)?;
    let old_count = template.u16(group + 0x18)? as usize;
    let list = b.add(Segment::System, vec![0u8; 8 * (old_count + materials.len())]);
    for i in 0..old_count {
        let p = template.u64(old_list + i as u64 * 8)?;
        b.data_mut(list)[i * 8..i * 8 + 8].copy_from_slice(&p.to_le_bytes());
    }
    // Materials sharing a texture (same name) share its one embedded copy.
    let mut new_textures: Vec<(Texture, BlockId)> = Vec::new();
    for (k, mat) in materials.iter().enumerate() {
        let src = template.u64(old_list + u64::from(mat.shader) * 8)?;
        let shader = b.add(Segment::System, template.bytes(src, SHADER_SIZE)?.to_vec());
        let infos = template.u64(src + 0x20)?;
        let slots = if infos != 0 { template.u8(infos + 1)? as usize } else { 0 };
        let refs_at = template.u64(src + 0x10)?;
        if slots > 0 && refs_at != 0 {
            let refs = b.add(Segment::System, template.bytes(refs_at, slots * 8)?.to_vec());
            b.pointer(shader, 0x10, refs, 0);
            if let Some(tex) = &mat.texture {
                let slot = colour_slot(template, src).map_err(|e| bad(format!("material {k}: {e}")))?;
                let old_ref = template.u64(refs_at + slot as u64 * 8)?;
                if old_ref == 0 {
                    return Err(bad(format!("material {k}: the shader's colour slot is empty")));
                }
                let reference = b.add(Segment::System, template.bytes(old_ref, TEXTURE_REF_SIZE)?.to_vec());
                let mut name = tex.name.clone().into_bytes();
                name.push(0);
                let name = b.add(Segment::System, name);
                b.pointer(reference, 0x28, name, 0);
                b.pointer(refs, slot * 8, reference, 0);
                if !new_textures.iter().any(|(t, _)| t.name.eq_ignore_ascii_case(&tex.name)) {
                    new_textures.push((tex.clone(), name));
                }
            }
        } else if mat.texture.is_some() {
            return Err(bad(format!("material {k}: the shader takes no textures")));
        }
        b.pointer(list, (old_count + k) * 8, shader, 0);
    }
    let group_at = (group - SYS) as usize;
    let count = (old_count + materials.len()) as u16;
    put_u16(b.data_mut(sys), group_at + 0x18, count);
    put_u16(b.data_mut(sys), group_at + 0x1A, count);
    b.pointer(sys, group_at + 0x10, list, 0);
    if !new_textures.is_empty() {
        add_textures(b, sys, template, group, new_textures, donor)?;
    }
    Ok(())
}

/// A new embedded dictionary: the template's textures (kept where they are)
/// and the new ones, sorted by name hash as the game expects.
fn add_textures(
    b: &mut Builder,
    sys: BlockId,
    template: &Resource,
    group: u64,
    textures: Vec<(Texture, BlockId)>,
    donor: Option<&Resource>,
) -> Result<(), ResourceError> {
    let (root_template, texture_template) =
        texture_templates(template)
            .or_else(|| donor.and_then(texture_templates))
            .ok_or_else(|| bad("no texture structure to copy: the model has no embedded textures (give a donor)"))?;
    // (hash, existing pointer or new block)
    enum Entry {
        Old(u64),
        New(BlockId),
    }
    let mut entries: Vec<(u32, Entry)> = Vec::new();
    let old = template.u64(group + 0x08)?;
    let new_hashes: Vec<u32> = textures.iter().map(|(t, _)| name_hash(&t.name)).collect();
    if old != 0 {
        let list = template.u64(old + 0x30)?;
        let hashes = template.u64(old + 0x20)?;
        for i in 0..template.u16(old + 0x38)? as u64 {
            let hash = template.u32(hashes + i * 4)?;
            // A new texture of the same name replaces the old one.
            if !new_hashes.contains(&hash) {
                entries.push((hash, Entry::Old(template.u64(list + i * 8)?)));
            }
        }
    }
    for ((tex, name), hash) in textures.into_iter().zip(new_hashes) {
        let size = tex.expected_size().ok_or_else(|| bad(format!("{}: unsupported format", tex.name)))?;
        let mut t = texture_template.clone();
        write_fields(DictLayout::Enhanced, &mut t, &tex, true)?;
        let block = b.add(Segment::System, t);
        let data = b.add(Segment::Graphics, tex.data[..size].to_vec());
        b.pointer(block, 0x28, name, 0);
        b.pointer(block, 0x30, block, 0x58);
        b.pointer(block, 0x38, data, 0);
        entries.push((hash, Entry::New(block)));
    }
    entries.sort_by_key(|(h, _)| *h);
    let mut root = if old != 0 { template.bytes(old, DICTIONARY_ROOT_SIZE)?.to_vec() } else { root_template };
    root[0x08..0x10].fill(0); // no pages info inside a drawable
    let count = entries.len() as u16;
    for at in [0x28, 0x2A, 0x38, 0x3A] {
        put_u16(&mut root, at, count);
    }
    let hashes: Vec<u8> = entries.iter().flat_map(|(h, _)| h.to_le_bytes()).collect();
    let mut pointers = vec![0u8; 8 * entries.len()];
    for (i, (_, e)) in entries.iter().enumerate() {
        if let Entry::Old(p) = e {
            pointers[i * 8..i * 8 + 8].copy_from_slice(&p.to_le_bytes());
        }
    }
    let hashes = b.add(Segment::System, hashes);
    let pointers = b.add(Segment::System, pointers);
    for (i, (_, e)) in entries.iter().enumerate() {
        if let Entry::New(block) = e {
            b.pointer(pointers, i * 8, *block, 0);
        }
    }
    let root = b.add(Segment::System, root);
    b.pointer(root, 0x20, hashes, 0);
    b.pointer(root, 0x30, pointers, 0);
    b.pointer(sys, (group - SYS) as usize + 0x08, root, 0);
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::drawable;

    /// A minimal Enhanced drawable: one model, one geometry (a triangle) with
    /// position, normal, tangent, colour and uv, shader group with `shaders`
    /// shaders (no parameters). Enough for the writer and the reader.
    pub(crate) fn template(shaders: u16) -> Resource {
        template_with(shaders, false)
    }

    /// With `embedded`, the shader group has an embedded dictionary with one
    /// 2×2 texture (`old_tex`).
    pub(crate) fn template_with(shaders: u16, embedded: bool) -> Resource {
        let mut b = Builder::new();
        let root = b.add(Segment::System, vec![0u8; 0xC0]);
        b.add_pages_info(root, 0x08);
        // Shader group: shaders with one texture slot (colour) referencing `orig_tex`.
        let mut list = Vec::new();
        let mut shader_blocks = Vec::new();
        let orig_name = b.add(Segment::System, b"orig_tex\0".to_vec());
        for s in 0..shaders {
            let mut sh = vec![0u8; SHADER_SIZE];
            put_u32(&mut sh, 0, 0x1000 + u32::from(s));
            let sh = b.add(Segment::System, sh);
            let mut infos = vec![0u8; 16];
            infos[1] = 1;
            infos[4] = 1;
            put_u32(&mut infos, 8, param::DIFFUSE_TEX);
            put_u32(&mut infos, 12, 0);
            let infos = b.add(Segment::System, infos);
            let reference = b.add(Segment::System, vec![0u8; TEXTURE_REF_SIZE]);
            b.pointer(reference, 0x28, orig_name, 0);
            let refs = b.add(Segment::System, vec![0u8; 8]);
            b.pointer(refs, 0, reference, 0);
            b.pointer(sh, 0x10, refs, 0);
            b.pointer(sh, 0x20, infos, 0);
            shader_blocks.push(sh);
            list.extend_from_slice(&[0u8; 8]);
        }
        let list = b.add(Segment::System, list);
        for (i, s) in shader_blocks.iter().enumerate() {
            b.pointer(list, i * 8, *s, 0);
        }
        let mut group = vec![0u8; 0x40];
        put_u16(&mut group, 0x18, shaders);
        put_u16(&mut group, 0x1A, shaders);
        let group = b.add(Segment::System, group);
        b.pointer(group, 0x10, list, 0);
        b.pointer(root, 0x10, group, 0);
        if embedded {
            let old = texture_from_rgba("old_tex", 2, 2, &[9u8; 16]).unwrap();
            let mut t = vec![0u8; DictLayout::Enhanced.texture_size()];
            write_fields(DictLayout::Enhanced, &mut t, &old, true).unwrap();
            let t = b.add(Segment::System, t);
            let name = b.add(Segment::System, b"old_tex\0".to_vec());
            let data = b.add(Segment::Graphics, old.data.clone());
            b.pointer(t, 0x28, name, 0);
            b.pointer(t, 0x30, t, 0x58);
            b.pointer(t, 0x38, data, 0);
            let mut dict = vec![0u8; DICTIONARY_ROOT_SIZE];
            for at in [0x28, 0x2A, 0x38, 0x3A] {
                put_u16(&mut dict, at, 1);
            }
            let hashes = b.add(Segment::System, name_hash("old_tex").to_le_bytes().to_vec());
            let ptrs = b.add(Segment::System, vec![0u8; 8]);
            b.pointer(ptrs, 0, t, 0);
            let dict = b.add(Segment::System, dict);
            b.pointer(dict, 0x20, hashes, 0);
            b.pointer(dict, 0x30, ptrs, 0);
            b.pointer(group, 0x08, dict, 0);
        }
        // Declaration: position (0), normal (4), tangent (8), colour (24), uv (28).
        let stride = 12 + 12 + 16 + 4 + 8;
        let mut decl = vec![0u8; 52 * 4 + 52 + 52 + 12];
        for (slot, off, fmt) in [(0usize, 0u32, 6u8), (4, 12, 6), (8, 24, 2), (24, 40, 28), (28, 44, 16)] {
            decl[slot * 4..slot * 4 + 4].copy_from_slice(&off.to_le_bytes());
            decl[52 * 4 + 52 + slot] = fmt;
        }
        decl[52 * 4..52 * 4 + 52].fill(stride as u8);
        let decl = b.add(Segment::System, decl);
        let mut verts = Vec::new();
        for p in [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
            let mut v = vec![0u8; stride];
            for (k, x) in p.iter().enumerate() {
                put_f32(&mut v, k * 4, *x);
            }
            verts.extend(v);
        }
        let vdata = b.add(Segment::System, verts);
        let idata = b.add(Segment::System, [0u16, 1, 2].iter().flat_map(|i| i.to_le_bytes()).collect());
        let obj = || vec![7u8; BUFFER_OBJECT_SIZE];
        let (vobj, iobj) = (b.add(Segment::System, obj()), b.add(Segment::System, obj()));
        let mut vb = vec![0u8; BUFFER_SIZE];
        put_u32(&mut vb, 0x08, 3);
        put_u32(&mut vb, 0x0C, stride as u32);
        let vb = b.add(Segment::System, vb);
        b.pointer(vb, 0x18, vdata, 0);
        b.pointer(vb, 0x30, vobj, 0);
        b.pointer(vb, 0x38, decl, 0);
        let mut ib = vec![0u8; BUFFER_SIZE];
        put_u32(&mut ib, 0x08, 3);
        put_u32(&mut ib, 0x0C, 2);
        let ib = b.add(Segment::System, ib);
        b.pointer(ib, 0x18, idata, 0);
        b.pointer(ib, 0x30, iobj, 0);
        let mut geos = Vec::new();
        for _ in 0..shaders {
            let g = b.add(Segment::System, vec![0u8; GEOMETRY_SIZE]);
            b.pointer(g, 0x18, vb, 0);
            b.pointer(g, 0x38, ib, 0);
            geos.push(g);
        }
        let gptrs = b.add(Segment::System, vec![0u8; 8 * geos.len()]);
        for (i, g) in geos.iter().enumerate() {
            b.pointer(gptrs, i * 8, *g, 0);
        }
        let mut mapping = vec![0u8; 16.max(2 * shaders as usize)];
        for s in 0..shaders {
            put_u16(&mut mapping, s as usize * 2, s);
        }
        let mapping = b.add(Segment::System, mapping);
        let aabbs = b.add(Segment::System, vec![0u8; 32 * (shaders as usize + 1)]);
        let mut model = vec![0u8; MODEL_SIZE];
        put_u16(&mut model, 0x10, shaders);
        put_u16(&mut model, 0x12, shaders);
        let model = b.add(Segment::System, model);
        b.pointer(model, 0x08, gptrs, 0);
        b.pointer(model, 0x18, aabbs, 0);
        b.pointer(model, 0x20, mapping, 0);
        let models = b.add(Segment::System, vec![0u8; 16]);
        b.pointer(models, 0, model, 0);
        let mut lod = vec![0u8; 16];
        put_u16(&mut lod, 8, 1);
        put_u16(&mut lod, 10, 1);
        let lod = b.add(Segment::System, lod);
        b.pointer(lod, 0, models, 0);
        b.pointer(root, 0x50, lod, 0);
        // A second detail level using the same model, to see it removed.
        b.pointer(root, 0x58, lod, 0);
        b.data_mut(root)[0x70..0x80].copy_from_slice(&[50.0f32, 300.0, 0.0, 0.0].map(f32::to_le_bytes).concat());
        let file = b.build(VERSION_YDR_ENHANCED).unwrap();
        Resource::from_rsc7(&file).unwrap()
    }

    fn quad(shader: u16, z: f32) -> NewMesh {
        NewMesh {
            shader,
            positions: vec![[0.0, 0.0, z], [2.0, 0.0, z], [2.0, 3.0, z], [0.0, 3.0, z]],
            normals: Vec::new(),
            uvs: vec![[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]],
            colors: Vec::new(),
            indices: vec![0, 1, 2, 0, 2, 3],
        }
    }

    #[test]
    fn writes_meshes_the_reader_reads_back() {
        let t = template(2);
        assert_eq!(usable_shaders(&t).unwrap(), [0, 1]);
        let meshes = [quad(0, 0.0), quad(1, 5.0)];
        let file = replace_meshes(&t, &meshes).unwrap();
        let res = Resource::from_rsc7(&file).unwrap();
        let d = drawable::parse_ydr(&res).unwrap();
        assert_eq!(d.meshes.len(), 2);
        assert_eq!(d.shaders.len(), 2, "the shader group is kept");
        for (got, want) in d.meshes.iter().zip(&meshes) {
            assert_eq!(got.positions, want.positions);
            assert_eq!(got.uvs, want.uvs);
            assert_eq!(got.indices, want.indices);
            assert_eq!(got.shader, want.shader);
            assert!(got.normals.iter().all(|n| (n[2] - 1.0).abs() < 1e-6), "computed normals face +z");
            assert_eq!(got.colors[0], [1.0; 4]);
        }
        assert_eq!(d.bounds_min, [0.0, 0.0, 0.0]);
        assert_eq!(d.bounds_max, [2.0, 3.0, 5.0]);
        // One detail level, as far as the template's furthest.
        assert_eq!(res.u64(SYS + 0x58).unwrap(), 0);
        assert_eq!(res.bytes(SYS + 0x70, 4).unwrap(), 300.0f32.to_le_bytes());
        // Several geometries: one box around all, then one each.
        let model = res.u64(res.u64(res.u64(SYS + 0x50).unwrap()).unwrap()).unwrap();
        let boxes = res.u64(model + 0x18).unwrap();
        assert_eq!(res.bytes(boxes + 0x18, 4).unwrap(), 5.0f32.to_le_bytes(), "overall max z");
        assert_eq!(res.bytes(boxes + 0x38, 4).unwrap(), 0.0f32.to_le_bytes(), "first mesh max z");
        // Writing again on the result works too.
        let again = replace_meshes(&res, &[quad(1, 1.0)]).unwrap();
        let d2 = drawable::parse_ydr(&Resource::from_rsc7(&again).unwrap()).unwrap();
        assert_eq!(d2.meshes.len(), 1);
        assert_eq!(d2.meshes[0].positions[1], [2.0, 0.0, 1.0]);
    }

    #[test]
    fn materials_get_their_own_shaders_and_textures() {
        let t = template_with(2, true);
        let rgba: Vec<u8> = (0..4 * 4 * 4).map(|i| i as u8).collect();
        let brick = texture_from_rgba("brick", 4, 4, &rgba).unwrap();
        assert_eq!(brick.levels, 3);
        assert_eq!(brick.data.len(), (16 + 4 + 1) * 4);
        let materials = [NewMaterial { shader: 1, texture: Some(brick) }, NewMaterial { shader: 1, texture: None }];
        let meshes = [quad(0, 0.0), quad(1, 1.0)];
        let file = replace_with_materials(&t, &meshes, &materials, None).unwrap();
        let d = drawable::parse_ydr(&Resource::from_rsc7(&file).unwrap()).unwrap();
        assert_eq!(d.shaders.len(), 4, "two template shaders, two copies");
        assert_eq!(d.shaders[2].textures, ["brick"]);
        assert_eq!(d.shaders[3].textures, ["orig_tex"], "no texture: the copy keeps the template's");
        assert_eq!(d.shaders[0].textures, ["orig_tex"], "the template's shaders stay");
        assert_eq!(d.meshes.iter().map(|m| m.shader).collect::<Vec<_>>(), [2, 3]);
        let dict = d.textures.expect("embedded dictionary");
        let names: Vec<&str> = dict.textures.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"old_tex") && names.contains(&"brick"));
        let got = dict.textures.iter().find(|t| t.name == "brick").unwrap();
        assert_eq!((got.width, got.height, got.levels), (4, 4, 3));
        assert_eq!(got.to_rgba().unwrap(), rgba, "pixels come back as given");

        // No embedded dictionary in the template: needs a donor.
        let plain = template_with(1, false);
        let tex = || Some(texture_from_rgba("x", 2, 2, &[1u8; 16]).unwrap());
        let mats = [NewMaterial { shader: 0, texture: tex() }];
        assert!(replace_with_materials(&plain, &[quad(0, 0.0)], &mats, None).is_err());
        let file = replace_with_materials(&plain, &[quad(0, 0.0)], &mats, Some(&t)).unwrap();
        let d = drawable::parse_ydr(&Resource::from_rsc7(&file).unwrap()).unwrap();
        assert_eq!(d.textures.unwrap().textures[0].name, "x");
        // Materials must name existing shaders, meshes existing materials.
        assert!(
            replace_with_materials(&t, &[quad(0, 0.0)], &[NewMaterial { shader: 7, texture: None }], None).is_err()
        );
        assert!(replace_with_materials(&t, &[quad(3, 0.0)], &materials, None).is_err());
    }

    #[test]
    fn writes_detail_levels() {
        // The template has two levels: 50 m and 300 m.
        let t = template(1);
        let (hi, med, low) = (vec![quad(0, 0.0), quad(0, 1.0)], vec![quad(0, 0.0)], vec![quad(0, 2.0)]);
        let file = replace_lods(&t, &[&hi, &med, &low], None, None).unwrap();
        let res = Resource::from_rsc7(&file).unwrap();
        let geometries = |level: u64| {
            let list = res.u64(SYS + 0x50 + level * 8).unwrap();
            (list != 0).then(|| res.u16(res.u64(res.u64(list).unwrap()).unwrap() + 0x10).unwrap())
        };
        assert_eq!([0, 1, 2, 3].map(geometries), [Some(2), Some(1), Some(1), None]);
        let distance = |k: u64| f32::from_le_bytes(res.bytes(SYS + 0x70 + k * 4, 4).unwrap().try_into().unwrap());
        assert_eq!([0, 1, 2].map(distance), [50.0, 100.0, 300.0]);
        // The reader shows the closest level.
        assert_eq!(drawable::parse_ydr(&res).unwrap().meshes.len(), 2);
        assert!(replace_lods(&t, &[&hi, &med, &low, &low, &low], None, None).is_err());
        assert!(replace_lods(&t, &[&hi, &[]], None, None).is_err());
    }

    #[test]
    fn tangents_follow_the_texture() {
        let m = quad(0, 0.0);
        let n = smooth_normals(&m);
        let t = tangents(&m, &n);
        // u grows along +x.
        assert!((t[0][0] - 1.0).abs() < 1e-5 && t[0][1].abs() < 1e-5, "{:?}", t[0]);
    }

    #[test]
    fn refuses_what_it_cannot_write() {
        let t = template(1);
        let mut bad_shader = quad(3, 0.0);
        assert!(replace_meshes(&t, &[bad_shader.clone()]).is_err());
        bad_shader.shader = 0;
        bad_shader.indices.push(9);
        bad_shader.indices.extend([0, 1]);
        assert!(replace_meshes(&t, &[bad_shader]).is_err());
        assert!(replace_meshes(&t, &[]).is_err());
        let mut legacy = t.clone();
        legacy.version = 165;
        assert!(replace_meshes(&legacy, &[quad(0, 0.0)]).is_err());
    }
}
