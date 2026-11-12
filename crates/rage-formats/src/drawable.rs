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
//!
//! Skeletons (`0x18` of a drawable): `0x20` bones (0x50 bytes each: rotation
//! quaternion, translation, scale, parent index at `0x32`, name at `0x38`,
//! tag at `0x44`), bone count at `0x5E`. A model's skeleton binding (`0x28`)
//! says whether it is skinned (byte 1) or rigidly attached to a bone (byte 3).
//!
//! Fragments (`.yft`): main drawable at `0x30`, name at `0x58`, physics LOD
//! group at `0xF0` → LOD (`0x10`) → children (`0xD0`, count at `0x11E`); a child
//! has its bone tag at `0x12` and its drawable at `0xA0`. Vehicle wheels are
//! such children.

use rage_resource::{Resource, ResourceError};

use crate::math::{self, Mat4};
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
    /// Second UV set (vehicle liveries and signs, see [`param::DIFFUSE_TEX2`]);
    /// empty if the mesh has none.
    pub uvs1: Vec<[f32; 2]>,
    pub colors: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
    /// Index into [`Drawable::shaders`].
    pub shader: u16,
    /// Skinning: up to four bones per vertex (skeleton bone indices) and
    /// their weights; empty for meshes that are not skinned.
    pub bone_indices: Vec<[u16; 4]>,
    pub bone_weights: Vec<[f32; 4]>,
    /// RDR2 ped parts: the bone tags the blend indices stand for (index `i`
    /// is the bone tagged `bone_tags[i]` of the skeleton the part is worn
    /// on); empty when the indices are skeleton indices already.
    pub bone_tags: Vec<u16>,
}

#[derive(Debug, Clone, Default)]
pub struct Bone {
    pub name: String,
    pub tag: u16,
    /// Index of the parent bone, `-1` for the root.
    pub parent: i16,
    pub translation: [f32; 3],
    /// Quaternion `x, y, z, w`.
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}

#[derive(Debug, Clone, Default)]
pub struct Skeleton {
    pub bones: Vec<Bone>,
}

impl Skeleton {
    /// Model-space transform of every bone (parents applied).
    pub fn world_transforms(&self) -> Vec<Mat4> {
        let mut out: Vec<Mat4> = Vec::with_capacity(self.bones.len());
        for (i, b) in self.bones.iter().enumerate() {
            let local = math::from_trs(b.translation, b.rotation, b.scale);
            // Parents come before their children; guard against bad indices anyway.
            let world = match usize::try_from(b.parent).ok().filter(|&p| p < i) {
                Some(p) => math::mul(&out[p], &local),
                None => local,
            };
            out.push(world);
        }
        out
    }

    pub fn find_tag(&self, tag: u16) -> Option<usize> {
        self.bones.iter().position(|b| b.tag == tag)
    }

    pub fn find_name(&self, name: &str) -> Option<usize> {
        self.bones.iter().position(|b| b.name.eq_ignore_ascii_case(name))
    }
}

/// Texture parameter names (joaat), Enhanced (`…Tex`) and Legacy (`…Sampler`).
pub mod param {
    /// `DiffuseTex`: the base colour.
    pub const DIFFUSE_TEX: u32 = 0xc8e8c282;
    /// `DiffuseSampler`: the base colour (Legacy).
    pub const DIFFUSE_SAMPLER: u32 = 0xf1fe2b71;
    /// `DiffuseTex2`: a second colour layer on the second UV set (vehicle liveries and signs).
    pub const DIFFUSE_TEX2: u32 = 0x0df47048;
    /// `DiffuseSampler2` (Legacy).
    pub const DIFFUSE_SAMPLER2: u32 = 0x0ad3a268;
    /// `DirtTex`, `SpecularTex`, `BumpTex`, `DamageTexture`.
    pub const DIRT_TEX: u32 = 0xa6f4cf58;
    pub const SPECULAR_TEX: u32 = 0xc6f46e79;
    pub const BUMP_TEX: u32 = 0x1e77bf4c;
    pub const DAMAGE_TEXTURE: u32 = 0xdf186a0e;
    /// Legacy: `DirtSampler`, `SpecSampler`, `BumpSampler`.
    pub const DIRT_SAMPLER: u32 = 0x7e9a27fe;
    pub const SPEC_SAMPLER: u32 = 0x608799c6;
    pub const BUMP_SAMPLER: u32 = 0x46b7c64f;

