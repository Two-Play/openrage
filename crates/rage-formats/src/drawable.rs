//! Drawables: the meshes of models (`.ydr`), fragments (`.yft`) and drawable
//! dictionaries (`.ydd`), for GTA V Legacy and Enhanced.
//!
//! Layout (shared by both versions unless noted):
//! * drawable: `0x10` shader group, `0x50..0x70` model lists for the four LODs
//!   (high, medium, low, very low), each a pointer-list header (pointer, count);
//! * model: `0x08` geometry pointers + count, `0x20` shader index per geometry;
//! * geometry: `0x18` vertex buffer, `0x38` index buffer;
//! * vertex buffer, Legacy: `0x08` stride, `0x10` data, `0x18` count,
//!   `0x30` declaration (component flags + 4-bit types);
//!   Enhanced: `0x08` count, `0x0C` stride, `0x18` data, `0x38` declaration
//!   (52 element slots with offsets and DXGI formats);
//! * index buffer: `0x08` count; data at `0x10` (Legacy) or `0x18` (Enhanced).
//!
//! Shaders reference textures by name; the names are resolved by the caller
//! (embedded texture dictionary, a `.ytd` next to the model, …).

use rage_resource::{Resource, ResourceError};

use crate::ytd::{self, TextureDictionary};

/// Resource versions of drawable resources.
pub const VERSION_YDR_LEGACY: u32 = 165;
pub const VERSION_YDR_ENHANCED: u32 = 159;
/// Older Enhanced drawable version, used by a few marker icons.
pub const VERSION_YDR_ENHANCED_OLD: u32 = 154;
pub const VERSION_YFT_LEGACY: u32 = 162;
pub const VERSION_YFT_ENHANCED: u32 = 171;

#[derive(Debug, Clone, Default)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub colors: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
    /// Index into [`Drawable::shaders`].
    pub shader: u16,
}

#[derive(Debug, Clone, Default)]
pub struct Shader {
    /// joaat of the shader name (e.g. `vehicle_paint1`).
    pub name_hash: u32,
    /// Names of the textures the shader uses.
    pub textures: Vec<String>,
}

impl Shader {
    /// The texture most likely to be the base colour (not a normal/specular/…).
    pub fn base_texture(&self) -> Option<&str> {
        let is_detail = |n: &str| {
            let n = n.to_ascii_lowercase();
            ["_n", "_s", "_normal", "_spec", "_bump", "_dirt", "_mask", "_detail", "_ao"]
                .iter()
                .any(|suffix| n.ends_with(suffix))
                || n.contains("normal")
                || n.contains("spec")
                || n.contains("dirt")
                || n.contains("mud")
                || n.starts_with("detail")
        };
        // No fallback: shaders like vehicle paint have no colour texture at all.
        self.textures.iter().find(|t| !is_detail(t)).map(String::as_str)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Drawable {
    pub name: String,
    /// Meshes of the most detailed LOD that has any.
    pub meshes: Vec<Mesh>,
    pub shaders: Vec<Shader>,
    /// Textures embedded in the drawable's shader group.
    pub textures: Option<TextureDictionary>,
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
}

impl Drawable {
    pub fn triangle_count(&self) -> usize {
        self.meshes.iter().map(|m| m.indices.len() / 3).sum()
    }

    pub fn vertex_count(&self) -> usize {
        self.meshes.iter().map(|m| m.positions.len()).sum()
    }
}

fn bad(msg: impl Into<String>) -> ResourceError {
    ResourceError::Format(msg.into())
}

fn f16(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = ((bits >> 10) & 0x1F) as i32;
    let frac = (bits & 0x3FF) as f32;
    match exp {
        0 => sign * frac * 2f32.powi(-24),
        31 => {
            if frac == 0.0 {
                sign * f32::INFINITY
            } else {
                f32::NAN
            }
        }
        _ => sign * (1.0 + frac / 1024.0) * 2f32.powi(exp - 15),
    }
}

fn f32_at(d: &[u8], o: usize) -> f32 {
    f32::from_le_bytes(d[o..o + 4].try_into().unwrap())
}

fn vec3(res: &Resource, ptr: u64) -> Result<[f32; 3], ResourceError> {
    let b = res.bytes(ptr, 12)?;
    Ok([f32_at(b, 0), f32_at(b, 4), f32_at(b, 8)])
}

/// How a vertex attribute is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Encoding {
    Float2,
    Float3,
    Float4,
    Half2,
    Half4,
    Unorm8x4,
    Snorm8x4,
    /// 10:10:10:2, signed normalized (Enhanced normals).
    R10G10B10A2,
}

impl Encoding {
    fn decode(self, d: &[u8]) -> [f32; 4] {
        let h = |i: usize| f16(u16::from_le_bytes([d[i], d[i + 1]]));
        match self {
            Encoding::Float2 => [f32_at(d, 0), f32_at(d, 4), 0.0, 0.0],
            Encoding::Float3 => [f32_at(d, 0), f32_at(d, 4), f32_at(d, 8), 0.0],
            Encoding::Float4 => [f32_at(d, 0), f32_at(d, 4), f32_at(d, 8), f32_at(d, 12)],
            Encoding::Half2 => [h(0), h(2), 0.0, 0.0],
            Encoding::Half4 => [h(0), h(2), h(4), h(6)],
            Encoding::Unorm8x4 => [d[0], d[1], d[2], d[3]].map(|v| v as f32 / 255.0),
            Encoding::Snorm8x4 => [d[0], d[1], d[2], d[3]].map(|v| (v as i8 as f32 / 127.0).max(-1.0)),
            Encoding::R10G10B10A2 => {
                let v = u32::from_le_bytes(d[0..4].try_into().unwrap());
                let s10 = |x: u32| {
                    let x = (x & 0x3FF) as i32;
                    let x = if x >= 512 { x - 1024 } else { x };
                    (x as f32 / 511.0).max(-1.0)
                };
                [s10(v), s10(v >> 10), s10(v >> 20), ((v >> 30) & 3) as f32 / 3.0]
            }
        }
    }

