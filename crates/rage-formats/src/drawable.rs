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

use rage_core::bytes::LeRead;
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
    /// The same for GTA VI: hashes of the bones' names.
    pub bone_hashes: Vec<u32>,
    /// GTA VI: what shades a vertex. Red: how much of the sky it sees; alpha:
    /// how much of a decal, of dirt or of the second layer shows there.
    pub shading: Vec<[u8; 4]>,
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

    /// The bone whose name has this hash (GTA VI's animations name bones so).
    pub fn find_hash(&self, hash: u32) -> Option<usize> {
        self.bones.iter().position(|b| rage_core::hash::joaat(&b.name) == hash)
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
    /// GTA VI: the colour of the second layer of layered shaders.
    pub const LAYER_1_TEX: u32 = 0x7d4f_12e2;
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

    /// `TextureSampler`: the base colour (GTA IV).
    pub const TEXTURE_SAMPLER: u32 = 0x2b5170fd;

    /// RDR2 `lyr0DiffuseTex`: the colour of the first layer of layered
    /// shaders (`standard_vehicle_2lyr`, …). With a tint palette it holds a
    /// shade (and masks) that the palette colours.
    pub const LYR0_DIFFUSE_TEX: u32 = 0x346394ea;
    /// RDR2 `TintPaletteTex`: palette colours, one column per palette slot
    /// (vertex colour blue), one row per tint.
    pub const TINT_PALETTE_TEX: u32 = 0x2c8d7182;
    /// RDR2 `vehTintPaletteTex`: vehicle colours, one row per colour
    /// (`carcols.ymt`), columns from dark to light.
    pub const VEH_TINT_PALETTE_TEX: u32 = 0x7b21acbf;

    /// Parameters that hold the base colour.
    pub const BASE: [u32; 4] = [DIFFUSE_TEX, DIFFUSE_SAMPLER, TEXTURE_SAMPLER, LYR0_DIFFUSE_TEX];
    /// Parameters that never hold the base colour.
    pub const NOT_BASE: [u32; 11] = [
        TINT_PALETTE_TEX,
        VEH_TINT_PALETTE_TEX,
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
    /// the second UV set (vehicle liveries and signs: `DiffuseTex2`), or the
    /// second layer of GTA VI's layered shaders, on the first.
    pub fn overlay_texture(&self) -> Option<&str> {
        let overlay = self.sampler(param::DIFFUSE_TEX2).or_else(|| self.sampler(param::DIFFUSE_SAMPLER2));
        overlay.or_else(|| self.sampler(param::LAYER_1_TEX))
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
    /// Cloth (RDR2's flags and straps) in its rest pose, in model space.
    pub cloths: Vec<Drawable>,
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

pub(crate) fn bad(msg: impl Into<String>) -> ResourceError {
    ResourceError::Format(msg.into())
}

/// The float at `o`; 0 past the end (callers pass slices of the right size).
fn f32_at(d: &[u8], o: usize) -> f32 {
    d.f32_at(o).unwrap_or(0.0)
}

/// `N` consecutive floats from `at`.
pub(crate) fn floats<const N: usize>(res: &Resource, at: u64) -> Result<[f32; N], ResourceError> {
    let mut out = [0.0; N];
    for (k, v) in out.iter_mut().enumerate() {
        *v = f32::from_bits(res.u32(at + k as u64 * 4)?);
    }
    Ok(out)
}

pub(crate) fn vec3(res: &Resource, ptr: u64) -> Result<[f32; 3], ResourceError> {
    let b = res.bytes(ptr, 12)?;
    Ok([f32_at(b, 0), f32_at(b, 4), f32_at(b, 8)])
}

/// How a vertex attribute is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Encoding {
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
    /// Decodes one value; bytes missing at the end of `d` read as zero.
    pub(crate) fn decode(self, d: &[u8]) -> [f32; 4] {
        let mut padded = [0u8; 16];
        let n = d.len().min(16);
        padded[..n].copy_from_slice(&d[..n]);
        let d = &padded;
        let h = |i: usize| d.f16_at(i).unwrap_or(0.0);
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
                let v = d.u32_at(0).unwrap_or(0);
                let s10 = |x: u32| {
                    let x = (x & 0x3FF) as i32;
                    let x = if x >= 512 { x - 1024 } else { x };
                    (x as f32 / 511.0).max(-1.0)
                };
                [s10(v), s10(v >> 10), s10(v >> 20), ((v >> 30) & 3) as f32 / 3.0]
            }
            Encoding::R10G10B10A2Unorm => {
                let v = d.u32_at(0).unwrap_or(0);
                let u10 = |x: u32| (x & 0x3FF) as f32 / 1023.0 * 2.0 - 1.0;
                [u10(v), u10(v >> 10), u10(v >> 20), ((v >> 30) & 3) as f32 / 3.0]
            }
        }
    }

    pub(crate) fn size(self) -> usize {
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
pub(crate) struct VertexLayout {
    pub(crate) stride: usize,
    /// Attributes are stored as separate streams (RDR2): attribute `a` of
    /// vertex `i` is at `offset + i × size` instead of `i × stride + offset`.
    pub(crate) streamed: bool,
    pub(crate) position: Option<(usize, Encoding)>,
    pub(crate) normal: Option<(usize, Encoding)>,
    pub(crate) uv: Option<(usize, Encoding)>,
    pub(crate) uv1: Option<(usize, Encoding)>,
    pub(crate) color: Option<(usize, Encoding)>,
    pub(crate) weights: Option<(usize, Encoding)>,
    pub(crate) indices: Option<(usize, Encoding)>,
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

pub(crate) fn legacy_layout(res: &Resource, decl: u64, stride: usize) -> Result<VertexLayout, ResourceError> {
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

fn gen9_encoding(t: u8) -> Option<Encoding> {
    match t {
        2 => Some(Encoding::Float4),
        6 => Some(Encoding::Float3),
        10 => Some(Encoding::Half4),
        16 => Some(Encoding::Float2), // R32G32_TYPELESS
        24 => Some(Encoding::R10G10B10A2),
        28 => Some(Encoding::Unorm8x4),
        30 => Some(Encoding::Uint8x4),
        34 => Some(Encoding::Half2),
        _ => None,
    }
}

/// The layout attribute a `GEN9_SLOTS` target number stands for.
fn gen9_target(layout: &mut VertexLayout, target: u8) -> &mut Option<(usize, Encoding)> {
    match target {
        0 => &mut layout.position,
        1 => &mut layout.normal,
        2 => &mut layout.color,
        3 => &mut layout.uv,
        4 => &mut layout.weights,
        5 => &mut layout.indices,
        _ => &mut layout.uv1,
    }
}

/// Declaration slots and the attribute (`gen9_target`) each holds: 0 position, 4 normal, 16 blend weights,
/// 20 blend indices, 24 colour, 28 uv0, 29 uv1.
const GEN9_SLOTS: [(usize, u8); 7] = [(0, 0), (4, 1), (24, 2), (28, 3), (16, 4), (20, 5), (29, 6)];

fn gen9_layout(res: &Resource, decl: u64, stride: usize) -> Result<VertexLayout, ResourceError> {
    let offsets = res.bytes(decl, 52 * 4)?;
    let types = res.bytes(decl + 52 * 4 + 52, 52)?;
    let mut layout = VertexLayout { stride, ..Default::default() };
    for (slot, target) in GEN9_SLOTS {
        let t = types[slot];
        if t == 0 {
            continue;
        }
        let off = u32::from_le_bytes(offsets[slot * 4..slot * 4 + 4].try_into().unwrap()) as usize;
        let Some(e) = gen9_encoding(t) else { continue };
        *gen9_target(&mut layout, target) = Some((off, e));
    }
    Ok(layout)
}

/// Vertex count, stride, data pointer and layout of a geometry's vertex buffer at `vb`.
fn read_vertex_buffer(
    res: &Resource,
    vb: u64,
    layout_gen: Gen,
) -> Result<(usize, usize, u64, VertexLayout), ResourceError> {
    if layout_gen.sga() {
        let count = res.u32(vb + 0x08)? as usize;
        let stride = res.u16(vb + 0x0C)? as usize;
        let data = res.u64(vb + 0x18)?;
        Ok((count, stride, data, gen9_layout(res, res.u64(vb + 0x38)?, stride)?))
    } else {
        let stride = res.u16(vb + 0x08)? as usize;
        let data = res.u64(vb + 0x10)?;
        let count = res.u32(vb + 0x18)? as usize;
        Ok((count, stride, data, legacy_layout(res, res.u64(vb + 0x30)?, stride)?))
    }
}

/// The vertex bytes of a geometry and where its attributes lie in them.
struct VertexReader<'a> {
    layout: &'a VertexLayout,
    bytes: &'a [u8],
    count: usize,
}

impl VertexReader<'_> {
    /// Where attribute (offset, encoding) of vertex i starts, if inside the buffer.
    fn at(&self, (o, e): (usize, Encoding), i: usize) -> Option<usize> {
        if self.layout.streamed {
            let start = o + i * e.size();
            (o + self.count * e.size() <= self.bytes.len()).then_some(start)
        } else {
            (o + e.size() <= self.layout.stride).then_some(i * self.layout.stride + o)
        }
    }

    fn attr(&self, slot: Option<(usize, Encoding)>, i: usize) -> Option<[f32; 4]> {
        slot.and_then(|s| Some(s.1.decode(&self.bytes[self.at(s, i)?..])))
    }

    fn read_into(&self, mesh: &mut Mesh, pos_slot: (usize, Encoding)) -> Result<(), ResourceError> {
        for i in 0..self.count {
            let Some(p_at) = self.at(pos_slot, i) else { return Err(bad("position outside vertex")) };
            let p = pos_slot.1.decode(&self.bytes[p_at..]);
            mesh.positions.push([p[0], p[1], p[2]]);
            self.push_attributes(mesh, i);
        }
        Ok(())
    }

    fn push_attributes(&self, mesh: &mut Mesh, i: usize) {
        let layout = self.layout;
        if let Some(n) = self.attr(layout.normal, i) {
            mesh.normals.push([n[0], n[1], n[2]]);
        }
        if let Some(t) = self.attr(layout.uv, i) {
            mesh.uvs.push([t[0], t[1]]);
        }
        if let Some(t) = self.attr(layout.uv1, i) {
            mesh.uvs1.push([t[0], t[1]]);
        }
        if let Some(c) = self.attr(layout.color, i) {
            mesh.colors.push(c);
        }
        if let (Some(w), Some(b)) = (self.attr(layout.weights, i), self.attr(layout.indices, i)) {
            mesh.bone_weights.push(w);
            mesh.bone_indices.push(b.map(|v| v as u16));
        }
    }
}

/// Addresses a bone table of RDR2 can have.
const RDR2_BONE_TABLE: std::ops::Range<u64> = 0x5000_0000..0x7000_0000;

/// Blend indices point into the geometry's bone table (skeleton indices).
/// RDR2 keeps the table at 0x18 without a count: look up just the entries used.
fn remap_bones_rdr2(res: &Resource, g: u64, mesh: &mut Mesh) -> Result<(), ResourceError> {
    let table = res.u64(g + 0x18)?;
    if RDR2_BONE_TABLE.contains(&table) {
        let used = mesh.bone_indices.iter().flatten().copied().max().unwrap_or(0) as u64 + 1;
        if let Ok(ids) = (0..used).map(|i| res.u16(table + i * 2)).collect::<Result<Vec<u16>, _>>() {
            for b in &mut mesh.bone_indices {
                *b = b.map(|i| ids.get(i as usize).copied().unwrap_or(0));
            }
        }
    }
    Ok(())
}

fn remap_bones(res: &Resource, g: u64, mesh: &mut Mesh) -> Result<(), ResourceError> {
    let table = res.u64(g + 0x68)?;
    let count = res.u16(g + 0x72)? as u64;
    if table != 0 && count > 0 && count <= 1024 {
        let ids: Vec<u16> = (0..count).map(|i| res.u16(table + i * 2)).collect::<Result<_, _>>()?;
        for b in &mut mesh.bone_indices {
            *b = b.map(|i| ids.get(i as usize).copied().unwrap_or(0));
        }
    }
    Ok(())
}

fn remap_blend_indices(res: &Resource, g: u64, layout_gen: Gen, mesh: &mut Mesh) -> Result<(), ResourceError> {
    if mesh.bone_indices.is_empty() {
        Ok(())
    } else if layout_gen == Gen::Rdr2 {
        remap_bones_rdr2(res, g, mesh)
    } else {
        remap_bones(res, g, mesh)
    }
}

fn read_indices(res: &Resource, ib: u64, layout_gen: Gen) -> Result<Vec<u32>, ResourceError> {
    let index_count = res.u32(ib + 0x08)? as usize;
    let (index_ptr, index_size) = if layout_gen.sga() {
        (res.u64(ib + 0x18)?, res.u16(ib + 0x0C)?.max(2) as usize)
    } else {
        (res.u64(ib + 0x10)?, 2)
    };
    let ib_bytes = res.bytes(index_ptr, index_count * index_size)?;
    Ok(if index_size == 4 {
        ib_bytes.as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c)).collect()
    } else {
        ib_bytes.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c) as u32).collect()
    })
}