    /// Parameters that hold the base colour.
    pub const BASE: [u32; 2] = [DIFFUSE_TEX, DIFFUSE_SAMPLER];
    /// Parameters that never hold the base colour.
    pub const NOT_BASE: [u32; 9] = [
        DIFFUSE_TEX2,
        DIFFUSE_SAMPLER2,
        DIRT_TEX,
        SPECULAR_TEX,
        BUMP_TEX,
        DAMAGE_TEXTURE,
        DIRT_SAMPLER,
        SPEC_SAMPLER,
        BUMP_SAMPLER,
    ];
}

#[derive(Debug, Clone, Default)]
pub struct Shader {
    /// joaat of the shader name (e.g. `vehicle_paint1`).
    pub name_hash: u32,
    /// Names of the textures the shader uses (each once).
    pub textures: Vec<String>,
    /// Texture parameters: (parameter name hash, see [`param`]; 0 if unknown,
    /// texture name).
    pub samplers: Vec<(u32, String)>,
}

impl Shader {
    /// The texture bound to a parameter.
    pub fn sampler(&self, hash: u32) -> Option<&str> {
        self.samplers.iter().find(|(h, _)| *h == hash).map(|(_, n)| n.as_str())
    }

    /// The base colour texture: the one bound to `DiffuseTex`/`DiffuseSampler`,
    /// else the first texture that is not bound to a known other parameter
    /// and does not look like a normal/specular/… map by its name.
    ///
    /// Vehicle shaders sometimes bind a placeholder or a grime map to
    /// `DiffuseTex` (paint: `vehicle_generic_smallspecmap`, its colour comes
    /// from the paint; decals: `roof_dirt`, alpha-blended in the game): those
    /// have no base colour texture.
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
        if let Some(name) = param::BASE.iter().find_map(|&h| self.sampler(h)) {
            let vehicle = crate::shaders::name(self.name_hash).is_some_and(|n| n.starts_with("vehicle_"));
            let placeholder = vehicle && is_detail(name);
            return (!placeholder).then_some(name);
        }
        // Bound only to parameters that are known not to be the base colour
        // (e.g. a livery in `DiffuseTex2`, which uses the second UV set).
        let secondary = |n: &str| {
            let mut bound = self.samplers.iter().filter(|(_, s)| s == n).peekable();
            bound.peek().is_some() && bound.all(|(h, _)| param::NOT_BASE.contains(h))
        };
        // No fallback: shaders like vehicle paint have no colour texture at all.
        self.textures.iter().find(|t| !is_detail(t) && !secondary(t)).map(String::as_str)
    }

    /// A second colour layer drawn over the base colour with its alpha, on
    /// the second UV set (vehicle liveries and signs: `DiffuseTex2`).
    pub fn overlay_texture(&self) -> Option<&str> {
        self.sampler(param::DIFFUSE_TEX2).or_else(|| self.sampler(param::DIFFUSE_SAMPLER2))
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
    pub skeleton: Option<Skeleton>,
}

/// A child drawable of a fragment, attached to a bone of the main skeleton.
#[derive(Debug, Clone)]
pub struct FragmentChild {
    pub bone_tag: u16,
    pub drawable: Drawable,
}

#[derive(Debug, Clone, Default)]
pub struct Fragment {
    pub drawable: Drawable,
    /// Children with meshes of their own (e.g. wheels).
    pub children: Vec<FragmentChild>,
}

impl Drawable {
    pub fn triangle_count(&self) -> usize {
        self.meshes.iter().map(|m| m.indices.len() / 3).sum()
    }