    fn size(self) -> usize {
        match self {
            Encoding::Float2 | Encoding::Half4 => 8,
            Encoding::Float3 => 12,
            Encoding::Float4 => 16,
            Encoding::Half2 | Encoding::Unorm8x4 | Encoding::Snorm8x4 | Encoding::R10G10B10A2 => 4,
        }
    }
}

/// Byte offsets and encodings of the attributes we use.
#[derive(Debug, Default)]
struct VertexLayout {
    stride: usize,
    position: Option<(usize, Encoding)>,
    normal: Option<(usize, Encoding)>,
    uv: Option<(usize, Encoding)>,
    color: Option<(usize, Encoding)>,
}

fn legacy_encoding(t: u64) -> Option<(usize, Option<Encoding>)> {
    // (size, encoding) for the legacy 4-bit component types.
    Some(match t {
        0 => (0, None),
        1 => (4, Some(Encoding::Half2)),
        2 => (4, None), // single float
        3 => (8, Some(Encoding::Half4)),
        5 => (8, Some(Encoding::Float2)),
        6 => (12, Some(Encoding::Float3)),
        7 => (16, Some(Encoding::Float4)),
        8 => (4, None), // ubyte4 (blend indices)
        9 => (4, Some(Encoding::Unorm8x4)),
        10 => (4, Some(Encoding::Snorm8x4)),
        _ => return None,
    })
}

fn legacy_layout(res: &Resource, decl: u64, stride: usize) -> Result<VertexLayout, ResourceError> {
    let flags = res.u32(decl)?;
    let types = res.u64(decl + 8)?;
    let mut layout = VertexLayout { stride, ..Default::default() };
    let mut offset = 0usize;
    for k in 0..16 {
        if (flags >> k) & 1 == 0 {
            continue;
        }
        let t = (types >> (k * 4)) & 0xF;
        let (size, enc) = legacy_encoding(t).ok_or_else(|| bad(format!("unknown vertex component type {t}")))?;
        if let Some(enc) = enc {
            let slot = Some((offset, enc));
            match k {
                0 => layout.position = slot,
                3 => layout.normal = slot,
                4 => layout.color = slot,
                6 => layout.uv = slot,
                _ => {}
            }
        }
        offset += size;
    }
    Ok(layout)
}

fn gen9_layout(res: &Resource, decl: u64, stride: usize) -> Result<VertexLayout, ResourceError> {
    let offsets = res.bytes(decl, 52 * 4)?;
    let types = res.bytes(decl + 52 * 4 + 52, 52)?;
    let mut layout = VertexLayout { stride, ..Default::default() };
    let enc = |t: u8| match t {
        2 => Some(Encoding::Float4),
        6 => Some(Encoding::Float3),
        10 => Some(Encoding::Half4),
        16 => Some(Encoding::Float2), // R32G32_TYPELESS
        24 => Some(Encoding::R10G10B10A2),
        28 => Some(Encoding::Unorm8x4),
        34 => Some(Encoding::Half2),
        _ => None,
    };
    for (slot, target) in [(0usize, 0u8), (4, 1), (24, 2), (28, 3)] {
        let t = types[slot];
        if t == 0 {
            continue;
        }
        let off = u32::from_le_bytes(offsets[slot * 4..slot * 4 + 4].try_into().unwrap()) as usize;
        let Some(e) = enc(t) else { continue };
        let v = Some((off, e));
        match target {
            0 => layout.position = v,
            1 => layout.normal = v,
            2 => layout.color = v,
            _ => layout.uv = v,
        }
    }
    Ok(layout)
}

fn read_geometry(res: &Resource, g: u64, enhanced: bool) -> Result<Mesh, ResourceError> {
    let vb = res.u64(g + 0x18)?;
    let ib = res.u64(g + 0x38)?;
    let (count, stride, data, layout) = if enhanced {
        let count = res.u32(vb + 0x08)? as usize;
        let stride = res.u16(vb + 0x0C)? as usize;
        let data = res.u64(vb + 0x18)?;
        (count, stride, data, gen9_layout(res, res.u64(vb + 0x38)?, stride)?)
    } else {
        let stride = res.u16(vb + 0x08)? as usize;
        let data = res.u64(vb + 0x10)?;
        let count = res.u32(vb + 0x18)? as usize;
        (count, stride, data, legacy_layout(res, res.u64(vb + 0x30)?, stride)?)
    };
    if stride == 0 || count > 1 << 22 {
        return Err(bad("implausible vertex buffer"));
    }
    let bytes = res.bytes(data, count * stride)?;
    let (pos_off, pos_enc) = layout.position.ok_or_else(|| bad("vertices without positions"))?;
    let attr = |slot: Option<(usize, Encoding)>, i: usize| {
        slot.filter(|(o, e)| o + e.size() <= layout.stride).map(|(o, e)| e.decode(&bytes[i * stride + o..]))
    };
    let mut mesh = Mesh::default();
    for i in 0..count {
        if pos_off + pos_enc.size() > stride {
            return Err(bad("position outside vertex"));
        }
        let p = pos_enc.decode(&bytes[i * stride + pos_off..]);
        mesh.positions.push([p[0], p[1], p[2]]);
        if let Some(n) = attr(layout.normal, i) {
            mesh.normals.push([n[0], n[1], n[2]]);
        }
        if let Some(t) = attr(layout.uv, i) {
            mesh.uvs.push([t[0], t[1]]);
        }
        if let Some(c) = attr(layout.color, i) {
            mesh.colors.push(c);
        }
    }

    let index_count = res.u32(ib + 0x08)? as usize;
    let (index_ptr, index_size) =
        if enhanced { (res.u64(ib + 0x18)?, res.u16(ib + 0x0C)?.max(2) as usize) } else { (res.u64(ib + 0x10)?, 2) };
    let ib_bytes = res.bytes(index_ptr, index_count * index_size)?;
    mesh.indices = if index_size == 4 {
        ib_bytes.as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c)).collect()
    } else {
        ib_bytes.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c) as u32).collect()
    };
    if mesh.indices.iter().any(|&i| i as usize >= count) {
        return Err(bad("index outside the vertex buffer"));
    }
    mesh.indices.truncate(mesh.indices.len() / 3 * 3);
    Ok(mesh)
}