/// The RDR2 vertex buffer keeps normals as unsigned 10:10:10:2.
fn fix_rdr2_normal(layout: &mut VertexLayout) {
    if let Some((o, Encoding::R10G10B10A2)) = layout.normal {
        layout.normal = Some((o, Encoding::R10G10B10A2Unorm));
    }
}

/// The vertices of a geometry's vertex buffer at `vb`, and their count.
fn read_vertices(res: &Resource, vb: u64, layout_gen: Gen) -> Result<(Mesh, usize), ResourceError> {
    let (count, stride, data, mut layout) = read_vertex_buffer(res, vb, layout_gen)?;
    if stride == 0 || count > 1 << 22 {
        return Err(bad("implausible vertex buffer"));
    }
    layout.streamed = layout_gen == Gen::Rdr2;
    if layout_gen == Gen::Rdr2 {
        fix_rdr2_normal(&mut layout);
    }
    let bytes = res.bytes(data, count * stride)?;
    let pos_slot = layout.position.ok_or_else(|| bad("vertices without positions"))?;
    let mut mesh = Mesh::default();
    VertexReader { layout: &layout, bytes, count }.read_into(&mut mesh, pos_slot)?;
    Ok((mesh, count))
}

fn read_geometry(res: &Resource, g: u64, layout_gen: Gen) -> Result<Mesh, ResourceError> {
    let (vb, ib) = if layout_gen == Gen::Rdr2 {
        (res.u64(g + 0x08)?, res.u64(g + 0x10)?)
    } else {
        (res.u64(g + 0x18)?, res.u64(g + 0x38)?)
    };
    let (mut mesh, count) = read_vertices(res, vb, layout_gen)?;
    remap_blend_indices(res, g, layout_gen, &mut mesh)?;

    mesh.indices = read_indices(res, ib, layout_gen)?;
    if mesh.indices.iter().any(|&i| i as usize >= count) {
        return Err(bad("index outside the vertex buffer"));
    }
    mesh.indices.truncate(mesh.indices.len() / 3 * 3);
    Ok(mesh)
}