    pub fn vertex_count(&self) -> usize {
        self.meshes.iter().map(|m| m.positions.len()).sum()
    }
}

/// Which game's drawable layout a resource uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gen {
    /// GTA V Legacy (`grm` structures, D3D9-era vertex declarations).
    Legacy,
    /// GTA V Enhanced (`rage::sga`, interleaved vertices).
    Enhanced,
    /// Red Dead Redemption 2 (`rage::sga`; geometry with the vertex buffer at
    /// `0x08` and the index buffer at `0x10`; vertex attributes stored as
    /// separate streams, one after another).
    Rdr2,
}

impl Gen {
    fn sga(self) -> bool {
        self != Gen::Legacy
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
    /// Four unsigned bytes as integers (bone indices).
    Uint8x4,
    /// 10:10:10:2, signed normalized (Enhanced normals).
    R10G10B10A2,
    /// 10:10:10:2, unsigned normalized and mapped to −1..1 (RDR2 normals).
    R10G10B10A2Unorm,
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
            Encoding::Uint8x4 => [d[0], d[1], d[2], d[3]].map(|v| v as f32),
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
            Encoding::R10G10B10A2Unorm => {
                let v = u32::from_le_bytes(d[0..4].try_into().unwrap());
                let u10 = |x: u32| (x & 0x3FF) as f32 / 1023.0 * 2.0 - 1.0;
                [u10(v), u10(v >> 10), u10(v >> 20), ((v >> 30) & 3) as f32 / 3.0]
            }
        }
    }

    fn size(self) -> usize {
        match self {
            Encoding::Float2 | Encoding::Half4 => 8,
            Encoding::Float3 => 12,
            Encoding::Float4 => 16,
            Encoding::Half2
            | Encoding::Unorm8x4
            | Encoding::Snorm8x4
            | Encoding::Uint8x4
            | Encoding::R10G10B10A2
            | Encoding::R10G10B10A2Unorm => 4,
        }
    }
}

/// Byte offsets and encodings of the attributes we use.
#[derive(Debug, Default)]
struct VertexLayout {
    stride: usize,
    /// Attributes are stored as separate streams (RDR2): attribute `a` of
    /// vertex `i` is at `offset + i × size` instead of `i × stride + offset`.
    streamed: bool,
    position: Option<(usize, Encoding)>,
    normal: Option<(usize, Encoding)>,
    uv: Option<(usize, Encoding)>,
    uv1: Option<(usize, Encoding)>,
    color: Option<(usize, Encoding)>,
    weights: Option<(usize, Encoding)>,
    indices: Option<(usize, Encoding)>,
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
        8 => (4, Some(Encoding::Uint8x4)), // ubyte4 (blend indices)
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
                1 => layout.weights = slot,
                2 => layout.indices = slot,
                3 => layout.normal = slot,
                4 => layout.color = slot,
                6 => layout.uv = slot,
                7 => layout.uv1 = slot,
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
        30 => Some(Encoding::Uint8x4),
        34 => Some(Encoding::Half2),
        _ => None,
    };
    // Slots: 0 position, 4 normal, 16 blend weights, 20 blend indices, 24 colour, 28 uv0, 29 uv1.
    for (slot, target) in [(0usize, 0u8), (4, 1), (24, 2), (28, 3), (16, 4), (20, 5), (29, 6)] {
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
            3 => layout.uv = v,
            4 => layout.weights = v,
            5 => layout.indices = v,
            _ => layout.uv1 = v,
        }
    }
    Ok(layout)
}