fn read_models(res: &Resource, list: u64, enhanced: bool, out: &mut Vec<Mesh>) -> Result<(), ResourceError> {
    let ptr = res.u64(list)?;
    let count = res.u16(list + 8)? as u64;
    for m in 0..count {
        let model = res.u64(ptr + m * 8)?;
        if model == 0 {
            continue;
        }
        let geoms = res.u64(model + 0x08)?;
        let gcount = res.u16(model + 0x10)? as u64;
        let mapping = res.u64(model + 0x20)?;
        for i in 0..gcount {
            let mut mesh = read_geometry(res, res.u64(geoms + i * 8)?, enhanced)?;
            mesh.shader = if mapping != 0 { res.u16(mapping + i * 2)? } else { 0 };
            out.push(mesh);
        }
    }
    Ok(())
}

/// Texture name of a texture structure referenced by a shader.
fn texture_name(res: &Resource, tex: u64) -> Option<String> {
    let name = res.string(res.u64(tex + 0x28).ok()?).ok()?;
    (!name.is_empty() && name.len() < 128 && name.chars().all(|c| c.is_ascii_graphic())).then_some(name)
}

fn read_shaders(res: &Resource, group: u64, enhanced: bool) -> Result<Vec<Shader>, ResourceError> {
    let list = res.u64(group + 0x10)?;
    let count = res.u16(group + 0x18)? as u64;
    let mut shaders = Vec::new();
    for i in 0..count.min(512) {
        let s = res.u64(list + i * 8)?;
        let mut shader = Shader::default();
        if enhanced {
            shader.name_hash = res.u32(s)?;
            // Texture references: a pointer array, its length is in the parameter info block.
            let refs = res.u64(s + 0x10)?;
            let infos = res.u64(s + 0x20)?;
            let num = if infos != 0 { res.u8(infos + 1).unwrap_or(0) as u64 } else { 0 };
            if refs != 0 {
                for t in 0..num.min(32) {
                    if let Some(name) =
                        res.u64(refs + t * 8).ok().filter(|&p| p != 0).and_then(|p| texture_name(res, p))
                        && !shader.textures.contains(&name)
                    {
                        shader.textures.push(name);
                    }
                }
            }
        } else {
            let params = res.u64(s)?;
            shader.name_hash = res.u32(s + 0x08)?;
            let pcount = res.u8(s + 0x10)? as u64;
            for p in 0..pcount {
                let at = params + p * 16;
                if res.u8(at)? == 0
                    && let Some(name) = res.u64(at + 8).ok().filter(|&p| p != 0).and_then(|p| texture_name(res, p))
                    && !shader.textures.contains(&name)
                {
                    shader.textures.push(name);
                }
            }
        }
        shaders.push(shader);
    }
    Ok(shaders)
}