/// What a model says about its geometries.
struct ModelInfo {
    geoms: u64,
    count: u64,
    mapping: u64,
    /// The bone a rigidly attached model sits on.
    bone: Option<usize>,
    tags: Vec<u16>,
}

/// The bone tags of a skinned RDR2 model (`binding & 0xFF` of them, from the table at `0x28`).
fn read_model_tags(res: &Resource, model: u64, binding: u32) -> Result<Vec<u16>, ResourceError> {
    let table = res.u64(model + 0x28)?;
    if !RDR2_BONE_TABLE.contains(&table) {
        return Ok(Vec::new());
    }
    (0..(binding & 0xFF) as u64).map(|k| res.u16(table + k * 2)).collect()
}

fn read_model_info(res: &Resource, model: u64, layout_gen: Gen) -> Result<ModelInfo, ResourceError> {
    let geoms = res.u64(model + 0x08)?;
    let count = res.u16(model + 0x10)? as u64;
    let mapping = res.u64(model + 0x20)?;
    // RDR2 keeps a pointer at 0x28 (the bone tags of a part without a
    // skeleton of its own) and the binding at 0x30.
    let rdr2 = layout_gen == Gen::Rdr2;
    let binding = res.u32(model + if rdr2 { 0x30 } else { 0x28 })?;
    let skinned = (binding >> 8) & 0xFF != 0;
    let bone = (!skinned && binding >> 24 != 0).then_some((binding >> 24) as usize);
    let tags = if rdr2 && skinned { read_model_tags(res, model, binding)? } else { Vec::new() };
    Ok(ModelInfo { geoms, count, mapping, bone, tags })
}