fn read_geometry(res: &Resource, g: u64, layout_gen: Gen) -> Result<Mesh, ResourceError> {
    let (vb, ib) = if layout_gen == Gen::Rdr2 {
        (res.u64(g + 0x08)?, res.u64(g + 0x10)?)
    } else {
        (res.u64(g + 0x18)?, res.u64(g + 0x38)?)
    };
    let (count, stride, data, mut layout) = if layout_gen.sga() {
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
    layout.streamed = layout_gen == Gen::Rdr2;
    if layout_gen == Gen::Rdr2
        && let Some((o, Encoding::R10G10B10A2)) = layout.normal
    {
        layout.normal = Some((o, Encoding::R10G10B10A2Unorm));
    }
    let bytes = res.bytes(data, count * stride)?;
    let (pos_off, pos_enc) = layout.position.ok_or_else(|| bad("vertices without positions"))?;
    // Where attribute (offset, encoding) of vertex i starts, if inside the buffer.
    let at = |(o, e): (usize, Encoding), i: usize| -> Option<usize> {
        if layout.streamed {
            let start = o + i * e.size();
            (o + count * e.size() <= bytes.len()).then_some(start)
        } else {
            (o + e.size() <= layout.stride).then_some(i * layout.stride + o)
        }
    };
    let attr = |slot: Option<(usize, Encoding)>, i: usize| slot.and_then(|s| Some(s.1.decode(&bytes[at(s, i)?..])));
    let mut mesh = Mesh::default();
    for i in 0..count {
        let Some(p_at) = at((pos_off, pos_enc), i) else { return Err(bad("position outside vertex")) };
        let p = pos_enc.decode(&bytes[p_at..]);
        mesh.positions.push([p[0], p[1], p[2]]);
        if let Some(n) = attr(layout.normal, i) {
            mesh.normals.push([n[0], n[1], n[2]]);
        }
        if let Some(t) = attr(layout.uv, i) {
            mesh.uvs.push([t[0], t[1]]);
        }
        if let Some(t) = attr(layout.uv1, i) {
            mesh.uvs1.push([t[0], t[1]]);
        }
        if let Some(c) = attr(layout.color, i) {
            mesh.colors.push(c);
        }
        if let (Some(w), Some(b)) = (attr(layout.weights, i), attr(layout.indices, i)) {
            mesh.bone_weights.push(w);
            mesh.bone_indices.push(b.map(|v| v as u16));
        }
    }
    // Blend indices point into the geometry's bone table (skeleton indices).
    // RDR2 keeps the table at 0x18 without a count: look up just the entries used.
    if !mesh.bone_indices.is_empty() && layout_gen == Gen::Rdr2 {
        let table = res.u64(g + 0x18)?;
        if (0x5000_0000..0x7000_0000).contains(&table) {
            let used = mesh.bone_indices.iter().flatten().copied().max().unwrap_or(0) as u64 + 1;
            if let Ok(ids) = (0..used).map(|i| res.u16(table + i * 2)).collect::<Result<Vec<u16>, _>>() {
                for b in &mut mesh.bone_indices {
                    *b = b.map(|i| ids.get(i as usize).copied().unwrap_or(0));
                }
            }
        }
    } else if !mesh.bone_indices.is_empty() {
        let table = res.u64(g + 0x68)?;
        let count = res.u16(g + 0x72)? as u64;
        if table != 0 && count > 0 && count <= 1024 {
            let ids: Vec<u16> = (0..count).map(|i| res.u16(table + i * 2)).collect::<Result<_, _>>()?;
            for b in &mut mesh.bone_indices {
                *b = b.map(|i| ids.get(i as usize).copied().unwrap_or(0));
            }
        }
    }

    let index_count = res.u32(ib + 0x08)? as usize;
    let (index_ptr, index_size) = if layout_gen.sga() {
        (res.u64(ib + 0x18)?, res.u16(ib + 0x0C)?.max(2) as usize)
    } else {
        (res.u64(ib + 0x10)?, 2)
    };
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

/// Reads the models of one LOD. Meshes of models rigidly attached to a bone
/// come with that bone's index.
fn read_models(
    res: &Resource,
    list: u64,
    layout_gen: Gen,
    out: &mut Vec<(Mesh, Option<usize>)>,
) -> Result<(), ResourceError> {
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
        // RDR2 keeps a pointer at 0x28 (the bone tags of a part without a
        // skeleton of its own) and the binding at 0x30.
        let rdr2 = layout_gen == Gen::Rdr2;
        let binding = res.u32(model + if rdr2 { 0x30 } else { 0x28 })?;
        let skinned = (binding >> 8) & 0xFF != 0;
        let bone = (!skinned && binding >> 24 != 0).then_some((binding >> 24) as usize);
        let tags: Vec<u16> = match (rdr2 && skinned).then(|| res.u64(model + 0x28)).transpose()? {
            Some(table) if (0x5000_0000..0x7000_0000).contains(&table) => {
                (0..(binding & 0xFF) as u64).map(|k| res.u16(table + k * 2)).collect::<Result<_, _>>()?
            }
            _ => Vec::new(),
        };
        for i in 0..gcount {
            let g = res.u64(geoms + i * 8)?;
            let mut mesh = read_geometry(res, g, layout_gen)?;
            mesh.shader = if mapping != 0 { res.u16(mapping + i * 2)? } else { 0 };
            // Without a bone table of its own the geometry's indices point into the model's tags.
            if !tags.is_empty() && !mesh.bone_indices.is_empty() && res.u64(g + 0x18)? == 0 {
                mesh.bone_tags = tags.clone();
            }
            out.push((mesh, bone));
        }
    }
    Ok(())
}

/// Texture name of a texture structure referenced by a shader.
fn texture_name(res: &Resource, tex: u64) -> Option<String> {
    let name = res.string(res.u64(tex + 0x28).ok()?).ok()?;
    (!name.is_empty() && name.len() < 128 && name.chars().all(|c| c.is_ascii_graphic())).then_some(name)
}

fn read_shaders(res: &Resource, group: u64, layout_gen: Gen) -> Result<Vec<Shader>, ResourceError> {
    let list = res.u64(group + 0x10)?;
    let count = res.u16(group + 0x18)? as u64;
    let mut shaders = Vec::new();
    for i in 0..count.min(512) {
        let s = res.u64(list + i * 8)?;
        let mut shader = Shader::default();
        // (parameter hash or 0, texture structure) per texture reference.
        let mut refs: Vec<(u32, u64)> = Vec::new();
        if layout_gen.sga() {
            shader.name_hash = res.u32(s)?;
            // Texture references: a pointer array at 0x10. The parameter info
            // block (0x20) has the number of textures at 1, the number of
            // parameters at 4 and from 8 on (name hash, slot) pairs; the low
            // two bits of a slot are its kind (0 = texture), the rest its index.
            let ptrs = res.u64(s + 0x10)?;
            let infos = res.u64(s + 0x20)?;
            let (num, total) = if infos != 0 {
                (res.u8(infos + 1).unwrap_or(0) as u64, res.u8(infos + 4).unwrap_or(0) as u64)
            } else {
                (0, 0)
            };
            let mut hashes = vec![0u32; num.min(32) as usize];
            for k in 0..total.min(128) {
                let (Ok(hash), Ok(slot)) = (res.u32(infos + 8 + k * 8), res.u32(infos + 12 + k * 8)) else { break };
                if slot & 3 == 0
                    && let Some(h) = hashes.get_mut((slot >> 2) as usize)
                {
                    *h = hash;
                }
            }
            if ptrs != 0 {
                for (t, hash) in hashes.into_iter().enumerate() {
                    if let Some(p) = res.u64(ptrs + t as u64 * 8).ok().filter(|&p| p != 0) {
                        refs.push((hash, p));
                    }
                }
            }
        } else {
            // Parameters: 16 bytes each (data type at 0, 0 = texture; data
            // pointer at 8), then the data of the other parameters (data type
            // × 16 bytes each), then one name hash per parameter.
            let params = res.u64(s)?;
            shader.name_hash = res.u32(s + 0x08)?;
            let pcount = res.u8(s + 0x10)? as u64;
            let mut types = Vec::new();
            for p in 0..pcount {
                types.push(res.u8(params + p * 16)? as u64);
            }
            let data: u64 = types.iter().sum::<u64>() * 16;
            let hashes = params + pcount * 16 + data;
            for (p, &t) in types.iter().enumerate() {
                let p = p as u64;
                if t == 0
                    && let Some(ptr) = res.u64(params + p * 16 + 8).ok().filter(|&ptr| ptr != 0)
                {
                    refs.push((res.u32(hashes + p * 4).unwrap_or(0), ptr));
                }
            }
        }
        for (hash, ptr) in refs {
            if let Some(name) = texture_name(res, ptr) {
                shader.samplers.push((hash, name.clone()));
                if !shader.textures.contains(&name) {
                    shader.textures.push(name);
                }
            }
        }
        shaders.push(shader);
    }
    Ok(shaders)
}

/// A skeleton: GTA V's bones list is at 0x20 (parent at bone+0x32, tag at
/// +0x44); RDR2's at 0x28 (parent at +0x46, tag at +0x4A). Both keep the
/// bone count at 0x5E and 0x50-byte bones with rotation, translation,
/// scale and name pointer first.
fn read_skeleton_gen(res: &Resource, ptr: u64, layout_gen: Gen) -> Result<Skeleton, ResourceError> {
    let rdr2 = layout_gen == Gen::Rdr2;
    let bones = res.u64(ptr + if rdr2 { 0x28 } else { 0x20 })?;
    let count = res.u16(ptr + 0x5E)? as u64;
    if bones == 0 || count > 2048 {
        return Err(bad("invalid skeleton"));
    }
    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count {
        let b = bones + i * 0x50;
        let f = |o: u64| -> Result<f32, ResourceError> { Ok(f32::from_bits(res.u32(b + o)?)) };
        let name_ptr = res.u64(b + if rdr2 { 0x30 } else { 0x38 })?;
        out.push(Bone {
            name: if name_ptr != 0 { res.string(name_ptr).unwrap_or_default() } else { String::new() },
            tag: res.u16(b + if rdr2 { 0x4A } else { 0x44 })?,
            parent: res.u16(b + if rdr2 { 0x46 } else { 0x32 })? as i16,
            rotation: [f(0x00)?, f(0x04)?, f(0x08)?, f(0x0C)?],
            translation: [f(0x10)?, f(0x14)?, f(0x18)?],
            scale: [f(0x20)?, f(0x24)?, f(0x28)?],
        });
    }
    Ok(Skeleton { bones: out })
}

/// Reads the drawable structure at `ptr`. Meshes rigidly attached to a bone
/// are moved to their place in model space.
pub fn read_drawable(res: &Resource, ptr: u64, enhanced: bool) -> Result<Drawable, ResourceError> {
    read_drawable_gen(res, ptr, if enhanced { Gen::Enhanced } else { Gen::Legacy })
}

/// Reads the drawable structure at `ptr` in the layout of `gen`.
pub fn read_drawable_gen(res: &Resource, ptr: u64, layout_gen: Gen) -> Result<Drawable, ResourceError> {
    let mut d =
        Drawable { bounds_min: vec3(res, ptr + 0x30)?, bounds_max: vec3(res, ptr + 0x40)?, ..Default::default() };
    let skel = res.u64(ptr + 0x18)?;
    if skel != 0 {
        d.skeleton = read_skeleton_gen(res, skel, layout_gen).ok();
    }
    let world = d.skeleton.as_ref().map(Skeleton::world_transforms).unwrap_or_default();
    for lod in 0..4 {
        let list = res.u64(ptr + 0x50 + lod * 8)?;
        if list != 0 {
            let mut meshes = Vec::new();
            read_models(res, list, layout_gen, &mut meshes)?;
            for (mut mesh, bone) in meshes {
                if let Some((b, m)) = bone.and_then(|b| Some((b, world.get(b)?))) {
                    mesh.positions.iter_mut().for_each(|p| *p = math::transform_point(m, *p));
                    mesh.normals.iter_mut().for_each(|n| *n = math::transform_dir(m, *n));
                    // Fully bound to its bone, so animations move it.
                    mesh.bone_indices = vec![[b as u16, 0, 0, 0]; mesh.positions.len()];
                    mesh.bone_weights = vec![[1.0, 0.0, 0.0, 0.0]; mesh.positions.len()];
                }
                d.meshes.push(mesh);
            }
            if !d.meshes.is_empty() {
                break;
            }
        }
    }
    let group = res.u64(ptr + 0x10)?;
    if group != 0 {
        d.shaders = read_shaders(res, group, layout_gen).unwrap_or_default();
        let txd = res.u64(group + 0x08)?;
        if txd != 0 {
            d.textures = if layout_gen == Gen::Rdr2 {
                ytd::parse_rdr2_at(res, txd).ok()
            } else {
                ytd::parse_at(res, txd, layout_gen.sga()).ok()
            };
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
    Ok(parse_fragment(res)?.drawable)
}

/// A `.yft` file with the child drawables (wheels and other parts).
/// A Red Dead Redemption 2 `.yft`: its main drawable (at `0x20` of the
/// fragment; found among the root's pointers if not). Children are not read yet.
pub fn parse_fragment_rdr2(res: &Resource) -> Result<Fragment, ResourceError> {
    let root = Resource::ROOT;
    let looks_like_drawable = |p: u64| -> bool {
        let ok = |q: u64| (0x5000_0000..0x6000_0000).contains(&q);
        ok(p)
            && res.u64(p + 0x10).is_ok_and(ok)
            && res.u64(p + 0x50).is_ok_and(|lod| {
                ok(lod) && res.u64(lod).is_ok_and(ok) && res.u16(lod + 8).is_ok_and(|n| n > 0 && n < 64)
            })
    };
    let candidates = std::iter::once(0x20).chain((0..0xC0).step_by(8));
    let drawable = candidates
        .filter_map(|o| res.u64(root + o).ok())
        .find(|&p| looks_like_drawable(p))
        .ok_or_else(|| bad("fragment without a drawable"))?;
    Ok(Fragment { drawable: read_drawable_gen(res, drawable, Gen::Rdr2)?, children: Vec::new() })
}

pub fn parse_fragment(res: &Resource) -> Result<Fragment, ResourceError> {
    let enhanced = is_enhanced(res.version)?;
    let root = Resource::ROOT;
    let drawable = res.u64(root + 0x30)?;
    if drawable == 0 {
        return Err(bad("fragment without drawable"));
    }
    let mut d = read_drawable(res, drawable, enhanced)?;
    if let Ok(name) = res.u64(root + 0x58).and_then(|p| res.string(p)) {
        d.name = name.strip_prefix("pack:/").unwrap_or(&name).to_owned();
    }
    // Children are optional extras: a fragment without readable ones is still fine.
    let children = read_fragment_children(res, enhanced).unwrap_or_default();
    Ok(Fragment { drawable: d, children })
}

fn read_fragment_children(res: &Resource, enhanced: bool) -> Result<Vec<FragmentChild>, ResourceError> {
    let group = res.u64(Resource::ROOT + 0xF0)?;
    if group == 0 {
        return Ok(Vec::new());
    }
    let Some(lod) = (0..3).map(|i| res.u64(group + 0x10 + i * 8)).find_map(|p| p.ok().filter(|&p| p != 0)) else {
        return Ok(Vec::new());
    };
    let list = res.u64(lod + 0xD0)?;
    let count = res.u8(lod + 0x11E)? as u64;
    let mut seen = Vec::new();
    let mut out = Vec::new();
    for i in 0..count {
        let child = res.u64(list + i * 8)?;
        if child == 0 {
            continue;
        }
        let ptr = res.u64(child + 0xA0)?;
        if ptr == 0 || seen.contains(&ptr) {
            continue;
        }
        seen.push(ptr);
        let drawable = read_drawable(res, ptr, enhanced)?;
        if !drawable.meshes.is_empty() {
            out.push(FragmentChild { bone_tag: res.u16(child + 0x12)?, drawable });
        }
    }
    Ok(out)
}

/// A `.ydd` file: all drawables with their name hashes.
pub fn parse_ydd(res: &Resource) -> Result<Vec<(u32, Drawable)>, ResourceError> {
    let layout_gen = if is_enhanced(res.version)? { Gen::Enhanced } else { Gen::Legacy };
    parse_ydd_gen(res, layout_gen)
}

/// A Red Dead Redemption 2 `.ydr`.
pub fn parse_ydr_rdr2(res: &Resource) -> Result<Drawable, ResourceError> {
    read_drawable_gen(res, Resource::ROOT, Gen::Rdr2)
}

/// A Red Dead Redemption 2 `.ydd`.
pub fn parse_ydd_rdr2(res: &Resource) -> Result<Vec<(u32, Drawable)>, ResourceError> {
    parse_ydd_gen(res, Gen::Rdr2)
}

fn parse_ydd_gen(res: &Resource, layout_gen: Gen) -> Result<Vec<(u32, Drawable)>, ResourceError> {
    let root = Resource::ROOT;
    let hashes = res.u64(root + 0x20)?;
    let list = res.u64(root + 0x30)?;
    let count = res.u16(root + 0x38)? as u64;
    let mut out = Vec::new();
    for i in 0..count.min(4096) {
        let hash = if hashes != 0 { res.u32(hashes + i * 4)? } else { 0 };
        out.push((hash, read_drawable_gen(res, res.u64(list + i * 8)?, layout_gen)?));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parameter_hashes() {
        use rage_core::hash::joaat;
        for (name, hash) in [
            ("DiffuseTex", param::DIFFUSE_TEX),
            ("DiffuseSampler", param::DIFFUSE_SAMPLER),
            ("DiffuseTex2", param::DIFFUSE_TEX2),
            ("DiffuseSampler2", param::DIFFUSE_SAMPLER2),
            ("DirtTex", param::DIRT_TEX),
            ("SpecularTex", param::SPECULAR_TEX),
            ("BumpTex", param::BUMP_TEX),
            ("DamageTexture", param::DAMAGE_TEXTURE),
            ("DirtSampler", param::DIRT_SAMPLER),
            ("SpecSampler", param::SPEC_SAMPLER),
            ("BumpSampler", param::BUMP_SAMPLER),
        ] {
            assert_eq!(joaat(name), hash, "{name}");
        }
    }

    #[test]
    fn base_texture_by_parameter() {
        let s = |samplers: &[(u32, &str)]| Shader {
            name_hash: 0,
            textures: samplers.iter().map(|(_, n)| n.to_string()).collect(),
            samplers: samplers.iter().map(|(h, n)| (*h, n.to_string())).collect(),
        };
        // The livery in DiffuseTex2 is not the base colour.
        let paint = s(&[
            (param::DIFFUSE_TEX, "vehicle_generic_smallspecmap"),
            (param::DIFFUSE_TEX2, "ambulance_sign_1"),
            (param::DIRT_TEX, "vehicle_genericmud_car"),
        ]);
        // Not paint: DiffuseTex is the base colour.
        assert_eq!(paint.base_texture(), Some("vehicle_generic_smallspecmap"));
        assert_eq!(paint.overlay_texture(), Some("ambulance_sign_1"));
        // Paint: the placeholder in DiffuseTex is no colour texture.
        let paint = Shader { name_hash: rage_core::hash::joaat("vehicle_paint3"), ..paint };
        assert_eq!(paint.base_texture(), None);
        assert_eq!(paint.overlay_texture(), Some("ambulance_sign_1"));
        // Without DiffuseTex, a texture bound only to DiffuseTex2 is skipped.
        let livery_only = s(&[(param::DIFFUSE_TEX2, "airbus_sign_1"), (0, "airbus_body")]);
        assert_eq!(livery_only.base_texture(), Some("airbus_body"));
        // Unknown parameters: by name.
        let unknown = s(&[(0, "plate01_n"), (0, "plate01")]);
        assert_eq!(unknown.base_texture(), Some("plate01"));
    }

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