/// Reads the drawable structure at `ptr`.
pub fn read_drawable(res: &Resource, ptr: u64, enhanced: bool) -> Result<Drawable, ResourceError> {
    let mut d =
        Drawable { bounds_min: vec3(res, ptr + 0x30)?, bounds_max: vec3(res, ptr + 0x40)?, ..Default::default() };
    for lod in 0..4 {
        let list = res.u64(ptr + 0x50 + lod * 8)?;
        if list != 0 {
            read_models(res, list, enhanced, &mut d.meshes)?;
            if !d.meshes.is_empty() {
                break;
            }
        }
    }
    let group = res.u64(ptr + 0x10)?;
    if group != 0 {
        d.shaders = read_shaders(res, group, enhanced).unwrap_or_default();
        let txd = res.u64(group + 0x08)?;
        if txd != 0 {
            d.textures = ytd::parse_at(res, txd, enhanced).ok();
        }
    }
    Ok(d)
}

fn is_enhanced(version: u32) -> Result<bool, ResourceError> {
    match version {
        VERSION_YDR_ENHANCED | VERSION_YFT_ENHANCED | VERSION_YDR_ENHANCED_OLD => Ok(true),
        VERSION_YDR_LEGACY | VERSION_YFT_LEGACY => Ok(false),
        v => Err(bad(format!("unsupported drawable resource version {v}"))),
    }
}

/// A `.ydr` file.
pub fn parse_ydr(res: &Resource) -> Result<Drawable, ResourceError> {
    read_drawable(res, Resource::ROOT, is_enhanced(res.version)?)
}

/// A `.yft` file: its main drawable.
pub fn parse_yft(res: &Resource) -> Result<Drawable, ResourceError> {
    let enhanced = is_enhanced(res.version)?;
    let drawable = res.u64(Resource::ROOT + 0x30)?;
    if drawable == 0 {
        return Err(bad("fragment without drawable"));
    }
    let mut d = read_drawable(res, drawable, enhanced)?;
    if let Ok(name) = res.u64(Resource::ROOT + 0x58).and_then(|p| res.string(p)) {
        d.name = name;
    }
    Ok(d)
}

/// A `.ydd` file: all drawables with their name hashes.
pub fn parse_ydd(res: &Resource) -> Result<Vec<(u32, Drawable)>, ResourceError> {
    let enhanced = is_enhanced(res.version)?;
    let root = Resource::ROOT;
    let hashes = res.u64(root + 0x20)?;
    let list = res.u64(root + 0x30)?;
    let count = res.u16(root + 0x38)? as u64;
    let mut out = Vec::new();
    for i in 0..count.min(4096) {
        let hash = if hashes != 0 { res.u32(hashes + i * 4)? } else { 0 };
        out.push((hash, read_drawable(res, res.u64(list + i * 8)?, enhanced)?));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_floats() {
        assert_eq!(f16(0x3C00), 1.0);
        assert_eq!(f16(0xC000), -2.0);
        assert_eq!(f16(0x0000), 0.0);
        assert!((f16(0x3555) - 0.333_25).abs() < 1e-4);
    }

    #[test]
    fn packed_normals() {
        let v: u32 = 511 | ((1024 - 511) << 20);
        let n = Encoding::R10G10B10A2.decode(&v.to_le_bytes());
        assert_eq!([n[0], n[1], n[2]], [1.0, 0.0, -1.0]);
    }
}