/// Geometry `i` of a model.
fn read_model_mesh(res: &Resource, info: &ModelInfo, i: u64, layout_gen: Gen) -> Result<Mesh, ResourceError> {
    let g = res.u64(info.geoms + i * 8)?;
    let mut mesh = read_geometry(res, g, layout_gen)?;
    mesh.shader = if info.mapping != 0 { res.u16(info.mapping + i * 2)? } else { 0 };
    // Without a bone table of its own the geometry's indices point into the model's tags.
    if !info.tags.is_empty() && !mesh.bone_indices.is_empty() && res.u64(g + 0x18)? == 0 {
        mesh.bone_tags = info.tags.clone();
    }
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
        let info = read_model_info(res, model, layout_gen)?;
        for i in 0..info.count {
            out.push((read_model_mesh(res, &info, i, layout_gen)?, info.bone));
        }
    }
    Ok(())
}

/// Texture name of a texture structure referenced by a shader.
fn texture_name(res: &Resource, tex: u64) -> Option<String> {
    let name = res.string(res.u64(tex + 0x28).ok()?).ok()?;
    (!name.is_empty() && name.len() < 128 && name.chars().all(|c| c.is_ascii_graphic())).then_some(name)
}

/// Most shaders and, per shader, textures and parameters read.
const MAX_SHADERS: u64 = 512;
const MAX_SHADER_TEXTURES: u64 = 32;
const MAX_SHADER_PARAMETERS: u64 = 128;

/// (parameter hash or 0, texture structure) per texture reference.
type TextureRefs = Vec<(u32, u64)>;

/// The name hashes of the texture slots of an Enhanced shader, from its parameter info block at `infos`.
/// The block has the number of textures at 1, the number of parameters at 4 and from 8 on
/// (name hash, slot) pairs; the low two bits of a slot are its kind (0 = texture), the rest its index.
fn sga_texture_hashes(res: &Resource, infos: u64) -> Vec<u32> {
    let (num, total) = if infos != 0 {
        (res.u8(infos + 1).unwrap_or(0) as u64, res.u8(infos + 4).unwrap_or(0) as u64)
    } else {
        (0, 0)
    };
    let mut hashes = vec![0u32; num.min(MAX_SHADER_TEXTURES) as usize];
    for k in 0..total.min(MAX_SHADER_PARAMETERS) {
        let (Ok(hash), Ok(slot)) = (res.u32(infos + 8 + k * 8), res.u32(infos + 12 + k * 8)) else { break };
        if slot & 3 == 0
            && let Some(h) = hashes.get_mut((slot >> 2) as usize)
        {
            *h = hash;
        }
    }
    hashes
}

/// Name hash and texture references of the Enhanced / RDR2 shader at `s`.
fn read_sga_shader(res: &Resource, s: u64) -> Result<(u32, TextureRefs), ResourceError> {
    let name_hash = res.u32(s)?;
    // Texture references: a pointer array at 0x10, the parameter info block at 0x20.
    let ptrs = res.u64(s + 0x10)?;
    let infos = res.u64(s + 0x20)?;
    let hashes = sga_texture_hashes(res, infos);
    let mut refs = TextureRefs::new();
    if ptrs != 0 {
        for (t, hash) in hashes.into_iter().enumerate() {
            if let Some(p) = res.u64(ptrs + t as u64 * 8).ok().filter(|&p| p != 0) {
                refs.push((hash, p));
            }
        }
    }
    Ok((name_hash, refs))
}

/// Name hash and texture references of the Legacy shader at `s`.
fn read_legacy_shader(res: &Resource, s: u64) -> Result<(u32, TextureRefs), ResourceError> {
    // Parameters: 16 bytes each (data type at 0, 0 = texture; data
    // pointer at 8), then the data of the other parameters (data type
    // × 16 bytes each), then one name hash per parameter.
    let params = res.u64(s)?;
    let name_hash = res.u32(s + 0x08)?;
    let pcount = res.u8(s + 0x10)? as u64;
    let types = (0..pcount).map(|p| res.u8(params + p * 16).map(u64::from)).collect::<Result<Vec<u64>, _>>()?;
    let data: u64 = types.iter().sum::<u64>() * 16;
    let hashes = params + pcount * 16 + data;
    let mut refs = TextureRefs::new();
    for (p, &t) in types.iter().enumerate() {
        let p = p as u64;
        if t == 0
            && let Some(ptr) = res.u64(params + p * 16 + 8).ok().filter(|&ptr| ptr != 0)
        {
            refs.push((res.u32(hashes + p * 4).unwrap_or(0), ptr));
        }
    }
    Ok((name_hash, refs))
}

fn read_shader(res: &Resource, s: u64, layout_gen: Gen) -> Result<Shader, ResourceError> {
    let (name_hash, refs) = if layout_gen.sga() { read_sga_shader(res, s)? } else { read_legacy_shader(res, s)? };
    let mut shader = Shader { name_hash, ..Default::default() };
    for (hash, ptr) in refs {
        if let Some(name) = texture_name(res, ptr) {
            shader.samplers.push((hash, name.clone()));
            if !shader.textures.contains(&name) {
                shader.textures.push(name);
            }
        }
    }
    Ok(shader)
}

fn read_shaders(res: &Resource, group: u64, layout_gen: Gen) -> Result<Vec<Shader>, ResourceError> {
    let list = res.u64(group + 0x10)?;
    let count = res.u16(group + 0x18)? as u64;
    (0..count.min(MAX_SHADERS)).map(|i| read_shader(res, res.u64(list + i * 8)?, layout_gen)).collect()
}

/// Where a skeleton and its bones keep what we read.
struct BoneLayout {
    /// Pointer to the bones list, in the skeleton.
    list: u64,
    /// Name pointer, tag and parent, in a bone.
    name: u64,
    tag: u64,
    parent: u64,
}

const BONES_GTA5: BoneLayout = BoneLayout { list: 0x20, name: 0x38, tag: 0x44, parent: 0x32 };
const BONES_RDR2: BoneLayout = BoneLayout { list: 0x28, name: 0x30, tag: 0x4A, parent: 0x46 };

fn read_bone_gen(res: &Resource, b: u64, layout: &BoneLayout) -> Result<Bone, ResourceError> {
    let name_ptr = res.u64(b + layout.name)?;
    Ok(Bone {
        name: if name_ptr != 0 { res.string(name_ptr).unwrap_or_default() } else { String::new() },
        tag: res.u16(b + layout.tag)?,
        parent: res.u16(b + layout.parent)? as i16,
        rotation: floats(res, b)?,
        translation: floats(res, b + 0x10)?,
        scale: floats(res, b + 0x20)?,
    })
}

/// A skeleton: GTA V's bones list is at 0x20 (parent at bone+0x32, tag at
/// +0x44); RDR2's at 0x28 (parent at +0x46, tag at +0x4A). Both keep the
/// bone count at 0x5E and 0x50-byte bones with rotation, translation,
/// scale and name pointer first.
fn read_skeleton_gen(res: &Resource, ptr: u64, layout_gen: Gen) -> Result<Skeleton, ResourceError> {
    let layout = if layout_gen == Gen::Rdr2 { &BONES_RDR2 } else { &BONES_GTA5 };
    let bones = res.u64(ptr + layout.list)?;
    let count = res.u16(ptr + 0x5E)? as u64;
    if bones == 0 || count > 2048 {
        return Err(bad("invalid skeleton"));
    }
    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count {
        out.push(read_bone_gen(res, bones + i * 0x50, layout)?);
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

/// Adds the drawables of the parts (wheels, doors, …) of a GTA V `.yft` that carry their own.
fn push_part_drawables(res: &Resource, drawables: &mut Vec<u64>) {
    if let Ok(group) = res.u64(Resource::ROOT + 0xF0)
        && group != 0
        && let Some(lod) = (0..3).filter_map(|i| res.u64(group + 0x10 + i * 8).ok()).find(|&p| p != 0)
        && let (Ok(list), Ok(count)) = (res.u64(lod + 0xD0), res.u8(lod + 0x11E))
    {
        for i in 0..count as u64 {
            if let Ok(child) = res.u64(list + i * 8)
                && child != 0
                && let Ok(d) = res.u64(child + 0xA0)
            {
                drawables.push(d);
            }
        }
    }
}

/// The drawables of a GTA V model (`ext`: `ydr`, `yft` with its parts, or `ydd`).
fn model_drawables(res: &Resource, ext: &str) -> Result<Vec<u64>, ResourceError> {
    let root = Resource::ROOT;
    let mut drawables = Vec::new();
    match ext.to_ascii_lowercase().as_str() {
        "ydr" => drawables.push(root),
        "yft" => {
            drawables.push(res.u64(root + 0x30)?);
            // Parts (wheels, doors, …) can carry their own.
            push_part_drawables(res, &mut drawables);
        }
        "ydd" => {
            let list = res.u64(root + 0x30)?;
            for i in 0..res.u16(root + 0x38)? as u64 {
                drawables.push(res.u64(list + i * 8)?);
            }
        }
        other => return Err(bad(format!("{other} files have no drawables"))),
    }
    Ok(drawables)
}

/// The texture dictionary of the drawable at `d`, if it has one.
fn texture_dict_of(res: &Resource, d: u64) -> Result<Option<u64>, ResourceError> {
    let group = res.u64(d + 0x10)?;
    if group == 0 {
        return Ok(None);
    }
    Ok(Some(res.u64(group + 0x08)?).filter(|&txd| txd != 0))
}

/// The texture dictionaries a GTA V model carries (`ext`: `ydr`, `yft`
/// with its parts, or `ydd`), each once, and whether the model is Enhanced.
pub fn embedded_texture_dicts(res: &Resource, ext: &str) -> Result<(bool, Vec<u64>), ResourceError> {
    let enhanced = is_enhanced(res.version)?;
    let drawables = model_drawables(res, ext)?;
    let mut dicts = Vec::new();
    for d in drawables.into_iter().filter(|&d| d != 0) {
        if let Some(txd) = texture_dict_of(res, d)?
            && !dicts.contains(&txd)
        {
            dicts.push(txd);
        }
    }
    Ok((enhanced, dicts))
}

/// A `.ydr` file.
pub fn parse_ydr(res: &Resource) -> Result<Drawable, ResourceError> {
    read_drawable(res, Resource::ROOT, is_enhanced(res.version)?)
}

/// A `.yft` file: its main drawable.
pub fn parse_yft(res: &Resource) -> Result<Drawable, ResourceError> {
    Ok(parse_fragment(res)?.drawable)
}

/// A Red Dead Redemption 2 `.yft`: its main drawable (at `0x20` of the
/// fragment; found among the root's pointers if not) and the children with
/// meshes of their own (wheels, see [`read_fragment_children_rdr2`]).
pub fn parse_fragment_rdr2(res: &Resource) -> Result<Fragment, ResourceError> {
    let body = read_drawable_gen(res, fragment_drawable_rdr2(res)?, Gen::Rdr2)?;
    // Cloth and children are optional extras, as in GTA V.
    let (cloth_ptrs, cloths) = read_cloths_rdr2(res, &body).unwrap_or_default();
    let children = read_fragment_children_rdr2(res, &body, &cloth_ptrs).unwrap_or_default();
    Ok(Fragment { drawable: body, children, cloths })
}

/// The cloth of an RDR2 fragment (flags, straps, awnings), in its rest pose:
/// a list at `0x48` of the fragment (16-byte entries, `u16` count at
/// `0x50`), each cloth with its drawable at `0x18`. A cloth drawable without
/// shaders of its own uses the body's. Returns the drawables' addresses too
/// (a cloth's drawable can also be a child of the physics LOD).
fn read_cloths_rdr2(res: &Resource, body: &Drawable) -> Result<(Vec<u64>, Vec<Drawable>), ResourceError> {
    let ok = |p: u64| (0x5000_0000..0x6000_0000).contains(&p);
    let list = res.u64(Resource::ROOT + 0x48)?;
    if !ok(list) {
        return Ok(Default::default());
    }
    let (mut ptrs, mut out) = (Vec::new(), Vec::new());
    for i in 0..res.u16(Resource::ROOT + 0x50)?.min(64) as u64 {
        let cloth = res.u64(list + i * 16)?;
        let ptr = if ok(cloth) { res.u64(cloth + 0x18)? } else { 0 };
        if !ok(ptr) || ptrs.contains(&ptr) {
            continue;
        }
        let mut drawable = read_drawable_gen(res, ptr, Gen::Rdr2)?;
        if drawable.shaders.is_empty() {
            drawable.shaders = body.shaders.clone();
        }
        ptrs.push(ptr);
        if !drawable.meshes.is_empty() {
            out.push(drawable);
        }
    }
    Ok((ptrs, out))
}

/// Only the skeleton of an RDR2 `.yft` (much faster than reading the meshes).
pub fn fragment_skeleton_rdr2(res: &Resource) -> Result<Option<Skeleton>, ResourceError> {
    let skeleton = res.u64(fragment_drawable_rdr2(res)? + 0x18)?;
    Ok(if skeleton != 0 { Some(read_skeleton_gen(res, skeleton, Gen::Rdr2)?) } else { None })
}

/// The main drawable of an RDR2 fragment: at `0x20`, else the first of the
/// root's pointers that looks like one.
fn fragment_drawable_rdr2(res: &Resource) -> Result<u64, ResourceError> {
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
    let found = candidates.filter_map(|o| res.u64(root + o).ok()).find(|&p| looks_like_drawable(p));
    // A fragment that is all cloth (a strap): the body has shaders and a
    // skeleton but no meshes of its own.
    let ok = |q: u64| (0x5000_0000..0x6000_0000).contains(&q);
    let body_of_cloth = || {
        let p = res.u64(root + 0x20).ok().filter(|&p| ok(p))?;
        (res.u64(p + 0x10).is_ok_and(ok) && res.u64(root + 0x48).is_ok_and(ok)).then_some(p)
    };
    found.or_else(body_of_cloth).ok_or_else(|| bad("fragment without a drawable"))
}

/// The children of an RDR2 fragment that have meshes. The physics LOD group
/// is at `0x90` of the fragment, its first LOD at `0x10`. Unlike GTA V's, a
/// child is a drawable itself (list at `0x118` of the LOD, `u8` count at
/// `0x146`) without a shader group: its meshes use the body's shaders. Its
/// bone is named by its group (group of each child: `u16` list at `0x110`;
/// groups at `0xC8`, with the parent group at `+0x06` and the name at
/// `+0x4F`), and a group without a bone of that name sits on its parent's.
fn read_fragment_children_rdr2(
    res: &Resource,
    body: &Drawable,
    cloths: &[u64],
) -> Result<Vec<FragmentChild>, ResourceError> {
    let Some(lod) = Rdr2ChildLod::read(res, body.skeleton.as_ref())? else { return Ok(Vec::new()) };
    let mut seen = cloths.to_vec();
    let mut out = Vec::new();
    for i in 0..lod.count {
        let ptr = res.u64(lod.list + i * 8)?;
        if !is_rdr2_pointer(ptr) || seen.contains(&ptr) {
            continue;
        }
        seen.push(ptr);
        if let Some(child) = lod.read_child(body, i, ptr)? {
            out.push(child);
        }
    }
    Ok(out)
}

/// Whether `p` points into the system part of an RDR2 resource.
fn is_rdr2_pointer(p: u64) -> bool {
    (0x5000_0000..0x6000_0000).contains(&p)
}

/// The first physics LOD of an RDR2 fragment: its drawables and the groups that name their bones.
struct Rdr2ChildLod<'a> {
    res: &'a Resource,
    skeleton: Option<&'a Skeleton>,
    groups: u64,
    group_count: u64,
    group_of: u64,
    list: u64,
    count: u64,
}

impl<'a> Rdr2ChildLod<'a> {
    /// The first LOD of the physics LOD group; `None` if the fragment has none.
    fn first_lod(res: &Resource) -> Result<Option<u64>, ResourceError> {
        let group = res.u64(Resource::ROOT + 0x90)?;
        if !is_rdr2_pointer(group) {
            return Ok(None);
        }
        Ok((0..3).map(|i| res.u64(group + 0x10 + i * 8)).find_map(|p| p.ok().filter(|&p| is_rdr2_pointer(p))))
    }

    /// The groups and drawables lists of the LOD at `lod`.
    fn at(res: &'a Resource, skeleton: Option<&'a Skeleton>, lod: u64) -> Result<Self, ResourceError> {
        let (groups, group_of, list) = (res.u64(lod + 0xC8)?, res.u64(lod + 0x110)?, res.u64(lod + 0x118)?);
        let (group_count, count) = (res.u8(lod + 0x140)? as u64, res.u8(lod + 0x146)? as u64);
        Ok(Self { res, skeleton, groups, group_count, group_of, list, count })
    }

    /// `None` if the fragment has no such LOD or its lists do not point into the resource.
    fn read(res: &'a Resource, skeleton: Option<&'a Skeleton>) -> Result<Option<Self>, ResourceError> {
        let Some(lod) = Self::first_lod(res)? else { return Ok(None) };
        let this = Self::at(res, skeleton, lod)?;
        let lists = [this.groups, this.group_of, this.list];
        Ok(lists.into_iter().all(is_rdr2_pointer).then_some(this))
    }

    /// The bone of group `g`: by its name, else its parent's (at most 64 steps up).
    fn bone_of(&self, mut g: u64) -> Option<u16> {
        let skeleton = self.skeleton?;
        for _ in 0..64 {
            if g >= self.group_count {
                return None;
            }
            let at = self.res.u64(self.groups + g * 8).ok().filter(|&p| is_rdr2_pointer(p))?;
            let name = self.res.string(at + 0x4F).ok()?;
            if let Some(b) = skeleton.find_name(&name) {
                return Some(skeleton.bones[b].tag);
            }
            g = self.res.u8(at + 0x06).ok().filter(|&p| p != 0xFF)? as u64;
        }
        None
    }

    /// Child `i`, whose drawable is at `ptr`; `None` if it has no meshes.
    fn read_child(&self, body: &Drawable, i: u64, ptr: u64) -> Result<Option<FragmentChild>, ResourceError> {
        let mut drawable = read_drawable_gen(self.res, ptr, Gen::Rdr2)?;
        if drawable.meshes.is_empty() {
            return Ok(None);
        }
        if drawable.shaders.is_empty() {
            drawable.shaders = body.shaders.clone();
        }
        let g = self.res.u16(self.group_of + i * 2)? as u64;
        let first_bone = || self.skeleton.and_then(|s| s.bones.first()).map(|b| b.tag);
        let bone_tag = self.bone_of(g).or_else(first_bone).unwrap_or(0);
        Ok(Some(FragmentChild { bone_tag, drawable }))
    }
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
    Ok(Fragment { drawable: d, children, cloths: Vec::new() })
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
        // RDR2 palette shaders: the shade in lyr0DiffuseTex, not the palette.
        let pal = s(&[
            (param::TINT_PALETTE_TEX, "w_repeater_cloth_strap01_pal"),
            (param::LYR0_DIFFUSE_TEX, "w_repeater_cloth_strap01_ab"),
        ]);
        assert_eq!(pal.base_texture(), Some("w_repeater_cloth_strap01_ab"));
    }

    #[test]
    fn half_floats() {
        let h = |bits: u16| Encoding::Half2.decode(&bits.to_le_bytes())[0];
        assert_eq!(h(0x3C00), 1.0);
        assert_eq!(h(0xC000), -2.0);
        assert_eq!(h(0x0000), 0.0);
        assert!((h(0x3555) - 0.333_25).abs() < 1e-4);
        // A short slice must not panic.
        assert_eq!(Encoding::Float4.decode(&[0, 0, 0x80, 0x3F]), [1.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn packed_normals() {
        let v: u32 = 511 | ((1024 - 511) << 20);
        let n = Encoding::R10G10B10A2.decode(&v.to_le_bytes());
        assert_eq!([n[0], n[1], n[2]], [1.0, 0.0, -1.0]);
    }
}
